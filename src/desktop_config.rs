use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PortConfig {
    #[serde(rename = "MAIN_SERVER_PORT")]
    pub main_server_port: u16,
    #[serde(rename = "MEMORY_SERVER_PORT")]
    pub memory_server_port: u16,
    #[serde(rename = "TOOL_SERVER_PORT")]
    pub tool_server_port: u16,
    #[serde(rename = "USER_PLUGIN_SERVER_PORT")]
    pub user_plugin_server_port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct DesktopAppConfig {
    pub api_base_url: Option<String>,
    pub auto_launch: Option<bool>,
    pub compatibility_mode: Option<bool>,
    pub use_system_proxy: Option<bool>,
    pub streamer_mode: Option<bool>,
    pub dark_mode: Option<bool>,
    pub custom_ports: Option<PortConfig>,
    pub transparent_background: Option<bool>,
    pub fullscreen: Option<bool>,
    pub window_width: Option<u32>,
    pub window_height: Option<u32>,
    pub render_width: Option<u32>,
    pub render_height: Option<u32>,
    pub render_fps: Option<u32>,
    pub target_display_id: Option<String>,
    pub target_display_index: Option<usize>,
    pub target_display_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
struct LegacyDesktopConfig {
    #[serde(rename = "apiBaseUrl")]
    api_base_url: Option<String>,
    #[serde(rename = "autoLaunch")]
    auto_launch: Option<bool>,
    #[serde(rename = "compatibilityMode")]
    compatibility_mode: Option<bool>,
    #[serde(rename = "useSystemProxy")]
    use_system_proxy: Option<bool>,
    #[serde(rename = "streamerMode")]
    streamer_mode: Option<bool>,
    #[serde(rename = "darkMode")]
    dark_mode: Option<bool>,
    #[serde(rename = "customPorts")]
    custom_ports: Option<PortConfig>,
}

pub fn load_or_migrate(repo_root: &Path) -> Result<DesktopAppConfig> {
    let path = app_config_path();
    if path.exists() {
        return load_from_path(&path);
    }

    let mut migrated = DesktopAppConfig::default();
    if let Some(legacy) = load_legacy_config(repo_root)? {
        migrated.api_base_url = legacy.api_base_url;
        migrated.auto_launch = legacy.auto_launch;
        migrated.compatibility_mode = legacy.compatibility_mode;
        migrated.use_system_proxy = legacy.use_system_proxy;
        migrated.streamer_mode = legacy.streamer_mode;
        migrated.dark_mode = legacy.dark_mode;
        migrated.custom_ports = legacy.custom_ports;
    }

    if migrated.custom_ports.is_none() {
        migrated.custom_ports = load_shared_port_config().ok().flatten();
    }

    save(&migrated)?;
    if let Some(ports) = &migrated.custom_ports {
        write_shared_port_config(ports)?;
    }
    Ok(migrated)
}

pub fn save(config: &DesktopAppConfig) -> Result<()> {
    let path = app_config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!("failed to create desktop config directory {}", parent.display())
        })?;
    }
    atomic_write_json(&path, config)
}

pub fn persist_dark_mode(enabled: bool) -> Result<()> {
    let path = app_config_path();
    let mut config = if path.exists() {
        load_from_path(&path)?
    } else {
        DesktopAppConfig::default()
    };
    config.dark_mode = Some(enabled);
    save(&config)
}

pub fn write_shared_port_config(ports: &PortConfig) -> Result<()> {
    let path = shared_port_config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!("failed to create shared config directory {}", parent.display())
        })?;
    }
    atomic_write_json(&path, ports)
}

fn load_from_path(path: &Path) -> Result<DesktopAppConfig> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read desktop config {}", path.display()))?;
    serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse desktop config {}", path.display()))
}

fn load_shared_port_config() -> Result<Option<PortConfig>> {
    let path = shared_port_config_path();
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&path)
        .with_context(|| format!("failed to read shared port config {}", path.display()))?;
    let ports = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse shared port config {}", path.display()))?;
    Ok(Some(ports))
}

fn load_legacy_config(repo_root: &Path) -> Result<Option<LegacyDesktopConfig>> {
    for candidate in legacy_config_candidates(repo_root) {
        if !candidate.exists() {
            continue;
        }
        let raw = fs::read_to_string(&candidate)
            .with_context(|| format!("failed to read legacy config {}", candidate.display()))?;
        let parsed = serde_json::from_str(&raw)
            .with_context(|| format!("failed to parse legacy config {}", candidate.display()))?;
        return Ok(Some(parsed));
    }
    Ok(None)
}

fn legacy_config_candidates(repo_root: &Path) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(path) = env::var("NEKO_PC_LEGACY_CONFIG") {
        candidates.push(PathBuf::from(path));
    }
    candidates.push(repo_root.join("core_config.txt"));
    if let Some(parent) = repo_root.parent() {
        candidates.push(parent.join("N.E.K.O.-PC/core_config.txt"));
    }
    candidates.push(shared_config_dir().join("core_config.txt"));
    candidates
}

fn app_config_path() -> PathBuf {
    shared_config_dir().join("wayland_app_config.json")
}

fn shared_port_config_path() -> PathBuf {
    shared_config_dir().join("port_config.json")
}

pub fn cef_profile_root() -> PathBuf {
    shared_config_dir().join("cef-profile")
}

pub fn shared_config_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        let base = env::var("APPDATA").unwrap_or_else(|_| {
            PathBuf::from(env::var("USERPROFILE").unwrap_or_else(|_| ".".to_string()))
                .join("AppData")
                .join("Roaming")
                .display()
                .to_string()
        });
        return PathBuf::from(base).join("N.E.K.O");
    }

    #[cfg(target_os = "macos")]
    {
        let home = env::var("HOME").unwrap_or_else(|_| ".".to_string());
        return PathBuf::from(home)
            .join("Library")
            .join("Application Support")
            .join("N.E.K.O");
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let base = env::var("XDG_CONFIG_HOME").unwrap_or_else(|_| {
            let home = env::var("HOME").unwrap_or_else(|_| ".".to_string());
            PathBuf::from(home).join(".config").display().to_string()
        });
        PathBuf::from(base).join("N.E.K.O")
    }
}

fn atomic_write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let temp_path = path.with_extension("tmp");
    let payload = serde_json::to_vec_pretty(value).context("failed to serialize config json")?;
    fs::write(&temp_path, payload)
        .with_context(|| format!("failed to write temporary config {}", temp_path.display()))?;
    fs::rename(&temp_path, path).with_context(|| {
        format!(
            "failed to move temporary config {} into place at {}",
            temp_path.display(),
            path.display()
        )
    })?;
    Ok(())
}
