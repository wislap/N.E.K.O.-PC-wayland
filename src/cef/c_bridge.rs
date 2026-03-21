#![allow(dead_code)]

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_void};
use std::ptr::NonNull;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::thread;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
#[cfg(has_cef_sdk)]
use anyhow::Context;
use xkeysym::key;

use crate::cef::bootstrap::CefRuntimePlan;
use crate::cef::c_shim::{
    NekoCefBrowserHandle, NekoCefKeyEventKind, NekoCefMessageLoopMode, NekoCefMouseButton,
    NekoCefRuntimeHandle,
};
#[cfg(has_cef_sdk)]
use crate::cef::c_shim::{
    NEKO_CEF_BRIDGE_ERROR_CAPACITY, NekoCefBrowserCallbacks, NekoCefBrowserConfig,
    NekoCefRuntimeSettings, neko_cef_bridge_browser_close, neko_cef_bridge_browser_is_ready,
    neko_cef_bridge_browser_notify_resized, neko_cef_bridge_browser_release,
    neko_cef_bridge_browser_send_key_event, neko_cef_bridge_browser_send_mouse_click,
    neko_cef_bridge_browser_send_mouse_move, neko_cef_bridge_browser_send_mouse_wheel,
    neko_cef_bridge_browser_set_focus, neko_cef_bridge_create_browser,
    neko_cef_bridge_do_message_loop_work, neko_cef_bridge_execute_process,
    neko_cef_bridge_initialize, neko_cef_bridge_message_loop_mode, neko_cef_bridge_shutdown,
};
use crate::cef::events::{CefLifecycleEvent, emit as emit_lifecycle_event};
use crate::wayland::raw_host::{
    RawHostFrame, RawHostHandle, RawHostKeyboardEvent, RawHostModifiers, RawHostPointerButton,
    RawHostPointerEvent,
};

#[derive(Debug, Clone)]
pub struct CefCBridgeConfig {
    pub url: String,
    pub width: u32,
    pub height: u32,
    pub transparent_painting: bool,
    pub frame_rate: u32,
}

impl CefCBridgeConfig {
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

struct CallbackState {
    raw_host: RawHostHandle,
}

pub struct CefCBridgeRuntime {
    runtime: NonNull<NekoCefRuntimeHandle>,
    config: CefCBridgeConfig,
    message_loop_mode: NekoCefMessageLoopMode,
}

pub struct CefCBridge {
    runtime: NonNull<NekoCefRuntimeHandle>,
    browser: NonNull<NekoCefBrowserHandle>,
    callback_state: Box<CallbackState>,
    config: CefCBridgeConfig,
    message_loop_mode: NekoCefMessageLoopMode,
}

impl CefCBridgeRuntime {
    #[cfg(not(has_cef_sdk))]
    pub fn initialize(_config: CefCBridgeConfig) -> Result<Self> {
        bail!("cannot initialize C CEF bridge because no local CEF SDK was discovered")
    }

    #[cfg(has_cef_sdk)]
    pub fn initialize(config: CefCBridgeConfig) -> Result<Self> {
        if config.width == 0 || config.height == 0 {
            bail!("C CEF bridge requires non-zero dimensions");
        }
        if config.frame_rate == 0 {
            bail!("C CEF bridge requires non-zero frame rate");
        }

        let runtime_plan = CefRuntimePlan::discover()?;
        ensure_runtime_library_path(&runtime_plan);
        let args = collect_main_args()?;
        let minimal_settings = matches!(
            std::env::var("NEKO_CEF_MINIMAL_SETTINGS").ok().as_deref(),
            Some("1") | Some("true") | Some("TRUE") | Some("yes") | Some("YES")
        );

        let cache_root = runtime_plan
            .sdk
            .root
            .join(format!("neko-cef-c-bridge-{}", std::process::id()));
        std::fs::create_dir_all(&cache_root).with_context(|| {
            format!("failed to create CEF cache root {}", cache_root.display())
        })?;
        let cache_dir = cache_root.join("cache");
        std::fs::create_dir_all(&cache_dir)
            .with_context(|| format!("failed to create CEF cache dir {}", cache_dir.display()))?;

        let force_self_subprocess = matches!(
            std::env::var("NEKO_CEF_FORCE_SELF_SUBPROCESS").ok().as_deref(),
            Some("1") | Some("true") | Some("TRUE") | Some("yes") | Some("YES")
        );
        let subprocess_path = if force_self_subprocess {
            CString::new(Vec::<u8>::new())?
        } else {
            CString::new(runtime_plan.subprocess_executable.to_string_lossy().as_bytes())?
        };
        let resources_dir = CString::new(runtime_plan.resources_dir.to_string_lossy().as_bytes())?;
        let locales_dir = CString::new(runtime_plan.locales_dir.to_string_lossy().as_bytes())?;
        let locale = CString::new(default_cef_locale().unwrap_or_else(|| "en-US".to_string()))?;
        let cache_path = CString::new(cache_dir.to_string_lossy().as_bytes())?;
        let root_cache_path = CString::new(cache_root.to_string_lossy().as_bytes())?;

        let runtime_settings = NekoCefRuntimeSettings {
            browser_subprocess_path: if force_self_subprocess {
                std::ptr::null()
            } else {
                subprocess_path.as_ptr()
            },
            resources_dir_path: if minimal_settings {
                std::ptr::null()
            } else {
                resources_dir.as_ptr()
            },
            locales_dir_path: if minimal_settings {
                std::ptr::null()
            } else {
                locales_dir.as_ptr()
            },
            locale: if minimal_settings {
                std::ptr::null()
            } else {
                locale.as_ptr()
            },
            cache_path: if minimal_settings {
                std::ptr::null()
            } else {
                cache_path.as_ptr()
            },
            root_cache_path: if minimal_settings {
                std::ptr::null()
            } else {
                root_cache_path.as_ptr()
            },
            no_sandbox: 1,
            multi_threaded_message_loop: 0,
            windowless_rendering_enabled: 1,
            external_message_pump: 0,
            remote_debugging_port: 0,
            use_app: 1,
        };

        let mut error = vec![0_i8; NEKO_CEF_BRIDGE_ERROR_CAPACITY];
        let runtime = unsafe {
            neko_cef_bridge_initialize(
                args.argc,
                args.argv.as_ptr() as *mut *mut c_char,
                &runtime_settings,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        let runtime =
            NonNull::new(runtime).ok_or_else(|| anyhow!("{}", error_buffer_to_string(&error)))?;

        let message_loop_mode = match unsafe { neko_cef_bridge_message_loop_mode(runtime.as_ptr()) }
        {
            1 => NekoCefMessageLoopMode::MultiThreaded,
            _ => NekoCefMessageLoopMode::ExternalPump,
        };

        Ok(Self {
            runtime,
            config,
            message_loop_mode,
        })
    }

    pub fn config(&self) -> &CefCBridgeConfig {
        &self.config
    }

    pub fn backend(&self) -> &'static str {
        "c_shim"
    }

    pub fn message_loop_mode(&self) -> NekoCefMessageLoopMode {
        self.message_loop_mode
    }

    #[cfg(not(has_cef_sdk))]
    pub fn do_message_loop_work(&self) {}

    #[cfg(has_cef_sdk)]
    pub fn do_message_loop_work(&self) {
        if matches!(self.message_loop_mode, NekoCefMessageLoopMode::ExternalPump) {
            unsafe { neko_cef_bridge_do_message_loop_work(self.runtime.as_ptr()) };
        }
    }

    #[cfg(not(has_cef_sdk))]
    pub fn attach_browser(self, _raw_host: RawHostHandle) -> Result<CefCBridge> {
        bail!("cannot attach C CEF browser because no local CEF SDK was discovered")
    }

    #[cfg(has_cef_sdk)]
    pub fn attach_browser(self, raw_host: RawHostHandle) -> Result<CefCBridge> {
        let runtime = self.runtime;
        let config = self.config.clone();
        let message_loop_mode = self.message_loop_mode;
        let mut callback_state = Box::new(CallbackState { raw_host });
        let url = CString::new(config.url.as_bytes())?;
        let window_name = CString::new("neko-cef-c-bridge")?;
        let browser_config = NekoCefBrowserConfig {
            url: url.as_ptr(),
            window_name: window_name.as_ptr(),
            width: config.width as c_int,
            height: config.height as c_int,
            frame_rate: config.frame_rate as c_int,
            transparent_painting: config.transparent_painting as c_int,
        };
        let callbacks = NekoCefBrowserCallbacks {
            on_after_created: Some(on_after_created),
            on_before_close: Some(on_before_close),
            on_loading_state_change: Some(on_loading_state_change),
            on_load_start: Some(on_load_start),
            on_load_end: Some(on_load_end),
            on_load_error: Some(on_load_error),
            on_console: Some(on_console),
            on_paint: Some(on_paint),
        };
        let mut error = vec![0_i8; NEKO_CEF_BRIDGE_ERROR_CAPACITY];
        let browser = unsafe {
            neko_cef_bridge_create_browser(
                runtime.as_ptr(),
                &browser_config,
                &callbacks,
                (&mut *callback_state) as *mut CallbackState as *mut c_void,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        let browser = match NonNull::new(browser) {
            Some(browser) => browser,
            None => {
                unsafe { neko_cef_bridge_shutdown(runtime.as_ptr()) };
                bail!("{}", error_buffer_to_string(&error));
            }
        };
        std::mem::forget(self);

        Ok(CefCBridge {
            runtime,
            browser,
            callback_state,
            config,
            message_loop_mode,
        })
    }
}

impl Drop for CefCBridgeRuntime {
    #[cfg(not(has_cef_sdk))]
    fn drop(&mut self) {}

    #[cfg(has_cef_sdk)]
    fn drop(&mut self) {
        unsafe {
            neko_cef_bridge_shutdown(self.runtime.as_ptr());
        }
    }
}

impl CefCBridge {
    pub fn initialize(config: CefCBridgeConfig) -> Result<CefCBridgeRuntime> {
        CefCBridgeRuntime::initialize(config)
    }

    pub fn attach(raw_host: RawHostHandle, config: CefCBridgeConfig) -> Result<Self> {
        CefCBridgeRuntime::initialize(config)?.attach_browser(raw_host)
    }

    pub fn config(&self) -> &CefCBridgeConfig {
        &self.config
    }

    pub fn backend(&self) -> &'static str {
        "c_shim"
    }

    pub fn message_loop_mode(&self) -> NekoCefMessageLoopMode {
        self.message_loop_mode
    }

    #[cfg(not(has_cef_sdk))]
    pub fn do_message_loop_work(&self) {}

    #[cfg(has_cef_sdk)]
    pub fn do_message_loop_work(&self) {
        if matches!(self.message_loop_mode, NekoCefMessageLoopMode::ExternalPump) {
            unsafe { neko_cef_bridge_do_message_loop_work(self.runtime.as_ptr()) };
        }
    }

    #[cfg(not(has_cef_sdk))]
    pub fn focus_browser(&self, _focused: bool) {}

    #[cfg(has_cef_sdk)]
    pub fn focus_browser(&self, focused: bool) {
        unsafe { neko_cef_bridge_browser_set_focus(self.browser.as_ptr(), focused as c_int) };
    }

    #[cfg(not(has_cef_sdk))]
    pub fn notify_resized(&self) {}

    #[cfg(has_cef_sdk)]
    pub fn notify_resized(&self) {
        unsafe {
            neko_cef_bridge_browser_notify_resized(
                self.browser.as_ptr(),
                self.config.width as c_int,
                self.config.height as c_int,
            )
        };
    }

    #[cfg(not(has_cef_sdk))]
    pub fn send_mouse_move_event(&self, _x: i32, _y: i32, _mouse_leave: bool, _modifiers: u32) {}

    #[cfg(has_cef_sdk)]
    pub fn send_mouse_move_event(&self, x: i32, y: i32, mouse_leave: bool, modifiers: u32) {
        unsafe {
            neko_cef_bridge_browser_send_mouse_move(
                self.browser.as_ptr(),
                x,
                y,
                mouse_leave as c_int,
                modifiers,
            );
        }
    }

    #[cfg(not(has_cef_sdk))]
    pub fn send_mouse_click_event(
        &self,
        _x: i32,
        _y: i32,
        _button: NekoCefMouseButton,
        _mouse_up: bool,
        _click_count: i32,
        _modifiers: u32,
    ) {
    }

    #[cfg(has_cef_sdk)]
    pub fn send_mouse_click_event(
        &self,
        x: i32,
        y: i32,
        button: NekoCefMouseButton,
        mouse_up: bool,
        click_count: i32,
        modifiers: u32,
    ) {
        unsafe {
            neko_cef_bridge_browser_send_mouse_click(
                self.browser.as_ptr(),
                x,
                y,
                button as c_int,
                mouse_up as c_int,
                click_count,
                modifiers,
            );
        }
    }

    #[cfg(not(has_cef_sdk))]
    pub fn send_mouse_wheel_event(
        &self,
        _x: i32,
        _y: i32,
        _delta_x: i32,
        _delta_y: i32,
        _modifiers: u32,
    ) {
    }

    #[cfg(has_cef_sdk)]
    pub fn send_mouse_wheel_event(
        &self,
        x: i32,
        y: i32,
        delta_x: i32,
        delta_y: i32,
        modifiers: u32,
    ) {
        unsafe {
            neko_cef_bridge_browser_send_mouse_wheel(
                self.browser.as_ptr(),
                x,
                y,
                delta_x,
                delta_y,
                modifiers,
            );
        }
    }

    #[cfg(not(has_cef_sdk))]
    pub fn send_key_event(
        &self,
        _kind: NekoCefKeyEventKind,
        _windows_key_code: i32,
        _native_key_code: i32,
        _modifiers: u32,
        _character: u16,
        _unmodified_character: u16,
    ) {
    }

    #[cfg(has_cef_sdk)]
    pub fn send_key_event(
        &self,
        kind: NekoCefKeyEventKind,
        windows_key_code: i32,
        native_key_code: i32,
        modifiers: u32,
        character: u16,
        unmodified_character: u16,
    ) {
        unsafe {
            neko_cef_bridge_browser_send_key_event(
                self.browser.as_ptr(),
                kind as c_int,
                windows_key_code,
                native_key_code,
                modifiers,
                character,
                unmodified_character,
            );
        }
    }

    #[cfg(not(has_cef_sdk))]
    pub fn request_close(&self) {}

    #[cfg(has_cef_sdk)]
    pub fn request_close(&self) {
        unsafe { neko_cef_bridge_browser_close(self.browser.as_ptr()) };
    }

    #[cfg(not(has_cef_sdk))]
    pub fn is_ready(&self) -> bool {
        false
    }

    #[cfg(has_cef_sdk)]
    pub fn is_ready(&self) -> bool {
        unsafe { neko_cef_bridge_browser_is_ready(self.browser.as_ptr()) != 0 }
    }
}

impl Drop for CefCBridge {
    #[cfg(not(has_cef_sdk))]
    fn drop(&mut self) {}

    #[cfg(has_cef_sdk)]
    fn drop(&mut self) {
        unsafe {
            neko_cef_bridge_browser_close(self.browser.as_ptr());
            neko_cef_bridge_browser_release(self.browser.as_ptr());
            neko_cef_bridge_shutdown(self.runtime.as_ptr());
        }
    }
}

#[cfg(not(has_cef_sdk))]
pub fn try_run_c_subprocess() -> Result<Option<i32>> {
    Ok(None)
}

#[cfg(has_cef_sdk)]
pub fn try_run_c_subprocess() -> Result<Option<i32>> {
    let args = collect_main_args()?;
    let code =
        unsafe { neko_cef_bridge_execute_process(args.argc, args.argv.as_ptr() as *mut *mut c_char, 1) };
    if code >= 0 {
        Ok(Some(code))
    } else {
        Ok(None)
    }
}

pub fn run_raw_input_loop_cbridge(
    bridge: &CefCBridge,
    handle: &RawHostHandle,
    pointer_events: Receiver<RawHostPointerEvent>,
    keyboard_events: Receiver<RawHostKeyboardEvent>,
) {
    bridge.focus_browser(true);
    bridge.notify_resized();
    let mut mouse_modifiers = 0_u32;
    let mut key_modifiers = 0_u32;

    while handle.is_running() {
        let mut progressed = false;

        if matches!(bridge.message_loop_mode(), NekoCefMessageLoopMode::ExternalPump) {
            bridge.do_message_loop_work();
        }

        loop {
            match pointer_events.try_recv() {
                Ok(event) => {
                    progressed = true;
                    forward_pointer_event_cbridge(bridge, event, &mut mouse_modifiers, key_modifiers);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }

        loop {
            match keyboard_events.try_recv() {
                Ok(event) => {
                    progressed = true;
                    key_modifiers = current_key_modifier_flags(&event);
                    forward_keyboard_event_cbridge(bridge, event);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }

        if !progressed {
            thread::sleep(Duration::from_millis(8));
        }
    }
}

struct MainArgs {
    _cstrings: Vec<CString>,
    argv: Vec<*const c_char>,
    argc: c_int,
}

fn collect_main_args() -> Result<MainArgs> {
    let cstrings = std::env::args_os()
        .map(|arg| CString::new(arg.to_string_lossy().as_bytes().to_vec()))
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|err| anyhow!("failed to convert argv into C strings: {err}"))?;
    let argv = cstrings.iter().map(|arg| arg.as_ptr()).collect::<Vec<_>>();
    Ok(MainArgs {
        argc: argv.len() as c_int,
        argv,
        _cstrings: cstrings,
    })
}

fn error_buffer_to_string(buffer: &[i8]) -> String {
    let ptr = buffer.as_ptr();
    if ptr.is_null() {
        return "unknown C bridge error".to_string();
    }
    unsafe { CStr::from_ptr(ptr) }.to_string_lossy().trim().to_string()
}

fn default_cef_locale() -> Option<String> {
    let raw = std::env::var("LC_ALL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| std::env::var("LANG").ok())?;
    let normalized = raw
        .split('.')
        .next()
        .unwrap_or(raw.as_str())
        .split('@')
        .next()
        .unwrap_or(raw.as_str())
        .replace('_', "-");
    let trimmed = normalized.trim();
    if trimmed.is_empty()
        || trimmed.eq_ignore_ascii_case("c")
        || trimmed.eq_ignore_ascii_case("posix")
    {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn ensure_runtime_library_path(runtime_plan: &CefRuntimePlan) {
    let runtime_root = runtime_plan.sdk.root.to_string_lossy();
    let current = std::env::var("LD_LIBRARY_PATH").unwrap_or_default();
    let already_present = current
        .split(':')
        .any(|entry| !entry.is_empty() && entry == runtime_root);
    if already_present {
        return;
    }

    let updated = if current.trim().is_empty() {
        runtime_root.to_string()
    } else {
        format!("{runtime_root}:{current}")
    };
    unsafe {
        std::env::set_var("LD_LIBRARY_PATH", updated);
    }
}

unsafe extern "C" fn on_after_created(user_data: *mut c_void) {
    let _ = user_data;
    emit_lifecycle_event(CefLifecycleEvent::BrowserCreated);
}

unsafe extern "C" fn on_before_close(user_data: *mut c_void) {
    let _ = user_data;
    emit_lifecycle_event(CefLifecycleEvent::BrowserBeforeClose);
}

unsafe extern "C" fn on_loading_state_change(
    user_data: *mut c_void,
    is_loading: c_int,
    can_go_back: c_int,
    can_go_forward: c_int,
) {
    let _ = user_data;
    emit_lifecycle_event(CefLifecycleEvent::LoadingStateChange {
        is_loading: is_loading != 0,
        can_go_back: can_go_back != 0,
        can_go_forward: can_go_forward != 0,
    });
}

unsafe extern "C" fn on_load_start(user_data: *mut c_void, transition_type: c_int) {
    let _ = user_data;
    emit_lifecycle_event(CefLifecycleEvent::LoadStart { transition_type });
}

unsafe extern "C" fn on_load_end(user_data: *mut c_void, http_status_code: c_int) {
    let _ = user_data;
    emit_lifecycle_event(CefLifecycleEvent::LoadEnd { http_status_code });
}

unsafe extern "C" fn on_load_error(
    user_data: *mut c_void,
    error_code: c_int,
    error_text: *const c_char,
    failed_url: *const c_char,
) {
    let _ = user_data;
    emit_lifecycle_event(CefLifecycleEvent::LoadError {
        error_code,
        error_text: cstr_to_string(error_text),
        failed_url: cstr_to_string(failed_url),
    });
}

unsafe extern "C" fn on_console(
    user_data: *mut c_void,
    level: c_int,
    source: *const c_char,
    line: c_int,
    message: *const c_char,
) {
    let _ = user_data;
    emit_lifecycle_event(CefLifecycleEvent::Console {
        level,
        source: cstr_to_string(source),
        line,
        message: cstr_to_string(message),
    });
}

unsafe extern "C" fn on_paint(
    user_data: *mut c_void,
    element_type: c_int,
    buffer: *const c_void,
    width: c_int,
    height: c_int,
) {
    let Some(state) = (unsafe { (user_data as *mut CallbackState).as_ref() }) else {
        return;
    };
    if element_type != 0 || buffer.is_null() || width <= 0 || height <= 0 {
        return;
    }

    let len = width as usize * height as usize * 4;
    let bgra = unsafe { std::slice::from_raw_parts(buffer.cast::<u8>(), len) };
    if let Ok(frame) = RawHostFrame::from_bgra(width as u32, height as u32, bgra.to_vec()) {
        let _ = state.raw_host.set_rgba_frame(frame);
    }
}

fn cstr_to_string(ptr: *const c_char) -> String {
    if ptr.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(ptr) }.to_string_lossy().into_owned()
}

fn forward_pointer_event_cbridge(
    bridge: &CefCBridge,
    event: RawHostPointerEvent,
    mouse_modifiers: &mut u32,
    key_modifiers: u32,
) {
    const EVENTFLAG_PRECISION_SCROLLING_DELTA: u32 = 1 << 14;

    match event {
        RawHostPointerEvent::Enter { x, y } => {
            bridge.focus_browser(true);
            bridge.send_mouse_move_event(
                x.round() as i32,
                y.round() as i32,
                false,
                key_modifiers | *mouse_modifiers,
            );
        }
        RawHostPointerEvent::Leave { x, y } => {
            bridge.send_mouse_move_event(
                x.round() as i32,
                y.round() as i32,
                true,
                key_modifiers | *mouse_modifiers,
            );
            bridge.focus_browser(false);
        }
        RawHostPointerEvent::Motion { x, y } => {
            bridge.send_mouse_move_event(
                x.round() as i32,
                y.round() as i32,
                false,
                key_modifiers | *mouse_modifiers,
            );
        }
        RawHostPointerEvent::Button {
            x,
            y,
            button,
            pressed,
        } => {
            let Some(button) = map_button(button) else {
                return;
            };
            let flag = button_flag(button);
            if pressed {
                *mouse_modifiers |= flag;
            } else {
                *mouse_modifiers &= !flag;
            }
            bridge.send_mouse_click_event(
                x.round() as i32,
                y.round() as i32,
                button,
                !pressed,
                1,
                key_modifiers | *mouse_modifiers,
            );
        }
        RawHostPointerEvent::Wheel {
            x,
            y,
            delta_x,
            delta_y,
        } => {
            bridge.send_mouse_wheel_event(
                x.round() as i32,
                y.round() as i32,
                delta_x.round() as i32,
                delta_y.round() as i32,
                key_modifiers | *mouse_modifiers | EVENTFLAG_PRECISION_SCROLLING_DELTA,
            );
        }
    }
}

fn forward_keyboard_event_cbridge(bridge: &CefCBridge, event: RawHostKeyboardEvent) {
    match event {
        RawHostKeyboardEvent::Press {
            raw_code,
            keysym,
            utf8,
            modifiers,
        } => {
            let cef_modifiers = map_modifiers(modifiers, keysym, false);
            let windows_key_code = map_windows_key_code(keysym, utf8.as_deref());
            bridge.send_key_event(
                NekoCefKeyEventKind::RawKeyDown,
                windows_key_code,
                raw_code as i32,
                cef_modifiers,
                0,
                0,
            );
            if let Some(text) = utf8 {
                for ch in text.encode_utf16() {
                    bridge.send_key_event(
                        NekoCefKeyEventKind::Char,
                        windows_key_code,
                        raw_code as i32,
                        cef_modifiers,
                        ch,
                        ch,
                    );
                }
            }
        }
        RawHostKeyboardEvent::Repeat {
            raw_code,
            keysym,
            utf8,
            modifiers,
        } => {
            let cef_modifiers = map_modifiers(modifiers, keysym, true);
            let windows_key_code = map_windows_key_code(keysym, utf8.as_deref());
            bridge.send_key_event(
                NekoCefKeyEventKind::RawKeyDown,
                windows_key_code,
                raw_code as i32,
                cef_modifiers,
                0,
                0,
            );
            if let Some(text) = utf8 {
                for ch in text.encode_utf16() {
                    bridge.send_key_event(
                        NekoCefKeyEventKind::Char,
                        windows_key_code,
                        raw_code as i32,
                        cef_modifiers,
                        ch,
                        ch,
                    );
                }
            }
        }
        RawHostKeyboardEvent::Release {
            raw_code,
            keysym,
            modifiers,
        } => {
            let cef_modifiers = map_modifiers(modifiers, keysym, false);
            let windows_key_code = map_windows_key_code(keysym, None);
            bridge.send_key_event(
                NekoCefKeyEventKind::KeyUp,
                windows_key_code,
                raw_code as i32,
                cef_modifiers,
                0,
                0,
            );
        }
    }
}

fn map_button(button: RawHostPointerButton) -> Option<NekoCefMouseButton> {
    match button {
        RawHostPointerButton::Left => Some(NekoCefMouseButton::Left),
        RawHostPointerButton::Middle => Some(NekoCefMouseButton::Middle),
        RawHostPointerButton::Right => Some(NekoCefMouseButton::Right),
        RawHostPointerButton::Other(_) => None,
    }
}

fn button_flag(button: NekoCefMouseButton) -> u32 {
    match button {
        NekoCefMouseButton::Left => 1 << 4,
        NekoCefMouseButton::Middle => 1 << 5,
        NekoCefMouseButton::Right => 1 << 6,
    }
}

fn map_modifiers(modifiers: RawHostModifiers, keysym: u32, is_repeat: bool) -> u32 {
    let mut flags = 0_u32;
    if modifiers.shift {
        flags |= 1 << 1;
    }
    if modifiers.ctrl {
        flags |= 1 << 2;
    }
    if modifiers.alt {
        flags |= 1 << 3;
    }
    if modifiers.logo {
        flags |= 1 << 7;
    }
    if modifiers.caps_lock {
        flags |= 1 << 0;
    }
    if modifiers.num_lock {
        flags |= 1 << 8;
    }
    if is_repeat {
        flags |= 1 << 13;
    }
    if matches!(keysym, key::Alt_L | key::Alt_R) {
        flags |= 1 << 3;
    }
    flags
}

fn current_key_modifier_flags(event: &RawHostKeyboardEvent) -> u32 {
    match event {
        RawHostKeyboardEvent::Press {
            modifiers, keysym, ..
        }
        | RawHostKeyboardEvent::Repeat {
            modifiers, keysym, ..
        }
        | RawHostKeyboardEvent::Release {
            modifiers, keysym, ..
        } => map_modifiers(*modifiers, *keysym, false),
    }
}

fn map_windows_key_code(keysym: u32, utf8: Option<&str>) -> i32 {
    match keysym {
        key::Return => 0x0D,
        key::BackSpace => 0x08,
        key::Tab => 0x09,
        key::Escape => 0x1B,
        key::Delete => 0x2E,
        key::Home => 0x24,
        key::End => 0x23,
        key::Page_Up => 0x21,
        key::Page_Down => 0x22,
        key::Left => 0x25,
        key::Up => 0x26,
        key::Right => 0x27,
        key::Down => 0x28,
        key::Shift_L | key::Shift_R => 0x10,
        key::Control_L | key::Control_R => 0x11,
        key::Alt_L | key::Alt_R => 0x12,
        key::Super_L | key::Super_R => 0x5B,
        _ => utf8
            .and_then(|text| text.chars().next())
            .map(|ch| ch.to_ascii_uppercase() as i32)
            .unwrap_or(keysym as i32),
    }
}
