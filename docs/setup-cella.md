# B. Configure and set up cella on your dev machine

**Run these steps on your laptop/workstation or CI runner—not the host VM.**
The operator must first complete [A. Set up celld-ctl](setup-host.md).
You need an existing Wrangler Worker project and restricted SSH access, but
**no host object-store credentials, bucket name, endpoint or region**.
Application R2 bindings in Wrangler, if used, are separate from the host's
storage provider.

## B1. Install/update cella

From this repository on your development machine:

```sh
cargo install --locked --path crates/cella --force
cella --version
```

Use client/host version 0.2.0 or later. The original 0.1 client published directly
to object storage and is not compatible with the new host deployment targets.
You need a Rust toolchain to build this CLI, curl for native release downloads,
OpenSSH client, and your project's Node.js/npm/pnpm/Yarn dependencies. Bundled
JS/TS projects need local esbuild; the host does not install or run your build
tools. Supported developer platforms are Linux x86_64/arm64 and macOS arm64.

## B2. Generate a dedicated deployment key

On **your dev machine**. If `~/.ssh/cella-deploy` already exists, choose another
filename and use it throughout this guide; do not overwrite an existing key.

```sh
mkdir -p "$HOME/.ssh"
chmod 700 "$HOME/.ssh"
ssh-keygen -t ed25519 -f "$HOME/.ssh/cella-deploy" -N '' -C 'cella developer laptop'
chmod 600 "$HOME/.ssh/cella-deploy"
cat "$HOME/.ssh/cella-deploy.pub"
```

Do not overwrite an existing key at that path; use another filename if needed.
This creates:

| File | Purpose | Share it? |
| --- | --- | --- |
| `~/.ssh/cella-deploy` | Your **private** deployment key | **Never**; keep it on this machine |
| `~/.ssh/cella-deploy.pub` | Your public authentication key | Send it to the host operator |

The empty passphrase is intentional: this MVP uses batch SSH, disables agent
lookup, and cannot prompt to unlock encrypted keys. Protect the private file
and use a dedicated, revocable key—not your administrator key. Generate a
separate key for CI and keep its private half in your CI secret store.

Send **only the `.pub` file** via an authenticated administrator channel. The
operator runs A6 to enroll it; wait for confirmation. The restricted deployment
account cannot receive files with SFTP/scp and cannot run `ssh-copy-id`.

## B3. Configure the SSH destination

Obtain the private deployment hostname/IP, port, server-key fingerprint, and
application URL from the operator. Set these on **your dev machine**:

```sh
DEPLOY_HOST=PRIVATE_DEPLOY_HOST  # replace with the real private hostname/IP
export CELLA_HOST="cella-deploy@$DEPLOY_HOST"
export CELLA_SSH_PORT=2222
export CELLA_SSH_KEY="$HOME/.ssh/cella-deploy"
```

These variables may be saved in your personal shell profile (for example,
`~/.zshrc` or `~/.bashrc`). Store only connection settings there, not private-key
contents. You can instead use `--host`, `--ssh-port`, `--identity` on each call.
Do not add SSH or storage settings to `wrangler.jsonc`.

`cella` deliberately ignores `~/.ssh/config` and uses a literal hostname,
explicit key and port. SSH aliases, ProxyJump/ProxyCommand and agent discovery
are not used. The application HTTPS hostname or ordinary VM-owner SSH address
is **not necessarily** the deployment endpoint. Join the approved private
network if required; a valid key alone cannot reach a loopback-only listener.

If you are the VM owner and prefer existing administrator SSH rather than a
tailnet, use the [SSH stdio relay](ssh-stdio-relay.md). It presents the dedicated
service on your own `127.0.0.1:2222`; follow that guide for destination/fingerprint
setup. No `cella` reinstall or TCP-forwarding permission is required.

## B4. Verify and enroll the dedicated SERVER host key

Strict host-key checking is enabled; `cella` will not auto-trust a new server.
This key is **different from your own deploy key** generated in B2.

Fetch the public server key over the approved network path:

```sh
ssh-keyscan -p "$CELLA_SSH_PORT" -t ed25519 "$DEPLOY_HOST" \
  > "$HOME/.ssh/cella-host-key.scan"
ssh-keygen -lf "$HOME/.ssh/cella-host-key.scan"
```

**Stop and compare the reported SHA256 fingerprint with the value supplied
by the operator through a trusted channel (A7).** `ssh-keyscan` alone does not
authenticate a server. Do not append the key if it differs or the operator has
not provided a fingerprint.

Only after a match:

```sh
cat "$HOME/.ssh/cella-host-key.scan" >> "$HOME/.ssh/known_hosts"
chmod 600 "$HOME/.ssh/known_hosts"
```

The scan records the exact hostname and port, such as `[PRIVATE_DEPLOY_HOST]:2222`.
Repeat verification for a different hostname/IP/port. Never work around an error
with `StrictHostKeyChecking=no`. A changed key requires operator confirmation
of an intentional server-key rotation, not blindly deleting known-host entries.

## B5. Deploy your existing project

Open a terminal in the directory containing your `wrangler.jsonc` or
`wrangler.json`. Install your project's dependencies with its usual package
manager. If required and missing, install esbuild as a development dependency
(e.g. `npm install --save-dev esbuild` for npm projects).

```sh
cella deploy
cella status
cella logs --lines 50
cella deployments list
```

Or use `cella --project /path/to/project deploy` from another directory.
**The first command creates the app automatically**; status/history are useful
*after* that first deployment. There is no need for an operator to pre-create
it or for you to configure a local bucket, region, endpoint or storage keys.

`cella` downloads the exact host-pinned native celld release, validates/bundles
locally without storage credentials, and uploads prepared files over restricted
SSH. The host publishes using its own credentials, verifies the deployment,
activates/reloads the service and records the source revision. Wrangler files
are not rewritten. The project's Worker `name` becomes `/name/` under the
operator's application base URL; `--slug` can override that route identity.
Caddy preserves the whole `/name/` path, so your router must account for it.

You cannot test this account by requesting an ordinary SSH shell: that is
intentionally rejected. Use `cella` commands, not arbitrary SSH commands.

## B6. Optional local development

```sh
cella dev --celld-version X.Y.Z  # replace with an exact supported release
```

With an explicit pin, this works without SSH or storage credentials. After a
successful deployment, `cella dev` can obtain the existing app's pin through
SSH without setting a separate version. Native celld owns watching/local state.

## Troubleshooting

| Symptom | Check |
| --- | --- |
| Connection refused, timeout or DNS failure | Private network and deployment hostname/port from A5; not the HTTPS proxy or primary SSH endpoint |
| `Permission denied (publickey)` | Correct private file, `cella-deploy` username, enrolled public key/label, no required key passphrase |
| Host-key verification failed | B4 for the **dedicated** service and exact hostname/port; compare the operator's fingerprint |
| Unknown app from `status` | Run the first successful `cella deploy`; status does not create apps |
| esbuild missing | Install the project's dependencies/local esbuild; or configure `CELLD_ESBUILD` locally |
| Native storage/publish error | Ask the host operator to check [host storage](setup-host.md#a3-configure-storage-on-the-host-only) and, on default x86_64 local-storage hosts, [RustFS health](operations.md#local-storage-service-and-recovery-when-installed); on external hosts, check the configured endpoint and bucket. Do **not** add host storage credentials on your laptop |
| Pin/version mismatch | Check client/host 0.2 compatibility and exact native pin; do not substitute latest |
| Publish succeeded but activation failed | Inspect status/logs and retry the same source; the published pointer can already be adopted |

For CI, follow B2–B5 with a separate key, private network access, a verified
`known_hosts` entry and `--source-revision COMMIT_SHA`. Only SSH credentials
belong in the CI deployment setup; no host object-store secret is needed.

Protocol, transport limits, tooling, rollback caveats and detailed behavior:
[developer reference](cella.md). Operator key revocation and backups:
[host operations](operations.md).
