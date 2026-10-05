use anyhow::{ensure, Result};
use celld_ctl::{
    config::{atomic_write, parse_credentials, storage_origin, Config, Paths},
    manager::Manager,
    registry::App,
    render,
    runtime::{validate_pointer, Runtime},
    transport::{parse_request, validate_original_command},
};
use celld_ctl_core::{Request, Target, MAX_REQUEST_BYTES};
use serde_json::json;
use std::{
    collections::HashSet,
    fs,
    os::unix::fs::{symlink, PermissionsExt},
    path::{Path, PathBuf},
};
use tempfile::TempDir;

#[derive(Default)]
struct Fake {
    calls: Vec<String>,
    running: HashSet<String>,
    deployed: Option<String>,
    fail_ready: bool,
    fail_validate: bool,
    fail_reload: bool,
    fail_native: bool,
    fail_pin: bool,
    fail_publish: bool,
    published: Option<celld_ctl_core::PreparedBundle>,
    blocked: HashSet<u16>,
    loaded: String,
}
impl Runtime for Fake {
    fn deploy(
        &mut self,
        _: &App,
        bundle: &celld_ctl_core::PreparedBundle,
        expected: &str,
    ) -> Result<celld_ctl::publish::NativePublish> {
        self.calls.push("native-publish".into());
        ensure!(!self.fail_publish, "native validation failed");
        self.published = Some(bundle.clone());
        self.deployed = Some(expected.into());
        Ok(celld_ctl::publish::NativePublish {
            native_output: json!({"version":expected,"dry_run":false}),
            native_stderr: "native progress".into(),
        })
    }

    fn systemctl(&mut self, action: &str, unit: Option<&str>) -> Result<()> {
        self.calls.push(format!("{action}:{}", unit.unwrap_or("")));
        if let Some(unit) = unit {
            if ["start", "restart"].contains(&action) {
                self.running.insert(unit.into());
            }
            if action == "stop" {
                self.running.remove(unit);
            }
        }
        Ok(())
    }
    fn active(&mut self, app: &App) -> Result<bool> {
        Ok(self.running.contains(&app.unit))
    }
    fn validate_caddy(&mut self, path: &Path) -> Result<()> {
        self.calls.push("validate-caddy".into());
        ensure!(path.is_file(), "candidate missing");
        ensure!(!self.fail_validate, "bad caddy");
        Ok(())
    }
    fn reload_caddy(&mut self, path: &Path) -> Result<()> {
        self.calls.push("reload-caddy".into());
        ensure!(!self.fail_reload, "reload failed");
        self.loaded = fs::read_to_string(path)?;
        Ok(())
    }
    fn verify_pointer(&mut self, _: &App, expected: Option<&str>) -> Result<String> {
        self.calls.push("pointer".into());
        let version = self
            .deployed
            .clone()
            .ok_or_else(|| anyhow::anyhow!("not deployed"))?;
        ensure!(expected.is_none_or(|x| x == version), "pointer mismatch");
        Ok(version)
    }
    fn readiness(&mut self, _: &App, _: &str, _: u64) -> Result<()> {
        self.calls.push("ready".into());
        ensure!(!self.fail_ready, "not ready");
        Ok(())
    }
    fn reload_app(&mut self, _: &App) -> Result<()> {
        self.calls.push("reload-native".into());
        ensure!(!self.fail_native, "native reload failed");
        Ok(())
    }
    fn logs(&mut self, _: &App, lines: u32) -> Result<String> {
        Ok(format!("{lines} log lines"))
    }
    fn port_free(&mut self, port: u16) -> bool {
        !self.blocked.contains(&port)
    }
    fn verify_binary(&mut self, _: &Path, _: &str) -> Result<()> {
        self.calls.push("pin".into());
        ensure!(!self.fail_pin, "wrong pin");
        Ok(())
    }
    fn observed_version(&mut self, _: &App) -> Result<Option<String>> {
        Ok(self.deployed.clone())
    }
}
struct Fixture {
    _root: TempDir,
    paths: Paths,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let paths = Paths::under(root.path());
        for p in [
            &paths.config,
            &paths.credentials,
            &paths.releases.join("v0.6.1/celld"),
        ] {
            fs::create_dir_all(p.parent().unwrap()).unwrap();
        }
        fs::write(&paths.config,serde_json::to_vec(&json!({"bucket":"s3://example-bucket","endpoint":"https://storage.example.invalid","region":"auto","celld_version":"0.6.1"})).unwrap()).unwrap();
        fs::write(
            &paths.credentials,
            "AWS_ACCESS_KEY_ID=EXAMPLEACCESS\nAWS_SECRET_ACCESS_KEY=EXAMPLESECRET\n",
        )
        .unwrap();
        fs::write(
            paths.releases.join("v0.6.1/celld"),
            b"test fixture, never executed",
        )
        .unwrap();
        fs::set_permissions(
            paths.releases.join("v0.6.1/celld"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        Self { _root: root, paths }
    }
    fn manager(&self) -> Manager<Fake> {
        Manager::open(self.paths.clone(), Fake::default()).unwrap()
    }
}
fn activated(f: &Fixture, slug: &str) -> Manager<Fake> {
    let mut m = f.manager();
    m.provision(slug).unwrap();
    m.runtime.deployed = Some("abc123".into());
    m.activate(slug, Some("abc123"), Some("0123456789"), false)
        .unwrap();
    m
}

#[test]
fn storage_origin_policy_is_identical_for_config_and_registry() {
    for good in [
        "https://storage.example.invalid",
        "https://storage.example.invalid:9443/",
        "http://127.0.0.1:9000",
        "http://127.0.0.1:9000/",
    ] {
        storage_origin(good).unwrap();
        let f = Fixture::new();
        let mut config = Config::load(&f.paths).unwrap();
        config.endpoint = good.into();
        config.validate().unwrap();
    }
    for bad in [
        "http://localhost:9000",
        "http://127.1:9000",
        "http://2130706433:9000",
        "http://0x7f000001:9000",
        "http://127.0.0.2:9000",
        "http://10.0.0.1:9000",
        "http://0.0.0.0:9000",
        "http://[::1]:9000",
        "http://127.0.0.1",
        "http://127.0.0.1:0",
        "http://127.0.0.1:9000/path",
        "http://127.0.0.1:9000?x=1",
        "http://127.0.0.1:9000/#frag",
        "http://user@127.0.0.1:9000",
        "ftp://storage.example.invalid",
        "https://user:password@storage.example.invalid",
        "https://storage.example.invalid/path",
    ] {
        assert!(storage_origin(bad).is_err(), "accepted {bad}");
        let f = Fixture::new();
        let mut config = Config::load(&f.paths).unwrap();
        config.endpoint = bad.into();
        assert!(config.validate().is_err(), "config accepted {bad}");
    }
}

#[test]
fn local_only_unit_depends_on_rustfs() {
    let f = Fixture::new();
    let mut m = f.manager();
    m.provision("external").unwrap();
    let external = m.registry.get("external").unwrap();
    assert!(!render::unit_override(&external, &f.paths).contains("rustfs.service"));
    let mut local = external.clone();
    local.target.endpoint = "http://127.0.0.1:9000".into();
    let unit = render::unit_override(&local, &f.paths);
    assert!(unit.contains("[Unit]\nWants=rustfs.service\nAfter=rustfs.service"));
}

#[test]
fn provision_is_disabled_and_never_contacts_fleet_or_starts() {
    let f = Fixture::new();
    let mut m = f.manager();
    let t = m.provision("123-app").unwrap();
    assert_eq!(t.bucket, "s3://example-bucket/cells/123-app");
    assert!(!t.enabled);
    assert_eq!(m.runtime.calls, vec!["pin", "daemon-reload:"]);
    assert!(!f.paths.caddy.exists());
    let app = m.registry.get("123-app").unwrap();
    assert_eq!((app.port, app.internal_port), (8101, 18101));
    let env = fs::read_to_string(f.paths.cells.join("123-app.env")).unwrap();
    assert!(!env.contains("EXAMPLESECRET"));
    assert!(env.contains("CELLD_BIN="));
    let unit = fs::read_to_string(
        f.paths
            .units
            .join("celld-cell@123-app.service.d/50-celld-ctl.conf"),
    )
    .unwrap();
    assert!(unit.contains("/v0.6.1/celld --listen 127.0.0.1:8101"));
    assert!(unit.contains("--advertise 127.0.0.1:18101"));
    assert!(unit.contains("LogNamespace=celld"));
    let calls = m.runtime.calls.len();
    assert_eq!(m.provision("123-app").unwrap(), t);
    assert_eq!(m.runtime.calls.len(), calls);
    let t = m.provision("transport").unwrap();
    assert_eq!(t.slug, "transport");
}
#[test]
fn allocation_avoids_bound_and_registered_ports() {
    let f = Fixture::new();
    let mut m = f.manager();
    m.runtime.blocked.insert(8101);
    m.provision("one").unwrap();
    m.provision("two").unwrap();
    assert_eq!(m.registry.get("one").unwrap().port, 8102);
    assert_eq!(m.registry.get("two").unwrap().port, 8103);
}
#[test]
fn no_initial_start_without_verified_pointer() {
    let f = Fixture::new();
    let mut m = f.manager();
    m.provision("app").unwrap();
    m.runtime.calls.clear();
    assert!(m.activate("app", Some("fake"), None, false).is_err());
    assert_eq!(m.runtime.calls, vec!["pointer"]);
    assert!(!m.registry.get("app").unwrap().target.enabled);
}
#[test]
fn activation_records_revision_only_after_readiness_and_routes() {
    let f = Fixture::new();
    let mut m = activated(&f, "app");
    let history = m.registry.history("app").unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].source_revision.as_deref(), Some("0123456789"));
    assert!(m.registry.get("app").unwrap().target.enabled);
    let ready = m.runtime.calls.iter().position(|s| s == "ready").unwrap();
    let publish = m
        .runtime
        .calls
        .iter()
        .rposition(|s| s == "reload-caddy")
        .unwrap();
    assert!(ready < publish);
    assert!(m.runtime.loaded.contains("handle /app/*"));
    assert!(!m.runtime.loaded.contains("handle_path"));
    assert!(!m.runtime.loaded.contains("18101"));
    assert!(m
        .runtime
        .loaded
        .contains("header_up X-Forwarded-Proto {upstream_proto}"));
    assert_eq!(m.status("app").unwrap()["observed_version_id"], "abc123");
}
#[test]
fn readiness_failure_leaves_first_deployment_unpublished_and_disabled() {
    let f = Fixture::new();
    let mut m = f.manager();
    m.provision("app").unwrap();
    m.runtime.deployed = Some("abc".into());
    m.runtime.fail_ready = true;
    assert!(m.activate("app", Some("abc"), None, false).is_err());
    assert!(!m.registry.get("app").unwrap().target.enabled);
    assert!(m.registry.history("app").unwrap().is_empty());
    assert!(!m.runtime.loaded.contains("handle /app/*"));
    assert!(m
        .runtime
        .calls
        .contains(&"disable:celld-cell@app.service".into()));
    assert!(!m.runtime.running.contains("celld-cell@app.service"));
}
#[test]
fn update_withdraws_route_before_native_reload_and_failure_keeps_it_closed() {
    let f = Fixture::new();
    let mut m = activated(&f, "app");
    m.runtime.calls.clear();
    m.runtime.deployed = Some("new".into());
    m.runtime.fail_native = true;
    assert!(m.activate("app", Some("new"), None, false).is_err());
    assert!(!m.registry.get("app").unwrap().target.enabled);
    assert!(!m.runtime.loaded.contains("handle /app/*"));
    let withdraw = m
        .runtime
        .calls
        .iter()
        .position(|s| s == "reload-caddy")
        .unwrap();
    let native = m
        .runtime
        .calls
        .iter()
        .position(|s| s == "reload-native")
        .unwrap();
    assert!(withdraw < native);
    assert_eq!(m.registry.history("app").unwrap().len(), 1);
}
#[test]
fn invalid_caddy_cannot_publish() {
    let f = Fixture::new();
    let mut m = f.manager();
    m.provision("app").unwrap();
    m.runtime.deployed = Some("abc".into());
    m.runtime.fail_validate = true;
    assert!(m.activate("app", None, None, false).is_err());
    assert!(!m.registry.get("app").unwrap().target.enabled);
    assert!(!m.runtime.calls.contains(&"reload-caddy".into()));
    assert!(m.registry.history("app").unwrap().is_empty());
}
#[test]
fn reload_failure_restores_disk_and_fails_closed_if_rollback_unavailable() {
    let f = Fixture::new();
    let mut m = f.manager();
    m.provision("app").unwrap();
    fs::write(&f.paths.caddy, "# previous routes\n").unwrap();
    m.runtime.deployed = Some("abc".into());
    m.runtime.fail_reload = true;
    assert!(m.activate("app", None, None, false).is_err());
    assert_eq!(
        fs::read_to_string(&f.paths.caddy).unwrap(),
        "# previous routes\n"
    );
    assert!(!m.registry.get("app").unwrap().target.enabled);
    assert!(m.runtime.calls.contains(&"stop:caddy.service".into()));
}
#[test]
fn failed_status_write_occurs_before_publication() {
    let f = Fixture::new();
    let mut m = f.manager();
    m.provision("app").unwrap();
    fs::remove_file(f.paths.public.join("index.html")).unwrap();
    fs::create_dir(f.paths.public.join("index.html")).unwrap();
    m.runtime.deployed = Some("abc".into());
    assert!(m.activate("app", None, None, false).is_err());
    assert!(!m.registry.get("app").unwrap().target.enabled);
    assert!(!m.runtime.calls.contains(&"reload-caddy".into()));
}
#[test]
fn postpublication_audit_failure_withdraws_route() {
    let f = Fixture::new();
    let mut m = f.manager();
    m.provision("app").unwrap();
    m.runtime.deployed = Some("abc".into());
    m.registry.conn.execute_batch("CREATE TRIGGER reject_activation BEFORE INSERT ON audit WHEN NEW.operation='activate' BEGIN SELECT RAISE(ABORT,'test failure'); END;").unwrap();
    assert!(m.activate("app", None, None, false).is_err());
    assert!(!m.registry.get("app").unwrap().target.enabled);
    assert!(!m.runtime.loaded.contains("handle /app/*"));
}
#[test]
fn disable_and_remove_never_delete_durable_or_local_state() {
    let f = Fixture::new();
    let mut m = activated(&f, "app");
    let local = f.paths.app_state.join("app");
    fs::create_dir_all(&local).unwrap();
    fs::write(local.join("cache"), "keep").unwrap();
    m.lifecycle("disable", "app").unwrap();
    assert!(!m.runtime.loaded.contains("handle /app/*"));
    m.lifecycle("remove", "app").unwrap();
    assert!(m.registry.find("app").unwrap().is_none());
    assert_eq!(fs::read_to_string(local.join("cache")).unwrap(), "keep");
    assert_eq!(m.registry.history("app").unwrap().len(), 1);
}
#[test]
fn backup_is_consistent_private_and_has_no_node_slug_collision() {
    let f = Fixture::new();
    let mut m = activated(&f, "node");
    fs::create_dir_all(f.paths.authorized_keys.parent().unwrap()).unwrap();
    fs::write(
        &f.paths.authorized_keys,
        "restrict ssh-ed25519 EXAMPLE owner",
    )
    .unwrap();
    let result = m.backup().unwrap();
    let path = PathBuf::from(result["backup"].as_str().unwrap());
    assert!(path.join("COMPLETE").is_file());
    assert!(fs::read_to_string(path.join("config/node.env"))
        .unwrap()
        .contains("EXAMPLESECRET"));
    assert!(fs::read_to_string(path.join("cells/node.env"))
        .unwrap()
        .contains("CELLD_BUCKET"));
    assert!(!fs::read_to_string(path.join("cells/node.env"))
        .unwrap()
        .contains("EXAMPLESECRET"));
    assert!(path.join("ssh/authorized_keys").is_file());
    assert!(path.join("config/pins.json").is_file());
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o700
    );
    let db = rusqlite::Connection::open(path.join("registry.sqlite")).unwrap();
    let slug: String = db
        .query_row("SELECT slug FROM apps", [], |r| r.get(0))
        .unwrap();
    assert_eq!(slug, "node");
    let bytes = fs::read(&f.paths.registry).unwrap();
    assert!(!bytes
        .windows("EXAMPLESECRET".len())
        .any(|b| b == b"EXAMPLESECRET"));
}
#[test]
fn imports_legacy_bucket_root_and_unit_then_restarts_before_publishing() {
    let f = Fixture::new();
    let mut m = f.manager();
    m.runtime.deployed = Some("abc".into());
    m.runtime.running.insert("celld-counter.service".into());
    fs::write(f.paths.units.join("celld-counter.service"), "[Service]\n").unwrap();
    m.import_counter("0.6.1", Some("abc"), true).unwrap();
    let app = m.registry.get("counter").unwrap();
    assert!(app.legacy);
    assert_eq!(app.target.bucket, "s3://example-bucket");
    assert_eq!(app.unit, "celld-counter.service");
    assert_eq!((app.port, app.internal_port), (8100, 18100));
    assert!(app.target.enabled);
    assert!(m
        .runtime
        .calls
        .contains(&"restart:celld-counter.service".into()));
    assert!(m
        .runtime
        .calls
        .contains(&"enable:celld-counter.service".into()));
    let backup = m.backup().unwrap();
    assert!(Path::new(backup["backup"].as_str().unwrap())
        .join("units/celld-counter.service")
        .is_file());
}
#[test]
fn app_pins_do_not_follow_default_config_changes() {
    let f = Fixture::new();
    let mut m = f.manager();
    m.provision("app").unwrap();
    m.config.celld_version = "99.0.0".into();
    assert_eq!(m.provision("app").unwrap().celld_version, "0.6.1");
    m.runtime.fail_pin = true;
    m.runtime.deployed = Some("abc".into());
    assert!(m.activate("app", None, None, false).is_err());
}
#[test]
fn lock_is_held_for_entire_manager_lifetime() {
    use fs2::FileExt;
    let f = Fixture::new();
    let m = f.manager();
    let other = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(f.paths.state.join("operation.lock"))
        .unwrap();
    assert!(other.try_lock_exclusive().is_err());
    drop(m);
    other.try_lock_exclusive().unwrap();
}
#[test]
fn transport_rejects_unknown_commands_fields_paths_and_large_inputs() {
    for command in [
        "",
        "bash",
        "celld-ctl transport",
        "celld-ctl-transport;id",
        "celld-ctl-transport ",
    ] {
        assert!(validate_original_command(Some(command)).is_err());
    }
    validate_original_command(Some("celld-ctl-transport")).unwrap();
    validate_original_command(None).unwrap();
    for bytes in [
        br#"{"op":"remove","slug":"app"}"#.as_slice(),
        br#"{"op":"target","slug":"app","config":"/tmp/x"}"#,
        br#"{"op":"provision","slug":"../x"}"#,
        br#"{"op":"activate","slug":"app","version_id":"abc","source_revision":"a\nb"}"#,
        br#"{"op":"logs","slug":"app","lines":1001}"#,
        br#"{"op":"target","slug":"app"} {}"#,
    ] {
        assert!(parse_request(bytes).is_err());
    }
    assert!(parse_request(&vec![b' '; MAX_REQUEST_BYTES + 1]).is_err());
    assert!(parse_request(br#"{"op":"target","slug":"transport"}"#).is_ok());
}
#[test]
fn target_shape_is_exact_and_secret_free() {
    let f = Fixture::new();
    let mut m = f.manager();
    m.provision("app").unwrap();
    let value = m.request(Request::Target { slug: "app".into() }).unwrap();
    assert_eq!(value.as_object().unwrap().len(), 6);
    assert_eq!(value["slug"], "app");
    assert!(!value.to_string().contains("EXAMPLE"));
    serde_json::from_value::<Target>(value).unwrap();
}
#[test]
fn config_is_strict_and_environment_parser_never_evaluates_shell() {
    let f = Fixture::new();
    let mut value: serde_json::Value =
        serde_json::from_slice(&fs::read(&f.paths.config).unwrap()).unwrap();
    value["root"] = json!("/tmp/evil");
    assert!(serde_json::from_value::<Config>(value).is_err());
    for quote in ["'", "\""] {
        fs::write(
            &f.paths.credentials,
            format!("AWS_ACCESS_KEY_ID={quote}\nAWS_SECRET_ACCESS_KEY=x\n"),
        )
        .unwrap();
        assert!(parse_credentials(&f.paths).is_err());
    }
    fs::write(
        &f.paths.credentials,
        "AWS_ACCESS_KEY_ID='literal'\nAWS_SECRET_ACCESS_KEY=$(id)\n",
    )
    .unwrap();
    assert_eq!(
        parse_credentials(&f.paths).unwrap()["AWS_SECRET_ACCESS_KEY"],
        "$(id)"
    );
}
#[test]
fn atomic_writes_reject_symlinks_and_preserve_target() {
    let f = Fixture::new();
    let target = f._root.path().join("untouched");
    fs::write(&target, "old").unwrap();
    let link = f._root.path().join("link");
    symlink(&target, &link).unwrap();
    assert!(atomic_write(&f.paths, &link, b"new", 0o600).is_err());
    assert_eq!(fs::read_to_string(target).unwrap(), "old");
}
#[test]
fn pointer_allows_different_worker_name_and_rejects_mismatch_or_partial_rollout() {
    let value = json!({"script_name":"native-worker","version":"abc123","prefix":"deploy/native-worker/abc123","rollout":{"percent":100}});
    assert_eq!(validate_pointer(&value, Some("abc123")).unwrap(), "abc123");
    assert!(validate_pointer(&value, Some("other")).is_err());
    let mut bad = value.clone();
    bad["prefix"] = json!("../../other");
    assert!(validate_pointer(&bad, None).is_err());
    let mut bad = value;
    bad["rollout"]["percent"] = json!(50);
    assert!(validate_pointer(&bad, None).is_err());
}
#[test]
fn html_escapes_values_and_never_exposes_internal_targets() {
    let f = Fixture::new();
    let mut m = f.manager();
    m.provision("app").unwrap();
    let mut app = m.registry.get("app").unwrap();
    app.version_id = Some("<script>alert(1)</script>".into());
    let text = render::html(&[app]);
    assert!(text.contains("&lt;script&gt;"));
    assert!(!text.contains("<script>"));
    assert!(!text.contains("18101"));
    assert!(!text.contains("storage.example.invalid"));
}

#[test]
fn operator_restart_does_not_hide_the_last_source_revision() {
    let f = Fixture::new();
    let mut m = activated(&f, "app");
    m.lifecycle("restart", "app").unwrap();
    m.lifecycle("reload", "app").unwrap();
    let history = m.registry.history("app").unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].source_revision.as_deref(), Some("0123456789"));
    m.activate(
        "app",
        Some("abc123"),
        Some("new-source-same-content"),
        false,
    )
    .unwrap();
    assert_eq!(m.registry.history("app").unwrap().len(), 2);
}
#[test]
fn cli_accepts_transport_as_a_slug_not_as_a_command() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_celld-ctl"))
        .args([
            "--root",
            "/nonexistent-celld-ctl-test-root",
            "status",
            "transport",
        ])
        .env_remove("SSH_ORIGINAL_COMMAND")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(!stderr.contains("transport accepts"));
    assert!(!stderr.contains("transport forbids"));
}
#[test]
fn cli_rejects_arbitrary_remote_command_before_any_host_io() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_celld-ctl"))
        .arg("transport")
        .env("SSH_ORIGINAL_COMMAND", "sh -c id")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["ok"], false);
    assert!(value["error"]
        .as_str()
        .unwrap()
        .contains("SSH command rejected"));
    assert!(output.stderr.is_empty());
}

#[test]
fn atomic_public_modes_survive_private_umask() {
    // Isolate the process-global umask from parallel tests.
    if std::env::var_os("CELLD_CTL_TEST_UMASK").is_none() {
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "atomic_public_modes_survive_private_umask"])
            .env("CELLD_CTL_TEST_UMASK", "1")
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    unsafe {
        libc::umask(0o077);
    }
    let f = Fixture::new();
    let public = f._root.path().join("public.html");
    atomic_write(&f.paths, &public, b"public", 0o644).unwrap();
    assert_eq!(
        fs::metadata(public).unwrap().permissions().mode() & 0o777,
        0o644
    );
    let private = f._root.path().join("private.env");
    atomic_write(&f.paths, &private, b"private", 0o600).unwrap();
    assert_eq!(
        fs::metadata(private).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn backup_includes_dedicated_ssh_daemon_and_private_host_identity() {
    let f = Fixture::new();
    let mut m = f.manager();
    for (path, contents) in [
        (
            &f.paths.dedicated_ssh_config,
            "Port 2222\nListenAddress 127.0.0.1\n",
        ),
        (
            &f.paths.ssh_host_key,
            "PRIVATE_TEST_HOST_KEY_NOT_A_REAL_KEY",
        ),
        (&f.paths.ssh_host_public_key, "ssh-ed25519 TEST_HOST_KEY"),
        (
            &f.paths.ssh_service,
            "[Service]\nExecStart=/usr/sbin/sshd -D\n",
        ),
    ] {
        fs::write(path, contents).unwrap();
    }
    let result = m.backup().unwrap();
    let root = Path::new(result["backup"].as_str().unwrap());
    for name in [
        "ssh/sshd_config",
        "ssh/ssh-host-ed25519-key",
        "ssh/ssh-host-ed25519-key.pub",
        "units/cella-sshd.service",
    ] {
        assert!(root.join(name).is_file());
        assert_eq!(
            fs::metadata(root.join(name)).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    assert_eq!(
        fs::read_to_string(root.join("ssh/ssh-host-ed25519-key")).unwrap(),
        "PRIVATE_TEST_HOST_KEY_NOT_A_REAL_KEY"
    );
    assert_eq!(result["contains_credentials"], true);
    assert!(!fs::read(&f.paths.registry)
        .unwrap()
        .windows(b"PRIVATE_TEST_HOST_KEY_NOT_".len())
        .any(|v| v == b"PRIVATE_TEST_HOST_KEY_NOT_"));
}

#[test]
fn caddy_forwarded_host_fallback_preserves_the_incoming_port() {
    let f = Fixture::new();
    let rendered = render::caddy(&[], &f.paths);
    assert!(rendered.contains("\"\" {http.request.hostport}"));
    assert!(!rendered.contains("{http.request.host}"));
    assert!(rendered.contains("default {http.request.header.X-Forwarded-Host}"));
}

#[test]
fn disable_reconciles_a_live_route_left_by_a_crash_after_sql_was_disabled() {
    let f = Fixture::new();
    let mut m = activated(&f, "app");
    m.registry.enabled("app", false).unwrap();
    assert!(m.runtime.loaded.contains("handle /app/*"));
    m.runtime.calls.clear();
    m.lifecycle("disable", "app").unwrap();
    assert!(!m.runtime.loaded.contains("handle /app/*"));
    assert!(!fs::read_to_string(&f.paths.caddy)
        .unwrap()
        .contains("handle /app/*"));
    assert!(!m.registry.get("app").unwrap().target.enabled);
    let reload = m
        .runtime
        .calls
        .iter()
        .position(|s| s == "reload-caddy")
        .unwrap();
    let stop = m
        .runtime
        .calls
        .iter()
        .position(|s| s == "stop:celld-cell@app.service")
        .unwrap();
    assert!(reload < stop);
}
#[test]
fn failed_withdrawal_restores_the_previous_disabled_registry_flag() {
    let f = Fixture::new();
    let mut m = f.manager();
    m.provision("app").unwrap();
    m.runtime.fail_validate = true;
    assert!(m.lifecycle("disable", "app").is_err());
    assert!(!m.registry.get("app").unwrap().target.enabled);
    assert!(!m
        .runtime
        .calls
        .iter()
        .any(|s| s == "stop:celld-cell@app.service"));
}

fn prepared() -> celld_ctl_core::PreparedBundle {
    use base64::Engine;
    celld_ctl_core::PreparedBundle {
        config: json!({"name":"native-worker","main":"src/main.ts"}),
        modules: vec![celld_ctl_core::UploadFile {
            path: "index.js".into(),
            content: base64::engine::general_purpose::STANDARD.encode(b"export default {};"),
        }],
        assets: vec![],
    }
}
fn deploy_request(bundle: &celld_ctl_core::PreparedBundle) -> Request {
    Request::Deploy {
        slug: "app".into(),
        celld_version: "0.6.1".into(),
        version_id: "new-version".into(),
        source_revision: Some("source-revision".into()),
        bundle_size: serde_json::to_vec(bundle).unwrap().len(),
    }
}
#[test]
fn ssh_targets_hide_storage_and_publishing_verifies_before_activation() {
    let f = Fixture::new();
    let mut m = f.manager();
    let target = m
        .transport_request(Request::Provision { slug: "app".into() }, None)
        .unwrap();
    assert_eq!(target.as_object().unwrap().len(), 3);
    assert!(target.get("bucket").is_none());
    assert!(target.get("endpoint").is_none());
    assert!(target.get("region").is_none());
    let bundle = prepared();
    let result = m
        .transport_request(deploy_request(&bundle), Some(bundle))
        .unwrap();
    assert_eq!(result["version_id"], "new-version");
    assert_eq!(result["source_revision"], "source-revision");
    assert_eq!(result["native_stderr"], "native progress");
    let published = m
        .runtime
        .calls
        .iter()
        .position(|s| s == "native-publish")
        .unwrap();
    let ready = m.runtime.calls.iter().position(|s| s == "ready").unwrap();
    assert!(published < ready);
    assert_eq!(
        m.runtime.published.as_ref().unwrap().config["main"],
        "modules/index.js"
    );
    assert_eq!(
        m.runtime.published.as_ref().unwrap().config["no_bundle"],
        true
    );
    assert_eq!(
        m.registry.history("app").unwrap()[0]
            .source_revision
            .as_deref(),
        Some("source-revision")
    );
}
#[test]
fn malformed_bundle_pin_mismatch_and_native_validation_failure_never_activate() {
    let f = Fixture::new();
    let mut m = f.manager();
    m.provision("app").unwrap();
    m.runtime.calls.clear();
    let mut bad = prepared();
    bad.modules[0].path = "../../root.js".into();
    assert!(m.deploy(deploy_request(&bad), bad).is_err());
    assert!(m.runtime.calls.is_empty());
    let bundle = prepared();
    let mut request = deploy_request(&bundle);
    if let Request::Deploy { celld_version, .. } = &mut request {
        *celld_version = "0.0.0".into();
    }
    assert!(m.deploy(request, bundle).is_err());
    assert!(m.runtime.calls.is_empty());
    m.runtime.fail_publish = true;
    let bundle = prepared();
    assert!(m.deploy(deploy_request(&bundle), bundle).is_err());
    assert!(!m.runtime.calls.contains(&"pointer".into()));
    assert!(!m.runtime.calls.contains(&"ready".into()));
    assert!(m.registry.history("app").unwrap().is_empty());
}
#[test]
fn postpublish_activation_error_returns_recoverable_version() {
    let f = Fixture::new();
    let mut m = f.manager();
    m.provision("app").unwrap();
    m.runtime.fail_ready = true;
    let bundle = prepared();
    let error = m
        .deploy(deploy_request(&bundle), bundle)
        .unwrap_err()
        .to_string();
    assert!(error.contains("new-version"));
    assert!(error.contains("was published but activation failed"));
    assert!(error.contains("retry"));
    assert!(!m.registry.get("app").unwrap().target.enabled);
}
#[test]
fn ssh_publish_cannot_replace_the_legacy_bucket_root_fleet() {
    let f = Fixture::new();
    let mut m = f.manager();
    m.runtime.deployed = Some("original".into());
    m.import_counter("0.6.1", None, false).unwrap();
    m.runtime.calls.clear();
    let bundle = prepared();
    let mut request = deploy_request(&bundle);
    if let Request::Deploy { slug, .. } = &mut request {
        *slug = "counter".into();
    }
    assert!(m
        .deploy(request, bundle)
        .unwrap_err()
        .to_string()
        .contains("legacy"));
    assert!(m.runtime.calls.is_empty());
}
#[test]
fn deploy_frames_require_exact_lengths_single_header_and_bounded_payload() {
    use celld_ctl::transport::parse_input;
    let bundle = prepared();
    let body = serde_json::to_vec(&bundle).unwrap();
    let header = serde_json::to_vec(&deploy_request(&bundle)).unwrap();
    let mut frame = header.clone();
    frame.push(b'\n');
    frame.extend(&body);
    let parsed = parse_input(&frame).unwrap();
    assert!(parsed.bundle.is_some());
    let mut extra = frame.clone();
    extra.push(b' ');
    assert!(parse_input(&extra).is_err());
    frame.pop();
    assert!(parse_input(&frame).is_err());
    assert!(parse_input(&header).is_err());
    assert!(parse_input(br#"{"op":"target","slug":"app"}"#)
        .unwrap()
        .bundle
        .is_none());
    let bad = json!({"op":"deploy","slug":"app","celld_version":"0.6.1","version_id":"x","bundle_size":celld_ctl_core::MAX_BUNDLE_BYTES+1});
    assert!(parse_input(&serde_json::to_vec(&bad).unwrap()).is_err());
    let mut bad = serde_json::to_value(deploy_request(&bundle)).unwrap();
    bad["bucket"] = json!("s3://attacker");
    assert!(parse_input(&serde_json::to_vec(&bad).unwrap()).is_err());
}

#[test]
fn ssh_status_uses_minimal_target_but_operator_status_retains_registry_details() {
    let f = Fixture::new();
    let mut m = f.manager();
    m.provision("app").unwrap();
    assert_eq!(
        m.status("app").unwrap()["target"]
            .as_object()
            .unwrap()
            .len(),
        6
    );
    let status = m
        .transport_request(Request::Status { slug: "app".into() }, None)
        .unwrap();
    assert_eq!(status["target"].as_object().unwrap().len(), 3);
    assert!(status["target"].get("bucket").is_none());
}

#[test]
fn pathological_directory_expansion_is_rejected_before_staging_or_native_commands() {
    let f = Fixture::new();
    let mut m = f.manager();
    m.provision("app").unwrap();
    m.runtime.calls.clear();
    let mut bundle = prepared();
    bundle.config["assets"] = json!({"directory":"assets"});
    bundle.assets = (0..celld_ctl_core::MAX_UPLOAD_FILES - 1)
        .map(|i| celld_ctl_core::UploadFile {
            path: format!("d{i}/inner/deeper/file"),
            content: String::new(),
        })
        .collect();
    let request = deploy_request(&bundle);
    let body = serde_json::to_vec(&bundle).unwrap();
    let mut frame = serde_json::to_vec(&request).unwrap();
    frame.push(b'\n');
    frame.extend(body);
    assert!(celld_ctl::transport::parse_input(&frame)
        .unwrap_err()
        .to_string()
        .contains("8192 staging directories"));
    assert!(m
        .deploy(request, bundle)
        .unwrap_err()
        .to_string()
        .contains("8192 staging directories"));
    assert!(m.runtime.calls.is_empty());
    assert!(!f.paths.state.join("staging").exists());
}
