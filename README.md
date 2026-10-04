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
python3 scripts/test_ssh_relay.py
```

## Getting started: A → B

### A. Set up celld-ctl on the host machine

**On the Linux VM/server, as its administrator:**

1. Build/install `celld-ctl` and the exact pinned native celld release.
2. Configure the host's bucket and private storage credentials.
3. Initialize Caddy and arrange private access to deployment SSH on port `2222`.
4. Enroll developers' **public** deploy keys with `celld-deploy-key`.
5. Give developers the private hostname/port and dedicated server-key fingerprint.

Follow **[A. Host setup](docs/setup-host.md)** for commands and the readiness checklist.
The installer never replaces your administrator/platform SSH service or joins a
private network automatically. Each deploy key is a trusted fleet-wide publisher.

### B. Configure and set up cella on your dev machine

**On your laptop/workstation or CI runner—not on the host VM:**

1. Install/update `cella` (client and host must be `0.2.0` or later).
2. Generate a dedicated key; send only its `.pub` file to the host operator.
3. Configure the private SSH destination, explicit key file and port.
4. Verify the server fingerprint and enroll its key in `known_hosts`.
5. In your existing Wrangler project, run `cella deploy`.

Follow **[B. Dev-machine setup](docs/setup-cella.md)** for key-generation,
public-key enrollment, server verification, configuration and troubleshooting.
**Developers and CI need no R2 credentials or bucket configuration.** Keep private
keys on the dev machine/CI secret store, never in Wrangler or this repository.

Once B is complete, daily usage is:

```sh
cella deploy
cella status
cella logs --lines 50
cella deployments list
```

`deploy` auto-provisions the Worker name, obtains the exact host-pinned release,
builds locally, and sends a bounded prepared package over restricted SSH. The
host publishes with its own storage credentials, then activates/reloads the app.
Initially supported: Linux x86_64/arm64 and macOS arm64 developer machines;
JavaScript/TypeScript with npm/pnpm/Yarn projects.

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
