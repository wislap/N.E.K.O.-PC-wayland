use anyhow::Result;
use neko_pc_wayland::{app, config};

fn main() {
    if let Err(err) = run() {
        eprintln!("N.E.K.O.-PC-wayland failed: {err:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    #[cfg(feature = "cef_osr")]
    if let Some(code) = neko_pc_wayland::cef::try_run_subprocess()? {
        std::process::exit(code);
    }

    let config = config::AppConfig::discover()?;
    app::run(config)
}
