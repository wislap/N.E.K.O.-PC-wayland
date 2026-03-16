use std::ffi::CString;
use std::fs;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::path::{Path, PathBuf};
use std::ptr;
use std::slice;
use std::sync::atomic::{Ordering, compiler_fence};

use anyhow::{Context, Result, anyhow, bail};

use crate::wayland::raw_host::RawHostFrame;

const SHARED_FRAME_MAGIC: u32 = 0x4E_4B_46_42;
const SHARED_FRAME_VERSION: u32 = 1;
const SHARED_FRAME_HEADER_WORDS: usize = 8;
const SHARED_FRAME_HEADER_SIZE: usize = SHARED_FRAME_HEADER_WORDS * std::mem::size_of::<u32>();

pub fn default_frame_dump_path() -> PathBuf {
    std::env::temp_dir().join(format!(
        "neko-cef-frame-{}-{}.bgra",
        std::process::id(),
        current_millis()
    ))
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SharedFrameHeader {
    pub magic: u32,
    pub version: u32,
    pub seq: u32,
    pub frame: u32,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub data_len: u32,
}

pub struct SharedFrameWriterConfig {
    pub fd: OwnedFd,
    pub size: usize,
}

pub struct SharedFrameReaderConfig {
    pub fd: OwnedFd,
    pub size: usize,
}

pub struct SharedFrameReader {
    map_ptr: *mut u8,
    map_len: usize,
}

unsafe impl Send for SharedFrameReader {}

pub struct FrameReader {
    path: PathBuf,
}

impl FrameReader {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        if !path.exists() {
            bail!("dumped CEF frame does not exist yet: {}", path.display());
        }
        Ok(Self { path })
    }

    pub fn load_bgra_frame(&mut self, width: u32, height: u32) -> Result<RawHostFrame> {
        let expected_len = width as usize * height as usize * 4;
        let bgra = fs::read(&self.path)
            .with_context(|| format!("failed to read dumped CEF frame {}", self.path.display()))?;
        if bgra.len() != expected_len {
            bail!(
                "invalid dumped CEF frame length {}, expected {} for {}x{} from {}",
                bgra.len(),
                expected_len,
                width,
                height,
                self.path.display()
            );
        }

        RawHostFrame::from_bgra(width, height, bgra)
    }
}

pub fn load_bgra_frame(path: &Path, width: u32, height: u32) -> Result<RawHostFrame> {
    let mut reader = FrameReader::open(path)?;
    reader.load_bgra_frame(width, height)
}

pub fn create_shared_frame_writer(
    width: u32,
    height: u32,
) -> Result<SharedFrameWriterConfig> {
    let data_len = frame_byte_len(width, height)?;
    let size = SHARED_FRAME_HEADER_SIZE
        .checked_add(data_len)
        .ok_or_else(|| anyhow!("shared frame bridge size overflow"))?;
    let name = CString::new(format!("neko-cef-shared-frame-{}", std::process::id()))
        .context("failed to build memfd name")?;
    let fd = unsafe { libc::memfd_create(name.as_ptr(), 0) };
    if fd < 0 {
        return Err(anyhow!(
            "memfd_create failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    let truncate_result = unsafe { libc::ftruncate(fd.as_raw_fd(), size as libc::off_t) };
    if truncate_result != 0 {
        return Err(anyhow!(
            "ftruncate failed for shared frame bridge: {}",
            std::io::Error::last_os_error()
        ));
    }

    Ok(SharedFrameWriterConfig { fd, size })
}

pub fn duplicate_fd(fd: RawFd) -> Result<OwnedFd> {
    let duplicated = unsafe { libc::dup(fd) };
    if duplicated < 0 {
        return Err(anyhow!(
            "dup failed for fd {fd}: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(unsafe { OwnedFd::from_raw_fd(duplicated) })
}

impl SharedFrameReader {
    pub fn open(config: SharedFrameReaderConfig) -> Result<Self> {
        let map_ptr = unsafe {
            libc::mmap(
                ptr::null_mut(),
                config.size,
                libc::PROT_READ,
                libc::MAP_SHARED,
                config.fd.as_raw_fd(),
                0,
            )
        };
        if map_ptr == libc::MAP_FAILED {
            return Err(anyhow!(
                "mmap failed for shared frame reader: {}",
                std::io::Error::last_os_error()
            ));
        }

        Ok(Self {
            map_ptr: map_ptr.cast::<u8>(),
            map_len: config.size,
        })
    }

    pub fn load_latest_frame(&self, expected_width: u32, expected_height: u32) -> Result<RawHostFrame> {
        for _ in 0..3 {
            let before = self.read_seq();
            if before == 0 || before % 2 == 1 {
                continue;
            }
            compiler_fence(Ordering::Acquire);
            let header = self.read_header();
            compiler_fence(Ordering::Acquire);
            let after = self.read_seq();
            if before != after || after % 2 == 1 {
                continue;
            }
            self.validate_header(&header, expected_width, expected_height)?;
            let data = unsafe {
                slice::from_raw_parts(
                    self.map_ptr.add(SHARED_FRAME_HEADER_SIZE),
                    header.data_len as usize,
                )
            };
            return RawHostFrame::from_bgra(header.width, header.height, data.to_vec());
        }

        bail!("shared frame bridge is being updated, retry on next paint")
    }

    fn validate_header(
        &self,
        header: &SharedFrameHeader,
        expected_width: u32,
        expected_height: u32,
    ) -> Result<()> {
        if header.magic != SHARED_FRAME_MAGIC {
            bail!("invalid shared frame magic: {}", header.magic);
        }
        if header.version != SHARED_FRAME_VERSION {
            bail!("unsupported shared frame version: {}", header.version);
        }
        if header.width != expected_width || header.height != expected_height {
            bail!(
                "shared frame size mismatch: got {}x{}, expected {}x{}",
                header.width,
                header.height,
                expected_width,
                expected_height
            );
        }
        let expected_len = frame_byte_len(header.width, header.height)?;
        if header.data_len as usize != expected_len {
            bail!(
                "shared frame payload length mismatch: got {}, expected {}",
                header.data_len,
                expected_len
            );
        }
        if SHARED_FRAME_HEADER_SIZE + expected_len > self.map_len {
            bail!(
                "shared frame mapping too small: map_len={}, need={}",
                self.map_len,
                SHARED_FRAME_HEADER_SIZE + expected_len
            );
        }
        Ok(())
    }

    fn read_seq(&self) -> u32 {
        unsafe { ptr::read_volatile(self.map_ptr.add(8).cast::<u32>()) }
    }

    fn read_header(&self) -> SharedFrameHeader {
        unsafe {
            SharedFrameHeader {
                magic: ptr::read_volatile(self.map_ptr.add(0).cast::<u32>()),
                version: ptr::read_volatile(self.map_ptr.add(4).cast::<u32>()),
                seq: ptr::read_volatile(self.map_ptr.add(8).cast::<u32>()),
                frame: ptr::read_volatile(self.map_ptr.add(12).cast::<u32>()),
                width: ptr::read_volatile(self.map_ptr.add(16).cast::<u32>()),
                height: ptr::read_volatile(self.map_ptr.add(20).cast::<u32>()),
                stride: ptr::read_volatile(self.map_ptr.add(24).cast::<u32>()),
                data_len: ptr::read_volatile(self.map_ptr.add(28).cast::<u32>()),
            }
        }
    }
}

impl Drop for SharedFrameReader {
    fn drop(&mut self) {
        unsafe {
            libc::munmap(self.map_ptr.cast::<libc::c_void>(), self.map_len);
        }
    }
}

pub fn shared_frame_magic() -> u32 {
    SHARED_FRAME_MAGIC
}

pub fn shared_frame_version() -> u32 {
    SHARED_FRAME_VERSION
}

pub fn shared_frame_header_size() -> usize {
    SHARED_FRAME_HEADER_SIZE
}

fn frame_byte_len(width: u32, height: u32) -> Result<usize> {
    width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(4))
        .map(|bytes| bytes as usize)
        .ok_or_else(|| anyhow!("frame size overflow for {}x{}", width, height))
}

fn current_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}
