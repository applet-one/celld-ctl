# Host installation and operations

Start with **[A. Set up celld-ctl on the host](setup-host.md)**, then give
developers **[B. Dev-machine setup](setup-cella.md)**. This page is the operator
reference for lifecycle, migration, key management, backups and security.

This is a single-host MVP, not a multi-tenant security boundary. All deploy keys
are trusted fleet publishers; keys are not scoped to individual apps. The runtime
uses celld's Worker isolation and a dedicated shared Unix service account. No
browser management interface, inbound deployment HTTP API, or app-secret
management is installed.

## Build and install

On a fresh Linux x86_64 single-node development/testing system with systemd,
Caddy, OpenSSH, Python 3 and a Rust toolchain:

```sh
cargo build --release --locked
sudo scripts/install-host.sh
```

The installer installs binaries, the systemd template, bounded application
journal namespace, a locked-down SSH deploy account, a separate loopback-only `cella-sshd` daemon
on port `2222`, and its narrow sudo rule. The primary SSH service is untouched.
It does **not** migrate existing application object data or app pins, join a
private network or add deploy keys. Inspect templates and back up existing
configuration before installation. On a **fresh x86_64 host**, no flag installs
single-node local RustFS, provisions its bucket and root-only credentials, and
checks native storage readiness. `--storage local` selects it explicitly;
`--storage external` opts out into manual S3-compatible storage setup.
On a **fresh arm64/aarch64 host**, a no-flag install refuses to choose:
specify `--storage local` (not natively qualified on arm64) or
`--storage external`. Reinstalling a configured host preserves its existing
mode, registry targets, credentials and app pins; flags do not migrate object
data. The [October 5, 2026 minimum live gate](rustfs-default-storage-plan.md#7-compatibility-gate-do-this-before-making-rustfs-the-default)
passed with real restricted-SSH deployment and cache-free named-object
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
Restricted transport targets contain only the slug, exact native version and
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

## Restricted deployment keys

Developers first generate a dedicated key on their own machine using
[B2. Generate a deployment key](setup-cella.md#b2-generate-a-dedicated-deployment-key).
Receive only the public `.pub` file through an authenticated administrator
channel, then run the following **on the host**. Do not use `ssh-copy-id` or
SFTP through the restricted account.

```sh
sudo celld-deploy-key add owner-laptop /path/to/owner.pub --kind owner
sudo celld-deploy-key add ci-main /path/to/ci.pub --kind ci
sudo celld-deploy-key list
sudo celld-deploy-key revoke ci-main
```

Only public keys are installed. Private keys remain on the developer machine or
in CI. Root-owned authorized keys carry `restrict`, and sshd independently
forces the fixed transport and disables shell commands, TTYs, forwarding,
tunnels and user startup scripts. The account's `/bin/sh` exists only so sshd
can launch its forced command; it does not grant an interactive shell.

`sudo` allows exactly `/usr/local/bin/celld-ctl transport`, not arbitrary host
commands or configurable paths. Ordinary requests are bounded JSON; deploy
adds a size-declared prepared-package body with independent bounds. The transport
rejects extra fields/unknown operations. Its allowlist is provision, target, deploy,
activate, status, logs and deployments. Deployers cannot choose storage endpoints,
buckets, host paths, build executables or environment variables. Authentication is SSH public-key
possession. CI should have a distinct revocable key, not a copy of the owner's.
Revocation blocks new SSH authentication; terminate existing deploy-account
sessions explicitly when immediate revocation is required.

## Private remote transport connectivity

The dedicated SSH service binds **only** `127.0.0.1:2222`. Local transport tests
are not proof of connectivity from a developer's laptop. Ordinary HTTP proxies
cannot carry raw SSH, and some VM platforms manage their own primary SSH daemon;
installing a Match block into a dormant system sshd does not restrict that
platform-managed access. Do not give CI a VM-owner/root access key as a shortcut.

VM owners can alternatively use an [SSH stdio relay](ssh-stdio-relay.md)
through their existing administrator login. This needs no Tailscale and does
not request SSH TCP forwarding; it is not a replacement for separately scoped
CI/developer connectivity because the outer credential still grants admin access.

Provide an independent private TCP path, for example by enrolling the VM and
approved developer/CI devices into your tailnet, then forwarding only this SSH
service with Tailscale Serve (not public Funnel):

```sh
sudo tailscale serve --bg --tcp=2222 tcp://127.0.0.1:2222
# From an approved tailnet client:
export CELLA_HOST=cella-deploy@TAILNET_HOST
export CELLA_SSH_PORT=2222
export CELLA_SSH_KEY=/path/to/restricted-deploy-key
cella deploy
```

Follow the [official Serve TCP documentation](https://tailscale.com/docs/reference/tailscale-cli/serve)
and restrict tailnet policy to approved owner/CI identities and this port. Tailnet
enrollment/credentials and ACL policy are operator instance state, never source
artifacts; the installer does not join an account automatically. No celld
serving/internal port is forwarded. The HTTPS application proxy remains private.

Enroll the dedicated server's public host key in the client's `known_hosts`
after comparing its fingerprint through a trusted operator channel:

```sh
sudo ssh-keygen -lf /etc/celld-ctl/ssh-host-ed25519-key.pub
```

`cella` deliberately uses strict host-key verification. Its SSH port defaults to
22 for ordinary SSH environments; use `--ssh-port 2222` or `CELLA_SSH_PORT=2222`
for this installed daemon. The old `cella-deploy.conf` is an optional legacy
Match fragment for operators with a conventional primary OpenSSH service; the
installer no longer installs it or reloads the primary SSH service.

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

Back up the registry and root-only configuration and dedicated SSH server identity with `sudo celld-ctl backup`
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
service environment, registry, host config, node credentials, deployment SSH
identity and installed-version metadata, preserving ownership and permissions.
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

After enrolling a deploy key, use the real, non-mutating SSH policy probe:

```sh
python3 scripts/check-ssh-policy.py --host cella-deploy@HOST \
  --identity /path/to/key --port 2222 --slug APP_SLUG
```

It first requires a successful read-only status operation, then checks rejected
shell/extra commands, unsupported operations, injected paths/slugs, oversized
requests and TCP forwarding. Do not confuse a failed connection with a passed
policy check. Key revocation can be verified by retrying a **new** connection
after revoking its label; existing sessions need separate termination if immediate
revocation is required.

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
