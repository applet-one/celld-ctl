# cella: owner SSH deployment

First [install the host](setup-host.md). `cella` is the owner's client; it
uses the existing exe.dev VM-owner SSH gateway, not a separate deployment
login, relay or port 2222. Keep your `wrangler.jsonc`/`wrangler.json`.
Host object-store bucket settings and credentials stay on the host; application
R2 bindings in Wrangler are separate. Client and host must be 0.2.0 or later:
the old 0.1 client published directly to object storage.

## Install

```sh
cargo install --locked --path crates/cella --force
```

Supported developer platforms: Linux x86_64/arm64 and macOS arm64. Install
OpenSSH, `curl` (for exact native downloads), Node.js and your project's
npm/pnpm/Yarn dependencies including esbuild when bundling requires it.
`cella` does not run package installs.

## Owner SSH setup

Reuse an exe.dev-registered owner SSH key if available. Otherwise, **on your
own machine**, choose a new filename if this one exists:

```sh
mkdir -p "$HOME/.ssh"; chmod 700 "$HOME/.ssh"
ssh-keygen -t ed25519 -f "$HOME/.ssh/cella-owner" -C 'exe.dev VM owner'
chmod 600 "$HOME/.ssh/cella-owner"
ssh-keygen -lf "$HOME/.ssh/cella-owner.pub"
cat "$HOME/.ssh/cella-owner.pub" | ssh exe.dev ssh-key add
```

The [exe.dev key command](https://exe.dev/docs/cli-ssh-key.md) requires an
already-authenticated account for registration; use your account's registration
flow if that fails. Only submit the `.pub` file, never the private key. Prefer
a passphrase and `ssh-add "$HOME/.ssh/cella-owner"` on the **local** agent
before deploying. Batch SSH cannot prompt for it; alternatively, protect an
empty-passphrase key carefully. Never share an owner key with CI or untrusted
users.

Test your actual [exe.dev SSH destination](https://exe.dev/docs/faq/ssh-destination.md)
on port 22 (placeholders below):

```sh
ssh -i "$HOME/.ssh/cella-owner" -o IdentitiesOnly=yes YOUR_VM.exe.xyz
# If necessary, use the gateway form:
ssh -i "$HOME/.ssh/cella-owner" -o IdentitiesOnly=yes vm+YOUR_VM@vm.exe.xyz
export CELLA_HOST=YOUR_VM.exe.xyz  # or vm+YOUR_VM@vm.exe.xyz
export CELLA_SSH_KEY="$HOME/.ssh/cella-owner"
```

**Verify the VM's SSH host-key fingerprint independently** through trusted VM
access or the operator before accepting its `known_hosts` entry. The owner
*.pub* authentication-key fingerprint above is not the VM's server fingerprint.
The [exe.dev host-key FAQ](https://exe.dev/docs/faq/host-key.md) covers
`ssh exe.dev`, not necessarily either VM destination. For the direct-hostname
form you may inspect a scan, but a scan alone cannot authenticate the host:

```sh
ssh-keyscan -t ed25519 YOUR_VM.exe.xyz > "$HOME/.ssh/cella-vm-key.scan"
ssh-keygen -lf "$HOME/.ssh/cella-vm-key.scan"
# Only after an independent fingerprint match:
cat "$HOME/.ssh/cella-vm-key.scan" >> "$HOME/.ssh/known_hosts"
chmod 600 "$HOME/.ssh/known_hosts"
```

For `vm+YOUR_VM@vm.exe.xyz`, verify the fingerprint presented for **that
actual destination** against its trusted value; do not reuse a scan from
`YOUR_VM.exe.xyz`. Investigate key changes, never disable host-key checking.
`cella` ignores `~/.ssh/config` (`-F /dev/null`), so set the literal working
`CELLA_HOST` and registered `CELLA_SSH_KEY` (or `--host` and `--identity`).
`CELLA_SSH_PORT`/`--ssh-port` defaults to 22. No SSH agent or storage credentials
are forwarded. On the VM the owner must be able to run
`sudo -n /usr/local/bin/celld-ctl transport` without a password. `cella` sends
only that fixed command, but **ordinary owner SSH/sudo remains VM-wide**, not
a scoped deploy identity.

## Deploy and inspect

Install your project dependencies and run from its Wrangler project directory:

```sh
cella deploy
cella status
cella logs --lines 50
cella deployments list
# Other choices:
cella --project ./my-project deploy
cella --slug alternate-slug deploy
cella deploy --source-revision COMMIT_SHA
cella --slug APP_SLUG status
```

First deploy provisions and activates the app. The Worker name is the default
slug; `--slug` changes the directory/routing identity, not the native Worker.
Port 8000 lists active slugs linking to their dedicated Caddy ports (9101–9999)
via exe.dev's authenticated alternate-port proxy. Each app serves at `/` on its
port, preserving Wrangler root routes without slug path prefixes. Status and
deployment history are JSON; logs are bounded journal text (`--lines` 1–1000).
A status request for an unknown slug before first deploy is expected.

`cella` downloads and verifies the host's exact native celld pin, builds locally
with native `deploy --dry-run`, and uploads a bounded package of built modules,
explicit assets and config. It sends no repository, node_modules, credential
files or build executables. The host independently validates and stages it,
uses a private deployment-only config without modifying your Wrangler file,
and publishes with its own credentials using the pinned native binary. No user
build tool executes on the VM. JavaScript/TypeScript, asset-only and standard
WASM modules are supported; container/Python builds and nonstandard copied
module extensions are rejected. Legacy bucket-root imports need an operator.

Keep secrets out of asset directories and Wrangler vars: explicit asset trees
include dotfiles and there is no runtime app-secret manager. The package limits
are 32 MiB JSON, 24 MiB decoded files, 64 KiB config, 4096 files and 8192
staging directories. The default revision label is Git HEAD (`-dirty` if
modified), or null without Git; `--source-revision` sets a label, not a source
archive. Rollback is redeployment of a prior revision, not reversal of data
migrations. If publish succeeds but activation fails, the pointer may already
be adopted; check status and logs before retrying.

`CELLD_ESBUILD` overrides project/ancestor `node_modules/.bin/esbuild`; tool
discovery uses npm/pnpm/Yarn lockfiles and `packageManager`. Asset-only or
prebundled projects may not need esbuild. Native release downloads use upstream
GitHub HTTPS and are cached privately under
`${CELLA_CACHE_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/cella}/releases/`;
version is verified on reuse. No automatic provenance attestation check is
provided. For operator lifecycle, old-SSH migration and storage recovery see
[operations](operations.md).
