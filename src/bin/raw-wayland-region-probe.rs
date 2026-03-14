use neko_pc_wayland::wayland::raw_host::{RawHostConfig, run};

fn main() {
    if let Err(err) = run(RawHostConfig::probe()) {
        eprintln!("raw-wayland-region-probe failed: {err:#}");
        std::process::exit(1);
    }
}
