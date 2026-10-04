use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};

pub use celld_ctl_core::{DeployTarget as Target, Request};
pub const MAX_RESPONSE_BYTES: u64 = celld_ctl_core::MAX_RESPONSE_BYTES as u64;
pub const MAX_REQUEST_BYTES: usize = celld_ctl_core::MAX_REQUEST_BYTES;

pub fn validate_source_revision(revision: &str) -> Result<()> {
    if revision.is_empty()
        || revision.len() > 200
        || !revision
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._/-".contains(&b))
    {
        bail!("source revision must be 1–200 ASCII letters, digits, '.', '_', '/', or '-'");
    }
    Ok(())
}

pub struct Ssh {
    pub host: String,
    pub identity: PathBuf,
    pub port: u16,
}

impl Ssh {
    pub fn command(&self) -> Result<Command> {
        if self.host.is_empty()
            || self.host.starts_with('-')
            || !self
                .host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._@:-[]".contains(&b))
        {
            bail!("invalid SSH host: supply a literal hostname or user@hostname, not SSH options");
        }
        if self.port == 0 {
            bail!("SSH port must be nonzero");
        }
        let mut cmd = Command::new("ssh");
        // Ignore user ssh_config: SendEnv/SetEnv/ProxyCommand/LocalCommand must not
        // turn this narrow transport into credential forwarding or shell execution.
        cmd.env_clear();
        for key in ["PATH", "HOME", "LANG"] {
            if let Some(value) = std::env::var_os(key) {
                cmd.env(key, value);
            }
        }
        cmd.args(["-F", "/dev/null", "-T", "-a", "-x"]);
        for option in [
            "BatchMode=yes",
            "IdentitiesOnly=yes",
            "IdentityAgent=none",
            "ForwardAgent=no",
            "ForwardX11=no",
            "ClearAllForwardings=yes",
            "RequestTTY=no",
            "StrictHostKeyChecking=yes",
            "PermitLocalCommand=no",
            "ControlMaster=no",
            "ControlPath=none",
            "ConnectTimeout=15",
        ] {
            cmd.args(["-o", option]);
        }
        cmd.arg("-i")
            .arg(&self.identity)
            .arg("-p")
            .arg(self.port.to_string())
            .arg("--")
            .arg(&self.host)
            .arg("celld-ctl-transport");
        Ok(cmd)
    }

    pub fn request(&self, request: &Request) -> Result<Value> {
        if matches!(request, Request::Deploy { .. }) {
            bail!("deploy requires a prepared payload");
        }
        self.exchange(request, &[])
    }

    pub fn deploy(&self, request: &Request, payload: &[u8]) -> Result<Value> {
        match request {
            Request::Deploy { bundle_size, .. } if *bundle_size == payload.len() => {}
            _ => bail!("deploy metadata must describe the exact prepared payload length"),
        }
        self.exchange(request, payload)
    }

    fn exchange(&self, request: &Request, payload: &[u8]) -> Result<Value> {
        request.validate().map_err(anyhow::Error::msg)?;
        let mut bytes = serde_json::to_vec(request)?;
        bytes.push(b'\n');
        if bytes.len() > MAX_REQUEST_BYTES {
            bail!("host transport request exceeds {MAX_REQUEST_BYTES} bytes");
        }
        let mut child = self
            .command()?
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .context("start restricted SSH transport")?;
        let mut stdin = child.stdin.take().context("SSH stdin unavailable")?;
        let stdout = child.stdout.take().context("SSH stdout unavailable")?;
        // Read concurrently: a host can reject the header before consuming a large
        // payload, and its stdout must not block while our stdin pipe is full.
        std::thread::scope(|scope| -> Result<Value> {
            let writer = scope.spawn(move || -> std::io::Result<()> {
                stdin.write_all(&bytes)?;
                stdin.write_all(payload)
                // stdin is dropped here, marking exact payload EOF.
            });
            let mut response = Vec::new();
            let read = stdout
                .take(MAX_RESPONSE_BYTES + 1)
                .read_to_end(&mut response);
            if read.is_err() || response.len() as u64 > MAX_RESPONSE_BYTES {
                let _ = child.kill();
                let _ = child.wait();
                let _ = writer.join();
                read.context("read SSH transport response")?;
                bail!("host transport response exceeds {MAX_RESPONSE_BYTES} bytes");
            }
            let status = child.wait().context("wait for SSH transport")?;
            let write = writer
                .join()
                .map_err(|_| anyhow::anyhow!("SSH payload writer panicked"))?;
            if !status.success() && response.is_empty() {
                bail!("SSH transport exited with {status}");
            }
            // Preserve a host rejection even if it closed stdin before our write.
            let result = decode_response(&response)?;
            write.context("write transport request/payload")?;
            if !status.success() {
                bail!("SSH transport exited with {status}");
            }
            Ok(result)
        })
    }

    pub fn target(&self, request: &Request, slug: &str) -> Result<Target> {
        let target: Target =
            serde_json::from_value(self.request(request)?).context("invalid target response")?;
        if target.slug != slug {
            bail!("host returned target for another slug");
        }
        Ok(target)
    }
}

pub fn decode_response(bytes: &[u8]) -> Result<Value> {
    let response: Value =
        serde_json::from_slice(bytes).context("host transport returned invalid JSON")?;
    match response.get("ok").and_then(Value::as_bool) {
        Some(true) => response
            .get("result")
            .cloned()
            .context("host transport response has no result"),
        Some(false) => bail!(
            "{}",
            response
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("host operation failed")
        ),
        None => bail!("host transport response has no boolean ok field"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wrapper_is_strict() {
        assert_eq!(
            decode_response(br#"{"ok":true,"result":[1]}"#).unwrap(),
            serde_json::json!([1])
        );
        for json in [
            br#"{"ok":false,"error":"denied"}"#.as_slice(),
            br#"{"ok":true}"#,
            b"banner\n{}",
            b"{}",
        ][..]
            .iter()
        {
            assert!(decode_response(json).is_err());
        }
    }
    #[test]
    fn restricted_ssh_and_json_only() {
        let ssh = Ssh {
            host: "cella-deploy@example.invalid".into(),
            identity: "/tmp/key".into(),
            port: 22,
        };
        let command = ssh.command().unwrap();
        let args: Vec<_> = command
            .get_args()
            .map(|x| x.to_string_lossy().to_string())
            .collect();
        for arg in [
            "-T",
            "-a",
            "-x",
            "BatchMode=yes",
            "ClearAllForwardings=yes",
            "StrictHostKeyChecking=yes",
            "IdentityAgent=none",
        ] {
            assert!(args.contains(&arg.to_string()));
        }
        assert_eq!(args.last().unwrap(), "celld-ctl-transport");
        assert!(!args.iter().any(|arg| arg.contains("AWS_")));
        let json = serde_json::to_value(Request::Deploy {
            slug: "app".into(),
            celld_version: "1.2.3".into(),
            version_id: "abc".into(),
            source_revision: None,
            bundle_size: 100,
        })
        .unwrap();
        assert_eq!(
            json,
            serde_json::json!({"op":"deploy","slug":"app","celld_version":"1.2.3","version_id":"abc","source_revision":null,"bundle_size":100})
        );
    }
    #[test]
    fn host_injection_rejected() {
        for host in ["-oProxyCommand=evil", "x;touch /tmp/oops", "x\ny", ""] {
            assert!(Ssh {
                host: host.into(),
                identity: "key".into(),
                port: 22
            }
            .command()
            .is_err());
        }
    }
}
