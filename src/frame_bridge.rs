use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::wayland::raw_host::RawHostFrame;

pub fn default_frame_dump_path() -> PathBuf {
    std::env::temp_dir().join(format!(
        "neko-cef-frame-{}-{}.bgra",
        std::process::id(),
        current_millis()
    ))
}

pub fn load_bgra_frame(path: &Path, width: u32, height: u32) -> Result<RawHostFrame> {
    let bgra = fs::read(path)
        .with_context(|| format!("failed to read dumped CEF frame {}", path.display()))?;
    let expected_len = width as usize * height as usize * 4;
    if bgra.len() != expected_len {
        bail!(
            "invalid dumped CEF frame length {}, expected {} for {}x{} from {}",
            bgra.len(),
            expected_len,
            width,
            height,
            path.display()
        );
    }

    let mut rgba = Vec::with_capacity(expected_len);
    for pixel in bgra.chunks_exact(4) {
        rgba.push(pixel[2]);
        rgba.push(pixel[1]);
        rgba.push(pixel[0]);
        rgba.push(pixel[3]);
    }

    RawHostFrame::new(width, height, rgba)
}

fn current_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}
