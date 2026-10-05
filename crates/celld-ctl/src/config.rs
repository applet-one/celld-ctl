use crate::registry::PUBLIC_PORT_OFFSET;
use anyhow::{bail, ensure, Context, Result};
use celld_ctl_core::valid_version;
use serde::{Deserialize, Serialize};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub bucket: String,
    pub endpoint: String,
    pub region: String,
    pub celld_version: String,
    #[serde(default = "port_start")]
    pub port_start: u16,
    #[serde(default = "port_end")]
    pub port_end: u16,
    #[serde(default = "internal_start")]
    pub internal_port_start: u16,
    #[serde(default = "internal_end")]
    pub internal_port_end: u16,
    #[serde(default = "timeout")]
    pub readiness_timeout_secs: u64,
}
fn port_start() -> u16 {
    8101
}
fn port_end() -> u16 {
    8999
}
fn internal_start() -> u16 {
    18101
}
fn internal_end() -> u16 {
    18999
}
fn timeout() -> u64 {
    30
}
impl Config {
    pub fn load(paths: &Paths) -> Result<Self> {
        paths.check_file(&paths.config, true)?;
        let bytes = fs::read(&paths.config).context("read host configuration")?;
        ensure!(bytes.len() <= 16384, "configuration exceeds size limit");
        let c: Self = serde_json::from_slice(&bytes).context("invalid host configuration")?;
        c.validate()?;
        Ok(c)
    }
    pub fn validate(&self) -> Result<()> {
        let bucket = self
            .bucket
            .strip_prefix("s3://")
            .context("bucket must be s3://BUCKET, without a prefix")?;
        ensure!(
            (3..=63).contains(&bucket.len())
                && bucket.bytes().all(|b| b.is_ascii_lowercase()
                    || b.is_ascii_digit()
                    || b == b'-'
                    || b == b'.')
                && !bucket.starts_with('-')
                && !bucket.ends_with('-'),
            "invalid bucket"
        );
        storage_origin(&self.endpoint)?;
        ensure!(
            !self.region.is_empty()
                && self.region.len() <= 64
                && self
                    .region
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-'),
            "invalid region"
        );
        ensure!(
            valid_version(&self.celld_version),
            "celld_version must be an exact numeric release, without v"
        );
        ensure!(
            self.port_start > 1024
                && self.port_start <= self.port_end
                && self.internal_port_start > 1024
                && self.internal_port_start <= self.internal_port_end,
            "invalid port range"
        );
        ensure!(
            self.port_end < self.internal_port_start || self.internal_port_end < self.port_start,
            "port ranges overlap"
        );
        ensure!(
            self.port_start >= 3000 && self.port_end <= 8999,
            "app ports must be 3000-8999 so their dedicated public ports are 4000-9999"
        );
        let public_start = self.port_start + PUBLIC_PORT_OFFSET;
        let public_end = self.port_end + PUBLIC_PORT_OFFSET;
        ensure!(
            self.port_end < public_start || public_end < self.port_start,
            "app and dedicated public port ranges overlap"
        );
        ensure!(
            self.internal_port_end < public_start || public_end < self.internal_port_start,
            "internal and dedicated public port ranges overlap"
        );
        ensure!(
            [8000, 2019]
                .iter()
                .all(|port| !(self.port_start..=self.port_end).contains(port)
                    && !(self.internal_port_start..=self.internal_port_end).contains(port)
                    && !(public_start..=public_end).contains(port)),
            "ports 8000 and 2019 belong to Caddy"
        );
        ensure!(
            (1..=120).contains(&self.readiness_timeout_secs),
            "readiness timeout must be 1-120 seconds"
        );
        Ok(())
    }
}

/// Accept public HTTPS origins and only a literal, explicitly ported IPv4
/// loopback HTTP origin. Inspect the original spelling: URL parsing normalizes
/// alternate numeric IPv4 forms (and could otherwise turn them into loopback).
pub fn storage_origin(endpoint: &str) -> Result<reqwest::Url> {
    let url = reqwest::Url::parse(endpoint).context("invalid storage endpoint")?;
    ensure!(
        url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
            && url.path() == "/",
        "storage endpoint must be an origin without credentials, path or query"
    );
    match url.scheme() {
        "https" => {}
        "http" => {
            let port: u16 = endpoint
                .strip_prefix("http://127.0.0.1:")
                .and_then(|s| s.strip_suffix('/').or(Some(s)))
                .context("HTTP storage requires literal 127.0.0.1 and an explicit port")?
                .parse()
                .context("invalid HTTP storage port")?;
            ensure!(
                port > 0
                    && (endpoint == format!("http://127.0.0.1:{port}")
                        || endpoint == format!("http://127.0.0.1:{port}/")),
                "HTTP storage is permitted only at literal 127.0.0.1 with an explicit port"
            );
        }
        _ => bail!("storage endpoint requires HTTPS or literal loopback HTTP"),
    }
    Ok(url)
}

/// Paths are selected by the *operator*, never deserialized from transport input/config.
#[derive(Clone, Debug)]
pub struct Paths {
    pub config: PathBuf,
    pub state: PathBuf,
    pub registry: PathBuf,
    pub credentials: PathBuf,
    pub cells: PathBuf,
    pub units: PathBuf,
    pub caddy: PathBuf,
    pub public: PathBuf,
    pub releases: PathBuf,
    pub app_state: PathBuf,
    pub strict: bool,
}
impl Default for Paths {
    fn default() -> Self {
        Self::production()
    }
}
impl Paths {
    pub fn production() -> Self {
        Self::at(Path::new("/"), true)
    }
    /// Library/operator fixture injection only. Never reachable from transport.
    pub fn under(root: &Path) -> Self {
        Self::at(root, false)
    }
    fn at(root: &Path, strict: bool) -> Self {
        let state = root.join("var/lib/celld-ctl");
        Self {
            config: root.join("etc/celld-ctl/config.json"),
            registry: state.join("registry.sqlite"),
            credentials: root.join("etc/celld/node.env"),
            cells: root.join("etc/celld/cells"),
            units: root.join("etc/systemd/system"),
            caddy: root.join("etc/caddy/Caddyfile"),
            public: state.join("public"),
            releases: root.join("usr/local/lib/celld/releases"),
            app_state: root.join("var/lib/celld"),
            state,
            strict,
        }
    }
    pub fn check_file(&self, path: &Path, private: bool) -> Result<()> {
        let m = fs::symlink_metadata(path)
            .with_context(|| format!("missing required file {}", path.display()))?;
        ensure!(
            m.is_file() && !m.file_type().is_symlink(),
            "unsafe file type"
        );
        if self.strict {
            ensure!(
                m.uid() == 0 && m.mode() & 0o022 == 0,
                "file must be root-owned and not writable by group/other"
            );
            if private {
                ensure!(
                    m.mode() & 0o077 == 0,
                    "private file must have mode 0600 or stricter"
                );
            }
            self.check_parents(path.parent().context("file has no parent")?)?;
        }
        Ok(())
    }
    pub fn check_parents(&self, path: &Path) -> Result<()> {
        for p in path.ancestors() {
            let m = fs::symlink_metadata(p)?;
            ensure!(
                m.is_dir() && !m.file_type().is_symlink(),
                "unsafe directory"
            );
            if self.strict {
                ensure!(
                    m.uid() == 0 && m.mode() & 0o022 == 0,
                    "directory must be root-owned and not writable by group/other"
                );
            }
        }
        Ok(())
    }
    pub fn make_dir(&self, path: &Path, mode: u32) -> Result<()> {
        if !path.exists() {
            let parent = path.parent().context("directory has no parent")?;
            if !parent.exists() {
                self.make_dir(parent, 0o755)?;
            }
            self.check_parents(parent)?;
            // Two installer/bootstrap callers can race on a previously absent
            // directory. Recheck type/ownership below rather than failing an
            // otherwise safe concurrent creation.
            match fs::create_dir(path) {
                Ok(()) => fs::set_permissions(path, fs::Permissions::from_mode(mode))?,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e.into()),
            }
        }
        self.check_parents(path)
    }
    pub fn initialize(&self) -> Result<()> {
        self.make_dir(&self.state, 0o755)?;
        // Public status needs traversal; private files have individual 0600 modes.
        self.make_dir(&self.cells, 0o700)?;
        for p in [&self.units, self.caddy.parent().unwrap(), &self.public] {
            self.make_dir(p, 0o755)?;
        }
        if self.registry.symlink_metadata().is_ok() {
            self.check_file(&self.registry, true)?;
        }
        for suffix in ["-wal", "-shm", "-journal"] {
            let p = PathBuf::from(format!("{}{suffix}", self.registry.display()));
            if p.symlink_metadata().is_ok() {
                self.check_file(&p, true)?;
            }
        }
        Ok(())
    }
}

pub fn atomic_write(paths: &Paths, path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let parent = path.parent().context("missing output directory")?;
    paths.check_parents(parent)?;
    if let Ok(m) = fs::symlink_metadata(path) {
        ensure!(
            m.is_file() && !m.file_type().is_symlink(),
            "refusing non-regular destination"
        );
    }
    let tmp = path.with_file_name(format!(
        ".{}.{}.tmp",
        path.file_name().unwrap().to_string_lossy(),
        std::process::id()
    ));
    let result = (|| {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&tmp)?;
        // The entry point uses umask 077 so SQLite and lock files are private
        // at creation. Explicitly restore the requested output mode on this
        // already-open, exclusively created inode (HTML/Caddy need 0644).
        f.set_permissions(fs::Permissions::from_mode(mode))?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        fs::rename(&tmp, path)?;
        fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

pub fn parse_credentials(paths: &Paths) -> Result<std::collections::BTreeMap<String, String>> {
    paths.check_file(&paths.credentials, true)?;
    let text = fs::read_to_string(&paths.credentials).context("read node credentials")?;
    ensure!(text.len() <= 64 * 1024, "credential file too large");
    let mut out = std::collections::BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            bail!("invalid node environment file");
        };
        if ![
            "AWS_ACCESS_KEY_ID",
            "AWS_SECRET_ACCESS_KEY",
            "AWS_SESSION_TOKEN",
        ]
        .contains(&key)
        {
            continue;
        }
        let mut value = value.trim();
        if value.len() >= 2
            && ((value.starts_with('"') && value.ends_with('"'))
                || (value.starts_with('\'') && value.ends_with('\'')))
        {
            value = &value[1..value.len() - 1];
        }
        ensure!(
            !value.is_empty()
                && value
                    .bytes()
                    .all(|b| b.is_ascii_graphic() && b != b'"' && b != b'\'' && b != b'\\'),
            "unsupported node credential encoding"
        );
        ensure!(
            out.insert(key.into(), value.into()).is_none(),
            "duplicate node credential"
        );
    }
    ensure!(
        out.contains_key("AWS_ACCESS_KEY_ID") && out.contains_key("AWS_SECRET_ACCESS_KEY"),
        "node environment lacks AWS credentials"
    );
    Ok(out)
}
