//! Line-oriented diagnostics for captured native CLI output.
use std::collections::HashSet;

pub fn allocator_warning(line: &str) -> bool {
    line == "<jemalloc>: option background_thread currently supports pthread only"
        || (line.contains(" WARN celld::memory:")
            && line.contains("the allocator will not run a background thread")
            && line.contains("unknown/invalid value"))
}

/// Strip terminal escape sequences and turn progress updates into ordinary lines.
pub fn plain(text: &str) -> String {
    let mut chars = text.chars().peekable();
    let mut out = String::new();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            match chars.next() {
                Some('[') => {
                    for c in chars.by_ref() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    while let Some(c) = chars.next() {
                        if c == '\u{7}' {
                            break;
                        }
                        if c == '\u{1b}' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                }
                _ => {}
            }
        } else if ch == '\r' {
            if chars.peek() != Some(&'\n') {
                out.push('\n');
            }
        } else {
            out.push(ch);
        }
    }
    out.lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
}

fn routine(line: &str) -> bool {
    let line = line.trim();
    // Never classify a diagnostic as routine just because it starts with a UI label.
    let lower = line.to_ascii_lowercase();
    if lower.contains("warn") || lower.contains("error") || lower.contains("failed") {
        return false;
    }
    line.is_empty()
        || line.starts_with("celld ")
        || line.chars().all(|c| c == '─')
        || line.starts_with("Total Upload:")
        || line == "Your Worker has access to the following bindings:"
        || (line.starts_with("Binding") && line.ends_with("Resource"))
        || line.starts_with("env.")
        || line.starts_with("Bundled ")
        || line.starts_with("Uploaded ")
        || line.starts_with("s3://")
        || line == "Nodes adopt this version at their next pointer poll, without a restart."
}

#[derive(Default)]
pub struct Diagnostics {
    seen: HashSet<String>,
}
impl Diagnostics {
    pub fn show(&mut self, text: &str, verbose: bool) {
        for line in plain(text).lines() {
            if verbose || (!allocator_warning(line) && !routine(line)) {
                if verbose || self.seen.insert(line.to_owned()) {
                    eprintln!("{line}");
                }
            }
        }
    }
}

/// Build a public app URL from an SSH host, omitting its optional username.
pub fn app_url(host: &str, port: u64) -> Option<String> {
    let hostname = host.rsplit('@').next()?.trim();
    if hostname.is_empty() {
        return None;
    }
    // Bracket bare IPv6 addresses for URL syntax.
    let hostname = if hostname.contains(':') && !hostname.starts_with('[') {
        format!("[{hostname}]")
    } else {
        hostname.to_owned()
    };
    Some(format!("https://{hostname}:{port}"))
}

/// Abbreviate Git hashes, but preserve arbitrary source labels.
pub fn source_label(source: Option<&str>) -> String {
    let source = source.unwrap_or("unknown");
    let (source, dirty) = source
        .strip_suffix("-dirty")
        .map(|s| (s, " (dirty)"))
        .unwrap_or((source, ""));
    let short = if source.len() == 40 && source.bytes().all(|b| b.is_ascii_hexdigit()) {
        &source[..12]
    } else {
        source
    };
    format!("{short}{dirty}")
}

pub fn status(value: &serde_json::Value) -> anyhow::Result<String> {
    use anyhow::Context;
    use serde::Deserialize;
    #[derive(Deserialize)]
    struct Status {
        target: celld_ctl_core::DeployTarget,
        active: bool,
        version_id: Option<String>,
        observed_version_id: Option<String>,
        unit: String,
        port: u16,
        internal_port: u16,
        public_port: u16,
    }
    let status: Status =
        serde_json::from_value(value.clone()).context("invalid status response")?;
    let state = if status.active { "running" } else { "stopped" };
    let enabled = if status.target.enabled {
        "enabled"
    } else {
        "disabled"
    };
    let adoption = if !status.active {
        "Runtime is stopped; adoption cannot be confirmed."
    } else {
        match (&status.version_id, &status.observed_version_id) {
            (Some(expected), Some(observed)) if expected == observed => {
                "Runtime has adopted the activated version."
            }
            (Some(_), Some(_)) => "Runtime has not yet adopted the activated version.",
            _ => "Runtime adoption is unknown.",
        }
    };
    Ok(format!(
        "{} · {state} · {enabled}\n\nCelld      {}\nActivated  {}\nObserved   {}\nPublic     port {}\nRuntime    127.0.0.1:{}\nInternal   127.0.0.1:{}\nUnit       {}\n\n{adoption}\n",
        status.target.slug, status.target.celld_version,
        status.version_id.as_deref().unwrap_or("none"),
        status.observed_version_id.as_deref().unwrap_or("unknown"),
        status.public_port, status.port, status.internal_port, status.unit,
    ))
}

pub fn deployments(slug: &str, value: &serde_json::Value) -> anyhow::Result<String> {
    use anyhow::Context;
    use std::fmt::Write;
    let entries: Vec<celld_ctl_core::Deployment> =
        serde_json::from_value(value.clone()).context("invalid deployments response")?;
    if entries.is_empty() {
        return Ok(format!("No deployments recorded for {slug}.\n"));
    }
    let rows: Vec<_> = entries
        .iter()
        .map(|entry| {
            (
                entry.version_id.as_str(),
                source_label(entry.source_revision.as_deref()),
                entry.deployed_at.as_str(),
            )
        })
        .collect();
    let version_width = rows.iter().map(|r| r.0.len()).max().unwrap_or(0).max(7);
    let source_width = rows.iter().map(|r| r.1.len()).max().unwrap_or(0).max(6);
    let mut out = format!("Deployments for {slug} (newest first)\n\n");
    writeln!(
        out,
        "{:<version_width$}  {:<source_width$}  DEPLOYED AT",
        "VERSION", "SOURCE"
    )?;
    for (version, source, time) in rows {
        writeln!(
            out,
            "{version:<version_width$}  {source:<source_width$}  {time}"
        )?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normalizes_terminal_output() {
        assert_eq!(
            plain("\x1b[32mBundled test\x1b[0m   \rUploaded test  \r\n"),
            "Bundled test\nUploaded test"
        );
    }
    #[test]
    fn only_known_allocator_warning_is_suppressed() {
        assert!(allocator_warning("2026 WARN celld::memory: the allocator will not run a background thread error=`name` or `mib` specifies an unknown/invalid value."));
        assert!(!allocator_warning(
            "2026 WARN celld::memory: something else"
        ));
        assert!(!routine("warning: build issue"));
    }
}
