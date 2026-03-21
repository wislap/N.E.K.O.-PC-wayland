use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::OnceLock;
use std::thread;
use std::time::Duration;

#[cfg(unix)]
use std::os::unix::process::CommandExt;

use anyhow::{Context, Result, anyhow, bail};
use serde_json::from_str;
use xkeysym::key;

use crate::ipc::InteractiveRectPayload;
use crate::wayland::raw_host::{
    RawHostKeyboardEvent, RawHostModifiers, RawHostPointerButton, RawHostPointerEvent,
};

const EVENT_PREFIX: &str = "NEKO_CEF_STANDALONE_EVENT ";

#[derive(Debug, Clone)]
pub struct CStandaloneHelperConfig {
    pub runtime_dir: PathBuf,
    pub executable: PathBuf,
    pub args: Vec<String>,
    pub extra_envs: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
pub enum CStandaloneHelperEvent {
    Startup,
    InitializeOk,
    InitializeFailed { message: String },
    CreateBrowserOk,
    CreateBrowserFailed { message: String },
    BrowserCreated,
    LoadStart { transition_type: i32 },
    LoadEnd { http_status_code: i32 },
    Paint {
        element_type: i32,
        width: i32,
        height: i32,
        frame: i32,
    },
    InputRegion {
        rects: Vec<InteractiveRectPayload>,
    },
    DragRegion {
        rects: Vec<InteractiveRectPayload>,
    },
    DragExclusionRegion {
        rects: Vec<InteractiveRectPayload>,
    },
    BrowserBeforeClose,
    BrowserReleased,
    ShutdownOk,
    SubprocessExit { code: i32 },
    RawLine { event: String, fields: Vec<(String, String)> },
}

#[derive(Debug)]
pub struct CStandaloneHelperHandle {
    child: Child,
    stdin: ChildStdin,
    events: Receiver<CStandaloneHelperEvent>,
}

impl CStandaloneHelperConfig {
    pub fn discover(repo_root: &Path) -> Result<Self> {
        let helper = discover_c_standalone_helper(repo_root)?;
        let runtime_dir = discover_staged_cef_runtime_dir(repo_root)?;
        Ok(Self {
            runtime_dir,
            executable: helper,
            args: vec!["https://example.com".to_string()],
            extra_envs: Vec::new(),
        })
    }

    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        let url = url.into();
        self.args.clear();
        if !url.trim().is_empty() {
            self.args.push(url);
        }
        self
    }

    pub fn with_env(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.extra_envs.push((name.into(), value.into()));
        self
    }
}

impl CStandaloneHelperHandle {
    pub fn spawn(config: &CStandaloneHelperConfig) -> Result<Self> {
        let cache_root = std::env::temp_dir().join(format!(
            "neko-cef-standalone-cache-{}-{}",
            std::process::id(),
            current_millis()
        ));
        let cache_dir = cache_root.join("cache");
        std::fs::create_dir_all(&cache_dir).with_context(|| {
            format!(
                "failed to create standalone helper cache dir {}",
                cache_dir.display()
            )
        })?;

        let mut command = Command::new(&config.executable);
        command
            .args(&config.args)
            .current_dir(&config.runtime_dir)
            .env("LD_LIBRARY_PATH", &config.runtime_dir)
            .env(
                "NEKO_CEF_BROWSER_SUBPROCESS_PATH",
                config.runtime_dir.join("cef-helper"),
            )
            .env("NEKO_CEF_RESOURCES_DIR", &config.runtime_dir)
            .env("NEKO_CEF_LOCALES_DIR", config.runtime_dir.join("locales"))
            .env("NEKO_CEF_LOCALE", "en-US")
            .env("NEKO_CEF_ROOT_CACHE_PATH", &cache_root)
            .env("NEKO_CEF_CACHE_PATH", &cache_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (name, value) in &config.extra_envs {
            command.env(name, value);
        }
        forward_env_if_present(&mut command, "NEKO_CEF_FRAME_DUMP_PATH");
        #[cfg(unix)]
        command.process_group(0);

        let mut child = command.spawn().with_context(|| {
            format!(
                "failed to spawn C standalone helper {}",
                config.executable.display()
            )
        })?;

        let stdout = child
            .stdout
            .take()
            .context("C standalone helper stdout is not piped")?;
        let stderr = child
            .stderr
            .take()
            .context("C standalone helper stderr is not piped")?;
        let stdin = child
            .stdin
            .take()
            .context("C standalone helper stdin is not piped")?;

        let (event_sender, events) = mpsc::channel::<CStandaloneHelperEvent>();
        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                match line {
                    Ok(line) => handle_stdout_line(&line, &event_sender),
                    Err(err) => {
                        eprintln!("[cef-c-standalone] failed reading stdout: {err}");
                        break;
                    }
                }
            }
        });

        thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                match line {
                    Ok(line) => eprintln!("[cef-c-standalone] {line}"),
                    Err(err) => {
                        eprintln!("[cef-c-standalone] failed reading stderr: {err}");
                        break;
                    }
                }
            }
        });

        Ok(Self {
            child,
            stdin,
            events,
        })
    }

    pub fn recv_event_timeout(&self, timeout: Duration) -> Result<Option<CStandaloneHelperEvent>> {
        match self.events.recv_timeout(timeout) {
            Ok(event) => Ok(Some(event)),
            Err(mpsc::RecvTimeoutError::Timeout) => Ok(None),
            Err(mpsc::RecvTimeoutError::Disconnected) => Ok(None),
        }
    }

    pub fn try_wait(&mut self) -> Result<Option<std::process::ExitStatus>> {
        self.child
            .try_wait()
            .map_err(|err| anyhow!("failed polling C standalone helper: {err}"))
    }

    pub fn wait(&mut self) -> Result<std::process::ExitStatus> {
        self.child
            .wait()
            .map_err(|err| anyhow!("failed waiting for C standalone helper: {err}"))
    }

    pub fn terminate(&mut self) -> Result<()> {
        self.child
            .kill()
            .map_err(|err| anyhow!("failed terminating C standalone helper: {err}"))
    }

    pub fn send_pointer_event(
        &mut self,
        event: RawHostPointerEvent,
        mouse_modifiers: &mut u32,
        key_modifiers: u32,
    ) -> Result<()> {
        const EVENTFLAG_PRECISION_SCROLLING_DELTA: u32 = 1 << 14;

        match event {
            RawHostPointerEvent::Enter { x, y } => {
                self.send_line("focus focused=1")?;
                self.send_line(format!(
                    "mouse_move x={} y={} leave=0 modifiers={}",
                    x.round() as i32,
                    y.round() as i32,
                    key_modifiers | *mouse_modifiers
                ))?;
            }
            RawHostPointerEvent::Leave { x, y } => {
                self.send_line(format!(
                    "mouse_move x={} y={} leave=1 modifiers={}",
                    x.round() as i32,
                    y.round() as i32,
                    key_modifiers | *mouse_modifiers
                ))?;
                self.send_line("focus focused=0")?;
            }
            RawHostPointerEvent::Motion { x, y } => {
                self.send_line(format!(
                    "mouse_move x={} y={} leave=0 modifiers={}",
                    x.round() as i32,
                    y.round() as i32,
                    key_modifiers | *mouse_modifiers
                ))?;
            }
            RawHostPointerEvent::Button {
                x,
                y,
                button,
                pressed,
            } => {
                let Some(button) = map_button(button) else {
                    return Ok(());
                };
                let flag = button_flag(button);
                if pressed {
                    *mouse_modifiers |= flag;
                } else {
                    *mouse_modifiers &= !flag;
                }
                self.send_line(format!(
                    "mouse_click x={} y={} button={} up={} clicks=1 modifiers={}",
                    x.round() as i32,
                    y.round() as i32,
                    button,
                    (!pressed) as i32,
                    key_modifiers | *mouse_modifiers
                ))?;
            }
            RawHostPointerEvent::Wheel {
                x,
                y,
                delta_x,
                delta_y,
            } => {
                self.send_line(format!(
                    "mouse_wheel x={} y={} dx={} dy={} modifiers={}",
                    x.round() as i32,
                    y.round() as i32,
                    delta_x.round() as i32,
                    delta_y.round() as i32,
                    key_modifiers | *mouse_modifiers | EVENTFLAG_PRECISION_SCROLLING_DELTA
                ))?;
            }
        }
        Ok(())
    }

    pub fn send_keyboard_event(&mut self, event: RawHostKeyboardEvent) -> Result<u32> {
        match event {
            RawHostKeyboardEvent::Press {
                raw_code,
                keysym,
                utf8,
                modifiers,
            } => {
                let cef_modifiers = map_modifiers(modifiers, keysym, false);
                let windows_key_code = map_windows_key_code(keysym, utf8.as_deref());
                self.send_line(format!(
                    "key kind=0 win={} native={} modifiers={} char=0 unmod=0",
                    windows_key_code, raw_code, cef_modifiers
                ))?;
                if let Some(text) = utf8 {
                    for ch in text.encode_utf16() {
                        self.send_line(format!(
                            "key kind=2 win={} native={} modifiers={} char={} unmod={}",
                            windows_key_code, raw_code, cef_modifiers, ch, ch
                        ))?;
                    }
                }
                Ok(map_modifiers(modifiers, keysym, false))
            }
            RawHostKeyboardEvent::Repeat {
                raw_code,
                keysym,
                utf8,
                modifiers,
            } => {
                let cef_modifiers = map_modifiers(modifiers, keysym, true);
                let windows_key_code = map_windows_key_code(keysym, utf8.as_deref());
                self.send_line(format!(
                    "key kind=0 win={} native={} modifiers={} char=0 unmod=0",
                    windows_key_code, raw_code, cef_modifiers
                ))?;
                if let Some(text) = utf8 {
                    for ch in text.encode_utf16() {
                        self.send_line(format!(
                            "key kind=2 win={} native={} modifiers={} char={} unmod={}",
                            windows_key_code, raw_code, cef_modifiers, ch, ch
                        ))?;
                    }
                }
                Ok(map_modifiers(modifiers, keysym, false))
            }
            RawHostKeyboardEvent::Release {
                raw_code,
                keysym,
                modifiers,
            } => {
                let cef_modifiers = map_modifiers(modifiers, keysym, false);
                let windows_key_code = map_windows_key_code(keysym, None);
                self.send_line(format!(
                    "key kind=1 win={} native={} modifiers={} char=0 unmod=0",
                    windows_key_code, raw_code, cef_modifiers
                ))?;
                Ok(map_modifiers(modifiers, keysym, false))
            }
        }
    }

    pub fn send_shutdown(&mut self) -> Result<()> {
        self.send_line("shutdown")
    }

    fn send_line(&mut self, line: impl AsRef<str>) -> Result<()> {
        let line = line.as_ref();
        if input_trace_enabled() {
            eprintln!("[cef-c-standalone/input] {line}");
        }
        self.stdin
            .write_all(line.as_bytes())
            .with_context(|| format!("failed writing command to C standalone helper: {line}"))?;
        self.stdin
            .write_all(b"\n")
            .with_context(|| format!("failed terminating command for C standalone helper: {line}"))?;
        self.stdin.flush().context("failed flushing C standalone helper stdin")
    }
}

fn current_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn input_trace_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("NEKO_CEF_INPUT_TRACE")
            .map(|value| {
                let value = value.trim().to_ascii_lowercase();
                matches!(value.as_str(), "1" | "true" | "yes" | "on")
            })
            .unwrap_or(false)
    })
}

fn forward_env_if_present(command: &mut Command, name: &str) {
    if let Some(value) = std::env::var_os(name) {
        command.env(name, value);
    }
}

fn handle_stdout_line(line: &str, sender: &mpsc::Sender<CStandaloneHelperEvent>) {
    if !line.starts_with(EVENT_PREFIX) {
        eprintln!("[cef-c-standalone/stdout] {line}");
        return;
    }
    if let Some(event) = parse_event_line(&line[EVENT_PREFIX.len()..]) {
        let _ = sender.send(event);
    }
}

fn parse_event_line(payload: &str) -> Option<CStandaloneHelperEvent> {
    let mut event_name = None::<String>;
    let mut fields = Vec::new();
    for part in payload.split_whitespace() {
        let (key, value) = part.split_once('=')?;
        if key == "event" {
            event_name = Some(value.to_string());
        } else {
            fields.push((key.to_string(), value.to_string()));
        }
    }
    let event_name = event_name?;
    Some(match event_name.as_str() {
        "startup" => CStandaloneHelperEvent::Startup,
        "initialize_ok" => CStandaloneHelperEvent::InitializeOk,
        "initialize_failed" => CStandaloneHelperEvent::InitializeFailed {
            message: field_value(&fields, "message")
                .unwrap_or_default()
                .to_string(),
        },
        "create_browser_ok" => CStandaloneHelperEvent::CreateBrowserOk,
        "create_browser_failed" => CStandaloneHelperEvent::CreateBrowserFailed {
            message: field_value(&fields, "message")
                .unwrap_or_default()
                .to_string(),
        },
        "browser_created" => CStandaloneHelperEvent::BrowserCreated,
        "load_start" => CStandaloneHelperEvent::LoadStart {
            transition_type: field_value(&fields, "transition_type")
                .and_then(|value| value.parse().ok())
                .unwrap_or_default(),
        },
        "load_end" => CStandaloneHelperEvent::LoadEnd {
            http_status_code: field_value(&fields, "http_status_code")
                .and_then(|value| value.parse().ok())
                .unwrap_or_default(),
        },
        "paint" => CStandaloneHelperEvent::Paint {
            element_type: field_value(&fields, "element_type")
                .and_then(|value| value.parse().ok())
                .unwrap_or_default(),
            width: field_value(&fields, "width")
                .and_then(|value| value.parse().ok())
                .unwrap_or_default(),
            height: field_value(&fields, "height")
                .and_then(|value| value.parse().ok())
                .unwrap_or_default(),
            frame: field_value(&fields, "frame")
                .and_then(|value| value.parse().ok())
                .unwrap_or_default(),
        },
        "input_region" => CStandaloneHelperEvent::InputRegion {
            rects: field_value(&fields, "rects")
                .and_then(|value| from_str::<Vec<InteractiveRectPayload>>(value).ok())
                .unwrap_or_default(),
        },
        "drag_region" => CStandaloneHelperEvent::DragRegion {
            rects: field_value(&fields, "rects")
                .and_then(|value| from_str::<Vec<InteractiveRectPayload>>(value).ok())
                .unwrap_or_default(),
        },
        "drag_exclusion_region" => CStandaloneHelperEvent::DragExclusionRegion {
            rects: field_value(&fields, "rects")
                .and_then(|value| from_str::<Vec<InteractiveRectPayload>>(value).ok())
                .unwrap_or_default(),
        },
        "browser_before_close" => CStandaloneHelperEvent::BrowserBeforeClose,
        "browser_released" => CStandaloneHelperEvent::BrowserReleased,
        "shutdown_ok" => CStandaloneHelperEvent::ShutdownOk,
        "subprocess_exit" => CStandaloneHelperEvent::SubprocessExit {
            code: field_value(&fields, "code")
                .and_then(|value| value.parse().ok())
                .unwrap_or_default(),
        },
        _ => CStandaloneHelperEvent::RawLine {
            event: event_name,
            fields,
        },
    })
}

fn field_value<'a>(fields: &'a [(String, String)], key: &str) -> Option<&'a str> {
    fields
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
}

fn map_button(button: RawHostPointerButton) -> Option<i32> {
    match button {
        RawHostPointerButton::Left => Some(0),
        RawHostPointerButton::Middle => Some(1),
        RawHostPointerButton::Right => Some(2),
        RawHostPointerButton::Other(_) => None,
    }
}

fn button_flag(button: i32) -> u32 {
    match button {
        0 => 1 << 4,
        1 => 1 << 5,
        2 => 1 << 6,
        _ => 0,
    }
}

fn map_modifiers(modifiers: RawHostModifiers, keysym: u32, is_repeat: bool) -> u32 {
    let mut flags = 0_u32;
    if modifiers.shift {
        flags |= 1 << 1;
    }
    if modifiers.ctrl {
        flags |= 1 << 2;
    }
    if modifiers.alt {
        flags |= 1 << 3;
    }
    if modifiers.logo {
        flags |= 1 << 7;
    }
    if modifiers.caps_lock {
        flags |= 1 << 0;
    }
    if modifiers.num_lock {
        flags |= 1 << 8;
    }
    if is_repeat {
        flags |= 1 << 13;
    }
    if matches!(keysym, key::Alt_L | key::Alt_R) {
        flags |= 1 << 3;
    }
    flags
}

fn map_windows_key_code(keysym: u32, utf8: Option<&str>) -> i32 {
    match keysym {
        key::Return => 0x0D,
        key::BackSpace => 0x08,
        key::Tab => 0x09,
        key::Escape => 0x1B,
        key::Delete => 0x2E,
        key::Home => 0x24,
        key::End => 0x23,
        key::Page_Up => 0x21,
        key::Page_Down => 0x22,
        key::Left => 0x25,
        key::Up => 0x26,
        key::Right => 0x27,
        key::Down => 0x28,
        key::Shift_L | key::Shift_R => 0x10,
        key::Control_L | key::Control_R => 0x11,
        key::Alt_L | key::Alt_R => 0x12,
        key::Super_L | key::Super_R => 0x5B,
        _ => utf8
            .and_then(|text| text.chars().next())
            .map(|ch| ch.to_ascii_uppercase() as i32)
            .unwrap_or(keysym as i32),
    }
}

fn discover_c_standalone_helper(repo_root: &Path) -> Result<PathBuf> {
    let candidates = [
        repo_root
            .parent()
            .map(|parent| parent.join("N.E.K.O.-PC-wayland/target/debug/cef-c-standalone-helper"))
            .unwrap_or_else(|| PathBuf::from("target/debug/cef-c-standalone-helper")),
        PathBuf::from("target/debug/cef-c-standalone-helper"),
    ];

    for candidate in candidates {
        if candidate.exists() {
            return Ok(candidate);
        }
    }

    bail!(
        "unable to find built C standalone helper; run scripts/build_cef_c_standalone_helper.sh first"
    )
}

fn discover_staged_cef_runtime_dir(repo_root: &Path) -> Result<PathBuf> {
    let candidates = [
        repo_root
            .parent()
            .map(|parent| parent.join("N.E.K.O.-PC-wayland/target/debug/cef-official-helper"))
            .unwrap_or_else(|| PathBuf::from("target/debug/cef-official-helper")),
        PathBuf::from("target/debug/cef-official-helper"),
    ];

    for candidate in candidates {
        if candidate.join("cef-helper").exists() && candidate.join("libcef.so").exists() {
            return Ok(candidate);
        }
    }

    bail!(
        "unable to find staged CEF runtime; run scripts/stage_cef_official_helper.sh first"
    )
}
