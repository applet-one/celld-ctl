//! Root-operator-only bootstrap for the installer-managed, single-node RustFS.
//! No transport/SSH request calls this module; paths are fixed by `Paths`.
use crate::{
    config::{atomic_write, parse_credentials, Config, Paths},
    s3,
};
use anyhow::{bail, ensure, Context, Result};
use fs2::FileExt;
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::Read,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

const ENDPOINT: &str = "http://127.0.0.1:9000";
const BUCKET: &str = "celld-dev";
const REGION: &str = "us-east-1";
const BUCKET_PATH: &str = "/celld-dev";
const DURABILITY_PATH: &str = "/rustfs/admin/v3/bucket-durability/celld-dev";
const STRICT_BODY: &[u8] = br#"{"mode":"strict"}"#;

// Paths are not accepted from CLI args, registry entries or SSH data. Fixture
// mapping changes only the root; production uses precisely these two files.
fn service_env(paths: &Paths) -> PathBuf {
    // Paths has no RustFS field; its config resides at ROOT/etc/celld-ctl/config.json.
    paths
        .config
        .parent()
        .expect("fixed config parent")
        .parent()
        .expect("fixed etc parent")
        .join("rustfs/rustfs.env")
}

fn exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}

fn require_operator(paths: &Paths) -> Result<()> {
    // SAFETY: geteuid is side-effect free.
    ensure!(
        !paths.strict || unsafe { libc::geteuid() } == 0,
        "storage bootstrap requires root"
    );
    Ok(())
}

fn lock(paths: &Paths) -> Result<File> {
    paths.make_dir(&paths.state, 0o755)?;
    let path = paths.state.join("storage.lock");
    if exists(&path)? {
        paths.check_file(&path, true)?;
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&path)
        .context("open storage bootstrap lock")?;
    paths.check_file(&path, true)?;
    let start = Instant::now();
    loop {
        match file.try_lock_exclusive() {
            Ok(()) => return Ok(file),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                ensure!(
                    start.elapsed() < Duration::from_secs(10),
                    "storage bootstrap busy"
                );
                std::thread::sleep(Duration::from_millis(40));
            }
            Err(e) => return Err(e.into()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Keys {
    access: String,
    secret: String,
}
impl Keys {
    fn validate(&self) -> Result<()> {
        ensure!(
            self.access.len() == 48
                && self.secret.len() == 64
                && self
                    .access
                    .bytes()
                    .chain(self.secret.bytes())
                    .all(|b| b.is_ascii_hexdigit()),
            "invalid local storage credential format"
        );
        Ok(())
    }
    fn random() -> Result<Self> {
        fn random_hex<const N: usize>() -> Result<String> {
            let mut bytes = [0u8; N];
            let mut offset = 0;
            while offset < N {
                // SAFETY: writable stack buffer is valid for the supplied length.
                let read =
                    unsafe { libc::getrandom(bytes[offset..].as_mut_ptr().cast(), N - offset, 0) };
                if read < 0 {
                    let e = std::io::Error::last_os_error();
                    if e.kind() == std::io::ErrorKind::Interrupted {
                        continue;
                    }
                    return Err(e).context("obtain operating-system entropy");
                }
                ensure!(read != 0, "operating-system entropy returned no bytes");
                offset += read as usize;
            }
            Ok(hex::encode(bytes))
        }
        Ok(Self {
            access: random_hex::<24>()?,
            secret: random_hex::<32>()?,
        })
    }
    fn rustfs_file(&self) -> Vec<u8> {
        format!(
            "RUSTFS_ACCESS_KEY={}\nRUSTFS_SECRET_KEY={}\n",
            self.access, self.secret
        )
        .into_bytes()
    }
    fn node_file(&self) -> Vec<u8> {
        format!(
            "AWS_ACCESS_KEY_ID={}\nAWS_SECRET_ACCESS_KEY={}\n",
            self.access, self.secret
        )
        .into_bytes()
    }
    fn aws(&self) -> BTreeMap<String, String> {
        BTreeMap::from([
            ("AWS_ACCESS_KEY_ID".into(), self.access.clone()),
            ("AWS_SECRET_ACCESS_KEY".into(), self.secret.clone()),
        ])
    }
}
fn read_service_keys(paths: &Paths, file: &Path) -> Result<Keys> {
    paths.check_file(file, true)?;
    ensure!(
        fs::metadata(file)?.len() <= 1024,
        "local service credentials too large"
    );
    let contents = fs::read_to_string(file).context("read local service credentials")?;
    ensure!(
        contents.len() <= 1024,
        "local service credentials too large"
    );
    let mut entries = BTreeMap::new();
    for line in contents.lines() {
        let (key, value) = line
            .split_once('=')
            .context("invalid local service credential line")?;
        ensure!(
            ["RUSTFS_ACCESS_KEY", "RUSTFS_SECRET_KEY"].contains(&key),
            "unexpected local service credential"
        );
        ensure!(
            entries.insert(key, value).is_none(),
            "duplicate local service credential"
        );
    }
    let keys = Keys {
        access: entries
            .get("RUSTFS_ACCESS_KEY")
            .context("missing local access key")?
            .to_string(),
        secret: entries
            .get("RUSTFS_SECRET_KEY")
            .context("missing local secret key")?
            .to_string(),
    };
    keys.validate()?;
    Ok(keys)
}
fn read_node_keys(paths: &Paths) -> Result<Keys> {
    let env = parse_credentials(paths)?;
    ensure!(env.len() == 2, "unexpected node credential variables");
    let keys = Keys {
        access: env["AWS_ACCESS_KEY_ID"].clone(),
        secret: env["AWS_SECRET_ACCESS_KEY"].clone(),
    };
    keys.validate()?;
    Ok(keys)
}
fn check_config(paths: &Paths) -> Result<bool> {
    if !exists(&paths.config)? {
        return Ok(false);
    }
    let config = Config::load(paths)?;
    ensure!(config.bucket == format!("s3://{BUCKET}") && config.endpoint == ENDPOINT && config.region == REGION,
        "existing host storage configuration is not installer-managed local storage; refusing overwrite");
    Ok(true)
}
fn prepare_locked(paths: &Paths) -> Result<Keys> {
    // Refuse external configs before making even the first credential file.
    check_config(paths)?;
    let rustfs = service_env(paths);
    let has_service = exists(&rustfs)?;
    let has_node = exists(&paths.credentials)?;
    // If node.env exists alone, it may be a manually configured external host.
    ensure!(
        has_service || !has_node,
        "node credentials exist without local service credentials; refusing takeover"
    );
    let keys = if has_service {
        read_service_keys(paths, &rustfs)?
    } else {
        Keys::random()?
    };
    if has_node {
        ensure!(
            read_node_keys(paths)? == keys,
            "local service and node credentials differ; refusing rotation"
        );
    }
    paths.make_dir(
        rustfs
            .parent()
            .context("missing service environment directory")?,
        0o700,
    )?;
    paths.make_dir(
        paths
            .credentials
            .parent()
            .context("missing node environment directory")?,
        0o700,
    )?;
    if !has_service {
        atomic_write(paths, &rustfs, &keys.rustfs_file(), 0o600)?;
    }
    if !has_node {
        atomic_write(paths, &paths.credentials, &keys.node_file(), 0o600)?;
    }
    Ok(keys)
}

/// Prepare persistent credentials *before* starting RustFS. No bucket or host
/// configuration is published in this phase. The installer owns install-state.
pub fn prepare_local(paths: &Paths) -> Result<()> {
    require_operator(paths)?;
    let _guard = lock(paths)?;
    prepare_locked(paths)?;
    Ok(())
}

fn client() -> Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(1))
        .timeout(Duration::from_secs(3))
        .build()?)
}
fn request(
    client: &reqwest::blocking::Client,
    endpoint: &str,
    method: reqwest::Method,
    path: &str,
    credentials: &BTreeMap<String, String>,
    body: &[u8],
) -> Result<reqwest::blocking::Response> {
    s3::signed_request(client, method, endpoint, path, REGION, credentials, body)?
        .send()
        .map_err(|_| anyhow::anyhow!("local storage request unavailable"))
}
fn head(
    client: &reqwest::blocking::Client,
    endpoint: &str,
    credentials: &BTreeMap<String, String>,
) -> Result<reqwest::StatusCode> {
    let status = request(
        client,
        endpoint,
        reqwest::Method::HEAD,
        BUCKET_PATH,
        credentials,
        b"",
    )?
    .status();
    ensure!(
        status == reqwest::StatusCode::OK || status == reqwest::StatusCode::NOT_FOUND,
        "local bucket HEAD failed (HTTP {status}); refusing to recreate credentials or bucket"
    );
    Ok(status)
}
fn ensure_bucket(
    client: &reqwest::blocking::Client,
    endpoint: &str,
    credentials: &BTreeMap<String, String>,
    timeout: Duration,
) -> Result<()> {
    let until = Instant::now() + timeout;
    loop {
        match head(client, endpoint, credentials) {
            Ok(reqwest::StatusCode::OK) => return Ok(()),
            Ok(reqwest::StatusCode::NOT_FOUND) => break,
            Ok(_) => unreachable!(),
            Err(e)
                if e.to_string() == "local storage request unavailable"
                    && Instant::now() < until =>
            {
                std::thread::sleep(Duration::from_millis(250))
            }
            Err(e) => return Err(e).context("local storage readiness failed"),
        }
    }
    // Path-style PUT with no LocationConstraint for us-east-1. A failed or
    // concurrent PUT is ambiguous until readback; never delete/reset a bucket.
    let result = request(
        client,
        endpoint,
        reqwest::Method::PUT,
        BUCKET_PATH,
        credentials,
        b"",
    );
    match &result {
        Ok(response)
            if response.status().is_success()
                || response.status() == reqwest::StatusCode::CONFLICT => {}
        Ok(response) if response.status() == reqwest::StatusCode::FORBIDDEN => {
            bail!("local bucket creation denied; credentials were not rotated")
        }
        Ok(response) => bail!(
            "local bucket creation failed (HTTP {}); refusing to publish host configuration",
            response.status()
        ),
        Err(_) => {} // Lost response: a HEAD readback determines whether PUT committed.
    }
    ensure!(
        head(client, endpoint, credentials)? == reqwest::StatusCode::OK,
        "local bucket creation not confirmed by HEAD; refusing to publish host configuration"
    );
    Ok(())
}
fn strict_durability(
    client: &reqwest::blocking::Client,
    endpoint: &str,
    credentials: &BTreeMap<String, String>,
) -> Result<()> {
    let put = request(
        client,
        endpoint,
        reqwest::Method::PUT,
        DURABILITY_PATH,
        credentials,
        STRICT_BODY,
    )?;
    ensure!(put.status().is_success(), "RustFS strict bucket durability update failed (HTTP {}); check pinned release and admin access", put.status());
    let get = request(
        client,
        endpoint,
        reqwest::Method::GET,
        DURABILITY_PATH,
        credentials,
        b"",
    )?;
    ensure!(
        get.status().is_success(),
        "RustFS bucket durability readback failed (HTTP {})",
        get.status()
    );
    let mut body = Vec::new();
    get.take(1025).read_to_end(&mut body)?;
    ensure!(
        body.len() <= 1024,
        "bucket durability response exceeds size limit"
    );
    let value: serde_json::Value =
        serde_json::from_slice(&body).context("invalid bucket durability response")?;
    ensure!(
        value.get("bucket").and_then(|v| v.as_str()) == Some(BUCKET)
            && value.get("mode").and_then(|v| v.as_str()) == Some("strict"),
        "bucket strict durability was not confirmed by readback"
    );
    Ok(())
}

/// Native celld 0.6.1's `diagnose` tests conditional create/reject/update/
/// stale-reject and a ranged read. Run with only the intended local credentials
/// (never pass secrets in arguments), without inherited AWS/cloud settings.
fn diagnose(paths: &Paths, keys: &Keys, version: &str, timeout: Duration) -> Result<()> {
    let binary = paths.releases.join(format!("v{version}/celld"));
    paths
        .check_file(&binary, false)
        .context("missing pinned celld release for storage diagnostics")?;
    let mut child = Command::new(&binary)
        .args([
            "diagnose",
            "--bucket",
            "s3://celld-dev",
            "--endpoint",
            ENDPOINT,
            "--region",
            REGION,
            "--listen",
            "127.0.0.1:0",
            "--internal-listen",
            "127.0.0.1:0",
        ])
        .env_clear()
        .env("AWS_ACCESS_KEY_ID", &keys.access)
        .env("AWS_SECRET_ACCESS_KEY", &keys.secret)
        .env("AWS_EC2_METADATA_DISABLED", "true")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("start pinned celld storage diagnostics")?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                ensure!(
                    status.success(),
                    "pinned celld storage diagnostics failed; host configuration not published"
                );
                return Ok(());
            }
            Ok(None) => {}
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                bail!("could not wait for pinned celld storage diagnostics");
            }
        }
        if start.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            bail!("pinned celld storage diagnostics timed out; host configuration not published");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Run only *after* the installed local RustFS service has started. Fail closed
/// on denied/unreachable storage or missing durability support. Existing local
/// config is preserved byte-for-byte; external config is never touched.
pub fn init_local(paths: &Paths) -> Result<()> {
    require_operator(paths)?;
    let _guard = lock(paths)?;
    let keys = prepare_locked(paths)?;
    let client = client()?;
    ensure_bucket(&client, ENDPOINT, &keys.aws(), Duration::from_secs(45))?;
    strict_durability(&client, ENDPOINT, &keys.aws())?;
    let existing = check_config(paths)?;
    let version = if existing {
        Config::load(paths)?.celld_version
    } else {
        "0.6.1".to_owned()
    };
    diagnose(paths, &keys, &version, Duration::from_secs(90))?;
    if !existing {
        // Use Config's own defaults/validation, not a duplicate port policy.
        let config: Config = serde_json::from_value(serde_json::json!({
            "bucket": format!("s3://{BUCKET}"), "endpoint": ENDPOINT,
            "region": REGION, "celld_version": "0.6.1"
        }))?;
        config.validate()?;
        paths.make_dir(
            paths
                .config
                .parent()
                .context("missing host configuration directory")?,
            0o700,
        )?;
        atomic_write(
            paths,
            &paths.config,
            &serde_json::to_vec_pretty(&config)?,
            0o600,
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        os::unix::fs::{symlink, PermissionsExt},
        thread,
    };

    fn setup() -> (tempfile::TempDir, Paths) {
        let tmp = tempfile::tempdir().unwrap();
        let mut paths = Paths::under(tmp.path());
        paths.config = tmp.path().join("etc/celld-ctl/config.json");
        (tmp, paths)
    }
    #[test]
    fn fresh_prepare_reuses_credential_pair_and_keeps_config_absent() {
        let (_tmp, paths) = setup();
        prepare_local(&paths).unwrap();
        let before = fs::read(service_env(&paths)).unwrap();
        assert!(!paths.config.exists());
        let service = read_service_keys(&paths, &service_env(&paths)).unwrap();
        assert_eq!(service, read_node_keys(&paths).unwrap());
        prepare_local(&paths).unwrap();
        assert_eq!(before, fs::read(service_env(&paths)).unwrap());
        assert_eq!(
            fs::metadata(service_env(&paths))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(&paths.credentials)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    #[test]
    fn partial_and_external_never_overwritten() {
        let (_tmp, paths) = setup();
        paths
            .make_dir(paths.credentials.parent().unwrap(), 0o700)
            .unwrap();
        fs::write(
            &paths.credentials,
            b"AWS_ACCESS_KEY_ID=external\nAWS_SECRET_ACCESS_KEY=external\n",
        )
        .unwrap();
        assert!(prepare_local(&paths).is_err());
        assert!(!service_env(&paths).exists());
        fs::remove_file(&paths.credentials).unwrap();
        prepare_local(&paths).unwrap();
        fs::remove_file(&paths.credentials).unwrap();
        prepare_local(&paths).unwrap(); // reconstruct from persisted service pair
        fs::write(
            &paths.credentials,
            b"AWS_ACCESS_KEY_ID=invalid\nAWS_SECRET_ACCESS_KEY=invalid\n",
        )
        .unwrap();
        assert!(prepare_local(&paths).is_err());
        let original = fs::read(service_env(&paths)).unwrap();
        paths
            .make_dir(paths.config.parent().unwrap(), 0o700)
            .unwrap();
        fs::write(&paths.config, br#"{"bucket":"s3://external","endpoint":"https://example.com","region":"us-east-1","celld_version":"0.6.1"}"#).unwrap();
        assert!(prepare_local(&paths).is_err());
        assert_eq!(fs::read(service_env(&paths)).unwrap(), original);
    }
    #[test]
    fn symlinks_rejected_even_in_fixture() {
        let (tmp, paths) = setup();
        paths
            .make_dir(paths.credentials.parent().unwrap(), 0o700)
            .unwrap();
        symlink(tmp.path().join("target"), &paths.credentials).unwrap();
        assert!(prepare_local(&paths).is_err());
    }
    #[test]
    fn concurrent_prepare_reuses_single_pair() {
        let (_tmp, paths) = setup();
        let a = paths.clone();
        let b = paths.clone();
        let one = thread::spawn(move || prepare_local(&a));
        let two = thread::spawn(move || prepare_local(&b));
        one.join().unwrap().unwrap();
        two.join().unwrap().unwrap();
        assert_eq!(
            read_service_keys(&paths, &service_env(&paths)).unwrap(),
            read_node_keys(&paths).unwrap()
        );
    }
    #[test]
    fn diagnose_uses_pinned_binary_clean_environment_and_timeout() {
        let (_tmp, paths) = setup();
        let parent = paths.releases.join("v0.6.1");
        paths.make_dir(&parent, 0o755).unwrap();
        let binary = parent.join("celld");
        fs::write(
            &binary,
            b"#!/bin/sh\n\
            test \"$1\" = diagnose && test \"$2\" = --bucket && test \"$3\" = s3://celld-dev && \
            test \"$4\" = --endpoint && test \"$5\" = http://127.0.0.1:9000 && \
            test \"$6\" = --region && test \"$7\" = us-east-1 && \
            test \"$8\" = --listen && test \"$9\" = 127.0.0.1:0 && \
            test -n \"$AWS_ACCESS_KEY_ID\" && test -n \"$AWS_SECRET_ACCESS_KEY\" && \
            test -z \"$AWS_SESSION_TOKEN\" && test -z \"$UNTRUSTED_INHERITED_VAR\"\n",
        )
        .unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
        let keys = Keys::random().unwrap();
        assert!(diagnose(&paths, &keys, "0.6.1", Duration::from_secs(2)).is_ok());
        fs::write(&binary, b"#!/bin/sh\nexit 7\n").unwrap();
        assert!(diagnose(&paths, &keys, "0.6.1", Duration::from_secs(2)).is_err());
        fs::write(&binary, b"#!/bin/sh\nwhile :; do :; done\n").unwrap();
        let start = Instant::now();
        assert!(diagnose(&paths, &keys, "0.6.1", Duration::from_millis(50)).is_err());
        assert!(start.elapsed() < Duration::from_secs(2));
        assert!(diagnose(&paths, &keys, "0.6.2", Duration::from_secs(2)).is_err());
    }
}

#[cfg(test)]
mod http_tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };

    // Scripted loopback server: real reqwest requests, no RustFS process or
    // network outside this test. Every request must be signed and path-style.
    fn mock(
        replies: Vec<Option<(u16, &'static str)>>,
    ) -> (String, thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let handle = thread::spawn(move || {
            let mut requests = Vec::new();
            for reply in replies {
                let start = Instant::now();
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(e)
                            if e.kind() == std::io::ErrorKind::WouldBlock
                                && start.elapsed() < Duration::from_secs(3) =>
                        {
                            thread::sleep(Duration::from_millis(5))
                        }
                        Err(e) => panic!("mock accept: {e}"),
                    }
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut buf = Vec::new();
                let mut tmp = [0u8; 1024];
                let end = loop {
                    let n = stream.read(&mut tmp).unwrap();
                    assert!(n > 0 && buf.len() + n < 8192);
                    buf.extend_from_slice(&tmp[..n]);
                    if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        break end + 4;
                    }
                };
                let header = String::from_utf8(buf[..end].to_vec()).unwrap();
                let length: usize = header
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length: ")
                            .and_then(|s| s.trim().parse().ok())
                    })
                    .unwrap_or(0);
                while buf.len() < end + length {
                    let n = stream.read(&mut tmp).unwrap();
                    assert!(n > 0);
                    buf.extend_from_slice(&tmp[..n]);
                }
                assert!(header
                    .to_ascii_lowercase()
                    .contains("authorization: aws4-hmac-sha256 "));
                assert!(header
                    .to_ascii_lowercase()
                    .contains(&format!("host: {addr}").to_lowercase()));
                requests.push(header.lines().next().unwrap().to_owned());
                if let Some((status, body)) = reply {
                    write!(stream, "HTTP/1.1 {status} Mock\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
                } // none simulates response lost after server received a PUT
            }
            requests
        });
        (format!("http://{addr}"), handle)
    }
    fn keys() -> BTreeMap<String, String> {
        BTreeMap::from([
            ("AWS_ACCESS_KEY_ID".into(), "abcd1234".into()),
            ("AWS_SECRET_ACCESS_KEY".into(), "abcd5678".into()),
        ])
    }
    #[test]
    fn existing_and_new_and_concurrent_buckets() {
        for (script, expected) in [
            (vec![Some((200, ""))], vec!["HEAD /celld-dev HTTP/1.1"]),
            (
                vec![Some((404, "")), Some((200, "")), Some((200, ""))],
                vec![
                    "HEAD /celld-dev HTTP/1.1",
                    "PUT /celld-dev HTTP/1.1",
                    "HEAD /celld-dev HTTP/1.1",
                ],
            ),
            (
                vec![Some((404, "")), Some((409, "")), Some((200, ""))],
                vec![
                    "HEAD /celld-dev HTTP/1.1",
                    "PUT /celld-dev HTTP/1.1",
                    "HEAD /celld-dev HTTP/1.1",
                ],
            ),
            (
                vec![Some((404, "")), None, Some((200, ""))],
                vec![
                    "HEAD /celld-dev HTTP/1.1",
                    "PUT /celld-dev HTTP/1.1",
                    "HEAD /celld-dev HTTP/1.1",
                ],
            ),
        ] {
            let (endpoint, server) = mock(script);
            ensure_bucket(
                &client().unwrap(),
                &endpoint,
                &keys(),
                Duration::from_secs(2),
            )
            .unwrap();
            assert_eq!(server.join().unwrap(), expected);
        }
    }
    #[test]
    fn denied_and_unavailable_fail_closed() {
        let (endpoint, server) = mock(vec![Some((403, ""))]);
        assert!(ensure_bucket(
            &client().unwrap(),
            &endpoint,
            &keys(),
            Duration::from_secs(1)
        )
        .is_err());
        assert_eq!(server.join().unwrap(), ["HEAD /celld-dev HTTP/1.1"]);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        let start = Instant::now();
        assert!(ensure_bucket(
            &client().unwrap(),
            &endpoint,
            &keys(),
            Duration::from_millis(50)
        )
        .is_err());
        assert!(start.elapsed() < Duration::from_secs(3));
    }
    #[test]
    fn strict_durability_requires_exact_readback() {
        let (endpoint, server) = mock(vec![
            Some((200, "")),
            Some((200, r#"{"bucket":"celld-dev","mode":"strict"}"#)),
        ]);
        strict_durability(&client().unwrap(), &endpoint, &keys()).unwrap();
        assert_eq!(
            server.join().unwrap(),
            [
                "PUT /rustfs/admin/v3/bucket-durability/celld-dev HTTP/1.1",
                "GET /rustfs/admin/v3/bucket-durability/celld-dev HTTP/1.1",
            ]
        );
        let (endpoint, server) = mock(vec![
            Some((200, "")),
            Some((200, r#"{"bucket":"celld-dev","mode":null}"#)),
        ]);
        assert!(strict_durability(&client().unwrap(), &endpoint, &keys()).is_err());
        server.join().unwrap();
        let (endpoint, server) = mock(vec![Some((404, ""))]);
        assert!(strict_durability(&client().unwrap(), &endpoint, &keys()).is_err());
        server.join().unwrap();
    }
}
