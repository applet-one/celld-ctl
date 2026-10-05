# celld-ctl

A small Rust control layer for hosting independent [celld](https://celld.dev)
applications on a systemd VM, plus the separate developer CLI, `cella`.

Apps keep their existing `wrangler.jsonc`; native celld remains the parser,
bundler and compatibility authority.

Each application gets its own object-store fleet prefix, pinned celld release,
loopback service, allocated ports, local cache and `/APP_SLUG/` route.

## Set up the host

On your Linux VM, using your administrator account:

```sh
git clone https://github.com/applet-one/celld-ctl.git
cd celld-ctl
cargo build --release --locked -p celld-ctl
sudo scripts/install-host.sh
```

Next, install the pinned celld release, configure storage, and set up Caddy and
private SSH access. See [Host setup, A2–A5](docs/setup-host.md#a2-install-the-exact-native-celld-release)
for commands, and [docs/setup-host.md](docs/setup-host.md) for prerequisites.

## Set up your dev machine

On your laptop/workstation or CI runner—not on the host VM.

### SSH access

Generate a dedicated deploy key on your dev machine ([instructions](docs/setup-cella.md#b2-generate-a-dedicated-deployment-key)).
Send only the `.pub` file to the host operator, who runs these commands **on the VM**
(replace the example path):

```sh
sudo celld-deploy-key add developer-laptop /path/to/cella-deploy.pub --kind owner
sudo ssh-keygen -lf /etc/celld-ctl/ssh-host-ed25519-key.pub
```

The operator provides the private deployment hostname, port `2222`, username
`cella-deploy`, server fingerprint and application URL. Keep your private deploy
key on your dev machine or in your CI secret store.

### cella

1. Install/update `cella` (client and host must be `0.2.0` or later).
2. Configure the private SSH destination, explicit key file and port.
3. Verify the server fingerprint before adding its key to `known_hosts`.

See [Dev-machine setup](docs/setup-cella.md) for commands and troubleshooting.
**Developers and CI need no R2 credentials or bucket configuration.** Never put
private keys in Wrangler or this repository.

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
celld release, builds locally, and uploads the prepared package over restricted
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
