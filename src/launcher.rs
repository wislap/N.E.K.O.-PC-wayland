use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};
use std::{env, fs, path::PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug)]
pub struct LauncherHandle {
    child: Child,
}

impl Drop for LauncherHandle {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;

            let pid = self.child.id() as i32;
            unsafe {
                libc::killpg(pid, libc::SIGTERM);
            }

            match self.child.wait() {
                Ok(status) if status.success() || status.signal().is_some() => return,
                Ok(_) | Err(_) => unsafe {
                    libc::killpg(pid, libc::SIGKILL);
                },
            }
        }

        #[cfg(not(unix))]
        let _ = self.child.kill();

        let _ = self.child.wait();
    }
}

#[derive(Debug)]
pub struct LauncherRuntime {
    pub receiver: Receiver<LauncherEventEnvelope>,
    pub handle: LauncherHandle,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LauncherEventEnvelope {
    pub event: String,
    pub launch_id: Option<String>,
    pub payload: Value,
}

#[derive(Debug, Clone, Default)]
pub struct PortSnapshot {
    pub launch_id: Option<String>,
    pub main_server_port: Option<u16>,
    pub memory_server_port: Option<u16>,
    pub tool_server_port: Option<u16>,
}

impl PortSnapshot {
    pub fn frontend_url(&self) -> Option<String> {
        self.main_server_port
            .map(|port| format!("http://127.0.0.1:{port}/"))
    }

    pub fn absorb(&mut self, event: &LauncherEventEnvelope) {
        if let Some(id) = &event.launch_id {
            self.launch_id = Some(id.clone());
        }

        match event.event.as_str() {
            "port_plan" | "startup_ready" | "attach_existing" => {
                if let Some(selected) = event.payload.get("selected") {
                    self.main_server_port = selected
                        .get("MAIN_SERVER_PORT")
                        .and_then(Value::as_u64)
                        .and_then(|value| u16::try_from(value).ok())
                        .or(self.main_server_port);
                    self.memory_server_port = selected
                        .get("MEMORY_SERVER_PORT")
                        .and_then(Value::as_u64)
                        .and_then(|value| u16::try_from(value).ok())
                        .or(self.memory_server_port);
                    self.tool_server_port = selected
                        .get("TOOL_SERVER_PORT")
                        .and_then(Value::as_u64)
                        .and_then(|value| u16::try_from(value).ok())
                        .or(self.tool_server_port);
                }
            }
            _ => {}
        }
    }
}

#[derive(Debug, Deserialize)]
struct RuntimeSnapshotFile {
    event: String,
    launch_id: Option<String>,
    payload: Value,
}

pub fn start_launcher(repo_root: &Path) -> Result<LauncherRuntime> {
    let mut child = spawn_launcher(repo_root)?;
    let stdout = child
        .stdout
        .take()
        .context("launcher stdout is not piped")?;
    let stderr = child
        .stderr
        .take()
        .context("launcher stderr is not piped")?;

    let (sender, receiver) = mpsc::channel();

    thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            match line {
                Ok(line) => {
                    eprintln!("{line}");
                    if let Some(event) = parse_neko_event(&line) {
                        let _ = sender.send(event);
                    }
                }
                Err(err) => {
                    eprintln!("failed reading launcher stdout: {err}");
                    break;
                }
            }
        }
    });

    thread::spawn(move || {
        let reader = BufReader::new(stderr);
        for line in reader.lines() {
            match line {
                Ok(line) => eprintln!("{line}"),
                Err(err) => {
                    eprintln!("failed reading launcher stderr: {err}");
                    break;
                }
            }
        }
    });

    Ok(LauncherRuntime {
        receiver,
        handle: LauncherHandle { child },
    })
}

fn spawn_launcher(repo_root: &Path) -> Result<Child> {
    let mut commands = Vec::new();
    commands.push(build_uv_launcher(repo_root));
    commands.push(build_python_launcher(repo_root, "python3"));
    commands.push(build_python_launcher(repo_root, "python"));

    let mut last_error = None;

    for mut command in commands {
        let debug = format!("{command:?}");
        match command.spawn() {
            Ok(child) => {
                eprintln!("spawned launcher via {debug}");
                return Ok(child);
            }
            Err(err) => {
                last_error = Some(anyhow!("failed to spawn {debug}: {err}"));
            }
        }
    }

    Err(last_error.unwrap_or_else(|| anyhow!("no launcher command available")))
}

fn build_uv_launcher(repo_root: &Path) -> Command {
    let mut command = Command::new("uv");
    command
        .arg("run")
        .arg("python")
        .arg("launcher.py")
        .current_dir(repo_root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    apply_process_group(&mut command);
    command
}

fn build_python_launcher(repo_root: &Path, python: &str) -> Command {
    let mut command = Command::new(python);
    command
        .arg("launcher.py")
        .current_dir(repo_root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    apply_process_group(&mut command);
    command
}

fn apply_process_group(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
}

pub fn wait_for_frontend_url(runtime: &LauncherRuntime) -> Result<PortSnapshot> {
    let mut ports = PortSnapshot::default();
    let mut saw_startup_in_progress = false;
    let snapshot_path = runtime_snapshot_path();
    let deadline = Instant::now() + Duration::from_secs(90);

    loop {
        match runtime.receiver.recv_timeout(Duration::from_millis(500)) {
            Ok(event) => {
                ports.absorb(&event);

                if matches!(event.event.as_str(), "startup_failure") {
                    bail!("launcher reported startup failure: {}", event.payload);
                }

                if matches!(event.event.as_str(), "startup_in_progress") {
                    saw_startup_in_progress = true;
                }

                if let Some(url) = ports.frontend_url() {
                    eprintln!("resolved frontend url from launcher: {url}");
                    return Ok(ports);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                if !saw_startup_in_progress {
                    break;
                }
            }
        }

        if let Some(snapshot) = try_read_runtime_snapshot(&snapshot_path)? {
            ports.absorb(&snapshot);
            if let Some(url) = ports.frontend_url() {
                eprintln!("resolved frontend url from shared runtime snapshot: {url}");
                return Ok(ports);
            }
        }

        if Instant::now() >= deadline {
            break;
        }
    }

    bail!("launcher event channel closed before frontend url became available")
}

fn parse_neko_event(line: &str) -> Option<LauncherEventEnvelope> {
    const PREFIX: &str = "NEKO_EVENT ";
    let payload = line.strip_prefix(PREFIX)?;
    serde_json::from_str(payload).ok()
}

fn runtime_snapshot_path() -> PathBuf {
    env::var("NEKO_RUNTIME_PORTS_SNAPSHOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| env::temp_dir().join("neko_runtime_ports.json"))
}

fn try_read_runtime_snapshot(path: &Path) -> Result<Option<LauncherEventEnvelope>> {
    if !path.exists() {
        return Ok(None);
    }

    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(_) => return Ok(None),
    };

    let snapshot: RuntimeSnapshotFile = match serde_json::from_str(&raw) {
        Ok(snapshot) => snapshot,
        Err(_) => return Ok(None),
    };

    Ok(Some(LauncherEventEnvelope {
        event: snapshot.event,
        launch_id: snapshot.launch_id,
        payload: snapshot.payload,
    }))
}
