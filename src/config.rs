use std::env;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::wayland::input_region::{InputRegion, InteractiveRect};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaylandHostMode {
    Legacy,
    Companion,
    RawOnly,
    OfficialHelperProbe,
    OfficialHelperRun,
    CStandaloneProbe,
}

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub repo_root: PathBuf,
    pub app_title: String,
    pub debug_input_region: Option<InputRegion>,
    pub trace_input_region: bool,
    pub wayland_host_mode: WaylandHostMode,
    pub window_width: u32,
    pub window_height: u32,
    pub render_width: u32,
    pub render_height: u32,
    pub render_fps: u32,
    pub fullscreen: bool,
    pub transparent_background: bool,
}

impl AppConfig {
    pub fn discover() -> Result<Self> {
        let repo_root = if let Ok(value) = env::var("NEKO_REPO_ROOT") {
            PathBuf::from(value)
        } else {
            discover_repo_root()?
        };

        if !repo_root.join("launcher.py").exists() {
            bail!(
                "repo root {:?} does not look like N.E.K.O (missing launcher.py)",
                repo_root
            );
        }

        Ok(Self {
            repo_root,
            app_title: "N.E.K.O.-PC-wayland".to_string(),
            debug_input_region: parse_debug_input_region()?,
            trace_input_region: env_flag("NEKO_WAYLAND_TRACE_INPUT_REGION"),
            wayland_host_mode: parse_wayland_host_mode()?,
            window_width: parse_dimension("NEKO_WAYLAND_WINDOW_WIDTH", 1920)?,
            window_height: parse_dimension("NEKO_WAYLAND_WINDOW_HEIGHT", 1080)?,
            render_width: parse_dimension("NEKO_WAYLAND_RENDER_WIDTH", default_render_width())?,
            render_height: parse_dimension(
                "NEKO_WAYLAND_RENDER_HEIGHT",
                default_render_height(),
            )?,
            render_fps: parse_dimension("NEKO_WAYLAND_RENDER_FPS", default_render_fps())?,
            fullscreen: parse_fullscreen_flag(),
            transparent_background: env_flag("NEKO_WAYLAND_TRANSPARENT_BACKGROUND"),
        })
    }
}

fn default_render_width() -> u32 {
    if parse_fullscreen_flag() { 1280 } else { 800 }
}

fn default_render_height() -> u32 {
    if parse_fullscreen_flag() { 720 } else { 600 }
}

fn default_render_fps() -> u32 {
    if parse_fullscreen_flag() { 12 } else { 30 }
}

fn parse_dimension(name: &str, default: u32) -> Result<u32> {
    let Some(raw) = env::var_os(name) else {
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

fn parse_fullscreen_flag() -> bool {
    match env::var("NEKO_WAYLAND_FULLSCREEN") {
        Ok(value) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => false,
    }
}

fn discover_repo_root() -> Result<PathBuf> {
    let current = env::current_dir().context("failed to read current dir")?;
    if let Some(root) = find_neko_repo(&current) {
        return Ok(root);
    }

    let exe = env::current_exe().context("failed to read current executable path")?;
    if let Some(parent) = exe.parent() {
        if let Some(root) = find_neko_repo(parent) {
            return Ok(root);
        }
    }

    bail!("unable to discover N.E.K.O repo root, set NEKO_REPO_ROOT explicitly")
}

fn find_neko_repo(base: &Path) -> Option<PathBuf> {
    for dir in base.ancestors() {
        if dir.join("launcher.py").exists() && dir.join("main_server.py").exists() {
            return Some(dir.to_path_buf());
        }

        let sibling = dir.join("N.E.K.O");
        if sibling.join("launcher.py").exists() && sibling.join("main_server.py").exists() {
            return Some(sibling);
        }
    }
    None
}

fn parse_debug_input_region() -> Result<Option<InputRegion>> {
    let Some(raw) = env::var_os("NEKO_WAYLAND_DEBUG_INPUT_REGION") else {
        return Ok(None);
    };

    let raw = raw.to_string_lossy();
    let mut rects = Vec::new();

    for chunk in raw.split(';').filter(|chunk| !chunk.trim().is_empty()) {
        let parts = chunk.split(',').map(str::trim).collect::<Vec<_>>();
        if parts.len() != 4 {
            bail!(
                "invalid NEKO_WAYLAND_DEBUG_INPUT_REGION entry {:?}, expected x,y,width,height",
                chunk
            );
        }

        let x = parts[0].parse::<i32>().with_context(|| {
            format!(
                "invalid x value {:?} in NEKO_WAYLAND_DEBUG_INPUT_REGION",
                parts[0]
            )
        })?;
        let y = parts[1].parse::<i32>().with_context(|| {
            format!(
                "invalid y value {:?} in NEKO_WAYLAND_DEBUG_INPUT_REGION",
                parts[1]
            )
        })?;
        let width = parts[2].parse::<u32>().with_context(|| {
            format!(
                "invalid width value {:?} in NEKO_WAYLAND_DEBUG_INPUT_REGION",
                parts[2]
            )
        })?;
        let height = parts[3].parse::<u32>().with_context(|| {
            format!(
                "invalid height value {:?} in NEKO_WAYLAND_DEBUG_INPUT_REGION",
                parts[3]
            )
        })?;

        rects.push(InteractiveRect {
            x,
            y,
            width,
            height,
        });
    }

    Ok(Some(InputRegion::from_rects(rects)))
}

fn env_flag(name: &str) -> bool {
    matches!(
        env::var(name).ok().as_deref(),
        Some("1")
            | Some("true")
            | Some("TRUE")
            | Some("yes")
            | Some("YES")
            | Some("on")
            | Some("ON")
    )
}

fn parse_wayland_host_mode() -> Result<WaylandHostMode> {
    if env_flag("NEKO_WAYLAND_ENABLE_RAW_HOST_COMPANION") {
        return Ok(WaylandHostMode::Companion);
    }

    let Some(raw) = env::var_os("NEKO_WAYLAND_HOST_MODE") else {
        return Ok(WaylandHostMode::Legacy);
    };

    match raw.to_string_lossy().trim().to_ascii_lowercase().as_str() {
        "" | "legacy" | "default" | "tao" | "wry" => Ok(WaylandHostMode::Legacy),
        "companion" | "mirror" => Ok(WaylandHostMode::Companion),
        "raw_only" | "raw-only" | "raw" => Ok(WaylandHostMode::RawOnly),
        "official_helper_probe" | "official-helper-probe" | "helper_probe" => {
            Ok(WaylandHostMode::OfficialHelperProbe)
        }
        "official_helper_run" | "official-helper-run" | "helper_run" | "official_helper"
        | "official-helper" => Ok(WaylandHostMode::OfficialHelperRun),
        "c_standalone_probe" | "c-standalone-probe" | "c_helper_probe" | "c-helper-probe" => {
            Ok(WaylandHostMode::CStandaloneProbe)
        }
        other => bail!(
            "invalid NEKO_WAYLAND_HOST_MODE value {:?}, expected legacy|companion|raw_only|official_helper_probe|official_helper_run|c_standalone_probe",
            other
        ),
    }
}
