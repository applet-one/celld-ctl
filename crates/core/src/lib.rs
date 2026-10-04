//! Credential-free, bounded host/developer transport contract.
use serde::{Deserialize, Serialize};

pub const SSH_COMMAND: &str = "celld-ctl-transport";
pub const MAX_REQUEST_BYTES: usize = 16 * 1024;
pub const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
pub const MAX_LOG_LINES: u32 = 1000;
pub const MAX_HISTORY: usize = 100;
pub const MAX_BUNDLE_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_DECODED_BYTES: usize = 24 * 1024 * 1024;
pub const MAX_UPLOAD_FILES: usize = 4096;
pub const MAX_CONFIG_BYTES: usize = 64 * 1024;
/// Includes the modules/ and assets/ roots; no additional per-path depth limit.
pub const MAX_STAGING_DIRECTORIES: usize = 8192;
pub mod bundle;
pub use bundle::{valid_upload_path, PreparedBundle, UploadFile};

pub fn valid_slug(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 63
        && s.as_bytes()[0].is_ascii_alphanumeric()
        && s.as_bytes().last().is_some_and(u8::is_ascii_alphanumeric)
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

pub fn valid_version(s: &str) -> bool {
    let parts: Vec<_> = s.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.len() <= 8 && p.bytes().all(|b| b.is_ascii_digit()))
}

pub fn valid_deployment_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub slug: String,
    /// Native celld bucket URI, including fleet prefix, e.g. s3://BUCKET/cells/app.
    pub bucket: String,
    pub endpoint: String,
    pub region: String,
    /// Numeric release without a leading `v`, e.g. 0.4.0.
    pub celld_version: String,
    pub enabled: bool,
}

/// SSH deployment metadata deliberately excludes all object-store configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DeployTarget {
    pub slug: String,
    pub celld_version: String,
    pub enabled: bool,
}
impl From<&Target> for DeployTarget {
    fn from(target: &Target) -> Self {
        Self {
            slug: target.slug.clone(),
            celld_version: target.celld_version.clone(),
            enabled: target.enabled,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Provision {
        slug: String,
    },
    Target {
        slug: String,
    },
    Activate {
        slug: String,
        version_id: String,
        #[serde(default)]
        source_revision: Option<String>,
    },
    Deploy {
        slug: String,
        celld_version: String,
        version_id: String,
        #[serde(default)]
        source_revision: Option<String>,
        bundle_size: usize,
    },
    Logs {
        slug: String,
        #[serde(default = "default_lines")]
        lines: u32,
    },
    Status {
        slug: String,
    },
    Deployments {
        slug: String,
    },
}
fn default_lines() -> u32 {
    100
}
impl Request {
    pub fn slug(&self) -> &str {
        match self {
            Self::Provision { slug }
            | Self::Target { slug }
            | Self::Activate { slug, .. }
            | Self::Deploy { slug, .. }
            | Self::Logs { slug, .. }
            | Self::Status { slug }
            | Self::Deployments { slug } => slug,
        }
    }
    pub fn validate(&self) -> Result<(), &'static str> {
        if !valid_slug(self.slug()) {
            return Err("invalid slug: use 1-63 lowercase letters, digits or hyphens, starting and ending alphanumeric");
        }
        if let Self::Activate {
            version_id,
            source_revision,
            ..
        }
        | Self::Deploy {
            version_id,
            source_revision,
            ..
        } = self
        {
            if !valid_deployment_id(version_id) {
                return Err("invalid deployment version ID");
            }
            if source_revision.as_ref().is_some_and(|s| {
                s.is_empty()
                    || s.len() > 200
                    || !s
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"._/-".contains(&b))
            }) {
                return Err("invalid source revision");
            }
        }
        if let Self::Deploy {
            celld_version,
            bundle_size,
            ..
        } = self
        {
            if !valid_version(celld_version) {
                return Err("invalid pinned celld version");
            }
            if *bundle_size == 0 || *bundle_size > MAX_BUNDLE_BYTES {
                return Err("bundle size must be 1..32 MiB");
            }
        }
        if let Self::Logs { lines, .. } = self {
            if *lines == 0 || *lines > MAX_LOG_LINES {
                return Err("lines must be between 1 and 1000");
            }
        }
        Ok(())
    }
    pub fn mutates(&self) -> bool {
        matches!(
            self,
            Self::Provision { .. } | Self::Activate { .. } | Self::Deploy { .. }
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Deployment {
    pub id: i64,
    pub slug: String,
    pub version_id: String,
    pub source_revision: Option<String>,
    pub deployed_at: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Response {
    Success { ok: bool, result: serde_json::Value },
    Failure { ok: bool, error: String },
}
impl Response {
    pub fn success(result: impl Serialize) -> Result<Self, serde_json::Error> {
        Ok(Self::Success {
            ok: true,
            result: serde_json::to_value(result)?,
        })
    }
    pub fn failure(error: impl Into<String>) -> Self {
        Self::Failure {
            ok: false,
            error: error.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validation() {
        for s in ["a", "counter", "app-123"] {
            assert!(valid_slug(s));
        }
        for s in [
            "",
            "../a",
            "-a",
            "a-",
            "A",
            "a.service",
            "a\n",
            "a/b",
            "a;id",
        ] {
            assert!(!valid_slug(s));
        }
        assert!(!valid_slug(&"a".repeat(64)));
        for s in ["0.4.0", "1.22.333"] {
            assert!(valid_version(s));
        }
        for s in ["latest", "v0.4.0", "../celld", "1.2", "1.2.3\n"] {
            assert!(!valid_version(s));
        }
    }
    #[test]
    fn strict_protocol() {
        for s in [
            r#"{"op":"target","slug":"a","path":"/etc/shadow"}"#,
            r#"{"op":"remove","slug":"a"}"#,
            r#"{"op":"provision","slug":"a","celld_version":"latest"}"#,
        ] {
            assert!(serde_json::from_str::<Request>(s).is_err());
        }
        let r: Request =
            serde_json::from_str(r#"{"op":"activate","slug":"a","version_id":"abc"}"#).unwrap();
        r.validate().unwrap();
    }
}
