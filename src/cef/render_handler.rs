#![allow(dead_code)]

use std::mem::offset_of;
use std::slice;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Result, bail};

use crate::cef::client::BrowserHostShared;
use crate::cef::ffi::{
    cef_base_ref_counted_t, cef_browser_host_t, cef_browser_t, cef_paint_element_type_t,
    cef_rect_t, cef_render_handler_t, cef_screen_info_t,
};
use crate::cef::log::trace_enabled;
use crate::wayland::raw_host::{RawHostFrame, RawHostHandle};

#[derive(Debug)]
pub struct CefRenderHandlerState {
    width: u32,
    height: u32,
    transparent_painting: bool,
    raw_host: RawHostHandle,
    browser_host: BrowserHostShared,
}

impl CefRenderHandlerState {
    pub fn new(
        width: u32,
        height: u32,
        transparent_painting: bool,
        raw_host: RawHostHandle,
        browser_host: BrowserHostShared,
    ) -> Result<Self> {
        if width == 0 || height == 0 {
            bail!("CEF render handler requires non-zero dimensions");
        }

        Ok(Self {
            width,
            height,
            transparent_painting,
            raw_host,
            browser_host,
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn transparent_painting(&self) -> bool {
        self.transparent_painting
    }

    pub fn get_view_rect(&self) -> cef_rect_t {
        cef_rect_t {
            x: 0,
            y: 0,
            width: self.width as i32,
            height: self.height as i32,
        }
    }

    pub fn fill_screen_info(&self, screen_info: &mut cef_screen_info_t) {
        *screen_info = cef_screen_info_t {
            #[cfg(has_cef_sdk)]
            size: std::mem::size_of::<cef_screen_info_t>(),
            device_scale_factor: 1.0,
            depth: 32,
            depth_per_component: 8,
            is_monochrome: 0,
            rect: self.get_view_rect(),
            available_rect: self.get_view_rect(),
        };
    }

    pub fn handle_paint(
        &self,
        element_type: cef_paint_element_type_t,
        bgra: &[u8],
        width: u32,
        height: u32,
    ) -> Result<()> {
        if !matches!(element_type, cef_paint_element_type_t::PET_VIEW) {
            return Ok(());
        }

        log_first_paint_stats(bgra, width, height, self.transparent_painting);
        let frame = RawHostFrame::from_bgra(width, height, bgra.to_vec())?;
        self.raw_host.set_rgba_frame(frame)
    }

    pub fn capture_browser_if_needed(&self, browser: *mut cef_browser_t) {
        if browser.is_null() {
            return;
        }

        let mut state = self
            .browser_host
            .lock()
            .expect("browser host mutex poisoned");
        if state.browser.is_some() && state.host.is_some() {
            return;
        }

        let host = unsafe {
            (*browser)
                .get_host
                .and_then(|get_host| std::ptr::NonNull::new(get_host(browser)))
        };
        unsafe {
            base_add_ref(browser.cast::<cef_base_ref_counted_t>());
            if let Some(host_ptr) = host {
                base_add_ref(host_ptr.as_ptr().cast::<cef_base_ref_counted_t>());
            }
        }
        state.browser = std::ptr::NonNull::new(browser);
        state.host = host;
        eprintln!(
            "CEF render handler captured browser={browser:p} host={:p}",
            host.map(|value| value.as_ptr())
                .unwrap_or(std::ptr::null_mut::<cef_browser_host_t>())
        );
    }
}

#[repr(C)]
pub struct CefRenderHandlerWrapper {
    raw: cef_render_handler_t,
    ref_count: AtomicUsize,
    state: CefRenderHandlerState,
}

impl CefRenderHandlerWrapper {
    pub fn new(
        width: u32,
        height: u32,
        transparent_painting: bool,
        raw_host: RawHostHandle,
        browser_host: BrowserHostShared,
    ) -> Result<Box<Self>> {
        let state = CefRenderHandlerState::new(
            width,
            height,
            transparent_painting,
            raw_host,
            browser_host,
        )?;
        Ok(Box::new(Self {
            raw: cef_render_handler_t {
                base: cef_base_ref_counted_t {
                    size: std::mem::size_of::<cef_render_handler_t>(),
                    add_ref: Some(render_handler_add_ref),
                    release: Some(render_handler_release),
                    has_one_ref: Some(render_handler_has_one_ref),
                    has_at_least_one_ref: Some(render_handler_has_at_least_one_ref),
                },
                get_accessibility_handler: None,
                get_root_screen_rect: None,
                get_view_rect: Some(render_handler_get_view_rect),
                get_screen_point: None,
                get_screen_info: Some(render_handler_get_screen_info),
                on_popup_show: None,
                on_popup_size: None,
                on_paint: Some(render_handler_on_paint),
                on_accelerated_paint: None,
                get_touch_handle_size: None,
                on_touch_handle_state_changed: None,
                start_dragging: None,
                update_drag_cursor: None,
                on_scroll_offset_changed: None,
                on_ime_composition_range_changed: None,
                on_text_selection_changed: None,
                on_virtual_keyboard_requested: None,
            },
            ref_count: AtomicUsize::new(1),
            state,
        }))
    }

    pub fn raw_ptr(&mut self) -> *mut cef_render_handler_t {
        &mut self.raw
    }

    pub fn state(&self) -> &CefRenderHandlerState {
        &self.state
    }
}

pub unsafe fn add_ref_raw(ptr: *mut cef_render_handler_t) {
    if ptr.is_null() {
        return;
    }
    let wrapper = unsafe { wrapper_from_raw(ptr) };
    let previous = wrapper.ref_count.fetch_add(1, Ordering::Relaxed);
    if trace_enabled() {
        eprintln!(
            "CEF render_handler_add_ref_raw: wrapper={:p} previous={} new={}",
            wrapper,
            previous,
            previous + 1
        );
    }
}

unsafe fn wrapper_from_raw<'a>(this: *mut cef_render_handler_t) -> &'a mut CefRenderHandlerWrapper {
    unsafe {
        let addr = (this.cast::<u8>()).sub(offset_of!(CefRenderHandlerWrapper, raw));
        &mut *addr.cast::<CefRenderHandlerWrapper>()
    }
}

unsafe fn wrapper_from_base<'a>(
    this: *mut cef_base_ref_counted_t,
) -> &'a mut CefRenderHandlerWrapper {
    unsafe {
        let raw = this.cast::<cef_render_handler_t>();
        wrapper_from_raw(raw)
    }
}

unsafe extern "C" fn render_handler_add_ref(this: *mut cef_base_ref_counted_t) {
    let wrapper = unsafe { wrapper_from_base(this) };
    let previous = wrapper.ref_count.fetch_add(1, Ordering::Relaxed);
    if trace_enabled() {
        eprintln!(
            "CEF render_handler_add_ref: wrapper={:p} previous={} new={}",
            wrapper,
            previous,
            previous + 1
        );
    }
}

unsafe extern "C" fn render_handler_release(this: *mut cef_base_ref_counted_t) -> i32 {
    let wrapper = unsafe { wrapper_from_base(this) };
    let previous = wrapper.ref_count.fetch_sub(1, Ordering::Release);
    if trace_enabled() {
        eprintln!(
            "CEF render_handler_release: wrapper={:p} previous={} new={}",
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

unsafe extern "C" fn render_handler_has_one_ref(this: *mut cef_base_ref_counted_t) -> i32 {
    let wrapper = this.cast::<CefRenderHandlerWrapper>();
    (unsafe { (*wrapper).ref_count.load(Ordering::Acquire) } == 1) as i32
}

unsafe extern "C" fn render_handler_has_at_least_one_ref(this: *mut cef_base_ref_counted_t) -> i32 {
    let wrapper = this.cast::<CefRenderHandlerWrapper>();
    (unsafe { (*wrapper).ref_count.load(Ordering::Acquire) } > 0) as i32
}

#[cfg(has_cef_sdk)]
unsafe extern "C" fn render_handler_get_view_rect(
    this: *mut cef_render_handler_t,
    browser: *mut cef_browser_t,
    rect: *mut cef_rect_t,
)
{
    render_handler_get_view_rect_impl(this, browser, rect);
}

#[cfg(not(has_cef_sdk))]
unsafe extern "C" fn render_handler_get_view_rect(
    this: *mut cef_render_handler_t,
    browser: *mut cef_browser_t,
    rect: *mut cef_rect_t,
) -> i32
{
    render_handler_get_view_rect_impl(this, browser, rect)
}

fn render_handler_get_view_rect_impl(
    this: *mut cef_render_handler_t,
    browser: *mut cef_browser_t,
    rect: *mut cef_rect_t,
) -> i32 {
    if trace_enabled() {
        eprintln!("CEF render_handler_get_view_rect: this={this:p} rect={rect:p}");
    }
    if rect.is_null() {
        return 0;
    }

    let wrapper = unsafe { wrapper_from_raw(this) };
    wrapper.state.capture_browser_if_needed(browser);
    unsafe {
        *rect = wrapper.state.get_view_rect();
    }
    1
}

unsafe extern "C" fn render_handler_get_screen_info(
    this: *mut cef_render_handler_t,
    browser: *mut cef_browser_t,
    screen_info: *mut cef_screen_info_t,
) -> i32 {
    if trace_enabled() {
        eprintln!("CEF render_handler_get_screen_info: this={this:p} screen_info={screen_info:p}");
    }
    if screen_info.is_null() {
        return 0;
    }

    let wrapper = unsafe { wrapper_from_raw(this) };
    wrapper.state.capture_browser_if_needed(browser);
    unsafe {
        wrapper.state.fill_screen_info(&mut *screen_info);
    }
    1
}

unsafe extern "C" fn render_handler_on_paint(
    this: *mut cef_render_handler_t,
    browser: *mut cef_browser_t,
    element_type: cef_paint_element_type_t,
    _dirty_rects_count: usize,
    _dirty_rects: *const cef_rect_t,
    buffer: *const std::ffi::c_void,
    width: i32,
    height: i32,
) {
    if buffer.is_null() || width <= 0 || height <= 0 {
        return;
    }

    if trace_enabled() {
        eprintln!(
            "CEF render_handler_on_paint: this={this:p} element={:?} size={}x{}",
            element_type, width, height
        );
    }

    let wrapper = unsafe { wrapper_from_raw(this) };
    wrapper.state.capture_browser_if_needed(browser);
    let len = width as usize * height as usize * 4;
    let bgra = unsafe { slice::from_raw_parts(buffer.cast::<u8>(), len) };
    if let Err(err) = wrapper
        .state
        .handle_paint(element_type, bgra, width as u32, height as u32)
    {
        eprintln!("CEF render handler on_paint failed: {err:#}");
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

fn log_first_paint_stats(bgra: &[u8], width: u32, height: u32, transparent_painting: bool) {
    static ONCE: OnceLock<()> = OnceLock::new();
    if ONCE.set(()).is_err() {
        return;
    }

    let mut non_zero_alpha = 0usize;
    let mut opaque_alpha = 0usize;
    for px in bgra.chunks_exact(4) {
        let alpha = px[3];
        if alpha != 0 {
            non_zero_alpha += 1;
        }
        if alpha == 255 {
            opaque_alpha += 1;
        }
    }

    let sample = bgra
        .chunks_exact(4)
        .take(4)
        .map(|px| format!("[b={},g={},r={},a={}]", px[0], px[1], px[2], px[3]))
        .collect::<Vec<_>>()
        .join(" ");

    eprintln!(
        "CEF first paint stats: size={}x{} transparent={} non_zero_alpha={}/{} opaque_alpha={} sample={}",
        width,
        height,
        transparent_painting,
        non_zero_alpha,
        (width as usize) * (height as usize),
        opaque_alpha,
        sample
    );
}
