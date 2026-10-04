# Host installation and operations

This is a single-host MVP, not a multi-tenant security boundary. All deploy keys
are trusted fleet publishers; keys are not scoped to individual apps. The runtime
uses celld's Worker isolation and a dedicated shared Unix service account. No
browser management interface, inbound deployment HTTP API, or app-secret
management is installed.

## Build and install

On a Linux system with systemd, Caddy, OpenSSH, Python 3 and a Rust toolchain:

```sh
cargo build --release --locked
sudo scripts/install-host.sh
```

The installer installs binaries, the systemd template, bounded application
journal namespace, a locked-down SSH deploy account, and its narrow sudo rule.
It does **not** migrate an existing service, replace a running Caddy configuration,
add deploy keys, install celld releases, or write storage credentials for you.
Inspect templates and back up existing configuration before installation.

Copy `examples/config/config.json.example` to `/etc/celld-ctl/config.json` and
replace placeholders. Make it root-owned mode `0600`. Install each exact native
celld release, root-owned and executable, at
`/usr/local/lib/celld/releases/vVERSION/celld`. The configured release becomes
new apps' pin; changing the default does not upgrade existing apps.

Write only the node's storage credential variables to `/etc/celld/node.env`,
root-owned mode `0600` (`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, optional
`AWS_SESSION_TOKEN`). Use separate least-privilege developer/CI storage
credentials where practical. `target` intentionally contains the endpoint,
bucket prefix, region and version, but **never** credential values.

Celld and internal/operator listeners bind to loopback. Only Caddy binds to
`:8000`; keep the upstream HTTPS proxy private. Incoming forwarded host/proto
headers are trusted **only** because this host is behind that trusted proxy.
Do not expose this configuration as a direct public HTTP server.

On a **new** host with no existing port-8000 workload, initialize Caddy explicitly:

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
commands or configurable paths. The transport accepts one bounded JSON request
and rejects extra fields/unknown operations. Its allowlist is provision, target,
activate, status, logs and deployments. Authentication is SSH public-key
possession. CI should have a distinct revocable key, not a copy of the owner's.
Revocation blocks new SSH authentication; terminate existing deploy-account
sessions explicitly when immediate revocation is required.

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
