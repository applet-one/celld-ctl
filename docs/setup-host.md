# Set up a host

For a **new** Linux VM/server with systemd, sudo, a Rust toolchain, Caddy,
Python 3, curl and gzip. The owner needs working SSH and non-interactive
sudo. This is single-node development/testing, not HA. Existing hosts:
[back up and review migration](operations.md) before
reinstalling; never overwrite an existing Caddy configuration or change an app's
storage target by changing an installer flag.

## Install

On a fresh x86_64 host:

```sh
git clone https://github.com/applet-one/celld-ctl.git; cd celld-ctl
cargo build --release --locked -p celld-ctl
sudo scripts/install-host.sh
```

The installer installs the pinned native celld binary, runtime/publisher
accounts and app unit, initializes Caddy on a fresh host, and sets up local
RustFS (bucket `celld-dev`, endpoint `http://127.0.0.1:9000`, region
`us-east-1`). Its data is `/var/lib/rustfs`; root-only generated credentials
are in `/etc/rustfs/rustfs.env` and `/etc/celld/node.env`. RustFS's administrative
credential pair is shared across apps, **not** per-app IAM isolation. Keep S3
loopback-only and its console disabled. No host storage credentials go to the
owner's development machine. Host Node.js/esbuild are not required to publish
prepared deployments.

On a fresh arm64/aarch64 host, the no-flag installer refuses to select a mode:
use `--storage external`, or explicitly `--storage local` only if you accept
that arm64 local storage is not qualified. Reinstalling a configured host
preserves its storage mode, credentials, registry targets and app pins; it does
not migrate objects or silently upgrade pins. Check the
[dated test limits](operations.md#historical-storage-gate-october-5-2026)
before relying on local data, and [back it up](operations.md#backup-and-recovery).

## External S3-compatible storage

For a **fresh** host, explicitly run `sudo scripts/install-host.sh --storage
external`. The installer does not create a bucket or credentials; missing
external configuration is incomplete setup, not a local-storage fallback.
The installer normally installs the pinned native celld release; if managing
an older host manually, install the exact release specified by `celld_version`
as a root-owned executable at `/usr/local/lib/celld/releases/vVERSION/celld`.
Verify its `--version`; do not use `latest` or alter existing app pins. The
example config pins 0.6.1.

```sh
sudo test -e /etc/celld-ctl/config.json || \
  sudo install -o root -g root -m 600 examples/config/config.external.json.example \
    /etc/celld-ctl/config.json
sudoedit /etc/celld-ctl/config.json
sudo touch /etc/celld/node.env
sudo chown root:root /etc/celld/node.env
sudo chmod 600 /etc/celld/node.env
sudoedit /etc/celld/node.env
```

In the config replace `BUCKET` and `OBJECT_STORE_ENDPOINT`, set the region,
and match `celld_version` to the installed native binary. The host allocates
`cells/SLUG` prefixes. Put `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY` and
(optional) `AWS_SESSION_TOKEN` in `/etc/celld/node.env`, not in Wrangler,
this repository or the owner client. Credentials must read/write the bucket;
the host uses them for native publication and serving. Changing a configured
host's installer flag **does not** migrate existing targets or object data.

## Proxy and readiness

On a fresh host **only if** the installer did not initialize Caddy, and only
if port 8000 and the intended app-port range are unused:

```sh
sudo install -o root -g root -m 644 examples/caddy/Caddyfile.initial /etc/caddy/Caddyfile
sudo caddy validate --config /etc/caddy/Caddyfile
sudo systemctl enable --now caddy
sudo systemctl reload caddy
```

Caddy's admin listener is private at `127.0.0.1:2019`. The directory at 8000
links enabled apps to dedicated Caddy ports 9101–9999; each port proxies all
paths unchanged to a loopback runtime port 8101–8999. On exe.dev, its
authenticated HTTPS proxy forwards these ports. On another VM, arrange trusted
HTTPS and access control for **every** directory/app port, preserving public
port numbers so the directory links work. Do not expose Caddy's plain HTTP
listeners as unauthenticated public services. The forwarded-header
configuration assumes a trusted upstream proxy.

```sh
sudo systemctl is-active caddy
sudo celld-ctl list
sudo ss -ltnp
# On local-storage hosts:
sudo systemctl is-active rustfs.service
```

Check the exact native pin and private storage files, RustFS S3 on loopback
9000, private app/internal listeners, and installer storage-readiness results.
A running service alone does not prove durable publication. An empty registry
is normal. The owner needs working SSH access and non-interactive
`sudo -n /usr/local/bin/celld-ctl transport`; the installer does not add a
second SSH daemon or deploy key. Next: [configure the owner client and verify the VM host
key](cella.md#owner-ssh-setup). For backup, migrations and service recovery,
see [operations](operations.md).
