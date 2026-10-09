//! Discover installed tools only. Never run a package install or a registry download.
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PackageManager {
    Npm,
    Pnpm,
    Yarn,
}
impl PackageManager {
    pub fn name(self) -> &'static str {
        match self {
            Self::Npm => "npm",
            Self::Pnpm => "pnpm",
            Self::Yarn => "yarn",
        }
    }
}

pub fn detect(root: &Path) -> PackageManager {
    for directory in root.ancestors() {
        if directory.join("pnpm-lock.yaml").is_file() {
            return PackageManager::Pnpm;
        }
        if directory.join("yarn.lock").is_file() {
            return PackageManager::Yarn;
        }
        if directory.join("package-lock.json").is_file()
            || directory.join("npm-shrinkwrap.json").is_file()
        {
            return PackageManager::Npm;
        }
        if let Ok(text) = std::fs::read_to_string(directory.join("package.json")) {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
                if let Some(manager) = value.get("packageManager").and_then(|v| v.as_str()) {
                    if manager.starts_with("pnpm@") {
                        return PackageManager::Pnpm;
                    }
                    if manager.starts_with("yarn@") {
                        return PackageManager::Yarn;
                    }
                    if manager.starts_with("npm@") {
                        return PackageManager::Npm;
                    }
                }
            }
        }
    }
    PackageManager::Npm
}

pub fn local_esbuild(root: &Path) -> Option<PathBuf> {
    root.ancestors()
        .map(|dir| dir.join("node_modules/.bin/esbuild"))
        .find(|path| path.is_file())
}

/// Keep the returned temporary directory alive until the native process exits.
pub fn configure(command: &mut Command, root: &Path) -> Result<Option<tempfile::TempDir>> {
    let manager = detect(root);
    if std::env::var_os("CELLD_ESBUILD").is_some() {
        return Ok(None);
    }
    if let Some(path) = local_esbuild(root) {
        command.env("CELLD_ESBUILD", path);
        return Ok(None);
    }
    // Yarn PnP has no node_modules/.bin. Native celld accepts a single executable
    // path, so bridge its arguments with a fixed, non-interpolated wrapper.
    if manager == PackageManager::Yarn && root.ancestors().any(|dir| dir.join(".pnp.cjs").is_file())
    {
        let temp = tempfile::tempdir().context("create Yarn esbuild bridge")?;
        let wrapper = temp.path().join("esbuild");
        std::fs::write(&wrapper, "#!/bin/sh\nexec yarn exec esbuild \"$@\"\n")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755))?;
        }
        command.env("CELLD_ESBUILD", wrapper);
        // Corepack must not fetch an absent package-manager version implicitly.
        command.env("COREPACK_ENABLE_NETWORK", "0");
        return Ok(Some(temp));
    }
    // Leave esbuild on PATH to native celld. In particular, don't preempt its
    // configuration validation or reject asset-only/no_bundle projects.
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn detects_managers_and_ancestor_local_tools() {
        for (file, expected) in [
            ("package-lock.json", PackageManager::Npm),
            ("pnpm-lock.yaml", PackageManager::Pnpm),
            ("yarn.lock", PackageManager::Yarn),
        ] {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join(file), "").unwrap();
            let project = dir.path().join("packages/app");
            std::fs::create_dir_all(&project).unwrap();
            let bin = dir.path().join("node_modules/.bin/esbuild");
            std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
            std::fs::write(&bin, "").unwrap();
            assert_eq!(detect(&project), expected);
            assert_eq!(local_esbuild(&project), Some(bin));
        }
    }
    #[test]
    fn package_manager_field_is_supported() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"packageManager":"pnpm@10.0.0"}"#,
        )
        .unwrap();
        assert_eq!(detect(dir.path()), PackageManager::Pnpm);
    }
}
