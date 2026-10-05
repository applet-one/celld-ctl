# Architecture

```text
Developer machine                                  Single host
-----------------                                  -----------
cella dev -> local celld dev
cella deploy -> local native dry-run/esbuild
             -> bounded prepared package over SSH -> celld-ctl
                                                    | native dry-run verification
                                                    | native publish (celld-publish user)
                                                    | host-only storage settings/credentials
                                                    +-> object-store app prefix
                                                        local RustFS (fresh x86_64 default;
                                                        loopback :9000)
                                                        or external HTTPS S3 storage
                                                        (explicit --storage external)
                                                    | pointer/readiness verification
                                                    +-> activate/reload + history

HTTPS proxy -> Caddy :8000 -> /app/* -> celld-cell@app.service (127.0.0.1:PORT)
```

VM owners configure their existing exe.dev SSH gateway destination and
registered owner key, not bucket settings or storage credentials. CI and
other developers have no independent deploy identity in this workflow. Local builds remove storage environment variables; SSH
never forwards credentials. The host independently validates a prepared package,
normalizes a private deployment-only config copy and runs exact pinned native
celld without user build tools. User Wrangler files stay unchanged. Native celld
remains the compatibility and durable-publication authority.

Every independently coded app gets a distinct celld fleet prefix such as
`s3://BUCKET/cells/app` and its own local celld process. Caddy preserves the full
path: `/app/foo` reaches the app as `/app/foo`. No celld internal listener is
public. The directory at `/` is read-only, not a management interface.

The local backend shares one RustFS process and bucket across apps;
prefixes, not storage processes, separate fleets. Local RustFS is single-node
development/testing storage, not HA. Its persistent data is authoritative,
whereas `/var/lib/celld/APP_SLUG` is an app cache. External storage remains
an explicit alternative; existing registry targets are not migrated by changing
host defaults. A fresh arm64 host requires an explicit `--storage local` or
`--storage external`; there is no no-flag arm64 default yet. The
[October 5 live gate](rustfs-default-storage-plan.md#7-compatibility-gate-do-this-before-making-rustfs-the-default)
demonstrated named-object recovery after removing **only** the local app cache
in an x86_64 systemd Docker container. It does not qualify VM/arm64 deployment,
cold restore, abrupt failure recovery or production durability.

This is a trusted-deployer single-host MVP, not a hostile multi-tenant boundary.
Owner SSH authority is VM-wide, not app-scoped; serving uses a shared runtime
account. `cella` requests only the fixed sudo transport command, but the
owner credential itself can use ordinary VM SSH and sudo. There is no
deployment-only SSH account/server, relay or tailnet. The dedicated
publisher cannot modify its staged root-owned inputs or access root-only files.
No **transport API** for arbitrary host paths, package installs, container
builds or app secrets is provided; the owner can still open a normal VM shell. Publication followed by activation is
not a distributed atomic transaction; a published pointer can already be adopted
when activation reports failure.
