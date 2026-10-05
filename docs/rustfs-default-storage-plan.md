# Plan: default local RustFS storage for development hosts

**Historical storage plan (October 5, 2026):** The transport and SSH
instructions below describe the **old dedicated deployment SSH workflow**, not
the current owner-only exe.dev gateway. Preserve Section 7 as dated evidence
of storage behavior, not a test of current SSH policy. For current deployment
and old-service cleanup see [host setup](setup-host.md) and
[operations](operations.md#existing-host-migration-from-dedicated-deployment-ssh).

**Status, October 5, 2026:** the core live release smoke passed on a disposable
x86_64 Ubuntu 24.04 systemd **Docker container**, including restricted-SSH
deploy and cache-free named Durable Object recovery. The installer now defaults
to local RustFS **only on fresh x86_64 hosts**. Fresh arm64/aarch64 hosts require
an explicit `--storage local` or `--storage external` until native arm64
qualification; existing hosts preserve their configured mode. See the dated
evidence in Section 7. No VM, arm64, abrupt-kill, VM-reboot or cold-restore
qualification is claimed.

## 1. Decision and scope

Make installer-managed RustFS the default storage for **new, single-node
celld-ctl development/testing hosts**. Keep external S3-compatible storage as an
explicit opt-in. Preserve existing host configurations on reinstall.

The primary user-facing goal is that `README.md` → `## Set up the host` needs
only clone/build/install commands and a link for prerequisites and owner SSH
setup. It must not ask the user to create an external bucket, obtain
cloud credentials, or edit storage settings.

This is not a production HA design. Persistent local data should survive normal
service restarts and reinstallations, but losing the VM's storage volume can
lose all application data. An independent backup is optional for disposable
hosts, not a prerequisite for installation.

### In scope

- One native RustFS process managed by systemd on the same Linux VM as celld.
- Use official prebuilt RustFS release binaries: no RustFS source build, Docker
  or container runtime required. Only celld-ctl is built from this checkout.
- Automatic installation, bucket creation, credential generation and host config.
- Linux x86_64 default; arm64 remains an explicit mode pending native
  qualification (availability of an asset is not evidence of compatibility).
- Loopback-only S3 access, with no public storage or console endpoint.
- Real compatibility and persistence tests using pinned RustFS/native celld.
- Idempotent installation and an opt-in external-storage path.
- Short README instructions and detailed operator documentation.

### Out of scope

- Distributed RustFS, erasure-coded multi-node deployments, Kubernetes or HA.
- A new developer-side storage workflow or changes to `wrangler.jsonc`.
- Automatic migration of existing buckets, fleets or legacy imported apps.
- Per-app storage credentials or a hostile multi-tenant security boundary.
- Continuous replication, scheduled backups or online point-in-time snapshots.
- A remotely accessible reset, storage-management or bucket-selection API.

## 2. Intended user experience

### Fresh x86_64 default host

README commands, after documented prerequisites are installed on x86_64:

```sh
git clone https://github.com/applet-one/celld-ctl.git
cd celld-ctl
cargo build --release --locked -p celld-ctl
sudo scripts/install-host.sh
```

The installer should leave the host ready for the owner's existing exe.dev
SSH and the first `cella deploy`, including:

1. Runtime and publisher accounts; no dedicated deployment SSH service.
2. The exact native celld release required by the generated host configuration.
3. RustFS running privately, with a persistent data directory and usable bucket.
4. Root-only host configuration and storage credentials already populated.
5. Initial Caddy configuration on a fresh host, with Caddy running.
6. A bounded local storage compatibility check that has passed.

Owner key registration with exe.dev remains separate. The installer must not
join a tailnet, publish storage, or modify exe.dev owner SSH. Its success
message should identify the owner SSH destination without printing
storage secrets or asking for an external storage account.

### External storage opt-in

Installer interface:

```sh
sudo scripts/install-host.sh --storage external
```

Document the bucket, HTTPS endpoint, region and host credentials under a separate
advanced section of `docs/setup-host.md`. External mode must not install, start
or provision RustFS. A missing external config is an explicit incomplete setup,
not a reason to silently fall back to local storage.

No new `cella` flags or SSH protocol fields are required.

## 3. Implementation constraints and history

At plan inception, the installer left native celld, storage configuration
and Caddy initialization to the operator and host config/pointer verification
required HTTPS. That description is no longer the current installer behavior:
the fresh external path installs pinned native celld and initializes fresh-host
Caddy; explicit local mode installs RustFS with operator-only preparation and
initialization helpers and allows canonical loopback HTTP. Inspect the current
installer and tests before treating any plan item as implemented or qualified.

Relevant enduring constraints:

- `crates/celld-ctl/src/config.rs` and pointer verification must restrict HTTP
  to the canonical loopback local endpoint; external origins retain HTTPS.
- `crates/celld-ctl/src/runtime.rs::verify_pointer` signs a path-style S3 GET,
  including the endpoint port in the signed host.
- `crates/celld-ctl/src/publish.rs` invokes native celld with the configured
  bucket/endpoint/region and a deliberately cleared environment.
- `crates/celld-ctl/src/render.rs` generates application environments and systemd
  overrides; credentials stay in the shared root-only `node.env` file.
- `crates/celld-ctl/src/manager.rs` stores each app's storage destination in the
  registry when provisioning. Changing the host default does not migrate apps.
- `celld-ctl backup` captures registry/configuration (and may preserve old
  SSH artifacts on an un-migrated host), not object-store contents. Local RustFS data is not included automatically.

Native celld 0.6.1 supports HTTP S3-compatible endpoints and path-style requests.
The loopback exception belongs in celld-ctl, not a fork of celld.

## 4. Local storage design

### Proposed fixed defaults

| Setting | Default |
| --- | --- |
| RustFS service account | `rustfs`, system account, no login shell |
| Service | `rustfs.service` |
| S3 listener | `127.0.0.1:9000` |
| Console | Disabled |
| Data directory | `/var/lib/rustfs` |
| Service environment | `/etc/rustfs/rustfs.env` |
| Bucket | `celld-dev` |
| Host endpoint | `http://127.0.0.1:9000` |
| Region | `us-east-1`, verified against the pinned RustFS release |
| Native celld version | `0.6.1` initially |
| RustFS candidate version | `1.0.1`, subject to the compatibility gate |
| Global and bucket durability | `strict` |

Keep the existing app prefix layout: `s3://celld-dev/cells/APP_SLUG`.
Each app still gets an independent celld fleet prefix; there is one shared
RustFS process and one bucket, not one storage process per app.

Download official prebuilt RustFS binaries rather than compiling RustFS. Release
1.0.1 provides versioned ZIP archives for Linux x86_64 and aarch64, in GNU and
musl variants, plus `SHA256SUMS` and `SHA512SUMS`. For example:

```text
https://github.com/rustfs/rustfs/releases/download/1.0.1/rustfs-linux-x86_64-musl-v1.0.1.zip
https://github.com/rustfs/rustfs/releases/download/1.0.1/rustfs-linux-aarch64-musl-v1.0.1.zip
```

Qualify the selected variant on both supported architectures. Include ZIP
extraction tooling in installer prerequisites. Record pinned versions, exact
asset URLs and expected checksums in a reusable release manifest, for example
`scripts/host-releases.json`. Verify downloads before extracting/installing them.
Do not use `latest`, a mutable release symlink, or a remote install script
executed as root. Do not claim provenance verification if only checksum
verification is implemented.

Keep binaries under versioned, root-owned release directories. Select a pinned
RustFS executable in the unit. Record installer ownership/version metadata
outside the checkout, for example `/etc/celld-ctl/install-state.json`; it contains
no private keys or credential values.

### Credentials and filesystem permissions

- Generate unique cryptographically random access/secret values on first setup.
  Use an encoding accepted by `parse_credentials`, such as hexadecimal.
- Never use `rustfsadmin` or any committed/default credential.
- Write RustFS's `RUSTFS_ACCESS_KEY` / `RUSTFS_SECRET_KEY` to its root-only
  systemd EnvironmentFile; write the matching AWS credential variables to
  `/etc/celld/node.env` for celld publication and serving.
- For this trusted-deployer dev MVP, one generated local credential pair is
  acceptable. It is a RustFS administrative credential, not bucket-scoped IAM;
  document that limitation. Separate bucket-scoped credentials can follow later.
- Root owns configuration/credential files with mode `0600`; RustFS owns its
  private data directory with mode `0700`. The service need not read the
  EnvironmentFile directly: systemd reads it before dropping privileges.
- Generate credentials once and reuse them. Do not rotate them on reinstall.
- Do not expose credentials in command arguments, output, test reports or shell
  traces. Downloads, temporary files and generated state stay outside the repo.
- Refuse symlinked configuration/state destinations and unsafe ownership or
  writable parents. Use exclusive staging, atomic writes and private umasks.

### Service integration

Add `examples/systemd/rustfs.service` with:

- A fixed native executable, dedicated user/group and private EnvironmentFile.
- `StateDirectory=rustfs`, explicit loopback binding and disabled console.
- `RUSTFS_DURABILITY_MODE=strict`; also enforce strict for the created bucket.
- Restart on failure, a bounded stop timeout and journald logging.
- Appropriate `NoNewPrivileges`, `ProtectSystem`, `ProtectHome`, private temp
  space and narrowly scoped writable paths, validated against the real binary.

Measure idle and active memory/CPU/disk overhead before setting resource limits;
choose documented development defaults from measurements, not headline benchmarks.
Avoid a low memory cap that repeatedly kills storage during ordinary deployment.

For local apps, generate an app-specific `[Unit]` drop-in with `Wants=` and
`After=rustfs.service`. Do not add a RustFS dependency to the shared app template
or to external-storage apps. Ordering is not proof of readiness: installation
must probe storage, and native celld retains its own startup checks.

## 5. Endpoint policy and S3 bootstrap

### Loopback HTTP policy

Extract one shared storage-origin validator and use it for host configuration,
registry pointer verification and local bootstrap.

Accept:

- Existing HTTPS origins, under the existing no-userinfo/path/query/fragment rules.
- HTTP only for the canonical literal `127.0.0.1` loopback address with an explicit
  valid port. The initial installer uses port `9000`.

Reject:

- HTTP to public addresses, private-network addresses, `0.0.0.0` or DNS names.
- `localhost` as an HTTP exception; avoid DNS-dependent decisions.
- IPv6/alternate IPv4 spellings in the first version unless separately reviewed.
- URL credentials, non-origin paths, query strings and fragments.

Continue to disable proxies and redirects for host S3 requests. This exception
must not weaken HTTPS certificate validation or change transport permissions.

### Operator-only bootstrap helper

Proposed command: `celld-ctl storage init-local`, implemented before the normal
`Manager::open` path so a fresh host does not need an existing registry/config.
It must require root, use fixed production paths, and never be available through
the remote transport request allowlist.

Split responsibilities:

1. Installer handles downloads, account/unit creation and service startup.
2. Bootstrap helper handles safe credential/config preparation and bucket checks.
3. A bounded helper readiness loop retries storage connection failures.
4. A signed HEAD/PUT workflow ensures the fixed bucket exists and is accessible.
5. Native celld's live checks verify the storage contract before setup succeeds.

Credential preparation occurs before RustFS startup; bucket readiness occurs
only afterwards. Use explicit internal phases or subcommands for this ordering,
not one helper that assumes the service can start without credentials.

Avoid a new AWS CLI, Python SDK or RustFS client dependency just to create a
bucket. Reuse the existing HMAC/SHA256/reqwest dependencies by extracting the
small SigV4 request builder from `runtime.rs` into a shared module. Preserve
canonical request and signed-host behavior, including port handling.

Bucket creation must distinguish absence from denied access and transient
errors. A 403 is not a reason to recreate credentials; ambiguous PUT failure
requires readback. Concurrent/repeated setup must not delete or empty a bucket.
Verify region/location-constraint behavior using RustFS 1.0.1 before finalizing
request construction.

## 6. Installer behavior and safety

### Preflight before making changes

Check root, architecture, required tools, built celld-ctl, available disk space,
existing config/credentials/registry and conflicting managed paths. After selecting
local mode, also check port `9000`; external mode does not reserve that port.
Reject an unrelated listener instead of silently selecting another endpoint.

Acquire an installation lock so concurrent invocations cannot generate competing
credentials or partially interleave state. Inspect all existing destinations
before replacing files. Stage and verify downloads before installation.

### Selection rules

| Existing host state | No storage flag | `--storage local` | `--storage external` |
| --- | --- | --- | --- |
| Fresh x86_64 host | Initialize local | Initialize local | External setup only |
| Fresh arm64/aarch64 host | Refuse; require explicit mode | Initialize local (unqualified) | External setup only |
| Recognized installer-managed local host | Reuse local | Reuse local | Refuse implicit migration |
| Configured external host | Preserve external | Refuse implicit migration | Preserve external |
| Partial/inconsistent or unrecognized local host | Stop with recovery instructions | Stop with recovery instructions | Stop with recovery instructions |

Do not infer ownership solely from a loopback endpoint or a bucket name. Use
validated installer metadata and the actual existing configuration together.
Reject contradictions between metadata, credentials and configuration rather
than assuming a flag authorizes overwriting them.
Recover interrupted phases only when the recorded inputs can be validated; do
not guess a new credential when one half of a credential pair is missing.

### Fresh installation sequence

1. Preflight, acquire lock and select mode.
2. Download and verify the exact native celld release; install it root-owned.
   Use the manifest default only on a fresh host. Preserve existing app pins and
   honor existing host configuration rather than silently changing its version.
3. Install existing host components without altering administrator SSH.
4. For local mode, install pinned RustFS, prepare its private directories and
   persist credentials and installer phase metadata.
5. Install/enable/start RustFS; wait for bounded readiness.
6. Ensure the bucket exists, verify credentials and strict durability, and run
   live storage diagnostics against a dedicated disposable probe prefix.
7. Atomically publish the complete host config and mark storage initialization
   complete. Failed checks must not report the host as ready.
8. On a fresh host only, initialize/validate/enable Caddy using existing templates.
   Never overwrite an unrelated Caddy config or port-8000 workload.
9. Print a concise readiness summary; owner SSH uses the existing exe.dev gateway.

Re-running installation must preserve RustFS data, host credentials, app pins,
registry/history and owner SSH access. Do not restart healthy
storage unnecessarily or change binary versions as an incidental side effect.
Explicit upgrades can be documented separately.

For existing hosts, install/update reusable components without rewriting their
storage destinations or Caddy setup. External mode must not run mutating storage
probes automatically against an existing operator bucket.

## 7. Compatibility gate: do this before making RustFS the default

### Minimum live release smoke (passed October 5, 2026)

The isolated test used x86_64 Ubuntu 24.04 with systemd in a **Docker
container**, not a VM, and checksum-verified RustFS 1.0.1 / native celld 0.6.1
releases. Recorded observations:

- Fresh local install, native installer `celld diagnose` and strict storage
  readback passed; a repeated install preserved credentials. The S3 and
  deployment-SSH listeners remained loopback-only. A second separately labeled
  container passed a **fresh no-flag x86_64** install at 15:24:39–44 UTC:
  installer metadata `mode=local`, `phase=ready`, correct pins/config,
  RustFS/Caddy/`cella-sshd` active, loopback S3 and SSH.
- An explicit native `celld diagnose` at 15:22 UTC exited 0 and exercised
  conditional create, reject-create, update and reject-stale conditions.
  This does **not** demonstrate competing-writer races or every listing/
  pagination boundary.
- A real restricted-SSH `cella deploy` published counter app `smoke-do` at
  version `b03cc17fc3cb3274`; activation through Caddy served
  `fixed-smoke-001` counts 1, 2, 3. App restart yielded count 4, RustFS
  restart yielded count 5. With the app stopped, **only**
  `/var/lib/celld/smoke-do` was moved to an archive (not deleted); restarting
  the app yielded count 6, demonstrating recovery without that local app cache.
- After the real deployment, a no-flag reinstall left SHA-256 hashes of
  credentials, config and deployment SSH public key, and the SQLite registry
  unchanged; services stayed active and the named DO responded through the
  proxy at count 7.
- At 15:24 UTC, with RustFS deliberately stopped, `celld diagnose` failed
  quickly (exit 1, bucket unavailable). After RustFS restart, diagnose
  succeeded and the named DO responded at count 8. This is a controlled
  stop/restart observation, **not** an abrupt process kill or VM reboot test.

This minimum smoke supports the **fresh x86_64 single-node development/testing
default** and cache-free recovery in the tested container only. It does not
prove VM or arm64 behavior, cold-snapshot restore, physical power-loss survival
or production HA. Keep credentials and generated instance state outside source
control. A separate explicit-external container passed storage-mode selection
but exposed a fresh SSH reload race. The installer was patched to skip a
fresh reload, restart an existing service and verify SSH is active. A fourth
new fresh no-flag local container passed install **and** no-flag reinstall
after the fix. A fifth new fresh explicit-external container passed
`--storage external` at 15:29:27–31 UTC: no host config, installer local state,
RustFS unit/environment or node credentials were provisioned, and the
dedicated SSH service became active after two seconds, bound to loopback only.
This checks external install selection and SSH service startup, **not** a live
deployment against an external bucket.

### Recommended expanded qualification (not established by the container smoke)

Use a disposable Linux VM and the exact pinned releases, then separately test
arm64 before documenting it as qualified. Retain these broader test targets
and record each result rather than inferring a pass from minimum smoke:

1. Conditional create succeeds for absence and rejects an existing object.
2. Conditional overwrite succeeds for the current ETag and rejects a stale ETag.
3. Competing conditional creates/updates allow exactly one winner.
4. Successful writes are immediately readable, with correct byte-range responses.
5. Immediate listings include new keys; test pagination/prefix boundaries used
   by native celld. Do not enable epoch GC without this evidence.
6. `celld diagnose` succeeds, and the native startup storage probe passes.
7. A real prepared `cella deploy` publishes, pointer verification succeeds, and
   the app activates through the existing proxy.
8. A named Durable Object retains acknowledged state across app and RustFS
   restarts. Repeat with local app cache removed on the disposable VM to prove
   recovery from object storage rather than only from cached SQLite files.
9. RustFS outage/recovery, abrupt process termination and VM reboot produce
   bounded failures and recovery rather than silent empty-state replacement.
10. Existing HTTPS external-storage behavior and the current owner SSH
    transport still pass.

A process kill/reboot test does not certify physical power-loss or volume-loss
survival. Container restart is not VM reboot. Even expanded development
qualification is not a new production durability claim.

If minimum release smoke fails, fix/upgrade the tested component or keep the
feature behind an explicit opt-in. Do not bypass conditions, disable native
checks or silently weaken durability to make the default work. An x86_64
container pass is not evidence for any unrun expanded item.

## 8. Test plan

### Fast hermetic tests

- Endpoint policy: HTTPS retained; canonical loopback HTTP accepted; all forbidden
  hosts/schemes/userinfo/path/query/fragment variants rejected at both consumers.
- SigV4 fixtures: GET/HEAD/PUT, body hashes and hosts with explicit ports; existing
  pointer verification signatures remain unchanged for external HTTPS origins.
- Bootstrap: already-existing bucket, first creation, concurrent setup, denied
  credentials, unreachable service, ambiguous writes and bounded retries.
- Installer with fake commands/downloads/systemctl: fresh local, repeat local,
  existing external, explicit external, invalid flag, conflicting port, failed
  download/version check and interrupted setup recovery.
- Permissions, unsafe paths, symlink rejection, credential reuse and redaction.
- Local app unit dependencies without modifying external/legacy app units.
- Storage metadata remains absent from SSH targets and requests.

Use dependency injection/fixture roots for operator tests only; do not add
production path overrides to remote deployment requests.

### Opt-in Linux integration suite

Add a clearly destructive/disposable host test harness, for example
`scripts/test_local_storage.py`, that requires explicit opt-in and validates its
fixture host before provisioning services. Never aim it at a developer's current
fleet by default.

Cover fresh install, real deployment, persisted named-object state, restart,
cache-free restore, repeated install, stored-credential identity, service exposure,
permission checks, storage downtime and a cold snapshot restore.

Run hermetic tests in ordinary CI. Run real Linux/systemd/RustFS tests in a
separate opt-in or dedicated disposable runner lane. Start with one architecture
for investigation, but validate released binaries and deployment on both Linux
x86_64 and arm64 before documenting support.

Measure idle and loaded storage resource use, deployment duration and durable
write latency. Record measurements without making untested performance claims.

## 9. Operations, persistence and migration

### Persistence and reset

Document which paths contain authoritative data versus caches. Disabling or
removing an app must continue to retain its object-store data. Reinstalling the
host must not reset anything.

For disposable-host reset, initially provide an explicit stop/archive/reset
procedure rather than a new remotely accessible command. Coordinate registry,
RustFS data and app caches; do not reset one of them and leave the others pointing
at incompatible state. Never hide recursive deletion inside installation.

### Backup and restore

Keep `celld-ctl backup` scoped to configuration/registry unless a separate storage
snapshot implementation is deliberately added. Update its documentation/output
as needed to state that local application data is **not included**.

Document an optional cold snapshot: block deployments, stop affected app services,
stop RustFS, capture RustFS data/config plus registry/configuration/SSH identity
and version metadata, then restart in order. Preserve ownership and permissions.
Store a copy outside the VM if surviving VM loss matters. Do not describe a live
recursive copy of active RustFS files as a consistent backup.

The acceptance suite should restore a cold snapshot onto a fresh disposable VM
and verify named-object state. Scheduled/online backup automation remains deferred.

### Existing-host migration

No automatic external-to-local or local-to-external migration in this change.
An intentional migration must quiesce writers, copy the full relevant object
namespace, update each persisted registry target and generated app inputs, and
verify recovery before returning to service. Native lease/session records need
an explicit reviewed recovery procedure; a generic live bucket sync is not enough.

Keep legacy bucket-root imports protected. Switching the host's default bucket
or endpoint only affects newly provisioned apps, not existing app storage.

## 10. Documentation changes

The README fresh-x86_64 claim follows the passed **minimum container gate** in
Section 7 and the architecture-limited selector. Expanded qualification
remains separate from this documentation change.

- `README.md`: keep the four host commands; explain that the default installer
  provisions local RustFS for development/testing. Link prerequisites, private
  connectivity and advanced external storage without listing bucket setup in the
  main host flow. Preserve the current key-generation/SCP/enrollment section.
- `docs/setup-host.md`: make automatic local storage the main path; explain
  generated settings, persistence, readiness and existing-host preservation.
  Move external configuration into an explicit optional section. Keep stable
  anchors where practical and update links when sections move.
- `docs/operations.md`: add RustFS service troubleshooting, local-data limitations,
  cold snapshot/restore, reset safeguards and explicit upgrade guidance.
- `docs/architecture.md`: show a local RustFS service by default, with external
  storage as an alternative. Replace R2-only phrasing with object storage where
  the behavior is actually provider-neutral.
- `docs/setup-cella.md` and `docs/cella.md`: preserve the no-storage-credentials
  developer contract; distinguish application R2 bindings from the storage vendor.
- `examples/config/`: add a local default example and retain a clearly named
  external example. Examples never contain credentials.
- `docs/implementation-plan.md`: update delivery status only after implementation.

## 11. Delivery sequence and acceptance criteria

### Phase A — Compatibility spike

The pinned-release minimum gate was exercised in a disposable x86_64
systemd Docker container and recorded in Section 7; it was **not** a VM test.
Expanded VM/arm64 coverage and resource measurements remain outstanding.

### Phase B — Endpoint and bootstrap foundation

Implemented shared origin validation/SigV4 support and operator-only local
bootstrap with hermetic tests; retain external HTTPS behavior and the existing
transport request allowlist in future changes.

### Phase C — Installer-managed local storage

Implemented release manifest, native download/install support, RustFS unit,
local config, selection/recovery rules, readiness checks and safe fresh Caddy
initialization. The no-flag default applies to **fresh x86_64 hosts only**.

### Phase D — End-to-end qualification

The minimum x86_64 container smoke includes cache-free named-DO recovery,
no-flag reinstall preservation and controlled RustFS stop/restart. Continue
expanded VM/arm64, abrupt-kill/reboot and cold snapshot/restore qualification;
do not infer these from container smoke.

### Phase E — Documentation and release

Document the fresh-x86_64-only local default and explicit external opt-out,
with the tested container scope and untested paths stated. Do not present
expanded checks as passed.

Release and longer-term acceptance checks (not all are qualified by Section 7):

- The four README host commands provision working local storage without cloud
  accounts, manual bucket creation or credential edits.
- The first real `cella deploy` succeeds through owner SSH after key and host-key setup.
- Storage listeners remain loopback-only; console is disabled; secrets stay local.
- Restart/reinstall preserves acknowledged named-object state without altering exe.dev SSH.
- Existing external hosts remain unchanged unless deliberately migrated.
- External-storage opt-in is documented and regression-tested.
- No compatibility or power-loss guarantee is claimed beyond measured evidence.

## 12. Research sources

Sources reviewed during planning; website documentation can change. Qualification
must use pinned release artifacts and the relevant versioned source.

- [RustFS 1.0.1 release](https://github.com/rustfs/rustfs/releases/tag/1.0.1)
- [RustFS single-node/single-disk guide](https://docs.rustfs.com/en/installation/linux/single-node-single-disk)
- [RustFS 1.0.1 S3 compatibility matrix](https://github.com/rustfs/rustfs/blob/1.0.1/docs/architecture/s3-compatibility-matrix.md)
- [RustFS 1.0.1 conditional-write tests](https://github.com/rustfs/rustfs/blob/1.0.1/crates/e2e_test/src/reliant/conditional_writes.rs)
- [RustFS 1.0.1 durability modes](https://github.com/rustfs/rustfs/blob/1.0.1/docs/operations/durability-modes.md)
- [celld 0.6.1 storage guarantees and diagnostic probes](https://github.com/denoland/celld/blob/v0.6.1/docs/guarantees.md)
- [celld 0.6.1 S3 client and HTTP/path-style support](https://github.com/denoland/celld/blob/v0.6.1/crates/celld/bucket.rs)
