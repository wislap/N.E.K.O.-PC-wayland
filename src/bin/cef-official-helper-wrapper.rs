use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError, TryRecvError};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

const EVENT_PREFIX: &str = "NEKO_CEF_HELPER_EVENT ";

#[derive(Debug, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
enum HelperEvent<'a> {
    Startup {
        runtime_dir: &'a str,
        executable: &'a str,
        args: &'a [String],
    },
    Spawned {
        pid: u32,
    },
    State {
        phase: &'a str,
        detail: &'a str,
    },
    Ready {
        pid: u32,
        uptime_ms: u64,
        initial_url: Option<&'a str>,
    },
    Unsupported {
        command: &'a str,
        reason: &'a str,
    },
    Output {
        stream: &'a str,
        line: &'a str,
    },
    Exit {
        code: Option<i32>,
        signal: Option<i32>,
        success: bool,
    },
    Error {
        message: &'a str,
    },
    Pong {
        nonce: Option<&'a str>,
    },
    ShutdownRequested,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
enum HelperCommand {
    Ping { nonce: Option<String> },
    Navigate { url: String },
    Shutdown,
}

fn main() {
    if let Err(err) = run() {
        emit(&HelperEvent::Error {
            message: &format!("{err:#}"),
        });
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let runtime_dir = std::env::var("NEKO_CEF_HELPER_RUNTIME_DIR")
        .map(PathBuf::from)
        .context("NEKO_CEF_HELPER_RUNTIME_DIR is not set")?;
    let executable = runtime_dir.join("cef-helper");
    if !executable.exists() {
        bail!("official helper executable is missing: {}", executable.display());
    }

    let args = std::env::var("NEKO_CEF_OFFICIAL_HELPER_ARGS")
        .ok()
        .map(|raw| {
            raw.split_whitespace()
                .filter(|part| !part.is_empty())
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let initial_url = args
        .iter()
        .find(|arg| arg.starts_with("http://") || arg.starts_with("https://"))
        .cloned();

    let runtime_dir_display = runtime_dir.display().to_string();
    let executable_display = executable.display().to_string();
    emit(&HelperEvent::Startup {
        runtime_dir: &runtime_dir_display,
        executable: &executable_display,
        args: &args,
    });

    let mut child = Command::new(&executable)
        .args(&args)
        .current_dir(&runtime_dir)
        .env("LD_LIBRARY_PATH", &runtime_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to spawn {}", executable.display()))?;

    emit(&HelperEvent::Spawned { pid: child.id() });
    emit(&HelperEvent::State {
        phase: "spawned",
        detail: "official helper child process created",
    });

    let stdout = child
        .stdout
        .take()
        .context("official helper stdout is not piped")?;
    let stderr = child
        .stderr
        .take()
        .context("official helper stderr is not piped")?;

    let (output_sender, output_receiver) = mpsc::channel::<(String, String)>();
    let stdout_sender = output_sender.clone();
    thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            match line {
                Ok(line) => {
                    let _ = stdout_sender.send(("stdout".to_string(), line));
                }
                Err(err) => {
                    let _ = stdout_sender.send(("stdout".to_string(), format!("read error: {err}")));
                    break;
                }
            }
        }
    });

    let stderr_sender = output_sender.clone();
    thread::spawn(move || {
        let reader = BufReader::new(stderr);
        for line in reader.lines() {
            match line {
                Ok(line) => {
                    let _ = stderr_sender.send(("stderr".to_string(), line));
                }
                Err(err) => {
                    let _ = stderr_sender.send(("stderr".to_string(), format!("read error: {err}")));
                    break;
                }
            }
        }
    });
    drop(output_sender);

    let (command_sender, command_receiver) = mpsc::channel::<HelperCommand>();
    thread::spawn(move || {
        let stdin = std::io::stdin();
        let reader = BufReader::new(stdin.lock());
        for line in reader.lines() {
            let Ok(line) = line else {
                break;
            };
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            match serde_json::from_str::<HelperCommand>(trimmed) {
                Ok(command) => {
                    let _ = command_sender.send(command);
                }
                Err(err) => {
                    let message = format!("invalid command payload: {err}");
                    emit(&HelperEvent::Error { message: &message });
                }
            }
        }
    });

    let mut shutdown_requested = false;
    let child_pid = child.id();
    let started_at = std::time::Instant::now();
    let mut ready_emitted = false;

    loop {
        match output_receiver.recv_timeout(Duration::from_millis(50)) {
            Ok((stream, line)) => emit(&HelperEvent::Output {
                stream: &stream,
                line: &line,
            }),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {}
        }

        loop {
            match command_receiver.try_recv() {
                Ok(HelperCommand::Ping { nonce }) => emit(&HelperEvent::Pong {
                    nonce: nonce.as_deref(),
                }),
                Ok(HelperCommand::Navigate { url }) => {
                    let detail = format!(
                        "official helper sample cannot navigate after launch; requested url={url}"
                    );
                    emit(&HelperEvent::State {
                        phase: "navigate_unsupported",
                        detail: &detail,
                    });
                    emit(&HelperEvent::Unsupported {
                        command: "navigate",
                        reason: "official helper sample only accepts initial URL at process launch",
                    });
                }
                Ok(HelperCommand::Shutdown) => {
                    emit(&HelperEvent::State {
                        phase: "shutdown",
                        detail: "shutdown command forwarded to official helper child",
                    });
                    emit(&HelperEvent::ShutdownRequested);
                    shutdown_requested = true;
                    terminate_child(&mut child);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }

        if !ready_emitted {
            let uptime_ms = started_at.elapsed().as_millis() as u64;
            if uptime_ms >= 1500 {
                emit(&HelperEvent::Ready {
                    pid: child_pid,
                    uptime_ms,
                    initial_url: initial_url.as_deref(),
                });
                emit(&HelperEvent::State {
                    phase: "ready",
                    detail: "official helper child remained alive past readiness threshold",
                });
                ready_emitted = true;
            }
        }

        if let Some(status) = child
            .try_wait()
            .map_err(|err| anyhow!("failed polling official helper: {err}"))?
        {
            #[cfg(unix)]
            {
                use std::os::unix::process::ExitStatusExt;
                let mut code = status.code();
                let mut signal = status.signal();
                let mut success = status.success();
                if shutdown_requested && signal == Some(9) {
                    code = Some(0);
                    signal = None;
                    success = true;
                }
                emit(&HelperEvent::Exit {
                    code,
                    signal,
                    success,
                });
                if let Some(code) = code {
                    std::process::exit(code);
                }
                if let Some(signal) = signal {
                    std::process::exit(128 + signal);
                }
            }

            #[cfg(not(unix))]
            {
                emit(&HelperEvent::Exit {
                    code: status.code(),
                    signal: None,
                    success: status.success(),
                });
                std::process::exit(status.code().unwrap_or(1));
            }
        }
    }
}

fn emit(event: &HelperEvent<'_>) {
    match serde_json::to_string(event) {
        Ok(json) => {
            println!("{EVENT_PREFIX}{json}");
            let _ = std::io::stdout().flush();
        }
        Err(err) => eprintln!("failed to encode helper event: {err}"),
    }
}

fn terminate_child(child: &mut Child) {
    let _ = child.kill();
}
