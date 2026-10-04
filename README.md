# celld-ctl

`celld-ctl` is a single-host control layer for [celld](https://celld.dev).

It provisions multiple independent celld applications on one machine. Each app gets its own object-store prefix, loopback celld service, systemd lifecycle, and route such as `/my-app/`. A companion developer CLI, `cella`, provides a Wrangler-like development and deployment workflow.

> Status: design and implementation repository. It contains no live deployment configuration, credentials, hostnames, or application data.

## Components

- **`celld-ctl`** — host/operator CLI for prefixes, ports, systemd, Caddy, status, logs, and reloads.
- **`cella`** — developer CLI for projects that retain their `wrangler.jsonc`.

## Start here

1. [Architecture](docs/architecture.md)
2. [Implementation plan](docs/implementation-plan.md)
3. Generic [systemd](examples/systemd/) and [Caddy](examples/caddy/) templates

## Repository boundary

This checkout is intentionally separate from installed instances. Keep instance state, object-store credentials, SSH keys, generated Caddy configuration, SQLite registries, deployed apps, and systemd environment files outside it.
