# Host operations, migration and recovery

Start with [host setup](setup-host.md); the owner follows [client SSH
setup](cella.md#owner-ssh-setup). This single-host trusted-owner system has no
scoped CI/developer deploy identity or runtime app-secret manager. Keep object
storage, internal listeners and root-only credentials private. The Caddy
listeners expect a trusted HTTPS and access-control proxy on matching public
ports: exe.dev supplies one, but other hosts must configure their own. Do not
expose Caddy as unauthenticated public HTTP.

## Historical storage gate (October 5, 2026)

The recorded minimum live test passed on **x86_64 Ubuntu 24.04 in a systemd
Docker container**, using pinned RustFS 1.0.1 and native celld 0.6.1. It used
the **old restricted deployment SSH service**, not today's owner-SSH transport.
A named Durable Object retained counts across an app restart, a controlled
RustFS stop/restart, and archiving **only** its `/var/lib/celld/APP_SLUG` cache;
a no-flag reinstall preserved credentials and registry. Storage unavailability
caused a diagnostic failure, followed by recovery after a controlled restart.
This is limited container evidence for fresh x86_64 single-node
development/testing. It does **not** qualify a VM, arm64, abrupt-kill/reboot,
cold-snapshot restore, competing-writer races, physical power-loss survival,
production durability or HA. The current owner-SSH workflow requires its own
checks; do not present the historical restricted-SSH deploy as its test.

## Lifecycle and routing

```sh
sudo celld-ctl create APP_SLUG
sudo celld-ctl target APP_SLUG
# Publish with native celld or cella before enabling.
sudo celld-ctl enable APP_SLUG
sudo celld-ctl list
sudo celld-ctl status APP_SLUG
sudo celld-ctl logs APP_SLUG
sudo celld-ctl restart APP_SLUG
sudo celld-ctl disable APP_SLUG
```

`cella deploy` provisions and activates automatically. Registry state lives
in `/var/lib/celld-ctl/registry.sqlite`, app environment files in
`/etc/celld/cells`, and app caches in `/var/lib/celld/APP_SLUG`. Never store
them in this checkout. SQLite allocates runtime ports 8101–8999; Caddy exposes
each active app on its allocated port + 1000 (9101–9999), preserving paths
at `/`. Port 8000 is a read-only directory of links, not a path router.
Caddy's private admin listener is `127.0.0.1:2019`. Enable verifies the
durable deployment pointer and runtime readiness before publishing the route.
Disable unpublishes and stops the unit. Removing an app must **not** delete
object-store data; clean up retained local cache only after operator review.

## Owner SSH transport and old-service migration

`cella` uses an **owner** SSH key (registered with exe.dev on that platform,
or authorized for the account on another VM), strict VM host-key checking,
`-F /dev/null`, no forwarding/PTY and the fixed command
`sudo -n /usr/local/bin/celld-ctl transport`. SSH defaults to port 22;
`CELLA_SSH_PORT` permits a different port. The owner needs non-interactive
sudo for that command. The transport bounds its request and permits only
provision, target, deploy, status, logs and deployments; it never
selects a bucket, executable or arbitrary host path. Host credentials are
root-only. **This does not restrict ordinary owner SSH/sudo**. Never give an
owner key to CI or untrusted users. Revoke a compromised key through your
provider or host account (on exe.dev, use its account key commands), considering
other keys and sessions. For authorization and independently verified VM
fingerprints, follow [client SSH setup](cella.md#owner-ssh-setup). Do not alter
a platform-managed primary SSH service.

For hosts **previously using `cella-sshd` only**: make a
[backup](#backup-and-recovery), keep an off-host copy, reinstall current host
binaries, verify direct owner SSH and `sudo -n /usr/local/bin/celld-ctl
transport`, then test read-only `cella status` using the new client. Do not
remove the only working access path until owner access works. The installer
does not remove the old SSH artifacts. Once verified, inspect and stop only
the old dedicated service, then remove its managed files if present:

```sh
sudo systemctl disable --now cella-sshd.service
sudo rm -f /etc/systemd/system/cella-sshd.service \
  /etc/celld-ctl/sshd_config \
  /etc/celld-ctl/ssh-host-ed25519-key /etc/celld-ctl/ssh-host-ed25519-key.pub \
  /etc/sudoers.d/cella-deploy \
  /etc/ssh/sshd_config.d/60-cella-deploy.conf
sudo systemctl daemon-reload
```

Also inspect `/etc/ssh/cella-deploy/authorized_keys`,
`/var/empty/cella-deploy`, `/usr/local/bin/celld-deploy-key` and the old
`cella-deploy` account; remove manually **only if unused elsewhere**. Remove
obsolete port-2222 relays and reviewed stale `known_hosts` entries; replace
old `CELLA_HOST=cella-deploy@...`/`CELLA_SSH_PORT=2222`/key settings with the
owner destination and key. Do not delete registry, objects, app units,
credentials, primary SSH configuration or backups. Reinstallation is not a
reset or automatic storage migration.

## Legacy bucket-root counter migration

An imported counter retains its original bucket-root fleet prefix: **do not**
redeploy it into `/cells/counter`. Back up its unit, root-only environment and
local state. Gracefully stop the old service; copy its cache to
`/var/lib/celld/counter`, give the service account ownership and set
`CELLD_WATCH` to that path. Keep credentials root-owned in systemd's
EnvironmentFile. Start the *same* pinned celld version with loopback serving
`127.0.0.1:8100` and internal `127.0.0.1:18100` as the starting point. This
special import uses dedicated Caddy port **9100**, outside new app ranges;
check availability and remove the old path route. Import its existing native
version ID with `import-counter`, then verify the port-8000 link and app
listener. Check a **known named Durable Object** before and after restart;
an HTTP 200 alone does not prove persistence. Keep backups outside the repo.
Migration does not repair unrelated pre-existing fleet ownership errors.

## Backup and recovery

Successful activation records native deployment ID and source revision. To
roll back code, check out the previous revision and redeploy; this does not
undo Durable Object data or schema changes. `sudo celld-ctl backup` writes
registry and root-only configuration under `/var/lib/celld-ctl/backups`.
Only snapshots with a `COMPLETE` marker are complete; config, app environments
and units have separate subdirectories. Store secure copies off-host: they
contain credentials. Preserve ownership/modes when restoring with services
stopped, verify the exact pinned binaries and Caddy config before starting.

**`celld-ctl backup` excludes object-store data.** On a local RustFS host,
`/var/lib/rustfs` holds authoritative application data;
`/var/lib/celld/APP_SLUG` is a disposable *cache*, not the durable store.
Losing the VM/storage volume may lose acknowledged writes. If VM-loss recovery
matters, maintain a separate off-VM storage snapshot. On external storage,
keep the bucket data and its own backup/recovery plan independently.

For local service checks inspect `systemctl status rustfs.service`,
`journalctl -u rustfs.service`, `ss -ltnp` (S3 only on `127.0.0.1:9000`),
disk space and permissions of `/var/lib/rustfs`, `/etc/rustfs/rustfs.env`,
`/etc/celld/node.env` and `/etc/celld-ctl/config.json`. Do not print secrets
in logs. After an app and RustFS restart, read a known named object; a cache
hit or HTTP success alone is insufficient. Never replace missing credentials
or initialize an apparently empty bucket during an outage.

For an operator-controlled **cold snapshot**, block deployments/writers; stop
affected apps and then RustFS. Capture `/var/lib/rustfs`, RustFS service
environment, registry, host config, node credentials and installed-release
metadata with ownership/modes intact. Copy off-host when VM loss is in scope.
Restart RustFS first, check readiness, then restart apps and reopen writers.
A live recursive copy is **not** a consistent snapshot. Restore onto an
isolated host with stopped writers and matching credentials/pins; verify a
known named object's state before reopening service. Cold restore was **not**
tested in the [October 5 container gate](#historical-storage-gate-october-5-2026):
practice on a disposable host before relying on it.

There is no remote reset. On a disposable host, stop writers and services,
archive registry, config, credentials and RustFS data together and use an
operator-reviewed whole-instance reset; never clear just the bucket or just
the registry while leaving incompatible references. Reinstall does not reset.
Changing the storage default only affects new apps: local↔external migration
requires quiescing writers, copying complete object namespaces, updating
persisted app targets/generated input and validating recovery. Native leases
and sessions need a separate reviewed plan; do not treat a live bucket sync as
sufficient. Protect legacy bucket-root imports. For upgrades, pin and verify
new native/RustFS artifacts, take a cold snapshot and test rollback/recovery
on a disposable host; do not rotate credentials or app pins through routine
reinstallation.

Capacity qualification is separate from installation. Test representative
fleets (e.g. 10, 25, then 50 apps) on disposable hosts while measuring cgroup
memory, latency and CPU. A small synthetic smoke is not production capacity
qualification; add hosts before sustained resource saturation.

## Host publication checks

The installer creates a non-login `celld-publish` account. The host validates
bounded prepared files in `/var/lib/celld-ctl/staging`, performs a credential-free
native dry run, then publishes with exact pinned native celld and root-held
storage credentials passed only to that process. The staged files are root-owned
and readable by the publisher group, not mutable by it; no project build tools
run on the host. Native output and errors are bounded/redacted. Inspect and
remove abandoned staging directories after a process crash, **not** durable
object-store data. Legacy bucket-root imports are excluded from automatic
SSH publication. Use matching client/host 0.2.0 or later.
