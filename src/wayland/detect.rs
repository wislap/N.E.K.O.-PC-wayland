use std::env;

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum SessionType {
    Wayland,
    X11,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum CompositorFamily {
    Mutter,
    KWin,
    Wlroots,
    Niri,
    Unknown,
}

#[derive(Debug, Clone, Serialize)]
pub struct WaylandProfile {
    pub session_type: SessionType,
    pub compositor_family: CompositorFamily,
    pub current_desktop: Option<String>,
    pub session_desktop: Option<String>,
    pub wayland_display: Option<String>,
}

impl WaylandProfile {
    pub fn detect() -> Self {
        let current_desktop = env::var("XDG_CURRENT_DESKTOP").ok();
        let session_desktop = env::var("XDG_SESSION_DESKTOP").ok();
        let session_type = match env::var("XDG_SESSION_TYPE").ok().as_deref() {
            Some("wayland") => SessionType::Wayland,
            Some("x11") => SessionType::X11,
            _ => SessionType::Unknown,
        };
        let wayland_display = env::var("WAYLAND_DISPLAY").ok();
        let compositor_family =
            infer_compositor_family(current_desktop.as_deref(), session_desktop.as_deref());

        Self {
            session_type,
            compositor_family,
            current_desktop,
            session_desktop,
            wayland_display,
        }
    }

    pub fn is_wayland(&self) -> bool {
        self.session_type == SessionType::Wayland || self.wayland_display.is_some()
    }
}

fn infer_compositor_family(
    current_desktop: Option<&str>,
    session_desktop: Option<&str>,
) -> CompositorFamily {
    let normalized = format!(
        "{}|{}",
        current_desktop.unwrap_or_default().to_ascii_lowercase(),
        session_desktop.unwrap_or_default().to_ascii_lowercase()
    );

    if normalized.contains("gnome") {
        CompositorFamily::Mutter
    } else if normalized.contains("kde") || normalized.contains("plasma") {
        CompositorFamily::KWin
    } else if normalized.contains("niri") {
        CompositorFamily::Niri
    } else if normalized.contains("sway")
        || normalized.contains("hypr")
        || normalized.contains("river")
        || normalized.contains("wayfire")
        || normalized.contains("labwc")
    {
        CompositorFamily::Wlroots
    } else {
        CompositorFamily::Unknown
    }
}
