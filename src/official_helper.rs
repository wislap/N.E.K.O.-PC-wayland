use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

#[cfg(unix)]
use std::os::unix::process::CommandExt;

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

const EVENT_PREFIX: &str = "NEKO_CEF_HELPER_EVENT ";

#[derive(Debug, Clone)]
pub struct OfficialHelperConfig {
    pub runtime_dir: PathBuf,
    pub executable: PathBuf,
    pub wrapper_executable: Option<PathBuf>,
    pub args: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
enum HelperEvent {
    Startup {
        runtime_dir: String,
        executable: String,
        args: Vec<String>,
    },
    Spawned {
        pid: u32,
    },
    State {
        phase: String,
        detail: String,
    },
    Ready {
        pid: u32,
        uptime_ms: u64,
        initial_url: Option<String>,
    },
    Unsupported {
        command: String,
        reason: String,
    },
    Output {
        stream: String,
        line: String,
    },
    Exit {
        code: Option<i32>,
        signal: Option<i32>,
        success: bool,
    },
    Error {
        message: String,
    },
    Pong {
        nonce: Option<String>,
    },
    ShutdownRequested,
}

#[derive(Debug, Serialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
enum HelperCommand {
    Ping { nonce: Option<String> },
    Navigate { url: String },
    Shutdown,
}

impl OfficialHelperConfig {
    pub fn discover(repo_root: &Path) -> Result<Self> {
        let candidates = [
            repo_root
                .parent()
                .map(|parent| parent.join("N.E.K.O.-PC-wayland/target/debug/cef-official-helper"))
                .unwrap_or_else(|| PathBuf::from("target/debug/cef-official-helper")),
            PathBuf::from("target/debug/cef-official-helper"),
        ];

        for candidate in candidates {
            let executable = candidate.join("cef-helper");
            if executable.exists() {
                let args = build_default_args_for_runtime(&candidate);
                let wrapper_executable = std::env::current_exe()
                    .ok()
                    .and_then(|path| path.parent().map(|parent| parent.join("cef-official-helper-wrapper")))
                    .filter(|path| path.exists());
                return Ok(Self {
                    runtime_dir: candidate,
                    executable,
                    wrapper_executable,
                    args,
                });
            }
        }

        bail!(
            "unable to find staged official CEF helper; run scripts/stage_cef_official_helper.sh first"
        )
    }

    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        let url = url.into();
        if !url.trim().is_empty() {
            self.args.push(url);
        }
        self
    }
}

fn build_default_args_for_runtime(runtime_dir: &Path) -> Vec<String> {
    let mut args = std::env::var("NEKO_CEF_OFFICIAL_HELPER_ARGS")
        .ok()
        .map(|raw| {
            raw.split_whitespace()
                .filter(|part| !part.is_empty())
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if !has_cli_switch(&args, "--no-sandbox") {
        args.push("--no-sandbox".to_string());
    }

    if !has_cli_switch_prefix(&args, "--user-data-dir=") {
        let user_data_dir = default_profile_dir(runtime_dir);
        args.push(format!("--user-data-dir={}", user_data_dir.display()));
    }

    if !has_cli_switch_prefix(&args, "--cache-path=") {
        let cache_dir = default_profile_dir(runtime_dir).join("cache");
        args.push(format!("--cache-path={}", cache_dir.display()));
    }

    if !has_cli_switch_prefix(&args, "--lang=") {
        if let Some(lang) = default_cef_lang() {
            args.push(format!("--lang={lang}"));
        }
    }

    args
}

fn default_profile_dir(runtime_dir: &Path) -> PathBuf {
    if let Ok(value) = std::env::var("NEKO_CEF_OFFICIAL_HELPER_PROFILE_DIR") {
        return PathBuf::from(value);
    }

    runtime_dir.join(format!("user-data-{}", std::process::id()))
}

fn default_cef_lang() -> Option<String> {
    let raw = std::env::var("LC_ALL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| std::env::var("LANG").ok())?;
    let normalized = raw
        .split('.')
        .next()
        .unwrap_or(raw.as_str())
        .split('@')
        .next()
        .unwrap_or(raw.as_str())
        .replace('_', "-");
    let trimmed = normalized.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("c") || trimmed.eq_ignore_ascii_case("posix") {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn has_cli_switch(args: &[String], exact: &str) -> bool {
    args.iter().any(|arg| arg == exact)
}

fn has_cli_switch_prefix(args: &[String], prefix: &str) -> bool {
    args.iter().any(|arg| arg.starts_with(prefix))
}

#[derive(Debug)]
pub struct OfficialHelperHandle {
    child: Child,
    stdin: Option<ChildStdin>,
    events: Receiver<HelperEvent>,
}

impl OfficialHelperHandle {
    pub fn spawn(config: &OfficialHelperConfig) -> Result<Self> {
        let mut command = if let Some(wrapper) = &config.wrapper_executable {
            let mut command = Command::new(wrapper);
            command
                .current_dir(&config.runtime_dir)
                .env("NEKO_CEF_HELPER_RUNTIME_DIR", &config.runtime_dir)
                .env("NEKO_CEF_OFFICIAL_HELPER_ARGS", config.args.join(" "));
            command
        } else {
            let mut command = Command::new(&config.executable);
            command
                .args(&config.args)
                .current_dir(&config.runtime_dir)
                .env("LD_LIBRARY_PATH", &config.runtime_dir);
            command
        };
        #[cfg(unix)]
        command.process_group(0);
        command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());

        let mut child = command.spawn().with_context(|| {
            format!(
                "failed to spawn official helper {}",
                config.executable.display()
            )
        })?;

        let stdin = child
            .stdin
            .take()
            .context("official helper stdin is not piped")?;
        let stdout = child
            .stdout
            .take()
            .context("official helper stdout is not piped")?;
        let stderr = child
            .stderr
            .take()
            .context("official helper stderr is not piped")?;

        let (event_sender, events) = mpsc::channel::<HelperEvent>();
        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                match line {
                    Ok(line) => handle_helper_stdout_line(&line, &event_sender),
                    Err(err) => {
                        eprintln!("[cef-helper] failed reading stdout: {err}");
                        break;
                    }
                }
            }
        });

        thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                match line {
                    Ok(line) => eprintln!("[cef-helper] {line}"),
                    Err(err) => {
                        eprintln!("[cef-helper] failed reading stderr: {err}");
                        break;
                    }
                }
            }
        });

        Ok(Self {
            child,
            stdin: Some(stdin),
            events,
        })
    }

    pub fn wait(&mut self) -> Result<std::process::ExitStatus> {
        self.child
            .wait()
            .map_err(|err| anyhow!("failed waiting for official helper: {err}"))
    }

    pub fn try_wait(&mut self) -> Result<Option<std::process::ExitStatus>> {
        self.child
            .try_wait()
            .map_err(|err| anyhow!("failed polling official helper: {err}"))
    }

    pub fn send_ping(&mut self, nonce: impl Into<String>) -> Result<()> {
        self.send_command(&HelperCommand::Ping {
            nonce: Some(nonce.into()),
        })
    }

    pub fn send_shutdown(&mut self) -> Result<()> {
        self.send_command(&HelperCommand::Shutdown)
    }

    pub fn send_navigate(&mut self, url: impl Into<String>) -> Result<()> {
        self.send_command(&HelperCommand::Navigate { url: url.into() })
    }

    pub fn recv_event_timeout(&self, timeout: Duration) -> Result<Option<HelperEventEnvelope>> {
        match self.events.recv_timeout(timeout) {
            Ok(event) => Ok(Some(HelperEventEnvelope { event })),
            Err(mpsc::RecvTimeoutError::Timeout) => Ok(None),
            Err(mpsc::RecvTimeoutError::Disconnected) => Ok(None),
        }
    }

    pub fn wait_for_pong(&self, expected_nonce: &str, timeout: Duration) -> Result<bool> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let Some(remaining) = deadline.checked_duration_since(std::time::Instant::now()) else {
                return Ok(false);
            };
            let Some(event) = self.recv_event_timeout(remaining)? else {
                return Ok(false);
            };
            if event.pong_nonce().as_deref() == Some(expected_nonce) {
                return Ok(true);
            }
        }
    }

    pub fn wait_for_startup(&self, timeout: Duration) -> Result<bool> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let Some(remaining) = deadline.checked_duration_since(std::time::Instant::now()) else {
                return Ok(false);
            };
            let Some(event) = self.recv_event_timeout(remaining)? else {
                return Ok(false);
            };
            if event.is_startup() {
                return Ok(true);
            }
            if let Some(message) = event.error_message() {
                bail!("official helper reported startup error: {message}");
            }
            if let Some(summary) = event.exit_summary() {
                bail!("official helper exited before startup handshake completed: {summary}");
            }
        }
    }

    pub fn wait_for_spawned(&self, timeout: Duration) -> Result<Option<u32>> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let Some(remaining) = deadline.checked_duration_since(std::time::Instant::now()) else {
                return Ok(None);
            };
            let Some(event) = self.recv_event_timeout(remaining)? else {
                return Ok(None);
            };
            if let Some(pid) = event.spawned_pid() {
                return Ok(Some(pid));
            }
            if let Some(message) = event.error_message() {
                bail!("official helper reported spawn error: {message}");
            }
            if let Some(summary) = event.exit_summary() {
                bail!("official helper exited before child spawn was observed: {summary}");
            }
        }
    }

    pub fn wait_for_ready(&self, timeout: Duration) -> Result<Option<(u32, u64)>> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let Some(remaining) = deadline.checked_duration_since(std::time::Instant::now()) else {
                return Ok(None);
            };
            let Some(event) = self.recv_event_timeout(remaining)? else {
                return Ok(None);
            };
            if let Some((pid, uptime_ms)) = event.ready_info() {
                return Ok(Some((pid, uptime_ms)));
            }
            if let Some((command, reason)) = event.unsupported_info() {
                bail!("official helper reported unsupported command during readiness wait: {command} ({reason})");
            }
            if let Some(message) = event.error_message() {
                bail!("official helper reported readiness error: {message}");
            }
            if let Some(summary) = event.exit_summary() {
                bail!("official helper exited before readiness was observed: {summary}");
            }
        }
    }

    pub fn wait_for_unsupported(
        &self,
        expected_command: &str,
        timeout: Duration,
    ) -> Result<Option<String>> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let Some(remaining) = deadline.checked_duration_since(std::time::Instant::now()) else {
                return Ok(None);
            };
            let Some(event) = self.recv_event_timeout(remaining)? else {
                return Ok(None);
            };
            if let Some((command, reason)) = event.unsupported_info() {
                if command == expected_command {
                    return Ok(Some(reason.to_string()));
                }
            }
            if let Some(message) = event.error_message() {
                bail!("official helper reported error while waiting for unsupported command: {message}");
            }
            if let Some(summary) = event.exit_summary() {
                bail!("official helper exited while waiting for unsupported command: {summary}");
            }
        }
    }

    pub fn terminate(&mut self) {
        let _ = self.send_shutdown();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    fn send_command(&mut self, command: &HelperCommand) -> Result<()> {
        let stdin = self
            .stdin
            .as_mut()
            .context("official helper stdin is unavailable")?;
        let payload =
            serde_json::to_string(command).map_err(|err| anyhow!("failed to encode helper command: {err}"))?;
        writeln!(stdin, "{payload}")
            .map_err(|err| anyhow!("failed writing helper command: {err}"))?;
        stdin
            .flush()
            .map_err(|err| anyhow!("failed flushing helper command: {err}"))?;
        Ok(())
    }
}

impl Drop for OfficialHelperHandle {
    fn drop(&mut self) {
        self.terminate();
    }
}

#[derive(Debug)]
pub struct HelperEventEnvelope {
    event: HelperEvent,
}

impl HelperEventEnvelope {
    pub fn is_startup(&self) -> bool {
        matches!(self.event, HelperEvent::Startup { .. })
    }

    pub fn pong_nonce(&self) -> Option<String> {
        match &self.event {
            HelperEvent::Pong { nonce } => nonce.clone(),
            _ => None,
        }
    }

    pub fn spawned_pid(&self) -> Option<u32> {
        match &self.event {
            HelperEvent::Spawned { pid } => Some(*pid),
            _ => None,
        }
    }

    pub fn ready_info(&self) -> Option<(u32, u64)> {
        match &self.event {
            HelperEvent::Ready {
                pid, uptime_ms, ..
            } => Some((*pid, *uptime_ms)),
            _ => None,
        }
    }

    pub fn ready_initial_url(&self) -> Option<&str> {
        match &self.event {
            HelperEvent::Ready {
                initial_url: Some(initial_url),
                ..
            } => Some(initial_url.as_str()),
            _ => None,
        }
    }

    pub fn is_exit(&self) -> bool {
        matches!(self.event, HelperEvent::Exit { .. })
    }

    pub fn is_shutdown_requested(&self) -> bool {
        matches!(self.event, HelperEvent::ShutdownRequested)
    }

    pub fn error_message(&self) -> Option<&str> {
        match &self.event {
            HelperEvent::Error { message } => Some(message.as_str()),
            _ => None,
        }
    }

    pub fn exit_summary(&self) -> Option<String> {
        match &self.event {
            HelperEvent::Exit {
                code,
                signal,
                success,
            } => Some(format!("success={success} code={code:?} signal={signal:?}")),
            _ => None,
        }
    }

    pub fn state_summary(&self) -> Option<String> {
        match &self.event {
            HelperEvent::State { phase, detail } => Some(format!("{phase}: {detail}")),
            _ => None,
        }
    }

    pub fn unsupported_info(&self) -> Option<(&str, &str)> {
        match &self.event {
            HelperEvent::Unsupported { command, reason } => Some((command.as_str(), reason.as_str())),
            _ => None,
        }
    }
}

fn handle_helper_stdout_line(line: &str, event_sender: &mpsc::Sender<HelperEvent>) {
    if let Some(payload) = line.strip_prefix(EVENT_PREFIX) {
        match serde_json::from_str::<HelperEvent>(payload) {
            Ok(HelperEvent::Startup {
                runtime_dir,
                executable,
                args,
            }) => {
                let _ = event_sender.send(HelperEvent::Startup {
                    runtime_dir: runtime_dir.clone(),
                    executable: executable.clone(),
                    args: args.clone(),
                });
                eprintln!(
                    "[cef-helper] startup runtime_dir={} executable={} args={args:?}",
                    runtime_dir, executable
                )
            }
            Ok(HelperEvent::Spawned { pid }) => {
                let _ = event_sender.send(HelperEvent::Spawned { pid });
                eprintln!("[cef-helper] spawned child pid={pid}");
            }
            Ok(HelperEvent::State { phase, detail }) => {
                let _ = event_sender.send(HelperEvent::State {
                    phase: phase.clone(),
                    detail: detail.clone(),
                });
                eprintln!("[cef-helper] state {phase}: {detail}");
            }
            Ok(HelperEvent::Ready {
                pid,
                uptime_ms,
                initial_url,
            }) => {
                let _ = event_sender.send(HelperEvent::Ready {
                    pid,
                    uptime_ms,
                    initial_url: initial_url.clone(),
                });
                eprintln!(
                    "[cef-helper] ready pid={pid} uptime_ms={uptime_ms} initial_url={initial_url:?}"
                );
            }
            Ok(HelperEvent::Unsupported { command, reason }) => {
                let _ = event_sender.send(HelperEvent::Unsupported {
                    command: command.clone(),
                    reason: reason.clone(),
                });
                eprintln!("[cef-helper] unsupported command={command} reason={reason}");
            }
            Ok(HelperEvent::Output { stream, line }) => {
                let _ = event_sender.send(HelperEvent::Output {
                    stream: stream.clone(),
                    line: line.clone(),
                });
                eprintln!("[cef-helper:{stream}] {line}");
            }
            Ok(HelperEvent::Exit {
                code,
                signal,
                success,
            }) => {
                let _ = event_sender.send(HelperEvent::Exit {
                    code,
                    signal,
                    success,
                });
                eprintln!(
                    "[cef-helper] exit success={} code={code:?} signal={signal:?}",
                    success
                )
            }
            Ok(HelperEvent::Error { message }) => {
                let _ = event_sender.send(HelperEvent::Error {
                    message: message.clone(),
                });
                eprintln!("[cef-helper] wrapper error: {message}");
            }
            Ok(HelperEvent::Pong { nonce }) => {
                let _ = event_sender.send(HelperEvent::Pong {
                    nonce: nonce.clone(),
                });
                eprintln!("[cef-helper] pong nonce={nonce:?}");
            }
            Ok(HelperEvent::ShutdownRequested) => {
                let _ = event_sender.send(HelperEvent::ShutdownRequested);
                eprintln!("[cef-helper] shutdown requested");
            }
            Err(err) => eprintln!("[cef-helper] invalid wrapper event: {err}; raw={line}"),
        }
        return;
    }

    eprintln!("[cef-helper] {line}");
}
