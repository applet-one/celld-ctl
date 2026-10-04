# Architecture

```text
Developer machine                                  Single host
-----------------                                  -----------
cella dev -> local celld dev
cella deploy -> local native dry-run/esbuild
             -> bounded prepared package over SSH -> celld-ctl
                                                    | native dry-run verification
                                                    | native publish (celld-publish user)
                                                    | host-only R2 settings/credentials
                                                    +-> object-store app prefix
                                                    | pointer/readiness verification
                                                    +-> activate/reload + history

HTTPS proxy -> Caddy :8000 -> /app/* -> celld-cell@app.service (127.0.0.1:PORT)
```

Developers and CI configure only restricted SSH access, not bucket settings or
storage credentials. Local builds remove storage environment variables; SSH
never forwards credentials. The host independently validates a prepared package,
normalizes a private deployment-only config copy and runs exact pinned native
celld without user build tools. User Wrangler files stay unchanged. Native celld
remains the compatibility and durable-publication authority.

Every independently coded app gets a distinct celld fleet prefix such as
`s3://BUCKET/cells/app` and its own local celld process. Caddy preserves the full
path: `/app/foo` reaches the app as `/app/foo`. No celld internal listener is
public. The directory at `/` is read-only, not a management interface.

This is a trusted-deployer single-host MVP, not a hostile multi-tenant boundary.
Deploy keys are fleet-wide; serving uses a shared runtime account. The dedicated
publisher cannot modify its staged root-owned inputs or access root-only files.
No remote shell, arbitrary host path, package install, container build or app-
secret management endpoint is provided. Publication followed by activation is
not a distributed atomic transaction; a published pointer can already be adopted
when activation reports failure.
