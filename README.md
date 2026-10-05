# celld-ctl

A small Rust control layer for hosting independent [celld](https://celld.dev)
applications on an exe.dev Linux VM with systemd, plus the developer CLI `cella`.
Apps keep their `wrangler.jsonc`; native celld parses and builds them. Each app
gets its own storage prefix, pinned celld release, loopback service and `/SLUG/`
route. Fresh x86_64 installs use local RustFS storage by default.

## Host (VM owner)

```sh
git clone https://github.com/applet-one/celld-ctl.git
cd celld-ctl
cargo build --release --locked -p celld-ctl
sudo scripts/install-host.sh
```

Use your **existing exe.dev owner SSH login**; the installer does not set up a
second SSH server, relay, tailnet or deployment key. See [host setup](docs/setup-host.md)
for prerequisites, storage options and [existing-host cleanup](docs/operations.md#existing-host-migration-from-dedicated-deployment-ssh).

## Developer machine (VM owner only)

In a checkout of this repository on your dev machine, use an existing registered
owner key or create a new one (do not overwrite an existing file). For a new key:

```sh
cargo install --locked --path crates/cella --force
ssh-keygen -t ed25519 -f ~/.ssh/cella-owner  # skip if you already have a registered key
cat ~/.ssh/cella-owner.pub | ssh exe.dev ssh-key add
ssh-add ~/.ssh/cella-owner  # if the key has a passphrase
export CELLA_HOST=YOUR_VM.exe.xyz
export CELLA_SSH_KEY="$HOME/.ssh/cella-owner"
cella deploy
cella status
cella logs --lines 50
```

If the direct hostname does not work, use `vm+YOUR_VM@vm.exe.xyz` instead.
For an existing key, substitute its path and skip generation/registration.
Verify the VM host-key fingerprint before adding it to `known_hosts`; see
[dev-machine setup](docs/setup-cella.md) for key generation, exact commands
and troubleshooting. Never put keys or host storage credentials in this repo or
Wrangler. There is no independently scoped CI/developer access in this owner-only
workflow.

## Documentation

- [Host setup](docs/setup-host.md) · [Dev-machine setup](docs/setup-cella.md)
- [Operations, migration and security](docs/operations.md) · [Developer CLI](docs/cella.md)
- [Architecture](docs/architecture.md) · [Implementation status](docs/implementation-plan.md)
- [Instance separation](docs/instance-separation.md)

**Repository boundary:** reusable code and generic templates only. Hostnames,
keys, credentials, registries, generated routes and deployed apps stay outside
this checkout. Capacity qualification and production durability remain separate.
