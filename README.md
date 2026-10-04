# celld-ctl

A small Rust control layer for hosting independent [celld](https://celld.dev)
applications on one systemd VM, plus the separate **cella** developer CLI.
Projects keep their existing `wrangler.jsonc`; native celld remains the parser,
bundler and compatibility authority.

Each application gets its own object-store fleet prefix, pinned celld release,
loopback service, allocated ports, local cache and `/APP_SLUG/` route. Caddy owns
port `8000` and preserves the full request path. `/` is a read-only app directory.

```sh
cargo build --release --locked
cargo test --workspace --locked
python3 scripts/test_deploy_keys.py
```

## Host operator

```text
celld-ctl create|enable|disable|start|stop|restart|status|logs|target|reload|remove APP_SLUG
celld-ctl list
celld-ctl deployments APP_SLUG
celld-ctl backup
```

The installer starts a separate, loopback-only deployment SSH daemon on port
`2222`; it never replaces the primary host SSH service. Provide a private TCP
access path before using it from developer machines or CI.

Run locally as root. Remote deploy keys can access only the fixed, bounded JSON
SSH transport, not the operator command surface or an arbitrary shell.

## Developer

```sh
cella dev --celld-version VERSION
cella --host cella-deploy@HOST --identity /path/to/key --ssh-port 2222 deploy
cella --host cella-deploy@HOST --identity /path/to/key --ssh-port 2222 status
cella --host cella-deploy@HOST --identity /path/to/key --ssh-port 2222 deployments list
```

`deploy` auto-provisions the Worker name, downloads/caches the exact host-pinned
release, finds local esbuild, runs native `celld deploy` locally, then activates
or reloads the node. Developer storage credentials come from standard `AWS_*`
variables and are never sent through SSH. Initially supported: Linux x86_64,
Linux arm64 and macOS arm64; JavaScript/TypeScript with npm/pnpm/Yarn projects.

## Documentation

- [Host installation, migration, security and operations](docs/operations.md)
- [Developer CLI](docs/cella.md)
- [Architecture](docs/architecture.md)
- [Implementation plan](docs/implementation-plan.md)
- [Instance separation](docs/instance-separation.md)
- Generic [systemd](examples/systemd/), [SSH](examples/ssh/) and
  [configuration](examples/config/) templates

**Repository boundary:** reusable code, tests and generic artifacts only.
Credentials, hostnames, keys, registries, generated routes, local celld state
and deployed applications stay outside this checkout. Capacity qualification,
Rust/Wasm toolchains, runtime app secrets and multi-host scheduling are deferred.
