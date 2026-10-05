# B. Set up cella on the owner's dev machine

Follow [A. Host setup](setup-host.md) first. This workflow is for an **exe.dev VM
owner**, using that owner's existing SSH gateway identity. There is no separate
`cella-deploy` login, deployment SSH daemon, relay or private network. `cella`
needs no host bucket, endpoint, region or object-store credentials. Existing
application R2 bindings in Wrangler are unrelated to host storage.

## B1. Install cella and local build tools

```sh
cargo install --locked --path crates/cella --force
cella --version
```

Use matching client/host version 0.2.0 or later; the 0.1 client published
directly to object storage. You need Rust to build `cella`, an OpenSSH client,
`curl` for exact native release downloads, and your project's Node.js package
tools and dependencies (including esbuild when needed). Supported developer
platforms: Linux x86_64/arm64 and macOS arm64.

## B2. Register the owner's public key with exe.dev

If you already SSH into the VM with an exe.dev-registered key, reuse it and
substitute its path throughout. For a new key, **on your dev machine**, choose
a new filename if the path exists:

```sh
mkdir -p "$HOME/.ssh"
chmod 700 "$HOME/.ssh"
ssh-keygen -t ed25519 -f "$HOME/.ssh/cella-owner" -C 'exe.dev VM owner'
chmod 600 "$HOME/.ssh/cella-owner"
ssh-keygen -lf "$HOME/.ssh/cella-owner.pub"
cat "$HOME/.ssh/cella-owner.pub" | ssh exe.dev ssh-key add
```

The fingerprint from `ssh-keygen -lf` identifies **your authentication key**,
not the VM server. Only the `.pub` file goes to exe.dev; never upload or commit
the private key. The `ssh exe.dev` registration command must authenticate with
an already registered owner key; if it cannot, use your account's existing key
registration flow first. exe.dev's [ssh-key command](https://exe.dev/docs/cli-ssh-key.md)
documents the pipe form, listing and revoking account keys. Avoid registering a
new key when the old one suffices. Prefer a passphrase and unlock the key in
your **local** agent with `ssh-add "$HOME/.ssh/cella-owner"` before using
`cella`. Alternatively, generate a key with `-N ''` if you cannot use an
agent. `cella` runs SSH in batch mode: it cannot prompt for a passphrase on
demand. Protect the private file (mode 0600), keep it on your dev machine and
revoke it at exe.dev if compromised. Do not reuse it for CI.

## B3. Select your actual VM SSH destination

First confirm ordinary SSH from your dev machine works:

```sh
ssh -i "$HOME/.ssh/cella-owner" -o IdentitiesOnly=yes YOUR_VM.exe.xyz
# If that form fails, use the gateway form instead:
ssh -i "$HOME/.ssh/cella-owner" -o IdentitiesOnly=yes vm+YOUR_VM@vm.exe.xyz
```

These are **placeholders**, not names of this project's VM. exe.dev documents
[both destination forms](https://exe.dev/docs/faq/ssh-destination.md). Use the
working, literal destination in your shell (do not include `ssh `):

```sh
export CELLA_HOST=YOUR_VM.exe.xyz
# Or: export CELLA_HOST=vm+YOUR_VM@vm.exe.xyz
export CELLA_SSH_KEY="$HOME/.ssh/cella-owner"
# Port 22 is the default; do not use the old deployment port 2222.
```

`cella` ignores `~/.ssh/config` (`-F /dev/null`), so aliases, ProxyJump and
custom proxy commands there are not used. Set `CELLA_SSH_KEY` explicitly to
the registered owner key (or pass `--identity`); `IdentitiesOnly=yes` prevents
accidentally offering another account's key. `CELLA_HOST`, `CELLA_SSH_KEY` and
`CELLA_SSH_PORT` can also be set with `--host`, `--identity` and `--ssh-port`. Keep them out of `wrangler.jsonc`.

## B4. Verify the VM's SSH host key

`cella` uses strict host-key checking: establish a trusted `known_hosts` entry
for the **exact hostname and port** of the working destination before running it.
On the first ordinary `ssh` connection, compare the presented **VM host-key
SHA256 fingerprint** with one obtained independently through trusted VM access
or the operator; reject an unexpected key. If you cannot establish a trusted
fingerprint, ask the platform/operator for help instead of blindly accepting it.
Never copy an instance-specific fingerprint into this repository.
For a key retrieved out of band in a file:

```sh
ssh-keygen -lf /path/to/trusted-vm-host-key.pub
```

Do not mistake your `.pub` authentication-key fingerprint in B2 for the
server fingerprint. exe.dev publishes a fingerprint for `ssh exe.dev` in its
[host-key FAQ](https://exe.dev/docs/faq/host-key.md); that FAQ does **not**
claim the fingerprint also applies to `YOUR_VM.exe.xyz` or `vm.exe.xyz`.
A scan alone does **not** authenticate the VM. For the direct-hostname form
only, you can inspect a scanned key without trusting it:

```sh
ssh-keyscan -t ed25519 YOUR_VM.exe.xyz > "$HOME/.ssh/cella-vm-key.scan"
ssh-keygen -lf "$HOME/.ssh/cella-vm-key.scan"
# Only after an independent fingerprint match:
cat "$HOME/.ssh/cella-vm-key.scan" >> "$HOME/.ssh/known_hosts"
chmod 600 "$HOME/.ssh/known_hosts"
```

For `vm+YOUR_VM@vm.exe.xyz`, do **not** assume a scan of
`YOUR_VM.exe.xyz` is interchangeable. Check the fingerprint presented by
`ssh -i "$CELLA_SSH_KEY" -o IdentitiesOnly=yes vm+YOUR_VM@vm.exe.xyz`
against a trusted value for that destination before accepting the entry.
Never bypass a mismatch with `StrictHostKeyChecking=no`; investigate rotation
with the VM owner.

## B5. Deploy your existing Wrangler project

Install your project's dependencies; from its `wrangler.jsonc`/`wrangler.json`
directory:

```sh
cella deploy
cella status
cella logs --lines 50
cella deployments list
```

The first deploy provisions the app automatically. The host publishes using
host-only credentials; `cella` builds and uploads a bounded prepared package.
Your Wrangler file is not rewritten. The Worker name becomes `/name/` at the
operator's app URL; `--slug` overrides the routing slug. Caddy keeps that
prefix, so account for it in your app's router. The transport runs `sudo -n /usr/local/bin/celld-ctl transport` through the
owner login, requiring non-interactive sudo access. Ordinary owner `ssh` may grant
a full shell: `cella` requests only the fixed transport command, but the SSH
identity is **not** restricted to deploying. Do not distribute it to CI or
untrusted users.

Optional local development without SSH or storage credentials:

```sh
cella dev --celld-version X.Y.Z
```

Without an explicit pin, `cella dev` can fetch an already deployed target's
pin via SSH. Native celld handles the local watcher and state.

## Troubleshooting

| Symptom | Check |
| --- | --- |
| SSH DNS/connection failure | Use the [documented VM destination](https://exe.dev/docs/faq/ssh-destination.md), not the app HTTPS URL; try `vm+YOUR_VM@vm.exe.xyz`. Port is 22. |
| `Permission denied (publickey)` | Does `ssh -i "$CELLA_SSH_KEY" -o IdentitiesOnly=yes "$CELLA_HOST"` work with the registered owner key? For passphrase keys, run `ssh-add "$CELLA_SSH_KEY"` first. |
| Host-key verification failure | B4: compare the **VM** fingerprint for the actual destination; do not auto-accept changed keys. |
| Remote transport rejected | Reinstall/update host binaries and verify owner SSH command policy with [host operations](operations.md#owner-ssh-transport-and-trust-boundary). |
| Unknown app from `status` | Run the first successful `cella deploy`. |
| esbuild missing | Install project dependencies/esbuild or set `CELLD_ESBUILD` locally. |
| Native storage/publish error | Operator checks [host storage](setup-host.md#a3-configure-storage-on-the-host-only); never add host storage credentials locally. |
| Pin/version mismatch | Check client/host compatibility and exact native pin, not `latest`. |
| Publish succeeded but activation failed | Check status/logs before retrying; the published pointer may already be adopted. |

Detailed CLI/protocol behavior: [developer reference](cella.md). Backups and
old SSH cleanup: [host operations](operations.md).
