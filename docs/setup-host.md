# A. Set up celld-ctl on the host machine

**Run these steps on the Linux VM/server, using its administrator account.**
Developers follow [B. Set up cella on your dev machine](setup-cella.md) afterward.
Only the host operator controls object storage; the VM owner needs only existing exe.dev
SSH access and their Worker project. The local path generates its own
host credentials and bucket; the external path requires operator configuration.

On a **fresh x86_64** single-node development/testing host, the no-flag
installer sets up local RustFS by default. Its
[October 5, 2026 live gate](rustfs-default-storage-plan.md#7-compatibility-gate-do-this-before-making-rustfs-the-default)
passed on an x86_64 Ubuntu 24.04 systemd **Docker container**, not a VM.
That evidence includes cache-free named Durable Object recovery but does not
qualify VM, arm64, abrupt-kill/reboot or cold-restore behavior. A controlled
RustFS stop/restart did recover. Local storage is not HA;
do not place irreplaceable data on a host without an independent backup.

This guide is for a **new host**. For an existing deployment, back up its
configuration and follow [migration/operations](operations.md) instead of
replacing its Caddy configuration or moving its fleet prefix.

## A1. Install prerequisites and celld-ctl

You need an exe.dev Linux VM with systemd, a Rust toolchain, Caddy,
Python 3, curl, gzip, and administrator/sudo access through the existing
exe.dev SSH gateway. On a **fresh x86_64**
host, from this repository:

```sh
cargo build --release --locked -p celld-ctl
sudo scripts/install-host.sh
```

The installer creates the runtime and publisher accounts; it uses your
existing exe.dev owner SSH command path for deployments. It does **not**
install a deployment SSH daemon, relay, tailnet or separate deploy account/key.
It leaves exe.dev gateway access untouched. On a **fresh x86_64**
host, no flag selects local RustFS and needs no cloud bucket or storage-secret
setup; `--storage external` instead selects manual external configuration in
A3. On a **fresh arm64/aarch64** host, no flag is accepted until native arm64
qualification: explicitly choose `--storage local` (unqualified) or
`--storage external`. The installer installs pinned native celld and initializes
Caddy on a fresh host; A2 and A4 give manual checks/fallbacks. Reinstalling an
existing host preserves its selected storage mode and app targets; flags do
not implicitly migrate buckets, objects or credentials. Node.js/esbuild are
not required on the host for publishing prepared deployments.

## A2. Install the exact native celld release

The installer normally installs the pinned native binary on a fresh host.
Use these manual steps only if the release was not installed or if managing an
older external-storage host manually. Do not replace an existing app's pin when
reinstalling.

The example host configuration pins `0.6.1`. For manual external setup,
install that release, or choose another exact supported release and set the
same version in A3. Do not hand-edit generated local storage configuration
to change an app's pin. Never use a `latest` symlink as an application's pin.

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

### Local RustFS (fresh x86_64 default)

On a **fresh x86_64 disposable development/testing host**,
`sudo scripts/install-host.sh` sets up local RustFS. `--storage local` makes
the same choice explicitly, including on a fresh arm64 host **at the operator's
own risk** pending native arm64 qualification. The installer and operator-only
`celld-ctl storage prepare-local` / `celld-ctl storage init-local` helpers
provision bucket `celld-dev`, endpoint `http://127.0.0.1:9000`, region
`us-east-1`, persistent data under `/var/lib/rustfs`, and root-only generated
credentials under `/etc/rustfs/rustfs.env` and `/etc/celld/node.env`. It
prepares credentials before starting the service and initializes the bucket
afterward. These are root-only host actions, not developer SSH commands. The
local S3 listener must remain loopback-only and the console disabled. The
shared generated credential pair is an administrative RustFS credential,
**not** per-app IAM isolation.

Reinstallation must preserve RustFS data, credentials, registry, app pins and
application state; inconsistent or unknown state must stop installation rather
than be overwritten. The default is for **fresh
x86_64 hosts only**; it does not move existing external app targets. Local
`celld-ctl backup` omits `/var/lib/rustfs` (authoritative application data).
See [backup and recovery](operations.md#history-rollback-backups-and-capacity)
before relying on this VM's storage volume. The
[recorded container smoke](rustfs-default-storage-plan.md#7-compatibility-gate-do-this-before-making-rustfs-the-default)
historically verified a restricted-SSH deployment (in the old transport
configuration) and named-object state after app
restart, RustFS restart and archiving **only** the local app cache. A
controlled RustFS stop/restart made diagnose fail promptly and then recover;
it did not test a VM, arm64, abrupt process kill, VM reboot, cold restore or
production durability. Do not copy the local example config over a live host.

### External S3-compatible storage

On a **fresh host**, select `sudo scripts/install-host.sh --storage external`
and follow the commands below. The installer does not create an external bucket
or credentials. A missing external configuration means setup is incomplete; it
must not silently fall back to RustFS. Do not switch an existing host by
changing the installer flag: defaults do not migrate persisted app targets or
objects.

```sh
sudo test -e /etc/celld-ctl/config.json || \
  sudo install -o root -g root -m 600 examples/config/config.external.json.example \
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

**Only on a fresh host with no existing port-8000 workload, if the installer
has not already initialized Caddy:**

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

## A5. Use existing exe.dev owner SSH

The VM owner deploys through the **same** exe.dev SSH destination they already
use to administer this VM. From the owner's dev machine, `ssh YOUR_VM.exe.xyz`
works normally; if not, exe.dev documents the fallback
`ssh vm+YOUR_VM@vm.exe.xyz`. Register a new public owner key with the exe.dev
account only if needed: `cat ~/.ssh/cella-owner.pub | ssh exe.dev ssh-key add`
(run from a machine already authenticated to that account). See
[B2–B4](setup-cella.md#b2-register-the-owners-public-key-with-exedev).
There is no new port to expose: `cella` uses port **22** and the owner
identity, not the app's HTTPS endpoint. No independently scoped developer or CI
role is offered by this workflow. Never give your VM-owner credential to CI or
untrusted users. Do not add or configure a second sshd, Tailscale or a relay.

## A6. Confirm host owner access and identity

From the owner's **dev machine**, verify the working SSH destination and
compare the VM host-key fingerprint with a trusted value before trusting it:

```sh
ssh -i "$HOME/.ssh/cella-owner" -o IdentitiesOnly=yes YOUR_VM.exe.xyz
# Or when necessary:
ssh -i "$HOME/.ssh/cella-owner" -o IdentitiesOnly=yes vm+YOUR_VM@vm.exe.xyz
```

These are generic placeholders, not actual instance names. The public key
fingerprint of your **client** key (`ssh-keygen -lf ~/.ssh/cella-owner.pub`)
is not the VM's **host** fingerprint and is not the exe.dev account-gateway
fingerprint. See the [client host-key procedure](setup-cella.md#b4-verify-the-vms-ssh-host-key).
The owner account must run `sudo -n /usr/local/bin/celld-ctl transport` without
a password. Ordinary owner SSH provides broad VM access; the fixed command used by `cella`
limits its own request format, **not** what the owner credential can do through
SSH. Host storage credentials remain root-only and outside the repository.

## Host readiness checklist

```sh
sudo systemctl is-active caddy
sudo celld-ctl list
sudo ss -ltnp
# On local-storage hosts:
sudo systemctl is-active rustfs.service
```

Confirm the pinned binary and root-only storage configuration exist; Caddy owns
8000, and the exe.dev owner SSH gateway reaches the VM from
the owner's dev machine. An empty app registry is normal before the first deploy.
No manual `create`/`enable` is needed: `cella deploy` provisions and activates it.
For local storage, verify the S3 listener is loopback-only on port 9000 and
check the installer's native compatibility/readiness results; a running service
by itself is not proof that durable publication works.

Next: **[B. Configure and set up cella on your dev machine](setup-cella.md)**.
Advanced lifecycle, migrations, backups and capacity: [operations](operations.md).
