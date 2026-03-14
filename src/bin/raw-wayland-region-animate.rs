use std::thread;
use std::time::Duration;

use neko_pc_wayland::wayland::input_region::{InputRegion, InteractiveRect};
use neko_pc_wayland::wayland::raw_host::{RawHostConfig, spawn};

fn main() {
    if let Err(err) = run() {
        eprintln!("raw-wayland-region-animate failed: {err:#}");
        std::process::exit(1);
    }
}

fn run() -> anyhow::Result<()> {
    let mut config = RawHostConfig::probe();
    config.title = "raw-wayland-region-animate".to_string();
    config.app_id = "moe.neko.raw-wayland-region-animate".to_string();
    config.install_ctrlc_handler = false;

    let host = spawn(config)?;
    let handle = host.handle.clone();

    let animator = thread::spawn(move || {
        let frames = [
            InputRegion::from_rects(vec![InteractiveRect {
                x: 0,
                y: 0,
                width: 220,
                height: 320,
            }]),
            InputRegion::from_rects(vec![InteractiveRect {
                x: 140,
                y: 60,
                width: 260,
                height: 260,
            }]),
            InputRegion::from_rects(vec![
                InteractiveRect {
                    x: 40,
                    y: 40,
                    width: 160,
                    height: 180,
                },
                InteractiveRect {
                    x: 260,
                    y: 180,
                    width: 180,
                    height: 180,
                },
            ]),
        ];

        let mut index = 0usize;
        while handle.is_running() {
            thread::sleep(Duration::from_secs(1));
            let region = frames[index % frames.len()].clone();
            if handle.set_input_region(region).is_err() {
                break;
            }
            let visible_region = Some(InputRegion::from_rects(vec![InteractiveRect {
                x: 0,
                y: 0,
                width: 520,
                height: 420,
            }]));
            if handle.set_visible_region(visible_region).is_err() {
                break;
            }
            index += 1;
        }
    });

    let join_result = host.join().map_err(|_| anyhow::anyhow!("raw host thread panicked"))?;
    let _ = animator.join();
    join_result
}
