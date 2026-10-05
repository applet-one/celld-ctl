#!/usr/bin/env python3
"""Opt-in REAL prepared deploy and named Durable Object persistence smoke.

Run only as root on a disposable, already-installed local-storage systemd host:
  install -o root -g root -m 600 /dev/stdin \
    /etc/celld-ctl/disposable-storage-test <<<"$(hostname)"
  python3 scripts/test_local_deploy.py --ack-disposable-host "$(hostname)" \
    --client-user YOUR_NONROOT_USER --identity /path/to/enrolled/deploy-key \
    --cella-binary /absolute/path/to/cella

Before running, install cella >=0.2.0, enroll the client's PUBLIC key using
celld-deploy-key, and verify/add the dedicated 127.0.0.1:2222 SSH server key
to that user's ~/.ssh/known_hosts. The client runs without AWS credentials.
The fixture, app, Durable Object data, and archived cache are intentionally
retained; this test never deletes an app or RustFS data. Do not rerun on a host
where smoke-do has already been provisioned. This does NOT test cold backups,
VM reboot, physical power loss, or production durability.
"""

import argparse
import fcntl
import json
import os
from pathlib import Path
import pwd
import secrets
import socket
import sqlite3
import stat
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request

from install_host import (CONFIG, NODE_ENV, REPO, RUSTFS_DATA, RUSTFS_ENV, RUSTFS_UNIT,
                          STATE, InstallError, check_pair, safe_path)
from test_local_storage import MARKER, REGISTRY, check_loopback_only

SLUG = 'smoke-do'
UNIT = f'celld-cell@{SLUG}.service'
CACHE = Path('/var/lib/celld') / SLUG
CELL_ENV = Path('/etc/celld/cells') / f'{SLUG}.env'
LOCK = Path('/run/celld-ctl-local-deploy-smoke.lock')
HOST_KEY = Path('/etc/celld-ctl/ssh-host-ed25519-key.pub')
FIXTURE_JS = '''export class Counter {
  constructor(state, env) { this.state = state; }
  async fetch(request) {
    const n = ((await this.state.storage.get("n")) ?? 0) + 1;
    await this.state.storage.put("n", n);
    return Response.json({ n });
  }
}
export default {
  async fetch(request, env) {
    const name = new URL(request.url).searchParams.get("name") ?? "default";
    return env.COUNTER.get(env.COUNTER.idFromName(name)).fetch(request);
  }
};
'''


def require(condition, reason):
    if not condition:
        raise InstallError(reason)


def existing(path):
    """An existing symlink or broken symlink is never an empty slot."""
    return path.exists() or path.is_symlink()


def private_file(path):
    safe_path(path, secret=True)
    require(path.is_file() and not path.is_symlink(), f'Missing private regular file: {path}')
    s = path.lstat()
    require(s.st_uid == 0 and stat.S_IMODE(s.st_mode) == 0o600,
            f'Expected root-owned 0600 private file: {path}')


def root_owned_unit(path):
    s = path.lstat()
    return stat.S_ISREG(s.st_mode) and s.st_uid == 0 and not s.st_mode & 0o022


def marker_gate(ack, hostname, marker=MARKER, *, euid=None):
    if euid is None:
        euid = os.geteuid()
    require(euid == 0, 'Run as root on an isolated disposable host')
    require(bool(ack) and ack == hostname and hostname == socket.gethostname(),
            'Explicit --ack-disposable-host matching this VM hostname required')
    private_file(marker)
    require(marker.read_text().strip() == hostname,
            'Dedicated disposable-host marker must contain this hostname')


def local_state_gate(state, config, node, rustfs, data, rustfs_unit):
    for p in (state, config, node, rustfs):
        private_file(p)
    metadata = json.loads(state.read_text())
    require(metadata == {'schema': 1, 'mode': 'local', 'phase': 'ready',
                         'rustfs_version': '1.0.1', 'celld_version': '0.6.1'},
            'Installer metadata is not the ready, pinned local installation')
    cfg = json.loads(config.read_text())
    require(cfg.get('endpoint') == 'http://127.0.0.1:9000'
            and cfg.get('bucket') == 's3://celld-dev'
            and cfg.get('region') == 'us-east-1'
            and cfg.get('celld_version') == metadata['celld_version'],
            'Host storage target does not match the ready local installation')
    require(check_pair(node, rustfs), 'RustFS and node credentials are not paired')
    safe_path(data, owner=pwd.getpwnam('rustfs').pw_uid)
    s = data.lstat()
    require(stat.S_ISDIR(s.st_mode) and s.st_uid == pwd.getpwnam('rustfs').pw_uid
            and stat.S_IMODE(s.st_mode) == 0o700,
            'RustFS data directory must belong to rustfs and have mode 0700')
    safe_path(rustfs_unit)
    require(root_owned_unit(rustfs_unit),
            'RustFS systemd unit must be an unmodified root-owned unit')
    require(rustfs_unit.read_bytes() == (REPO / 'examples/systemd/rustfs.service').read_bytes(),
            'RustFS service differs from the pinned local installer unit')


def registry_gate(registry=REGISTRY, slug=SLUG):
    safe_path(registry, secret=True)
    # An absent registry is valid immediately after installation. Opening SQLite
    # with mode=ro must not create or migrate one as a side effect of the gate.
    if not existing(registry):
        return
    private_file(registry)
    with sqlite3.connect(f'file:{registry}?mode=ro', uri=True, timeout=5) as conn:
        rows = conn.execute('SELECT slug FROM apps').fetchall()
    require(not any(row[0] == slug for row in rows),
            f'Target {slug} already exists in registry; refusing to reuse it')
    require(not rows, 'Existing apps would be interrupted by the RustFS restart')


def namespace_gate(cache=CACHE, cell_env=CELL_ENV, unit=UNIT, unit_bases=None):
    for path in (cache, cell_env):
        require(not existing(path), f'Target already has local state: {path}')
    if unit_bases is None:
        unit_bases = ('/etc/systemd/system', '/run/systemd/system',
                      '/usr/lib/systemd/system', '/lib/systemd/system')
    for base in unit_bases:
        require(not existing(Path(base) / unit), f'Target already has a systemd unit in {base}')
    for action in ('is-active', 'is-enabled'):
        result = subprocess.run(['systemctl', action, '--quiet', unit], check=False, timeout=10)
        require(result.returncode != 0, f'Target {unit} is already {action[3:]}')


def active(unit):
    return subprocess.run(['systemctl', 'is-active', '--quiet', unit], check=False,
                          timeout=10).returncode == 0


def prereqs(user, identity, binary):
    require(user.pw_uid != 0, 'Client must be a non-root user')
    require(identity.is_absolute() and binary.is_absolute(),
            'Client identity and cella binary must be absolute paths')
    s = identity.lstat()
    require(stat.S_ISREG(s.st_mode) and s.st_uid == user.pw_uid
            and stat.S_IMODE(s.st_mode) & 0o077 == 0,
            'Client key must be a private regular file owned by client user')
    require(binary.is_file() and os.access(binary, os.X_OK), 'Missing executable cella binary')
    known = Path(user.pw_dir) / '.ssh/known_hosts'
    require(known.is_file() and not known.is_symlink(),
            f'Client must verify the server fingerprint and enroll {known}')
    safe_path(HOST_KEY)
    require(root_owned_unit(HOST_KEY), 'Dedicated server public key is not root-owned')
    private = HOST_KEY.read_text().split()
    require(len(private) >= 2 and private[0] == 'ssh-ed25519',
            'Missing dedicated server host public key')
    found = subprocess.run(['ssh-keygen', '-F', '[127.0.0.1]:2222', '-f', str(known)],
                           capture_output=True, text=True, timeout=10)
    require(found.returncode == 0
            and any(line.split()[1:3] == private[:2] for line in found.stdout.splitlines()
                    if not line.startswith('#')),
            'Known-hosts entry does not match the dedicated deployment server key')
    for unit in ('rustfs.service', 'caddy.service', 'cella-sshd.service'):
        require(active(unit), f'Required service not active: {unit}')
    check_loopback_only()


def client(user, identity, binary, project, action, timeout):
    # Explicitly empty environment: no host-owned S3/AWS credential can reach
    # the CLI, its native dry-run build, or the restricted SSH transport.
    cmd = ['runuser', '-u', user.pw_name, '--', 'env', '-i',
           f'HOME={user.pw_dir}', 'PATH=/usr/local/bin:/usr/bin:/bin',
           'CELLA_HOST=cella-deploy@127.0.0.1', 'CELLA_SSH_PORT=2222',
           f'CELLA_SSH_KEY={identity}', str(binary), '--project', str(project), *action]
    result = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout, check=False)
    require(result.returncode == 0,
            f'cella {action[0]} failed (exit {result.returncode}): '
            f'{result.stderr[-2000:]} {result.stdout[-1000:]}')
    return result.stdout


def counter(name, expected, timeout=45):
    url = f'http://127.0.0.1:8000/{SLUG}/?name={name}'
    deadline = time.monotonic() + timeout
    direct = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    while True:
        try:
            with direct.open(url, timeout=5) as response:
                require(response.status == 200, f'Worker returned HTTP {response.status}')
                obj = json.loads(response.read(4096))
            require(type(obj.get('n')) is int and obj['n'] == expected,
                    f'Named DO state mismatch: expected {expected}, got {obj!r}')
            return
        except urllib.error.HTTPError as e:
            # A Worker may have already committed before an application/proxy
            # error; never issue another mutating GET after an HTTP response.
            raise InstallError(f'Worker returned HTTP {e.code}, expected {expected}') from e
        except (urllib.error.URLError, TimeoutError, ConnectionError):
            # Only transport failures may be retried. The fixture increments on
            # every GET, so retrying after an ambiguous HTTP response is unsafe.
            if time.monotonic() >= deadline:
                raise InstallError(f'Worker did not return named DO value {expected}')
            time.sleep(1)


def service(*args):
    subprocess.run(['systemctl', *args], check=True, timeout=120)


def make_fixture(user):
    path = Path(tempfile.mkdtemp(prefix='celld-smoke-fixture-', dir='/var/tmp'))
    os.chown(path, user.pw_uid, user.pw_gid)
    cfg = {'name': SLUG, 'main': 'index.js', 'no_bundle': True,
           'compatibility_date': '2026-01-01',
           'durable_objects': {'bindings': [{'name': 'COUNTER', 'class_name': 'Counter'}]},
           'migrations': [{'tag': 'v1', 'new_sqlite_classes': ['Counter']}]}
    for name, body in (('wrangler.jsonc', json.dumps(cfg) + '\n'), ('index.js', FIXTURE_JS)):
        p = path / name
        p.write_text(body)
        os.chown(p, user.pw_uid, user.pw_gid)
    return path


def verified_status(user, identity, binary, project):
    status = json.loads(client(user, identity, binary, project, ['status'], 30))
    require(status.get('target', {}).get('slug') == SLUG
            and status['target'].get('enabled') is True
            and status.get('active') is True
            and isinstance(status.get('version_id'), str)
            and status['version_id']
            and status.get('observed_version_id') == status['version_id'],
            f'App status does not report the active published deployment: {status!r}')
    return status['version_id']


def run(args):
    marker_gate(args.ack_disposable_host, socket.gethostname())
    local_state_gate(STATE, CONFIG, NODE_ENV, RUSTFS_ENV, RUSTFS_DATA, RUSTFS_UNIT)
    registry_gate()
    namespace_gate()
    user = pwd.getpwnam(args.client_user)
    prereqs(user, args.identity, args.cella_binary)
    safe_path(LOCK, secret=True)
    fd = os.open(LOCK, os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600)
    with os.fdopen(fd, 'r+') as lock:
        require(os.fstat(lock.fileno()).st_uid == 0
                and stat.S_IMODE(os.fstat(lock.fileno()).st_mode) == 0o600,
                'Smoke lock is not root-owned 0600')
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as e:
            raise InstallError('Another disposable deploy smoke is running') from e
        # Avoid provisioning the same slug if another session changed the registry.
        registry_gate()
        namespace_gate()
        project = make_fixture(user)
        print(f'Fixture retained at {project}', flush=True)
        client(user, args.identity, args.cella_binary, project,
               ['deploy', '--source-revision', 'smoke-do-test'], 660)
        published = verified_status(user, args.identity, args.cella_binary, project)
        name = f'persistence-{secrets.token_hex(12)}'
        counter(name, 1)
        counter(name, 2)
        counter(name, 3)
        service('restart', UNIT)
        counter(name, 4)
        service('restart', 'rustfs.service')
        counter(name, 5)
        # Cache move is allowed ONLY after all five acknowledged writes passed;
        # never copy/delete the authoritative /var/lib/rustfs directory.
        service('stop', UNIT)
        require(not active(UNIT), f'{UNIT} did not stop; refusing to move live cache')
        require(CACHE.is_dir() and not CACHE.is_symlink(),
                f'Expected new app cache to archive: {CACHE}')
        archive_root = Path(tempfile.mkdtemp(prefix='.smoke-do-cache-archive-',
                                             dir='/var/lib/celld'))
        archived = archive_root / SLUG
        os.rename(CACHE, archived)  # atomic; fails rather than copying/deleting across devices
        print(f'Archived app cache at {archived} (do not delete)', flush=True)
        service('restart', 'rustfs.service')
        service('start', UNIT)
        counter(name, 6)
        require(verified_status(user, args.identity, args.cella_binary, project) == published,
                'App adopted a different deployment during the recovery test')
        print(f'PASS: prepared restricted-SSH deploy and named DO {name}: '
              '1,2,3,4,5,6 across app restart, RustFS restart and cache-free restore.')
        print(f'Retained app {SLUG}, fixture {project} and archive {archived}; '
              'no cleanup or rollback performed.')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--ack-disposable-host', required=True, metavar='HOSTNAME')
    parser.add_argument('--client-user', required=True, metavar='NONROOT_USER')
    parser.add_argument('--identity', type=Path, required=True, metavar='PRIVATE_KEY')
    parser.add_argument('--cella-binary', type=Path, required=True, metavar='ABSOLUTE_PATH')
    args = parser.parse_args()
    os.umask(0o077)
    try:
        run(args)
    except (InstallError, OSError, ValueError, KeyError, sqlite3.Error,
            subprocess.SubprocessError, urllib.error.URLError) as e:
        print(f'Real local deploy smoke stopped (NOT qualified): {e}', file=sys.stderr)
        sys.exit(1)


if __name__ == '__main__':
    main()
