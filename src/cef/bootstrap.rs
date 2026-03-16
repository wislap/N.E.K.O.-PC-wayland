#![allow(dead_code)]

use std::env;
use std::fs;
use std::process::Command;
#[cfg(target_family = "unix")]
use std::os::unix::fs as unix_fs;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};

#[derive(Debug, Clone)]
pub struct CefSdkLayout {
    pub root: PathBuf,
    pub include_dir: PathBuf,
    pub release_dir: PathBuf,
    pub resources_dir: PathBuf,
    pub locales_dir: PathBuf,
    pub libcef_so: PathBuf,
}

impl CefSdkLayout {
    pub fn discover() -> Result<Self> {
        if let Some(layout) = Self::discover_colocated()? {
            return Ok(layout);
        }

        let root = env::var_os("NEKO_CEF_SDK_DIR")
            .map(PathBuf::from)
            .context("NEKO_CEF_SDK_DIR is not set")?;
        let sdk = Self::from_sdk_root(root)?;
        Self::materialize_runtime_layout(&sdk)
    }

    fn discover_colocated() -> Result<Option<Self>> {
        let exe = env::current_exe().context("failed to resolve current executable")?;
        let exe_root = exe
            .parent()
            .context("current executable has no parent directory")?
            .to_path_buf();

        let candidates = [exe_root.join("cef-official-helper"), exe_root.clone()];
        for root in candidates {
            let libcef_so = root.join("libcef.so");
            let resources_dir = root.clone();
            let locales_dir = root.join("locales");
            let icu_data = root.join("icudtl.dat");
            let snapshot = root.join("v8_context_snapshot.bin");
            let locales_ok = locales_look_usable(&locales_dir);
            eprintln!(
                "CEF bootstrap: candidate={} libcef={} icudtl={} snapshot={} locales_ok={}",
                root.display(),
                libcef_so.exists(),
                icu_data.exists(),
                snapshot.exists(),
                locales_ok
            );

            if libcef_so.exists()
                && icu_data.exists()
                && snapshot.exists()
                && locales_ok
            {
                eprintln!(
                    "CEF bootstrap: discovered colocated runtime at {}",
                    root.display()
                );
                return Ok(Some(Self {
                    root: root.clone(),
                    include_dir: root.clone(),
                    release_dir: root.clone(),
                    resources_dir,
                    locales_dir,
                    libcef_so,
                }));
            }
        }

        Ok(None)
    }

    fn from_sdk_root(root: PathBuf) -> Result<Self> {
        let include_dir = root.join("include");
        let release_dir = root.join("Release");
        let resources_dir = root.join("Resources");
        let locales_dir = resources_dir.join("locales");
        let libcef_so = release_dir.join("libcef.so");

        if !include_dir.exists() {
            bail!(
                "CEF SDK include directory is missing: {}",
                include_dir.display()
            );
        }

        if !libcef_so.exists() {
            bail!("CEF runtime library is missing: {}", libcef_so.display());
        }

        if !resources_dir.exists() {
            bail!(
                "CEF resources directory is missing: {}",
                resources_dir.display()
            );
        }

        if !locales_dir.exists() {
            bail!(
                "CEF locales directory is missing: {}",
                locales_dir.display()
            );
        }

        Ok(Self {
            root,
            include_dir,
            release_dir,
            resources_dir,
            locales_dir,
            libcef_so,
        })
    }

    fn materialize_runtime_layout(sdk: &Self) -> Result<Self> {
        let exe = env::current_exe().context("failed to resolve current executable")?;
        let runtime_root = exe
            .parent()
            .context("current executable has no parent directory")?
            .to_path_buf();

        fs::create_dir_all(&runtime_root).with_context(|| {
            format!(
                "failed to create CEF runtime directory {}",
                runtime_root.display()
            )
        })?;

        let runtime_files = [
            (
                sdk.release_dir.join("libcef.so"),
                runtime_root.join("libcef.so"),
            ),
            (
                sdk.release_dir.join("libEGL.so"),
                runtime_root.join("libEGL.so"),
            ),
            (
                sdk.release_dir.join("libGLESv2.so"),
                runtime_root.join("libGLESv2.so"),
            ),
            (
                sdk.release_dir.join("libvk_swiftshader.so"),
                runtime_root.join("libvk_swiftshader.so"),
            ),
            (
                sdk.release_dir.join("libvulkan.so.1"),
                runtime_root.join("libvulkan.so.1"),
            ),
            (
                sdk.release_dir.join("v8_context_snapshot.bin"),
                runtime_root.join("v8_context_snapshot.bin"),
            ),
            (
                sdk.release_dir.join("chrome-sandbox"),
                runtime_root.join("chrome-sandbox"),
            ),
            (
                sdk.release_dir.join("vk_swiftshader_icd.json"),
                runtime_root.join("vk_swiftshader_icd.json"),
            ),
            (
                sdk.resources_dir.join("icudtl.dat"),
                runtime_root.join("icudtl.dat"),
            ),
            (
                sdk.resources_dir.join("resources.pak"),
                runtime_root.join("resources.pak"),
            ),
            (
                sdk.resources_dir.join("chrome_100_percent.pak"),
                runtime_root.join("chrome_100_percent.pak"),
            ),
            (
                sdk.resources_dir.join("chrome_200_percent.pak"),
                runtime_root.join("chrome_200_percent.pak"),
            ),
        ];

        for (source, destination) in runtime_files {
            if source.exists() {
                ensure_symlink_or_copy(&source, &destination)?;
            }
        }

        let locales_dir = runtime_root.join("locales");
        materialize_locales_dir(sdk, &locales_dir)?;

        Ok(Self {
            root: runtime_root.clone(),
            include_dir: sdk.include_dir.clone(),
            release_dir: runtime_root.clone(),
            resources_dir: runtime_root.clone(),
            locales_dir,
            libcef_so: runtime_root.join("libcef.so"),
        })
    }
}

fn materialize_locales_dir(sdk: &CefSdkLayout, destination: &PathBuf) -> Result<()> {
    if destination.exists() || destination.symlink_metadata().is_ok() {
        let metadata = fs::symlink_metadata(destination).with_context(|| {
            format!(
                "failed to inspect existing locales directory {}",
                destination.display()
            )
        })?;
        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            fs::remove_dir_all(destination).with_context(|| {
                format!(
                    "failed to remove stale locales directory {}",
                    destination.display()
                )
            })?;
        } else {
            fs::remove_file(destination).with_context(|| {
                format!(
                    "failed to remove stale locales artifact {}",
                    destination.display()
                )
            })?;
        }
    }

    eprintln!(
        "CEF bootstrap: materializing locales into {}",
        destination.display()
    );

    if try_reuse_staged_helper_locales(destination)? && locales_look_usable(destination) {
        eprintln!("CEF bootstrap: re-used staged helper locales");
        return Ok(());
    }

    if try_extract_locales_from_sdk_archive(sdk, destination)? && locales_look_usable(destination) {
        eprintln!("CEF bootstrap: extracted locales from SDK archive");
        return Ok(());
    }

    ensure_symlink_or_copy(&sdk.locales_dir, destination)?;
    if locales_look_usable(destination) {
        eprintln!("CEF bootstrap: linked locales directory from SDK");
        return Ok(());
    }

    bail!(
        "CEF locales are unreadable after materialization: {}",
        destination.display()
    )
}

fn try_reuse_staged_helper_locales(destination: &PathBuf) -> Result<bool> {
    let exe = env::current_exe().context("failed to resolve current executable")?;
    let runtime_root = exe
        .parent()
        .context("current executable has no parent directory")?;
    let helper_locales = runtime_root.join("cef-official-helper").join("locales");
    eprintln!(
        "CEF bootstrap: checking staged helper locales at {}",
        helper_locales.display()
    );
    if !locales_look_usable(&helper_locales) {
        eprintln!("CEF bootstrap: staged helper locales not usable");
        return Ok(false);
    }

    ensure_symlink_or_copy(&helper_locales, destination)?;
    Ok(true)
}

fn try_extract_locales_from_sdk_archive(sdk: &CefSdkLayout, destination: &PathBuf) -> Result<bool> {
    let Some(archive_path) = sdk.root.with_extension("tar.bz2").canonicalize().ok().or_else(|| {
        let candidate = sdk.root.parent()?.join(format!(
            "{}.tar.bz2",
            sdk.root.file_name()?.to_string_lossy()
        ));
        candidate.exists().then_some(candidate)
    }) else {
        eprintln!("CEF bootstrap: no SDK archive found for locale extraction");
        return Ok(false);
    };

    eprintln!(
        "CEF bootstrap: extracting locales from archive {}",
        archive_path.display()
    );

    fs::create_dir_all(destination).with_context(|| {
        format!(
            "failed to create extracted locales directory {}",
            destination.display()
        )
    })?;

    let archive_root = archive_root_name(&archive_path)?;
    let status = Command::new("tar")
        .arg("-xjf")
        .arg(&archive_path)
        .arg("-C")
        .arg(destination)
        .arg("--strip-components=3")
        .arg(format!("{archive_root}/Resources/locales/en-US.pak"))
        .arg(format!("{archive_root}/Resources/locales/zh-CN.pak"))
        .status()
        .with_context(|| format!("failed to spawn tar for {}", archive_path.display()))?;

    if !status.success() {
        eprintln!("CEF bootstrap: locale extraction command failed with status {status}");
        return Ok(false);
    }

    Ok(true)
}

fn archive_root_name(archive_path: &PathBuf) -> Result<String> {
    let output = Command::new("tar")
        .arg("-tjf")
        .arg(archive_path)
        .output()
        .with_context(|| format!("failed to inspect archive {}", archive_path.display()))?;
    if !output.status.success() {
        bail!(
            "failed to inspect CEF archive {}",
            archive_path.display()
        );
    }

    let listing = String::from_utf8(output.stdout)
        .with_context(|| format!("archive listing is not utf-8: {}", archive_path.display()))?;
    let first = listing
        .lines()
        .find(|line| !line.trim().is_empty())
        .context("CEF archive listing is empty")?;
    let root = first
        .split('/')
        .next()
        .filter(|part| !part.is_empty())
        .context("failed to parse CEF archive root")?;
    Ok(root.to_string())
}

fn locales_look_usable(locales_dir: &PathBuf) -> bool {
    ["en-US.pak", "zh-CN.pak"]
        .iter()
        .any(|name| {
            let path = locales_dir.join(name);
            fs::metadata(path).map(|meta| meta.len() > 0).unwrap_or(false)
        })
}

fn ensure_symlink_or_copy(source: &PathBuf, destination: &PathBuf) -> Result<()> {
    if destination.exists() || destination.symlink_metadata().is_ok() {
        let metadata = fs::symlink_metadata(destination).with_context(|| {
            format!(
                "failed to inspect existing runtime artifact {}",
                destination.display()
            )
        })?;

        if metadata.file_type().is_symlink() {
            let current = fs::read_link(destination)
                .with_context(|| format!("failed to read symlink {}", destination.display()))?;
            if current == *source {
                return Ok(());
            }
        }

        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            fs::remove_dir_all(destination).with_context(|| {
                format!(
                    "failed to remove stale runtime directory {}",
                    destination.display()
                )
            })?;
        } else {
            fs::remove_file(destination).with_context(|| {
                format!(
                    "failed to remove stale runtime file {}",
                    destination.display()
                )
            })?;
        }
    }

    #[cfg(target_family = "unix")]
    {
        unix_fs::symlink(source, destination).with_context(|| {
            format!(
                "failed to symlink CEF runtime artifact {} -> {}",
                destination.display(),
                source.display()
            )
        })?;
        return Ok(());
    }

    #[allow(unreachable_code)]
    {
        if source.is_dir() {
            bail!(
                "directory runtime copy is not implemented for {}",
                source.display()
            );
        }
        fs::copy(source, destination).with_context(|| {
            format!(
                "failed to copy CEF runtime artifact {} -> {}",
                source.display(),
                destination.display()
            )
        })?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct CefRuntimePlan {
    pub sdk: CefSdkLayout,
    pub subprocess_executable: PathBuf,
    pub locales_dir: PathBuf,
    pub resources_dir: PathBuf,
}

impl CefRuntimePlan {
    pub fn discover() -> Result<Self> {
        let sdk = CefSdkLayout::discover()?;
        let resources_dir = sdk.resources_dir.clone();
        let locales_dir = sdk.locales_dir.clone();
        let subprocess_executable = {
            let official_helper = sdk.root.join("cef-helper");
            if official_helper.exists() {
                official_helper
            } else {
                env::current_exe()
                    .context("failed to resolve current executable for CEF subprocess planning")?
            }
        };

        Ok(Self {
            sdk,
            subprocess_executable,
            locales_dir,
            resources_dir,
        })
    }
}
