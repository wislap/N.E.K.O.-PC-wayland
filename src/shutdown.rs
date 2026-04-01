use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{Context, Result};

#[derive(Clone)]
struct ShutdownFlagRegistration {
    flag: Arc<AtomicBool>,
    value: bool,
}

fn shutdown_flags() -> &'static Mutex<Vec<ShutdownFlagRegistration>> {
    static FLAGS: OnceLock<Mutex<Vec<ShutdownFlagRegistration>>> = OnceLock::new();
    FLAGS.get_or_init(|| Mutex::new(Vec::new()))
}

pub fn register_ctrlc_flag(flag: Arc<AtomicBool>) -> Result<()> {
    register_ctrlc_flag_value(flag, true)
}

pub fn register_ctrlc_running_flag(flag: Arc<AtomicBool>) -> Result<()> {
    register_ctrlc_flag_value(flag, false)
}

fn register_ctrlc_flag_value(flag: Arc<AtomicBool>, value: bool) -> Result<()> {
    static HANDLER_INSTALLED: OnceLock<()> = OnceLock::new();

    if HANDLER_INSTALLED.get().is_none() {
        ctrlc::set_handler(|| {
            let flags = shutdown_flags()
                .lock()
                .expect("shutdown flags mutex poisoned");
            for registration in flags.iter() {
                registration
                    .flag
                    .store(registration.value, Ordering::SeqCst);
            }
        })
        .context("failed to install Ctrl+C handler")?;
        let _ = HANDLER_INSTALLED.set(());
    }

    shutdown_flags()
        .lock()
        .expect("shutdown flags mutex poisoned")
        .push(ShutdownFlagRegistration { flag, value });
    Ok(())
}
