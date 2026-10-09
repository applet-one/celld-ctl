//! Exact release URLs follow celld's install.sh and .github/workflows/release.yml.
use anyhow::{bail, Context, Result};
use flate2::read::GzDecoder;
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub fn target(os: &str, arch: &str) -> Result<&'static str> {
    match (os, arch) {
        ("linux", "x86_64") => Ok("x86_64-unknown-linux-gnu"),
        ("linux", "aarch64") => Ok("aarch64-unknown-linux-gnu"),
        ("macos", "aarch64") => Ok("aarch64-apple-darwin"),
        _ => bail!("no supported celld release for {os}/{arch}; supported: Linux x86_64/arm64, macOS arm64"),
    }
}

pub fn version_tag(version: &str) -> Result<String> {
    let raw = version.strip_prefix('v').unwrap_or(version);
    if raw.contains('+') {
        bail!("release version must not contain build metadata");
    }
    semver::Version::parse(raw)
        .context("pin an exact celld release, e.g. v0.0.1 (never latest or a version range)")?;
    Ok(format!("v{raw}"))
}

pub fn release_url(tag: &str, platform: &str) -> Result<String> {
    let tag = version_tag(tag)?;
    if ![
        "x86_64-unknown-linux-gnu",
        "aarch64-unknown-linux-gnu",
        "aarch64-apple-darwin",
    ]
    .contains(&platform)
    {
        bail!("unsupported celld release platform");
    }
    Ok(format!(
        "https://github.com/denoland/celld/releases/download/{tag}/celld-{platform}.gz"
    ))
}

pub fn verify_output(output: &[u8], tag: &str) -> Result<()> {
    verify_output_verbose(output, tag, false)
}

fn verify_output_verbose(output: &[u8], tag: &str, verbose: bool) -> Result<()> {
    let expected = version_tag(tag)?;
    let text = std::str::from_utf8(output)
        .context("celld --version was not UTF-8")?
        .trim();
    let version_line = format!("celld {}", &expected[1..]);
    if text == version_line {
        return Ok(());
    }
    // Some macOS releases log an allocator warning to stdout before printing
    // the real version. Permit diagnostic WARN lines, but never a different
    // version or arbitrary extra output masquerading as an exact release.
    let lines: Vec<_> = text.lines().collect();
    if lines.iter().filter(|line| **line == version_line).count() == 1
        && lines
            .iter()
            .filter(|line| **line != version_line)
            .all(|line| line.contains(" WARN "))
    {
        for line in lines.into_iter().filter(|line| *line != version_line) {
            if verbose || !crate::output::allocator_warning(line) {
                eprintln!("cella: warning: celld --version: {line}");
            }
        }
        return Ok(());
    }
    bail!("celld version mismatch: required {expected}, binary reported {text:?}; refusing to substitute another release")
}

fn verify(binary: &Path, tag: &str, verbose: bool) -> Result<()> {
    let output = Command::new(binary)
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("verify cached celld {}", binary.display()))?;
    if !output.status.success() {
        let mut diagnostics = crate::output::Diagnostics::default();
        diagnostics.show(&String::from_utf8_lossy(&output.stderr), true);
        diagnostics.show(&String::from_utf8_lossy(&output.stdout), true);
        bail!("cached celld --version failed: {}", output.status);
    }
    crate::output::Diagnostics::default().show(&String::from_utf8_lossy(&output.stderr), verbose);
    verify_output_verbose(&output.stdout, tag, verbose)
}

pub fn cache_root() -> Result<PathBuf> {
    if let Some(root) = std::env::var_os("CELLA_CACHE_DIR") {
        return Ok(root.into());
    }
    if let Some(root) = std::env::var_os("XDG_CACHE_HOME") {
        return Ok(PathBuf::from(root).join("cella"));
    }
    let home =
        std::env::var_os("HOME").context("set HOME or CELLA_CACHE_DIR for the release cache")?;
    Ok(PathBuf::from(home).join(".cache/cella"))
}

pub fn ensure(version: &str, cache: &Path) -> Result<PathBuf> {
    ensure_verbose(version, cache, false)
}

pub fn ensure_verbose(version: &str, cache: &Path, verbose: bool) -> Result<PathBuf> {
    let tag = version_tag(version)?;
    let platform = target(std::env::consts::OS, std::env::consts::ARCH)?;
    let parent = cache.join("releases").join(&tag).join(platform);
    fs::create_dir_all(&parent).context("create celld release cache")?;
    let parent = parent.canonicalize().context("resolve celld cache path")?;
    let binary = parent.join("celld");
    if binary.exists() {
        verify(&binary, &tag, verbose)?;
        return Ok(binary);
    }
    let staging = tempfile::tempdir_in(&parent)?;
    let archive = staging.path().join("celld.gz");
    let url = release_url(&tag, platform)?;
    eprintln!("Downloading celld {tag} ({platform})");
    let status = Command::new("curl")
        .args([
            "--disable",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--tlsv1.2",
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--connect-timeout",
            "15",
            "--max-time",
            "300",
        ])
        .arg(&url)
        .arg("--output")
        .arg(&archive)
        .stdin(Stdio::null())
        .status()
        .context("start curl (required for release downloads)")?;
    if !status.success() {
        bail!(
            "download of exact release {tag} failed ({status}); no newer version will be selected"
        );
    }
    let staged = staging.path().join("celld");
    let mut decoder = GzDecoder::new(File::open(archive)?);
    let mut executable = File::create(&staged)?;
    io::copy(&mut decoder, &mut executable).context("decompress celld release (gzip CRC)")?;
    executable.sync_all()?;
    drop(executable);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&staged, fs::Permissions::from_mode(0o755))?;
    }
    verify(&staged, &tag, verbose)?;
    fs::rename(staged, &binary).context("atomically install exact celld release")?;
    Ok(binary)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn platforms_and_actual_asset_urls() {
        for (os, arch, triple) in [
            ("linux", "x86_64", "x86_64-unknown-linux-gnu"),
            ("linux", "aarch64", "aarch64-unknown-linux-gnu"),
            ("macos", "aarch64", "aarch64-apple-darwin"),
        ] {
            assert_eq!(target(os, arch).unwrap(), triple);
            assert_eq!(
                release_url("0.0.7", triple).unwrap(),
                format!(
                    "https://github.com/denoland/celld/releases/download/v0.0.7/celld-{triple}.gz"
                )
            );
        }
        assert!(target("macos", "x86_64").is_err());
        assert!(target("windows", "x86_64").is_err());
    }
    #[test]
    fn exact_versions_only() {
        for version in [
            "latest",
            "1.2",
            "^1.2.3",
            "v1.2.3/evil",
            "v1.2.3+build",
            "v01.2.3",
        ] {
            assert!(version_tag(version).is_err());
        }
        assert_eq!(version_tag("1.2.3-rc.1").unwrap(), "v1.2.3-rc.1");
        assert!(verify_output(b"celld 1.2.3\n", "v1.2.3").is_ok());
        assert!(verify_output(
            b"2026-10-05T17:00:48Z  WARN celld::memory: allocator warning\ncelld 1.2.3\n",
            "v1.2.3"
        )
        .is_ok());
        for output in [
            b"celld 1.2.4".as_slice(),
            b"celld 1.2.2",
            b"celld 1.2.3 (debug)",
            b"other 1.2.3",
            b"2026-10-05T17:00:48Z  WARN celld::memory: allocator warning\ncelld 1.2.4",
            b"2026-10-05T17:00:48Z  WARN celld::memory: allocator warning\ncelld 1.2.4\ncelld 1.2.3",
            b"unexpected extra output\ncelld 1.2.3",
        ] {
            assert!(verify_output(output, "1.2.3").is_err());
        }
    }
}
