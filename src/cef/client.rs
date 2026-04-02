#![allow(dead_code)]

use std::mem::offset_of;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::cef::events::{CefLifecycleEvent, emit as emit_lifecycle_event};
use crate::cef::ffi::{
    cef_base_ref_counted_t, cef_browser_host_t, cef_browser_t, cef_client_t, cef_display_handler_t,
    cef_frame_t, cef_key_event_t, cef_key_event_type_t, cef_life_span_handler_t, cef_load_handler_t,
    cef_mouse_button_type_t, cef_mouse_event_t, cef_render_handler_t,
};
use crate::cef::log::trace_enabled;
use crate::cef::render_handler::{CefRenderHandlerState, CefRenderHandlerWrapper, add_ref_raw};
use crate::cef::strings::{CefOwnedString, decode_cef_string};
use crate::cef::{CefKeyEventKind, CefMouseButton};
use crate::wayland::raw_host::RawHostHandle;

#[repr(C)]
pub struct CefClientWrapper {
    raw: cef_client_t,
    ref_count: AtomicUsize,
    display_handler: Box<CefDisplayHandlerWrapper>,
    life_span_handler: Box<CefLifeSpanHandlerWrapper>,
    load_handler: Box<CefLoadHandlerWrapper>,
    render_handler: Box<CefRenderHandlerWrapper>,
}

impl CefClientWrapper {
    fn new(
        width: u32,
        height: u32,
        transparent_painting: bool,
        raw_host: RawHostHandle,
        browser_host: Arc<Mutex<BrowserHostState>>,
    ) -> Result<Box<Self>> {
        let render_handler = CefRenderHandlerWrapper::new(
            width,
            height,
            transparent_painting,
            raw_host,
            Arc::clone(&browser_host),
        )?;
        let display_handler = CefDisplayHandlerWrapper::new();
        let life_span_handler = CefLifeSpanHandlerWrapper::new(Arc::clone(&browser_host));
        let load_handler = CefLoadHandlerWrapper::new();
        Ok(Box::new(Self {
            raw: cef_client_t {
                base: cef_base_ref_counted_t {
                    size: std::mem::size_of::<cef_client_t>(),
                    add_ref: Some(client_add_ref),
                    release: Some(client_release),
                    has_one_ref: Some(client_has_one_ref),
                    has_at_least_one_ref: Some(client_has_at_least_one_ref),
                },
                get_audio_handler: None,
                get_command_handler: None,
                get_context_menu_handler: None,
                get_dialog_handler: None,
                get_display_handler: Some(client_get_display_handler),
                get_download_handler: None,
                get_drag_handler: None,
                get_find_handler: None,
                get_focus_handler: None,
                get_frame_handler: None,
                get_permission_handler: None,
                get_jsdialog_handler: None,
                get_keyboard_handler: None,
                get_life_span_handler: Some(client_get_life_span_handler),
                get_load_handler: Some(client_get_load_handler),
                get_print_handler: None,
                get_render_handler: Some(client_get_render_handler),
                get_request_handler: None,
                on_process_message_received: None,
            },
            ref_count: AtomicUsize::new(1),
            display_handler,
            life_span_handler,
            load_handler,
            render_handler,
        }))
    }

    pub fn raw_ptr(&mut self) -> *mut cef_client_t {
        &mut self.raw
    }

    pub fn render_handler_ptr(&mut self) -> *mut cef_render_handler_t {
        self.render_handler.raw_ptr()
    }

    pub fn render_handler_state(&self) -> &CefRenderHandlerState {
        self.render_handler.state()
    }

    pub fn life_span_handler_ptr(&mut self) -> *mut cef_life_span_handler_t {
        self.life_span_handler.raw_ptr()
    }

    pub fn display_handler_ptr(&mut self) -> *mut cef_display_handler_t {
        self.display_handler.raw_ptr()
    }

    pub fn load_handler_ptr(&mut self) -> *mut cef_load_handler_t {
        self.load_handler.raw_ptr()
    }
}

pub struct CefClientState {
    client: NonNull<CefClientWrapper>,
    browser_host: Arc<Mutex<BrowserHostState>>,
}

impl CefClientState {
    pub fn new(
        width: u32,
        height: u32,
        transparent_painting: bool,
        raw_host: RawHostHandle,
    ) -> Result<Self> {
        let browser_host = Arc::new(Mutex::new(BrowserHostState::default()));
        let client = CefClientWrapper::new(
            width,
            height,
            transparent_painting,
            raw_host,
            Arc::clone(&browser_host),
        )?;
        Ok(Self {
            client: NonNull::new(Box::into_raw(client))
                .expect("cef client pointer should not be null"),
            browser_host,
        })
    }

    pub fn raw_ptr(&mut self) -> *mut cef_client_t {
        unsafe { self.client.as_mut().raw_ptr() }
    }

    pub fn render_handler_state(&self) -> &CefRenderHandlerState {
        unsafe { self.client.as_ref().render_handler_state() }
    }

    pub fn add_ref_for_cef(&self) {
        unsafe {
            client_add_ref_raw(self.client.as_ptr().cast::<cef_client_t>());
        }
    }

    pub fn release_for_cef(&self) {
        unsafe {
            client_release_raw(self.client.as_ptr().cast::<cef_client_t>());
        }
    }

    pub fn browser_host_snapshot(&self) -> BrowserHostSnapshot {
        let state = self
            .browser_host
            .lock()
            .expect("browser host mutex poisoned");
        BrowserHostSnapshot {
            browser: state.browser,
            host: state.host,
        }
    }

    pub fn focus_browser(&self, focused: bool) {
        let state = self
            .browser_host
            .lock()
            .expect("browser host mutex poisoned");
        if let Some(host) = state.host {
            unsafe {
                if let Some(set_focus) = (*host.as_ptr()).set_focus {
                    set_focus(host.as_ptr(), focused as i32);
                }
            }
        }
    }

    pub fn notify_resized(&self) {
        let state = self
            .browser_host
            .lock()
            .expect("browser host mutex poisoned");
        if let Some(host) = state.host {
            unsafe {
                if let Some(was_resized) = (*host.as_ptr()).was_resized {
                    was_resized(host.as_ptr());
                }
            }
        }
    }

    pub fn send_mouse_move_event(&self, x: i32, y: i32, mouse_leave: bool, modifiers: u32) {
        self.with_host(|host| unsafe {
            if let Some(send_mouse_move_event) = (*host.as_ptr()).send_mouse_move_event {
                let event = cef_mouse_event_t { x, y, modifiers };
                send_mouse_move_event(host.as_ptr(), &event, mouse_leave as i32);
            }
        });
    }

    pub fn send_mouse_click_event(
        &self,
        x: i32,
        y: i32,
        button: CefMouseButton,
        mouse_up: bool,
        click_count: i32,
        modifiers: u32,
    ) {
        self.with_host(|host| unsafe {
            if let Some(send_mouse_click_event) = (*host.as_ptr()).send_mouse_click_event {
                let event = cef_mouse_event_t { x, y, modifiers };
                send_mouse_click_event(
                    host.as_ptr(),
                    &event,
                    map_mouse_button(button),
                    mouse_up as i32,
                    click_count,
                );
            }
        });
    }

    pub fn send_mouse_wheel_event(
        &self,
        x: i32,
        y: i32,
        delta_x: i32,
        delta_y: i32,
        modifiers: u32,
    ) {
        self.with_host(|host| unsafe {
            if let Some(send_mouse_wheel_event) = (*host.as_ptr()).send_mouse_wheel_event {
                let event = cef_mouse_event_t { x, y, modifiers };
                send_mouse_wheel_event(host.as_ptr(), &event, delta_x, delta_y);
            }
        });
    }

    pub fn send_key_event(
        &self,
        kind: CefKeyEventKind,
        windows_key_code: i32,
        native_key_code: i32,
        modifiers: u32,
        character: u16,
        unmodified_character: u16,
    ) {
        self.with_host(|host| unsafe {
            if let Some(send_key_event) = (*host.as_ptr()).send_key_event {
                let event = cef_key_event_t {
                    size: std::mem::size_of::<cef_key_event_t>(),
                    type_: map_key_event_kind(kind),
                    modifiers,
                    windows_key_code,
                    native_key_code,
                    is_system_key: 0,
                    character,
                    unmodified_character,
                    focus_on_editable_field: 0,
                };
                send_key_event(host.as_ptr(), &event);
            }
        });
    }

    fn with_host(&self, f: impl FnOnce(NonNull<cef_browser_host_t>)) {
        let state = self
            .browser_host
            .lock()
            .expect("browser host mutex poisoned");
        if let Some(host) = state.host {
            f(host);
        }
    }

    pub fn request_close(&self) {
        self.with_host(|host| unsafe {
            if let Some(close_browser) = (*host.as_ptr()).close_browser {
                eprintln!("CEF request_close: host={:p}", host.as_ptr());
                close_browser(host.as_ptr(), 1);
            }
        });

        let deadline = Instant::now() + Duration::from_millis(750);
        while Instant::now() < deadline {
            let state = self
                .browser_host
                .lock()
                .expect("browser host mutex poisoned");
            if state.browser.is_none() && state.host.is_none() {
                eprintln!("CEF request_close: browser closed");
                return;
            }
            drop(state);
            std::thread::sleep(Duration::from_millis(10));
        }

        eprintln!("CEF request_close: timed out waiting for before_close");
    }

    pub fn execute_javascript(&self, code: &str, script_url: &str, start_line: i32) {
        let state = self
            .browser_host
            .lock()
            .expect("browser host mutex poisoned");
        let Some(browser) = state.browser else {
            return;
        };

        unsafe {
            let Some(get_main_frame) = (*browser.as_ptr()).get_main_frame else {
                return;
            };
            let Some(frame) = NonNull::<cef_frame_t>::new(get_main_frame(browser.as_ptr())) else {
                return;
            };

            if let Some(execute_java_script) = (*frame.as_ptr()).execute_java_script {
                let code = CefOwnedString::new(code);
                let script_url = CefOwnedString::new(script_url);
                execute_java_script(frame.as_ptr(), code.as_cef(), script_url.as_cef(), start_line);
            }

            let _ = base_release(frame.as_ptr().cast::<cef_base_ref_counted_t>());
        }
    }
}

fn map_mouse_button(button: CefMouseButton) -> cef_mouse_button_type_t {
    match button {
        CefMouseButton::Left => cef_mouse_button_type_t::MBT_LEFT,
        CefMouseButton::Middle => cef_mouse_button_type_t::MBT_MIDDLE,
        CefMouseButton::Right => cef_mouse_button_type_t::MBT_RIGHT,
    }
}

fn map_key_event_kind(kind: CefKeyEventKind) -> cef_key_event_type_t {
    match kind {
        CefKeyEventKind::RawKeyDown => cef_key_event_type_t::KEYEVENT_RAWKEYDOWN,
        CefKeyEventKind::KeyUp => cef_key_event_type_t::KEYEVENT_KEYUP,
        CefKeyEventKind::Char => cef_key_event_type_t::KEYEVENT_CHAR,
    }
}

impl Drop for CefClientState {
    fn drop(&mut self) {
        self.release_for_cef();
    }
}

unsafe fn wrapper_from_raw<'a>(this: *mut cef_client_t) -> &'a mut CefClientWrapper {
    unsafe {
        let addr = (this.cast::<u8>()).sub(offset_of!(CefClientWrapper, raw));
        &mut *addr.cast::<CefClientWrapper>()
    }
}

unsafe fn wrapper_from_base<'a>(this: *mut cef_base_ref_counted_t) -> &'a mut CefClientWrapper {
    unsafe {
        let raw = this.cast::<cef_client_t>();
        wrapper_from_raw(raw)
    }
}

unsafe extern "C" fn client_add_ref(this: *mut cef_base_ref_counted_t) {
    let wrapper = unsafe { wrapper_from_base(this) };
    let previous = wrapper.ref_count.fetch_add(1, Ordering::Relaxed);
    if trace_enabled() {
        eprintln!(
            "CEF client_add_ref: wrapper={:p} previous={} new={}",
            wrapper,
            previous,
            previous + 1
        );
    }
}

pub unsafe fn client_add_ref_raw(this: *mut cef_client_t) {
    if this.is_null() {
        return;
    }
    let wrapper = unsafe { wrapper_from_raw(this) };
    let previous = wrapper.ref_count.fetch_add(1, Ordering::Relaxed);
    if trace_enabled() {
        eprintln!(
            "CEF client_add_ref_raw: wrapper={:p} previous={} new={}",
            wrapper,
            previous,
            previous + 1
        );
    }
}

unsafe extern "C" fn client_release(this: *mut cef_base_ref_counted_t) -> i32 {
    let wrapper = unsafe { wrapper_from_base(this) };
    let previous = wrapper.ref_count.fetch_sub(1, Ordering::Release);
    if trace_enabled() {
        eprintln!(
            "CEF client_release: wrapper={:p} previous={} new={}",
            wrapper,
            previous,
            previous.saturating_sub(1)
        );
    }
    if previous == 1 {
        std::sync::atomic::fence(Ordering::Acquire);
        unsafe {
            drop(Box::from_raw(wrapper));
        }
        1
    } else {
        0
    }
}

pub unsafe fn client_release_raw(this: *mut cef_client_t) -> i32 {
    if this.is_null() {
        return 0;
    }
    let wrapper = unsafe { wrapper_from_raw(this) };
    let previous = wrapper.ref_count.fetch_sub(1, Ordering::Release);
    if trace_enabled() {
        eprintln!(
            "CEF client_release_raw: wrapper={:p} previous={} new={}",
            wrapper,
            previous,
            previous.saturating_sub(1)
        );
    }
    if previous == 1 {
        std::sync::atomic::fence(Ordering::Acquire);
        unsafe {
            drop(Box::from_raw(wrapper));
        }
        1
    } else {
        0
    }
}

unsafe extern "C" fn client_has_one_ref(this: *mut cef_base_ref_counted_t) -> i32 {
    let wrapper = unsafe { wrapper_from_base(this) };
    (wrapper.ref_count.load(Ordering::Acquire) == 1) as i32
}

unsafe extern "C" fn client_has_at_least_one_ref(this: *mut cef_base_ref_counted_t) -> i32 {
    let wrapper = unsafe { wrapper_from_base(this) };
    (wrapper.ref_count.load(Ordering::Acquire) > 0) as i32
}

unsafe extern "C" fn client_get_render_handler(
    this: *mut cef_client_t,
) -> *mut cef_render_handler_t {
    if trace_enabled() {
        eprintln!("CEF client_get_render_handler: this={this:p}");
    }
    let wrapper = unsafe { wrapper_from_raw(this) };
    let ptr = wrapper.render_handler_ptr();
    unsafe {
        add_ref_raw(ptr);
    }
    if trace_enabled() {
        eprintln!("CEF client_get_render_handler: returning render_handler={ptr:p}");
    }
    ptr
}

unsafe extern "C" fn client_get_life_span_handler(
    this: *mut cef_client_t,
) -> *mut cef_life_span_handler_t {
    if trace_enabled() {
        eprintln!("CEF client_get_life_span_handler: this={this:p}");
    }
    let wrapper = unsafe { wrapper_from_raw(this) };
    let ptr = wrapper.life_span_handler_ptr();
    unsafe {
        life_span_add_ref_raw(ptr);
    }
    if trace_enabled() {
        eprintln!("CEF client_get_life_span_handler: returning life_span_handler={ptr:p}");
    }
    ptr
}

unsafe extern "C" fn client_get_load_handler(this: *mut cef_client_t) -> *mut cef_load_handler_t {
    if trace_enabled() {
        eprintln!("CEF client_get_load_handler: this={this:p}");
    }
    let wrapper = unsafe { wrapper_from_raw(this) };
    let ptr = wrapper.load_handler_ptr();
    unsafe {
        load_add_ref_raw(ptr);
    }
    if trace_enabled() {
        eprintln!("CEF client_get_load_handler: returning load_handler={ptr:p}");
    }
    ptr
}

unsafe extern "C" fn client_get_display_handler(
    this: *mut cef_client_t,
) -> *mut cef_display_handler_t {
    if trace_enabled() {
        eprintln!("CEF client_get_display_handler: this={this:p}");
    }
    let wrapper = unsafe { wrapper_from_raw(this) };
    let ptr = wrapper.display_handler_ptr();
    unsafe {
        display_add_ref_raw(ptr);
    }
    if trace_enabled() {
        eprintln!("CEF client_get_display_handler: returning display_handler={ptr:p}");
    }
    ptr
}

#[derive(Debug, Clone, Copy)]
pub struct BrowserHostSnapshot {
    pub browser: Option<NonNull<cef_browser_t>>,
    pub host: Option<NonNull<cef_browser_host_t>>,
}

pub(crate) type BrowserHostShared = Arc<Mutex<BrowserHostState>>;

#[derive(Debug, Default)]
pub(crate) struct BrowserHostState {
    pub(crate) browser: Option<NonNull<cef_browser_t>>,
    pub(crate) host: Option<NonNull<cef_browser_host_t>>,
}

#[repr(C)]
struct CefLifeSpanHandlerWrapper {
    raw: cef_life_span_handler_t,
    ref_count: AtomicUsize,
    browser_host: Arc<Mutex<BrowserHostState>>,
}

#[repr(C)]
struct CefDisplayHandlerWrapper {
    raw: cef_display_handler_t,
    ref_count: AtomicUsize,
}

#[repr(C)]
struct CefLoadHandlerWrapper {
    raw: cef_load_handler_t,
    ref_count: AtomicUsize,
}

impl CefLifeSpanHandlerWrapper {
    fn new(browser_host: Arc<Mutex<BrowserHostState>>) -> Box<Self> {
        Box::new(Self {
            raw: cef_life_span_handler_t {
                base: cef_base_ref_counted_t {
                    size: std::mem::size_of::<cef_life_span_handler_t>(),
                    add_ref: Some(life_span_add_ref),
                    release: Some(life_span_release),
                    has_one_ref: Some(life_span_has_one_ref),
                    has_at_least_one_ref: Some(life_span_has_at_least_one_ref),
                },
                on_before_popup: None,
                on_before_popup_aborted: None,
                on_before_dev_tools_popup: None,
                on_after_created: Some(life_span_on_after_created),
                do_close: None,
                on_before_close: Some(life_span_on_before_close),
            },
            ref_count: AtomicUsize::new(1),
            browser_host,
        })
    }

    fn raw_ptr(&mut self) -> *mut cef_life_span_handler_t {
        &mut self.raw
    }
}

impl CefDisplayHandlerWrapper {
    fn new() -> Box<Self> {
        Box::new(Self {
            raw: cef_display_handler_t {
                base: cef_base_ref_counted_t {
                    size: std::mem::size_of::<cef_display_handler_t>(),
                    add_ref: Some(display_add_ref),
                    release: Some(display_release),
                    has_one_ref: Some(display_has_one_ref),
                    has_at_least_one_ref: Some(display_has_at_least_one_ref),
                },
                on_address_change: None,
                on_title_change: None,
                on_favicon_urlchange: None,
                on_fullscreen_mode_change: None,
                on_tooltip: None,
                on_status_message: None,
                on_console_message: Some(display_on_console_message),
                on_auto_resize: None,
                on_loading_progress_change: None,
                on_cursor_change: None,
                on_media_access_change: None,
                on_contents_bounds_change: None,
                get_root_window_screen_rect: None,
            },
            ref_count: AtomicUsize::new(1),
        })
    }

    fn raw_ptr(&mut self) -> *mut cef_display_handler_t {
        &mut self.raw
    }
}

impl CefLoadHandlerWrapper {
    fn new() -> Box<Self> {
        Box::new(Self {
            raw: cef_load_handler_t {
                base: cef_base_ref_counted_t {
                    size: std::mem::size_of::<cef_load_handler_t>(),
                    add_ref: Some(load_add_ref),
                    release: Some(load_release),
                    has_one_ref: Some(load_has_one_ref),
                    has_at_least_one_ref: Some(load_has_at_least_one_ref),
                },
                on_loading_state_change: Some(load_on_loading_state_change),
                on_load_start: Some(load_on_load_start),
                on_load_end: Some(load_on_load_end),
                on_load_error: Some(load_on_load_error),
            },
            ref_count: AtomicUsize::new(1),
        })
    }

    fn raw_ptr(&mut self) -> *mut cef_load_handler_t {
        &mut self.raw
    }
}

unsafe fn life_span_wrapper_from_raw<'a>(
    this: *mut cef_life_span_handler_t,
) -> &'a mut CefLifeSpanHandlerWrapper {
    unsafe {
        let addr = (this.cast::<u8>()).sub(offset_of!(CefLifeSpanHandlerWrapper, raw));
        &mut *addr.cast::<CefLifeSpanHandlerWrapper>()
    }
}

unsafe fn load_wrapper_from_raw<'a>(
    this: *mut cef_load_handler_t,
) -> &'a mut CefLoadHandlerWrapper {
    unsafe {
        let addr = (this.cast::<u8>()).sub(offset_of!(CefLoadHandlerWrapper, raw));
        &mut *addr.cast::<CefLoadHandlerWrapper>()
    }
}

unsafe fn display_wrapper_from_raw<'a>(
    this: *mut cef_display_handler_t,
) -> &'a mut CefDisplayHandlerWrapper {
    unsafe {
        let addr = (this.cast::<u8>()).sub(offset_of!(CefDisplayHandlerWrapper, raw));
        &mut *addr.cast::<CefDisplayHandlerWrapper>()
    }
}

unsafe fn display_wrapper_from_base<'a>(
    this: *mut cef_base_ref_counted_t,
) -> &'a mut CefDisplayHandlerWrapper {
    unsafe {
        let raw = this.cast::<cef_display_handler_t>();
        display_wrapper_from_raw(raw)
    }
}

unsafe fn load_wrapper_from_base<'a>(
    this: *mut cef_base_ref_counted_t,
) -> &'a mut CefLoadHandlerWrapper {
    unsafe {
        let raw = this.cast::<cef_load_handler_t>();
        load_wrapper_from_raw(raw)
    }
}

unsafe fn life_span_wrapper_from_base<'a>(
    this: *mut cef_base_ref_counted_t,
) -> &'a mut CefLifeSpanHandlerWrapper {
    unsafe {
        let raw = this.cast::<cef_life_span_handler_t>();
        life_span_wrapper_from_raw(raw)
    }
}

unsafe fn base_add_ref(base: *mut cef_base_ref_counted_t) {
    if base.is_null() {
        return;
    }
    if let Some(add_ref) = unsafe { (*base).add_ref } {
        unsafe { add_ref(base) };
    }
}

unsafe fn base_release(base: *mut cef_base_ref_counted_t) -> i32 {
    if base.is_null() {
        return 0;
    }
    if let Some(release) = unsafe { (*base).release } {
        unsafe { release(base) }
    } else {
        0
    }
}

unsafe fn life_span_add_ref_raw(this: *mut cef_life_span_handler_t) {
    unsafe { base_add_ref(this.cast::<cef_base_ref_counted_t>()) };
}

unsafe fn load_add_ref_raw(this: *mut cef_load_handler_t) {
    unsafe { base_add_ref(this.cast::<cef_base_ref_counted_t>()) };
}

unsafe fn display_add_ref_raw(this: *mut cef_display_handler_t) {
    unsafe { base_add_ref(this.cast::<cef_base_ref_counted_t>()) };
}

unsafe extern "C" fn life_span_add_ref(this: *mut cef_base_ref_counted_t) {
    let wrapper = unsafe { life_span_wrapper_from_base(this) };
    wrapper.ref_count.fetch_add(1, Ordering::Relaxed);
}

unsafe extern "C" fn life_span_release(this: *mut cef_base_ref_counted_t) -> i32 {
    let wrapper = unsafe { life_span_wrapper_from_base(this) };
    let previous = wrapper.ref_count.fetch_sub(1, Ordering::Release);
    if previous == 1 {
        std::sync::atomic::fence(Ordering::Acquire);
        unsafe {
            drop(Box::from_raw(wrapper));
        }
        1
    } else {
        0
    }
}

unsafe extern "C" fn life_span_has_one_ref(this: *mut cef_base_ref_counted_t) -> i32 {
    let wrapper = unsafe { life_span_wrapper_from_base(this) };
    (wrapper.ref_count.load(Ordering::Acquire) == 1) as i32
}

unsafe extern "C" fn life_span_has_at_least_one_ref(this: *mut cef_base_ref_counted_t) -> i32 {
    let wrapper = unsafe { life_span_wrapper_from_base(this) };
    (wrapper.ref_count.load(Ordering::Acquire) > 0) as i32
}

unsafe extern "C" fn load_add_ref(this: *mut cef_base_ref_counted_t) {
    let wrapper = unsafe { load_wrapper_from_base(this) };
    wrapper.ref_count.fetch_add(1, Ordering::Relaxed);
}

unsafe extern "C" fn load_release(this: *mut cef_base_ref_counted_t) -> i32 {
    let wrapper = unsafe { load_wrapper_from_base(this) };
    let previous = wrapper.ref_count.fetch_sub(1, Ordering::Release);
    if previous == 1 {
        std::sync::atomic::fence(Ordering::Acquire);
        unsafe {
            drop(Box::from_raw(wrapper));
        }
        1
    } else {
        0
    }
}

unsafe extern "C" fn load_has_one_ref(this: *mut cef_base_ref_counted_t) -> i32 {
    let wrapper = unsafe { load_wrapper_from_base(this) };
    (wrapper.ref_count.load(Ordering::Acquire) == 1) as i32
}

unsafe extern "C" fn load_has_at_least_one_ref(this: *mut cef_base_ref_counted_t) -> i32 {
    let wrapper = unsafe { load_wrapper_from_base(this) };
    (wrapper.ref_count.load(Ordering::Acquire) > 0) as i32
}

unsafe extern "C" fn display_add_ref(this: *mut cef_base_ref_counted_t) {
    let wrapper = unsafe { display_wrapper_from_base(this) };
    wrapper.ref_count.fetch_add(1, Ordering::Relaxed);
}

unsafe extern "C" fn display_release(this: *mut cef_base_ref_counted_t) -> i32 {
    let wrapper = unsafe { display_wrapper_from_base(this) };
    let previous = wrapper.ref_count.fetch_sub(1, Ordering::Release);
    if previous == 1 {
        std::sync::atomic::fence(Ordering::Acquire);
        unsafe {
            drop(Box::from_raw(wrapper));
        }
        1
    } else {
        0
    }
}

unsafe extern "C" fn display_has_one_ref(this: *mut cef_base_ref_counted_t) -> i32 {
    let wrapper = unsafe { display_wrapper_from_base(this) };
    (wrapper.ref_count.load(Ordering::Acquire) == 1) as i32
}

unsafe extern "C" fn display_has_at_least_one_ref(this: *mut cef_base_ref_counted_t) -> i32 {
    let wrapper = unsafe { display_wrapper_from_base(this) };
    (wrapper.ref_count.load(Ordering::Acquire) > 0) as i32
}

unsafe extern "C" fn life_span_on_after_created(
    this: *mut cef_life_span_handler_t,
    browser: *mut cef_browser_t,
) {
    eprintln!("CEF life_span_on_after_created: browser={browser:p}");
    emit_lifecycle_event(CefLifecycleEvent::BrowserCreated);
    if browser.is_null() {
        return;
    }

    let wrapper = unsafe { life_span_wrapper_from_raw(this) };
    let browser_host = unsafe {
        (*browser)
            .get_host
            .and_then(|get_host| NonNull::new(get_host(browser)))
    };

    unsafe {
        base_add_ref(browser.cast::<cef_base_ref_counted_t>());
        if let Some(host) = browser_host {
            base_add_ref(host.as_ptr().cast::<cef_base_ref_counted_t>());
        }
    }

    let mut state = wrapper
        .browser_host
        .lock()
        .expect("browser host mutex poisoned");
    state.browser = NonNull::new(browser);
    state.host = browser_host;

    if let Some(host) = browser_host {
        eprintln!("CEF life_span_on_after_created: host={:p}", host.as_ptr());
        unsafe {
            if let Some(set_focus) = (*host.as_ptr()).set_focus {
                set_focus(host.as_ptr(), 1);
            }
            if let Some(was_hidden) = (*host.as_ptr()).was_hidden {
                was_hidden(host.as_ptr(), 0);
            }
            if let Some(notify_screen_info_changed) = (*host.as_ptr()).notify_screen_info_changed {
                notify_screen_info_changed(host.as_ptr());
            }
            if let Some(was_resized) = (*host.as_ptr()).was_resized {
                was_resized(host.as_ptr());
            }
        }
        eprintln!("CEF life_span_on_after_created: initialized host focus/visibility/resize");
    }
}

unsafe extern "C" fn life_span_on_before_close(
    this: *mut cef_life_span_handler_t,
    browser: *mut cef_browser_t,
) {
    eprintln!("CEF life_span_on_before_close: browser={browser:p}");
    emit_lifecycle_event(CefLifecycleEvent::BrowserBeforeClose);
    let wrapper = unsafe { life_span_wrapper_from_raw(this) };
    let mut state = wrapper
        .browser_host
        .lock()
        .expect("browser host mutex poisoned");

    if let Some(host) = state.host.take() {
        unsafe {
            base_release(host.as_ptr().cast::<cef_base_ref_counted_t>());
        }
    }

    if let Some(browser_ptr) = state.browser.take() {
        unsafe {
            base_release(browser_ptr.as_ptr().cast::<cef_base_ref_counted_t>());
        }
    }
}

unsafe extern "C" fn load_on_loading_state_change(
    _this: *mut cef_load_handler_t,
    browser: *mut cef_browser_t,
    is_loading: i32,
    can_go_back: i32,
    can_go_forward: i32,
) {
    eprintln!(
        "CEF load_on_loading_state_change: browser={browser:p} is_loading={} can_go_back={} can_go_forward={}",
        is_loading, can_go_back, can_go_forward
    );
    emit_lifecycle_event(CefLifecycleEvent::LoadingStateChange {
        is_loading: is_loading != 0,
        can_go_back: can_go_back != 0,
        can_go_forward: can_go_forward != 0,
    });
}

unsafe extern "C" fn load_on_load_start(
    _this: *mut cef_load_handler_t,
    browser: *mut cef_browser_t,
    frame: *mut crate::cef::ffi::cef_frame_t,
    transition_type: crate::cef::ffi::cef_transition_type_t,
) {
    eprintln!(
        "CEF load_on_load_start: browser={browser:p} frame={frame:p} transition_type={transition_type}"
    );
    emit_lifecycle_event(CefLifecycleEvent::LoadStart {
        transition_type: transition_type as i32,
    });
}

unsafe extern "C" fn load_on_load_end(
    _this: *mut cef_load_handler_t,
    browser: *mut cef_browser_t,
    frame: *mut crate::cef::ffi::cef_frame_t,
    http_status_code: i32,
) {
    eprintln!(
        "CEF load_on_load_end: browser={browser:p} frame={frame:p} http_status_code={http_status_code}"
    );
    emit_lifecycle_event(CefLifecycleEvent::LoadEnd { http_status_code });
}

unsafe extern "C" fn load_on_load_error(
    _this: *mut cef_load_handler_t,
    browser: *mut cef_browser_t,
    frame: *mut crate::cef::ffi::cef_frame_t,
    error_code: i32,
    error_text: *const crate::cef::ffi::cef_string_t,
    failed_url: *const crate::cef::ffi::cef_string_t,
) {
    let error_text = if error_text.is_null() {
        String::new()
    } else {
        decode_cef_string(unsafe { &*error_text }).unwrap_or_else(|_| "<decode-failed>".to_string())
    };
    let failed_url = if failed_url.is_null() {
        String::new()
    } else {
        decode_cef_string(unsafe { &*failed_url }).unwrap_or_else(|_| "<decode-failed>".to_string())
    };
    eprintln!(
        "CEF load_on_load_error: browser={browser:p} frame={frame:p} error_code={} error_text={:?} failed_url={:?}",
        error_code, error_text, failed_url
    );
    emit_lifecycle_event(CefLifecycleEvent::LoadError {
        error_code,
        error_text,
        failed_url,
    });
}

unsafe extern "C" fn display_on_console_message(
    _this: *mut cef_display_handler_t,
    browser: *mut cef_browser_t,
    level: crate::cef::ffi::cef_log_severity_t,
    message: *const crate::cef::ffi::cef_string_t,
    source: *const crate::cef::ffi::cef_string_t,
    line: i32,
) -> i32 {
    let message = if message.is_null() {
        String::new()
    } else {
        decode_cef_string(unsafe { &*message }).unwrap_or_else(|_| "<decode-failed>".to_string())
    };
    let source = if source.is_null() {
        String::new()
    } else {
        decode_cef_string(unsafe { &*source }).unwrap_or_else(|_| "<decode-failed>".to_string())
    };
    if (level as i32) >= 2 && !trace_enabled() {
        return 0;
    }
    eprintln!(
        "CEF console: browser={browser:p} level={} source={:?}:{} message={:?}",
        level, source, line, message
    );
    emit_lifecycle_event(CefLifecycleEvent::Console {
        level: level as i32,
        source,
        line,
        message,
    });
    0
}
