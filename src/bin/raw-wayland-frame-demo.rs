use std::thread;
use std::time::Duration;

use neko_pc_wayland::wayland::input_region::{InputRegion, InteractiveRect};
use neko_pc_wayland::wayland::raw_host::{RawHostConfig, RawHostFrame, spawn};

const WIDTH: u32 = 800;
const HEIGHT: u32 = 600;

fn main() {
    if let Err(err) = run() {
        eprintln!("raw-wayland-frame-demo failed: {err:#}");
        std::process::exit(1);
    }
}

fn run() -> anyhow::Result<()> {
    let mut config = RawHostConfig::probe();
    config.title = "raw-wayland-frame-demo".to_string();
    config.app_id = "moe.neko.raw-wayland-frame-demo".to_string();
    config.width = WIDTH;
    config.height = HEIGHT;
    config.install_ctrlc_handler = false;
    config.transparent_outside_input_region = true;
    config.visible_region = Some(InputRegion::from_rects(vec![InteractiveRect {
        x: 80,
        y: 60,
        width: 560,
        height: 420,
    }]));
    config.input_region = InputRegion::from_rects(vec![InteractiveRect {
        x: 120,
        y: 100,
        width: 240,
        height: 260,
    }]);

    let host = spawn(config)?;
    let handle = host.handle.clone();

    let animator = thread::spawn(move || {
        let mut tick = 0u32;
        while handle.is_running() {
            let rgba = build_demo_frame(WIDTH, HEIGHT, tick);
            let frame = match RawHostFrame::new(WIDTH, HEIGHT, rgba) {
                Ok(frame) => frame,
                Err(_) => break,
            };
            if handle.set_rgba_frame(frame).is_err() {
                break;
            }
            tick = tick.wrapping_add(3);
            thread::sleep(Duration::from_millis(33));
        }
    });

    let join_result = host.join().map_err(|_| anyhow::anyhow!("raw host thread panicked"))?;
    let _ = animator.join();
    join_result
}

fn build_demo_frame(width: u32, height: u32, tick: u32) -> Vec<u8> {
    let mut rgba = vec![0_u8; width as usize * height as usize * 4];

    for y in 0..height {
        for x in 0..width {
            let i = ((y * width + x) * 4) as usize;
            let wave = (((x + tick) % width) * 255 / width) as u8;
            let band = (((y + tick * 2) % height) * 255 / height) as u8;
            let r = wave;
            let g = 255_u8.saturating_sub(band);
            let b = (((x ^ y) + tick) % 255) as u8;
            let in_circle = in_circle(
                x as i32,
                y as i32,
                360 + ((tick as i32 * 3) % 120) - 60,
                240,
                96,
            );
            let a = if in_circle || (x > 80 && x < 640 && y > 60 && y < 480) {
                255
            } else {
                0
            };

            rgba[i] = r;
            rgba[i + 1] = g;
            rgba[i + 2] = b;
            rgba[i + 3] = a;
        }
    }

    rgba
}

fn in_circle(x: i32, y: i32, cx: i32, cy: i32, radius: i32) -> bool {
    let dx = x - cx;
    let dy = y - cy;
    dx * dx + dy * dy <= radius * radius
}
