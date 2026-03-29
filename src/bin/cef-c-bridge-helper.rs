use anyhow::{Result, bail};
#[cfg(feature = "cef_osr")]
use anyhow::Context;

#[cfg(feature = "cef_osr")]
use neko_pc_wayland::cef::{
    CefCBridgeConfig, CefCBridgeRuntime, CefLifecycleEvent, clear_event_callback,
    install_event_callback, run_raw_input_loop_cbridge, try_run_c_subprocess,
};
#[cfg(feature = "cef_osr")]
use neko_pc_wayland::wayland::input_region::{InputRegion, InteractiveRect};
#[cfg(feature = "cef_osr")]
use neko_pc_wayland::wayland::raw_host::{RawHostConfig, spawn};

fn main() {
    if let Err(err) = run() {
        eprintln!("cef-c-bridge-helper failed: {err:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    #[cfg(feature = "cef_osr")]
    if let Some(code) = try_run_c_subprocess()? {
        std::process::exit(code);
    }

    #[cfg(not(feature = "cef_osr"))]
    {
        bail!("build with --features cef_osr");
    }

    #[cfg(feature = "cef_osr")]
    {
        let url = resolve_url()?;
        let width = resolve_dimension("NEKO_CEF_HELPER_WIDTH", 800)?;
        let height = resolve_dimension("NEKO_CEF_HELPER_HEIGHT", 600)?;
        let transparent = resolve_flag("NEKO_CEF_HELPER_TRANSPARENT");
        let frame_rate = resolve_dimension("NEKO_CEF_HELPER_FRAME_RATE", 30)?;
        if let Some(runtime_dir) = discover_runtime_dir()? {
            std::env::set_current_dir(&runtime_dir).with_context(|| {
                format!(
                    "failed to switch cef-c-bridge-helper working directory to {}",
                    runtime_dir.display()
                )
            })?;
            eprintln!(
                "cef-c-bridge-helper switched working directory to {}",
                runtime_dir.display()
            );
        }

        let mut host_config = RawHostConfig::probe();
        host_config.title = "cef-c-bridge-helper".to_string();
        host_config.app_id = "moe.neko.cef-c-bridge-helper".to_string();
        host_config.width = width;
        host_config.height = height;
        host_config.install_ctrlc_handler = true;
        host_config.move_on_left_press = false;
        host_config.show_debug_regions_when_empty = false;
        host_config.transparent_outside_input_region = transparent;

        let full_region = InputRegion::from_rects(vec![InteractiveRect {
            x: 0,
            y: 0,
            width,
            height,
        }]);
        host_config.input_region = full_region.clone();
        host_config.visible_region = Some(full_region);

        install_event_callback(|event| match event {
            CefLifecycleEvent::BrowserCreated => {
                eprintln!("CEF_C_BRIDGE_EVENT browser_created");
            }
            CefLifecycleEvent::BrowserBeforeClose => {
                eprintln!("CEF_C_BRIDGE_EVENT browser_before_close");
            }
            CefLifecycleEvent::LoadStart { transition_type } => {
                eprintln!("CEF_C_BRIDGE_EVENT load_start transition_type={transition_type}");
            }
            CefLifecycleEvent::LoadEnd { http_status_code } => {
                eprintln!("CEF_C_BRIDGE_EVENT load_end http_status_code={http_status_code}");
            }
            CefLifecycleEvent::LoadError {
                error_code,
                error_text,
                failed_url,
            } => {
                eprintln!(
                    "CEF_C_BRIDGE_EVENT load_error error_code={} error_text={:?} failed_url={:?}",
                    error_code, error_text, failed_url
                );
            }
            CefLifecycleEvent::LoadingStateChange {
                is_loading,
                can_go_back,
                can_go_forward,
            } => {
                eprintln!(
                    "CEF_C_BRIDGE_EVENT loading_state is_loading={} can_go_back={} can_go_forward={}",
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
                    "CEF_C_BRIDGE_EVENT console level={} source={:?}:{} message={:?}",
                    level, source, line, message
                );
            }
        });

        eprintln!(
            "cef-c-bridge-helper starting raw host url={} size={}x{} transparent={} fps={}",
            url, width, height, transparent, frame_rate
        );

        let mut cef_config = CefCBridgeConfig::demo();
        cef_config.url = url;
        cef_config.width = width;
        cef_config.height = height;
        cef_config.transparent_painting = transparent;
        cef_config.frame_rate = frame_rate;

        let runtime = CefCBridgeRuntime::initialize(cef_config)
            .context("failed to initialize CEF C bridge runtime")?;
        eprintln!(
            "cef-c-bridge-helper runtime initialized backend=c_shim loop_mode={:?}",
            runtime.message_loop_mode()
        );

        let raw_host = spawn(host_config).context("failed to spawn raw Wayland host")?;
        let handle = raw_host.handle.clone();
        let pointer_events = handle.subscribe_pointer_events();
        let keyboard_events = handle.subscribe_keyboard_events();

        match runtime.attach_browser(handle.clone()) {
            Ok(bridge) => {
                eprintln!(
                    "cef-c-bridge-helper attached backend={} loop_mode={:?} url={}",
                    bridge.backend(),
                    bridge.message_loop_mode(),
                    bridge.config().url
                );
                run_raw_input_loop_cbridge(&bridge, &handle, pointer_events, keyboard_events);
                bridge.request_close();
                clear_event_callback();
                raw_host
                    .join()
                    .map_err(|_| anyhow::anyhow!("raw host thread panicked"))??;
                Ok(())
            }
            Err(err) => {
                clear_event_callback();
                let _ = handle.shutdown();
                let _ = raw_host.join();
                Err(err)
            }
        }
    }
}

#[cfg(feature = "cef_osr")]
fn resolve_url() -> Result<String> {
    if let Some(value) = std::env::args().nth(1) {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }

    if let Ok(value) = std::env::var("NEKO_CEF_HELPER_URL") {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }

    Ok("https://example.com".to_string())
}

#[cfg(feature = "cef_osr")]
fn resolve_dimension(name: &str, default: u32) -> Result<u32> {
    let Some(raw) = std::env::var_os(name) else {
        return Ok(default);
    };
    let value = raw
        .to_string_lossy()
        .trim()
        .parse::<u32>()
        .with_context(|| format!("invalid {name} value {:?}", raw))?;
    if value == 0 {
        bail!("{name} must be greater than zero");
    }
    Ok(value)
}

#[cfg(feature = "cef_osr")]
fn resolve_flag(name: &str) -> bool {
    matches!(
        std::env::var(name).ok().as_deref(),
        Some("1")
            | Some("true")
            | Some("TRUE")
            | Some("yes")
            | Some("YES")
            | Some("on")
            | Some("ON")
    )
}

#[cfg(feature = "cef_osr")]
fn discover_runtime_dir() -> Result<Option<std::path::PathBuf>> {
    if let Ok(value) = std::env::var("NEKO_CEF_RUNTIME_DIR") {
        let path = std::path::PathBuf::from(value);
        if path.exists() {
            return Ok(Some(path));
        }
    }

    let exe = std::env::current_exe().context("failed to resolve current executable path")?;
    let Some(exe_dir) = exe.parent() else {
        return Ok(None);
    };

    let candidate = exe_dir.join("cef-official-helper");
    if candidate.join("libcef.so").exists() {
        return Ok(Some(candidate));
    }

    Ok(None)
}
