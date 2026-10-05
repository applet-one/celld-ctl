# Architecture and trust boundary

```text
Owner's machine                      Single host
cella deploy -> native dry-run       -> owner SSH -> celld-ctl transport
                                     -> host native verification and publication
                                     -> object store (local RustFS or external S3)
                                     -> per-app systemd service and Caddy route
exe.dev authenticated HTTPS proxy -> Caddy :8000 (slug directory)
                                  -> Caddy :9101..9999 (one port per app at /)
                                     -> celld-cell@SLUG (127.0.0.1:8101..8999)
```

The host pins exact native celld releases, validates prepared packages, uses a
private deployment-only config and publishes with host-only storage settings
and credentials. Local client builds clear storage environment variables;
SSH does not forward credentials. Existing Wrangler files are unchanged.
Each independently coded app has a separate `cells/SLUG` object-store prefix,
a local celld runtime and a dedicated Caddy port. The directory on 8000 only
links slugs; it does not proxy app paths. Each app gets requests unchanged at
`/` on its own port (runtime port + 1000). No internal listener is public.

On fresh x86_64 installs the single RustFS process and bucket serve all app
prefixes; `--storage external` is the explicit alternative. Local data in
`/var/lib/rustfs` is authoritative, while `/var/lib/celld/APP_SLUG` is a cache.
Existing registry targets do not migrate when defaults change. Fresh arm64
needs an explicit storage choice. See [the historical container test and its
limits](operations.md#historical-storage-gate-october-5-2026) and
[backup procedure](operations.md#backup-and-recovery).

This is a trusted-owner, single-host system, not hostile multi-tenancy.
The shared runtime account is not per-app Unix isolation. `cella` requests a
fixed sudo transport command, but the owner's SSH key can have ordinary VM
shell/sudo authority. It must not be shared with CI or untrusted developers.
The deployment transport cannot select arbitrary host paths, package installs,
containers or app secrets; owner SSH itself is not similarly restricted.
Publication and activation are not a distributed atomic transaction: a
published pointer can be adopted even if activation reports failure.
