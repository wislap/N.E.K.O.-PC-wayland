use anyhow::Result;
use neko_pc_wayland::{app, config};

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
