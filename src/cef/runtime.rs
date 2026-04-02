#![allow(dead_code)]

use std::ffi::{CString, c_char};
use std::mem;
use std::path::Path;

use anyhow::{Result, anyhow, bail};

#[cfg(has_cef_sdk)]
use crate::cef::app::{CefAppState, stash_pending_app, take_pending_app};
use crate::cef::bootstrap::CefRuntimePlan;
use crate::desktop_config;
use crate::language;
#[cfg(has_cef_sdk)]
use crate::cef::ffi::{
    cef_api_hash, cef_api_version, cef_do_message_loop_work, cef_execute_process, cef_initialize,
    cef_main_args_t, cef_settings_t, cef_shutdown,
};
#[cfg(not(has_cef_sdk))]
use crate::cef::ffi::{cef_main_args_t, cef_settings_t};
use crate::cef::strings::CefOwnedString;

const NEKO_CEF_API_VERSION: i32 = 14500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CefProcessRole {
    Browser,
    Subprocess,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CefMessageLoopMode {
    MultiThreaded,
    ExternalPump,
}

#[derive(Debug)]
pub struct CefMainArgs {
    cstrings: Vec<CString>,
    argv: Vec<*mut c_char>,
    raw: cef_main_args_t,
}

impl CefMainArgs {
    pub fn from_env() -> Result<Self> {
        let args = std::env::args_os()
            .map(|arg| CString::new(arg.to_string_lossy().as_bytes().to_vec()))
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|err| anyhow!("failed to convert argv into C strings: {err}"))?;

        let mut argv = args
            .iter()
            .map(|value| value.as_ptr() as *mut c_char)
            .collect::<Vec<_>>();

        let raw = cef_main_args_t {
            argc: argv.len() as i32,
            argv: argv.as_mut_ptr(),
        };

        Ok(Self {
            cstrings: args,
            argv,
            raw,
        })
    }

    pub fn raw(&self) -> &cef_main_args_t {
        &self.raw
    }
}

#[derive(Debug)]
pub struct CefSettingsBuilder {
    browser_subprocess_path: CefOwnedString,
    resources_dir_path: CefOwnedString,
    locales_dir_path: CefOwnedString,
    cache_path: CefOwnedString,
    root_cache_path: CefOwnedString,
    locale: CefOwnedString,
    background_color: u32,
    windowless_rendering_enabled: bool,
    multi_threaded_message_loop: bool,
    no_sandbox: bool,
    remote_debugging_port: i32,
}

impl CefSettingsBuilder {
    pub fn from_runtime_plan(plan: &CefRuntimePlan) -> Self {
        let compat_c_probe = matches!(
            std::env::var("NEKO_CEF_COMPAT_C_PROBE").ok().as_deref(),
            Some("1") | Some("true") | Some("TRUE") | Some("yes") | Some("YES")
        );
        let cache_root = default_cache_root(plan);
        Self {
            browser_subprocess_path: if compat_c_probe {
                CefOwnedString::new("")
            } else {
                CefOwnedString::new(plan.subprocess_executable.to_string_lossy())
            },
            resources_dir_path: CefOwnedString::new(plan.resources_dir.to_string_lossy()),
            locales_dir_path: CefOwnedString::new(plan.locales_dir.to_string_lossy()),
            cache_path: CefOwnedString::new(cache_root.join("cache").to_string_lossy()),
            root_cache_path: CefOwnedString::new(cache_root.to_string_lossy()),
            locale: CefOwnedString::new(if compat_c_probe {
                "en-US".to_string()
            } else {
                language::default_cef_locale()
            }),
            background_color: 0,
            windowless_rendering_enabled: true,
            multi_threaded_message_loop: false,
            no_sandbox: true,
            remote_debugging_port: parse_remote_debugging_port().unwrap_or(0),
        }
    }

    pub fn build(self) -> cef_settings_t {
        cef_settings_t {
            size: mem::size_of::<cef_settings_t>(),
            no_sandbox: self.no_sandbox as i32,
            browser_subprocess_path: self.browser_subprocess_path.into_raw(),
            framework_dir_path: CefOwnedString::new("").into_raw(),
            main_bundle_path: CefOwnedString::new("").into_raw(),
            multi_threaded_message_loop: self.multi_threaded_message_loop as i32,
            external_message_pump: 0,
            windowless_rendering_enabled: self.windowless_rendering_enabled as i32,
            command_line_args_disabled: 0,
            cache_path: self.cache_path.into_raw(),
            root_cache_path: self.root_cache_path.into_raw(),
            persist_session_cookies: 1,
            user_agent: CefOwnedString::new("").into_raw(),
            user_agent_product: CefOwnedString::new("").into_raw(),
            locale: self.locale.into_raw(),
            log_file: CefOwnedString::new("").into_raw(),
            log_severity: 0,
            log_items: 0,
            javascript_flags: CefOwnedString::new("").into_raw(),
            resources_dir_path: self.resources_dir_path.into_raw(),
            locales_dir_path: self.locales_dir_path.into_raw(),
            remote_debugging_port: self.remote_debugging_port,
            uncaught_exception_stack_size: 0,
            background_color: self.background_color,
            accept_language_list: CefOwnedString::new("").into_raw(),
            cookieable_schemes_list: CefOwnedString::new("").into_raw(),
            cookieable_schemes_exclude_defaults: 0,
            chrome_policy_id: CefOwnedString::new("").into_raw(),
            chrome_app_icon_id: 0,
            disable_signal_handlers: 1,
        }
    }
}

fn default_cache_root(plan: &CefRuntimePlan) -> std::path::PathBuf {
    if let Ok(value) = std::env::var("NEKO_CEF_CACHE_ROOT") {
        return std::path::PathBuf::from(value);
    }

    let cache_root = desktop_config::cef_profile_root();
    if let Err(err) = std::fs::create_dir_all(&cache_root) {
        eprintln!(
            "CEF cache root creation failed at {}: {err}; falling back to SDK-local profile",
            cache_root.display()
        );
        return plan.sdk.root.join("neko-cef-profile");
    }
    cache_root
}

pub fn detect_process_role() -> CefProcessRole {
    let is_subprocess = std::env::args()
        .skip(1)
        .any(|arg| arg.starts_with("--type=") && arg != "--type=");
    if is_subprocess {
        CefProcessRole::Subprocess
    } else {
        CefProcessRole::Browser
    }
}

#[derive(Debug)]
pub struct CefRuntimeSession {
    plan: CefRuntimePlan,
    initialized: bool,
    message_loop_mode: CefMessageLoopMode,
}

impl CefRuntimeSession {
    #[cfg(not(has_cef_sdk))]
    pub fn initialize() -> Result<Self> {
        bail!(
            "cannot initialize CEF runtime because no local CEF SDK was discovered; set NEKO_CEF_SDK_DIR first"
        )
    }

    #[cfg(has_cef_sdk)]
    pub fn initialize() -> Result<Self> {
        ensure_api_version()?;
        let plan = CefRuntimePlan::discover()?;
        let args = CefMainArgs::from_env()?;
        let settings = CefSettingsBuilder::from_runtime_plan(&plan).build();
        let use_null_app = matches!(
            std::env::var("NEKO_CEF_USE_NULL_APP").ok().as_deref(),
            Some("1") | Some("true") | Some("TRUE") | Some("yes") | Some("YES")
        );
        let app = if use_null_app {
            None
        } else {
            Some(take_pending_app().unwrap_or(CefAppState::new()?))
        };

        eprintln!(
            "CEF initialize: subprocess={} resources={} locales={} argv_len={} remote_debugging_port={} use_null_app={}",
            plan.subprocess_executable.display(),
            plan.resources_dir.display(),
            plan.locales_dir.display(),
            args.raw().argc,
            settings.remote_debugging_port,
            use_null_app
        );

        let result = unsafe {
            cef_initialize(
                args.raw(),
                &settings,
                app.as_ref()
                    .map(|app| app.raw_ptr())
                    .unwrap_or(std::ptr::null_mut()),
                std::ptr::null_mut(),
            )
        };

        eprintln!("CEF initialize: cef_initialize returned {result}");

        if result == 0 {
            bail!("cef_initialize returned 0");
        }

        Ok(Self {
            plan,
            initialized: true,
            message_loop_mode: if settings.multi_threaded_message_loop != 0 {
                CefMessageLoopMode::MultiThreaded
            } else {
                CefMessageLoopMode::ExternalPump
            },
        })
    }

    #[cfg(not(has_cef_sdk))]
    pub fn execute_subprocess() -> Result<Option<i32>> {
        Ok(None)
    }

    #[cfg(has_cef_sdk)]
    pub fn execute_subprocess() -> Result<Option<i32>> {
        ensure_api_version()?;
        let args = CefMainArgs::from_env()?;
        let app = CefAppState::new()?;
        app.add_ref_for_cef();
        eprintln!("CEF execute_subprocess: argv_len={}", args.raw().argc);
        let code = unsafe { cef_execute_process(args.raw(), app.raw_ptr(), std::ptr::null_mut()) };
        eprintln!("CEF execute_subprocess: returned {code}");
        if code >= 0 {
            app.release_for_cef();
            Ok(Some(code))
        } else {
            stash_pending_app(app);
            Ok(None)
        }
    }

    pub fn do_message_loop_work(&self) {
        #[cfg(has_cef_sdk)]
        unsafe {
            cef_do_message_loop_work();
        }
    }

    pub fn sdk_root(&self) -> &Path {
        &self.plan.sdk.root
    }

    pub fn message_loop_mode(&self) -> CefMessageLoopMode {
        self.message_loop_mode
    }
}

fn parse_remote_debugging_port() -> Option<i32> {
    let raw = std::env::var("NEKO_CEF_REMOTE_DEBUGGING_PORT").ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    trimmed.parse::<i32>().ok().filter(|port| *port > 0)
}

#[cfg(has_cef_sdk)]
fn ensure_api_version() -> Result<()> {
    let hash = unsafe { cef_api_hash(NEKO_CEF_API_VERSION, 0) };
    if hash.is_null() {
        bail!("cef_api_hash returned null for version {NEKO_CEF_API_VERSION}");
    }

    let configured = unsafe { cef_api_version() };
    if configured != NEKO_CEF_API_VERSION {
        bail!(
            "CEF configured unexpected API version {configured}, expected {}",
            NEKO_CEF_API_VERSION
        );
    }

    Ok(())
}

impl Drop for CefRuntimeSession {
    fn drop(&mut self) {
        if self.initialized {
            #[cfg(has_cef_sdk)]
            unsafe {
                cef_shutdown();
            }
        }
    }
}
