//! Applet-style scaffolding without package installs or host access.
use anyhow::{bail, Context, Result};
use serde_json::json;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};

fn exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).with_context(|| format!("inspect {}", path.display())),
    }
}

fn project_files(name: &str) -> Result<Vec<(&'static str, String)>> {
    let package = json!({
        "name": name,
        "private": true,
        "version": "0.0.0",
        "type": "module",
        "scripts": { "deploy": "cella deploy" },
        "dependencies": { "esbuild": "0.25.10" }
    });
    let wrangler = json!({
        "name": name,
        "main": "src/index.js",
        "compatibility_date": "2026-09-22",
        "durable_objects": {
            "bindings": [{ "name": "APPLET_STATE", "class_name": "AppletState" }]
        },
        "migrations": [{ "tag": "v1", "new_sqlite_classes": ["AppletState"] }]
    });
    Ok(vec![
        (".gitignore", "node_modules\n.wrangler\n.applet\n.applet-build\ndev.vars\n".into()),
        ("pnpm-workspace.yaml", "packages: []\n".into()),
        ("package.json", format!("{}\n", serde_json::to_string_pretty(&package)?)),
        ("wrangler.jsonc", format!("{}\n", serde_json::to_string_pretty(&wrangler)?)),
        ("src/index.js", include_str!("templates/index.js").into()),
        ("README.md", format!("# {name}\n\nA Worker created with Cella, with a SQLite-backed Durable Object counter.\n\n## Install dependencies\n\n```sh\npnpm install\n```\n\n## Deploy\n\nSet CELLA_HOST and CELLA_SSH_KEY to your owner SSH destination and private key, then run:\n\n```sh\ncella deploy\ncella status\ncella logs --lines 50\n```\n\nWorker configuration lives in `wrangler.jsonc`. Cella builds locally with esbuild and deploys over SSH; it does not install dependencies.\n")),
    ])
}

/// Create a named directory or safely add the scaffold to an existing directory.
pub fn create(base: &Path, directory: &Path) -> Result<PathBuf> {
    let base = base.canonicalize().context("resolve project directory")?;
    let mut target = PathBuf::new();
    for component in base.join(directory).components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                target.pop();
            }
            other => target.push(other.as_os_str()),
        }
    }
    let name = target
        .file_name()
        .and_then(|name| name.to_str())
        .context("project directory must have a UTF-8 name")?;
    crate::config::validate_slug(name)?;
    if !name.as_bytes()[0].is_ascii_lowercase() {
        bail!("project names must start with a lowercase letter");
    }
    let in_current_directory = directory.as_os_str() == ".";
    if !in_current_directory && exists(&target)? {
        bail!(
            "Cannot create {}: the directory already exists.",
            target.display()
        );
    }

    let files = project_files(name)?;
    if in_current_directory {
        let mut conflicts = Vec::new();
        let src_conflict = exists(&target.join("src"))? && !target.join("src").is_dir();
        if src_conflict {
            conflicts.push("src/");
        }
        for path in files
            .iter()
            .map(|(path, _)| *path)
            .chain(["wrangler.json", "wrangler.toml"])
        {
            if src_conflict && path == "src/index.js" {
                continue;
            }
            if exists(&target.join(path))? {
                conflicts.push(path);
            }
        }
        if !conflicts.is_empty() {
            bail!("Cannot initialize this directory because these paths already exist: {}. Move or rename them first; no files were changed.", conflicts.join(", "));
        }
    }
    fs::create_dir_all(&target).with_context(|| format!("create {}", target.display()))?;
    for (path, content) in files {
        let destination = target.join(path);
        fs::create_dir_all(destination.parent().unwrap())?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&destination)
            .with_context(|| format!("create {}", destination.display()))?;
        file.write_all(content.as_bytes())?;
    }
    Ok(target)
}
