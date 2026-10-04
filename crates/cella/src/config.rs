//! Only extract routing identity. Native celld owns all Worker compatibility checks.
use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::path::{Path, PathBuf};

pub fn config_path(project: &Path) -> Result<PathBuf> {
    if project.is_file() {
        return Ok(project.canonicalize()?);
    }
    for name in ["wrangler.jsonc", "wrangler.json"] {
        let path = project.join(name);
        if path.is_file() {
            return Ok(path.canonicalize()?);
        }
    }
    bail!(
        "no wrangler.jsonc or wrangler.json in {}",
        project.display()
    )
}

pub fn validate_slug(slug: &str) -> Result<()> {
    if slug.is_empty()
        || slug.len() > 63
        || !slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        || !slug.as_bytes()[0].is_ascii_alphanumeric()
        || slug.ends_with('-')
    {
        bail!("invalid hosted slug: use 1–63 lowercase letters, digits or hyphens, starting and ending with a letter or digit");
    }
    Ok(())
}

pub fn slug(project: &Path, override_slug: Option<&str>) -> Result<String> {
    if let Some(value) = override_slug {
        validate_slug(value)?;
        return Ok(value.to_owned());
    }
    let path = config_path(project)?;
    name_from_jsonc(
        &std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?,
    )
}

pub fn parse_jsonc(source: &str) -> Result<Value> {
    serde_json::from_str(&strip_jsonc(source)?)
        .context("read Wrangler JSONC (native celld validates Worker configuration)")
}

pub fn read_project(project: &Path) -> Result<(PathBuf, Value)> {
    let path = config_path(project)?;
    if std::fs::metadata(&path)?.len() > 1024 * 1024 {
        bail!("Wrangler configuration exceeds the 1 MiB packaging limit");
    }
    let config = parse_jsonc(&std::fs::read_to_string(&path)?)?;
    Ok((path, config))
}

pub fn name_from_jsonc(source: &str) -> Result<String> {
    let config = parse_jsonc(source)?;
    let name = config.get("name").and_then(Value::as_str).context(
        "Wrangler name must be a string; use --slug to override hosted routing identity",
    )?;
    validate_slug(name)?;
    Ok(name.to_owned())
}

/// JSONC comments and trailing commas, not JSON5. Preserve string contents and line breaks.
fn strip_jsonc(source: &str) -> Result<String> {
    let mut out = String::with_capacity(source.len());
    let mut chars = source.chars().peekable();
    let (mut in_string, mut escaped) = (false, false);
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                out.push(c);
            }
            '/' if chars.peek() == Some(&'/') => {
                out.push(' ');
                for c in chars.by_ref() {
                    if c == '\n' {
                        out.push(c);
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                out.push(' ');
                let mut closed = false;
                while let Some(c) = chars.next() {
                    if c == '*' && chars.peek() == Some(&'/') {
                        chars.next();
                        closed = true;
                        break;
                    }
                    if c == '\n' {
                        out.push(c);
                    }
                }
                if !closed {
                    bail!("unterminated JSONC block comment");
                }
            }
            '}' | ']' => {
                let trimmed = out.trim_end().len();
                if out[..trimmed].ends_with(',') {
                    out.remove(trimmed - 1);
                }
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn jsonc_preserves_strings_and_ignores_unsupported_features() {
        let json = r#"{/* hi */ "name": "my-app", "url":"https://host/a//b", "unknown": [1,], // comment
        "escaped":"a\"/*b*/", }"#;
        assert_eq!(name_from_jsonc(json).unwrap(), "my-app");
    }
    #[test]
    fn rejects_bad_names_and_non_jsonc() {
        assert!(validate_slug("0abc").is_ok());
        for name in ["", "../x", "a/b", "UPPER", "-a", "a-", "a;true"] {
            assert!(validate_slug(name).is_err(), "{name}");
        }
        for json in [
            "{name:'app'}",
            "{\"name\": 4}",
            "{}",
            "{/*",
            "{\"name\":\"app\"} /*",
        ] {
            assert!(name_from_jsonc(json).is_err(), "{json}");
        }
    }
    #[test]
    fn override_does_not_rewrite_or_parse_config() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(slug(dir.path(), Some("override")).unwrap(), "override");
        assert!(slug(dir.path(), None).is_err());
    }
}
