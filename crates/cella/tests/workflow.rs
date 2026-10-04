//! Hermetic end-to-end fixtures: no SSH server, release network, or AWS account.
#![cfg(unix)]
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    bin: PathBuf,
    cache: PathBuf,
    requests: PathBuf,
}
fn executable(path: &Path, source: &str) {
    fs::write(path, source).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}
fn quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("project");
        let bin = temp.path().join("bin");
        let cache = temp.path().join("cache");
        let requests = temp.path().join("requests");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&bin).unwrap();
        fs::write(
            root.join("wrangler.jsonc"),
            "{/* leave native validation alone */\n\"name\":\"my-app\",\"unknown_feature\":true,}",
        )
        .unwrap();
        fs::write(root.join("pnpm-lock.yaml"), "").unwrap();
        fs::create_dir_all(root.join("node_modules/.bin")).unwrap();
        executable(
            &root.join("node_modules/.bin/esbuild"),
            "#!/bin/sh\nexit 0\n",
        );
        let fixture = Self {
            _temp: temp,
            root,
            bin,
            cache,
            requests,
        };
        fixture.ssh(false);
        fixture.native("1.2.3", "printf '%s\\n' '{\"worker\":\"my-app\",\"version\":\"native-version-id\",\"location\":\"s3://BUCKET/cells/my-app\",\"dry_run\":false}'");
        executable(
            &fixture.bin.join("curl"),
            "#!/bin/sh\necho 'unexpected network' >&2\nexit 98\n",
        );
        fixture
    }
    fn binary_path(&self) -> PathBuf {
        self.cache
            .join("releases/v1.2.3")
            .join(cella::release::target(std::env::consts::OS, std::env::consts::ARCH).unwrap())
            .join("celld")
    }
    fn native_source(&self, version: &str, body: &str) -> String {
        format!(
            r#"#!/bin/sh
if [ "$1" = --version ]; then echo 'celld {version}'; exit 0; fi
printf '%s\n' "$@" > {args}
printf '%s\n' "$CELLD_ESBUILD" > {esbuild}
if [ "$1" = deploy ] && [ "$AWS_SECRET_ACCESS_KEY" != 'local-only-secret' ]; then echo 'missing local credential' >&2; exit 97; fi
{body}
"#,
            args = quote(&self.root.join("native-args")),
            esbuild = quote(&self.root.join("esbuild-path"))
        )
    }
    fn native(&self, version: &str, body: &str) {
        let binary = self.binary_path();
        fs::create_dir_all(binary.parent().unwrap()).unwrap();
        executable(&binary, &self.native_source(version, body));
    }
    fn ssh(&self, fail: bool) {
        executable(
            &self.bin.join("ssh"),
            &format!(
                r#"#!/bin/sh
if [ -n "$AWS_SECRET_ACCESS_KEY$AWS_ACCESS_KEY_ID$AWS_SESSION_TOKEN$SSH_AUTH_SOCK" ]; then echo 'credentials leaked to SSH' >&2; exit 90; fi
printf '%s\n' "$@" >> {ssh_args}
request=$(cat)
printf '%s\n' "$request" >> {requests}
{failure}
case "$request" in
  *'"op":"provision"'*|*'"op":"target"'*) printf '%s\n' '{{"ok":true,"result":{{"slug":"my-app","bucket":"s3://BUCKET/cells/my-app","endpoint":"https://object.example.invalid","region":"auto","celld_version":"v1.2.3","enabled":false}}}}';;
  *'"op":"activate"'*) printf '%s\n' '{{"ok":true,"result":{{"enabled":true}}}}';;
  *'"op":"logs"'*) printf '%s\n' '{{"ok":true,"result":{{"text":"journal entry\n"}}}}';;
  *'"op":"deployments"'*) printf '%s\n' '{{"ok":true,"result":[{{"version_id":"native-version-id"}}]}}';;
  *) printf '%s\n' '{{"ok":true,"result":{{"slug":"my-app"}}}}';;
esac
"#,
                ssh_args = quote(&self.root.join("ssh-args")),
                requests = quote(&self.requests),
                failure = if fail {
                    "echo '{\"ok\":false,\"error\":\"permission denied by host\"}'; exit 1"
                } else {
                    ""
                }
            ),
        );
    }
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cella"));
        let path = std::env::join_paths(
            std::iter::once(self.bin.clone())
                .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
        )
        .unwrap();
        command
            .env_clear()
            .env("PATH", path)
            .env("HOME", self._temp.path())
            .env("CELLA_CACHE_DIR", &self.cache)
            .env("AWS_ACCESS_KEY_ID", "local-key")
            .env("AWS_SECRET_ACCESS_KEY", "local-only-secret")
            .env("AWS_SESSION_TOKEN", "local-session")
            .env("SSH_AUTH_SOCK", "/unused-agent")
            .arg("--project")
            .arg(&self.root)
            .arg("--host")
            .arg("cella-deploy@example.invalid")
            .arg("--identity")
            .arg(self._temp.path().join("deploy-key"));
        command
    }
    fn run(&self, args: &[&str]) -> Output {
        self.command().args(args).output().unwrap()
    }
    fn requests(&self) -> Vec<serde_json::Value> {
        fs::read_to_string(&self.requests)
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
}

#[test]
fn publish_uses_local_credentials_native_output_and_records_revision() {
    let f = Fixture::new();
    let output = f.run(&["deploy", "--source-revision", "commit-123"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let native: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(native["version"], "native-version-id");
    let requests = f.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0]["op"], "provision");
    assert_eq!(
        requests[1],
        serde_json::json!({"op":"activate","slug":"my-app","version_id":"native-version-id","source_revision":"commit-123"})
    );
    let args = fs::read_to_string(f.root.join("native-args")).unwrap();
    assert!(args.contains("--bucket\ns3://BUCKET/cells/my-app\n--endpoint\nhttps://object.example.invalid\n--region\nauto"));
    assert_eq!(
        fs::read_to_string(f.root.join("esbuild-path"))
            .unwrap()
            .trim(),
        f.root.join("node_modules/.bin/esbuild").to_str().unwrap()
    );
    let ssh_args = fs::read_to_string(f.root.join("ssh-args")).unwrap();
    assert!(ssh_args.contains("celld-ctl-transport"));
    assert!(!ssh_args.contains("native-version-id"));
}

#[test]
fn native_failure_preserves_errors_and_exit_code_without_activation() {
    let f = Fixture::new();
    f.native(
        "1.2.3",
        "echo 'native unsupported Wrangler option error exactly' >&2; exit 17",
    );
    let output = f.run(&["deploy"]);
    assert_eq!(output.status.code(), Some(17));
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("native unsupported Wrangler option error exactly\n"));
    assert_eq!(f.requests().len(), 1);
}

#[test]
fn version_mismatch_never_runs_deploy_or_downloads_newer() {
    let f = Fixture::new();
    f.native("1.2.4", "exit 0");
    let output = f.run(&["deploy"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("version mismatch"));
    assert!(!f.root.join("native-args").exists());
    assert_eq!(f.requests().len(), 1);
}

#[test]
fn host_failure_prevents_build_and_surfaces_error() {
    let f = Fixture::new();
    f.ssh(true);
    let output = f.run(&["deploy"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("permission denied by host"));
    assert!(!f.root.join("native-args").exists());
}

#[test]
fn malformed_or_dry_run_native_result_never_activates() {
    for body in [
        "echo invalid",
        "echo '{\"version\":\"id\",\"dry_run\":true}'",
        "echo '{\"version\":\"\",\"dry_run\":false}'",
    ] {
        let f = Fixture::new();
        f.native("1.2.3", body);
        assert!(!f.run(&["deploy"]).status.success());
        assert_eq!(f.requests().len(), 1);
    }
}

#[test]
fn explicit_dev_pin_needs_no_host_or_identity() {
    let f = Fixture::new();
    f.native("1.2.3", "echo local-dev");
    let output = f
        .command()
        .args(["dev", "--celld-version", "1.2.3", "--", "--no-watch"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(f.requests().is_empty());
    assert!(fs::read_to_string(f.root.join("native-args"))
        .unwrap()
        .contains("--no-watch"));
    // Also invoke without the helper's globally configured host/key.
    let output = Command::new(env!("CARGO_BIN_EXE_cella"))
        .env_clear()
        .env("CELLA_CACHE_DIR", &f.cache)
        .arg("--project")
        .arg(&f.root)
        .args(["dev", "--celld-version", "v1.2.3"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn read_only_commands_use_wrapped_transport() {
    let f = Fixture::new();
    assert!(f.run(&["status"]).status.success());
    let logs = f.run(&["logs", "--lines", "5"]);
    assert_eq!(String::from_utf8(logs.stdout).unwrap(), "journal entry\n");
    let history = f.run(&["deployments", "list"]);
    assert!(String::from_utf8_lossy(&history.stdout).contains("native-version-id"));
    assert_eq!(f.requests()[1]["lines"], 5);
    assert!(!f.run(&["logs", "--lines", "1001"]).status.success());
}

#[test]
fn failed_download_is_not_cached_and_does_not_activate() {
    let f = Fixture::new();
    fs::remove_file(f.binary_path()).unwrap();
    let output = f.run(&["deploy"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("no newer version will be selected"));
    assert!(!f.binary_path().exists());
    assert_eq!(f.requests().len(), 1);
}

#[test]
fn downloads_actual_gzip_asset_atomically_then_checks_version() {
    use std::io::Write;
    for version in ["1.2.3", "1.2.4"] {
        let f = Fixture::new();
        fs::remove_file(f.binary_path()).unwrap();
        let archive = f._temp.path().join("release.gz");
        let mut gzip = flate2::write::GzEncoder::new(
            fs::File::create(&archive).unwrap(),
            flate2::Compression::default(),
        );
        gzip.write_all(
            f.native_source(version, "echo '{\"version\":\"new-id\",\"dry_run\":false}'")
                .as_bytes(),
        )
        .unwrap();
        gzip.finish().unwrap();
        executable(&f.bin.join("curl"), &format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > {}\nwhile [ $# -gt 0 ]; do if [ \"$1\" = --output ]; then cp {} \"$2\"; exit; fi; shift; done\nexit 1\n", quote(&f.root.join("curl-args")), quote(&archive)));
        let output = f.run(&["deploy"]);
        assert_eq!(
            output.status.success(),
            version == "1.2.3",
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(f.binary_path().exists(), version == "1.2.3");
        let args = fs::read_to_string(f.root.join("curl-args")).unwrap();
        assert!(args.contains("/releases/download/v1.2.3/celld-"));
        assert!(!args.contains("latest"));
    }
}

#[test]
fn oversized_transport_response_is_killed_and_rejected() {
    let f = Fixture::new();
    executable(
        &f.bin.join("ssh"),
        "#!/bin/sh\ncat >/dev/null\nexec head -c 2000000 /dev/zero\n",
    );
    let output = f.run(&["status"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("response exceeds 1048576 bytes"));
}

#[test]
fn invalid_revision_is_rejected_before_provision_or_publish() {
    let f = Fixture::new();
    let output = f.run(&["deploy", "--source-revision", "bad;revision"]);
    assert!(!output.status.success());
    assert!(f.requests().is_empty());
    assert!(!f.root.join("native-args").exists());
}

#[test]
fn activation_failure_reports_already_published_version() {
    let f = Fixture::new();
    let ssh = f.bin.join("ssh");
    let source = fs::read_to_string(&ssh).unwrap().replace(
        "printf '%s\\n' '{\"ok\":true,\"result\":{\"enabled\":true}}'",
        "echo '{\"ok\":false,\"error\":\"readiness failed\"}'; exit 1",
    );
    executable(&ssh, &source);
    let output = f.run(&["deploy", "--source-revision", "abc123"]);
    assert!(!output.status.success());
    assert_eq!(f.requests().len(), 2);
    assert!(String::from_utf8_lossy(&output.stdout).contains("native-version-id"));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("was published, but host activation failed"));
    assert!(stderr.contains("readiness failed"));
}

#[test]
fn dev_without_pin_reads_target_but_never_provisions() {
    let f = Fixture::new();
    f.native("1.2.3", "echo local-dev");
    let output = f.run(&["dev"]);
    assert!(output.status.success());
    assert_eq!(
        f.requests(),
        vec![serde_json::json!({"op":"target","slug":"my-app"})]
    );
}

#[test]
fn yarn_pnp_esbuild_wrapper_preserves_arguments_without_network() {
    let f = Fixture::new();
    fs::remove_file(f.root.join("pnpm-lock.yaml")).unwrap();
    fs::remove_file(f.root.join("node_modules/.bin/esbuild")).unwrap();
    fs::write(f.root.join("yarn.lock"), "").unwrap();
    fs::write(f.root.join(".pnp.cjs"), "").unwrap();
    executable(&f.bin.join("yarn"), &format!("#!/bin/sh\ntest \"$COREPACK_ENABLE_NETWORK\" = 0 || exit 92\nprintf '%s\\n' \"$@\" > {}\n", quote(&f.root.join("yarn-args"))));
    f.native("1.2.3", "\"$CELLD_ESBUILD\" 'entry with spaces.ts' --bundle || exit $?; echo '{\"version\":\"id\",\"dry_run\":false}'");
    let output = f.run(&["deploy"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(f.root.join("yarn-args")).unwrap(),
        "exec\nesbuild\nentry with spaces.ts\n--bundle\n"
    );
    let wrapper = fs::read_to_string(f.root.join("esbuild-path")).unwrap();
    assert!(
        !Path::new(wrapper.trim()).exists(),
        "temporary bridge should be removed"
    );
}
