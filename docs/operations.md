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
journal namespace, a locked-down SSH deploy account, a separate loopback-only `cella-sshd` daemon
on port `2222`, and its narrow sudo rule. The primary SSH service is untouched.
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

## Private remote transport connectivity

The dedicated SSH service binds **only** `127.0.0.1:2222`. Local transport tests
are not proof of connectivity from a developer's laptop. Ordinary HTTP proxies
cannot carry raw SSH, and some VM platforms manage their own primary SSH daemon;
installing a Match block into a dormant system sshd does not restrict that
platform-managed access. Do not give CI a VM-owner/root access key as a shortcut.

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
