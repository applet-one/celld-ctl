CREATE TABLE IF NOT EXISTS schema_version(version INTEGER NOT NULL);
INSERT INTO schema_version SELECT 1 WHERE NOT EXISTS(SELECT 1 FROM schema_version);
CREATE TABLE IF NOT EXISTS apps(
slug TEXT PRIMARY KEY, bucket TEXT NOT NULL UNIQUE, endpoint TEXT NOT NULL, region TEXT NOT NULL,
celld_version TEXT NOT NULL, enabled INTEGER NOT NULL DEFAULT 0 CHECK(enabled IN (0,1)),
port INTEGER NOT NULL UNIQUE, internal_port INTEGER NOT NULL UNIQUE, unit TEXT NOT NULL UNIQUE,
legacy INTEGER NOT NULL DEFAULT 0, version_id TEXT,
created_at TEXT NOT NULL DEFAULT(strftime('%Y-%m-%dT%H:%M:%SZ','now')));
CREATE TABLE IF NOT EXISTS deployments(
id INTEGER PRIMARY KEY, slug TEXT NOT NULL, version_id TEXT NOT NULL, source_revision TEXT,
deployed_at TEXT NOT NULL DEFAULT(strftime('%Y-%m-%dT%H:%M:%SZ','now')));
CREATE INDEX IF NOT EXISTS deployments_slug ON deployments(slug,id DESC);
CREATE TABLE IF NOT EXISTS audit(
id INTEGER PRIMARY KEY, slug TEXT NOT NULL, operation TEXT NOT NULL,
at TEXT NOT NULL DEFAULT(strftime('%Y-%m-%dT%H:%M:%SZ','now')));
