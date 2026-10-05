# celld-ctl

A small Rust control layer for hosting independent [celld](https://celld.dev) applications on a Linux VM.

Developers keep their apps defined in `wrangler.jsonc` and use the CLI `cella` to deploy them.

Each app gets its own storage prefix, pinned celld release, loopback service and `/SLUG/`
route. Fresh installs use local RustFS storage by default.

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
