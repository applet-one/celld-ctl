//! Native publishing runs as a non-login, unprivileged reader of an immutable
//! staging tree. Uploaded JS/Wasm is data: no_bundle is forced, Python/container
//! entry points are rejected, and no client path ever reaches the host filesystem.
use crate::{
    config::{parse_credentials, Paths},
    registry::App,
};
use anyhow::{bail, ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use celld_ctl_core::PreparedBundle;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    os::{
        fd::AsRawFd,
        unix::{
            fs::{OpenOptionsExt, PermissionsExt},
            process::CommandExt,
        },
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

const OUTPUT_LIMIT: usize = 64 * 1024;
#[derive(Debug)]
pub struct NativePublish {
    pub native_output: Value,
    pub native_stderr: String,
}
#[derive(Clone, Copy)]
struct Identity {
    uid: libc::uid_t,
    gid: libc::gid_t,
    drop_privileges: bool,
}
impl Identity {
    fn resolve(paths: &Paths) -> Result<Self> {
        if !paths.strict {
            // Explicit operator fixture injection only; never selected by transport.
            return Ok(Self {
                uid: unsafe { libc::geteuid() },
                gid: unsafe { libc::getegid() },
                drop_privileges: false,
            });
        }
        let mut pwd = std::mem::MaybeUninit::<libc::passwd>::uninit();
        let mut found = std::ptr::null_mut();
        let mut buffer = vec![0u8; 16384];
        // SAFETY: writable passwd/buffer storage remains alive throughout the lookup.
        let status = unsafe {
            libc::getpwnam_r(
                c"celld-publish".as_ptr(),
                pwd.as_mut_ptr(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut found,
            )
        };
        ensure!(
            status == 0 && !found.is_null(),
            "install the dedicated celld-publish system account before publishing"
        );
        let pwd = unsafe { pwd.assume_init() };
        ensure!(
            pwd.pw_uid != 0 && pwd.pw_gid != 0,
            "celld-publish must be unprivileged"
        );
        Ok(Self {
            uid: pwd.pw_uid,
            gid: pwd.pw_gid,
            drop_privileges: true,
        })
    }
    fn mode(&self, path: &Path, mode: u32) -> Result<()> {
        let file = fs::File::open(path)?;
        if self.drop_privileges {
            // SAFETY: fd references a root-created file/dir; no user-owned links exist.
            ensure!(
                unsafe { libc::fchown(file.as_raw_fd(), 0, self.gid) } == 0,
                "set publisher staging group"
            );
        }
        file.set_permissions(fs::Permissions::from_mode(mode))?;
        Ok(())
    }
}

struct Stage {
    _dir: tempfile::TempDir,
    config: PathBuf,
    identity: Identity,
}
impl Stage {
    fn create(paths: &Paths, bundle: &PreparedBundle) -> Result<Self> {
        let identity = Identity::resolve(paths)?;
        let parent = paths.state.join("staging");
        paths.make_dir(&parent, 0o700)?;
        identity.mode(&parent, 0o710)?;
        let dir = tempfile::Builder::new()
            .prefix("publish-")
            .tempdir_in(&parent)?;
        identity.mode(dir.path(), 0o750)?;
        for (kind, files) in [("modules", &bundle.modules), ("assets", &bundle.assets)] {
            let base = dir.path().join(kind);
            fs::create_dir(&base)?;
            identity.mode(&base, 0o750)?;
            for upload in files {
                let output = base.join(&upload.path);
                let relative_parent = Path::new(&upload.path)
                    .parent()
                    .context("invalid staged path")?;
                let mut ancestor = base.clone();
                for component in relative_parent.components() {
                    ancestor.push(component);
                    if !ancestor.exists() {
                        fs::create_dir(&ancestor)?;
                        identity.mode(&ancestor, 0o750)?;
                    }
                }
                let bytes = STANDARD
                    .decode(&upload.content)
                    .context("invalid staged base64")?;
                write_staged(&output, &bytes, identity)?;
            }
        }
        let config = dir.path().join("wrangler.json");
        write_staged(&config, &serde_json::to_vec(&bundle.config)?, identity)?;
        Ok(Self {
            _dir: dir,
            config,
            identity,
        })
    }
}
fn write_staged(path: &Path, bytes: &[u8], identity: Identity) -> Result<()> {
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    identity.mode(path, 0o640)
}

/// Redact current node credentials from every successful/error native output.
pub fn redact(text: &str, credentials: &BTreeMap<String, String>) -> String {
    let mut output = text.to_owned();
    let mut secrets = credentials.values().collect::<Vec<_>>();
    secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
    for value in secrets {
        if !value.is_empty() {
            output = output.replace(value, "[REDACTED]");
        }
    }
    output
}
struct Captured {
    success: bool,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}
fn capture(mut command: Command, identity: Identity, timeout: Duration) -> Result<Captured> {
    let parent = unsafe { libc::getpid() };
    // SAFETY: the closure executes only async-signal-safe syscalls, with no Rust
    // allocation/locks. Drop supplementary groups BEFORE dropping uid. setuid may
    // clear PDEATHSIG, so install it after the irreversible privilege transition.
    unsafe {
        command.pre_exec(move || {
            if identity.drop_privileges
                && (libc::setgroups(0, std::ptr::null()) != 0
                    || libc::setgid(identity.gid) != 0
                    || libc::setuid(identity.uid) != 0)
            {
                return Err(std::io::Error::last_os_error());
            }
            if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0
                || libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0
            {
                return Err(std::io::Error::last_os_error());
            }
            if libc::getppid() != parent {
                libc::_exit(125);
            }
            Ok(())
        });
    }
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("start pinned native publisher")?;
    let result = (|| {
        let mut out = child.stdout.take().context("missing native stdout")?;
        let mut err = child.stderr.take().context("missing native stderr")?;
        for fd in [out.as_raw_fd(), err.as_raw_fd()] {
            ensure!(
                unsafe { libc::fcntl(fd, libc::F_SETFL, libc::O_NONBLOCK) } != -1,
                "set native output nonblocking"
            );
        }
        let start = Instant::now();
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        loop {
            drain(&mut out, &mut stdout)?;
            drain(&mut err, &mut stderr)?;
            if let Some(status) = child.try_wait()? {
                drain(&mut out, &mut stdout)?;
                drain(&mut err, &mut stderr)?;
                return Ok(Captured {
                    success: status.success(),
                    stdout,
                    stderr,
                });
            }
            ensure!(start.elapsed() < timeout, "native publisher timed out");
            std::thread::sleep(Duration::from_millis(10));
        }
    })();
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}
fn drain(pipe: &mut impl Read, bytes: &mut Vec<u8>) -> Result<()> {
    let mut buffer = [0u8; 8192];
    loop {
        match pipe.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(n) => {
                bytes.extend_from_slice(&buffer[..n]);
                ensure!(
                    bytes.len() <= OUTPUT_LIMIT,
                    "native output exceeds 64 KiB limit"
                );
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        }
    }
}
fn native_command(
    paths: &Paths,
    app: &App,
    stage: &Stage,
    credentials: Option<&BTreeMap<String, String>>,
) -> Command {
    let mut command = Command::new(
        paths
            .releases
            .join(format!("v{}/celld", app.target.celld_version)),
    );
    command
        .env_clear()
        .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
        .env("HOME", "/var/empty/celld-publish")
        .env("LC_ALL", "C")
        .env("NO_COLOR", "1")
        .current_dir(stage.config.parent().expect("fixed stage config parent"))
        .arg("deploy")
        .arg("--config")
        .arg(&stage.config)
        .arg("--bucket")
        .arg(&app.target.bucket)
        .arg("--endpoint")
        .arg(&app.target.endpoint)
        .arg("--region")
        .arg(&app.target.region)
        .arg("--json");
    if let Some(credentials) = credentials {
        for (key, value) in credentials {
            command.env(key, value);
        }
        command.env("AWS_REGION", &app.target.region);
    } else {
        command.arg("--dry-run");
    }
    command
}
fn parse_native(
    output: &Captured,
    phase: &str,
    credentials: &BTreeMap<String, String>,
    expected: &str,
    worker: &str,
    dry_run: bool,
) -> Result<(Value, String)> {
    let stdout = redact(&String::from_utf8_lossy(&output.stdout), credentials);
    let stderr = redact(&String::from_utf8_lossy(&output.stderr), credentials);
    if !output.success {
        bail!(
            "native {phase} failed: {}",
            if stderr.is_empty() { &stdout } else { &stderr }
        );
    }
    let value: Value =
        serde_json::from_str(&stdout).context("native publisher returned invalid JSON")?;
    ensure!(
        value.get("version").and_then(Value::as_str) == Some(expected),
        "native {phase} version differs from the expected prepared bundle"
    );
    ensure!(
        value.get("worker").and_then(Value::as_str) == Some(worker),
        "native {phase} worker differs from the staged configuration"
    );
    ensure!(
        value.get("dry_run").and_then(Value::as_bool) == Some(dry_run),
        "native {phase} returned an unexpected publish state"
    );
    Ok((value, stderr))
}
pub fn deploy(
    paths: &Paths,
    app: &App,
    bundle: &PreparedBundle,
    expected: &str,
) -> Result<NativePublish> {
    let bundle = bundle.normalize().map_err(anyhow::Error::msg)?;
    let stage = Stage::create(paths, &bundle)?;
    let credentials = parse_credentials(paths)?;
    let worker = bundle
        .config
        .get("name")
        .and_then(Value::as_str)
        .context("config name must be a string")?;
    let started = Instant::now();
    // The dry run does not get AWS credentials and returns before native bucket I/O.
    let check = capture(
        native_command(paths, app, &stage, None),
        stage.identity,
        Duration::from_secs(45),
    )?;
    let (_, dry_stderr) = parse_native(&check, "dry-run", &credentials, expected, worker, true)?;
    let remaining = Duration::from_secs(90).saturating_sub(started.elapsed());
    ensure!(!remaining.is_zero(), "native publisher timed out");
    // Stage files are immutable to this uid, so the publish reads the exact bytes
    // whose native version was verified. No esbuild/docker/Python can run here.
    let publish = capture(
        native_command(paths, app, &stage, Some(&credentials)),
        stage.identity,
        remaining,
    )
    .map_err(|e|anyhow::anyhow!("native publish may have completed for version {expected}: {e}; check deployments/status or retry this exact bundle"))?;
    let (native_output,native_stderr)=parse_native(&publish,"publish",&credentials,expected,worker,false)
        .map_err(|e|anyhow::anyhow!("publication may have completed for version {expected}: {e}; check deployments/status or retry this exact bundle"))?;
    let combined = format!("{dry_stderr}{native_stderr}");
    ensure!(
        combined.len() <= 2 * OUTPUT_LIMIT,
        "native version {expected} was published but redacted stderr exceeds limit; retry this exact bundle to activate"
    );
    Ok(NativePublish {
        native_output,
        native_stderr: combined,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use celld_ctl_core::{Target, UploadFile};
    use serde_json::json;
    struct Fixture {
        dir: tempfile::TempDir,
        paths: Paths,
        app: App,
        bundle: PreparedBundle,
    }
    impl Fixture {
        fn new(script: &str) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let paths = Paths::under(dir.path());
            for path in [
                &paths.state,
                paths.credentials.parent().unwrap(),
                &paths.releases.join("v0.6.1"),
            ] {
                fs::create_dir_all(path).unwrap();
            }
            fs::write(&paths.credentials,"AWS_ACCESS_KEY_ID=FAKEACCESSKEY\nAWS_SECRET_ACCESS_KEY=FAKESECRETKEY\nAWS_SESSION_TOKEN=FAKESESSIONTOKEN\n").unwrap();
            let binary = paths.releases.join("v0.6.1/celld");
            fs::write(&binary, format!("#!/bin/sh\n{script}\n")).unwrap();
            fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
            let app = App {
                target: Target {
                    slug: "app".into(),
                    bucket: "s3://fixture-bucket/cells/app".into(),
                    endpoint: "https://store.invalid".into(),
                    region: "auto".into(),
                    celld_version: "0.6.1".into(),
                    enabled: false,
                },
                port: 8101,
                internal_port: 18101,
                unit: "celld-cell@app.service".into(),
                legacy: false,
                version_id: None,
            };
            let bundle = PreparedBundle {
                config: json!({"name":"worker","main":"/etc/no-host-path.js","no_bundle":false,"assets":{"directory":"/etc"}}),
                modules: vec![UploadFile {
                    path: "index.js".into(),
                    content: STANDARD
                        .encode(b"throw new Error('never executed by the publisher');"),
                }],
                assets: vec![UploadFile {
                    path: "nested/asset.txt".into(),
                    content: STANDARD.encode(b"asset data"),
                }],
            };
            Self {
                dir,
                paths,
                app,
                bundle,
            }
        }
    }
    const NATIVE: &str = r#"
[ "$1" = deploy ] || exit 10
[ -z "${CELLD_ESBUILD+x}" ] || exit 11
[ -z "${CELLD_DOCKER+x}" ] || exit 12
[ -z "${AWS_SHARED_CREDENTIALS_FILE+x}" ] || exit 13
[ "$HOME" = /var/empty/celld-publish ] || exit 14
dry=false
for arg in "$@"; do [ "$arg" != --dry-run ] || dry=true; done
if [ "$dry" = true ]; then
  [ -z "${AWS_ACCESS_KEY_ID+x}" ] || exit 15
  [ -z "${AWS_SECRET_ACCESS_KEY+x}" ] || exit 16
  [ -z "${AWS_SESSION_TOKEN+x}" ] || exit 17
  printf '{"version":"expected","worker":"worker","dry_run":true}'
else
  [ "$AWS_ACCESS_KEY_ID" = FAKEACCESSKEY ] || exit 18
  [ "$AWS_SECRET_ACCESS_KEY" = FAKESECRETKEY ] || exit 19
  [ "$AWS_SESSION_TOKEN" = FAKESESSIONTOKEN ] || exit 20
  [ "$AWS_REGION" = auto ] || exit 21
  printf 'native warning %s %s %s' "$AWS_ACCESS_KEY_ID" "$AWS_SECRET_ACCESS_KEY" "$AWS_SESSION_TOKEN" >&2
  printf '{"version":"expected","worker":"worker","dry_run":false}'
fi
"#;
    #[test]
    fn dry_run_is_credential_free_publish_is_scoped_and_output_is_redacted() {
        let f = Fixture::new(NATIVE);
        let result = deploy(&f.paths, &f.app, &f.bundle, "expected").unwrap();
        assert_eq!(result.native_output["version"], "expected");
        assert!(result.native_stderr.contains("[REDACTED]"));
        assert!(!result.native_stderr.contains("FAKE"));
        assert_eq!(
            fs::read_dir(f.paths.state.join("staging")).unwrap().count(),
            0
        );
    }
    #[test]
    fn immutable_stage_rewrites_host_paths_and_is_removed_by_raii() {
        let f = Fixture::new(NATIVE);
        let normalized = f.bundle.normalize().unwrap();
        let stage = Stage::create(&f.paths, &normalized).unwrap();
        let root = stage.config.parent().unwrap().to_owned();
        let config: Value = serde_json::from_slice(&fs::read(&stage.config).unwrap()).unwrap();
        assert_eq!(config["main"], "modules/index.js");
        assert_eq!(config["assets"]["directory"], "assets");
        assert_eq!(config["no_bundle"], true);
        for path in [&root, &root.join("modules"), &root.join("assets/nested")] {
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o750
            );
        }
        for path in [
            &stage.config,
            &root.join("modules/index.js"),
            &root.join("assets/nested/asset.txt"),
        ] {
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o640
            );
            assert!(fs::symlink_metadata(path).unwrap().is_file());
        }
        assert_eq!(
            fs::metadata(f.paths.state.join("staging"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o710
        );
        drop(stage);
        assert!(!root.exists());
    }
    #[test]
    fn native_version_mismatch_cannot_publish() {
        let f = Fixture::new(NATIVE);
        let marker = f.dir.path().join("should-not-publish");
        let script = NATIVE.replace(
            "else\n",
            &format!("else\n  printf bad > '{}'\n", marker.display()),
        );
        fs::write(
            f.paths.releases.join("v0.6.1/celld"),
            format!("#!/bin/sh\n{script}"),
        )
        .unwrap();
        let error = deploy(&f.paths, &f.app, &f.bundle, "different-version")
            .unwrap_err()
            .to_string();
        assert!(error.contains("version differs"));
        assert!(!marker.exists());
        assert_eq!(
            fs::read_dir(f.paths.state.join("staging")).unwrap().count(),
            0
        );
    }
    #[test]
    fn native_parser_failure_preserves_diagnostics_without_publishing() {
        let f = Fixture::new("printf 'unsupported native config field' >&2; exit 2");
        let error = deploy(&f.paths, &f.app, &f.bundle, "expected")
            .unwrap_err()
            .to_string();
        assert!(error.contains("unsupported native config field"));
        assert_eq!(
            fs::read_dir(f.paths.state.join("staging")).unwrap().count(),
            0
        );
    }
    #[test]
    fn simultaneous_pipes_are_bounded_without_deadlock() {
        let f=Fixture::new("while :; do printf '0123456789012345678901234567890123456789012345678901234567890123456789'; printf '0123456789012345678901234567890123456789012345678901234567890123456789' >&2; done");
        let command = Command::new(f.paths.releases.join("v0.6.1/celld"));
        let result = capture(
            command,
            Identity::resolve(&f.paths).unwrap(),
            Duration::from_secs(3),
        );
        assert!(result.err().unwrap().to_string().contains("64 KiB"));
    }
    #[test]
    fn subprocess_timeout_is_enforced_and_reaped() {
        let f = Fixture::new("while :; do :; done");
        let command = Command::new(f.paths.releases.join("v0.6.1/celld"));
        let start = Instant::now();
        let result = capture(
            command,
            Identity::resolve(&f.paths).unwrap(),
            Duration::from_millis(50),
        );
        assert!(result.err().unwrap().to_string().contains("timed out"));
        assert!(start.elapsed() < Duration::from_secs(2));
    }
    #[test]
    fn env_and_argv_cannot_be_overridden_by_bundle_fields() {
        let f = Fixture::new(NATIVE);
        let stage = Stage::create(&f.paths, &f.bundle.normalize().unwrap()).unwrap();
        let command = native_command(&f.paths, &f.app, &stage, None);
        let args = command
            .get_args()
            .map(|v| v.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(args.contains(&f.app.target.bucket));
        assert!(args.contains(&f.app.target.endpoint));
        assert!(args.contains(&"--dry-run".into()));
        assert!(!args.contains(&"/etc/no-host-path.js".into()));
        let vars = command
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect::<BTreeMap<_, _>>();
        assert!(!vars.contains_key("AWS_ACCESS_KEY_ID"));
        assert!(!vars.contains_key("CELLD_ESBUILD"));
        assert!(!vars.contains_key("CELLD_DOCKER"));
    }
}
