use crate::config::Paths;
use anyhow::{ensure, Context, Result};
use celld_ctl_core::{Deployment, Target, MAX_HISTORY};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::os::unix::fs::PermissionsExt;

/// exe.dev forwards owner-authenticated alternate ports 3000..=9999. Keep
/// the public Caddy listener separate from the loopback native listener.
pub const PUBLIC_PORT_OFFSET: u16 = 1000;

#[derive(Debug, Clone, Serialize)]
pub struct App {
    #[serde(flatten)]
    pub target: Target,
    pub port: u16,
    pub internal_port: u16,
    pub unit: String,
    pub legacy: bool,
    pub version_id: Option<String>,
}
impl App {
    pub fn public_port(&self) -> u16 {
        // validate() requires port <= 8999; registry callers validate rows.
        self.port + PUBLIC_PORT_OFFSET
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            celld_ctl_core::valid_slug(&self.target.slug)
                && celld_ctl_core::valid_version(&self.target.celld_version),
            "invalid registry identity"
        );
        ensure!(
            self.unit == format!("celld-cell@{}.service", self.target.slug)
                || (self.legacy
                    && self.target.slug == "counter"
                    && self.unit == "celld-counter.service"),
            "invalid registry unit"
        );
        ensure!(
            (3000..=8999).contains(&self.port)
                && self.internal_port > 1024
                && self.port != self.internal_port
                && self.port != 8000
                && self.internal_port != 8000
                && self.internal_port != self.public_port(),
            "invalid registry ports"
        );
        Ok(())
    }
}

pub struct Registry {
    pub conn: Connection,
}
impl Registry {
    pub fn open(paths: &Paths) -> Result<Self> {
        let conn = Connection::open(&paths.registry)?;
        std::fs::set_permissions(&paths.registry, std::fs::Permissions::from_mode(0o600))?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch(
            "PRAGMA foreign_keys=ON; PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL;",
        )?;
        conn.execute_batch(include_str!("../migrations/0001.sql"))?;
        let version: i64 =
            conn.query_row("SELECT version FROM schema_version", [], |r| r.get(0))?;
        ensure!(version == 1, "unsupported registry schema");
        Ok(Self { conn })
    }
    fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<App> {
        Ok(App {
            target: Target {
                slug: r.get(0)?,
                bucket: r.get(1)?,
                endpoint: r.get(2)?,
                region: r.get(3)?,
                celld_version: r.get(4)?,
                enabled: r.get(5)?,
            },
            port: r.get(6)?,
            internal_port: r.get(7)?,
            unit: r.get(8)?,
            legacy: r.get(9)?,
            version_id: r.get(10)?,
        })
    }
    pub fn get(&self, slug: &str) -> Result<App> {
        let a = self
            .find(slug)?
            .context("unknown app; provision it first")?;
        a.validate()?;
        Ok(a)
    }
    pub fn find(&self, slug: &str) -> Result<Option<App>> {
        Ok(self.conn.query_row("SELECT slug,bucket,endpoint,region,celld_version,enabled,port,internal_port,unit,legacy,version_id FROM apps WHERE slug=?", [slug], Self::row).optional()?)
    }
    pub fn list(&self) -> Result<Vec<App>> {
        let mut s = self.conn.prepare("SELECT slug,bucket,endpoint,region,celld_version,enabled,port,internal_port,unit,legacy,version_id FROM apps ORDER BY slug")?;
        let rows = s
            .query_map([], Self::row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for a in &rows {
            a.validate()?;
        }
        Ok(rows)
    }
    pub fn insert(&self, a: &App) -> Result<()> {
        a.validate()?;
        self.conn.execute("INSERT INTO apps(slug,bucket,endpoint,region,celld_version,enabled,port,internal_port,unit,legacy,version_id) VALUES(?,?,?,?,?,?,?,?,?,?,?)", params![a.target.slug,a.target.bucket,a.target.endpoint,a.target.region,a.target.celld_version,a.target.enabled,a.port,a.internal_port,a.unit,a.legacy,a.version_id])?;
        Ok(())
    }
    pub fn enabled(&self, slug: &str, enabled: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE apps SET enabled=? WHERE slug=?",
            params![enabled, slug],
        )?;
        Ok(())
    }
    pub fn record(&mut self, slug: &str, version: &str, revision: Option<&str>) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "UPDATE apps SET version_id=? WHERE slug=?",
            params![version, slug],
        )?;
        tx.execute(
            "INSERT INTO deployments(slug,version_id,source_revision) VALUES(?,?,?)",
            params![slug, version, revision],
        )?;
        tx.commit()?;
        Ok(())
    }
    pub fn history(&self, slug: &str) -> Result<Vec<Deployment>> {
        let mut s = self.conn.prepare("SELECT id,slug,version_id,source_revision,deployed_at FROM deployments WHERE slug=? ORDER BY id DESC LIMIT ?")?;
        let rows = s
            .query_map(params![slug, MAX_HISTORY], |r| {
                Ok(Deployment {
                    id: r.get(0)?,
                    slug: r.get(1)?,
                    version_id: r.get(2)?,
                    source_revision: r.get(3)?,
                    deployed_at: r.get(4)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }
    pub fn audit(&self, slug: &str, operation: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO audit(slug,operation) VALUES(?,?)",
            params![slug, operation],
        )?;
        Ok(())
    }
}
