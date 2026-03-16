#![allow(dead_code)]

use anyhow::{Result, bail};

use crate::cef::client::CefClientState;
#[cfg(has_cef_sdk)]
use crate::cef::ffi::{
    cef_browser_host_create_browser, cef_browser_settings_t, cef_request_context_t,
    cef_window_info_t,
};
#[cfg(not(has_cef_sdk))]
use crate::cef::ffi::{cef_browser_settings_t, cef_window_info_t};
use crate::cef::strings::CefOwnedString;

#[derive(Debug)]
pub struct CefWindowInfoBuilder {
    width: u32,
    height: u32,
    window_name: String,
    hidden: bool,
    shared_texture_enabled: bool,
    external_begin_frame_enabled: bool,
    windowless_rendering_enabled: bool,
}

impl CefWindowInfoBuilder {
    pub fn windowless(width: u32, height: u32, window_name: impl Into<String>) -> Self {
        Self {
            width,
            height,
            window_name: window_name.into(),
            hidden: false,
            shared_texture_enabled: false,
            external_begin_frame_enabled: false,
            windowless_rendering_enabled: true,
        }
    }

    pub fn build(self) -> cef_window_info_t {
        cef_window_info_t {
            size: std::mem::size_of::<cef_window_info_t>(),
            window_name: CefOwnedString::new(self.window_name).into_raw(),
            bounds: crate::cef::ffi::cef_rect_t {
                x: 0,
                y: 0,
                width: self.width as i32,
                height: self.height as i32,
            },
            parent_window: 0,
            windowless_rendering_enabled: self.windowless_rendering_enabled as i32,
            shared_texture_enabled: self.shared_texture_enabled as i32,
            external_begin_frame_enabled: self.external_begin_frame_enabled as i32,
            window: 0,
            runtime_style: 0,
        }
    }
}

#[derive(Debug)]
pub struct CefBrowserSettingsBuilder {
    width: u32,
    height: u32,
    frame_rate: u32,
    background_color: u32,
}

impl CefBrowserSettingsBuilder {
    pub fn windowless(
        width: u32,
        height: u32,
        frame_rate: u32,
        transparent_painting: bool,
    ) -> Self {
        let background_color = if transparent_painting {
            0x00000000
        } else {
            0xFFFF_FFFF
        };
        Self {
            width,
            height,
            frame_rate,
            background_color,
        }
    }

    pub fn build(self) -> cef_browser_settings_t {
        cef_browser_settings_t {
            size: std::mem::size_of::<cef_browser_settings_t>(),
            windowless_frame_rate: self.frame_rate as i32,
            standard_font_family: CefOwnedString::new("").into_raw(),
            fixed_font_family: CefOwnedString::new("").into_raw(),
            serif_font_family: CefOwnedString::new("").into_raw(),
            sans_serif_font_family: CefOwnedString::new("").into_raw(),
            cursive_font_family: CefOwnedString::new("").into_raw(),
            fantasy_font_family: CefOwnedString::new("").into_raw(),
            default_font_size: 16,
            default_fixed_font_size: 13,
            minimum_font_size: 0,
            minimum_logical_font_size: 0,
            default_encoding: CefOwnedString::new("utf-8").into_raw(),
            remote_fonts: 1,
            javascript: 1,
            javascript_close_windows: 1,
            javascript_access_clipboard: 0,
            javascript_dom_paste: 0,
            image_loading: 1,
            image_shrink_standalone_to_fit: 1,
            text_area_resize: 1,
            tab_to_links: 1,
            local_storage: 1,
            databases_deprecated: 1,
            webgl: 1,
            background_color: self.background_color,
            chrome_status_bubble: 0,
            chrome_zoom_bubble: 0,
        }
    }

    pub fn view_size(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}

pub fn create_windowless_browser(
    client: &mut CefClientState,
    url: &str,
    width: u32,
    height: u32,
    frame_rate: u32,
    transparent_painting: bool,
) -> Result<()> {
    if width == 0 || height == 0 {
        bail!("windowless CEF browser requires non-zero width and height");
    }

    if frame_rate == 0 {
        bail!("windowless CEF browser requires non-zero frame rate");
    }

    #[cfg(not(has_cef_sdk))]
    {
        let _ = (client, url, width, height, frame_rate, transparent_painting);
        bail!("cannot create windowless CEF browser because no local CEF SDK was discovered");
    }

    #[cfg(has_cef_sdk)]
    {
        eprintln!(
            "CEF create_windowless_browser: url={} size={}x{} fps={} transparent={}",
            url, width, height, frame_rate, transparent_painting
        );
        let window_info = CefWindowInfoBuilder::windowless(width, height, "neko-cef-osr").build();
        let browser_settings =
            CefBrowserSettingsBuilder::windowless(width, height, frame_rate, transparent_painting)
                .build();
        let url = CefOwnedString::new(url);

        eprintln!(
            "CEF create_windowless_browser: window_info.size={} browser_settings.size={} client_ptr={:p}",
            window_info.size,
            browser_settings.size,
            client.raw_ptr()
        );

        client.add_ref_for_cef();

        let result = unsafe {
            cef_browser_host_create_browser(
                &window_info,
                client.raw_ptr(),
                url.as_cef(),
                &browser_settings,
                std::ptr::null_mut(),
                std::ptr::null_mut::<cef_request_context_t>(),
            )
        };

        eprintln!(
            "CEF create_windowless_browser: cef_browser_host_create_browser returned {}",
            result
        );

        if result == 0 {
            client.release_for_cef();
            bail!("cef_browser_host_create_browser returned 0");
        }

        Ok(())
    }
}
