use anyhow::{Context, Result};
use tao::event::{Event, StartCause, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoop, EventLoopBuilder, EventLoopProxy};
use tao::window::WindowBuilder;
use wry::WebViewBuilder;
#[cfg(target_os = "linux")]
use wry::WebViewBuilderExtUnix;

#[cfg(target_os = "linux")]
use tao::platform::unix::WindowExtUnix;

use crate::config::AppConfig;
use crate::ipc::{HostAction, HostEvent, StrategySelectionSnapshot, WaylandProfileSnapshot, build_emit_script, handle_frontend_message, init_script};
use crate::wayland::detect::WaylandProfile;
use crate::wayland::engine::{PrototypeBackend, WindowInputBackend};
use crate::wayland::input_region::{InputRegion, InteractiveRect};

#[derive(Debug, Clone)]
enum UserEvent {
    EmitToFrontend(HostEvent),
    ApplyInputRegion(InputRegion),
}

pub fn run(config: AppConfig) -> Result<()> {
    eprintln!("discovered N.E.K.O repo root: {}", config.repo_root.display());
    eprintln!("loading frontend url: {}", config.frontend_url);
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
    let window = WindowBuilder::new()
        .with_title(config.app_title.clone())
        .with_resizable(true)
        .build(&event_loop)
        .context("failed to create tao window")?;

    let builder = WebViewBuilder::new()
        .with_url(&config.frontend_url)
        .with_initialization_script(init_script())
        .with_ipc_handler(make_ipc_handler(
            proxy.clone(),
            profile.clone(),
            input_backend.strategy().clone(),
        ));

    #[cfg(target_os = "linux")]
    let _webview = {
        let vbox = window
            .default_vbox()
            .context("tao window does not provide default GTK vbox on Linux")?;
        builder
            .build_gtk(vbox)
            .context("failed to build GTK webview for Wayland/X11")?
    };

    #[cfg(not(target_os = "linux"))]
    let _webview = builder
        .build(&window)
        .context("failed to build webview")?;

    let ready_event = HostEvent::Ready {
        profile: WaylandProfileSnapshot::from(&profile),
        strategy: StrategySelectionSnapshot::from(input_backend.strategy()),
    };

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            Event::NewEvents(StartCause::Init) => {
                let _ = _webview.evaluate_script(
                    &build_emit_script(&ready_event)
                        .unwrap_or_else(|_| "console.error('failed to emit ready event');".to_string()),
                );
            }
            Event::UserEvent(user_event) => {
                match user_event {
                    UserEvent::EmitToFrontend(host_event) => {
                        if let Ok(script) = build_emit_script(&host_event) {
                            let _ = _webview.evaluate_script(&script);
                        }
                    }
                    UserEvent::ApplyInputRegion(region) => {
                        let rect_count = region.rects().len();
                        match input_backend.apply_input_region(&region) {
                            Ok(()) => {
                                if let Ok(script) = build_emit_script(&HostEvent::InputRegionApplied {
                                    rect_count,
                                }) {
                                    let _ = _webview.evaluate_script(&script);
                                }
                            }
                            Err(err) => {
                                if let Ok(script) = build_emit_script(&HostEvent::Error {
                                    message: format!("failed to apply input region: {err}"),
                                }) {
                                    let _ = _webview.evaluate_script(&script);
                                }
                            }
                        }
                    }
                }
            }
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                *control_flow = ControlFlow::Exit;
            }
            _ => {}
        }
    })
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
