# celld-ctl

A small Rust control layer for hosting independent [celld](https://celld.dev)
applications on a Linux VM with systemd, plus the developer CLI `cella`.
Apps keep their `wrangler.jsonc`; native celld parses and builds them. Each app
gets its own storage prefix, pinned celld release and loopback service. The
dedicated-port routing scheme assigns each app a Caddy port (9101–9999) that
serves the app at `/`, through exe.dev's authenticated alternate-port proxy;
port 8000 is a directory of slug links to those ports, not an app router.
The app runtime stays on loopback ports 8101–8999. Fresh x86_64 installs use
local RustFS storage by default.

## Host

```sh
git clone https://github.com/applet-one/celld-ctl.git; cd celld-ctl
cargo build --release --locked -p celld-ctl
sudo scripts/install-host.sh
```

See [host setup](docs/setup-host.md) for prerequisites and storage options.

## Developer machine

```sh
git clone https://github.com/applet-one/celld-ctl.git; cd celld-ctl
cargo install --locked --path crates/cella --force

export CELLA_HOST=YOUR_VM_SSH_HOST
export CELLA_SSH_KEY="$HOME/.ssh/YOUR_VM_KEY"

cd /path/to/your/wrangler-project
cella deploy
cella status
cella logs --lines 50
```

See [dev-machine setup](docs/setup-cella.md) for SSH prerequisites.

## Documentation

- [Host setup](docs/setup-host.md) · [Dev-machine setup](docs/setup-cella.md)
- [Operations, migration and security](docs/operations.md) · [Developer CLI](docs/cella.md)
- [Architecture](docs/architecture.md) · [Implementation status](docs/implementation-plan.md)
- [Instance separation](docs/instance-separation.md)

**Repository boundary:** reusable code and generic templates only. Hostnames,
keys, credentials, registries, generated routes and deployed apps stay outside
this checkout. Capacity qualification and production durability remain separate.
