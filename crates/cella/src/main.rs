use anyhow::{bail, Context, Result};
use cella::{
    config, release, toolchain,
    transport::{Request, Ssh},
};
use clap::{Parser, Subcommand};
use serde::Deserialize;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Parser)]
#[command(
    version,
    about = "Local celld development and restricted SSH deployments"
)]
struct Cli {
    /// Wrangler project directory or configuration file (never rewritten).
    #[arg(long, global = true, default_value = ".")]
    project: PathBuf,
    /// Override hosted routing slug, not the native Worker name.
    #[arg(long, global = true)]
    slug: Option<String>,
    /// Explicit SSH destination, normally cella-deploy@HOST (no ssh_config aliases).
    #[arg(long, global = true, env = "CELLA_HOST")]
    host: Option<String>,
    /// Dedicated restricted deploy private key; not read from Wrangler config.
    #[arg(long, global = true, env = "CELLA_SSH_KEY")]
    identity: Option<PathBuf>,
    #[arg(long, global = true, env = "CELLA_SSH_PORT", default_value_t = 22)]
    ssh_port: u16,
    #[command(subcommand)]
    command: Action,
}

#[derive(Subcommand)]
enum Action {
    /// Run local native celld dev. No SSH needed when a release is explicitly pinned.
    Dev {
        #[arg(long, env = "CELLA_CELLD_VERSION")]
        celld_version: Option<String>,
        /// Additional native dev arguments, after --.
        #[arg(last = true)]
        native_args: Vec<String>,
    },
    /// Provision, publish locally with native celld, then activate the target.
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
                .context("set --identity or CELLA_SSH_KEY to a dedicated restricted SSH key")?,
            port: self.ssh_port,
        })
    }
    fn slug(&self) -> Result<String> {
        config::slug(&self.project, self.slug.as_deref())
    }
}

fn project_root(project: &Path) -> Result<PathBuf> {
    let path = project.canonicalize().context("resolve project path")?;
    if path.is_file() {
        Ok(path
            .parent()
            .context("project config has no parent")?
            .to_path_buf())
    } else {
        Ok(path)
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
        Action::Dev {
            celld_version,
            native_args,
        } => {
            let version = match celld_version {
                Some(version) => version.clone(),
                None => {
                    let slug = cli.slug()?;
                    cli.ssh().context("dev needs --celld-version/CELLA_CELLD_VERSION, or an SSH target to obtain its exact pin")?
                        .target(&Request::Target { slug: &slug }, &slug)?.celld_version
                }
            };
            let binary = release::ensure(&version, &release::cache_root()?)?;
            let root = project_root(&cli.project)?;
            let mut command = Command::new(binary);
            command
                .arg("dev")
                .arg(cli.project.canonicalize()?)
                .args(native_args)
                .current_dir(&root);
            let _tools = toolchain::configure(&mut command, &root)?;
            Ok(command
                .status()
                .context("run native celld dev")?
                .code()
                .unwrap_or(1))
        }
        Action::Deploy {
            source_revision: explicit_revision,
        } => {
            if let Some(revision) = explicit_revision {
                cella::transport::validate_source_revision(revision)?;
            }
            let slug = cli.slug()?;
            let ssh = cli.ssh()?;
            let target = ssh.target(&Request::Provision { slug: &slug }, &slug)?;
            let binary = release::ensure(&target.celld_version, &release::cache_root()?)?;
            let root = project_root(&cli.project)?;
            let revision = explicit_revision.clone().or_else(|| source_revision(&root));
            if revision.is_none() {
                eprintln!("No Git revision available; recording source_revision=null (use --source-revision in CI)");
            }
            let mut command = Command::new(binary);
            command
                .arg("deploy")
                .arg(cli.project.canonicalize()?)
                .arg("--json")
                .arg("--bucket")
                .arg(&target.bucket)
                .arg("--endpoint")
                .arg(&target.endpoint)
                .arg("--region")
                .arg(&target.region)
                .current_dir(&root)
                .stdin(Stdio::null())
                .stderr(Stdio::inherit())
                .stdout(Stdio::piped());
            // Standard AWS credentials remain solely in this local child's environment.
            let _tools = toolchain::configure(&mut command, &root)?;
            let output = command.output().context("run native celld deploy")?;
            std::io::stdout().write_all(&output.stdout)?;
            std::io::stdout().flush()?;
            if !output.status.success() {
                return Ok(output.status.code().unwrap_or(1));
            }
            let deployment: NativeDeployment = serde_json::from_slice(&output.stdout).context(
                "native publish returned invalid deployment JSON; host was not activated",
            )?;
            if deployment.dry_run || deployment.version.is_empty() {
                bail!("native deploy did not return a published version; host was not activated");
            }
            ssh.request(&Request::Activate { slug: &slug, version_id: &deployment.version, source_revision: revision.as_deref() })
                .with_context(|| format!("version {} was published, but host activation failed; check cella status/logs before retrying (the R2 pointer may already be adopted)", deployment.version))?;
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
                Action::Status => Request::Status { slug: &slug },
                Action::Logs { lines } => Request::Logs { slug: &slug, lines },
                Action::Deployments { .. } => Request::Deployments { slug: &slug },
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
    match run(Cli::parse()) {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("cella: {error:#}");
            std::process::exit(1);
        }
    }
}
