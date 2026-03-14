use std::collections::HashMap;

use anyhow::{Context, Result};
use tao::event::{Event, StartCause, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoop, EventLoopBuilder, EventLoopProxy};
use tao::window::{Window, WindowBuilder};
use wry::{NewWindowResponse, WebView, WebViewBuilder};
#[cfg(target_os = "linux")]
use wry::WebViewBuilderExtUnix;

#[cfg(target_os = "linux")]
use tao::platform::unix::WindowExtUnix;

use crate::config::AppConfig;
use crate::ipc::{HostAction, HostEvent, StrategySelectionSnapshot, WaylandProfileSnapshot, build_emit_script, handle_frontend_message, init_script};
use crate::launcher;
use crate::wayland::detect::WaylandProfile;
use crate::wayland::engine::{PrototypeBackend, WindowInputBackend};
use crate::wayland::input_region::{InputRegion, InteractiveRect};

#[derive(Debug, Clone)]
enum UserEvent {
    EmitToFrontend(HostEvent),
    ApplyInputRegion(InputRegion),
    OpenWindow(String),
    Terminate,
}

struct WindowEntry {
    window: Window,
    webview: WebView,
}

pub fn run(config: AppConfig) -> Result<()> {
    eprintln!("discovered N.E.K.O repo root: {}", config.repo_root.display());
    let launcher_runtime = launcher::start_launcher(&config.repo_root)?;
    let frontend_ports = launcher::wait_for_frontend_url(&launcher_runtime)?;
    let frontend_url = frontend_ports
        .frontend_url()
        .context("launcher did not provide MAIN_SERVER_PORT")?;
    eprintln!("loading frontend url: {frontend_url}");
    let profile = WaylandProfile::detect();
    eprintln!("detected Wayland profile: {profile:#?}");
    let mut input_backend = PrototypeBackend::new(&profile);
    eprintln!(
        "selected window strategy: {:?} ({})",
        input_backend.strategy().tier,
        input_backend.strategy().reason
    );
    input_backend.apply_input_region(&InputRegion::from_rects(vec![InteractiveRect {
        x: 0,
        y: 0,
        width: 320,
        height: 480,
    }]))?;

    let event_loop: EventLoop<UserEvent> = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    install_signal_handler(proxy.clone())?;
    let first_window = create_window(
        &event_loop,
        &proxy,
        &config.app_title,
        &frontend_url,
        &profile,
        input_backend.strategy().clone(),
    )?;
    let first_window_id = first_window.window.id();
    let mut windows = HashMap::new();
    windows.insert(first_window_id, first_window);

    let ready_event = HostEvent::Ready {
        profile: WaylandProfileSnapshot::from(&profile),
        strategy: StrategySelectionSnapshot::from(input_backend.strategy()),
    };

    let _launcher_runtime = launcher_runtime;

    event_loop.run(move |event, event_loop_window_target, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            Event::NewEvents(StartCause::Init) => {
                if let Some(entry) = windows.get(&first_window_id) {
                    let _ = entry.webview.evaluate_script(
                        &build_emit_script(&ready_event)
                            .unwrap_or_else(|_| "console.error('failed to emit ready event');".to_string()),
                    );
                }
            }
            Event::UserEvent(user_event) => {
                match user_event {
                    UserEvent::EmitToFrontend(host_event) => {
                        if let Ok(script) = build_emit_script(&host_event) {
                            for entry in windows.values() {
                                let _ = entry.webview.evaluate_script(&script);
                            }
                        }
                    }
                    UserEvent::ApplyInputRegion(region) => {
                        let rect_count = region.rects().len();
                        match input_backend.apply_input_region(&region) {
                            Ok(()) => {
                                if let Ok(script) = build_emit_script(&HostEvent::InputRegionApplied {
                                    rect_count,
                                }) {
                                    for entry in windows.values() {
                                        let _ = entry.webview.evaluate_script(&script);
                                    }
                                }
                            }
                            Err(err) => {
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
                    UserEvent::OpenWindow(url) => {
                        match create_window(
                            event_loop_window_target,
                            &proxy,
                            &config.app_title,
                            &url,
                            &profile,
                            input_backend.strategy().clone(),
                        ) {
                            Ok(entry) => {
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
                        *control_flow = ControlFlow::Exit;
                    }
                }
            }
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                window_id,
                ..
            } => {
                windows.remove(&window_id);
                if windows.is_empty() {
                    *control_flow = ControlFlow::Exit;
                }
            }
            _ => {}
        }
    })
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
    strategy: crate::wayland::engine::StrategySelection,
) -> Result<WindowEntry> {
    let window = WindowBuilder::new()
        .with_title(app_title)
        .with_resizable(true)
        .build(event_loop)
        .context("failed to create tao window")?;

    let new_window_proxy = proxy.clone();
    let builder = WebViewBuilder::new()
        .with_url(url)
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
    let webview = builder
        .build(&window)
        .context("failed to build webview")?;

    Ok(WindowEntry { window, webview })
}
