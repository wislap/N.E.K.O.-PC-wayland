#![allow(dead_code, non_camel_case_types)]

use std::ffi::{c_char, c_int, c_void};

pub type cef_transition_type_t = c_int;
pub type cef_log_severity_t = c_int;

#[derive(Debug)]
#[repr(C)]
pub struct cef_base_ref_counted_t {
    pub size: usize,
    pub add_ref: Option<unsafe extern "C" fn(this: *mut cef_base_ref_counted_t)>,
    pub release: Option<unsafe extern "C" fn(this: *mut cef_base_ref_counted_t) -> c_int>,
    pub has_one_ref: Option<unsafe extern "C" fn(this: *mut cef_base_ref_counted_t) -> c_int>,
    pub has_at_least_one_ref:
        Option<unsafe extern "C" fn(this: *mut cef_base_ref_counted_t) -> c_int>,
}

#[derive(Debug)]
#[repr(C)]
pub struct cef_main_args_t {
    pub argc: c_int,
    pub argv: *mut *mut c_char,
}

#[derive(Debug)]
#[repr(C)]
pub struct cef_string_t {
    pub str_: *mut u16,
    pub length: usize,
    pub dtor: Option<unsafe extern "C" fn(str_: *mut u16)>,
}

#[derive(Debug)]
#[repr(C)]
pub struct cef_settings_t {
    pub size: usize,
    pub no_sandbox: c_int,
    pub browser_subprocess_path: cef_string_t,
    pub framework_dir_path: cef_string_t,
    pub main_bundle_path: cef_string_t,
    pub multi_threaded_message_loop: c_int,
    pub external_message_pump: c_int,
    pub windowless_rendering_enabled: c_int,
    pub command_line_args_disabled: c_int,
    pub cache_path: cef_string_t,
    pub root_cache_path: cef_string_t,
    pub persist_session_cookies: c_int,
    pub user_agent: cef_string_t,
    pub user_agent_product: cef_string_t,
    pub locale: cef_string_t,
    pub log_file: cef_string_t,
    pub log_severity: c_int,
    pub log_items: c_int,
    pub javascript_flags: cef_string_t,
    pub resources_dir_path: cef_string_t,
    pub locales_dir_path: cef_string_t,
    pub remote_debugging_port: c_int,
    pub uncaught_exception_stack_size: c_int,
    pub background_color: u32,
    pub accept_language_list: cef_string_t,
    pub cookieable_schemes_list: cef_string_t,
    pub cookieable_schemes_exclude_defaults: c_int,
    pub chrome_policy_id: cef_string_t,
    pub chrome_app_icon_id: c_int,
    pub disable_signal_handlers: c_int,
}

#[derive(Debug)]
#[repr(C)]
pub struct cef_app_t {
    pub base: cef_base_ref_counted_t,
    pub on_before_command_line_processing: Option<
        unsafe extern "C" fn(
            self_: *mut cef_app_t,
            process_type: *const cef_string_t,
            command_line: *mut c_void,
        ),
    >,
    pub on_register_custom_schemes:
        Option<unsafe extern "C" fn(self_: *mut cef_app_t, registrar: *mut c_void)>,
    pub get_resource_bundle_handler:
        Option<unsafe extern "C" fn(self_: *mut cef_app_t) -> *mut c_void>,
    pub get_browser_process_handler:
        Option<unsafe extern "C" fn(self_: *mut cef_app_t) -> *mut c_void>,
    pub get_render_process_handler:
        Option<unsafe extern "C" fn(self_: *mut cef_app_t) -> *mut c_void>,
}

#[derive(Debug)]
#[repr(C)]
pub struct cef_display_handler_t {
    pub base: cef_base_ref_counted_t,
    pub on_address_change: Option<unsafe extern "C" fn()>,
    pub on_title_change: Option<unsafe extern "C" fn()>,
    pub on_favicon_urlchange: Option<unsafe extern "C" fn()>,
    pub on_fullscreen_mode_change: Option<unsafe extern "C" fn()>,
    pub on_tooltip: Option<unsafe extern "C" fn() -> c_int>,
    pub on_status_message: Option<unsafe extern "C" fn()>,
    pub on_console_message: Option<
        unsafe extern "C" fn(
            self_: *mut cef_display_handler_t,
            browser: *mut cef_browser_t,
            level: c_int,
            message: *const cef_string_t,
            source: *const cef_string_t,
            line: c_int,
        ) -> c_int,
    >,
    pub on_auto_resize: Option<unsafe extern "C" fn() -> c_int>,
    pub on_loading_progress_change: Option<unsafe extern "C" fn()>,
    pub on_cursor_change: Option<unsafe extern "C" fn() -> c_int>,
    pub on_media_access_change: Option<unsafe extern "C" fn()>,
    pub on_contents_bounds_change: Option<unsafe extern "C" fn() -> c_int>,
    pub get_root_window_screen_rect: Option<unsafe extern "C" fn() -> c_int>,
}

#[derive(Debug)]
#[repr(C)]
pub struct cef_browser_t {
    pub base: cef_base_ref_counted_t,
    pub is_valid: Option<unsafe extern "C" fn(self_: *mut cef_browser_t) -> c_int>,
    pub get_host:
        Option<unsafe extern "C" fn(self_: *mut cef_browser_t) -> *mut cef_browser_host_t>,
}

#[derive(Debug)]
#[repr(C)]
pub struct cef_browser_host_t {
    pub base: cef_base_ref_counted_t,
    pub get_browser:
        Option<unsafe extern "C" fn(self_: *mut cef_browser_host_t) -> *mut cef_browser_t>,
    pub close_browser:
        Option<unsafe extern "C" fn(self_: *mut cef_browser_host_t, force_close: c_int)>,
    pub try_close_browser: Option<unsafe extern "C" fn(self_: *mut cef_browser_host_t) -> c_int>,
    pub is_ready_to_be_closed:
        Option<unsafe extern "C" fn(self_: *mut cef_browser_host_t) -> c_int>,
    pub set_focus: Option<unsafe extern "C" fn(self_: *mut cef_browser_host_t, focus: c_int)>,
    pub get_window_handle: Option<unsafe extern "C" fn(self_: *mut cef_browser_host_t) -> usize>,
    pub get_opener_window_handle:
        Option<unsafe extern "C" fn(self_: *mut cef_browser_host_t) -> usize>,
    pub get_opener_identifier:
        Option<unsafe extern "C" fn(self_: *mut cef_browser_host_t) -> c_int>,
    pub has_view: Option<unsafe extern "C" fn(self_: *mut cef_browser_host_t) -> c_int>,
    pub get_client:
        Option<unsafe extern "C" fn(self_: *mut cef_browser_host_t) -> *mut cef_client_t>,
    pub get_request_context:
        Option<unsafe extern "C" fn(self_: *mut cef_browser_host_t) -> *mut cef_request_context_t>,
    pub can_zoom:
        Option<unsafe extern "C" fn(self_: *mut cef_browser_host_t, command: c_int) -> c_int>,
    pub zoom: Option<unsafe extern "C" fn(self_: *mut cef_browser_host_t, command: c_int)>,
    pub get_default_zoom_level: Option<unsafe extern "C" fn(self_: *mut cef_browser_host_t) -> f64>,
    pub get_zoom_level: Option<unsafe extern "C" fn(self_: *mut cef_browser_host_t) -> f64>,
    pub set_zoom_level:
        Option<unsafe extern "C" fn(self_: *mut cef_browser_host_t, zoom_level: f64)>,
    pub run_file_dialog: Option<unsafe extern "C" fn()>,
    pub start_download: Option<unsafe extern "C" fn()>,
    pub download_image: Option<unsafe extern "C" fn()>,
    pub print: Option<unsafe extern "C" fn()>,
    pub print_to_pdf: Option<unsafe extern "C" fn()>,
    pub find: Option<unsafe extern "C" fn()>,
    pub stop_finding: Option<unsafe extern "C" fn()>,
    pub show_dev_tools: Option<unsafe extern "C" fn()>,
    pub close_dev_tools: Option<unsafe extern "C" fn(self_: *mut cef_browser_host_t)>,
    pub has_dev_tools: Option<unsafe extern "C" fn(self_: *mut cef_browser_host_t) -> c_int>,
    pub send_dev_tools_message: Option<unsafe extern "C" fn() -> c_int>,
    pub execute_dev_tools_method: Option<unsafe extern "C" fn() -> c_int>,
    pub add_dev_tools_message_observer: Option<unsafe extern "C" fn() -> *mut c_void>,
    pub get_navigation_entries: Option<unsafe extern "C" fn()>,
    pub replace_misspelling: Option<unsafe extern "C" fn()>,
    pub add_word_to_dictionary: Option<unsafe extern "C" fn()>,
    pub is_window_rendering_disabled:
        Option<unsafe extern "C" fn(self_: *mut cef_browser_host_t) -> c_int>,
    pub was_resized: Option<unsafe extern "C" fn(self_: *mut cef_browser_host_t)>,
    pub was_hidden: Option<unsafe extern "C" fn(self_: *mut cef_browser_host_t, hidden: c_int)>,
    pub notify_screen_info_changed: Option<unsafe extern "C" fn(self_: *mut cef_browser_host_t)>,
    pub invalidate: Option<
        unsafe extern "C" fn(self_: *mut cef_browser_host_t, type_: cef_paint_element_type_t),
    >,
    pub send_external_begin_frame: Option<unsafe extern "C" fn(self_: *mut cef_browser_host_t)>,
    pub send_key_event:
        Option<unsafe extern "C" fn(self_: *mut cef_browser_host_t, event: *const cef_key_event_t)>,
    pub send_mouse_click_event: Option<
        unsafe extern "C" fn(
            self_: *mut cef_browser_host_t,
            event: *const cef_mouse_event_t,
            type_: cef_mouse_button_type_t,
            mouse_up: c_int,
            click_count: c_int,
        ),
    >,
    pub send_mouse_move_event: Option<
        unsafe extern "C" fn(
            self_: *mut cef_browser_host_t,
            event: *const cef_mouse_event_t,
            mouse_leave: c_int,
        ),
    >,
    pub send_mouse_wheel_event: Option<
        unsafe extern "C" fn(
            self_: *mut cef_browser_host_t,
            event: *const cef_mouse_event_t,
            delta_x: c_int,
            delta_y: c_int,
        ),
    >,
    pub send_touch_event: Option<unsafe extern "C" fn()>,
    pub send_capture_lost_event: Option<unsafe extern "C" fn(self_: *mut cef_browser_host_t)>,
    pub notify_move_or_resize_started: Option<unsafe extern "C" fn(self_: *mut cef_browser_host_t)>,
    pub get_windowless_frame_rate:
        Option<unsafe extern "C" fn(self_: *mut cef_browser_host_t) -> c_int>,
    pub set_windowless_frame_rate:
        Option<unsafe extern "C" fn(self_: *mut cef_browser_host_t, frame_rate: c_int)>,
}

#[derive(Debug)]
#[repr(C)]
pub struct cef_life_span_handler_t {
    pub base: cef_base_ref_counted_t,
    pub on_before_popup: Option<unsafe extern "C" fn() -> c_int>,
    pub on_before_popup_aborted: Option<unsafe extern "C" fn()>,
    pub on_before_dev_tools_popup: Option<unsafe extern "C" fn()>,
    pub on_after_created: Option<
        unsafe extern "C" fn(self_: *mut cef_life_span_handler_t, browser: *mut cef_browser_t),
    >,
    pub do_close: Option<unsafe extern "C" fn() -> c_int>,
    pub on_before_close: Option<
        unsafe extern "C" fn(self_: *mut cef_life_span_handler_t, browser: *mut cef_browser_t),
    >,
}

#[derive(Debug)]
#[repr(C)]
pub struct cef_load_handler_t {
    pub base: cef_base_ref_counted_t,
    pub on_loading_state_change: Option<
        unsafe extern "C" fn(
            self_: *mut cef_load_handler_t,
            browser: *mut cef_browser_t,
            is_loading: c_int,
            can_go_back: c_int,
            can_go_forward: c_int,
        ),
    >,
    pub on_load_start: Option<
        unsafe extern "C" fn(
            self_: *mut cef_load_handler_t,
            browser: *mut cef_browser_t,
            frame: *mut cef_frame_t,
            transition_type: c_int,
        ),
    >,
    pub on_load_end: Option<
        unsafe extern "C" fn(
            self_: *mut cef_load_handler_t,
            browser: *mut cef_browser_t,
            frame: *mut cef_frame_t,
            http_status_code: c_int,
        ),
    >,
    pub on_load_error: Option<
        unsafe extern "C" fn(
            self_: *mut cef_load_handler_t,
            browser: *mut cef_browser_t,
            frame: *mut cef_frame_t,
            error_code: c_int,
            error_text: *const cef_string_t,
            failed_url: *const cef_string_t,
        ),
    >,
}

#[derive(Debug)]
#[repr(C)]
pub struct cef_frame_t {
    pub base: cef_base_ref_counted_t,
}

#[derive(Debug)]
#[repr(C)]
pub struct cef_render_handler_t {
    pub base: cef_base_ref_counted_t,
    pub get_accessibility_handler:
        Option<unsafe extern "C" fn(this: *mut cef_render_handler_t) -> *mut c_void>,
    pub get_root_screen_rect: Option<
        unsafe extern "C" fn(
            this: *mut cef_render_handler_t,
            browser: *mut cef_browser_t,
            rect: *mut cef_rect_t,
        ) -> c_int,
    >,
    pub get_view_rect: Option<
        unsafe extern "C" fn(
            this: *mut cef_render_handler_t,
            browser: *mut cef_browser_t,
            rect: *mut cef_rect_t,
        ) -> c_int,
    >,
    pub get_screen_point: Option<
        unsafe extern "C" fn(
            this: *mut cef_render_handler_t,
            browser: *mut cef_browser_t,
            view_x: c_int,
            view_y: c_int,
            screen_x: *mut c_int,
            screen_y: *mut c_int,
        ) -> c_int,
    >,
    pub get_screen_info: Option<
        unsafe extern "C" fn(
            this: *mut cef_render_handler_t,
            browser: *mut cef_browser_t,
            screen_info: *mut cef_screen_info_t,
        ) -> c_int,
    >,
    pub on_popup_show: Option<
        unsafe extern "C" fn(
            this: *mut cef_render_handler_t,
            browser: *mut cef_browser_t,
            show: c_int,
        ),
    >,
    pub on_popup_size: Option<
        unsafe extern "C" fn(
            this: *mut cef_render_handler_t,
            browser: *mut cef_browser_t,
            rect: *const cef_rect_t,
        ),
    >,
    pub on_paint: Option<
        unsafe extern "C" fn(
            this: *mut cef_render_handler_t,
            browser: *mut cef_browser_t,
            element_type: cef_paint_element_type_t,
            dirty_rects_count: usize,
            dirty_rects: *const cef_rect_t,
            buffer: *const c_void,
            width: c_int,
            height: c_int,
        ),
    >,
    pub on_accelerated_paint: Option<unsafe extern "C" fn()>,
    pub get_touch_handle_size: Option<unsafe extern "C" fn()>,
    pub on_touch_handle_state_changed: Option<unsafe extern "C" fn()>,
    pub start_dragging: Option<unsafe extern "C" fn() -> c_int>,
    pub update_drag_cursor: Option<unsafe extern "C" fn()>,
    pub on_scroll_offset_changed: Option<
        unsafe extern "C" fn(
            this: *mut cef_render_handler_t,
            browser: *mut cef_browser_t,
            x: f64,
            y: f64,
        ),
    >,
    pub on_ime_composition_range_changed: Option<unsafe extern "C" fn()>,
    pub on_text_selection_changed: Option<unsafe extern "C" fn()>,
    pub on_virtual_keyboard_requested: Option<unsafe extern "C" fn()>,
}

#[derive(Debug)]
#[repr(C)]
pub struct cef_client_t {
    pub base: cef_base_ref_counted_t,
    pub get_audio_handler: Option<unsafe extern "C" fn(this: *mut cef_client_t) -> *mut c_void>,
    pub get_command_handler: Option<unsafe extern "C" fn(this: *mut cef_client_t) -> *mut c_void>,
    pub get_context_menu_handler:
        Option<unsafe extern "C" fn(this: *mut cef_client_t) -> *mut c_void>,
    pub get_dialog_handler: Option<unsafe extern "C" fn(this: *mut cef_client_t) -> *mut c_void>,
    pub get_display_handler:
        Option<unsafe extern "C" fn(this: *mut cef_client_t) -> *mut cef_display_handler_t>,
    pub get_download_handler: Option<unsafe extern "C" fn(this: *mut cef_client_t) -> *mut c_void>,
    pub get_drag_handler: Option<unsafe extern "C" fn(this: *mut cef_client_t) -> *mut c_void>,
    pub get_find_handler: Option<unsafe extern "C" fn(this: *mut cef_client_t) -> *mut c_void>,
    pub get_focus_handler: Option<unsafe extern "C" fn(this: *mut cef_client_t) -> *mut c_void>,
    pub get_frame_handler: Option<unsafe extern "C" fn(this: *mut cef_client_t) -> *mut c_void>,
    pub get_permission_handler:
        Option<unsafe extern "C" fn(this: *mut cef_client_t) -> *mut c_void>,
    pub get_jsdialog_handler: Option<unsafe extern "C" fn(this: *mut cef_client_t) -> *mut c_void>,
    pub get_keyboard_handler: Option<unsafe extern "C" fn(this: *mut cef_client_t) -> *mut c_void>,
    pub get_life_span_handler:
        Option<unsafe extern "C" fn(this: *mut cef_client_t) -> *mut cef_life_span_handler_t>,
    pub get_load_handler:
        Option<unsafe extern "C" fn(this: *mut cef_client_t) -> *mut cef_load_handler_t>,
    pub get_print_handler: Option<unsafe extern "C" fn(this: *mut cef_client_t) -> *mut c_void>,
    pub get_render_handler:
        Option<unsafe extern "C" fn(this: *mut cef_client_t) -> *mut cef_render_handler_t>,
    pub get_request_handler: Option<unsafe extern "C" fn(this: *mut cef_client_t) -> *mut c_void>,
    pub on_process_message_received: Option<unsafe extern "C" fn() -> c_int>,
}

#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct cef_rect_t {
    pub x: c_int,
    pub y: c_int,
    pub width: c_int,
    pub height: c_int,
}

#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct cef_screen_info_t {
    pub device_scale_factor: f32,
    pub depth: c_int,
    pub depth_per_component: c_int,
    pub is_monochrome: c_int,
    pub rect: cef_rect_t,
    pub available_rect: cef_rect_t,
}

#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub enum cef_paint_element_type_t {
    PET_VIEW = 0,
    PET_POPUP = 1,
}

#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub enum cef_mouse_button_type_t {
    MBT_LEFT = 0,
    MBT_MIDDLE = 1,
    MBT_RIGHT = 2,
}

#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct cef_mouse_event_t {
    pub x: c_int,
    pub y: c_int,
    pub modifiers: u32,
}

pub const EVENTFLAG_NONE: u32 = 0;
pub const EVENTFLAG_LEFT_MOUSE_BUTTON: u32 = 1 << 4;
pub const EVENTFLAG_MIDDLE_MOUSE_BUTTON: u32 = 1 << 5;
pub const EVENTFLAG_RIGHT_MOUSE_BUTTON: u32 = 1 << 6;
pub const EVENTFLAG_PRECISION_SCROLLING_DELTA: u32 = 1 << 14;
pub const EVENTFLAG_CAPS_LOCK_ON: u32 = 1 << 0;
pub const EVENTFLAG_SHIFT_DOWN: u32 = 1 << 1;
pub const EVENTFLAG_CONTROL_DOWN: u32 = 1 << 2;
pub const EVENTFLAG_ALT_DOWN: u32 = 1 << 3;
pub const EVENTFLAG_COMMAND_DOWN: u32 = 1 << 7;
pub const EVENTFLAG_NUM_LOCK_ON: u32 = 1 << 8;
pub const EVENTFLAG_IS_KEY_PAD: u32 = 1 << 9;
pub const EVENTFLAG_IS_REPEAT: u32 = 1 << 13;

#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub enum cef_key_event_type_t {
    KEYEVENT_RAWKEYDOWN = 0,
    KEYEVENT_KEYDOWN = 1,
    KEYEVENT_KEYUP = 2,
    KEYEVENT_CHAR = 3,
}

#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct cef_key_event_t {
    pub size: usize,
    pub type_: cef_key_event_type_t,
    pub modifiers: u32,
    pub windows_key_code: c_int,
    pub native_key_code: c_int,
    pub is_system_key: c_int,
    pub character: u16,
    pub unmodified_character: u16,
    pub focus_on_editable_field: c_int,
}

#[derive(Debug)]
#[repr(C)]
pub struct cef_browser_settings_t {
    pub size: usize,
    pub windowless_frame_rate: c_int,
    pub standard_font_family: cef_string_t,
    pub fixed_font_family: cef_string_t,
    pub serif_font_family: cef_string_t,
    pub sans_serif_font_family: cef_string_t,
    pub cursive_font_family: cef_string_t,
    pub fantasy_font_family: cef_string_t,
    pub default_font_size: c_int,
    pub default_fixed_font_size: c_int,
    pub minimum_font_size: c_int,
    pub minimum_logical_font_size: c_int,
    pub default_encoding: cef_string_t,
    pub remote_fonts: c_int,
    pub javascript: c_int,
    pub javascript_close_windows: c_int,
    pub javascript_access_clipboard: c_int,
    pub javascript_dom_paste: c_int,
    pub image_loading: c_int,
    pub image_shrink_standalone_to_fit: c_int,
    pub text_area_resize: c_int,
    pub tab_to_links: c_int,
    pub local_storage: c_int,
    pub databases_deprecated: c_int,
    pub webgl: c_int,
    pub background_color: u32,
    pub chrome_status_bubble: c_int,
    pub chrome_zoom_bubble: c_int,
}

#[derive(Debug)]
#[repr(C)]
pub struct cef_window_info_t {
    pub size: usize,
    pub window_name: cef_string_t,
    pub bounds: cef_rect_t,
    pub parent_window: usize,
    pub windowless_rendering_enabled: c_int,
    pub shared_texture_enabled: c_int,
    pub external_begin_frame_enabled: c_int,
    pub window: usize,
    pub runtime_style: c_int,
}

#[derive(Debug)]
#[repr(C)]
pub struct cef_request_context_t {
    pub base: cef_base_ref_counted_t,
}

unsafe extern "C" {
    pub fn cef_api_hash(version: c_int, entry: c_int) -> *const c_char;

    pub fn cef_api_version() -> c_int;

    pub fn cef_initialize(
        args: *const cef_main_args_t,
        settings: *const cef_settings_t,
        application: *mut cef_app_t,
        windows_sandbox_info: *mut c_void,
    ) -> c_int;

    pub fn cef_execute_process(
        args: *const cef_main_args_t,
        application: *mut cef_app_t,
        windows_sandbox_info: *mut c_void,
    ) -> c_int;

    pub fn cef_shutdown();

    pub fn cef_do_message_loop_work();

    pub fn cef_browser_host_create_browser(
        windowInfo: *const cef_window_info_t,
        client: *mut cef_client_t,
        url: *const cef_string_t,
        settings: *const cef_browser_settings_t,
        extra_info: *mut c_void,
        request_context: *mut cef_request_context_t,
    ) -> c_int;
}
