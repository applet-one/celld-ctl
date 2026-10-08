# celld-ctl

`celld-ctl` is a tiny Rust control layer for hosting [celld](https://celld.dev) applications on a single VM with no external dependencies. Developers keep their apps defined in `wrangler.jsonc` and use the CLI `cella` to deploy them. The goal is a simple testing setup to run cell based apps.

## Setup

### Host Machine

```sh
git clone git@github.com:applet-one/celld-ctl.git; cd celld-ctl
cargo build --release --locked -p celld-ctl
sudo scripts/install-host.sh
```

See [host setup](docs/setup-host.md) for prerequisites, external storage and
existing-host precautions.

### Dev machine

Setup:
```sh
git clone git@github.com:applet-one/celld-ctl.git; cd celld-ctl
cargo install --locked --path crates/cella --force

export CELLA_HOST=YOUR_VM_SSH_HOST
export CELLA_SSH_KEY="$HOME/.ssh/YOUR_VM_KEY"
```

## App development
```sh
cella init
cella deploy
cella status
cella logs
```
## Documentation

See [owner SSH setup](docs/cella.md#owner-ssh-setup) for key authorization
and VM host-key verification.


Each app gets its own object-store prefix, pinned celld release and loopback runtime (8101–8999). Caddy serves a slug directory on port 8000 and each app at `/` on a dedicated port (9101–9999). On exe.dev, its authenticated HTTPS proxy forwards those ports; other VMs need equivalent trusted HTTPS/port forwarding and owner SSH access. 

Fresh x86_64 hosts default to local RustFS for single-node development/testing; external S3-compatible storage is optional.


- [Host setup](docs/setup-host.md) · [Owner CLI and SSH setup](docs/cella.md)
- [Operations, migration, backup and historical test limits](docs/operations.md)
- [Architecture and security boundary](docs/architecture.md)
