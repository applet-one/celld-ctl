# A. Set up celld-ctl on the host machine

**Run these steps on the Linux VM/server, using its administrator account.**
Developers follow [B. Set up cella on your dev machine](setup-cella.md) afterward.
Only the host operator configures object storage; developers need only restricted
SSH access and their existing Worker project.

This guide is for a **new host**. For an existing deployment, back up its
configuration and follow [migration/operations](operations.md) instead of
replacing its Caddy configuration or moving its fleet prefix.

## A1. Install prerequisites and celld-ctl

You need Linux with systemd, a Rust toolchain, Caddy, OpenSSH server/client,
Python 3, curl, gzip, and administrator/sudo access. From this repository:

```sh
cargo build --release --locked -p celld-ctl
sudo scripts/install-host.sh
```

The installer creates the runtime/publisher/deployment accounts and starts a
separate deployment-only SSH service, `cella-sshd`, on **127.0.0.1:2222**.
It leaves your existing administrator/platform SSH service untouched. It does
not install native celld, configure storage, join a private network, or enroll
any developer keys. Node.js/esbuild are not required on the host for publishing
prepared deployments.

## A2. Install the exact native celld release

The example host configuration pins `0.6.1`. Install that release, or choose
another exact supported release and set the same version in A3. Never use a
`latest` symlink as an application's pin.

```sh
CELLD_VERSION=0.6.1
case "$(uname -m)" in
  x86_64) CELLD_TARGET=x86_64-unknown-linux-gnu ;;
  aarch64|arm64) CELLD_TARGET=aarch64-unknown-linux-gnu ;;
  *) echo 'Unsupported host architecture'; exit 1 ;;
esac
workdir=$(mktemp -d)
curl --fail --location --proto '=https' --proto-redir '=https' \
  "https://github.com/denoland/celld/releases/download/v${CELLD_VERSION}/celld-${CELLD_TARGET}.gz" \
  --output "$workdir/celld.gz"
gzip -dc "$workdir/celld.gz" > "$workdir/celld"
chmod 755 "$workdir/celld"
"$workdir/celld" --version
test "$("$workdir/celld" --version)" = "celld $CELLD_VERSION" || {
  echo 'Downloaded celld does not match the pin'; exit 1;
}
sudo install -d -o root -g root -m 755 "/usr/local/lib/celld/releases/v${CELLD_VERSION}"
sudo install -o root -g root -m 755 "$workdir/celld" \
  "/usr/local/lib/celld/releases/v${CELLD_VERSION}/celld"
```

Check that the reported version matches the pin before installing. Downloads
trust upstream GitHub HTTPS; this example does not verify provenance attestations.
Temporary downloads and installed binaries stay outside the source checkout.

## A3. Configure storage on the host only

```sh
sudo test -e /etc/celld-ctl/config.json || \
  sudo install -o root -g root -m 600 examples/config/config.json.example \
    /etc/celld-ctl/config.json
sudoedit /etc/celld-ctl/config.json
```

Replace `BUCKET` and `OBJECT_STORE_ENDPOINT`, set the storage region, and ensure
`celld_version` matches A2. The host allocates each new app its own `cells/SLUG`
prefix and loopback ports; developers never specify these settings.

Create/edit the private credential file **on this host**:

```sh
sudo touch /etc/celld/node.env
sudo chown root:root /etc/celld/node.env
sudo chmod 600 /etc/celld/node.env
sudoedit /etc/celld/node.env
```

Its contents are the operator's real storage credentials:

```text
AWS_ACCESS_KEY_ID=REPLACE_WITH_HOST_ACCESS_KEY
AWS_SECRET_ACCESS_KEY=REPLACE_WITH_HOST_SECRET_KEY
# AWS_SESSION_TOKEN=OPTIONAL_SESSION_TOKEN
```

Use credentials able to read/write the host's fleet bucket. Never commit this
file, put credentials in Wrangler, or send them to developers/CI. The same
host-owned credentials support native publication and durable runtime storage.

## A4. Initialize the application proxy

**Only on a fresh host with no existing port-8000 workload:**

```sh
sudo install -o root -g root -m 644 examples/caddy/Caddyfile.initial /etc/caddy/Caddyfile
sudo caddy validate --config /etc/caddy/Caddyfile
sudo systemctl enable --now caddy
sudo systemctl reload caddy
```

Caddy owns port 8000; its admin listener stays on loopback port 2019. Keep the
upstream HTTPS proxy private. This configuration trusts that proxy's forwarded
headers; it is not a standalone public HTTPS installation. App routes and the
read-only directory are generated when the manager publishes routes.

## A5. Make deployment SSH reachable privately

The dedicated listener is **loopback-only**, so the VM's ordinary hostname or
HTTPS proxy does not automatically reach it. Arrange an approved private TCP
path from developers/CI to `127.0.0.1:2222`.

For **VM owners** with an existing administrator SSH login, an
[SSH stdio relay](ssh-stdio-relay.md) provides a local-only path without Tailscale
or SSH TCP forwarding. This reuses full administrator access; never distribute
that credential to CI/developers.

For independently restricted developer/CI access, for example, install Tailscale
separately, enroll the host and approved clients
in your tailnet, restrict access policy to approved identities and this port,
and run on the host:

```sh
sudo tailscale serve --bg --tcp=2222 tcp://127.0.0.1:2222
sudo tailscale serve status
```

Use private Serve, not public Funnel. Tailnet setup is independent of SSH key
authentication: clients need both network access and an enrolled deploy key.
See [private transport operations](operations.md#private-remote-transport-connectivity).
Do not give CI your VM-owner/root SSH key as a connectivity shortcut.

## A6. Enroll each developer's PUBLIC deploy key

The developer generates their key in **B2**, then sends you only the `.pub` file
through an authenticated administrator channel. Install that public key on the
host (example source path; do not use the private key):

```sh
sudo celld-deploy-key add developer-laptop /path/to/cella-deploy.pub --kind owner
sudo celld-deploy-key list
```

Use a separate key and label for CI:

```sh
sudo celld-deploy-key add ci-main /path/to/ci-deploy.pub --kind ci
```

The deployment account is **cella-deploy**, not your administrator account.
Do not use `ssh-copy-id` or hand-edit its authorized keys: the management tool
maintains the labels and restrictive options. The account has no interactive
shell, SFTP, arbitrary commands, or forwarding. All enrolled keys are trusted
fleet-wide publishers, **not** per-app access controls; `owner`/`ci` are labels,
not different permission levels.

To revoke a lost/retired key:

```sh
sudo celld-deploy-key revoke developer-laptop
```

Revocation blocks new connections. Terminate existing sessions separately if
immediate revocation is required.

## A7. Give the developer connection details and server identity

There are two distinct key pairs: the developer's key authenticates **them**;
the server's host key authenticates **this deployment service**.

```sh
sudo cat /etc/celld-ctl/ssh-host-ed25519-key.pub
sudo ssh-keygen -lf /etc/celld-ctl/ssh-host-ed25519-key.pub
```

Send the developer, through a trusted channel:

- The reachable **private deployment hostname/IP** from A5.
- SSH port **2222** and username **cella-deploy**.
- The dedicated server's public host key and its **SHA256 fingerprint**.
- Confirmation that their public deploy key was enrolled in A6.
- The application base URL behind your private HTTPS proxy.

Do not send either private key or any storage credentials. The dedicated
server's key may differ from the primary/platform SSH key on port 22.

## Host readiness checklist

```sh
sudo systemctl is-active caddy cella-sshd
sudo celld-ctl list
sudo ss -ltnp
```

Confirm the pinned binary and root-only storage configuration exist; Caddy owns
8000, deployment SSH is loopback 2222, and the private TCP path is reachable from
the developer's machine. An empty app registry is normal before the first deploy.
No manual `create`/`enable` is needed: `cella deploy` provisions and activates it.

Next: **[B. Configure and set up cella on your dev machine](setup-cella.md)**.
Advanced lifecycle, migrations, backups and capacity: [operations](operations.md).
