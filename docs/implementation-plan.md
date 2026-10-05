# Implementation plan

## Host layer

1. Put Caddy behind exe.dev's authenticated alternate-port proxy: port 8000
   serves the slug directory, and dedicated Caddy ports 9101–9999 serve apps
   at `/` without path prefixes.
2. Run each fleet as a loopback-only systemd service on 8101–8999; map its
   allocated runtime port to external Caddy port +1000.
3. Keep host registry, credentials, generated listeners, and celld state outside this source tree.

## `celld-ctl`

A Rust host CLI backed by SQLite:

```text
celld-ctl create <slug>
celld-ctl enable|disable <slug>
celld-ctl start|stop|restart <slug>
celld-ctl list
celld-ctl status <slug>
celld-ctl logs <slug>
celld-ctl target <slug>
celld-ctl reload <slug>
celld-ctl remove <slug>
```

It uses fixed, validated operations for systemd, Caddy, and configuration; never arbitrary shell execution.

## `cella`

A separate developer CLI:

```text
cella dev
cella deploy
cella deployments list
cella logs
cella status
```

`cella deploy` reads `wrangler.jsonc`, asks the host to provision its slug, builds locally with esbuild/native `celld deploy --dry-run`, then sends a bounded prepared package over the existing exe.dev VM-owner SSH gateway. The host publishes using its own object-store configuration and credentials and enables or reloads the service. The VM owner needs only SSH settings, not bucket configuration or storage credentials. This owner-only workflow does not grant CI a scoped deploy identity.

## Initial scope

- JavaScript/TypeScript + esbuild.
- npm, pnpm, and Yarn lockfile detection.
- Linux x86_64, Linux arm64, and macOS arm64 developer platforms.
- Initial per-app limits: 256 MiB memory and 50% CPU.

## Deferred

Runtime secret management, Rust/Wasm build toolchains, and multi-host scheduling.

## Delivery status

The host registry/lifecycle/target/history/backup commands and separate `cella`
client are implemented with hermetic tests. The installer provides pinned app
units and bounded journal retention. Deployments reuse the existing exe.dev
VM-owner SSH gateway and owner sudo; no second SSH daemon or managed deploy
keys are installed. Native deployment, reload and durable-state
restart behavior have been exercised against a real object store.
Dedicated per-app port routing replaces the former path-based public routes.
Previous native/storage smoke results are not validation of the new routing.

As of October 5, 2026, the **fresh x86_64** no-flag installer selects local
RustFS for single-node development/testing; `--storage external` opts out.
A fresh arm64/aarch64 no-flag install refuses to choose without native arm64
qualification; it requires `--storage local` or `--storage external`.
Existing host modes, app pins and objects are preserved on reinstall. The
[minimum live gate](rustfs-default-storage-plan.md#7-compatibility-gate-do-this-before-making-rustfs-the-default)
passed in a disposable x86_64 Ubuntu 24.04 systemd Docker container, including
historical restricted-SSH deployment (old transport) and named Durable Object state after app and RustFS
restarts and archiving the local app cache. This is not VM/arm64, cold-snapshot
restore, abrupt-kill or HA qualification.

The VM owner registers their public key with exe.dev (if needed), verifies
the VM host-key fingerprint and uses the normal SSH gateway destination. The
owner account requires non-interactive sudo; it is not a restricted deploy
principal. Existing hosts should follow the
[old-service migration](operations.md#existing-host-migration-from-dedicated-deployment-ssh). Full 10/25/50-app capacity qualification
remains an operational task; the explicit load probe is provided but installation
never provisions or stresses production fleets automatically. The optional
starter command is not implemented; existing Wrangler projects need no replacement
manifest or edits. Runtime app-secret management and additional Rust/Wasm toolchains
remain deferred.
