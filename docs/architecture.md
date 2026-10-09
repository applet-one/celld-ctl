# Architecture and trust boundary

A single-host system for one trusted owner running multiple apps:

- **`cella`** is the CLI on the owner's machine. It builds and sends deployments.
- **`celld-ctl`** manages deployment, app services and routing on the host.
- **`celld`** is the native executable that builds, publishes and runs apps.

## Deployment

```text
Owner's machine                         Host
cella -> celld deploy --dry-run -> SSH -> celld-ctl transport
                                         -> validate package
                                         -> celld publishes to object storage
                                         -> activate systemd service and Caddy route
```

The host pins exact celld releases; the client builds with the host's pin.
Only a prepared package of modules, assets and config is uploaded, not the
repository or build tools. No user build tool executes on the host.

The host validates the package and publishes using a private deployment-only
config and host-held storage credentials. Local builds clear storage environment
variables; SSH does not forward credentials. See [client deployment details](cella.md).

## Serving

```text
Browser -> trusted HTTPS/access proxy -> Caddy :8000 (app directory)
                                      -> Caddy :9101..9999 (one port per app)
                                         -> celld-cell@SLUG on 127.0.0.1:8101..8999
```

Port 8000 only links to apps; it does not proxy app paths. Each app has a
dedicated Caddy port (runtime port + 1000), forwarding all paths unchanged so
the app is served at `/`.

exe.dev supplies authenticated HTTPS forwarding. Other hosts need equivalent
trusted forwarding and an access policy. Do not expose the plain Caddy
listeners as unauthenticated public HTTP. No native celld listener should be
public.

## State

Each app has a separate `cells/SLUG` object-store prefix and a local runtime.
All apps share one object store: local RustFS or external S3-compatible storage.

- `/var/lib/rustfs`: authoritative object-store data when using local RustFS.
- `/var/lib/celld/SLUG`: runtime cache, not the authoritative app data.
- `/var/lib/celld-ctl/registry.sqlite`: app registry and port allocations.
- `/etc/celld/cells`: per-app environment files.

Recovery needs storage, registry, configuration and credentials, not just the
runtime caches. See [storage setup](setup-host.md) for defaults and platform
limits, and [backup and recovery](operations.md#backup-and-recovery) for the
procedure. Reinstallation does not migrate existing storage
targets.

## Trust boundary

This is not hostile multi-tenancy. Separate app processes and storage prefixes
are not security isolation; apps share a runtime Unix account.

`cella` requests the fixed sudo command `celld-ctl transport`. That transport
cannot select arbitrary host paths, package installs, containers or app secrets.
However, the owner's SSH key can also have ordinary shell/sudo authority. Do
not share it with CI or untrusted developers. See [owner SSH setup](cella.md#owner-ssh-setup).

## Deployment failures

Publication and activation are not atomic. A published deployment can be
adopted by a running app even if activation reports failure. Check status and
logs before retrying; a failed deploy does not necessarily mean nothing changed.
