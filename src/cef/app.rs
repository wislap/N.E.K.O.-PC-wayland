#![allow(dead_code)]

use std::cell::RefCell;
use std::mem;
use std::mem::offset_of;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::Result;

use crate::cef::ffi::{
    cef_app_t, cef_base_ref_counted_t, cef_browser_process_handler_t, cef_command_line_t,
    cef_string_t,
};
use crate::cef::strings::decode_cef_string;
use crate::cef::strings::CefOwnedString;

thread_local! {
    static PENDING_APP: RefCell<Option<CefAppState>> = const { RefCell::new(None) };
}

#[derive(Debug)]
pub struct CefAppState {
    app: NonNull<CefAppWrapper>,
}

impl CefAppState {
    pub fn new() -> Result<Self> {
        let browser_process_handler = CefBrowserProcessHandlerWrapper::new();
        let app = CefAppWrapper::new(browser_process_handler);
        Ok(Self {
            app: NonNull::new(Box::into_raw(app)).expect("cef app pointer should not be null"),
        })
    }

    pub fn raw_ptr(&self) -> *mut cef_app_t {
        self.app.as_ptr().cast::<cef_app_t>()
    }

    pub fn add_ref_for_cef(&self) {
        unsafe {
            app_add_ref_raw(self.raw_ptr());
        }
    }

    pub fn release_for_cef(&self) {
        unsafe {
            app_release_raw(self.raw_ptr());
        }
    }
}

pub fn stash_pending_app(app: CefAppState) {
    PENDING_APP.with(|slot| {
        *slot.borrow_mut() = Some(app);
    });
}

pub fn take_pending_app() -> Option<CefAppState> {
    PENDING_APP.with(|slot| slot.borrow_mut().take())
}

#[repr(C)]
struct CefBrowserProcessHandlerWrapper {
    raw: cef_browser_process_handler_t,
    ref_count: AtomicUsize,
}

impl CefBrowserProcessHandlerWrapper {
    fn new() -> Box<Self> {
        let mut raw: cef_browser_process_handler_t = unsafe { mem::zeroed() };
        raw.base = cef_base_ref_counted_t {
            size: mem::size_of::<cef_browser_process_handler_t>(),
            add_ref: Some(browser_process_handler_add_ref),
            release: Some(browser_process_handler_release),
            has_one_ref: Some(browser_process_handler_has_one_ref),
            has_at_least_one_ref: Some(browser_process_handler_has_at_least_one_ref),
        };
        raw.on_context_initialized = Some(browser_process_handler_on_context_initialized);
        raw.on_schedule_message_pump_work = Some(browser_process_handler_on_schedule_message_pump_work);
        raw.get_default_client = Some(browser_process_handler_get_default_client);

        Box::new(Self {
            raw,
            ref_count: AtomicUsize::new(1),
        })
    }

    fn raw_ptr(&mut self) -> *mut cef_browser_process_handler_t {
        &mut self.raw
    }
}

#[repr(C)]
struct CefAppWrapper {
    raw: cef_app_t,
    ref_count: AtomicUsize,
    browser_process_handler: NonNull<CefBrowserProcessHandlerWrapper>,
}

impl CefAppWrapper {
    fn new(
        mut browser_process_handler: Box<CefBrowserProcessHandlerWrapper>,
    ) -> Box<Self> {
        let handler_ptr = NonNull::new(browser_process_handler.raw_ptr())
            .expect("cef browser process handler pointer should not be null")
            .cast::<CefBrowserProcessHandlerWrapper>();
        let _ = Box::into_raw(browser_process_handler);

        let mut raw: cef_app_t = unsafe { mem::zeroed() };
        raw.base = cef_base_ref_counted_t {
            size: mem::size_of::<cef_app_t>(),
            add_ref: Some(app_add_ref),
            release: Some(app_release),
            has_one_ref: Some(app_has_one_ref),
            has_at_least_one_ref: Some(app_has_at_least_one_ref),
        };
        raw.on_before_command_line_processing = Some(app_on_before_command_line_processing);
        raw.get_browser_process_handler = Some(app_get_browser_process_handler);

        Box::new(Self {
            raw,
            ref_count: AtomicUsize::new(1),
            browser_process_handler: handler_ptr,
        })
    }
}

unsafe fn app_wrapper_from_raw<'a>(this: *mut cef_app_t) -> &'a mut CefAppWrapper {
    unsafe {
        let addr = this.cast::<u8>().sub(offset_of!(CefAppWrapper, raw));
        &mut *addr.cast::<CefAppWrapper>()
    }
}

unsafe fn app_wrapper_from_base<'a>(this: *mut cef_base_ref_counted_t) -> &'a mut CefAppWrapper {
    unsafe { app_wrapper_from_raw(this.cast::<cef_app_t>()) }
}

unsafe fn browser_process_handler_wrapper_from_raw<'a>(
    this: *mut cef_browser_process_handler_t,
) -> &'a mut CefBrowserProcessHandlerWrapper {
    unsafe {
        let addr = this
            .cast::<u8>()
            .sub(offset_of!(CefBrowserProcessHandlerWrapper, raw));
        &mut *addr.cast::<CefBrowserProcessHandlerWrapper>()
    }
}

unsafe fn browser_process_handler_wrapper_from_base<'a>(
    this: *mut cef_base_ref_counted_t,
) -> &'a mut CefBrowserProcessHandlerWrapper {
    unsafe { browser_process_handler_wrapper_from_raw(this.cast::<cef_browser_process_handler_t>()) }
}

unsafe fn app_add_ref_raw(ptr: *mut cef_app_t) {
    if ptr.is_null() {
        return;
    }
    let wrapper = unsafe { app_wrapper_from_raw(ptr) };
    wrapper.ref_count.fetch_add(1, Ordering::Relaxed);
}

unsafe fn app_release_raw(ptr: *mut cef_app_t) {
    if ptr.is_null() {
        return;
    }
    let base = ptr.cast::<cef_base_ref_counted_t>();
    unsafe {
        app_release(base);
    }
}

unsafe fn browser_process_handler_add_ref_raw(ptr: *mut cef_browser_process_handler_t) {
    if ptr.is_null() {
        return;
    }
    let wrapper = unsafe { browser_process_handler_wrapper_from_raw(ptr) };
    wrapper.ref_count.fetch_add(1, Ordering::Relaxed);
}

unsafe fn browser_process_handler_release_raw(ptr: *mut cef_browser_process_handler_t) {
    if ptr.is_null() {
        return;
    }
    let base = ptr.cast::<cef_base_ref_counted_t>();
    unsafe {
        browser_process_handler_release(base);
    }
}

unsafe extern "C" fn app_add_ref(this: *mut cef_base_ref_counted_t) {
    let wrapper = unsafe { app_wrapper_from_base(this) };
    wrapper.ref_count.fetch_add(1, Ordering::Relaxed);
}

unsafe extern "C" fn app_release(this: *mut cef_base_ref_counted_t) -> i32 {
    let wrapper = unsafe { app_wrapper_from_base(this) };
    let previous = wrapper.ref_count.fetch_sub(1, Ordering::Release);
    if previous == 1 {
        std::sync::atomic::fence(Ordering::Acquire);
        unsafe {
            browser_process_handler_release_raw(
                wrapper.browser_process_handler.as_ptr().cast::<cef_browser_process_handler_t>(),
            );
            drop(Box::from_raw(wrapper));
        }
        1
    } else {
        0
    }
}

unsafe extern "C" fn app_has_one_ref(this: *mut cef_base_ref_counted_t) -> i32 {
    let wrapper = unsafe { app_wrapper_from_base(this) };
    (wrapper.ref_count.load(Ordering::Acquire) == 1) as i32
}

unsafe extern "C" fn app_has_at_least_one_ref(this: *mut cef_base_ref_counted_t) -> i32 {
    let wrapper = unsafe { app_wrapper_from_base(this) };
    (wrapper.ref_count.load(Ordering::Acquire) >= 1) as i32
}

unsafe extern "C" fn app_get_browser_process_handler(
    this: *mut cef_app_t,
) -> *mut cef_browser_process_handler_t {
    let wrapper = unsafe { app_wrapper_from_raw(this) };
    let ptr = wrapper
        .browser_process_handler
        .as_ptr()
        .cast::<cef_browser_process_handler_t>();
    unsafe {
        browser_process_handler_add_ref_raw(ptr);
    }
    ptr
}

unsafe extern "C" fn app_on_before_command_line_processing(
    _this: *mut cef_app_t,
    _process_type: *const cef_string_t,
    command_line: *mut cef_command_line_t,
) {
    if command_line.is_null() {
        return;
    }

    let process_type = if _process_type.is_null() {
        String::new()
    } else {
        decode_cef_string(unsafe { &*_process_type }).unwrap_or_else(|_| "<decode-error>".into())
    };
    eprintln!("CEF app.on_before_command_line_processing process_type={process_type:?}");

    let lang_switch = CefOwnedString::new("lang");
    let lang_value = CefOwnedString::new(default_cef_lang());
    unsafe {
        append_switch(command_line, "no-sandbox");
        append_switch(command_line, "disable-gpu-sandbox");
        append_switch(command_line, "no-zygote");
        if let Some(append_switch_with_value) = (*command_line).append_switch_with_value {
            append_switch_with_value(command_line, lang_switch.as_cef(), lang_value.as_cef());
        }
        append_extra_switches_from_env(command_line);
        if let Some(has_switch) = (*command_line).has_switch {
            let applied = has_switch(command_line, lang_switch.as_cef());
            eprintln!("CEF app.on_before_command_line_processing lang_applied={applied}");
        }
    }
}

unsafe fn append_switch(command_line: *mut cef_command_line_t, name: &str) {
    if command_line.is_null() {
        return;
    }
    let switch = CefOwnedString::new(name);
    unsafe {
        if let Some(append_switch) = (*command_line).append_switch {
            append_switch(command_line, switch.as_cef());
        }
    }
}

unsafe fn append_switch_with_value(command_line: *mut cef_command_line_t, name: &str, value: &str) {
    if command_line.is_null() {
        return;
    }
    let switch = CefOwnedString::new(name);
    let value = CefOwnedString::new(value);
    unsafe {
        if let Some(append_switch_with_value) = (*command_line).append_switch_with_value {
            append_switch_with_value(command_line, switch.as_cef(), value.as_cef());
        }
    }
}

unsafe fn append_extra_switches_from_env(command_line: *mut cef_command_line_t) {
    let Some(raw) = std::env::var("NEKO_CEF_APPEND_SWITCHES").ok() else {
        return;
    };
    for part in raw.split_whitespace() {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some((name, value)) = trimmed.split_once('=') {
            unsafe { append_switch_with_value(command_line, name.trim_start_matches('-'), value) };
        } else {
            unsafe { append_switch(command_line, trimmed.trim_start_matches('-')) };
        }
    }
}

fn default_cef_lang() -> String {
    let raw = std::env::var("LC_ALL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| std::env::var("LANG").ok());
    let Some(raw) = raw else {
        return "en-US".to_string();
    };
    let normalized = raw
        .split('.')
        .next()
        .unwrap_or(raw.as_str())
        .split('@')
        .next()
        .unwrap_or(raw.as_str())
        .replace('_', "-");
    let trimmed = normalized.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("c") || trimmed.eq_ignore_ascii_case("posix") {
        "en-US".to_string()
    } else {
        trimmed.to_string()
    }
}

unsafe extern "C" fn browser_process_handler_add_ref(this: *mut cef_base_ref_counted_t) {
    let wrapper = unsafe { browser_process_handler_wrapper_from_base(this) };
    wrapper.ref_count.fetch_add(1, Ordering::Relaxed);
}

unsafe extern "C" fn browser_process_handler_release(this: *mut cef_base_ref_counted_t) -> i32 {
    let wrapper = unsafe { browser_process_handler_wrapper_from_base(this) };
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

unsafe extern "C" fn browser_process_handler_has_one_ref(
    this: *mut cef_base_ref_counted_t,
) -> i32 {
    let wrapper = unsafe { browser_process_handler_wrapper_from_base(this) };
    (wrapper.ref_count.load(Ordering::Acquire) == 1) as i32
}

unsafe extern "C" fn browser_process_handler_has_at_least_one_ref(
    this: *mut cef_base_ref_counted_t,
) -> i32 {
    let wrapper = unsafe { browser_process_handler_wrapper_from_base(this) };
    (wrapper.ref_count.load(Ordering::Acquire) >= 1) as i32
}

unsafe extern "C" fn browser_process_handler_on_context_initialized(
    _this: *mut cef_browser_process_handler_t,
) {
    eprintln!("CEF browser_process_handler.on_context_initialized");
}

unsafe extern "C" fn browser_process_handler_on_schedule_message_pump_work(
    _this: *mut cef_browser_process_handler_t,
    delay_ms: i64,
) {
    eprintln!(
        "CEF browser_process_handler.on_schedule_message_pump_work delay_ms={delay_ms}"
    );
}

unsafe extern "C" fn browser_process_handler_get_default_client(
    _this: *mut cef_browser_process_handler_t,
) -> *mut crate::cef::ffi::cef_client_t {
    std::ptr::null_mut()
}
