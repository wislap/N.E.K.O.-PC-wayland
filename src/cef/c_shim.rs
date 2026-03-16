#![allow(dead_code)]

use std::os::raw::{c_char, c_int, c_void};

#[cfg(has_cef_sdk)]
use std::ffi::CString;

use anyhow::{Result, anyhow};

#[repr(C)]
pub struct NekoCefRuntimeHandle {
    _private: [u8; 0],
}

#[repr(C)]
pub struct NekoCefBrowserHandle {
    _private: [u8; 0],
}

pub const NEKO_CEF_BRIDGE_ERROR_CAPACITY: usize = 1024;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct NekoCefRuntimeSettings {
    pub browser_subprocess_path: *const c_char,
    pub resources_dir_path: *const c_char,
    pub locales_dir_path: *const c_char,
    pub locale: *const c_char,
    pub cache_path: *const c_char,
    pub root_cache_path: *const c_char,
    pub no_sandbox: c_int,
    pub multi_threaded_message_loop: c_int,
    pub windowless_rendering_enabled: c_int,
    pub external_message_pump: c_int,
    pub remote_debugging_port: c_int,
    pub use_app: c_int,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct NekoCefBrowserConfig {
    pub url: *const c_char,
    pub window_name: *const c_char,
    pub width: c_int,
    pub height: c_int,
    pub frame_rate: c_int,
    pub transparent_painting: c_int,
}

pub type NekoCefAfterCreatedCallback = Option<unsafe extern "C" fn(user_data: *mut c_void)>;
pub type NekoCefBeforeCloseCallback = Option<unsafe extern "C" fn(user_data: *mut c_void)>;
pub type NekoCefLoadingStateChangeCallback = Option<
    unsafe extern "C" fn(
        user_data: *mut c_void,
        is_loading: c_int,
        can_go_back: c_int,
        can_go_forward: c_int,
    ),
>;
pub type NekoCefLoadStartCallback =
    Option<unsafe extern "C" fn(user_data: *mut c_void, transition_type: c_int)>;
pub type NekoCefLoadEndCallback =
    Option<unsafe extern "C" fn(user_data: *mut c_void, http_status_code: c_int)>;
pub type NekoCefLoadErrorCallback = Option<
    unsafe extern "C" fn(
        user_data: *mut c_void,
        error_code: c_int,
        error_text: *const c_char,
        failed_url: *const c_char,
    ),
>;
pub type NekoCefConsoleCallback = Option<
    unsafe extern "C" fn(
        user_data: *mut c_void,
        level: c_int,
        source: *const c_char,
        line: c_int,
        message: *const c_char,
    ),
>;
pub type NekoCefPaintCallback = Option<
    unsafe extern "C" fn(
        user_data: *mut c_void,
        element_type: c_int,
        buffer: *const c_void,
        width: c_int,
        height: c_int,
    ),
>;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct NekoCefBrowserCallbacks {
    pub on_after_created: NekoCefAfterCreatedCallback,
    pub on_before_close: NekoCefBeforeCloseCallback,
    pub on_loading_state_change: NekoCefLoadingStateChangeCallback,
    pub on_load_start: NekoCefLoadStartCallback,
    pub on_load_end: NekoCefLoadEndCallback,
    pub on_load_error: NekoCefLoadErrorCallback,
    pub on_console: NekoCefConsoleCallback,
    pub on_paint: NekoCefPaintCallback,
}

#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NekoCefMessageLoopMode {
    ExternalPump = 0,
    MultiThreaded = 1,
}

#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NekoCefMouseButton {
    Left = 0,
    Middle = 1,
    Right = 2,
}

#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NekoCefKeyEventKind {
    RawKeyDown = 0,
    KeyUp = 1,
    Char = 2,
}

#[cfg(has_cef_sdk)]
#[link(name = "neko_cef_c_probe", kind = "static")]
unsafe extern "C" {
    fn neko_cef_capi_probe_run(argc: c_int, argv: *mut *mut c_char) -> c_int;
    fn neko_cef_capi_probe_run_null_app(argc: c_int, argv: *mut *mut c_char) -> c_int;
    pub fn neko_cef_bridge_execute_process(argc: c_int, argv: *mut *mut c_char, use_app: c_int)
        -> c_int;
    pub fn neko_cef_bridge_initialize(
        argc: c_int,
        argv: *mut *mut c_char,
        options: *const NekoCefRuntimeSettings,
        error_message: *mut c_char,
        error_message_capacity: usize,
    ) -> *mut NekoCefRuntimeHandle;
    pub fn neko_cef_bridge_do_message_loop_work(runtime: *mut NekoCefRuntimeHandle);
    pub fn neko_cef_bridge_message_loop_mode(runtime: *const NekoCefRuntimeHandle) -> c_int;
    pub fn neko_cef_bridge_shutdown(runtime: *mut NekoCefRuntimeHandle);
    pub fn neko_cef_bridge_create_browser(
        runtime: *mut NekoCefRuntimeHandle,
        config: *const NekoCefBrowserConfig,
        callbacks: *const NekoCefBrowserCallbacks,
        user_data: *mut c_void,
        error_message: *mut c_char,
        error_message_capacity: usize,
    ) -> *mut NekoCefBrowserHandle;
    pub fn neko_cef_bridge_browser_close(browser: *mut NekoCefBrowserHandle);
    pub fn neko_cef_bridge_browser_release(browser: *mut NekoCefBrowserHandle);
    pub fn neko_cef_bridge_browser_is_ready(browser: *const NekoCefBrowserHandle) -> c_int;
    pub fn neko_cef_bridge_browser_set_focus(browser: *mut NekoCefBrowserHandle, focused: c_int);
    pub fn neko_cef_bridge_browser_notify_resized(
        browser: *mut NekoCefBrowserHandle,
        width: c_int,
        height: c_int,
    );
    pub fn neko_cef_bridge_browser_send_mouse_move(
        browser: *mut NekoCefBrowserHandle,
        x: c_int,
        y: c_int,
        mouse_leave: c_int,
        modifiers: u32,
    );
    pub fn neko_cef_bridge_browser_send_mouse_click(
        browser: *mut NekoCefBrowserHandle,
        x: c_int,
        y: c_int,
        button: c_int,
        mouse_up: c_int,
        click_count: c_int,
        modifiers: u32,
    );
    pub fn neko_cef_bridge_browser_send_mouse_wheel(
        browser: *mut NekoCefBrowserHandle,
        x: c_int,
        y: c_int,
        delta_x: c_int,
        delta_y: c_int,
        modifiers: u32,
    );
    pub fn neko_cef_bridge_browser_send_key_event(
        browser: *mut NekoCefBrowserHandle,
        kind: c_int,
        windows_key_code: c_int,
        native_key_code: c_int,
        modifiers: u32,
        character: u16,
        unmodified_character: u16,
    );
}

#[derive(Debug, Clone, Copy)]
pub enum CShimMode {
    WithApp,
    NullApp,
}

#[cfg(has_cef_sdk)]
pub fn run_c_probe(mode: CShimMode) -> Result<i32> {
    let args = std::env::args_os()
        .map(|arg| CString::new(arg.to_string_lossy().as_bytes().to_vec()))
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|err| anyhow!("failed to convert argv into C strings: {err}"))?;
    let mut argv = args
        .iter()
        .map(|value| value.as_ptr() as *mut c_char)
        .collect::<Vec<_>>();

    let code = unsafe {
        match mode {
            CShimMode::WithApp => neko_cef_capi_probe_run(argv.len() as c_int, argv.as_mut_ptr()),
            CShimMode::NullApp => {
                neko_cef_capi_probe_run_null_app(argv.len() as c_int, argv.as_mut_ptr())
            }
        }
    };
    Ok(code)
}

#[cfg(not(has_cef_sdk))]
pub fn run_c_probe(_mode: CShimMode) -> Result<i32> {
    Err(anyhow!(
        "C CEF shim is unavailable because no local CEF SDK was discovered"
    ))
}
