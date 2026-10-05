use crate::{
    config::{atomic_write, Config, Paths},
    registry::{App, Registry},
    render,
    runtime::Runtime,
};
use anyhow::{bail, ensure, Context, Result};
use celld_ctl_core::{DeployTarget, PreparedBundle, Request, Target};
use fs2::FileExt;
use serde_json::{json, Value};
use std::{
    fs::{self, File, OpenOptions},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::PathBuf,
    time::{Duration, Instant},
};

/// Holding this manager holds the host operation lock, including external effects.
pub struct Manager<R: Runtime> {
    pub config: Config,
    pub paths: Paths,
    pub registry: Registry,
    pub runtime: R,
    _lock: File,
}
impl<R: Runtime> Manager<R> {
    pub fn open(paths: Paths, runtime: R) -> Result<Self> {
        let config = Config::load(&paths)?;
        paths.initialize()?;
        let lock_path = paths.state.join("operation.lock");
        if lock_path.symlink_metadata().is_ok() {
            paths.check_file(&lock_path, true)?;
        }
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(lock_path)?;
        let start = Instant::now();
        loop {
            match lock.try_lock_exclusive() {
                Ok(()) => break,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    ensure!(
                        start.elapsed() < Duration::from_secs(10),
                        "host busy; retry later"
                    );
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(e) => return Err(e.into()),
            }
        }
        let registry = Registry::open(&paths)?;
        Ok(Self {
            config,
            paths,
            registry,
            runtime,
            _lock: lock,
        })
    }
    pub fn request(&mut self, request: Request) -> Result<Value> {
        request.validate().map_err(anyhow::Error::msg)?;
        match request {
            Request::Provision { slug } => Ok(serde_json::to_value(self.provision(&slug)?)?),
            Request::Target { slug } => Ok(serde_json::to_value(self.registry.get(&slug)?.target)?),
            Request::Activate {
                slug,
                version_id,
                source_revision,
            } => self.activate(&slug, Some(&version_id), source_revision.as_deref(), false),
            Request::Deploy { .. } => bail!("deploy requires a framed prepared bundle"),
            Request::Status { slug } => self.status(&slug),
            Request::Logs { slug, lines } => {
                let app = self.registry.get(&slug)?;
                Ok(json!({"text":self.runtime.logs(&app,lines)?}))
            }
            Request::Deployments { slug } => {
                self.registry.get(&slug)?;
                Ok(serde_json::to_value(self.registry.history(&slug)?)?)
            }
        }
    }
    /// The SSH boundary never exposes storage configuration or accepts storage paths.
    pub fn transport_request(
        &mut self,
        request: Request,
        bundle: Option<PreparedBundle>,
    ) -> Result<Value> {
        request.validate().map_err(anyhow::Error::msg)?;
        if matches!(request, Request::Deploy { .. }) {
            return self.deploy(
                request,
                bundle.context("deploy requires a prepared bundle")?,
            );
        }
        ensure!(bundle.is_none(), "only deploy accepts a bundle");
        match request {
            Request::Provision { slug } => Ok(serde_json::to_value(DeployTarget::from(
                &self.provision(&slug)?,
            ))?),
            Request::Target { slug } => Ok(serde_json::to_value(DeployTarget::from(
                &self.registry.get(&slug)?.target,
            ))?),
            Request::Status { slug } => {
                let mut status = self.status(&slug)?;
                status["target"] =
                    serde_json::to_value(DeployTarget::from(&self.registry.get(&slug)?.target))?;
                Ok(status)
            }
            other => self.request(other),
        }
    }
    pub fn deploy(&mut self, request: Request, bundle: PreparedBundle) -> Result<Value> {
        request.validate().map_err(anyhow::Error::msg)?;
        let Request::Deploy {
            slug,
            celld_version,
            version_id,
            source_revision,
            ..
        } = request
        else {
            bail!("expected deploy metadata");
        };
        let app = self.registry.get(&slug)?;
        ensure!(!app.legacy,"SSH publish is disabled for the legacy bucket-root fleet; use an explicit root-operator workflow");
        ensure!(
            celld_version == app.target.celld_version,
            "prepared bundle celld version does not match the target pin"
        );
        let bundle = bundle.normalize().map_err(anyhow::Error::msg)?;
        self.pin(&app)?;
        let native = self.runtime.deploy(&app, &bundle, &version_id)?;
        let activation=self.activate(&slug,Some(&version_id),source_revision.as_deref(),false)
            .map_err(|e|anyhow::anyhow!("native version {version_id} was published but activation failed: {e}; retry this exact deployment or ask the operator to enable/reload {slug}"))?;
        let mut result = activation
            .as_object()
            .context("invalid activation result")?
            .clone();
        result.insert(
            "source_revision".into(),
            serde_json::to_value(source_revision)?,
        );
        result.insert("native_output".into(), native.native_output);
        result.insert("native_stderr".into(), Value::String(native.native_stderr));
        Ok(Value::Object(result))
    }
    fn check_slug(slug: &str) -> Result<()> {
        ensure!(celld_ctl_core::valid_slug(slug), "invalid slug");
        Ok(())
    }
    fn pin(&mut self, app: &App) -> Result<()> {
        let binary = self
            .paths
            .releases
            .join(format!("v{}/celld", app.target.celld_version));
        self.paths
            .check_file(&binary, false)
            .context("pinned celld release is not installed securely")?;
        let metadata = fs::metadata(&binary)?;
        ensure!(
            metadata.permissions().mode() & 0o111 != 0,
            "pinned celld binary is not executable"
        );
        self.runtime
            .verify_binary(&binary, &app.target.celld_version)
    }
    fn inputs(&mut self, app: &App) -> Result<()> {
        self.pin(app)?;
        self.paths.check_file(&self.paths.credentials, true)?;
        let env = self.paths.cells.join(format!("{}.env", app.target.slug));
        atomic_write(
            &self.paths,
            &env,
            render::environment(app, &self.paths).as_bytes(),
            0o600,
        )?;
        let directory = self.paths.units.join(format!("{}.d", app.unit));
        self.paths.make_dir(&directory, 0o755)?;
        atomic_write(
            &self.paths,
            &directory.join("50-celld-ctl.conf"),
            render::unit_override(app, &self.paths).as_bytes(),
            0o644,
        )?;
        self.runtime.systemctl("daemon-reload", None)
    }
    pub fn provision(&mut self, slug: &str) -> Result<Target> {
        Self::check_slug(slug)?;
        if let Some(app) = self.registry.find(slug)? {
            app.validate()?;
            return Ok(app.target);
        }
        let apps = self.registry.list()?;
        ensure!(apps.len() < 500, "host application limit reached");
        let mut ports = None;
        for (port, internal) in (self.config.port_start..=self.config.port_end)
            .zip(self.config.internal_port_start..=self.config.internal_port_end)
        {
            if apps.iter().all(|a| {
                ![a.port, a.internal_port].contains(&port)
                    && ![a.port, a.internal_port].contains(&internal)
            }) && self.runtime.port_free(port)
                && self.runtime.port_free(internal)
            {
                ports = Some((port, internal));
                break;
            }
        }
        let (port, internal_port) = ports.context("no free app port pair")?;
        let app = App {
            target: Target {
                slug: slug.into(),
                bucket: format!("{}/cells/{slug}", self.config.bucket),
                endpoint: self.config.endpoint.clone(),
                region: self.config.region.clone(),
                celld_version: self.config.celld_version.clone(),
                enabled: false,
            },
            port,
            internal_port,
            unit: format!("celld-cell@{slug}.service"),
            legacy: false,
            version_id: None,
        };
        // No fleet request, service start, or route publication during provisioning.
        self.inputs(&app)?;
        self.registry.insert(&app)?;
        self.registry.audit(slug, "provision")?;
        self.write_status()?;
        Ok(app.target)
    }
    /// Import the existing bucket-root fleet, never migrate/rewrite its object-store data.
    pub fn import_counter(
        &mut self,
        version: &str,
        expected: Option<&str>,
        enabled: bool,
    ) -> Result<Value> {
        ensure!(
            celld_ctl_core::valid_version(version),
            "invalid celld version"
        );
        ensure!(
            self.registry.find("counter")?.is_none(),
            "counter already registered"
        );
        let apps = self.registry.list()?;
        ensure!(
            apps.iter()
                .all(|a| ![a.port, a.internal_port].contains(&8100)
                    && ![a.port, a.internal_port].contains(&18100)),
            "legacy ports already allocated"
        );
        let app = App {
            target: Target {
                slug: "counter".into(),
                bucket: self.config.bucket.clone(),
                endpoint: self.config.endpoint.clone(),
                region: self.config.region.clone(),
                celld_version: version.into(),
                enabled: false,
            },
            port: 8100,
            internal_port: 18100,
            unit: "celld-counter.service".into(),
            legacy: true,
            version_id: None,
        };
        let deployed = self.runtime.verify_pointer(&app, expected)?;
        if enabled {
            self.runtime
                .readiness(&app, &deployed, self.config.readiness_timeout_secs)?;
        }
        self.inputs(&app)?;
        self.registry.insert(&app)?;
        // Withdraw the previously unmanaged legacy route before changing its runtime.
        self.sync_routes()?;
        self.registry.audit("counter", "import")?;
        if enabled {
            self.activate("counter", Some(&deployed), None, true)
        } else {
            self.runtime.systemctl("disable", Some(&app.unit))?;
            self.runtime.systemctl("stop", Some(&app.unit))?;
            self.registry.record("counter", &deployed, None)?;
            self.write_status()?;
            self.status("counter")
        }
    }
    fn write_status(&self) -> Result<()> {
        atomic_write(
            &self.paths,
            &self.paths.public.join("index.html"),
            render::html(&self.registry.list()?).as_bytes(),
            0o644,
        )
    }
    /// Validate a separate candidate before replacing; roll back disk state on reload failure.
    fn sync_routes(&mut self) -> Result<()> {
        let rendered = render::caddy(&self.registry.list()?, &self.paths);
        let candidate = self.paths.caddy.with_file_name(".celld-ctl-candidate");
        atomic_write(&self.paths, &candidate, rendered.as_bytes(), 0o644)?;
        let validation = self.runtime.validate_caddy(&candidate);
        if validation.is_err() {
            let _ = fs::remove_file(&candidate);
            return validation;
        }
        let old = match fs::read(&self.paths.caddy) {
            Ok(x) => Some(x),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.into()),
        };
        atomic_write(&self.paths, &self.paths.caddy, rendered.as_bytes(), 0o644)?;
        // Status rendering happens before the authoritative Caddy reload: no fallible
        // status writes may turn a committed route into a reported rollback.
        if let Err(e) = self.write_status() {
            if let Some(ref old) = old {
                atomic_write(&self.paths, &self.paths.caddy, old, 0o644)?;
            } else {
                let _ = fs::remove_file(&self.paths.caddy);
            }
            let _ = fs::remove_file(&candidate);
            return Err(e);
        }
        let result = self.runtime.reload_caddy(&self.paths.caddy);
        let _ = fs::remove_file(&candidate);
        if let Err(error) = result {
            if let Some(old) = old {
                atomic_write(&self.paths, &self.paths.caddy, &old, 0o644)?;
                if self.runtime.reload_caddy(&self.paths.caddy).is_err() {
                    let _ = self.runtime.systemctl("stop", Some("caddy.service"));
                }
            } else {
                let _ = fs::remove_file(&self.paths.caddy);
                let _ = self.runtime.systemctl("stop", Some("caddy.service"));
            }
            return Err(error.context("Caddy reload failed; configuration restored"));
        }
        Ok(())
    }
    /// SQLite and Caddy cannot share a transaction. Reapply the desired route
    /// state even when SQL already says disabled: a crash may have happened
    /// between the registry update and the external reload. After interruption,
    /// rerunning disable/stop/activation reconciles this known slug's routes.
    fn unpublish(&mut self, slug: &str) -> Result<()> {
        let was_enabled = self.registry.get(slug)?.target.enabled;
        self.registry.enabled(slug, false)?;
        if let Err(e) = self.sync_routes() {
            self.registry.enabled(slug, was_enabled)?;
            return Err(e);
        }
        Ok(())
    }
    fn publish(&mut self, slug: &str) -> Result<()> {
        self.registry.enabled(slug, true)?;
        if let Err(e) = self.sync_routes() {
            self.registry.enabled(slug, false)?;
            let _ = self.write_status();
            return Err(e);
        }
        Ok(())
    }
    /// Native deploy must be complete; check durable pointer before starting a new fleet.
    pub fn activate(
        &mut self,
        slug: &str,
        expected: Option<&str>,
        revision: Option<&str>,
        restart: bool,
    ) -> Result<Value> {
        Self::check_slug(slug)?;
        let app = self.registry.get(slug)?;
        let version = self.runtime.verify_pointer(&app, expected)?;
        self.pin(&app)?;
        self.unpublish(slug)?;
        // Failures after route withdrawal cannot publish an unverified deployment.
        let result = (|| {
            self.inputs(&app)?;
            if restart {
                self.runtime.systemctl("restart", Some(&app.unit))?;
            } else if self.runtime.active(&app)? {
                self.runtime.reload_app(&app)?;
            } else {
                self.runtime.systemctl("start", Some(&app.unit))?;
            }
            self.runtime
                .readiness(&app, &version, self.config.readiness_timeout_secs)?;
            // Catch concurrent native publishers before calling this a verified activation.
            self.runtime.verify_pointer(&app, Some(&version))?;
            self.runtime.systemctl("enable", Some(&app.unit))?;
            self.publish(slug)?;
            if expected.is_some() || app.version_id.as_deref() != Some(&version) {
                self.registry.record(slug, &version, revision)?;
            }
            self.registry.audit(slug, "activate")?;
            // A stale read-only page is not an activation failure after commit.
            let _ = self.write_status();
            Ok(())
        })();
        if let Err(e) = result {
            // If a post-publication registry/audit operation failed, withdraw again.
            if self.unpublish(slug).is_err() {
                let _ = self.runtime.systemctl("stop", Some("caddy.service"));
            }
            self.registry.enabled(slug, false)?;
            // Keep provisioned first deployments disabled at boot after failure.
            if app.version_id.is_none() {
                let _ = self.runtime.systemctl("disable", Some(&app.unit));
                let _ = self.runtime.systemctl("stop", Some(&app.unit));
            }
            let _ = self.registry.audit(slug, "activation-failed");
            let _ = self.write_status();
            return Err(e);
        }
        Ok(json!({"slug":slug,"version_id":version,"enabled":true}))
    }
    pub fn lifecycle(&mut self, operation: &str, slug: &str) -> Result<Value> {
        Self::check_slug(slug)?;
        match operation {
            "enable" | "start" | "reload" => self.activate(slug, None, None, false),
            "restart" => self.activate(slug, None, None, true),
            "disable" | "stop" => {
                let app = self.registry.get(slug)?;
                self.unpublish(slug)?;
                if operation == "disable" {
                    self.runtime.systemctl("disable", Some(&app.unit))?;
                }
                self.runtime.systemctl("stop", Some(&app.unit))?;
                self.registry.audit(slug, operation)?;
                self.write_status()?;
                self.status(slug)
            }
            "remove" => {
                let app = self.registry.get(slug)?;
                self.unpublish(slug)?;
                self.runtime.systemctl("disable", Some(&app.unit))?;
                self.runtime.systemctl("stop", Some(&app.unit))?;
                // Remove only our generated inputs. Retain cache/state, history and ALL R2 data.
                for path in [
                    self.paths.cells.join(format!("{slug}.env")),
                    self.paths
                        .units
                        .join(format!("{}.d/50-celld-ctl.conf", app.unit)),
                ] {
                    if path.exists() {
                        self.paths.check_file(&path, false)?;
                        fs::remove_file(path)?;
                    }
                }
                self.runtime.systemctl("daemon-reload", None)?;
                self.registry
                    .conn
                    .execute("DELETE FROM apps WHERE slug=?", [slug])?;
                self.registry.audit(slug, "remove")?;
                self.write_status()?;
                Ok(
                    json!({"slug":slug,"removed":true,"durable_data_deleted":false,"local_state_deleted":false}),
                )
            }
            _ => bail!("unknown lifecycle operation"),
        }
    }
    pub fn status(&mut self, slug: &str) -> Result<Value> {
        let app = self.registry.get(slug)?;
        let active = self.runtime.active(&app)?;
        let observed = if active {
            self.runtime.observed_version(&app).ok().flatten()
        } else {
            None
        };
        Ok(
            json!({"target":app.target,"active":active,"version_id":app.version_id,"observed_version_id":observed,"unit":app.unit,"port":app.port,"internal_port":app.internal_port}),
        )
    }
    /// Online SQLite backup API gives a consistent snapshot; credentials never enter SQLite.
    pub fn backup(&mut self) -> Result<Value> {
        let root = self.paths.state.join("backups");
        self.paths.make_dir(&root, 0o700)?;
        let name = format!(
            "{}-{}",
            chrono::Utc::now().format("%Y%m%dT%H%M%S%.9fZ"),
            std::process::id()
        );
        let dest = root.join(name);
        fs::create_dir(&dest)?;
        fs::set_permissions(&dest, fs::Permissions::from_mode(0o700))?;
        self.registry.conn.backup(
            rusqlite::DatabaseName::Main,
            dest.join("registry.sqlite"),
            None,
        )?;
        fs::set_permissions(
            dest.join("registry.sqlite"),
            fs::Permissions::from_mode(0o600),
        )?;
        for directory in ["config", "cells", "units"] {
            self.paths.make_dir(&dest.join(directory), 0o700)?;
        }
        let mut files = vec![
            (
                self.paths.config.clone(),
                PathBuf::from("config/config.json"),
            ),
            (
                self.paths.credentials.clone(),
                PathBuf::from("config/node.env"),
            ),
        ];
        if self.paths.caddy.exists() {
            files.push((self.paths.caddy.clone(), PathBuf::from("config/Caddyfile")));
        }
        let apps = self.registry.list()?;
        for app in &apps {
            files.push((
                self.paths.cells.join(format!("{}.env", app.target.slug)),
                PathBuf::from(format!("cells/{}.env", app.target.slug)),
            ));
            files.push((
                self.paths
                    .units
                    .join(format!("{}.d/50-celld-ctl.conf", app.unit)),
                PathBuf::from(format!("units/{}.override.conf", app.target.slug)),
            ));
            if app.legacy {
                let base = self.paths.units.join(&app.unit);
                if base.exists() {
                    files.push((base, PathBuf::from(format!("units/{}", app.unit))));
                }
            }
        }
        let template = self.paths.units.join("celld-cell@.service");
        if template.exists() {
            files.push((template, PathBuf::from("units/celld-cell@.service")));
        }
        for (source, name) in files {
            self.paths.check_file(
                &source,
                source == self.paths.credentials || source == self.paths.config,
            )?;
            let data = fs::read(source)?;
            atomic_write(&self.paths, &dest.join(name), &data, 0o600)?;
        }
        let pins=apps.iter().map(|app|json!({"slug":app.target.slug,"celld_version":app.target.celld_version,"binary":format!("{}/v{}/celld",self.paths.releases.display(),app.target.celld_version)})).collect::<Vec<_>>();
        atomic_write(
            &self.paths,
            &dest.join("config/pins.json"),
            &serde_json::to_vec_pretty(&pins)?,
            0o600,
        )?;
        atomic_write(&self.paths,&dest.join("COMPLETE"),b"Registry/configuration snapshot. Contains credentials. Preserve root ownership and private modes.\n",0o600)?;
        self.registry.audit("", "backup")?;
        Ok(json!({"backup":dest,"contains_credentials":true}))
    }
}
