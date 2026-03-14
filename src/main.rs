mod app;
mod config;
mod ipc;
mod wayland;

use anyhow::Result;

fn main() {
    if let Err(err) = run() {
        eprintln!("N.E.K.O.-PC-wayland failed: {err:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let config = config::AppConfig::discover()?;
    app::run(config)
}
