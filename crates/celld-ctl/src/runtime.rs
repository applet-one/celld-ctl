use crate::{
    config::{parse_credentials, storage_origin, Paths},
    registry::App,
    s3,
};
use anyhow::{bail, ensure, Context, Result};
use std::{
    io::Read,
    net::{SocketAddr, TcpStream},
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

/// All effects are injectable for hermetic tests. The transport always uses RealRuntime.
pub trait Runtime {
    fn systemctl(&mut self, action: &str, unit: Option<&str>) -> Result<()>;
    fn active(&mut self, app: &App) -> Result<bool>;
    fn validate_caddy(&mut self, path: &Path) -> Result<()>;
    fn reload_caddy(&mut self, path: &Path) -> Result<()>;
    fn verify_pointer(&mut self, app: &App, expected: Option<&str>) -> Result<String>;
    fn readiness(&mut self, app: &App, expected: &str, timeout: u64) -> Result<()>;
    fn reload_app(&mut self, app: &App) -> Result<()>;
    fn logs(&mut self, app: &App, lines: u32) -> Result<String>;
    fn port_free(&mut self, port: u16) -> bool;
    fn verify_binary(&mut self, path: &Path, version: &str) -> Result<()>;
    fn observed_version(&mut self, app: &App) -> Result<Option<String>>;
    fn deploy(
        &mut self,
        _app: &App,
        _bundle: &celld_ctl_core::PreparedBundle,
        _expected: &str,
    ) -> Result<crate::publish::NativePublish> {
        bail!("native publisher is not implemented by this runtime")
    }
}

pub struct RealRuntime {
    pub paths: Paths,
}
impl RealRuntime {
    fn client(timeout: u64) -> Result<reqwest::blocking::Client> {
        Ok(reqwest::blocking::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(timeout))
            .build()?)
    }
    fn internal(
        &self,
        app: &App,
        method: reqwest::Method,
        path: &str,
    ) -> Result<serde_json::Value> {
        let url = format!("http://127.0.0.1:{}{}", app.internal_port, path);
        let response = Self::client(5)?
            .request(method, url)
            .header("Connection", "close")
            .send()
            .map_err(|_| anyhow::anyhow!("internal listener unavailable"))?;
        ensure!(response.status().is_success(), "internal operation failed");
        let bytes = read_bounded(response, 1024 * 1024)?;
        serde_json::from_slice(&bytes).context("invalid internal response")
    }
}

fn read_bounded(mut input: impl Read, limit: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    input
        .by_ref()
        .take((limit + 1) as u64)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= limit, "response exceeds size limit");
    Ok(bytes)
}

/// No shell, no inherited environment, no pager; timeout and output limits apply to children.
fn command(binary: &str, args: &[&str], limit: usize, timeout: u64) -> Result<Vec<u8>> {
    use std::os::fd::AsRawFd;
    let mut child = Command::new(binary)
        .args(args)
        .env_clear()
        .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
        .env("LC_ALL", "C")
        .env("SYSTEMD_PAGER", "cat")
        .env("SYSTEMD_COLORS", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("start trusted host command")?;
    let mut pipe = child.stdout.take().context("missing child output")?;
    let fd = pipe.as_raw_fd();
    // SAFETY: fd belongs to a live pipe; fcntl changes only this process's descriptor flags.
    if unsafe { libc::fcntl(fd, libc::F_SETFL, libc::O_NONBLOCK) } == -1 {
        let _ = child.kill();
        let _ = child.wait();
        bail!("configure command output");
    }
    let start = Instant::now();
    let mut out = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        loop {
            match pipe.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    out.extend_from_slice(&buf[..n]);
                    if out.len() > limit {
                        let _ = child.kill();
                        let _ = child.wait();
                        bail!("host command output exceeds limit");
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(_) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    bail!("read host command output");
                }
            }
        }
        if let Some(status) = child.try_wait()? {
            ensure!(
                status.success(),
                "host command failed (inspect root journal)"
            );
            loop {
                match pipe.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        out.extend_from_slice(&buf[..n]);
                        ensure!(out.len() <= limit, "host command output exceeds limit");
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(e) => return Err(e.into()),
                }
            }
            return Ok(out);
        }
        if start.elapsed() > Duration::from_secs(timeout) {
            let _ = child.kill();
            let _ = child.wait();
            bail!("host command timed out");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
impl Runtime for RealRuntime {
    fn deploy(
        &mut self,
        app: &App,
        bundle: &celld_ctl_core::PreparedBundle,
        expected: &str,
    ) -> Result<crate::publish::NativePublish> {
        crate::publish::deploy(&self.paths, app, bundle, expected)
    }
    fn systemctl(&mut self, action: &str, unit: Option<&str>) -> Result<()> {
        ensure!(
            [
                "start",
                "stop",
                "restart",
                "enable",
                "disable",
                "daemon-reload"
            ]
            .contains(&action),
            "invalid system action"
        );
        let mut args = vec!["--no-pager", action];
        if let Some(unit) = unit {
            args.push(unit);
        }
        command("/usr/bin/systemctl", &args, 64 * 1024, 90)?;
        Ok(())
    }
    fn active(&mut self, app: &App) -> Result<bool> {
        let bytes = command(
            "/usr/bin/systemctl",
            &[
                "--no-pager",
                "show",
                "--property=ActiveState",
                "--value",
                &app.unit,
            ],
            1024,
            10,
        )?;
        Ok(bytes == b"active\n")
    }
    fn validate_caddy(&mut self, path: &Path) -> Result<()> {
        command(
            "/usr/bin/caddy",
            &[
                "validate",
                "--adapter",
                "caddyfile",
                "--config",
                path.to_str().context("invalid Caddy path")?,
            ],
            64 * 1024,
            15,
        )?;
        Ok(())
    }
    fn reload_caddy(&mut self, path: &Path) -> Result<()> {
        command(
            "/usr/bin/caddy",
            &[
                "reload",
                "--adapter",
                "caddyfile",
                "--address",
                "127.0.0.1:2019",
                "--config",
                path.to_str().context("invalid Caddy path")?,
            ],
            64 * 1024,
            15,
        )?;
        Ok(())
    }
    fn verify_pointer(&mut self, app: &App, expected: Option<&str>) -> Result<String> {
        let creds = parse_credentials(&self.paths)?;
        let root = app
            .target
            .bucket
            .strip_prefix("s3://")
            .context("invalid bucket scheme")?;
        // Only registry-generated bucket paths enter the signed request.
        ensure!(
            root.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"/.-".contains(&b)),
            "invalid bucket path"
        );
        let path = format!("/{root}/deploy/current.json");
        storage_origin(&app.target.endpoint).context("invalid registry endpoint")?;
        let client = Self::client(10)?;
        let response = s3::signed_request(
            &client,
            reqwest::Method::GET,
            &app.target.endpoint,
            &path,
            &app.target.region,
            &creds,
            b"",
        )?
        .send()
        .map_err(|_| anyhow::anyhow!("deployment pointer request failed"))?;
        ensure!(
            response.status().is_success(),
            "deployment pointer unavailable; complete native celld deploy first"
        );
        let value: serde_json::Value = serde_json::from_slice(&read_bounded(response, 16384)?)
            .context("invalid deployment pointer")?;
        validate_pointer(&value, expected)
    }
    fn readiness(&mut self, app: &App, expected: &str, timeout: u64) -> Result<()> {
        let start = Instant::now();
        loop {
            let attempt = (|| {
                for port in [app.port, app.internal_port] {
                    let addr = SocketAddr::from(([127, 0, 0, 1], port));
                    TcpStream::connect_timeout(&addr, Duration::from_millis(200))
                        .context("listener not ready")?;
                }
                let state = self.internal(app, reqwest::Method::GET, "/state")?;
                ensure!(
                    state
                        .pointer("/deployment/version")
                        .and_then(|v| v.as_str())
                        == Some(expected),
                    "node has not adopted requested deployment"
                );
                // This reserved celld path never dispatches to Worker code.
                let response = Self::client(2)?
                    .get(format!(
                        "http://127.0.0.1:{}/.well-known/celld/health",
                        app.port
                    ))
                    .send()
                    .map_err(|_| anyhow::anyhow!("node health unavailable"))?;
                ensure!(response.status().is_success(), "node fleet not ready");
                let health: serde_json::Value =
                    serde_json::from_slice(&read_bounded(response, 1024)?)?;
                ensure!(
                    health.get("ok").and_then(|v| v.as_bool()) == Some(true),
                    "node fleet not ready"
                );
                Ok(())
            })();
            if attempt.is_ok() {
                return attempt;
            }
            if start.elapsed() >= Duration::from_secs(timeout) {
                bail!("readiness timed out; route remains unpublished");
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }
    fn reload_app(&mut self, app: &App) -> Result<()> {
        let value = self.internal(app, reqwest::Method::POST, "/reload")?;
        ensure!(
            value.get("ok").and_then(|v| v.as_bool()) == Some(true),
            "native reload failed"
        );
        Ok(())
    }
    fn logs(&mut self, app: &App, lines: u32) -> Result<String> {
        let n = lines.to_string();
        let mut args = vec![
            "--no-pager",
            "--quiet",
            "--output=short-iso",
            "--unit",
            &app.unit,
            "--lines",
            &n,
        ];
        args.push(if app.legacy {
            "--namespace=*"
        } else {
            "--namespace=celld"
        });
        let bytes = command("/usr/bin/journalctl", &args, 256 * 1024, 10)?;
        let mut text = String::from_utf8_lossy(&bytes).to_string();
        // Fail closed if credentials cannot be read: don't return unredacted journal data.
        for secret in parse_credentials(&self.paths)?.values() {
            text = text.replace(secret, "[REDACTED]");
        }
        Ok(text)
    }
    fn verify_binary(&mut self, path: &Path, version: &str) -> Result<()> {
        let bytes = command(
            path.to_str().context("invalid pinned path")?,
            &["--version"],
            1024,
            5,
        )?;
        let text = std::str::from_utf8(&bytes).context("invalid pinned version output")?;
        let fields = text.split_whitespace().collect::<Vec<_>>();
        ensure!(
            fields.len() >= 2 && fields[0] == "celld" && fields[1] == version,
            "pinned binary does not report the target release"
        );
        Ok(())
    }
    fn observed_version(&mut self, app: &App) -> Result<Option<String>> {
        let state = self.internal(app, reqwest::Method::GET, "/state")?;
        let version = state
            .pointer("/deployment/version")
            .and_then(|v| v.as_str());
        ensure!(
            version.is_none_or(celld_ctl_core::valid_deployment_id),
            "invalid observed version"
        );
        Ok(version.map(str::to_owned))
    }
    fn port_free(&mut self, port: u16) -> bool {
        std::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], port))).is_ok()
    }
}

/// Validate only native deployment identity, not the hosted routing slug: cella --slug
/// intentionally permits a different native Worker name in the selected fleet prefix.
pub fn validate_pointer(value: &serde_json::Value, expected: Option<&str>) -> Result<String> {
    let version = value
        .get("version")
        .and_then(|v| v.as_str())
        .context("deployment pointer lacks version")?;
    ensure!(
        celld_ctl_core::valid_deployment_id(version),
        "invalid pointer version"
    );
    let script = value
        .get("script_name")
        .and_then(|v| v.as_str())
        .context("deployment pointer lacks script name")?;
    ensure!(
        celld_ctl_core::valid_slug(script),
        "invalid deployment script name"
    );
    ensure!(
        value.get("prefix").and_then(|v| v.as_str())
            == Some(format!("deploy/{script}/{version}").as_str()),
        "invalid pointer manifest prefix"
    );
    ensure!(
        value.pointer("/rollout/percent").and_then(|v| v.as_u64()) == Some(100),
        "partial rollout cannot be activated"
    );
    if let Some(expected) = expected {
        ensure!(
            version == expected,
            "deployment pointer does not match requested version"
        );
    }
    Ok(version.into())
}
