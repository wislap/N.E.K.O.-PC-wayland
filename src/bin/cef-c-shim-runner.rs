#[cfg(feature = "cef_osr")]
use neko_pc_wayland::cef::{CShimMode, run_c_probe};

fn main() {
    if let Err(err) = run() {
        eprintln!("cef-c-shim-runner failed: {err:#}");
        std::process::exit(1);
    }
}

fn run() -> anyhow::Result<()> {
    #[cfg(not(feature = "cef_osr"))]
    {
        anyhow::bail!("build with --features cef_osr");
    }

    #[cfg(feature = "cef_osr")]
    {
        let mode = match std::env::var("NEKO_CEF_C_SHIM_MODE").ok().as_deref() {
            Some("null_app") | Some("null-app") | Some("null") => CShimMode::NullApp,
            _ => CShimMode::WithApp,
        };
        let code = run_c_probe(mode)?;
        eprintln!("cef-c-shim-runner exited with code {code} mode={mode:?}");
        Ok(())
    }
}
