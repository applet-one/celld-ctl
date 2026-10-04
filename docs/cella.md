# cella: local development and SSH-only deployment

First complete **[A. Host setup](setup-host.md)**, then follow
**[B. Configure cella on your dev machine](setup-cella.md)**. B includes generating
a dedicated SSH key, operator enrollment and verified `known_hosts` setup. This
page is the detailed client/protocol reference, not the onboarding checklist.

`cella` is the developer client, separate from the host/operator `celld-ctl`.
Keep your existing `wrangler.jsonc` (or `wrangler.json`). **Deployment requires
only the restricted SSH connection: no local R2 credentials, bucket name,
endpoint, or region.** Both client and host must use version 0.2.0 or later;
the original 0.1 client used direct-to-object-store publication.

## Install and prerequisites

```sh
cargo install --locked --path crates/cella
```

Supported developer platforms: Linux x86_64, Linux arm64, macOS arm64. Install
Node.js and your project's npm/pnpm/Yarn dependencies, including esbuild when
bundling is required. `cella` does not run package installs. `curl` is required
for exact native release downloads, and OpenSSH for remote operations.

## Configure SSH once

If you have not created/enrolled a key and verified the server identity, follow
[B2–B4](setup-cella.md#b2-generate-a-dedicated-deployment-key) first.

```sh
export CELLA_HOST=cella-deploy@PRIVATE_HOST
export CELLA_SSH_PORT=2222
export CELLA_SSH_KEY="$HOME/.ssh/cella-deploy"
```

Equivalent flags are `--host`, `--ssh-port`, `--identity`. Use a dedicated
restricted key enrolled by the host operator; CI should have a distinct,
independently revocable key. Private keys stay on your machine or CI runner.
The batch client does not use an agent or prompt for key passphrases.

Supply a real hostname, not an alias requiring `~/.ssh/config`: client SSH uses
`-F /dev/null`. Host-key verification is strict; enroll the dedicated server's
key in `known_hosts` after checking its fingerprint through a trusted channel.
The client disables PTY, agent/X11/port forwarding, multiplexing and local
commands. It sends only the fixed command `celld-ctl-transport`; parameters and
prepared files are data on stdin, never shell commands. SSH inherits only
`PATH`, `HOME`, `LANG`, not AWS credentials or agent state.

The bundled host daemon listens only on `127.0.0.1:2222`. Remote machines need
an approved private TCP access path; normal platform/VM-owner SSH is not the
restricted deployment account. See [host operations](operations.md#private-remote-transport-connectivity).
The client's default port remains 22 for conventional SSH installations.

## Deploy

From your existing Wrangler project:

```sh
cella deploy
cella --project ./my-project deploy
cella --slug alternate-route deploy
cella deploy --source-revision COMMIT_SHA  # useful in CI
```

1. Extract the Worker name as the hosted slug (or use `--slug`) and ask the
   host to provision it. The host returns its exact native celld pin, not storage
   configuration. Invalid builds may leave a disabled allocation, never a route.
2. Download/cache and verify that exact native release. Run native `celld deploy
   --dry-run --json` locally, with local esbuild and a fixed dummy bucket argument.
   No object-store client is created in this dry run; storage environment variables
   are removed from the build process. Native configuration/build errors keep
   their exit code. No package or user build commands execute on the VM.
3. Capture the resulting JavaScript/standard WASM modules and explicit static
   assets. Send the bounded prepared package and source revision over SSH. Do not
   send the repository, node_modules, local credential files, or build executables.
4. The host independently validates paths and package bounds, stages immutable
   regular files and constructs a private deployment-only config copy. It uses
   `no_bundle` with the already-built module, preserving runtime settings while
   removing build-only `define`/`rules` fields and replacing source paths. Your
   original Wrangler file is never rewritten. Native celld validates this copy
   and its version must match the local build before publication.
5. The host's dedicated unprivileged publisher runs exact pinned native celld
   with host-owned storage settings and credentials. It uploads using native
   publication semantics, then the manager verifies the durable pointer,
   activates/reloads the app, checks readiness and records history.

Only the host has storage credentials. Existing local AWS settings are neither
needed nor sent to it. Bucket allocation and credentials remain operator state.
Native publish JSON is printed on stdout; build/publish progress is stderr.

`--slug` changes routing identity only; it does not rename the native Worker.
Caddy preserves the full `/SLUG/` path. Initial support is JavaScript/TypeScript,
asset-only Workers, prebundled `no_bundle` projects and standard WASM modules.
Container/Python builds and nonstandard copied module extensions are rejected
rather than executing user build tools on the host. Legacy bucket-root imports
are protected from this automated publishing path; updates need an operator.

Transport policy: at most 32 MiB of package JSON, 24 MiB decoded files, 64 KiB
configuration JSON, 4096 files, 8192 staging directories, and safe relative paths
up to 512 bytes. These are independent of native limits.
Use a dedicated `public`/`dist` assets directory: every regular file in the
explicit asset tree is included, including dotfiles. Native forbids symlinks,
special files, root `.assetsignore` and root `_worker.js`; `_headers` and
`_redirects` remain meaningful metadata. Do not put credentials in your assets
folder or Wrangler vars. No runtime app-secret manager is provided.

The default source revision is Git HEAD, suffixed `-dirty` for uncommitted or
untracked changes; without Git it is null with a warning. Explicit revisions are
1–200 ASCII letters/digits or `.`, `_`, `/`, `-`. A revision is a label, not a
source archive. Rollback means redeploying a previous source revision. If native
publication succeeds but activation fails, the command fails with the published
version and recovery guidance; a running node can already adopt that pointer.
Check `status`/`logs` before retrying. There is no distributed rollback transaction.

## Local development

```sh
cella dev --celld-version X.Y.Z
cella --project ./my-project dev --celld-version X.Y.Z -- --port 9876 --no-watch
# Or: export CELLA_CELLD_VERSION=X.Y.Z; cella dev
```

An explicit exact pin needs neither SSH nor credentials. Without one, `dev`
obtains an existing target's pin via SSH; it does not provision a target.
Native celld owns watching, local state and serving. Extra native args follow `--`.

## Read-only commands

```sh
cella status
cella logs --lines 100             # 1–1000, bounded snapshot
cella deployments list
cella --slug APP_SLUG status        # no project directory needed
```

These require only SSH settings. Status/history are JSON; logs are journal text.
The optional starter command is omitted: existing projects work as-is.

## Tool discovery and cache

`CELLD_ESBUILD` wins; otherwise project/ancestor `node_modules/.bin/esbuild`
is detected. npm/pnpm/Yarn lockfiles and `packageManager` select tooling; Yarn
PnP uses a fixed local wrapper with Corepack networking disabled. Native missing-
tool errors remain authoritative; asset-only/prebundled projects need no esbuild.

Exact releases are cached in
`${CELLA_CACHE_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/cella}/releases/vX.Y.Z/TARGET/celld`.
Downloads use upstream GitHub HTTPS gzip assets; every reuse verifies the native
version. There is no latest fallback, release mirror, or automatic provenance
attestation verification. Keep the cache private to the invoking user.

## Wire contract

Ordinary requests remain one JSON document followed by EOF, limited to 16 KiB.
Deploy uses a <=16 KiB JSON header followed by newline, then **exactly**
`bundle_size` bytes of prepared-package JSON, then EOF. The header is:

```json
{"op":"deploy","slug":"app","celld_version":"X.Y.Z","version_id":"16hex","source_revision":"COMMIT_SHA","bundle_size":1234}
```

The package contains `config`, `modules`, `assets`; file records have `path` and
base64 `content`. It carries no target paths, credentials or executable commands.
The server independently validates it, rejects trailing/truncated data and
returns the existing bounded `{ok:true,result:...}` / `{ok:false,error:...}` wrapper.
Ordinary/incomplete headers have a 30-second input deadline; a valid deploy
header extends total upload time to 180 seconds, including EOF. The operation
has a separate overall failsafe and bounded native phases.
Responses are <=1 MiB; banners/non-JSON responses fail. Native storage failures
are redacted before reaching the client.

## Tests

```sh
cargo test -p cella
cargo clippy -p cella --all-targets -- -D warnings
```

Hermetic native/SSH/tool fixtures and host tests cover local credential-free
builds, capture, bounded packages, exact pin/version checks, rejected paths,
native failure gating and publish/activate behavior. Real instance deployments
are operator integration tests, not automatic production load tests.
