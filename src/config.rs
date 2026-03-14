use std::env;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub repo_root: PathBuf,
    pub app_title: String,
    pub frontend_url: String,
}

impl AppConfig {
    pub fn discover() -> Result<Self> {
        let repo_root = if let Ok(value) = env::var("NEKO_REPO_ROOT") {
            PathBuf::from(value)
        } else {
            discover_repo_root()?
        };

        if !repo_root.join("launcher.py").exists() {
            bail!(
                "repo root {:?} does not look like N.E.K.O (missing launcher.py)",
                repo_root
            );
        }

        Ok(Self {
            repo_root,
            app_title: "N.E.K.O.-PC-wayland".to_string(),
            frontend_url: env::var("NEKO_FRONTEND_URL")
                .unwrap_or_else(|_| "http://127.0.0.1:48911/".to_string()),
        })
    }
}

fn discover_repo_root() -> Result<PathBuf> {
    let current = env::current_dir().context("failed to read current dir")?;
    if let Some(root) = find_neko_repo(&current) {
        return Ok(root);
    }

    let exe = env::current_exe().context("failed to read current executable path")?;
    if let Some(parent) = exe.parent() {
        if let Some(root) = find_neko_repo(parent) {
            return Ok(root);
        }
    }

    bail!("unable to discover N.E.K.O repo root, set NEKO_REPO_ROOT explicitly")
}

fn find_neko_repo(base: &Path) -> Option<PathBuf> {
    for dir in base.ancestors() {
        if dir.join("launcher.py").exists() && dir.join("main_server.py").exists() {
            return Some(dir.to_path_buf());
        }

        let sibling = dir.join("N.E.K.O");
        if sibling.join("launcher.py").exists() && sibling.join("main_server.py").exists() {
            return Some(sibling);
        }
    }
    None
}
