# celld-ctl

`celld-ctl` is a small Rust control layer for hosting [celld](https://celld.dev) applications on a single Linux VM, with no external managed services required. Developers define their apps in `wrangler.jsonc` and deploy them over SSH using the `cella` CLI.

This is a single-host development and testing setup, not a production HA platform. Fresh x86_64 hosts default to local RustFS storage; external S3-compatible storage is optional.

For app inspirations see https://applet.one/examples

## Set up the host

You'll need a Linux VM with systemd, sudo, a Rust toolchain, Caddy, Python 3, curl and gzip. See [host setup](docs/setup-host.md) for prerequisites, storage options and existing-host precautions.

Use your preferred VPS provider. For example, on exe.dev:

```sh
ssh exe.dev new
ssh OWNER@YOUR_VM_HOST
```

On the host, clone the repository and install `celld-ctl`:

```sh
git clone https://github.com/applet-one/celld-ctl.git
cd celld-ctl
cargo build --release --locked -p celld-ctl
sudo scripts/install-host.sh
```

On arm64, choose `--storage external`, or explicitly opt into unqualified local storage with `--storage local`. Back up existing hosts before installing; reinstalling does not migrate storage.

## Set up your development machine

You'll need Rust, OpenSSH, curl and your app's build tools. The `cella` client supports Linux x86_64/arm64 and macOS arm64.

```sh
git clone https://github.com/applet-one/celld-ctl.git
cd celld-ctl
cargo install --locked --path crates/cella --force
export CELLA_HOST=OWNER@YOUR_VM_HOST
export CELLA_SSH_KEY="$HOME/.ssh/YOUR_VM_KEY"
```

Use a key authorized for the VM owner account, with passwordless sudo for `/usr/local/bin/celld-ctl transport`. Verify the VM's SSH host-key fingerprint before connecting. `cella` does not read `~/.ssh/config`; see [owner SSH setup](docs/cella.md#owner-ssh-setup) for details, including exe.dev key authorization.

## Create and deploy an app

From the directory where you keep your projects:

```sh
cella init my-app
cd my-app
pnpm install
cella deploy
cella status
cella logs --lines 50
```

The starter includes a SQLite-backed Durable Object counter. Use `cella init .` to scaffold an existing directory. npm and Yarn work too. Open the URL printed after deployment.

## Routing and access

Each app gets its own object-store prefix, pinned celld release and loopback runtime (8101–8999). Caddy serves an app directory on port 8000 and each app at `/` on a dedicated port (9101–9999).

On exe.dev, its authenticated HTTPS proxy forwards those ports. Other VMs need equivalent trusted HTTPS forwarding and access control. **Do not expose Caddy's plain HTTP listeners as unauthenticated public services.** Keep native celld and storage private.

## Documentation

- [Host setup](docs/setup-host.md)
- [Owner CLI and SSH setup](docs/cella.md)
- [Host operations, backup and recovery](docs/operations.md)
- [Architecture and security boundary](docs/architecture.md)

## License

Licensed under the [Apache License, Version 2.0](LICENSE).
