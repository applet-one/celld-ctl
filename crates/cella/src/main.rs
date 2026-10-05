use anyhow::{bail, Context, Result};
use cella::{
    bundle, config, init, release, toolchain,
    transport::{Request, Ssh},
};
use clap::{Parser, Subcommand};
use serde::Deserialize;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Parser)]
#[command(version, about = "Owner SSH deployments for celld applications")]
struct Cli {
    /// Wrangler project directory or config file; base directory for init.
    #[arg(long, global = true, default_value = ".")]
    project: PathBuf,
    /// Override hosted routing slug, not the native Worker name.
    #[arg(long, global = true)]
    slug: Option<String>,
    /// Owner SSH destination, e.g. owner@host or vm+NAME@vm.exe.xyz (no aliases).
    #[arg(long, global = true, env = "CELLA_HOST")]
    host: Option<String>,
    /// Explicit private key authorized for the owner SSH account.
    #[arg(long, global = true, env = "CELLA_SSH_KEY")]
    identity: Option<PathBuf>,
    #[arg(long, global = true, env = "CELLA_SSH_PORT", default_value_t = 22)]
    ssh_port: u16,
    #[command(subcommand)]
    command: Action,
}

#[derive(Subcommand)]
enum Action {
    /// Create a starter Worker project; use . to scaffold the current directory.
    Init {
        /// New project directory, or . (the folder name becomes the app name).
        directory: PathBuf,
    },
    /// Build locally without storage credentials; upload over SSH for host publication.
    Deploy {
        /// Source label; defaults to Git HEAD, suffixed -dirty for local changes.
        #[arg(long)]
        source_revision: Option<String>,
    },
    Deployments {
        #[command(subcommand)]
        command: Deployments,
    },
    /// Read bounded recent host logs (not a streaming SSH session).
    Logs {
        #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u16).range(1..=1000))]
        lines: u16,
    },
    Status,
}

#[derive(Subcommand)]
enum Deployments {
    List,
}

impl Cli {
    fn ssh(&self) -> Result<Ssh> {
        Ok(Ssh {
            host: self
                .host
                .clone()
                .context("set --host or CELLA_HOST outside wrangler.jsonc")?,
            identity: self
                .identity
                .clone()
                .context("set --identity or CELLA_SSH_KEY to your owner SSH private key")?,
            port: self.ssh_port,
        })
    }
    fn slug(&self) -> Result<String> {
        config::slug(&self.project, self.slug.as_deref())
    }
}

fn source_revision(root: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--verify", "HEAD"])
        .current_dir(root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let revision = String::from_utf8(output.stdout).ok()?.trim().to_owned();
    let status = Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=normal"])
        .current_dir(root)
        .output()
        .ok()?;
    if !status.status.success() {
        return None;
    }
    Some(if status.stdout.is_empty() {
        revision
    } else {
        format!("{revision}-dirty")
    })
}

#[derive(Deserialize)]
struct NativeDeployment {
    version: String,
    dry_run: bool,
}

fn run(cli: Cli) -> Result<i32> {
    match &cli.command {
        Action::Init { directory } => {
            let target = init::create(&cli.project, directory)?;
            println!(
                "Created {} in {}",
                target.file_name().unwrap().to_string_lossy(),
                target.display()
            );
            if directory == Path::new(".") {
                println!("Next: pnpm install && cella deploy");
            } else {
                println!(
                    "Next: cd '{}' && pnpm install && cella deploy",
                    target.display().to_string().replace('\'', "'\\''")
                );
            }
            Ok(0)
        }
        Action::Deploy {
            source_revision: explicit_revision,
        } => {
            if let Some(revision) = explicit_revision {
                cella::transport::validate_source_revision(revision)?;
            }
            let slug = cli.slug()?;
            let ssh = cli.ssh()?;
            let target = ssh.target(&Request::Provision { slug: slug.clone() }, &slug)?;
            let binary = release::ensure(&target.celld_version, &release::cache_root()?)?;
            let (config_path, original_config) = config::read_project(&cli.project)?;
            bundle::check_scope(&original_config)?;
            let root = config_path
                .parent()
                .context("project config has no directory")?;
            let revision = explicit_revision.clone().or_else(|| source_revision(root));
            if revision.is_none() {
                eprintln!("No Git revision available; recording source_revision=null (use --source-revision in CI)");
            }
            let capture = tempfile::tempdir().context("create bounded native build capture")?;
            let mut command = Command::new(binary);
            command
                .arg("deploy")
                .arg(&config_path)
                .arg("--json")
                .arg("--dry-run")
                .arg("--bucket")
                .arg("s3://cella-build")
                .current_dir(root)
                .stdin(Stdio::null())
                .stderr(Stdio::inherit())
                .stdout(Stdio::piped());
            let _tools = toolchain::configure(&mut command, root)?;
            bundle::configure_capture(&mut command, capture.path())?;
            bundle::clear_storage_environment(&mut command);
            // Native build runs before bucket-client initialization on --dry-run.
            // No local R2 credentials, profile, endpoint or real bucket is needed.
            let output = command.output().context("run native celld dry-run build")?;
            if !output.status.success() {
                std::io::stderr().write_all(&output.stdout)?;
                return Ok(output.status.code().unwrap_or(1));
            }
            let deployment: NativeDeployment = serde_json::from_slice(&output.stdout)
                .context("native dry-run returned invalid deployment JSON; no upload was sent")?;
            if !deployment.dry_run || !celld_ctl_core::valid_deployment_id(&deployment.version) {
                bail!("native build did not return a dry-run version; no upload was sent");
            }
            let payload = bundle::prepare(original_config, root, capture.path())?;
            let pin = release::version_tag(&target.celld_version)?;
            let result = ssh
                .deploy(
                    &Request::Deploy {
                        slug: slug.clone(),
                        celld_version: pin[1..].to_owned(),
                        version_id: deployment.version.clone(),
                        source_revision: revision.clone(),
                        bundle_size: payload.len(),
                    },
                    &payload,
                )
                .context(
                    "host publication/activation failed; inspect status/logs before retrying",
                )?;
            let native_output = result
                .get("native_output")
                .context("host publish response has no native_output")?;
            let native_stderr = result
                .get("native_stderr")
                .and_then(|v| v.as_str())
                .context("host publish response has no native_stderr")?;
            std::io::stderr().write_all(native_stderr.as_bytes())?;
            if result.get("version_id").and_then(|v| v.as_str())
                != Some(deployment.version.as_str())
                || native_output.get("version").and_then(|v| v.as_str())
                    != Some(deployment.version.as_str())
                || native_output.get("dry_run").and_then(|v| v.as_bool()) != Some(false)
            {
                bail!("host returned an unexpected publication version or dry-run result");
            }
            println!("{}", serde_json::to_string(native_output)?);
            eprintln!(
                "Activated {slug}: {} (source {})",
                deployment.version,
                revision.as_deref().unwrap_or("unknown")
            );
            Ok(0)
        }
        Action::Status | Action::Logs { .. } | Action::Deployments { .. } => {
            let slug = cli.slug()?;
            let request = match cli.command {
                Action::Status => Request::Status { slug: slug.clone() },
                Action::Logs { lines } => Request::Logs {
                    slug: slug.clone(),
                    lines: lines.into(),
                },
                Action::Deployments { .. } => Request::Deployments { slug: slug.clone() },
                _ => unreachable!(),
            };
            let value = cli.ssh()?.request(&request)?;
            if matches!(request, Request::Logs { .. }) {
                let text = value
                    .get("text")
                    .and_then(|v| v.as_str())
                    .context("invalid logs response: missing text")?;
                print!("{text}");
            } else {
                println!("{}", serde_json::to_string_pretty(&value)?);
            }
            Ok(0)
        }
    }
}

fn main() {
    let result = if std::env::var_os(bundle::CAPTURE_ENV).is_some() {
        bundle::capture_esbuild()
    } else {
        run(Cli::parse())
    };
    match result {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("cella: {error:#}");
            std::process::exit(1);
        }
    }
}
