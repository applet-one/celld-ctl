# celld-ctl

A small Rust control layer for hosting independent [celld](https://celld.dev)
applications on an exe.dev Linux VM with systemd, plus the developer CLI `cella`.
Apps keep their `wrangler.jsonc`; native celld parses and builds them. Each app
gets its own storage prefix, pinned celld release, loopback service and `/SLUG/`
route. Fresh x86_64 installs use local RustFS storage by default.

## Host

```sh
git clone https://github.com/applet-one/celld-ctl.git; cd celld-ctl
cargo build --release --locked -p celld-ctl
sudo scripts/install-host.sh
```

See [host setup](docs/setup-host.md) for prerequisites and storage options.

## Developer machine

Use a key registered to your exe.dev owner account. Skip key generation and
registration if you already have one.

```sh
git clone https://github.com/applet-one/celld-ctl.git; cd celld-ctl
cargo install --locked --path crates/cella --force

mkdir -p "$HOME/.ssh"; chmod 700 "$HOME/.ssh"
ssh-keygen -t ed25519 -f "$HOME/.ssh/cella-owner"
export CELLA_HOST=YOUR_VM.exe.xyz
export CELLA_SSH_KEY="$HOME/.ssh/cella-owner"

cat "${CELLA_SSH_KEY}.pub" | ssh exe.dev ssh-key add
ssh-add "$CELLA_SSH_KEY"  # if the key has a passphrase
ssh -i "$CELLA_SSH_KEY" -o IdentitiesOnly=yes "$CELLA_HOST" true

cd /path/to/your/wrangler-project
cella deploy
cella status
cella logs --lines 50
```

Register the key through `ssh exe.dev`, **not** `ssh "$CELLA_HOST"`; registration
uses an existing exe.dev login. Skip registration if that key is already
registered. Compare the VM host-key fingerprint with a trusted value before
accepting the SSH connection. If the direct hostname fails, use
`vm+YOUR_VM@vm.exe.xyz`. See [dev-machine setup](docs/setup-cella.md).

## Documentation

- [Host setup](docs/setup-host.md) · [Dev-machine setup](docs/setup-cella.md)
- [Operations, migration and security](docs/operations.md) · [Developer CLI](docs/cella.md)
- [Architecture](docs/architecture.md) · [Implementation status](docs/implementation-plan.md)
- [Instance separation](docs/instance-separation.md)

**Repository boundary:** reusable code and generic templates only. Hostnames,
keys, credentials, registries, generated routes and deployed apps stay outside
this checkout. Capacity qualification and production durability remain separate.
