# Implementation plan

## Host layer

1. Install Caddy as the public listener.
2. Run each fleet as a loopback-only systemd service.
3. Keep host registry, credentials, generated routes, and celld state outside this source tree.

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

`cella deploy` reads `wrangler.jsonc`, asks the host to provision its slug, runs local esbuild/native `celld deploy`, then asks the host to enable or reload the service. R2 deploy credentials remain on the developer machine or CI.

## Initial scope

- JavaScript/TypeScript + esbuild.
- npm, pnpm, and Yarn lockfile detection.
- Linux x86_64, Linux arm64, and macOS arm64 developer platforms.
- Initial per-app limits: 256 MiB memory and 50% CPU.

## Deferred

Runtime secret management, Rust/Wasm build toolchains, and multi-host scheduling.
