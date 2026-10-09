# Host operations

For installation, see [host setup](setup-host.md). For deployment from your
machine, see [cella](cella.md).

## Manage apps

`cella deploy` creates and activates apps automatically. On the host:

```sh
sudo celld-ctl list
sudo celld-ctl status APP_SLUG
sudo celld-ctl logs APP_SLUG
sudo celld-ctl restart APP_SLUG
sudo celld-ctl disable APP_SLUG
sudo celld-ctl enable APP_SLUG
```

Disable removes the route and stops the app. Enable requires a published
deployment. Do not delete object-store data when removing an app.

To roll back code, check out an earlier revision and redeploy. This does not
undo data migrations. If a deploy fails, check status and logs before retrying:
the published version may already be running.

## Troubleshooting

```sh
sudo systemctl status celld-cell@APP_SLUG
sudo journalctl -u celld-cell@APP_SLUG -n 100
sudo systemctl status caddy
sudo ss -ltnp
# For local storage:
sudo systemctl status rustfs.service
sudo journalctl -u rustfs.service -n 100
```

Check disk space, permissions and private listeners. RustFS should listen on
`127.0.0.1:9000`; native celld and Caddy's admin listener must not be public.
Do not print credentials in logs.

During a storage outage, do not replace missing credentials or initialize an
apparently empty bucket. After recovery, verify a known Durable Object's saved
state; an HTTP success alone does not prove persistence.

## Backup and recovery

```sh
sudo celld-ctl backup
```

This saves registry and configuration under `/var/lib/celld-ctl/backups`.
Only backups with a `COMPLETE` marker are complete. They contain credentials:
keep secure copies off-host.

**This command does not back up object-store data.**

- Local RustFS data in `/var/lib/rustfs` is authoritative.
- `/var/lib/celld/APP_SLUG` is a runtime cache, not a data backup.
- External storage needs its own bucket backup and recovery plan.

For a consistent local snapshot:

1. Block deployments and app traffic; stop apps, then RustFS.
2. Capture RustFS data, registry, host configuration, credentials, app units
   and installed-release metadata together, preserving ownership and permissions.
3. Keep a secure off-host copy.
4. Start RustFS and check readiness, then start apps and reopen traffic.

A live recursive copy is not a consistent snapshot. Restore on an isolated
host with writers stopped and matching credentials and version pins. Start
storage before apps, and verify a known Durable Object's state before reopening
traffic. Practice recovery on a disposable host before relying on it; local
storage is not a guarantee of production durability.

## Updates and storage changes

Back up before updating and test rollback/recovery on a disposable host.
Reinstallation is not a reset, a storage migration or an automatic app-version
upgrade.

Changing storage requires stopping writers, copying complete object namespaces,
updating app targets and validating recovery. A live bucket sync is not enough.
Do not clear only the bucket or only the registry; they must remain consistent.
