# Set up a host

Use a Linux VM with systemd, sudo, a Rust toolchain, Caddy, Python 3, curl
and gzip. You'll need owner SSH access and passwordless sudo for
`/usr/local/bin/celld-ctl transport`.

This is a single-host development/testing setup, not HA. For an existing host,
[back up first](operations.md#backup-and-recovery). Do not overwrite an existing
Caddy configuration or assume reinstalling will migrate storage.

## Install

On a fresh x86_64 host:

```sh
git clone https://github.com/applet-one/celld-ctl.git
cd celld-ctl
cargo build --release --locked -p celld-ctl
sudo scripts/install-host.sh
```

The installer sets up pinned celld, app services, Caddy and local RustFS storage.
RustFS data lives in `/var/lib/rustfs`; its S3 endpoint is `127.0.0.1:9000`.
Keep storage private and [back up its data separately](operations.md#backup-and-recovery).

On arm64, choose `--storage external`, or explicitly use `--storage local`
if you accept that local storage has not been qualified on arm64.
Reinstallation preserves existing storage settings and app version pins.

## External S3-compatible storage

For a fresh host, use:

```sh
sudo scripts/install-host.sh --storage external
```

Create the bucket and credentials with your storage provider, then configure
the host:

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

In `config.json`, set the bucket, HTTPS endpoint and region. Keep
`celld_version` matched to the installed release.

In `node.env`, set `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY` and, if needed,
`AWS_SESSION_TOKEN`. Credentials need read/write access to the bucket. Keep
them on the host, not in Wrangler or this repository.

Changing an installer flag does not migrate existing apps or object-store data.

## Proxy and readiness

Caddy serves an app directory on port 8000 and apps on ports 9101–9999.
exe.dev supplies authenticated HTTPS forwarding. Other hosts need a trusted
HTTPS proxy and access control for every directory/app port, preserving the
public port numbers so directory links work.

**Do not expose Caddy's plain HTTP listeners as unauthenticated public services.**
Keep native celld, storage and Caddy's admin listener private.

The installer initializes Caddy on fresh hosts. If it did not, and there is
no existing configuration or port conflict:

```sh
sudo install -o root -g root -m 644 examples/caddy/Caddyfile.initial /etc/caddy/Caddyfile
sudo caddy validate --config /etc/caddy/Caddyfile
sudo systemctl enable --now caddy
sudo systemctl reload caddy
```

Check services and listeners:

```sh
sudo systemctl is-active caddy
sudo celld-ctl list
sudo ss -ltnp
# For local storage:
sudo systemctl is-active rustfs.service
```

An empty app list is normal. Next, [configure cella and deploy an app](cella.md).
