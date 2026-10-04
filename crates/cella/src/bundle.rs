//! Capture native esbuild outputs without changing the project or reimplementing bundling.
use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::Value;
use std::ffi::OsStr;
use std::fs;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::Command;

pub use celld_ctl_core::{PreparedBundle, UploadFile, MAX_BUNDLE_BYTES};
use celld_ctl_core::{MAX_DECODED_BYTES as MAX_RAW_BYTES, MAX_UPLOAD_FILES as MAX_FILES};
pub const CAPTURE_ENV: &str = "CELLA_INTERNAL_ESBUILD_CAPTURE";
const REAL_ESBUILD_ENV: &str = "CELLA_INTERNAL_REAL_ESBUILD";

/// The only early compatibility gate: avoid invoking Docker or Python/package
/// installers for workflows that cannot be transported by this JS-only protocol.
pub fn check_scope(config: &Value) -> Result<()> {
    if config.get("containers").is_some() {
        bail!("cella prepared uploads support JavaScript/TypeScript only; container builds are not supported (Docker was not invoked)");
    }
    if config
        .get("main")
        .and_then(Value::as_str)
        .is_some_and(|s| s.ends_with(".py"))
    {
        bail!("cella prepared uploads support JavaScript/TypeScript only; Python builds are not supported");
    }
    Ok(())
}

pub fn clear_storage_environment(command: &mut Command) {
    for (key, _) in std::env::vars_os() {
        let name = key.to_string_lossy();
        if name.starts_with("AWS_") || name.starts_with("S3_") || name == "CELLD_BUCKET" {
            command.env_remove(key);
        }
    }
}

/// Toolchain::configure runs first. Resolve the resulting executable override,
/// then install our own executable as native celld's transparent esbuild bridge.
pub fn configure_capture(command: &mut Command, capture: &Path) -> Result<()> {
    let configured = command
        .get_envs()
        .find(|(k, _)| *k == OsStr::new("CELLD_ESBUILD"))
        .and_then(|(_, v)| v)
        .map(|v| v.to_owned());
    let real = configured
        .or_else(|| std::env::var_os("CELLD_ESBUILD"))
        .unwrap_or_else(|| "esbuild".into());
    command
        .env(REAL_ESBUILD_ENV, real)
        .env(CAPTURE_ENV, capture)
        .env("CELLD_ESBUILD", std::env::current_exe()?);
    Ok(())
}

/// Private subprocess mode; invoked only by the native builder with its own argv.
pub fn capture_esbuild() -> Result<i32> {
    let capture = PathBuf::from(std::env::var_os(CAPTURE_ENV).context("missing capture path")?);
    let real = std::env::var_os(REAL_ESBUILD_ENV).context("missing real esbuild path")?;
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let outdirs: Vec<_> = args
        .iter()
        .filter_map(|arg| arg.to_str()?.strip_prefix("--outdir="))
        .collect();
    if outdirs.len() != 1 {
        bail!("native esbuild capture requires exactly one --outdir");
    }
    let mut command = Command::new(real);
    command
        .args(&args)
        .env_remove(CAPTURE_ENV)
        .env_remove(REAL_ESBUILD_ENV);
    clear_storage_environment(&mut command);
    let status = command
        .status()
        .context("run installed esbuild for native celld")?;
    if !status.success() {
        return Ok(status.code().unwrap_or(1));
    }
    let outdir = Path::new(outdirs[0]);
    let copied: Vec<_> = args
        .iter()
        .filter_map(|arg| {
            arg.to_str()?
                .strip_prefix("--loader:")?
                .strip_suffix("=copy")
        })
        .collect();
    let mut budget = Budget::default();
    for entry in fs::read_dir(outdir)? {
        let entry = entry?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| anyhow::anyhow!("non-UTF-8 esbuild output"))?;
        if name != "index.js" && !copied.iter().any(|extension| name.ends_with(extension)) {
            continue;
        }
        if name != "index.js" && !name.ends_with(".wasm") {
            bail!("cella cannot upload nonstandard CompiledWasm extension: {name}; only .wasm modules are supported");
        }
        validate_path(&name)?;
        let bytes = budget.read(&entry.path())?;
        fs::write(capture.join(name), bytes)?;
    }
    if !capture.join("index.js").is_file() {
        bail!("native esbuild emitted no index.js");
    }
    Ok(0)
}

pub fn validate_path(path: &str) -> Result<()> {
    if !celld_ctl_core::valid_upload_path(path) {
        bail!("unsafe or oversized upload path: {path:?}");
    }
    Ok(())
}

#[derive(Default)]
struct Budget {
    files: usize,
    bytes: usize,
}
impl Budget {
    fn read(&mut self, path: &Path) -> Result<Vec<u8>> {
        self.files += 1;
        if self.files > MAX_FILES {
            bail!("prepared upload exceeds {MAX_FILES} files");
        }
        let metadata =
            fs::symlink_metadata(path).with_context(|| format!("inspect {}", path.display()))?;
        if !metadata.file_type().is_file() {
            bail!("upload input is not a regular file: {}", path.display());
        }
        let remaining = MAX_RAW_BYTES - self.bytes;
        if metadata.len() > remaining as u64 {
            bail!("prepared upload exceeds 24 MiB raw bytes");
        }
        let mut bytes = Vec::new();
        fs::File::open(path)?
            .take(remaining as u64 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > remaining || bytes.len() as u64 != metadata.len() {
            bail!("upload input changed or exceeds 24 MiB: {}", path.display());
        }
        self.bytes += bytes.len();
        Ok(bytes)
    }
    fn file(&mut self, path: &Path, name: String) -> Result<UploadFile> {
        validate_path(&name)?;
        Ok(UploadFile {
            path: name,
            content: STANDARD.encode(self.read(path)?),
        })
    }
}

fn project_path(root: &Path, relative: &str) -> Result<PathBuf> {
    // Native already checked lexical project paths; additionally reject symlink
    // components so packaging cannot follow them outside the intended project.
    if Path::new(relative).is_absolute() {
        bail!("absolute upload input path is unsupported");
    }
    let mut path = root.to_path_buf();
    for component in Path::new(relative).components() {
        match component {
            Component::Normal(c) => path.push(c),
            Component::CurDir => continue,
            _ => bail!("upload input must remain inside the project"),
        }
        if fs::symlink_metadata(&path)?.file_type().is_symlink() {
            bail!("upload input contains a symbolic link: {}", path.display());
        }
    }
    Ok(path)
}

fn assets(
    directory: &Path,
    relative: &str,
    files: &mut Vec<UploadFile>,
    budget: &mut Budget,
) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| anyhow::anyhow!("non-UTF-8 asset name"))?;
        let name = if relative.is_empty() {
            name
        } else {
            format!("{relative}/{name}")
        };
        validate_path(&name)?;
        let kind = entry.file_type()?;
        if kind.is_dir() {
            assets(&entry.path(), &name, files, budget)?;
        } else {
            files.push(budget.file(&entry.path(), name)?);
        }
    }
    Ok(())
}

fn unbundled_wasm(
    directory: &Path,
    relative: &str,
    main: &Path,
    files: &mut Vec<UploadFile>,
    budget: &mut Budget,
) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if path == main {
            continue;
        }
        let kind = entry.file_type()?;
        let is_wasm = path.extension() == Some(OsStr::new("wasm"))
            || path
                .file_name()
                .and_then(OsStr::to_str)
                .is_some_and(|s| s.ends_with(".wasm?module"));
        if !kind.is_dir() && !is_wasm {
            continue;
        }
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| anyhow::anyhow!("non-UTF-8 wasm name"))?;
        if kind.is_dir() && matches!(name.as_str(), ".git" | ".celld" | ".wrangler") {
            continue;
        }
        let name = if relative.is_empty() {
            name
        } else {
            format!("{relative}/{name}")
        };
        validate_path(&name)?;
        if kind.is_dir() {
            unbundled_wasm(&path, &name, main, files, budget)?;
        } else {
            files.push(budget.file(&path, name)?);
        }
    }
    Ok(())
}

pub fn prepare(config: Value, root: &Path, capture: &Path) -> Result<Vec<u8>> {
    let mut budget = Budget::default();
    let (mut modules, mut asset_files) = (Vec::new(), Vec::new());
    if let Some(main) = config.get("main").and_then(Value::as_str) {
        if config.get("no_bundle").and_then(Value::as_bool) == Some(true) {
            let main = project_path(root, main)?;
            modules.push(budget.file(&main, "index.js".into())?);
            unbundled_wasm(
                main.parent().context("main has no parent")?,
                "",
                &main,
                &mut modules,
                &mut budget,
            )?;
        } else {
            for entry in fs::read_dir(capture)? {
                let entry = entry?;
                let name = entry
                    .file_name()
                    .into_string()
                    .map_err(|_| anyhow::anyhow!("non-UTF-8 captured module"))?;
                if name != "index.js" && !name.ends_with(".wasm") {
                    bail!("unsupported captured module {name:?}");
                }
                modules.push(budget.file(&entry.path(), name)?);
            }
            if !modules.iter().any(|file| file.path == "index.js") {
                bail!("native build succeeded without a captured index.js; refusing incomplete upload");
            }
        }
    }
    if let Some(directory) = config
        .get("assets")
        .and_then(|v| v.get("directory"))
        .and_then(Value::as_str)
    {
        assets(
            &project_path(root, directory)?,
            "",
            &mut asset_files,
            &mut budget,
        )?;
    }
    modules.sort_by(|a, b| a.path.cmp(&b.path));
    asset_files.sort_by(|a, b| a.path.cmp(&b.path));
    let bundle = PreparedBundle {
        config,
        modules,
        assets: asset_files,
    }
    .normalize()
    .map_err(anyhow::Error::msg)?;
    let mut encoded = LimitedWriter(Vec::new());
    serde_json::to_writer(&mut encoded, &bundle)
        .context("encode prepared upload within 32 MiB limit")?;
    Ok(encoded.0)
}

struct LimitedWriter(Vec<u8>);
impl Write for LimitedWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.0.len() + bytes.len() > MAX_BUNDLE_BYTES {
            return Err(std::io::Error::other("prepared upload exceeds 32 MiB"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn safe_upload_paths_are_shared_with_host() {
        for path in [
            "",
            "/root",
            "../escape",
            "a/../b",
            "a\\b",
            "a//b",
            "a/./b",
            "a\nb",
            "drive:path",
        ] {
            assert!(validate_path(path).is_err(), "{path:?}");
        }
        assert!(validate_path(&"a".repeat(256)).is_err());
        assert!(validate_path(&format!(
            "{}/{}/{}",
            "a".repeat(200),
            "b".repeat(200),
            "c".repeat(200)
        ))
        .is_err());
        assert!(validate_path("nested/compiled.wasm?module").is_ok());
    }

    #[test]
    fn files_and_raw_bytes_are_bounded_before_allocation() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("input");
        let file = fs::File::create(&path).unwrap();
        file.set_len(MAX_RAW_BYTES as u64 + 1).unwrap();
        assert!(Budget::default()
            .read(&path)
            .unwrap_err()
            .to_string()
            .contains("24 MiB"));
        file.set_len(0).unwrap();
        let mut budget = Budget {
            files: MAX_FILES,
            bytes: 0,
        };
        assert!(budget
            .read(&path)
            .unwrap_err()
            .to_string()
            .contains("4096 files"));
    }

    #[test]
    fn serialization_is_bounded_while_writing() {
        let mut writer = LimitedWriter(Vec::new());
        writer.write_all(&vec![0; MAX_BUNDLE_BYTES]).unwrap();
        assert!(writer.write_all(b"x").is_err());
        assert_eq!(writer.0.len(), MAX_BUNDLE_BYTES);
    }

    #[cfg(unix)]
    #[test]
    fn asset_symlinks_and_symlinked_main_parents_cannot_be_packaged() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir(temp.path().join("assets")).unwrap();
        fs::write(temp.path().join("outside"), "not an asset").unwrap();
        symlink("../outside", temp.path().join("assets/linked")).unwrap();
        let config = json!({"name":"app","assets":{"directory":"assets"}});
        assert!(prepare(config, temp.path(), temp.path())
            .unwrap_err()
            .to_string()
            .contains("regular file"));
        fs::create_dir(temp.path().join("actual")).unwrap();
        fs::write(temp.path().join("actual/index.js"), "export default {}").unwrap();
        symlink("actual", temp.path().join("linked")).unwrap();
        let config = json!({"name":"app","main":"linked/index.js","no_bundle":true});
        assert!(prepare(config, temp.path(), temp.path())
            .unwrap_err()
            .to_string()
            .contains("symbolic link"));
    }

    #[test]
    fn wasm_discovery_skips_native_state_directories() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("entry.js"), "export default {}").unwrap();
        for excluded in [".git", ".celld", ".wrangler"] {
            fs::create_dir(temp.path().join(excluded)).unwrap();
            fs::write(
                temp.path().join(excluded).join("state.wasm"),
                "not a module",
            )
            .unwrap();
        }
        fs::create_dir(temp.path().join("nested")).unwrap();
        fs::write(temp.path().join("nested/actual.wasm"), b"\0asm").unwrap();
        let config = json!({"name":"app","main":"entry.js","no_bundle":true});
        let encoded = prepare(config, temp.path(), temp.path()).unwrap();
        let bundle: PreparedBundle = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(
            bundle
                .modules
                .iter()
                .map(|f| f.path.as_str())
                .collect::<Vec<_>>(),
            ["index.js", "nested/actual.wasm"]
        );
    }
}
