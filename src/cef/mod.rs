use anyhow::{Result, anyhow, bail};

use crate::wayland::raw_host::{RawHostHandle, RawHostPointerButton};

#[cfg(feature = "cef_osr")]
#[cfg(has_cef_sdk)]
mod app;
#[cfg(feature = "cef_osr")]
mod bootstrap;
#[cfg(feature = "cef_osr")]
mod browser;
#[cfg(feature = "cef_osr")]
mod c_bridge;
#[cfg(feature = "cef_osr")]
mod c_shim;
#[cfg(feature = "cef_osr")]
mod client;
#[cfg(feature = "cef_osr")]
mod events;
#[cfg(feature = "cef_osr")]
mod ffi;
#[cfg(feature = "cef_osr")]
mod input_bridge;
#[cfg(feature = "cef_osr")]
mod log;
#[cfg(feature = "cef_osr")]
mod render_handler;
#[cfg(feature = "cef_osr")]
mod runtime;
#[cfg(feature = "cef_osr")]
mod strings;

#[derive(Debug, Clone)]
pub struct CefOsrConfig {
    pub url: String,
    pub width: u32,
    pub height: u32,
    pub transparent_painting: bool,
    pub frame_rate: u32,
}

impl CefOsrConfig {
    pub fn demo() -> Self {
        Self {
            url: "https://example.com".to_string(),
            width: 800,
            height: 600,
            transparent_painting: true,
            frame_rate: 60,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CefMouseButton {
    Left,
    Middle,
    Right,
}

impl TryFrom<RawHostPointerButton> for CefMouseButton {
    type Error = ();

    fn try_from(value: RawHostPointerButton) -> std::result::Result<Self, Self::Error> {
        match value {
            RawHostPointerButton::Left => Ok(Self::Left),
            RawHostPointerButton::Middle => Ok(Self::Middle),
            RawHostPointerButton::Right => Ok(Self::Right),
            RawHostPointerButton::Other(_) => Err(()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CefKeyEventKind {
    RawKeyDown,
    KeyUp,
    Char,
}

pub struct CefOsrBridge {
    config: CefOsrConfig,
    attached: bool,
    backend: &'static str,
    sdk_root: Option<std::path::PathBuf>,
    #[cfg(feature = "cef_osr")]
    client_state: client::CefClientState,
    #[cfg(feature = "cef_osr")]
    runtime_session: runtime::CefRuntimeSession,
}

impl CefOsrBridge {
    pub fn config(&self) -> &CefOsrConfig {
        &self.config
    }

    pub fn is_attached(&self) -> bool {
        self.attached
    }

    pub fn backend(&self) -> &'static str {
        self.backend
    }

    pub fn sdk_root(&self) -> Option<&std::path::Path> {
        self.sdk_root.as_deref()
    }

    #[cfg(feature = "cef_osr")]
    pub fn message_loop_mode(&self) -> runtime::CefMessageLoopMode {
        self.runtime_session.message_loop_mode()
    }

    #[cfg(feature = "cef_osr")]
    pub fn do_message_loop_work(&self) {
        if matches!(
            self.runtime_session.message_loop_mode(),
            runtime::CefMessageLoopMode::ExternalPump
        ) {
            self.runtime_session.do_message_loop_work();
        }
    }

    #[cfg(feature = "cef_osr")]
    pub fn render_size(&self) -> (u32, u32) {
        let state = self.client_state.render_handler_state();
        (state.width(), state.height())
    }

    #[cfg(feature = "cef_osr")]
    pub fn focus_browser(&self, focused: bool) {
        self.client_state.focus_browser(focused);
    }

    #[cfg(feature = "cef_osr")]
    pub fn notify_resized(&self) {
        self.client_state.notify_resized();
    }

    #[cfg(feature = "cef_osr")]
    pub fn send_mouse_move_event(&self, x: i32, y: i32, mouse_leave: bool, modifiers: u32) {
        self.client_state
            .send_mouse_move_event(x, y, mouse_leave, modifiers);
    }

    #[cfg(feature = "cef_osr")]
    pub fn send_mouse_click_event(
        &self,
        x: i32,
        y: i32,
        button: CefMouseButton,
        mouse_up: bool,
        click_count: i32,
        modifiers: u32,
    ) {
        self.client_state
            .send_mouse_click_event(x, y, button, mouse_up, click_count, modifiers);
    }

    #[cfg(feature = "cef_osr")]
    pub fn send_mouse_wheel_event(
        &self,
        x: i32,
        y: i32,
        delta_x: i32,
        delta_y: i32,
        modifiers: u32,
    ) {
        self.client_state
            .send_mouse_wheel_event(x, y, delta_x, delta_y, modifiers);
    }

    #[cfg(feature = "cef_osr")]
    pub fn send_key_event(
        &self,
        kind: CefKeyEventKind,
        windows_key_code: i32,
        native_key_code: i32,
        modifiers: u32,
        character: u16,
        unmodified_character: u16,
    ) {
        self.client_state.send_key_event(
            kind,
            windows_key_code,
            native_key_code,
            modifiers,
            character,
            unmodified_character,
        );
    }

    #[cfg(feature = "cef_osr")]
    pub fn request_close(&self) {
        self.client_state.request_close();
    }

    #[cfg(feature = "cef_osr")]
    pub fn execute_javascript(&self, code: &str, script_url: &str, start_line: i32) {
        self.client_state
            .execute_javascript(code, script_url, start_line);
    }
}

#[cfg(feature = "cef_osr")]
pub fn try_run_subprocess() -> Result<Option<i32>> {
    #[cfg(not(has_cef_sdk))]
    {
        Ok(None)
    }

    #[cfg(has_cef_sdk)]
    {
        runtime::CefRuntimeSession::execute_subprocess()
    }
}

#[cfg(feature = "cef_osr")]
pub use input_bridge::{RawInputSource, run_multi_raw_input_loop, run_raw_input_loop};
#[cfg(feature = "cef_osr")]
pub use events::{CefLifecycleEvent, clear_event_callback, install_event_callback};
#[cfg(feature = "cef_osr")]
pub use c_bridge::{
    CefCBridge, CefCBridgeConfig, CefCBridgeRuntime, run_raw_input_loop_cbridge,
    try_run_c_subprocess,
};
#[cfg(feature = "cef_osr")]
pub use c_shim::{CShimMode, run_c_probe};

pub fn spawn_osr_bridge(raw_host: RawHostHandle, config: CefOsrConfig) -> Result<CefOsrBridge> {
    if config.width == 0 || config.height == 0 {
        bail!("CEF OSR bridge requires non-zero dimensions");
    }

    if config.frame_rate == 0 {
        bail!("CEF OSR bridge requires a non-zero frame_rate");
    }

    #[cfg(feature = "cef_osr")]
    {
        return spawn_osr_bridge_impl(raw_host, config);
    }

    #[cfg(not(feature = "cef_osr"))]
    {
        let _ = raw_host;
        Err(anyhow!(
            "CEF OSR bridge scaffold is present, but this build does not include a CEF SDK binding yet. Enable the future `cef_osr` integration path after wiring the SDK/runtime."
        ))
    }
}

#[cfg(feature = "cef_osr")]
pub fn abi_sizes() -> Vec<(&'static str, usize)> {
    vec![
        ("cef_string_t", std::mem::size_of::<ffi::cef_string_t>()),
        ("cef_settings_t", std::mem::size_of::<ffi::cef_settings_t>()),
        (
            "cef_window_info_t",
            std::mem::size_of::<ffi::cef_window_info_t>(),
        ),
        (
            "cef_browser_settings_t",
            std::mem::size_of::<ffi::cef_browser_settings_t>(),
        ),
        ("cef_app_t", std::mem::size_of::<ffi::cef_app_t>()),
        ("cef_client_t", std::mem::size_of::<ffi::cef_client_t>()),
        (
            "cef_render_handler_t",
            std::mem::size_of::<ffi::cef_render_handler_t>(),
        ),
    ]
}

#[cfg(feature = "cef_osr")]
fn spawn_osr_bridge_impl(raw_host: RawHostHandle, config: CefOsrConfig) -> Result<CefOsrBridge> {
    #[cfg(not(has_cef_sdk))]
    {
        let _ = raw_host;
        let _ = config;
        return Err(anyhow!(
            "CEF OSR feature is enabled, but no local CEF SDK was discovered. Set NEKO_CEF_SDK_DIR to a Linux CEF binary distribution root that contains include/ and Release/libcef.so"
        ));
    }

    #[cfg(has_cef_sdk)]
    {
        let role = runtime::detect_process_role();
        let runtime_plan = bootstrap::CefRuntimePlan::discover()?;
        eprintln!(
            "CEF spawn_osr_bridge_impl: role={role:?} subprocess={} resources={} locales={}",
            runtime_plan.subprocess_executable.display(),
            runtime_plan.resources_dir.display(),
            runtime_plan.locales_dir.display()
        );
        if let Some(code) = runtime::CefRuntimeSession::execute_subprocess()? {
            return Err(anyhow!(
                "CEF subprocess exited early with code {code}; browser-process OSR path was not entered"
            ));
        }
        let runtime_session = runtime::CefRuntimeSession::initialize()?;
        eprintln!("CEF spawn_osr_bridge_impl: runtime initialized");
        let mut client_state = client::CefClientState::new(
            config.width,
            config.height,
            config.transparent_painting,
            raw_host,
        )?;
        eprintln!(
            "CEF spawn_osr_bridge_impl: client state ready render_size={}x{}",
            config.width, config.height
        );
        browser::create_windowless_browser(
            &mut client_state,
            &config.url,
            config.width,
            config.height,
            config.frame_rate,
            config.transparent_painting,
        )?;
        eprintln!("CEF spawn_osr_bridge_impl: create_windowless_browser returned");

        eprintln!(
            "CEF OSR bridge initialized: role={role:?} sdk_root={} message_loop={:?} url={}",
            runtime_plan.sdk.root.display(),
            runtime_session.message_loop_mode(),
            config.url
        );

        Ok(CefOsrBridge {
            config,
            attached: true,
            backend: "cef_osr",
            sdk_root: Some(runtime_session.sdk_root().to_path_buf()),
            runtime_session,
            client_state,
        })
    }
}
