# cella: local development and deployment

`cella` is the developer client, separate from the host/operator `celld-ctl`.
It reads an existing `wrangler.jsonc` (or `wrangler.json`); no replacement
manifest, SSH settings, or credentials belong in that file.

## Install and prerequisites

From this workspace:

```sh
cargo install --locked --path crates/cella
```

Supported prebuilt celld platforms: Linux x86_64, Linux arm64, macOS arm64.
You need `curl` for release downloads; remote commands also need OpenSSH.
Install Node.js and your project's npm/pnpm/Yarn dependencies yourself.
`cella` does not run package installs or fetch an esbuild package implicitly.
The MVP targets JavaScript/TypeScript/esbuild, not a managed Rust/Wasm toolchain.

## Local development

```sh
cella dev --celld-version vX.Y.Z
cella --project ./my-project dev --celld-version vX.Y.Z -- --port 9876 --no-watch
```

Replace `vX.Y.Z` with an **exact published version**. You can also set
`CELLA_CELLD_VERSION`. With an explicit pin, development needs neither a
host nor a deploy key. Without a pin, `dev` reads the existing host target's
version via SSH; it does not provision a target. Native `celld dev` owns
watching, local persistence, HTTP serving, and configuration validation.
Extra native dev options follow `--`.

## SSH configuration

```sh
export CELLA_HOST=cella-deploy@HOST
export CELLA_SSH_KEY="$HOME/.ssh/cella-deploy"
# optional: export CELLA_SSH_PORT=22
```

Equivalent command-line options: `--host`, `--identity`, `--ssh-port`.
Use a dedicated restricted deploy key installed by the operator. CI should
have a different, independently revocable key. A key file is required;
agent-based key discovery is deliberately disabled. Encrypted private keys
requiring an interactive passphrase are unsuitable for this batch transport.

The client ignores SSH configuration files (`-F /dev/null`), so supply a real
hostname or `user@hostname`, not an alias requiring `~/.ssh/config`.
Verify and provision the host key in `known_hosts` out of band before use.
Strict host-key checking remains enabled. The client disables PTY, agent/X11
and port forwarding, multiplexing, and local commands. Only the fixed remote
command `celld-ctl-transport` is sent. Slugs, version IDs, and revisions travel
as JSON on stdin, never as shell command arguments. Its environment contains
only local `PATH`, `HOME`, and `LANG`, not AWS credentials or SSH agent state.
The host must enforce the matching forced command and deploy-key restrictions;
client-side flags are not a substitute for server policy.

## Deploy

```sh
# Standard local AWS credentials, from your normal shell/CI secret provider:
# AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY, optionally AWS_SESSION_TOKEN
cella deploy
cella --project ./my-project deploy --source-revision COMMIT_SHA
cella --slug alternate-route deploy
```

1. Extract `name` from JSONC as the hosted slug (or use `--slug`). Comments,
   quoted strings, and trailing commas are supported. The slug uses native
   Worker-name rules: 1–63 lowercase letters/digits/hyphens, with alphanumeric
   first and last characters. Unknown Wrangler fields are **not** interpreted
   or rejected by `cella`; native celld owns compatibility validation.
2. Ask the host to provision a missing slug and return its non-secret target.
   Invalid native configuration may leave a newly provisioned **disabled**
   allocation, but never triggers activation.
3. Cache the exact target-pinned release and verify `celld --version`. Neither
   a newer nor older binary is accepted. A mismatch fails rather than silently
   overwriting the cache or selecting `latest`.
4. Detect installed npm/pnpm/Yarn tools and run native `celld deploy --json`
   locally with the host's bucket/prefix, HTTPS endpoint, and region. AWS
   environment credentials stay local to celld; none enter SSH requests.
5. Forward native build/validation stderr and deployment JSON stdout unchanged.
   Native failures preserve their exit code and never activate the target.
6. Send the native JSON `version` as `activate.version_id` with a source
   revision. The host enables a first deployment or reloads an existing one,
   checks readiness, and records history.

Source revision labels are 1–200 ASCII letters, digits, `.`, `_`, `/`, or `-`.
The default source revision is Git HEAD, with `-dirty` appended when there
are uncommitted/untracked changes. Without Git it is `null`, with a warning;
`--source-revision` is recommended in CI. A dirty label is not a source archive.
Rollback is redeploying a chosen previous Git revision, not editing R2 pointers.

`--slug` changes host routing identity only; it never rewrites the Worker name
or configuration passed to native celld. Caddy preserves the full `/SLUG/` path.
If publish succeeds but activation fails, the command exits unsuccessfully
and reports the published version. The R2 pointer already exists and a running
node might adopt it independently; this is not a transactional rollback.
Check `status` and `logs` before retrying. The native stdout JSON is still
available to recover the deployment ID.

### Tool discovery

Lockfiles `pnpm-lock.yaml`, `yarn.lock`, `package-lock.json`, and
`npm-shrinkwrap.json`, or a `package.json` `packageManager` field select the
project manager. Ancestor directories support common workspaces. If conflicting
lockfiles exist at one level, precedence is pnpm, Yarn, then npm.
`CELLD_ESBUILD` wins; otherwise `node_modules/.bin/esbuild` in the project or
an ancestor is used. Yarn PnP gets a temporary fixed `yarn exec esbuild`
wrapper, with Corepack networking disabled. Otherwise native celld resolves
`esbuild` on PATH and supplies its own missing-tool errors. Asset-only and
`no_bundle` projects are not rejected for lacking esbuild.

### Release cache and trust

Cache layout:

```text
${CELLA_CACHE_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/cella}/releases/vX.Y.Z/TARGET/celld
```

Assets follow upstream's installer/release workflow:
`https://github.com/denoland/celld/releases/download/vX.Y.Z/celld-TARGET.gz`.
Targets are `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, and
`aarch64-apple-darwin`. These are gzip-compressed binaries, not tar archives.
Downloads require HTTPS (including redirects), disable curl configuration files,
check gzip decompression/CRC and reported version, then install atomically.
Every cache reuse rechecks the version; debug/version-mismatched builds fail.
There is no `latest` fallback and no configurable release mirror.

Trust is upstream GitHub HTTPS plus the local user's cache permissions, matching
upstream installer basics. CRC and version checking are **not** a cryptographic
provenance/signature verification mechanism. Automatic GitHub attestation
verification is not implemented. Keep the cache private to the invoking user.

## Read-only commands

```sh
cella status
cella logs --lines 100             # 1–1000, bounded snapshot, not a stream
cella deployments list
```

`status` and deployment history print JSON; logs print the returned journal
text. These commands need host/key settings, but no local AWS credentials,
release download, or Node toolchain. Use `--slug` to avoid reading a project.
`init` is intentionally omitted: existing Wrangler projects work as-is.

## Wire contract

One JSON request plus newline on SSH stdin, then EOF. The server emits exactly
one JSON response and uses stderr for diagnostics. Responses are limited to
1 MiB while reading; oversized output kills the SSH child and fails the command.
Requests are limited to 16 KiB.

```json
{"op":"provision","slug":"app"}
{"ok":true,"result":{"slug":"app","bucket":"s3://BUCKET/cells/app","endpoint":"https://OBJECT_ENDPOINT","region":"auto","celld_version":"X.Y.Z","enabled":false}}
```

Other requests: `target`, `status`, `deployments`, `logs` (additional `lines`),
and `activate` (additional `version_id`, nullable `source_revision`). The wrapper
is always `{"ok":true,"result":...}` or `{"ok":false,"error":"..."}`.
Logs return `{"text":"..."}`; deployments return an array. Neither requests
nor target metadata contain credentials. Shell banners/non-JSON responses fail.

## Tests

```sh
cargo test -p cella
cargo clippy -p cella --all-targets -- -D warnings
```

Unit and hermetic process tests cover JSONC/name extraction, toolchain lookup,
restricted SSH requests/environment, supported platforms, actual asset URL
shapes, exact pin checking, download/cache failures, native validation failure
propagation, activation gating, history/logs, and host-free local development.
Fake binaries/SSH/curl use temporary directories; no network or AWS is needed.

## Dedicated host transport port

The bundled host installer uses a separate deployment-only SSH daemon bound to
`127.0.0.1:2222`; primary/platform SSH is unchanged. Set `CELLA_SSH_PORT=2222`
(or `--ssh-port 2222`) and arrange an approved private TCP access path, as
explained in [host operations](operations.md#private-remote-transport-connectivity).
Developer storage credentials remain local even when that path uses a tailnet.
