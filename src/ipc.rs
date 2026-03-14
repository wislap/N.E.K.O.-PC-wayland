use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::wayland::detect::WaylandProfile;
use crate::wayland::engine::StrategySelection;
use crate::wayland::input_region::{InputRegion, InteractiveRect};

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HostEvent {
    Ready {
        profile: WaylandProfileSnapshot,
        strategy: StrategySelectionSnapshot,
    },
    Pong {
        nonce: Option<Value>,
    },
    HostInfo {
        profile: WaylandProfileSnapshot,
        strategy: StrategySelectionSnapshot,
    },
    LogAck {
        message: String,
    },
    InputRegionApplied {
        rect_count: usize,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum FrontendCommand {
    Ping { nonce: Option<Value> },
    GetHostInfo,
    Log { message: String },
    SetInputRegion { rects: Vec<InteractiveRectPayload> },
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct InteractiveRectPayload {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl From<InteractiveRectPayload> for InteractiveRect {
    fn from(value: InteractiveRectPayload) -> Self {
        Self {
            x: value.x,
            y: value.y,
            width: value.width,
            height: value.height,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct WaylandProfileSnapshot {
    pub session_type: String,
    pub compositor_family: String,
    pub current_desktop: Option<String>,
    pub session_desktop: Option<String>,
    pub wayland_display: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StrategySelectionSnapshot {
    pub tier: String,
    pub reason: String,
}

impl From<&WaylandProfile> for WaylandProfileSnapshot {
    fn from(value: &WaylandProfile) -> Self {
        Self {
            session_type: format!("{:?}", value.session_type).to_ascii_lowercase(),
            compositor_family: format!("{:?}", value.compositor_family).to_ascii_lowercase(),
            current_desktop: value.current_desktop.clone(),
            session_desktop: value.session_desktop.clone(),
            wayland_display: value.wayland_display.clone(),
        }
    }
}

impl From<&StrategySelection> for StrategySelectionSnapshot {
    fn from(value: &StrategySelection) -> Self {
        Self {
            tier: format!("{:?}", value.tier).to_ascii_lowercase(),
            reason: value.reason.to_string(),
        }
    }
}

pub fn init_script() -> &'static str {
    r#"
(function () {
  const listeners = new Set();
  function emit(payload) {
    for (const listener of listeners) {
      try { listener(payload); } catch (err) { console.error('[NekoHost] listener error', err); }
    }
    window.dispatchEvent(new CustomEvent('neko-host-message', { detail: payload }));
  }
  window.__NEKO_HOST_EMIT__ = emit;
  window.NekoHost = {
    postMessage(message) {
      const payload = typeof message === 'string' ? message : JSON.stringify(message);
      if (!window.ipc || typeof window.ipc.postMessage !== 'function') {
        throw new Error('window.ipc.postMessage is unavailable');
      }
      window.ipc.postMessage(payload);
    },
    addEventListener(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    removeEventListener(listener) {
      listeners.delete(listener);
    },
    setInputRegion(rects) {
      window.NekoHost.postMessage({ cmd: 'set_input_region', rects });
    }
  };
})();
"#
}

pub enum HostAction {
    Emit(HostEvent),
    ApplyInputRegion(InputRegion),
}

pub fn handle_frontend_message(
    raw: &str,
    profile: &WaylandProfile,
    strategy: &StrategySelection,
) -> HostAction {
    match serde_json::from_str::<FrontendCommand>(raw) {
        Ok(FrontendCommand::Ping { nonce }) => HostAction::Emit(HostEvent::Pong { nonce }),
        Ok(FrontendCommand::GetHostInfo) => HostAction::Emit(HostEvent::HostInfo {
            profile: profile.into(),
            strategy: strategy.into(),
        }),
        Ok(FrontendCommand::Log { message }) => {
            eprintln!("[frontend] {message}");
            HostAction::Emit(HostEvent::LogAck { message })
        }
        Ok(FrontendCommand::SetInputRegion { rects }) => HostAction::ApplyInputRegion(
            InputRegion::from_rects(rects.into_iter().map(InteractiveRect::from).collect()),
        ),
        Err(err) => HostAction::Emit(HostEvent::Error {
            message: format!("invalid frontend IPC payload: {err}"),
        }),
    }
}

pub fn build_emit_script(event: &HostEvent) -> Result<String> {
    let json = serde_json::to_string(event).map_err(|err| anyhow!(err))?;
    Ok(format!(
        "window.__NEKO_HOST_EMIT__ && window.__NEKO_HOST_EMIT__({json});"
    ))
}
