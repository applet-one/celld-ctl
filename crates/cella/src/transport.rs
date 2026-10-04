use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};

pub const MAX_RESPONSE_BYTES: u64 = 1024 * 1024;
pub const MAX_REQUEST_BYTES: usize = 16 * 1024;

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

#[derive(Debug, Deserialize, Serialize)]
pub struct Target {
    pub slug: String,
    pub bucket: String,
    pub endpoint: String,
    pub region: String,
    pub celld_version: String,
    pub enabled: bool,
}

#[derive(Debug, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request<'a> {
    Provision {
        slug: &'a str,
    },
    Target {
        slug: &'a str,
    },
    Activate {
        slug: &'a str,
        version_id: &'a str,
        source_revision: Option<&'a str>,
    },
    Logs {
        slug: &'a str,
        lines: u16,
    },
    Status {
        slug: &'a str,
    },
    Deployments {
        slug: &'a str,
    },
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

    pub fn request(&self, request: &Request<'_>) -> Result<Value> {
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
        let write = child
            .stdin
            .take()
            .context("SSH stdin unavailable")?
            .write_all(&bytes);
        let mut stdout = Vec::new();
        let read = child
            .stdout
            .take()
            .context("SSH stdout unavailable")?
            .take(MAX_RESPONSE_BYTES + 1)
            .read_to_end(&mut stdout);
        if read.is_err() || stdout.len() as u64 > MAX_RESPONSE_BYTES {
            let _ = child.kill();
            let _ = child.wait();
            read.context("read SSH transport response")?;
            bail!("host transport response exceeds {MAX_RESPONSE_BYTES} bytes");
        }
        let status = child.wait().context("wait for SSH transport")?;
        // A server can reject and close stdin early; preserve its structured error.
        if !status.success() && stdout.is_empty() {
            bail!("SSH transport exited with {status}");
        }
        let result = decode_response(&stdout)?;
        write.context("write transport request")?;
        if !status.success() {
            bail!("SSH transport exited with {status}");
        }
        Ok(result)
    }

    pub fn target(&self, request: &Request<'_>, slug: &str) -> Result<Target> {
        let target: Target =
            serde_json::from_value(self.request(request)?).context("invalid target response")?;
        if target.slug != slug {
            bail!("host returned target for another slug");
        }
        if !target.bucket.starts_with("s3://")
            || target.bucket.len() <= 5
            || !target.endpoint.starts_with("https://")
            || target.region.is_empty()
        {
            bail!("host returned invalid S3 target metadata (HTTPS endpoint required)");
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
        let json = serde_json::to_value(Request::Activate {
            slug: "app",
            version_id: "abc",
            source_revision: None,
        })
        .unwrap();
        assert_eq!(
            json,
            serde_json::json!({"op":"activate","slug":"app","version_id":"abc","source_revision":null})
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
