# Host installation and operations

Start with **[A. Set up celld-ctl on the host](setup-host.md)**, then give
developers **[B. Dev-machine setup](setup-cella.md)**. This page is the operator
reference for lifecycle, migration, key management, backups and security.

This is a single-host MVP, not a multi-tenant security boundary. The VM-owner SSH credential has broad host privileges; there are no scoped
deployment keys or independently authorized CI/developer publishers. The runtime
uses celld's Worker isolation and a dedicated shared Unix service account. No
browser management interface, inbound deployment HTTP API, or app-secret
management is installed.

## Build and install

On a fresh exe.dev Linux x86_64 single-node development/testing VM with
systemd, Caddy, Python 3 and a Rust toolchain:

```sh
cargo build --release --locked
sudo scripts/install-host.sh
```

The installer installs binaries, the systemd app template and bounded app
journal namespace. Deployments reuse the existing exe.dev VM-owner SSH gateway
and non-interactive owner sudo; the installer does not set up another sshd,
a deploy account/key, a relay, Tailscale, or change the primary SSH service.
It does **not** migrate existing application object data or app pins. Inspect templates and back up existing
configuration before installation. On a **fresh x86_64 host**, no flag installs
single-node local RustFS, provisions its bucket and root-only credentials, and
checks native storage readiness. `--storage local` selects it explicitly;
`--storage external` opts out into manual S3-compatible storage setup.
On a **fresh arm64/aarch64 host**, a no-flag install refuses to choose:
specify `--storage local` (not natively qualified on arm64) or
`--storage external`. Reinstalling a configured host preserves its existing
mode, registry targets, credentials and app pins; flags do not migrate object
data. The [October 5, 2026 minimum live gate](rustfs-default-storage-plan.md#7-compatibility-gate-do-this-before-making-rustfs-the-default)
historically passed with the old restricted-SSH transport and cache-free named-object
recovery in an x86_64 Ubuntu 24.04 **systemd Docker container**, not a VM.
It does not qualify VM/arm64, abrupt process kills, cold restore, or HA.

For external storage setup (explicitly select `--storage external` on a new
host), copy
`examples/config/config.external.json.example` to `/etc/celld-ctl/config.json`
and replace placeholders. Make it root-owned mode `0600`. Install each exact native
celld release if managing the host manually or if installer installation failed,
root-owned and executable, at
`/usr/local/lib/celld/releases/vVERSION/celld`. The configured release becomes
new apps' pin; changing the default does not upgrade existing apps.

For external setup, write only the node's storage credential variables to `/etc/celld/node.env`,
root-owned mode `0600` (`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, optional
`AWS_SESSION_TOKEN`). The host uses these for native publication as well as
serving. Developers and CI do not need storage credentials or bucket settings.
Remote transport targets contain only the slug, exact native version and
enabled flag; the root operator can still inspect full registry targets.

Celld and internal/operator listeners bind to loopback. Only Caddy binds to
`:8000`; keep the upstream HTTPS proxy private. Incoming forwarded host/proto
headers are trusted **only** because this host is behind that trusted proxy.
Do not expose this configuration as a direct public HTTP server.

On a **new** host with no existing port-8000 workload, initialize Caddy if the
installer has not already done so:

```sh
sudo install -o root -g root -m 644 examples/caddy/Caddyfile.initial /etc/caddy/Caddyfile
sudo caddy validate --config /etc/caddy/Caddyfile
sudo systemctl enable --now caddy
sudo systemctl reload caddy
```

Do not overwrite an existing application's proxy configuration blindly. For a
legacy counter, complete its migration first and let `import-counter --enabled`
render the route. Route publication needs Caddy's private admin listener running
at `127.0.0.1:2019`; it is not an external management endpoint.

## Application lifecycle

```sh
sudo celld-ctl create APP_SLUG
sudo celld-ctl target APP_SLUG
# Publish using native celld deploy/cella before enabling.
sudo celld-ctl enable APP_SLUG
sudo celld-ctl list
sudo celld-ctl status APP_SLUG
sudo celld-ctl logs APP_SLUG
sudo celld-ctl restart APP_SLUG
sudo celld-ctl disable APP_SLUG
```

Registry state is `/var/lib/celld-ctl/registry.sqlite`; app environment files live
in `/etc/celld/cells`, and app caches in `/var/lib/celld/APP_SLUG`. Never put these
paths inside this repository. Ports are transactionally allocated in SQLite;
they are not hashes of app names. The internal port range must be disjoint from
the serving range and must never be published.

Enable/activation verifies the durable deployment pointer, starts the pinned
service and checks its private deployment state before publishing a route.
Routes preserve `/APP_SLUG/` (no prefix stripping); `/APP_SLUG` redirects to it.
The root page is a generated, read-only directory, not live monitoring. Use
`status` to inspect the actual process and deployed version.

`disable` unpublishes the route and stops/disables its unit. Stop/start and
restart are operator actions, not available to deploy keys. Removal must never
delete durable object-store data; retained local state should be archived or
removed deliberately by the operator, not a remote developer.

## Owner SSH transport and trust boundary

The VM **owner** registers their public authentication key with the exe.dev
account (not `celld-deploy-key` on the host), then uses the working direct SSH
destination `YOUR_VM.exe.xyz` or fallback `vm+YOUR_VM@vm.exe.xyz` on port **22**.
See [dev-machine setup](setup-cella.md) and exe.dev's
[SSH destination](https://exe.dev/docs/faq/ssh-destination.md) and
[ssh-key](https://exe.dev/docs/cli-ssh-key.md) docs. The local key's public
fingerprint (`ssh-keygen -lf ~/.ssh/cella-owner.pub`) is distinct from the
server host-key fingerprint verified in `known_hosts`. The published
[exe.dev fingerprint](https://exe.dev/docs/faq/host-key.md) refers specifically
to `ssh exe.dev`, not an asserted VM destination fingerprint.

`cella` invokes batch SSH with `-F /dev/null`, explicit `-i`,
`IdentitiesOnly=yes`, strict host-key checking, no TTY or forwarding, and the fixed command
`sudo -n /usr/local/bin/celld-ctl transport`. The owner account must already
have **passwordless sudo** for this command; exe.dev VM owner accounts may have
broader sudo access. A passphrase-protected key may be preloaded into the local
agent; the agent is not forwarded. `celld-ctl transport` requires root, accepts
`transport` with no extra argv and bounds the stdin request. No remote parameter in
the protocol selects a storage bucket, host path or executable. The transport
allowlist is provision, target, deploy, activate, status, logs and deployments.
Only root-held host storage credentials are used during publication.

**This is not a restricted SSH principal.** The owner can use ordinary SSH to
run other VM commands, and the account's sudo authority is independent of
`cella`'s narrow wire format. Do not give VM-owner SSH keys to CI, other
developers or untrusted parties. There is no independently scoped CI deploy
role, fleet key revocation inventory or dedicated deployment port. Revocation
of a compromised owner key belongs to the exe.dev account; use
`ssh exe.dev ssh-key list` / `ssh exe.dev ssh-key remove ...`, and consider
existing sessions and other owner keys. Avoid altering the platform-managed
SSH service or opening internal celld/RustFS ports.

## Existing-host migration from dedicated deployment SSH

**For hosts previously installed with the old `cella-sshd` workflow only.**
Take a [host backup](#history-rollback-backups-and-capacity) and preserve it
off-host before cleanup. Reinstall the current host binaries, verify direct
owner SSH and `sudo -n /usr/local/bin/celld-ctl transport`, then test a
non-mutating `cella status` with the new CLI. The new installer does not remove
old managed SSH artifacts and does not touch exe.dev's primary gateway. Do not
delete the only working access path until owner access is verified.

Once owner deployment works, on the VM stop and disable only the *old dedicated*
service, inspect then remove its known managed artifacts if present:

```sh
sudo systemctl disable --now cella-sshd.service
sudo rm -f /etc/systemd/system/cella-sshd.service \
  /etc/celld-ctl/sshd_config \
  /etc/celld-ctl/ssh-host-ed25519-key /etc/celld-ctl/ssh-host-ed25519-key.pub \
  /etc/sudoers.d/cella-deploy \
  /etc/ssh/sshd_config.d/60-cella-deploy.conf
sudo systemctl daemon-reload
```

The old installation may also contain `/etc/ssh/cella-deploy/authorized_keys`,
`/var/empty/cella-deploy`, `/usr/local/bin/celld-deploy-key`, and the
`cella-deploy` system account. Confirm those are unused by anything else
before deleting the files/helper/account manually.
Remove obsolete local relays/port forwards and stale `known_hosts` entries for
`[OLD_DEPLOY_HOST]:2222` **only after verifying what each entry is**. Remove
old `CELLA_HOST=cella-deploy@...`, `CELLA_SSH_PORT=2222` and `CELLA_SSH_KEY`
settings from profiles/CI; replace them with owner destination and registered
owner key in the private dev-machine environment. Do not delete app registry,
object data, storage credentials, app units, the real SSH service, or backups.
Re-running the installer is not a reset or an automatic cleanup/migration.

## Counter migration

An existing bucket-root counter is a special imported legacy app. Do not move
its fleet prefix or redeploy it into `/cells/counter`. Back up the old unit,
root-only environment and local state. Gracefully stop the service, copy its
cache to `/var/lib/celld/counter`, give the service account ownership, and point
`CELLD_WATCH` at that path. Keep credentials root-owned and readable only via
systemd's EnvironmentFile. Start the same celld version with serving/internal
listeners `127.0.0.1:8100` / `127.0.0.1:18100`, and proxy `/counter/*` through
Caddy. Import its existing native version ID with `import-counter`.

Check a known named Durable Object before and after restart; a successful HTTP
response alone does not prove persistence. Keep backups outside the repository.
Migration does not repair unrelated pre-existing fleet ownership failures.

## History, rollback, backups and capacity

Successful developer activation records the native deployment ID and source
revision. Roll back by checking out the chosen prior Git revision and running
`cella deploy`; this uses the same pinned native parser and deployment engine.
This does not revert Durable Object data/schema migrations.

Back up the registry and root-only configuration with `sudo celld-ctl backup`
(the destination is under `/var/lib/celld-ctl/backups`) and store the result securely off-host. Only snapshots with a `COMPLETE` marker
are complete; configuration, app environments and units have separate subdirectories. Registry/config backups contain
node credentials; they are not public artifacts. Object-store durability does
not replace registry, configuration, key inventory or release-binary backups.
Restoration should be performed with services stopped and ownership/modes
preserved; validate the pinned binaries and Caddy configuration before starting.

**`celld-ctl backup` does not include object-store data.** On a local RustFS
host, `/var/lib/rustfs` is authoritative app data, not a disposable cache.
`/var/lib/celld/APP_SLUG` is a local app cache. Losing the VM's RustFS volume
may lose acknowledged application data. Store an independent off-VM snapshot
if recovery from VM loss matters; this development/testing setup is not HA.

### Local-storage service and recovery (when installed)

Inspect `systemctl status rustfs.service` and `journalctl -u rustfs.service`
on the host; verify the S3 listener is only `127.0.0.1:9000` with `ss -ltnp`
and that the console has not been enabled. Check available disk space and
permissions on `/var/lib/rustfs` (private to `rustfs`) and root-only
`/etc/rustfs/rustfs.env`, `/etc/celld/node.env` and
`/etc/celld-ctl/config.json`. Never print credential contents into shared logs.
An app's local-cache contents alone cannot prove that durable state was restored;
check a known named Durable Object after restarting both storage and the app.
Do not replace missing credentials or recreate an apparently empty bucket as
an outage workaround.

For an **optional cold snapshot**, block deployments and other writers, stop
affected app services, then stop RustFS. Capture its data directory, RustFS
service environment, registry, host config, node credentials and
installed-version metadata, preserving ownership and permissions.
Copy the snapshot off-host if VM loss is in scope. Restart RustFS first, check
readiness, then start apps and reopen deployments. Do **not** treat a live
recursive copy of RustFS's files as a consistent backup. Restore only onto an
isolated host with writers stopped, the correct pinned binaries/configuration
and matching credential pair; verify a known named object's state before
reopening service. Cold-snapshot restore remains unproven by the
[minimum container gate](rustfs-default-storage-plan.md#7-compatibility-gate-do-this-before-making-rustfs-the-default);
test it separately on a disposable host before relying on this recovery plan.

There is no remotely accessible reset command. For a disposable host, stop
deployment access and app/storage services, archive registry/config/credentials
and RustFS data together, then explicitly reset the whole instance under an
operator-reviewed procedure. Never clear only RustFS data while retaining app
registry targets/caches, or clear only the registry while retaining app object
data. Reinstalling is **not** a reset and must not delete application data.

No automatic local-to-external or external-to-local migration is provided.
Changing the host default affects new apps, not persisted targets. A migration
requires quiescing writers, copying complete object namespaces, updating each
registry target/generated app input and validating recovery; native lease and
session handling require a separate reviewed procedure. Keep legacy bucket-root
imports intact. Upgrades must be explicit: pin and verify the replacement native
celld/RustFS artifacts, take a cold snapshot, test rollback/recovery on a
disposable host, and never let a routine reinstall silently rotate credentials
or change an app pin.

Capacity testing is an explicit operational step, not something installation
runs against a production bucket. Exercise 10, then 25, then 50 representative
apps while measuring cgroup memory, request latency and CPU. Increase capacity
only after inspection; add a second host before sustained memory/CPU saturation.
Rust/Wasm developer toolchains and runtime app-secret management remain deferred.

Use `scripts/load-test.py` against already deployed **disposable** apps:

```sh
python3 scripts/load-test.py --base-url http://127.0.0.1:8000 \
  --slugs test-1,test-2 --requests 1000 --concurrency 4 --dry-run
# Remove --dry-run and add --allow-mutations after checking the fleet list.
```

It reports status counts, throughput and latency percentiles, and returns failure
for any non-2xx result. It never provisions fleets or passes credentials. For
capacity qualification supply 10, then 25, then 50 representative deployed app
slugs and measure cgroup/host resources concurrently. A small two-app smoke test
is not capacity qualification, and synthetic counter traffic is not necessarily
representative of your workload.

After host installation, verify direct owner login and non-interactive sudo,
then use a read-only client command:

```sh
ssh -i "$HOME/.ssh/cella-owner" -o IdentitiesOnly=yes YOUR_VM.exe.xyz \
  'sudo -n /usr/local/bin/celld-ctl list'
CELLA_HOST=YOUR_VM.exe.xyz CELLA_SSH_KEY="$HOME/.ssh/cella-owner" \
  cella --slug APP_SLUG status
```

An unknown app from `status` is expected before its first deploy. `list` is an
operator check, not the deployment protocol. Neither test proves least-privilege
owner access: the owner account still has broad VM authority.

## Host-side publication

The installer creates a separate `celld-publish` system account with no login
shell. Prepared packages are staged outside the source tree under
`/var/lib/celld-ctl/staging`, with root-owned immutable files readable only by
the publisher group. Packages contain built modules and explicit assets, not
repositories, package-manager scripts or node_modules. The host independently
normalizes private source paths and uses native `no_bundle`; containers/Python
builds are refused, so uploaded code is not executed during publication.

A credential-free native dry run validates the prepared deployment and expected
version. Native publication then runs as the unprivileged publisher with the
root-read node credentials passed only to that trusted process. Child output,
execution time, file/directory counts, configuration size, transfer size and
decoded bytes are bounded; storage
credentials are redacted from errors. Temporary staging is cleaned up after
normal completion/failure. Following a process crash, an operator can inspect
and remove abandoned stage directories; never delete durable object-store data.

Keep client and host at version 0.2.0 or later. The older client published directly
to object storage and is not compatible with minimal SSH deployment targets.
Legacy bucket-root imports are protected from automatic SSH publication.
