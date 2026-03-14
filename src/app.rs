use std::collections::HashMap;

use anyhow::{Context, Result};
use tao::event::{Event, StartCause, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoop, EventLoopBuilder, EventLoopProxy};
use tao::window::{Window, WindowBuilder};
#[cfg(target_os = "linux")]
use wry::WebViewBuilderExtUnix;
use wry::{NewWindowResponse, WebView, WebViewBuilder};

#[cfg(target_os = "linux")]
use tao::platform::unix::WindowExtUnix;

use crate::config::{AppConfig, WaylandHostMode};
use crate::ipc::{
    HostAction, HostEvent, StrategySelectionSnapshot, WaylandProfileSnapshot, build_emit_script,
    handle_frontend_message, init_script,
};
use crate::launcher;
use crate::wayland::detect::WaylandProfile;
use crate::wayland::engine::{
    StrategySelection, apply_input_region_to_window, choose_strategy, maybe_dump_widget_tree,
};
use crate::wayland::input_region::InputRegion;
use crate::wayland::raw_host::{RawHostConfig, RawHostHandle, spawn as spawn_raw_host};

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
        eprintln!("running in raw-only Wayland host mode");
        return crate::wayland::raw_host::run(build_raw_host_config(
            &config,
            current_input_region_or_empty(&config),
            true,
        ));
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

fn current_input_region_or_empty(config: &AppConfig) -> InputRegion {
    config.debug_input_region.clone().unwrap_or_default()
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
