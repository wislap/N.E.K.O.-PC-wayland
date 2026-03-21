use std::collections::HashMap;
use std::fs;
#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;
#[cfg(unix)]
use std::os::fd::AsRawFd;
use std::process::Command;
use std::sync::mpsc::{self, SyncSender, TryRecvError, TrySendError};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use arboard::Clipboard;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use rfd::FileDialog;
use tao::event::{Event, StartCause, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoop, EventLoopBuilder, EventLoopProxy};
use tao::monitor::MonitorHandle;
use tao::dpi::PhysicalPosition;
use tao::window::{Fullscreen, Window, WindowBuilder};
#[cfg(target_os = "linux")]
use wry::WebViewBuilderExtUnix;
use wry::{NewWindowResponse, WebView, WebViewBuilder};

#[cfg(target_os = "linux")]
use tao::platform::unix::WindowExtUnix;

#[cfg(feature = "cef_osr")]
use crate::cef::{
    CefLifecycleEvent, CefOsrConfig, clear_event_callback, install_event_callback,
    run_raw_input_loop, spawn_osr_bridge,
};
use crate::config::{AppConfig, WaylandHostMode};
use crate::frame_bridge::{
    FrameReader, SharedFrameReader, SharedFrameReaderConfig, create_shared_frame_writer,
    default_frame_dump_path, duplicate_fd,
};
use crate::ipc::{
    DisplaySnapshot, HostAction, HostCapabilitiesSnapshot, HostEvent, HostRequest,
    InteractiveRectPayload, StrategySelectionSnapshot, WaylandProfileSnapshot,
    WindowStateSnapshot, build_emit_script, handle_frontend_message, init_script,
};
use crate::launcher;
use crate::official_helper::{OfficialHelperConfig, OfficialHelperHandle};
use crate::standalone_helper::{
    CStandaloneHelperConfig, CStandaloneHelperEvent, CStandaloneHelperHandle,
};
use crate::wayland::detect::WaylandProfile;
use crate::wayland::engine::{
    StrategySelection, apply_input_region_to_window, choose_strategy, maybe_dump_widget_tree,
};
use crate::wayland::input_region::InputRegion;
use crate::wayland::raw_host::{RawHostConfig, RawHostHandle, spawn as spawn_raw_host};
use crate::wayland::raw_host::RawHostPointerEvent;

fn verbose_paint_trace_enabled() -> bool {
    matches!(
        std::env::var("NEKO_CEF_VERBOSE_PAINT")
            .ok()
            .as_deref()
            .map(|value| value.trim().to_ascii_lowercase())
            .as_deref(),
        Some("1") | Some("true") | Some("yes") | Some("on")
    )
}

fn clamp_cef_frame_rate(frame_rate: u32) -> u32 {
    frame_rate.clamp(1, 60)
}

fn helper_loop_sleep_ms(frame_rate: u32) -> u32 {
    let frame_rate = frame_rate.max(1);
    (1000 / frame_rate).clamp(1, 8)
}

fn helper_event_wait_ms(frame_rate: u32) -> u64 {
    let frame_rate = frame_rate.max(1);
    u64::from((1000 / frame_rate).clamp(1, 8))
}

fn input_region_apply_interval_ms() -> u64 {
    std::env::var("NEKO_INPUT_REGION_MIN_INTERVAL_MS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(80)
}

#[derive(Debug, Clone, Copy)]
struct FramePumpRequest {
    width: u32,
    height: u32,
    frame: i32,
}

#[derive(Debug, Clone)]
struct FramePumpHandle {
    sender: SyncSender<FramePumpRequest>,
}

enum FrameDelivery {
    Shared,
    Pump(FramePumpHandle),
}

enum FramePumpSource {
    File {
        path: std::path::PathBuf,
        reader: Option<FrameReader>,
    },
}

impl FramePumpHandle {
    fn request(&self, request: FramePumpRequest) {
        match self.sender.try_send(request) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {}
            Err(TrySendError::Disconnected(_)) => {}
        }
    }
}

fn spawn_frame_pump(raw_host: RawHostHandle, mut source: FramePumpSource) -> FramePumpHandle {
    let (sender, receiver) = mpsc::sync_channel::<FramePumpRequest>(1);
    thread::Builder::new()
        .name("neko-cef-frame-pump".to_string())
        .spawn(move || {
            while let Ok(mut request) = receiver.recv() {
                while let Ok(next) = receiver.try_recv() {
                    request = next;
                }

                let frame_result = match &mut source {
                    FramePumpSource::File { path, reader } => {
                        if reader.is_none() {
                            match FrameReader::open(&*path) {
                                Ok(opened_reader) => {
                                    *reader = Some(opened_reader);
                                }
                                Err(err) => {
                                    if verbose_paint_trace_enabled() || request.frame <= 3 {
                                        eprintln!(
                                            "frame pump waiting for dumped CEF frame {}: {err:#}",
                                            path.display()
                                        );
                                    }
                                    continue;
                                }
                            }
                        }

                        reader
                            .as_mut()
                            .expect("frame reader must be initialized")
                            .load_bgra_frame(request.width, request.height)
                    }
                };

                match frame_result {
                    Ok(frame) => {
                        if let Err(err) = raw_host.set_rgba_frame(frame) {
                            eprintln!("frame pump failed to push frame into raw host: {err:#}");
                            break;
                        }
                    }
                    Err(err) => {
                        let FramePumpSource::File { reader, path } = &mut source;
                        *reader = None;
                        eprintln!(
                            "frame pump failed to load dumped CEF frame {}: {err:#}",
                            path.display()
                        );
                    }
                }

                if verbose_paint_trace_enabled() && request.frame % 60 == 0 {
                    eprintln!(
                        "frame pump delivered frame={} size={}x{}",
                        request.frame, request.width, request.height
                    );
                }
            }
        })
        .expect("failed to spawn frame pump thread");
    FramePumpHandle { sender }
}

fn scale_pointer_event_to_render_space(
    event: RawHostPointerEvent,
    window_size: (u32, u32),
    render_size: (u32, u32),
) -> RawHostPointerEvent {
    fn scale(value: f64, src: u32, dst: u32) -> f64 {
        if src == 0 || dst == 0 {
            return value;
        }
        value * f64::from(dst) / f64::from(src)
    }

    let (window_width, window_height) = window_size;
    let (render_width, render_height) = render_size;

    match event {
        RawHostPointerEvent::Enter { x, y } => RawHostPointerEvent::Enter {
            x: scale(x, window_width, render_width),
            y: scale(y, window_height, render_height),
        },
        RawHostPointerEvent::Leave { x, y } => RawHostPointerEvent::Leave {
            x: scale(x, window_width, render_width),
            y: scale(y, window_height, render_height),
        },
        RawHostPointerEvent::Motion { x, y } => RawHostPointerEvent::Motion {
            x: scale(x, window_width, render_width),
            y: scale(y, window_height, render_height),
        },
        RawHostPointerEvent::Button {
            x,
            y,
            button,
            pressed,
        } => RawHostPointerEvent::Button {
            x: scale(x, window_width, render_width),
            y: scale(y, window_height, render_height),
            button,
            pressed,
        },
        RawHostPointerEvent::Wheel {
            x,
            y,
            delta_x,
            delta_y,
        } => RawHostPointerEvent::Wheel {
            x: scale(x, window_width, render_width),
            y: scale(y, window_height, render_height),
            delta_x: scale(delta_x, window_width, render_width),
            delta_y: scale(delta_y, window_height, render_height),
        },
    }
}

#[derive(Debug, Clone)]
enum UserEvent {
    EmitToFrontend(HostEvent),
    ApplyInputRegion(InputRegion),
    ReapplyInputRegion(InputRegion),
    HostRequest(HostRequest),
    OpenWindow(String),
    Terminate,
}

struct WindowEntry {
    window: Window,
    webview: WebView,
}

fn host_capabilities() -> HostCapabilitiesSnapshot {
    HostCapabilitiesSnapshot {
        input_region: true,
        screen_info: true,
        window_state: true,
        dark_mode: true,
        open_external: true,
        clipboard: true,
        move_window: true,
        file_dialog: true,
    }
}

fn build_host_info_event(
    profile: &WaylandProfile,
    strategy: &StrategySelection,
    dark_mode: bool,
) -> HostEvent {
    HostEvent::HostInfo {
        profile: WaylandProfileSnapshot::from(profile),
        strategy: StrategySelectionSnapshot::from(strategy),
        capabilities: host_capabilities(),
        dark_mode,
    }
}

fn collect_screen_info(window: &Window) -> HostEvent {
    let current = window.current_monitor();
    let current_name = current.as_ref().and_then(|monitor| monitor.name());
    let current_position = current.as_ref().map(|monitor| monitor.position());
    let current_size = current.as_ref().map(|monitor| monitor.size());

    let displays = window
        .available_monitors()
        .enumerate()
        .map(|(index, monitor)| {
            let position = monitor.position();
            let size = monitor.size();
            let name = monitor.name();
            let is_current = current_position.as_ref() == Some(&position)
                && current_size.as_ref() == Some(&size)
                && current_name == name;

            DisplaySnapshot {
                id: format!("display-{index}"),
                name,
                x: position.x,
                y: position.y,
                width: size.width,
                height: size.height,
                scale_factor: monitor.scale_factor(),
                is_current,
            }
        })
        .collect::<Vec<_>>();

    let current_display_id = displays
        .iter()
        .find(|display| display.is_current)
        .map(|display| display.id.clone());

    HostEvent::ScreenInfo {
        current_display_id,
        displays,
    }
}

fn find_display_for_point(window: &Window, screen_x: i32, screen_y: i32) -> Option<DisplaySnapshot> {
    window
        .available_monitors()
        .enumerate()
        .map(|(index, monitor)| {
            let position = monitor.position();
            let size = monitor.size();
            DisplaySnapshot {
                id: format!("display-{index}"),
                name: monitor.name(),
                x: position.x,
                y: position.y,
                width: size.width,
                height: size.height,
                scale_factor: monitor.scale_factor(),
                is_current: false,
            }
        })
        .find(|display| {
            screen_x >= display.x
                && screen_x < display.x + display.width as i32
                && screen_y >= display.y
                && screen_y < display.y + display.height as i32
        })
}

fn collect_window_state(window: &Window) -> HostEvent {
    let position = window.outer_position().unwrap_or(tao::dpi::PhysicalPosition::new(0, 0));
    let size = window.outer_size();

    HostEvent::WindowState {
        state: WindowStateSnapshot {
            x: position.x,
            y: position.y,
            width: size.width,
            height: size.height,
            scale_factor: window.scale_factor(),
            fullscreen: window.fullscreen().is_some(),
            maximized: window.is_maximized(),
            visible: window.is_visible(),
            focused: window.is_focused(),
        },
    }
}

fn monitor_matches_name(monitor: &MonitorHandle, expected: &str) -> bool {
    let expected = expected.trim().to_ascii_lowercase();
    if expected.is_empty() {
        return false;
    }
    monitor
        .name()
        .map(|name| {
            let name = name.trim().to_ascii_lowercase();
            name == expected || name.contains(&expected)
        })
        .unwrap_or(false)
}

fn describe_monitor(monitor: &MonitorHandle) -> String {
    let name = monitor.name().unwrap_or_else(|| "<unnamed>".to_string());
    let position = monitor.position();
    let size = monitor.size();
    format!(
        "{} @ {}x{}+{},{}",
        name, size.width, size.height, position.x, position.y
    )
}

fn select_target_monitor(
    event_loop: &tao::event_loop::EventLoopWindowTarget<UserEvent>,
    config: &AppConfig,
) -> Option<MonitorHandle> {
    let monitors = event_loop.available_monitors().collect::<Vec<_>>();
    if monitors.is_empty() {
        return None;
    }

    if let Some(index) = config.target_display_index {
        if let Some(monitor) = monitors.get(index).cloned() {
            eprintln!(
                "selected target display by index {}: {}",
                index,
                describe_monitor(&monitor)
            );
            return Some(monitor);
        }
        eprintln!(
            "requested display index {} is out of range; available displays={}",
            index,
            monitors.len()
        );
    }

    if let Some(name) = config.target_display_name.as_deref() {
        if let Some(monitor) = monitors
            .iter()
            .find(|monitor| monitor_matches_name(monitor, name))
            .cloned()
        {
            eprintln!(
                "selected target display by name {:?}: {}",
                name,
                describe_monitor(&monitor)
            );
            return Some(monitor);
        }
        eprintln!("requested display name {:?} was not found; falling back", name);
    }

    None
}

fn open_external_url(url: &str) -> Result<()> {
    if url.trim().is_empty() {
        bail!("external url must not be empty");
    }

    #[cfg(target_os = "linux")]
    {
        Command::new("xdg-open")
            .arg(url)
            .spawn()
            .with_context(|| format!("failed to launch xdg-open for {url}"))?;
        return Ok(());
    }

    #[cfg(not(target_os = "linux"))]
    {
        bail!("open_external is not implemented for this platform")
    }
}

fn read_clipboard_text() -> Result<Option<String>> {
    let mut clipboard = Clipboard::new().context("failed to open system clipboard")?;
    match clipboard.get_text() {
        Ok(text) => Ok(Some(text)),
        Err(err) => {
            let message = err.to_string();
            if message.contains("ContentNotAvailable") || message.contains("Clipboard") {
                Ok(None)
            } else {
                Err(err).context("failed to read clipboard text")
            }
        }
    }
}

fn write_clipboard_text(text: &str) -> Result<Option<String>> {
    let mut clipboard = Clipboard::new().context("failed to open system clipboard")?;
    clipboard
        .set_text(text.to_string())
        .context("failed to write clipboard text")?;
    Ok(Some(text.to_string()))
}

fn build_file_dialog(
    title: Option<&str>,
    directory: Option<&str>,
    filters: &[crate::ipc::FileDialogFilterPayload],
) -> FileDialog {
    let mut dialog = FileDialog::new();
    if let Some(title) = title.filter(|value| !value.trim().is_empty()) {
        dialog = dialog.set_title(title);
    }
    if let Some(directory) = directory.filter(|value| !value.trim().is_empty()) {
        dialog = dialog.set_directory(directory);
    }
    for filter in filters {
        let extensions = filter
            .extensions
            .iter()
            .map(|ext| ext.trim_start_matches('.'))
            .filter(|ext| !ext.is_empty())
            .collect::<Vec<_>>();
        if !extensions.is_empty() {
            dialog = dialog.add_filter(&filter.name, &extensions);
        }
    }
    dialog
}

fn open_file_dialog(
    directory: bool,
    multiple: bool,
    title: Option<&str>,
    filters: &[crate::ipc::FileDialogFilterPayload],
) -> Vec<String> {
    let dialog = build_file_dialog(title, None, filters);
    if directory {
        return dialog
            .pick_folder()
            .into_iter()
            .map(|path| path.display().to_string())
            .collect();
    }
    if multiple {
        return dialog
            .pick_files()
            .unwrap_or_default()
            .into_iter()
            .map(|path| path.display().to_string())
            .collect();
    }
    dialog
        .pick_file()
        .into_iter()
        .map(|path| path.display().to_string())
        .collect()
}

fn save_file_dialog(
    title: Option<&str>,
    suggested_name: Option<&str>,
    directory: Option<&str>,
    filters: &[crate::ipc::FileDialogFilterPayload],
) -> Option<String> {
    let mut dialog = build_file_dialog(title, directory, filters);
    if let Some(suggested_name) = suggested_name.filter(|value| !value.trim().is_empty()) {
        dialog = dialog.set_file_name(suggested_name);
    }
    dialog.save_file().map(|path| path.display().to_string())
}

fn read_file_payload(path: &str) -> Result<HostEvent> {
    let bytes = fs::read(path).with_context(|| format!("failed to read file {path}"))?;
    Ok(HostEvent::FileRead {
        path: path.to_string(),
        name: std::path::Path::new(path)
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or(path)
            .to_string(),
        content_base64: BASE64.encode(bytes),
    })
}

fn write_file_payload(path: &str, content_base64: &str) -> Result<HostEvent> {
    let bytes = BASE64
        .decode(content_base64)
        .with_context(|| format!("failed to decode base64 payload for {path}"))?;
    fs::write(path, bytes).with_context(|| format!("failed to write file {path}"))?;
    Ok(HostEvent::FileWriteComplete {
        path: path.to_string(),
    })
}

pub fn run(config: AppConfig) -> Result<()> {
    eprintln!(
        "discovered N.E.K.O repo root: {}",
        config.repo_root.display()
    );
    let profile = WaylandProfile::detect();
    eprintln!("detected Wayland profile: {profile:#?}");
    let strategy = choose_strategy(&profile);
    eprintln!(
        "selected window strategy: {:?} ({})",
        strategy.tier, strategy.reason
    );

    if matches!(config.wayland_host_mode, WaylandHostMode::RawOnly) && profile.is_wayland() {
        #[cfg(feature = "cef_osr")]
        {
            eprintln!("running in raw-only Wayland host mode with CEF OSR");
            return run_raw_only_cef(config);
        }

        #[cfg(not(feature = "cef_osr"))]
        {
            eprintln!("running in raw-only Wayland host mode");
            return crate::wayland::raw_host::run(build_raw_host_config(
                &config,
                current_input_region_or_empty(&config),
                true,
            ));
        }
    }

    if matches!(config.wayland_host_mode, WaylandHostMode::OfficialHelperProbe) {
        eprintln!("running in official helper probe mode");
        return run_official_helper_probe(&config);
    }

    if matches!(config.wayland_host_mode, WaylandHostMode::OfficialHelperRun) {
        eprintln!("running in official helper runtime mode");
        return run_official_helper_runtime(&config);
    }

    if matches!(config.wayland_host_mode, WaylandHostMode::CStandaloneProbe) {
        eprintln!("running in C standalone helper probe mode");
        return run_c_standalone_probe(&config);
    }

    let launcher_runtime = launcher::start_launcher(&config.repo_root)?;
    let frontend_ports = launcher::wait_for_frontend_url(&launcher_runtime)?;
    let frontend_url = frontend_ports
        .frontend_url()
        .context("launcher did not provide MAIN_SERVER_PORT")?;
    eprintln!("loading frontend url: {frontend_url}");

    let event_loop: EventLoop<UserEvent> = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    install_signal_handler(proxy.clone())?;

    let raw_host_handle =
        maybe_spawn_raw_host_companion(&config, &profile, current_input_region_or_empty(&config))?;

    let first_window = create_window(
        &event_loop,
        &proxy,
        &config,
        &config.app_title,
        &frontend_url,
    )?;
    let first_window_id = first_window.window.id();
    maybe_dump_widget_tree(&first_window.window);
    let mut windows = HashMap::new();
    windows.insert(first_window_id, first_window);
    let mut current_input_region = config.debug_input_region.clone();

    if config.trace_input_region {
        eprintln!(
            "input-region tracing enabled; startup debug region: {:?}",
            current_input_region
        );
    }

    if let Some(region) = current_input_region.as_ref() {
        if let Some(entry) = windows.get(&first_window_id) {
            eprintln!(
                "applying startup debug input region with {} rect(s): {:?}",
                region.rects().len(),
                region.rects()
            );
            apply_input_region_to_window(&entry.window, region)?;
            schedule_input_region_reapply(proxy.clone(), region.clone());
        }
    }

    if let (Some(handle), Some(region)) = (&raw_host_handle, current_input_region.as_ref()) {
        if let Err(err) = handle.set_input_region(region.clone()) {
            eprintln!("failed to push startup input region into raw host companion: {err:#}");
        }
    }

    let mut dark_mode_enabled = false;
    let ready_event = HostEvent::Ready {
        profile: WaylandProfileSnapshot::from(&profile),
        strategy: StrategySelectionSnapshot::from(&strategy),
        capabilities: host_capabilities(),
        dark_mode: dark_mode_enabled,
    };

    let mut launcher_runtime = Some(launcher_runtime);
    let raw_host_handle = raw_host_handle;

    event_loop.run(move |event, event_loop_window_target, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            Event::NewEvents(StartCause::Init) => {
                if let Some(entry) = windows.get(&first_window_id) {
                    let _ = entry.webview.evaluate_script(
                        &build_emit_script(&ready_event).unwrap_or_else(|_| {
                            "console.error('failed to emit ready event');".to_string()
                        }),
                    );
                    if let Ok(script) = build_emit_script(&collect_window_state(&entry.window)) {
                        let _ = entry.webview.evaluate_script(&script);
                    }
                    if let Ok(script) = build_emit_script(&collect_screen_info(&entry.window)) {
                        let _ = entry.webview.evaluate_script(&script);
                    }
                }
            }
            Event::UserEvent(user_event) => match user_event {
                UserEvent::EmitToFrontend(host_event) => {
                    if let Ok(script) = build_emit_script(&host_event) {
                        for entry in windows.values() {
                            let _ = entry.webview.evaluate_script(&script);
                        }
                    }
                }
                UserEvent::ApplyInputRegion(region) => {
                    if config.trace_input_region {
                        eprintln!(
                            "received frontend input region with {} rect(s): {:?}",
                            region.rects().len(),
                            region.rects()
                        );
                    }
                    let rect_count = region.rects().len();
                    let mut apply_error = None;

                    for entry in windows.values() {
                        if let Err(err) = apply_input_region_to_window(&entry.window, &region) {
                            apply_error = Some(err);
                            break;
                        }
                    }

                    match apply_error {
                        None => {
                            if let Some(handle) = raw_host_handle.as_ref() {
                                if let Err(err) = handle.set_input_region(region.clone()) {
                                    eprintln!(
                                        "failed to push input region into raw host companion: {err:#}"
                                    );
                                }
                            }
                            schedule_input_region_reapply(proxy.clone(), region.clone());
                            current_input_region = Some(region);
                            if let Ok(script) =
                                build_emit_script(&HostEvent::InputRegionApplied { rect_count })
                            {
                                for entry in windows.values() {
                                    let _ = entry.webview.evaluate_script(&script);
                                }
                            }
                        }
                        Some(err) => {
                            if let Ok(script) = build_emit_script(&HostEvent::Error {
                                message: format!("failed to apply input region: {err}"),
                            }) {
                                for entry in windows.values() {
                                    let _ = entry.webview.evaluate_script(&script);
                                }
                            }
                        }
                    }
                }
                UserEvent::HostRequest(request) => {
                    let main_window = windows
                        .get(&first_window_id)
                        .or_else(|| windows.values().next())
                        .map(|entry| &entry.window);

                    let event = match request {
                        HostRequest::GetHostInfo => {
                            Some(build_host_info_event(&profile, &strategy, dark_mode_enabled))
                        }
                        HostRequest::GetScreenInfo => {
                            main_window.map(collect_screen_info)
                        }
                        HostRequest::GetWindowState => {
                            main_window.map(collect_window_state)
                        }
                        HostRequest::GetDarkMode => {
                            Some(HostEvent::DarkModeChanged {
                                enabled: dark_mode_enabled,
                            })
                        }
                        HostRequest::SetDarkMode { enabled } => {
                            dark_mode_enabled = enabled;
                            Some(HostEvent::DarkModeChanged { enabled })
                        }
                        HostRequest::OpenExternal { url } => {
                            match open_external_url(&url) {
                                Ok(()) => Some(HostEvent::ExternalOpened { url }),
                                Err(err) => Some(HostEvent::Error {
                                    message: format!("failed to open external url: {err}"),
                                }),
                            }
                        }
                        HostRequest::GetClipboardText => match read_clipboard_text() {
                            Ok(text) => Some(HostEvent::ClipboardText { text }),
                            Err(err) => Some(HostEvent::Error {
                                message: format!("failed to read clipboard text: {err}"),
                            }),
                        },
                        HostRequest::SetClipboardText { text } => {
                            match write_clipboard_text(&text) {
                                Ok(text) => Some(HostEvent::ClipboardText { text }),
                                Err(err) => Some(HostEvent::Error {
                                    message: format!("failed to write clipboard text: {err}"),
                                }),
                            }
                        }
                        HostRequest::MoveWindowToDisplay { screen_x, screen_y } => {
                            if let Some(window) = main_window {
                                let target_position =
                                    find_display_for_point(window, screen_x, screen_y)
                                        .map(|display| PhysicalPosition::new(display.x, display.y))
                                        .unwrap_or_else(|| PhysicalPosition::new(screen_x, screen_y));
                                window.set_outer_position(target_position);
                                Some(collect_window_state(window))
                            } else {
                                Some(HostEvent::Error {
                                    message: "no active window available to move".to_string(),
                                })
                            }
                        }
                        HostRequest::OpenFileDialog {
                            directory,
                            multiple,
                            title,
                            filters,
                        } => Some(HostEvent::FileDialogResult {
                            paths: open_file_dialog(
                                directory,
                                multiple,
                                title.as_deref(),
                                &filters,
                            ),
                        }),
                        HostRequest::SaveFileDialog {
                            title,
                            suggested_name,
                            directory,
                            filters,
                        } => Some(HostEvent::SaveDialogResult {
                            path: save_file_dialog(
                                title.as_deref(),
                                suggested_name.as_deref(),
                                directory.as_deref(),
                                &filters,
                            ),
                        }),
                        HostRequest::ReadFile { path } => match read_file_payload(&path) {
                            Ok(event) => Some(event),
                            Err(err) => Some(HostEvent::Error {
                                message: format!("failed to read file: {err}"),
                            }),
                        },
                        HostRequest::WriteFile {
                            path,
                            content_base64,
                        } => match write_file_payload(&path, &content_base64) {
                            Ok(event) => Some(event),
                            Err(err) => Some(HostEvent::Error {
                                message: format!("failed to write file: {err}"),
                            }),
                        },
                    };

                    if let Some(event) = event {
                        if let Ok(script) = build_emit_script(&event) {
                            for entry in windows.values() {
                                let _ = entry.webview.evaluate_script(&script);
                            }
                        }
                    }
                }
                UserEvent::ReapplyInputRegion(region) => {
                    if let Some(handle) = raw_host_handle.as_ref() {
                        if let Err(err) = handle.set_input_region(region.clone()) {
                            eprintln!(
                                "failed to reapply input region into raw host companion: {err:#}"
                            );
                        }
                    }
                    for entry in windows.values() {
                        if let Err(err) = apply_input_region_to_window(&entry.window, &region) {
                            eprintln!("deferred input-region reapply failed: {err:#}");
                            break;
                        }
                    }
                }
                UserEvent::OpenWindow(url) => {
                    match create_window(
                        event_loop_window_target,
                        &proxy,
                        &config,
                        &config.app_title,
                        &url,
                    ) {
                        Ok(entry) => {
                            maybe_dump_widget_tree(&entry.window);
                            if let Some(region) = current_input_region.as_ref() {
                                if let Err(err) = apply_input_region_to_window(&entry.window, region)
                                {
                                    eprintln!(
                                        "failed to apply current input region to new window: {err:#}"
                                    );
                                } else {
                                    schedule_input_region_reapply(proxy.clone(), region.clone());
                                }
                            }
                            windows.insert(entry.window.id(), entry);
                        }
                        Err(err) => {
                            if let Ok(script) = build_emit_script(&HostEvent::Error {
                                message: format!("failed to open new window: {err}"),
                            }) {
                                for entry in windows.values() {
                                    let _ = entry.webview.evaluate_script(&script);
                                }
                            }
                        }
                    }
                }
                UserEvent::Terminate => {
                    shutdown_raw_host(raw_host_handle.as_ref());
                    if let Some(mut runtime) = launcher_runtime.take() {
                        runtime.handle.terminate();
                    }
                    *control_flow = ControlFlow::Exit;
                }
            },
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                window_id,
                ..
            } => {
                windows.remove(&window_id);
                if windows.is_empty() {
                    shutdown_raw_host(raw_host_handle.as_ref());
                    if let Some(mut runtime) = launcher_runtime.take() {
                        runtime.handle.terminate();
                    }
                    *control_flow = ControlFlow::Exit;
                }
            }
            Event::WindowEvent {
                event,
                window_id,
                ..
            } => {
                let should_emit_window_state = matches!(
                    event,
                    WindowEvent::Moved(_)
                        | WindowEvent::Resized(_)
                        | WindowEvent::Focused(_)
                        | WindowEvent::ScaleFactorChanged { .. }
                );
                let should_emit_screen_info = matches!(
                    event,
                    WindowEvent::Moved(_) | WindowEvent::ScaleFactorChanged { .. }
                );

                if should_emit_window_state || should_emit_screen_info {
                    if let Some(entry) = windows.get(&window_id) {
                        if should_emit_window_state {
                            if let Ok(script) = build_emit_script(&collect_window_state(&entry.window)) {
                                for window_entry in windows.values() {
                                    let _ = window_entry.webview.evaluate_script(&script);
                                }
                            }
                        }
                        if should_emit_screen_info {
                            if let Ok(script) = build_emit_script(&collect_screen_info(&entry.window)) {
                                for window_entry in windows.values() {
                                    let _ = window_entry.webview.evaluate_script(&script);
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    })
}

#[cfg(feature = "cef_osr")]
fn run_raw_only_cef(config: AppConfig) -> Result<()> {
    let launcher_runtime = launcher::start_launcher(&config.repo_root)?;
    let frontend_ports = launcher::wait_for_frontend_url(&launcher_runtime)?;
    let frontend_url = frontend_ports
        .frontend_url()
        .context("launcher did not provide MAIN_SERVER_PORT")?;
    let frontend_url = append_query_pairs(
        &frontend_url,
        &[
            ("neko_host", "wayland_cef_raw"),
            ("neko_disable_tutorial", "1"),
        ],
    );
    eprintln!("loading frontend url: {frontend_url}");

    let raw_host = spawn_raw_host(build_raw_cef_host_config(&config, true))
        .context("failed to spawn raw-only Wayland host")?;
    let handle = raw_host.handle.clone();
    let pointer_events = handle.subscribe_pointer_events();
    let keyboard_events = handle.subscribe_keyboard_events();

    let mut cef_config = CefOsrConfig::demo();
    cef_config.width = 800;
    cef_config.height = 600;
    cef_config.url = frontend_url;
    cef_config.transparent_painting = false;
    cef_config.frame_rate = clamp_cef_frame_rate(30);

    install_event_callback(|event| match event {
        CefLifecycleEvent::BrowserCreated => {
            eprintln!("NEKO_CEF_EVENT browser_created");
        }
        CefLifecycleEvent::BrowserBeforeClose => {
            eprintln!("NEKO_CEF_EVENT browser_before_close");
        }
        CefLifecycleEvent::LoadStart { transition_type } => {
            eprintln!("NEKO_CEF_EVENT load_start transition_type={transition_type}");
        }
        CefLifecycleEvent::LoadEnd { http_status_code } => {
            eprintln!("NEKO_CEF_EVENT load_end http_status_code={http_status_code}");
        }
        CefLifecycleEvent::LoadError {
            error_code,
            error_text,
            failed_url,
        } => {
            eprintln!(
                "NEKO_CEF_EVENT load_error error_code={} error_text={:?} failed_url={:?}",
                error_code, error_text, failed_url
            );
        }
        CefLifecycleEvent::LoadingStateChange {
            is_loading,
            can_go_back,
            can_go_forward,
        } => {
            eprintln!(
                "NEKO_CEF_EVENT loading_state is_loading={} can_go_back={} can_go_forward={}",
                is_loading, can_go_back, can_go_forward
            );
        }
        CefLifecycleEvent::Console {
            level,
            source,
            line,
            message,
        } => {
            eprintln!(
                "NEKO_CEF_EVENT console level={} source={:?}:{} message={:?}",
                level, source, line, message
            );
        }
    });

    let mut launcher_runtime = Some(launcher_runtime);
    match spawn_osr_bridge(handle.clone(), cef_config) {
        Ok(bridge) => {
            #[cfg(feature = "cef_osr")]
            let loop_mode = format!("{:?}", bridge.message_loop_mode());
            #[cfg(not(feature = "cef_osr"))]
            let loop_mode = "unavailable".to_string();
            let render_size = bridge.render_size();
            eprintln!(
                "raw-only CEF bridge attached: url={} size={}x{} transparent={} loop_mode={}",
                bridge.config().url,
                render_size.0,
                render_size.1,
                bridge.config().transparent_painting,
                loop_mode
            );
            run_raw_input_loop(&bridge, &handle, pointer_events, keyboard_events);
            bridge.request_close();
            clear_event_callback();
            if let Some(mut runtime) = launcher_runtime.take() {
                runtime.handle.terminate();
            }
            raw_host
                .join()
                .map_err(|_| anyhow::anyhow!("raw host thread panicked"))??;
            Ok(())
        }
        Err(err) => {
            clear_event_callback();
            let _ = handle.shutdown();
            if let Some(mut runtime) = launcher_runtime.take() {
                runtime.handle.terminate();
            }
            let _ = raw_host.join();
            Err(err)
        }
    }
}

fn current_input_region_or_empty(config: &AppConfig) -> InputRegion {
    config.debug_input_region.clone().unwrap_or_default()
}

fn append_query_pairs(url: &str, pairs: &[(&str, &str)]) -> String {
    let separator = if url.contains('?') { '&' } else { '?' };
    let mut result = String::with_capacity(url.len() + pairs.len() * 32);
    result.push_str(url);
    result.push(separator);

    for (index, (key, value)) in pairs.iter().enumerate() {
        if index > 0 {
            result.push('&');
        }
        result.push_str(key);
        result.push('=');
        result.push_str(value);
    }

    result
}

fn run_official_helper_probe(config: &AppConfig) -> Result<()> {
    let helper_config = OfficialHelperConfig::discover(&config.repo_root)?;
    eprintln!(
        "launching official helper from {} args={:?}",
        helper_config.runtime_dir.display(),
        helper_config.args
    );
    let mut helper = OfficialHelperHandle::spawn(&helper_config)?;
    let started = helper.wait_for_startup(Duration::from_secs(2))?;
    eprintln!("official helper startup event observed: {started}");
    let spawned_pid = helper.wait_for_spawned(Duration::from_secs(2))?;
    eprintln!("official helper child pid: {spawned_pid:?}");
    let ready = helper.wait_for_ready(Duration::from_secs(3))?;
    eprintln!("official helper ready event: {ready:?}");
    let _ = helper.send_navigate("https://example.com/");
    let unsupported = helper.wait_for_unsupported("navigate", Duration::from_secs(1))?;
    eprintln!("official helper navigate support probe: {unsupported:?}");
    let ping_nonce = "official-helper-probe";
    helper.send_ping(ping_nonce)?;
    let pong = helper.wait_for_pong(ping_nonce, Duration::from_secs(1))?;
    eprintln!("official helper ping handshake: {pong}");
    if should_request_official_helper_probe_shutdown() {
        helper.send_shutdown()?;
    }
    let status = helper.wait()?;
    if status.success() {
        eprintln!("official helper exited successfully");
        return Ok(());
    }

    bail!("official helper exited with status {status}");
}

fn run_official_helper_runtime(config: &AppConfig) -> Result<()> {
    let launcher_runtime = launcher::start_launcher(&config.repo_root)?;
    let frontend_ports = launcher::wait_for_frontend_url(&launcher_runtime)?;
    let frontend_url = frontend_ports
        .frontend_url()
        .context("launcher did not provide MAIN_SERVER_PORT")?;
    let frontend_url = append_query_pairs(
        &frontend_url,
        &[
            ("neko_host", "cef_official_helper"),
            ("neko_disable_tutorial", "1"),
        ],
    );
    eprintln!("loading frontend url: {frontend_url}");

    let helper_config = OfficialHelperConfig::discover(&config.repo_root)?.with_url(&frontend_url);
    eprintln!(
        "launching official helper runtime from {} args={:?}",
        helper_config.runtime_dir.display(),
        helper_config.args
    );

    let mut helper = OfficialHelperHandle::spawn(&helper_config)?;
    let stop_requested = Arc::new(AtomicBool::new(false));
    install_shutdown_flag_handler(stop_requested.clone())?;
    let started = helper.wait_for_startup(Duration::from_secs(2))?;
    eprintln!("official helper runtime startup event observed: {started}");
    let spawned_pid = helper.wait_for_spawned(Duration::from_secs(2))?;
    eprintln!("official helper runtime child pid: {spawned_pid:?}");
    let ready = helper.wait_for_ready(Duration::from_secs(3))?;
    eprintln!("official helper runtime ready event: {ready:?}");
    helper.send_navigate(frontend_url.clone())?;
    let unsupported = helper.wait_for_unsupported("navigate", Duration::from_secs(1))?;
    eprintln!("official helper runtime navigate support probe: {unsupported:?}");

    let ping_nonce = format!("official-helper-runtime-{}", std::process::id());
    helper.send_ping(&ping_nonce)?;
    let pong = helper.wait_for_pong(&ping_nonce, Duration::from_secs(2))?;
    eprintln!("official helper runtime handshake: {pong}");

    let mut launcher_runtime = Some(launcher_runtime);
    let mut shutdown_sent = false;
    let mut shutdown_deadline = None;
    let mut exit_status = None;

    loop {
        if stop_requested.load(Ordering::Relaxed) && !shutdown_sent {
            eprintln!("shutdown requested, forwarding graceful stop to official helper");
            if let Err(err) = helper.send_shutdown() {
                eprintln!("failed to send shutdown to official helper: {err:#}");
            }
            shutdown_sent = true;
            shutdown_deadline = Some(std::time::Instant::now() + Duration::from_secs(5));
        }

        if let Some(deadline) = shutdown_deadline {
            if std::time::Instant::now() >= deadline {
                eprintln!("official helper did not exit after grace period, forcing termination");
                helper.terminate();
                shutdown_deadline = None;
            }
        }

        if let Some(event) = helper.recv_event_timeout(Duration::from_millis(100))? {
            if let Some(state) = event.state_summary() {
                eprintln!("official helper runtime state: {state}");
            }
            if let Some((command, reason)) = event.unsupported_info() {
                eprintln!("official helper runtime unsupported command={command}: {reason}");
            }
            if let Some(initial_url) = event.ready_initial_url() {
                eprintln!("official helper runtime ready initial_url={initial_url}");
            }
            if event.is_shutdown_requested() {
                shutdown_sent = true;
            }
            if event.is_exit() {
                break;
            }
        }

        if let Some(status) = helper.try_wait()? {
            exit_status = Some(status);
            break;
        }
    }

    let status = match exit_status {
        Some(status) => status,
        None => helper.wait()?,
    };
    if let Some(mut runtime) = launcher_runtime.take() {
        runtime.handle.terminate();
    }
    #[cfg(unix)]
    if stop_requested.load(Ordering::Relaxed)
        && matches!(status.signal(), Some(2) | Some(15))
    {
        eprintln!(
            "official helper runtime exited via signal {:?} during requested shutdown; treating as clean exit",
            status.signal()
        );
        return Ok(());
    }
    if status.success() {
        eprintln!("official helper runtime exited successfully");
        return Ok(());
    }

    bail!("official helper runtime exited with status {status}");
}

fn should_request_official_helper_probe_shutdown() -> bool {
    !matches!(
        std::env::var("NEKO_CEF_OFFICIAL_HELPER_PROBE_KEEP_RUNNING")
            .ok()
            .as_deref(),
        Some("1") | Some("true") | Some("TRUE") | Some("yes") | Some("YES")
    )
}

fn run_c_standalone_probe(config: &AppConfig) -> Result<()> {
    let launcher_runtime = launcher::start_launcher(&config.repo_root)?;
    let frontend_ports = launcher::wait_for_frontend_url(&launcher_runtime)?;
    let frontend_url = frontend_ports
        .frontend_url()
        .context("launcher did not provide MAIN_SERVER_PORT")?;
    let frontend_url = append_query_pairs(
        &frontend_url,
        &[
            ("neko_host", "wayland_cef_c_standalone"),
            ("neko_disable_tutorial", "1"),
            (
                "neko_transparent_bg",
                if config.transparent_background { "1" } else { "0" },
            ),
        ],
    );
    eprintln!("loading frontend url: {frontend_url}");
    let effective_render_fps = clamp_cef_frame_rate(config.render_fps);
    if effective_render_fps != config.render_fps {
        eprintln!(
            "requested render fps {} is above CEF OSR limit; clamping to {}",
            config.render_fps, effective_render_fps
        );
    }

    let raw_host = spawn_raw_host(build_raw_cef_host_config(config, false))
        .context("failed to spawn raw host for C standalone probe")?;
    let raw_host_handle = raw_host.handle.clone();
    let pointer_events = raw_host_handle.subscribe_pointer_events();
    let keyboard_events = raw_host_handle.subscribe_keyboard_events();

    let mut helper_config = CStandaloneHelperConfig::discover(&config.repo_root)?
        .with_url(frontend_url)
        .with_env("NEKO_CEF_HELPER_WIDTH", config.render_width.to_string())
        .with_env("NEKO_CEF_HELPER_HEIGHT", config.render_height.to_string())
        .with_env("NEKO_CEF_HELPER_FRAME_RATE", effective_render_fps.to_string())
        .with_env(
            "NEKO_CEF_HELPER_LOOP_SLEEP_MS",
            helper_loop_sleep_ms(config.render_fps).to_string(),
        )
        .with_env(
            "NEKO_CEF_HELPER_TRANSPARENT",
            if config.transparent_background { "1" } else { "0" },
        );
    let mut shared_frame_writer = None;
    let frame_delivery = match create_shared_frame_writer(config.render_width, config.render_height) {
        Ok(shared_writer) => {
            let reader = SharedFrameReader::open(SharedFrameReaderConfig {
                fd: duplicate_fd(shared_writer.fd.as_raw_fd())?,
                size: shared_writer.size,
            })
            .context("failed to open shared frame reader")?;
            raw_host_handle
                .attach_shared_frame_reader(reader)
                .context("failed to attach shared frame reader to raw host")?;
            helper_config = helper_config
                .with_env("NEKO_CEF_SHARED_FRAME_FD", shared_writer.fd.as_raw_fd().to_string())
                .with_env("NEKO_CEF_SHARED_FRAME_SIZE", shared_writer.size.to_string());
            eprintln!(
                "using memfd shared frame bridge: fd={} size={} render={}x{}",
                shared_writer.fd.as_raw_fd(),
                shared_writer.size,
                config.render_width,
                config.render_height
            );
            shared_frame_writer = Some(shared_writer);
            FrameDelivery::Shared
        }
        Err(err) => {
            let frame_dump_path = default_frame_dump_path();
            helper_config = helper_config.with_env(
                "NEKO_CEF_FRAME_DUMP_PATH",
                frame_dump_path.to_string_lossy().to_string(),
            );
            eprintln!(
                "shared frame bridge unavailable, falling back to file bridge {}: {err:#}",
                frame_dump_path.display()
            );
            FrameDelivery::Pump(spawn_frame_pump(
                raw_host_handle.clone(),
                FramePumpSource::File {
                    path: frame_dump_path,
                    reader: None,
                },
            ))
        }
    };
    eprintln!(
        "launching C standalone helper from {} with runtime {} render={}x{}@{}fps window={}x{} fullscreen={}",
        helper_config.executable.display(),
        helper_config.runtime_dir.display(),
        config.render_width,
        config.render_height,
        effective_render_fps,
        config.window_width,
        config.window_height,
        config.fullscreen
    );
    let mut helper = CStandaloneHelperHandle::spawn(&helper_config)?;
    drop(shared_frame_writer);
    let stop_requested = Arc::new(AtomicBool::new(false));
    install_shutdown_flag_handler(stop_requested.clone())?;
    let mut launcher_runtime = Some(launcher_runtime);

    let mut saw_initialize_ok = false;
    let mut saw_browser_created = false;
    let mut saw_load_end = false;
    let mut saw_shutdown_ok = false;
    let mut mouse_modifiers = 0_u32;
    let mut key_modifiers = 0_u32;
    let mut last_frame_push_at = None::<Instant>;
    let mut last_frontend_input_region = None::<InputRegion>;
    let mut last_frontend_input_region_applied_at = None::<Instant>;
    let mut last_frontend_drag_region = None::<InputRegion>;
    let mut last_frontend_drag_exclusion_region = None::<InputRegion>;
    let frame_push_interval = Duration::from_millis(
        (1000_u64 / u64::from(effective_render_fps.max(1))).max(1),
    );
    let helper_event_wait = Duration::from_millis(helper_event_wait_ms(config.render_fps));
    let mut helper_shutdown_started = false;
    let mut helper_shutdown_deadline = None::<Instant>;

    loop {
        if !raw_host_handle.is_running() && !stop_requested.load(Ordering::Relaxed) {
            eprintln!("raw host exited; requesting helper shutdown");
            stop_requested.store(true, Ordering::Relaxed);
        }

        if stop_requested.load(Ordering::Relaxed) {
            if !helper_shutdown_started {
                eprintln!("shutdown requested; terminating C standalone helper");
                let _ = helper.send_shutdown();
                let _ = raw_host_handle.shutdown();
                helper_shutdown_started = true;
                helper_shutdown_deadline = Some(Instant::now() + Duration::from_secs(2));
            }
        }

        if saw_browser_created {
            loop {
                match pointer_events.try_recv() {
                    Ok(event) => {
                        let event = scale_pointer_event_to_render_space(
                            event,
                            raw_host_handle.surface_size(),
                            (config.render_width, config.render_height),
                        );
                        if let Err(err) =
                            helper.send_pointer_event(event, &mut mouse_modifiers, key_modifiers)
                        {
                            eprintln!(
                                "failed to forward pointer event into C standalone helper: {err:#}"
                            );
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => break,
                }
            }

            loop {
                match keyboard_events.try_recv() {
                    Ok(event) => match helper.send_keyboard_event(event) {
                        Ok(updated_modifiers) => {
                            key_modifiers = updated_modifiers;
                        }
                        Err(err) => {
                            eprintln!(
                                "failed to forward keyboard event into C standalone helper: {err:#}"
                            );
                        }
                    },
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => break,
                }
            }
        }

        if let Some(event) = helper.recv_event_timeout(helper_event_wait)? {
            apply_c_standalone_event(
                event,
                &mut saw_initialize_ok,
                &mut saw_browser_created,
                &mut saw_load_end,
                &mut saw_shutdown_ok,
                frame_push_interval,
                &mut last_frame_push_at,
                &mut last_frontend_input_region,
                &mut last_frontend_input_region_applied_at,
                &mut last_frontend_drag_region,
                &mut last_frontend_drag_exclusion_region,
                Some((&raw_host_handle, &frame_delivery)),
            )?;
        }

        if let Some(status) = helper.try_wait()? {
            drain_c_standalone_events(
                &helper,
                &mut saw_initialize_ok,
                &mut saw_browser_created,
                &mut saw_load_end,
                &mut saw_shutdown_ok,
                frame_push_interval,
                &mut last_frame_push_at,
                &mut last_frontend_input_region,
                &mut last_frontend_input_region_applied_at,
                &mut last_frontend_drag_region,
                &mut last_frontend_drag_exclusion_region,
                Some((&raw_host_handle, &frame_delivery)),
            )?;
            let _ = raw_host_handle.shutdown();
            if let Some(mut runtime) = launcher_runtime.take() {
                runtime.handle.terminate();
            }
            #[cfg(unix)]
            if stop_requested.load(Ordering::Relaxed)
                && matches!(status.signal(), Some(2) | Some(15))
            {
                eprintln!(
                    "C standalone helper exited via signal {:?} during requested shutdown; treating as clean exit",
                    status.signal()
                );
                return Ok(());
            }
            if status.success() {
                if !saw_initialize_ok {
                    bail!("C standalone helper exited before initialize_ok");
                }
                if !saw_browser_created {
                    eprintln!("C standalone helper exited without browser_created");
                }
                if !saw_load_end {
                    eprintln!("C standalone helper exited without load_end");
                }
                if !saw_shutdown_ok {
                    eprintln!("C standalone helper exited without shutdown_ok");
                }
                let _ = raw_host.join();
                return Ok(());
            }
            let _ = raw_host.join();
            bail!("C standalone helper exited with status {status}");
        }

        if helper_shutdown_started {
            if let Some(deadline) = helper_shutdown_deadline {
                if Instant::now() >= deadline {
                    let _ = helper.terminate();
                    helper_shutdown_deadline = None;
                }
            }
            if let Some(status) = helper.try_wait()? {
                let _ = raw_host.join();
                if let Some(mut runtime) = launcher_runtime.take() {
                    runtime.handle.terminate();
                }
                if status.success() {
                    return Ok(());
                }
                #[cfg(unix)]
                if matches!(status.signal(), Some(2) | Some(15)) {
                    return Ok(());
                }
                bail!("C standalone helper exited with status {status}");
            }
        }
    }
}

fn drain_c_standalone_events(
    helper: &CStandaloneHelperHandle,
    saw_initialize_ok: &mut bool,
    saw_browser_created: &mut bool,
    saw_load_end: &mut bool,
    saw_shutdown_ok: &mut bool,
    frame_push_interval: Duration,
    last_frame_push_at: &mut Option<Instant>,
    last_frontend_input_region: &mut Option<InputRegion>,
    last_frontend_input_region_applied_at: &mut Option<Instant>,
    last_frontend_drag_region: &mut Option<InputRegion>,
    last_frontend_drag_exclusion_region: &mut Option<InputRegion>,
    frame_sink: Option<(&RawHostHandle, &FrameDelivery)>,
) -> Result<()> {
    let deadline = std::time::Instant::now() + Duration::from_millis(500);
    while std::time::Instant::now() < deadline {
        match helper.recv_event_timeout(Duration::from_millis(25))? {
            Some(event) => apply_c_standalone_event(
                event,
                saw_initialize_ok,
                saw_browser_created,
                saw_load_end,
                saw_shutdown_ok,
                frame_push_interval,
                last_frame_push_at,
                last_frontend_input_region,
                last_frontend_input_region_applied_at,
                last_frontend_drag_region,
                last_frontend_drag_exclusion_region,
                frame_sink,
            )?,
            None => break,
        }
    }
    Ok(())
}

fn apply_c_standalone_event(
    event: CStandaloneHelperEvent,
    saw_initialize_ok: &mut bool,
    saw_browser_created: &mut bool,
    saw_load_end: &mut bool,
    saw_shutdown_ok: &mut bool,
    frame_push_interval: Duration,
    last_frame_push_at: &mut Option<Instant>,
    last_frontend_input_region: &mut Option<InputRegion>,
    last_frontend_input_region_applied_at: &mut Option<Instant>,
    last_frontend_drag_region: &mut Option<InputRegion>,
    last_frontend_drag_exclusion_region: &mut Option<InputRegion>,
    frame_sink: Option<(&RawHostHandle, &FrameDelivery)>,
) -> Result<()> {
    match event {
        CStandaloneHelperEvent::Startup => {
            eprintln!("C standalone helper event: startup");
        }
        CStandaloneHelperEvent::InitializeOk => {
            *saw_initialize_ok = true;
            eprintln!("C standalone helper event: initialize_ok");
        }
        CStandaloneHelperEvent::InitializeFailed { message } => {
            bail!("C standalone helper initialize failed: {message}");
        }
        CStandaloneHelperEvent::CreateBrowserOk => {
            eprintln!("C standalone helper event: create_browser_ok");
        }
        CStandaloneHelperEvent::CreateBrowserFailed { message } => {
            bail!("C standalone helper create_browser failed: {message}");
        }
        CStandaloneHelperEvent::BrowserCreated => {
            *saw_browser_created = true;
            eprintln!("C standalone helper event: browser_created");
        }
        CStandaloneHelperEvent::LoadStart { transition_type } => {
            eprintln!("C standalone helper event: load_start transition_type={transition_type}");
        }
        CStandaloneHelperEvent::LoadEnd { http_status_code } => {
            *saw_load_end = true;
            eprintln!("C standalone helper event: load_end http_status_code={http_status_code}");
        }
        CStandaloneHelperEvent::Paint {
            element_type,
            width,
            height,
            frame,
        } => {
            let now = Instant::now();
            if last_frame_push_at
                .as_ref()
                .is_some_and(|last| now.duration_since(*last) < frame_push_interval)
            {
                return Ok(());
            }
            if verbose_paint_trace_enabled() || frame <= 3 || frame % 60 == 0 {
                eprintln!(
                    "C standalone helper event: paint element_type={element_type} size={}x{} frame={frame}",
                    width, height
                );
            }
            if let Some((raw_host, frame_delivery)) = frame_sink {
                match frame_delivery {
                    FrameDelivery::Shared => {
                        raw_host.refresh_shared_frame(width as u32, height as u32)?;
                    }
                    FrameDelivery::Pump(frame_pump) => {
                        frame_pump.request(FramePumpRequest {
                            width: width as u32,
                            height: height as u32,
                            frame,
                        });
                    }
                }
                *last_frame_push_at = Some(now);
                if frame <= 2 {
                    eprintln!(
                        "queued CEF frame for presentation: size={}x{} frame={frame}",
                        width, height
                    );
                }
            }
        }
        CStandaloneHelperEvent::InputRegion { rects } => {
            let region = InputRegion::from_rects(
                rects
                    .into_iter()
                    .map(|rect: InteractiveRectPayload| rect.into())
                    .collect(),
            );
            if last_frontend_input_region.as_ref() == Some(&region) {
                return Ok(());
            }
            let now = Instant::now();
            if last_frontend_input_region_applied_at
                .as_ref()
                .is_some_and(|last| {
                    now.duration_since(*last)
                        < Duration::from_millis(input_region_apply_interval_ms())
                })
            {
                return Ok(());
            }
            if let Some((raw_host, _)) = frame_sink {
                if let Err(err) = raw_host.set_input_region(region.clone()) {
                    return Err(err).context("failed to push frontend input region into raw host");
                } else {
                    *last_frontend_input_region = Some(region.clone());
                    *last_frontend_input_region_applied_at = Some(now);
                    eprintln!(
                        "applied frontend input region to raw host: {} rect(s)",
                        region.rects().len()
                    );
                }
            }
        }
        CStandaloneHelperEvent::DragRegion { rects } => {
            let region = InputRegion::from_rects(
                rects
                    .into_iter()
                    .map(|rect: InteractiveRectPayload| rect.into())
                    .collect(),
            );
            if last_frontend_drag_region.as_ref() == Some(&region) {
                return Ok(());
            }
            if let Some((raw_host, _)) = frame_sink {
                raw_host
                    .set_drag_region(region.clone())
                    .context("failed to push drag region into raw host")?;
                *last_frontend_drag_region = Some(region);
            }
        }
        CStandaloneHelperEvent::DragExclusionRegion { rects } => {
            let region = InputRegion::from_rects(
                rects
                    .into_iter()
                    .map(|rect: InteractiveRectPayload| rect.into())
                    .collect(),
            );
            if last_frontend_drag_exclusion_region.as_ref() == Some(&region) {
                return Ok(());
            }
            if let Some((raw_host, _)) = frame_sink {
                raw_host
                    .set_drag_exclusion_region(region.clone())
                    .context("failed to push drag exclusion region into raw host")?;
                *last_frontend_drag_exclusion_region = Some(region);
            }
        }
        CStandaloneHelperEvent::BrowserBeforeClose => {
            eprintln!("C standalone helper event: browser_before_close");
        }
        CStandaloneHelperEvent::BrowserReleased => {
            eprintln!("C standalone helper event: browser_released");
        }
        CStandaloneHelperEvent::ShutdownOk => {
            *saw_shutdown_ok = true;
            eprintln!("C standalone helper event: shutdown_ok");
        }
        CStandaloneHelperEvent::SubprocessExit { code } => {
            eprintln!("C standalone helper event: subprocess_exit code={code}");
        }
        CStandaloneHelperEvent::RawLine { event, fields } => {
            eprintln!("C standalone helper event: {event} fields={fields:?}");
        }
    }
    Ok(())
}

fn install_shutdown_flag_handler(stop_requested: Arc<AtomicBool>) -> Result<()> {
    ctrlc::set_handler(move || {
        stop_requested.store(true, Ordering::Relaxed);
    })
    .context("failed to install Ctrl+C handler")
}

fn maybe_spawn_raw_host_companion(
    config: &AppConfig,
    profile: &WaylandProfile,
    initial_region: InputRegion,
) -> Result<Option<RawHostHandle>> {
    if !matches!(config.wayland_host_mode, WaylandHostMode::Companion) || !profile.is_wayland() {
        return Ok(None);
    }

    let raw_host = spawn_raw_host(build_raw_host_config(config, initial_region, false))
        .context("failed to spawn raw Wayland host companion")?;
    let handle = raw_host.detach();
    eprintln!("spawned raw Wayland host companion");
    Ok(Some(handle))
}

fn build_raw_host_config(
    config: &AppConfig,
    initial_region: InputRegion,
    install_ctrlc_handler: bool,
) -> RawHostConfig {
    let mut raw_host_config = RawHostConfig::probe();
    raw_host_config.title = if install_ctrlc_handler {
        config.app_title.clone()
    } else {
        format!("{} raw-host", config.app_title)
    };
    raw_host_config.app_id = if install_ctrlc_handler {
        "moe.neko.raw-wayland-host".to_string()
    } else {
        "moe.neko.raw-wayland-host-companion".to_string()
    };
    raw_host_config.width = config.window_width;
    raw_host_config.height = config.window_height;
    raw_host_config.fullscreen = config.fullscreen;
    raw_host_config.target_output_index = config.target_display_index;
    raw_host_config.target_output_name = config.target_display_name.clone();
    raw_host_config.input_region = if initial_region.is_empty() {
        raw_host_config.input_region.clone()
    } else {
        initial_region
    };
    raw_host_config.install_ctrlc_handler = install_ctrlc_handler;
    raw_host_config.log_pointer_events = config.trace_input_region;
    if install_ctrlc_handler {
        // In raw-only mode, default to an opaque debug host so the window is obviously present
        // even before we connect a real web-rendering path.
        raw_host_config.transparent_outside_input_region = false;
        raw_host_config.visible_region = Some(InputRegion::from_rects(vec![
            crate::wayland::input_region::InteractiveRect {
                x: 0,
                y: 0,
                width: raw_host_config.width,
                height: raw_host_config.height,
            },
        ]));
    }
    raw_host_config
}

fn build_raw_cef_host_config(config: &AppConfig, install_ctrlc_handler: bool) -> RawHostConfig {
    let mut raw_host_config = build_raw_host_config(
        config,
        current_input_region_or_empty(config),
        install_ctrlc_handler,
    );
    raw_host_config.input_region_source_size = Some((config.render_width, config.render_height));
    let full_region =
        InputRegion::from_rects(vec![crate::wayland::input_region::InteractiveRect {
            x: 0,
            y: 0,
            width: raw_host_config.width,
            height: raw_host_config.height,
        }]);
    raw_host_config.input_region = current_input_region_or_empty(config);
    raw_host_config.visible_region = Some(full_region);
    raw_host_config.transparent_outside_input_region = true;
    raw_host_config.show_debug_regions_when_empty = false;
    raw_host_config.move_on_left_press = false;
    raw_host_config
}

fn shutdown_raw_host(handle: Option<&RawHostHandle>) {
    if let Some(handle) = handle {
        if let Err(err) = handle.shutdown() {
            eprintln!("failed to shutdown raw host companion cleanly: {err:#}");
        }
    }
}

fn install_signal_handler(proxy: EventLoopProxy<UserEvent>) -> Result<()> {
    ctrlc::set_handler(move || {
        let _ = proxy.send_event(UserEvent::Terminate);
    })
    .context("failed to install Ctrl+C handler")
}

fn make_ipc_handler(
    proxy: EventLoopProxy<UserEvent>,
) -> impl Fn(wry::http::Request<String>) + 'static {
    move |request| {
        let body = request.body();
        match handle_frontend_message(body) {
            HostAction::Emit(event) => {
                let _ = proxy.send_event(UserEvent::EmitToFrontend(event));
            }
            HostAction::ApplyInputRegion(region) => {
                let _ = proxy.send_event(UserEvent::ApplyInputRegion(region));
            }
            HostAction::Request(request) => {
                let _ = proxy.send_event(UserEvent::HostRequest(request));
            }
        }
    }
}

fn create_window(
    event_loop: &tao::event_loop::EventLoopWindowTarget<UserEvent>,
    proxy: &EventLoopProxy<UserEvent>,
    config: &AppConfig,
    app_title: &str,
    url: &str,
) -> Result<WindowEntry> {
    let target_monitor = select_target_monitor(event_loop, config);
    let mut builder = WindowBuilder::new()
        .with_title(app_title)
        .with_decorations(false)
        .with_resizable(true)
        .with_transparent(true);
    if let Some(monitor) = target_monitor.as_ref() {
        builder = builder.with_position(monitor.position());
        if config.fullscreen {
            builder = builder.with_inner_size(monitor.size());
        }
    }
    if config.fullscreen {
        builder = builder.with_fullscreen(Some(Fullscreen::Borderless(target_monitor.clone())));
    }

    let window = builder
        .build(event_loop)
        .context("failed to create tao window")?;
    if let Some(monitor) = target_monitor {
        window.set_outer_position(monitor.position());
        if config.fullscreen {
            window.set_fullscreen(Some(Fullscreen::Borderless(Some(monitor))));
        }
    }

    let new_window_proxy = proxy.clone();
    let builder = WebViewBuilder::new()
        .with_url(url)
        .with_transparent(true)
        .with_initialization_script(init_script())
        .with_ipc_handler(make_ipc_handler(proxy.clone()))
        .with_new_window_req_handler(move |requested_url, _features| {
            let _ = new_window_proxy.send_event(UserEvent::OpenWindow(requested_url));
            NewWindowResponse::Deny
        });

    #[cfg(target_os = "linux")]
    let webview = {
        let vbox = window
            .default_vbox()
            .context("tao window does not provide default GTK vbox on Linux")?;
        builder
            .build_gtk(vbox)
            .context("failed to build GTK webview for Wayland/X11")?
    };

    #[cfg(not(target_os = "linux"))]
    let webview = builder.build(&window).context("failed to build webview")?;

    Ok(WindowEntry { window, webview })
}

fn schedule_input_region_reapply(proxy: EventLoopProxy<UserEvent>, region: InputRegion) {
    for delay_ms in [100_u64, 350, 1000, 2000] {
        let proxy = proxy.clone();
        let region = region.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(delay_ms));
            let _ = proxy.send_event(UserEvent::ReapplyInputRegion(region));
        });
    }
}
