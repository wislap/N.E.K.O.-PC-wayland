use std::collections::HashMap;
#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;
use std::sync::mpsc::TryRecvError;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use tao::event::{Event, StartCause, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoop, EventLoopBuilder, EventLoopProxy};
use tao::window::{Window, WindowBuilder};
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
use crate::frame_bridge::{default_frame_dump_path, load_bgra_frame};
use crate::ipc::{
    HostAction, HostEvent, InteractiveRectPayload, StrategySelectionSnapshot,
    WaylandProfileSnapshot, build_emit_script, handle_frontend_message, init_script,
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

#[derive(Debug, Clone)]
enum UserEvent {
    EmitToFrontend(HostEvent),
    ApplyInputRegion(InputRegion),
    ReapplyInputRegion(InputRegion),
    OpenWindow(String),
    Terminate,
}

struct WindowEntry {
    window: Window,
    webview: WebView,
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
        &config.app_title,
        &frontend_url,
        &profile,
        strategy.clone(),
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

    let ready_event = HostEvent::Ready {
        profile: WaylandProfileSnapshot::from(&profile),
        strategy: StrategySelectionSnapshot::from(&strategy),
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
                        &config.app_title,
                        &url,
                        &profile,
                        strategy.clone(),
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
    cef_config.frame_rate = 30;

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

    let raw_host = spawn_raw_host(build_raw_cef_host_config(config, false))
        .context("failed to spawn raw host for C standalone probe")?;
    let raw_host_handle = raw_host.handle.clone();
    let pointer_events = raw_host_handle.subscribe_pointer_events();
    let keyboard_events = raw_host_handle.subscribe_keyboard_events();

    let frame_dump_path = default_frame_dump_path();
    let helper_config = CStandaloneHelperConfig::discover(&config.repo_root)?
        .with_url(frontend_url)
        .with_env("NEKO_CEF_HELPER_WIDTH", config.render_width.to_string())
        .with_env("NEKO_CEF_HELPER_HEIGHT", config.render_height.to_string())
        .with_env("NEKO_CEF_HELPER_FRAME_RATE", config.render_fps.to_string())
        .with_env(
            "NEKO_CEF_HELPER_TRANSPARENT",
            if config.transparent_background { "1" } else { "0" },
        )
        .with_env(
            "NEKO_CEF_FRAME_DUMP_PATH",
            frame_dump_path.to_string_lossy().to_string(),
        );
    eprintln!(
        "launching C standalone helper from {} with runtime {} render={}x{}@{}fps window={}x{} fullscreen={}",
        helper_config.executable.display(),
        helper_config.runtime_dir.display(),
        config.render_width,
        config.render_height,
        config.render_fps,
        config.window_width,
        config.window_height,
        config.fullscreen
    );
    let mut helper = CStandaloneHelperHandle::spawn(&helper_config)?;
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

    loop {
        if stop_requested.load(Ordering::Relaxed) {
            eprintln!("shutdown requested; terminating C standalone helper");
            let _ = helper.send_shutdown();
            let _ = helper.terminate();
            let _ = raw_host_handle.shutdown();
        }

        if saw_browser_created {
            loop {
                match pointer_events.try_recv() {
                    Ok(event) => {
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

        if let Some(event) = helper.recv_event_timeout(Duration::from_millis(16))? {
            apply_c_standalone_event(
                event,
                &mut saw_initialize_ok,
                &mut saw_browser_created,
                &mut saw_load_end,
                &mut saw_shutdown_ok,
                &mut last_frame_push_at,
                &mut last_frontend_input_region,
                Some((&raw_host_handle, &frame_dump_path)),
            )?;
        }

        if let Some(status) = helper.try_wait()? {
            drain_c_standalone_events(
                &helper,
                &mut saw_initialize_ok,
                &mut saw_browser_created,
                &mut saw_load_end,
                &mut saw_shutdown_ok,
                &mut last_frame_push_at,
                &mut last_frontend_input_region,
                Some((&raw_host_handle, &frame_dump_path)),
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
    }
}

fn drain_c_standalone_events(
    helper: &CStandaloneHelperHandle,
    saw_initialize_ok: &mut bool,
    saw_browser_created: &mut bool,
    saw_load_end: &mut bool,
    saw_shutdown_ok: &mut bool,
    last_frame_push_at: &mut Option<Instant>,
    last_frontend_input_region: &mut Option<InputRegion>,
    frame_sink: Option<(&RawHostHandle, &std::path::Path)>,
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
                last_frame_push_at,
                last_frontend_input_region,
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
    last_frame_push_at: &mut Option<Instant>,
    last_frontend_input_region: &mut Option<InputRegion>,
    frame_sink: Option<(&RawHostHandle, &std::path::Path)>,
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
                .is_some_and(|last| now.duration_since(*last) < Duration::from_millis(33))
            {
                return Ok(());
            }
            if verbose_paint_trace_enabled() || frame <= 3 || frame % 60 == 0 {
                eprintln!(
                    "C standalone helper event: paint element_type={element_type} size={}x{} frame={frame}",
                    width, height
                );
            }
            if let Some((raw_host, frame_dump_path)) = frame_sink {
                match load_bgra_frame(frame_dump_path, width as u32, height as u32) {
                    Ok(rgba_frame) => {
                        if let Err(err) = raw_host.set_rgba_frame(rgba_frame) {
                            eprintln!("failed to push dumped CEF frame into raw host: {err:#}");
                        } else {
                            *last_frame_push_at = Some(now);
                            if frame <= 2 {
                                eprintln!(
                                    "pushed dumped CEF frame into raw host: size={}x{} frame={frame}",
                                    width, height
                                );
                            }
                        }
                    }
                    Err(err) => {
                        eprintln!(
                            "failed to load dumped CEF frame {}: {err:#}",
                            frame_dump_path.display()
                        );
                    }
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
            if let Some((raw_host, _)) = frame_sink {
                if let Err(err) = raw_host.set_input_region(region.clone()) {
                    eprintln!("failed to push frontend input region into raw host: {err:#}");
                } else {
                    *last_frontend_input_region = Some(region.clone());
                    eprintln!(
                        "applied frontend input region to raw host: {} rect(s)",
                        region.rects().len()
                    );
                }
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
    profile: WaylandProfile,
    strategy: crate::wayland::engine::StrategySelection,
) -> impl Fn(wry::http::Request<String>) + 'static {
    move |request| {
        let body = request.body();
        match handle_frontend_message(body, &profile, &strategy) {
            HostAction::Emit(event) => {
                let _ = proxy.send_event(UserEvent::EmitToFrontend(event));
            }
            HostAction::ApplyInputRegion(region) => {
                let _ = proxy.send_event(UserEvent::ApplyInputRegion(region));
            }
        }
    }
}

fn create_window(
    event_loop: &tao::event_loop::EventLoopWindowTarget<UserEvent>,
    proxy: &EventLoopProxy<UserEvent>,
    app_title: &str,
    url: &str,
    profile: &WaylandProfile,
    strategy: StrategySelection,
) -> Result<WindowEntry> {
    let window = WindowBuilder::new()
        .with_title(app_title)
        .with_decorations(false)
        .with_resizable(true)
        .with_transparent(true)
        .build(event_loop)
        .context("failed to create tao window")?;

    let new_window_proxy = proxy.clone();
    let builder = WebViewBuilder::new()
        .with_url(url)
        .with_transparent(true)
        .with_initialization_script(init_script())
        .with_ipc_handler(make_ipc_handler(
            proxy.clone(),
            profile.clone(),
            strategy.clone(),
        ))
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
