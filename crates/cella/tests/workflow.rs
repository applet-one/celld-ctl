//! Hermetic end-to-end fixtures: no SSH server, release network, or AWS account.
#![cfg(unix)]
use base64::{engine::general_purpose::STANDARD, Engine};
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
        fs::write(root.join("wrangler.jsonc"), "{/* immutable */\n\"name\":\"my-app\",\"main\":\"src.ts\",\"define\":{\"VALUE\":\"1\"},}").unwrap();
        fs::write(
            root.join("src.ts"),
            "export default {fetch(){return new Response(VALUE)}}",
        )
        .unwrap();
        fs::write(root.join("pnpm-lock.yaml"), "").unwrap();
        fs::create_dir_all(root.join("node_modules/.bin")).unwrap();
        let fixture = Self {
            _temp: temp,
            root,
            bin,
            cache,
            requests,
        };
        fixture.esbuild();
        fixture.ssh(false);
        fixture.native("1.2.3", &fixture.build_body("native-version-id"));
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
    fn esbuild(&self) {
        executable(
            &self.root.join("node_modules/.bin/esbuild"),
            &format!(
                r#"#!/bin/sh
if env | grep -E '^(AWS_|S3_|CELLD_BUCKET=)' >/dev/null; then echo 'storage env leaked to esbuild' >&2; exit 91; fi
printf '%s\n' "$@" > {args}
for arg in "$@"; do case "$arg" in --outdir=*) out=${{arg#--outdir=}};; esac; done
mkdir -p "$out"
printf '%s' 'export default {{fetch(){{return new Response("bundled")}}}}' > "$out/index.js"
printf '\000asm' > "$out/module.wasm"
"#,
                args = quote(&self.root.join("esbuild-args"))
            ),
        );
    }
    fn build_body(&self, id: &str) -> String {
        let out = self._temp.path().join("native-out");
        format!(
            r#"if [ "$1" = deploy ]; then
"$CELLD_ESBUILD" src.ts --bundle --loader:.wasm=copy --outdir={out} || exit $?
rm {index} {wasm}
rmdir {out}
fi
printf '%s\n' '{{"worker":"my-app","version":"{id}","location":"","dry_run":true}}'
"#,
            out = quote(&out),
            index = quote(&out.join("index.js")),
            wasm = quote(&out.join("module.wasm"))
        )
    }
    fn native_source(&self, version: &str, body: &str) -> String {
        format!(
            r#"#!/bin/sh
if [ "$1" = --version ]; then echo 'celld {version}'; exit 0; fi
printf '%s\n' "$@" > {args}
printf '%s\n' "$CELLD_ESBUILD" > {esbuild}
if [ "$1" = deploy ] && env | grep -E '^(AWS_|S3_|CELLD_BUCKET=)' >/dev/null; then echo 'storage env leaked to local build' >&2; exit 97; fi
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
if env | grep -E '^(AWS_|S3_|CELLD_BUCKET=)' >/dev/null; then echo 'credentials leaked to SSH' >&2; exit 90; fi
if [ "$SSH_AUTH_SOCK" != "/unused-agent" ]; then echo 'expected local agent socket' >&2; exit 90; fi
printf '%s\n' "$@" >> {ssh_args}
IFS= read -r request
printf '%s\n' "$request" >> {requests}
{failure}
case "$request" in
  *'"op":"provision"'*|*'"op":"target"'*) cat >/dev/null; printf '%s\n' '{{"ok":true,"result":{{"slug":"my-app","celld_version":"1.2.3","enabled":false}}}}';;
  *'"op":"deploy"'*) cat > {payload}; id=$(printf '%s' "$request" | sed -n 's/.*"version_id":"\([^"]*\)".*/\1/p'); printf '%s\n' '{{"ok":true,"result":{{"version_id":"'"$id"'","native_output":{{"worker":"my-app","version":"'"$id"'","location":"s3://HOST_ONLY_PREFIX","dry_run":false}},"native_stderr":"host native uploaded\n","enabled":true}}}}';;
  *'"op":"logs"'*) cat >/dev/null; printf '%s\n' '{{"ok":true,"result":{{"text":"journal entry\n"}}}}';;
  *'"op":"deployments"'*) cat >/dev/null; printf '%s\n' '{{"ok":true,"result":[{{"version_id":"native-version-id"}}]}}';;
  *) cat >/dev/null; printf '%s\n' '{{"ok":true,"result":{{"slug":"my-app"}}}}';;
esac
"#,
                ssh_args = quote(&self.root.join("ssh-args")),
                requests = quote(&self.requests),
                payload = quote(&self._temp.path().join("payload")),
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
            .env("AWS_PROFILE", "never-read")
            .env("AWS_CONFIG_FILE", "/nonexistent/config")
            .env("S3_ENDPOINT", "https://must-not-be-used.invalid")
            .env("CELLD_BUCKET", "s3://must-not-be-used")
            .env("SSH_AUTH_SOCK", "/unused-agent")
            .arg("--project")
            .arg(&self.root)
            .arg("--host")
            .arg("vm+my-vm@vm.exe.xyz")
            .arg("--identity")
            .arg(self._temp.path().join("owner-key"));
        command
    }
    fn run(&self, args: &[&str]) -> Output {
        self.command().args(args).output().unwrap()
    }
    fn requests(&self) -> Vec<serde_json::Value> {
        fs::read_to_string(&self.requests)
            .unwrap_or_default()
            .lines()
            .map(|s| serde_json::from_str(s).unwrap())
            .collect()
    }
    fn payload(&self) -> cella::bundle::PreparedBundle {
        serde_json::from_slice(&fs::read(self._temp.path().join("payload")).unwrap()).unwrap()
    }
}

#[test]
fn ssh_only_publish_captures_native_output_without_credentials_or_project_changes() {
    let f = Fixture::new();
    let original = fs::read(f.root.join("wrangler.jsonc")).unwrap();
    let output = f.run(&["deploy", "--source-revision", "commit-123"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let native: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(native["version"], "native-version-id");
    assert_eq!(native["dry_run"], false);
    assert!(String::from_utf8_lossy(&output.stderr).contains("host native uploaded\n"));
    let requests = f.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0]["op"], "provision");
    assert_eq!(requests[1]["op"], "deploy");
    assert_eq!(requests[1]["version_id"], "native-version-id");
    assert_eq!(requests[1]["source_revision"], "commit-123");
    assert_eq!(requests[1]["celld_version"], "1.2.3");
    assert_eq!(
        requests[1]["bundle_size"].as_u64().unwrap(),
        fs::metadata(f._temp.path().join("payload")).unwrap().len()
    );
    let payload = f.payload();
    assert_eq!(payload.config["main"], "modules/index.js");
    assert_eq!(payload.config["no_bundle"], true);
    assert!(payload.config.get("define").is_none());
    assert_eq!(payload.modules.len(), 2);
    assert_eq!(
        STANDARD
            .decode(
                &payload
                    .modules
                    .iter()
                    .find(|x| x.path == "module.wasm")
                    .unwrap()
                    .content
            )
            .unwrap(),
        b"\0asm"
    );
    assert_eq!(fs::read(f.root.join("wrangler.jsonc")).unwrap(), original);
    let args = fs::read_to_string(f.root.join("native-args")).unwrap();
    assert!(args.contains("--dry-run\n--bucket\ns3://cella-build"));
    assert!(!args.contains("--endpoint"));
    assert!(!args.contains("--region"));
    let request_json = serde_json::to_string(&requests).unwrap();
    assert!(!request_json.contains("AWS_"));
    assert!(!request_json.contains("bucket"));
    let ssh_args = fs::read_to_string(f.root.join("ssh-args")).unwrap();
    assert!(ssh_args.contains("vm+my-vm@vm.exe.xyz"));
    assert!(ssh_args.contains("sudo -n /usr/local/bin/celld-ctl transport"));
    assert!(ssh_args.contains("StrictHostKeyChecking=yes"));
    assert!(ssh_args.contains("ForwardAgent=no"));
    assert!(ssh_args.contains("-p\n22"));
    assert!(!ssh_args.contains("native-version-id"));
    assert!(!f._temp.path().join("native-out").exists());
}

#[test]
fn no_storage_environment_is_required_at_all() {
    let f = Fixture::new();
    let mut command = f.command();
    for key in [
        "AWS_ACCESS_KEY_ID",
        "AWS_SECRET_ACCESS_KEY",
        "AWS_SESSION_TOKEN",
        "AWS_PROFILE",
        "AWS_CONFIG_FILE",
        "S3_ENDPOINT",
        "CELLD_BUCKET",
    ] {
        command.env_remove(key);
    }
    let output = command.arg("deploy").output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
#[test]
fn native_failure_preserves_errors_exit_code_and_never_uploads() {
    let f = Fixture::new();
    f.native(
        "1.2.3",
        "echo 'native unsupported Wrangler option error exactly' >&2; exit 17",
    );
    let output = f.run(&["deploy"]);
    assert_eq!(output.status.code(), Some(17));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("native unsupported Wrangler option error exactly\n"));
    assert_eq!(f.requests().len(), 1);
}
#[test]
fn version_mismatch_never_runs_build_or_downloads_newer() {
    let f = Fixture::new();
    f.native("1.2.4", "exit 0");
    let output = f.run(&["deploy"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("version mismatch"));
    assert!(!f.root.join("native-args").exists());
    assert_eq!(f.requests().len(), 1);
}
#[test]
fn host_failure_prevents_build() {
    let f = Fixture::new();
    f.ssh(true);
    let output = f.run(&["deploy"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("permission denied by host"));
    assert!(!f.root.join("native-args").exists());
}
#[test]
fn malformed_non_dryrun_or_missing_capture_never_uploads() {
    for body in [
        "echo invalid",
        "echo '{\"version\":\"id\",\"dry_run\":false}'",
        "echo '{\"version\":\"\",\"dry_run\":true}'",
        "echo '{\"version\":\"id\",\"dry_run\":true}'",
    ] {
        let f = Fixture::new();
        f.native("1.2.3", body);
        let output = f.run(&["deploy"]);
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert_eq!(f.requests().len(), 1);
    }
}
#[test]
fn explicit_dev_pin_needs_no_host_or_identity() {
    let f = Fixture::new();
    f.native("1.2.3", "echo local-dev");
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
    assert!(f.requests().is_empty());
}
#[test]
fn dev_without_pin_reads_minimal_target() {
    let f = Fixture::new();
    f.native("1.2.3", "echo local-dev");
    assert!(f.run(&["dev"]).status.success());
    assert_eq!(
        f.requests(),
        vec![serde_json::json!({"op":"target","slug":"my-app"})]
    );
}
#[test]
fn read_only_commands() {
    let f = Fixture::new();
    assert!(f.run(&["status"]).status.success());
    assert_eq!(f.run(&["logs", "--lines", "5"]).stdout, b"journal entry\n");
    assert!(
        String::from_utf8_lossy(&f.run(&["deployments", "list"]).stdout)
            .contains("native-version-id")
    );
    assert_eq!(f.requests()[1]["lines"], 5);
    assert!(!f.run(&["logs", "--lines", "1001"]).status.success());
}
#[test]
fn failed_download_is_not_cached_and_does_not_upload() {
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
        gzip.write_all(f.native_source(version, &f.build_body("new-id")).as_bytes())
            .unwrap();
        gzip.finish().unwrap();
        executable(&f.bin.join("curl"),&format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > {}\nwhile [ $# -gt 0 ]; do if [ \"$1\" = --output ]; then cp {} \"$2\"; exit; fi; shift; done\nexit 1\n",quote(&f.root.join("curl-args")),quote(&archive)));
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
fn invalid_revision_is_rejected_before_provision_or_build() {
    let f = Fixture::new();
    assert!(!f
        .run(&["deploy", "--source-revision", "bad;revision"])
        .status
        .success());
    assert!(f.requests().is_empty());
    assert!(!f.root.join("native-args").exists());
}
#[test]
fn early_upload_rejection_does_not_deadlock_on_full_stdin_pipe() {
    let f = Fixture::new();
    fs::create_dir(f.root.join("public")).unwrap();
    fs::write(f.root.join("public/large.bin"), vec![42u8; 2 * 1024 * 1024]).unwrap();
    fs::write(
        f.root.join("wrangler.jsonc"),
        r#"{"name":"my-app","main":"src.ts","assets":{"directory":"public"}}"#,
    )
    .unwrap();
    let script = fs::read_to_string(f.bin.join("ssh")).unwrap().replace(
        "cat > ",
        "echo '{\"ok\":false,\"error\":\"deploy denied before upload\"}'; exit 1; cat > ",
    );
    executable(&f.bin.join("ssh"), &script);
    let output = f.run(&["deploy"]);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("deploy denied before upload"));
    assert_eq!(f.requests().len(), 2);
}
#[test]
fn yarn_pnp_wrapper_is_used_inside_capture_without_network() {
    let f = Fixture::new();
    fs::remove_file(f.root.join("pnpm-lock.yaml")).unwrap();
    fs::rename(
        f.root.join("node_modules/.bin/esbuild"),
        f.bin.join("real-esbuild"),
    )
    .unwrap();
    fs::write(f.root.join("yarn.lock"), "").unwrap();
    fs::write(f.root.join(".pnp.cjs"), "").unwrap();
    executable(&f.bin.join("yarn"),&format!("#!/bin/sh\ntest \"$COREPACK_ENABLE_NETWORK\" = 0 || exit 92\ntest \"$1\" = exec || exit 93\ntest \"$2\" = esbuild || exit 94\nshift 2\nexec {} \"$@\"\n",quote(&f.bin.join("real-esbuild"))));
    let output = f.run(&["deploy"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(f.root.join("esbuild-args").exists());
}
#[test]
fn containers_and_python_are_rejected_before_native_tools() {
    for config in [
        r#"{"name":"my-app","main":"src.ts","containers":[]}"#,
        r#"{"name":"my-app","main":"worker.py"}"#,
    ] {
        let f = Fixture::new();
        fs::write(f.root.join("wrangler.jsonc"), config).unwrap();
        let output = f.run(&["deploy"]);
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("JavaScript/TypeScript only"));
        assert!(!f.root.join("native-args").exists());
        assert_eq!(f.requests().len(), 1);
    }
}
#[test]
fn captures_asset_directives_and_nested_unbundled_wasm_without_touching_originals() {
    let f = Fixture::new();
    fs::create_dir_all(f.root.join("built/nested")).unwrap();
    fs::create_dir_all(f.root.join("built/.git")).unwrap();
    fs::write(f.root.join("built/main.js"), "export default {}").unwrap();
    fs::write(f.root.join("built/nested/a.wasm?module"), b"\0asm").unwrap();
    fs::write(f.root.join("built/.git/ignored.wasm"), b"ignored").unwrap();
    fs::create_dir(f.root.join("public")).unwrap();
    fs::write(f.root.join("public/_headers"), "/*\n X-Test: yes\n").unwrap();
    fs::write(f.root.join("public/_redirects"), "/old /new 301\n").unwrap();
    fs::write(f.root.join("public/index.html"), "hi").unwrap();
    fs::write(f.root.join("wrangler.jsonc"),r#"{"name":"my-app","main":"built/main.js","no_bundle":true,"assets":{"directory":"public"}}"#).unwrap();
    f.native(
        "1.2.3",
        "echo '{\"version\":\"native-version-id\",\"dry_run\":true}'",
    );
    let output = f.run(&["deploy"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let payload = f.payload();
    assert_eq!(
        payload
            .modules
            .iter()
            .map(|m| m.path.as_str())
            .collect::<Vec<_>>(),
        vec!["index.js", "nested/a.wasm?module"]
    );
    assert_eq!(
        payload
            .assets
            .iter()
            .map(|m| m.path.as_str())
            .collect::<Vec<_>>(),
        vec!["_headers", "_redirects", "index.html"]
    );
    assert_eq!(payload.config["assets"]["directory"], "assets");
}
#[test]
fn asset_only_bundle_does_not_invent_main_or_no_bundle() {
    let f = Fixture::new();
    fs::create_dir(f.root.join("public")).unwrap();
    fs::write(f.root.join("public/index.html"), "hi").unwrap();
    fs::write(
        f.root.join("wrangler.jsonc"),
        r#"{"name":"my-app","assets":{"directory":"public"}}"#,
    )
    .unwrap();
    f.native(
        "1.2.3",
        "echo '{\"version\":\"native-version-id\",\"dry_run\":true}'",
    );
    let output = f.run(&["deploy"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let payload = f.payload();
    assert!(payload.modules.is_empty());
    assert!(payload.config.get("main").is_none());
    assert!(payload.config.get("no_bundle").is_none());
}

#[test]
fn nonstandard_compiled_wasm_extension_is_rejected_not_dropped() {
    let f = Fixture::new();
    let esbuild = f.root.join("node_modules/.bin/esbuild");
    let script = fs::read_to_string(&esbuild)
        .unwrap()
        .replace("module.wasm", "module.binary");
    executable(&esbuild, &script);
    let body = f
        .build_body("native-version-id")
        .replace("--loader:.wasm=copy", "--loader:.binary=copy");
    f.native("1.2.3", &body);
    let output = f.run(&["deploy"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("nonstandard CompiledWasm extension"));
    assert_eq!(f.requests().len(), 1);
}

#[test]
fn explicit_esbuild_override_wins_over_installed_project_tool() {
    let f = Fixture::new();
    let custom = f.bin.join("custom-esbuild");
    fs::copy(f.root.join("node_modules/.bin/esbuild"), &custom).unwrap();
    executable(
        &f.root.join("node_modules/.bin/esbuild"),
        "#!/bin/sh\nexit 96\n",
    );
    let output = f
        .command()
        .env("CELLD_ESBUILD", &custom)
        .arg("deploy")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
