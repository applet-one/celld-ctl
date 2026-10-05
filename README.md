# celld-ctl

A small Rust control layer for hosting independent [celld](https://celld.dev)
applications on a Linux VM with systemd, with a separate developer CLI, `cella`.

Apps keep their existing `wrangler.jsonc`; native celld remains the parser,
bundler and compatibility authority.

Each application gets its own object-store fleet prefix, pinned celld release,
loopback service, allocated ports, local cache and `/APP_SLUG/` route.

The default installation includes local object storage powered by RustFS.

## Set up the host

On a fresh Linux VM:

```sh
git clone https://github.com/applet-one/celld-ctl.git
cd celld-ctl
cargo build --release --locked -p celld-ctl
sudo scripts/install-host.sh
```

This provisions local RustFS storage. No cloud storage account required.
See [host setup](docs/setup-host.md) for prerequisites, arm64 instructions and
external storage options.

## Set up your dev machine

On you our dev machine:

### SSH access

Use your existing administrator SSH access to enroll a separate deployment key.
Complete host setup first; replace `admin@vm` with your usual SSH destination.

**1. Create a key on your dev machine.** Choose a different filename if the key already exists.

```sh
ssh-keygen -t ed25519 -f ~/.ssh/cella-deploy -N '' -C 'cella developer laptop'
```

Keep the private key on your dev machine. The empty passphrase is required by
`cella`'s current batch SSH transport.

**2. Copy only the public key using your existing SSH access.**

```sh
scp ~/.ssh/cella-deploy.pub admin@vm:~/cella-deploy.pub
```

**3. Log in to the VM and enroll the key.**

```sh
ssh admin@vm
sudo celld-deploy-key add developer-laptop "$HOME/cella-deploy.pub" --kind owner
sudo ssh-keygen -lf /etc/celld-ctl/ssh-host-ed25519-key.pub
```

The last command prints the deployment server's fingerprint; save it for
verification on your dev machine. Use your administrator account for these
steps, not the restricted `cella-deploy` account.

Key enrollment grants access but does not set up connectivity. Obtain the private
deployment hostname, port `2222`, username `cella-deploy` and application URL from
the host operator. See [Dev-machine setup](docs/setup-cella.md) for details.

### Configure cella

1. Install or update `cella` (client and host must be `0.2.0` or later).
2. Configure the private SSH destination, key file and port explicitly.
3. Verify the server fingerprint before adding its key to `known_hosts`.

See [Dev-machine setup](docs/setup-cella.md) for commands and troubleshooting.

**Developers and CI need no host object-store credentials or bucket
configuration.** Never put private keys in Wrangler or this repository.

## Deploy an app

In your existing Wrangler project, install dependencies with your usual package
manager, then run:

```sh
cella deploy
cella status
cella logs --lines 50
cella deployments list
```

`cella deploy` creates the app automatically, downloads the exact host-pinned
celld release, builds locally and uploads the prepared package over restricted
SSH. The host publishes with its own storage credentials, then activates or
reloads the app.

Supported dev platforms: Linux x86_64/arm64 and macOS arm64, with
JavaScript/TypeScript projects using npm, pnpm or Yarn.

## Documentation

- [A. Host setup](docs/setup-host.md)
- [B. Dev-machine setup and SSH keys](docs/setup-cella.md)
- [Host installation, migration, security and operations](docs/operations.md)
- [Developer CLI](docs/cella.md)
- [Owner SSH stdio relay (no Tailscale/TCP forwarding)](docs/ssh-stdio-relay.md)
- [Architecture](docs/architecture.md)
- [Implementation plan](docs/implementation-plan.md)
- [Instance separation](docs/instance-separation.md)
- Generic [systemd](examples/systemd/), [SSH](examples/ssh/) and
  [configuration](examples/config/) templates

**Repository boundary:** reusable code, tests and generic artifacts only.
Credentials, hostnames, keys, registries, generated routes, local celld state
and deployed applications stay outside this checkout. Capacity qualification,
Rust/Wasm toolchains, runtime app secrets and multi-host scheduling are deferred.
