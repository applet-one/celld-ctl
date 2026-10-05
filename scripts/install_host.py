#!/usr/bin/env python3
"""Install native celld and (on new hosts) private RustFS. Not a migration tool.

No production path overrides: the small pure inspectors are unit-tested with fixtures.
"""
import argparse
import fcntl
import gzip
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil

import stat
import subprocess
import sys
import tempfile
import zipfile

REPO = Path(__file__).resolve().parent.parent
STATE = Path('/etc/celld-ctl/install-state.json')
CONFIG = Path('/etc/celld-ctl/config.json')
NODE_ENV = Path('/etc/celld/node.env')
RUSTFS_ENV = Path('/etc/rustfs/rustfs.env')
RUSTFS_DATA = Path('/var/lib/rustfs')
RUSTFS_UNIT = Path('/etc/systemd/system/rustfs.service')
CELLD_RELEASES = Path('/usr/local/lib/celld/releases')
RUSTFS_RELEASES = Path('/usr/local/lib/rustfs/releases')
REGISTRY = Path('/var/lib/celld-ctl/registry.sqlite')
# /run/lock is sticky world-writable on common distros; use root-owned /run.
LOCK = Path('/run/celld-ctl-install.lock')
CADDY = Path('/etc/caddy/Caddyfile')
LOCAL_ENDPOINT = 'http://127.0.0.1:9000'
LOCAL_BUCKET = 's3://celld-dev'


class InstallError(Exception):
    pass


def require(condition, message):
    if not condition:
        raise InstallError(message)


def run(*args, **kw):
    return subprocess.run(args, check=kw.pop('check', True), **kw)


def is_present(path):
    return path.exists() or path.is_symlink()


def safe_path(path, *, secret=False, owner=0):
    """Reject links, unsafe parents and unexpected ownership BEFORE touching paths."""
    for p in reversed((path, *path.parents)):
        if not is_present(p):
            continue
        s = p.lstat()
        require(not stat.S_ISLNK(s.st_mode), f'Refusing symlink: {p}')
        require(s.st_uid == (owner if p == path else 0), f'Unsafe owner: {p}')
        if p != path:
            require(stat.S_ISDIR(s.st_mode) and not s.st_mode & 0o022,
                    f'Unsafe parent directory: {p}')
        else:
            require(stat.S_ISREG(s.st_mode) or stat.S_ISDIR(s.st_mode), f'Unsafe destination: {p}')
            if secret and stat.S_ISREG(s.st_mode):
                require(not s.st_mode & 0o077, f'Unsafe permissions: {p}')


def read_json(path):
    try:
        return json.loads(path.read_text())
    except (OSError, ValueError) as e:
        raise InstallError(f'Invalid {path}: {e}') from e


def parse_env(path, keys):
    """Only inspect names and paired values, never print secrets."""
    try:
        lines = path.read_text().splitlines()
    except OSError as e:
        raise InstallError(f'Cannot read {path}: {e.strerror}') from e
    result = {}
    for line in lines:
        line = line.strip()
        if not line or line.startswith('#'):
            continue
        match = re.fullmatch(r'([A-Z_]+)=([A-Za-z0-9]+)', line)
        require(match is not None, f'Invalid credential file: {path}')
        key, val = match.groups()
        require(key not in result, f'Duplicate credential key in {path}')
        result[key] = val
    require(set(result) == set(keys) and all(result.values()), f'Incomplete credential file: {path}')
    return result


def check_pair(node, rustfs):
    require(is_present(node) == is_present(rustfs),
            'Only one local credential file exists; recover from a verified cold snapshot')
    if not is_present(node):
        return False
    a = parse_env(node, ('AWS_ACCESS_KEY_ID', 'AWS_SECRET_ACCESS_KEY'))
    b = parse_env(rustfs, ('RUSTFS_ACCESS_KEY', 'RUSTFS_SECRET_KEY'))
    require(a['AWS_ACCESS_KEY_ID'] == b['RUSTFS_ACCESS_KEY'] and
            a['AWS_SECRET_ACCESS_KEY'] == b['RUSTFS_SECRET_KEY'],
            'Local credential files disagree; restore a verified matching pair')
    return True


def valid_service_keys(path):
    values = parse_env(path, ('RUSTFS_ACCESS_KEY', 'RUSTFS_SECRET_KEY'))
    require(re.fullmatch('[0-9a-fA-F]{48}', values['RUSTFS_ACCESS_KEY']) is not None and
            re.fullmatch('[0-9a-fA-F]{64}', values['RUSTFS_SECRET_KEY']) is not None,
            'Invalid persisted RustFS credential format; manual recovery required')


def classify(config, state, node, rustfs):
    """Pure selection inspector. Never infer local ownership from an endpoint alone."""
    has_config = is_present(config)
    has_state = is_present(state)
    has_node = is_present(node)
    has_rustfs = is_present(rustfs)
    if has_state:
        data = read_json(state)
        require(data.get('schema') == 1 and data.get('mode') == 'local' and
                data.get('phase') in ('preparing', 'ready') and
                data.get('rustfs_version') == '1.0.1' and
                data.get('celld_version') == '0.6.1',
                'Unknown installer metadata; manual recovery required')
        # Rust helper writes the service pair first, then node.env. Only a
        # validated service-only preparing phase may resume that interrupted write.
        require(not has_node or has_rustfs, 'Node credentials without service credentials; manual recovery required')
        require(has_node == has_rustfs or data['phase'] == 'preparing',
                'Partial ready credentials; manual recovery required')
        if has_rustfs:
            valid_service_keys(rustfs)
        if has_node:
            check_pair(node, rustfs)
        require(data['phase'] != 'ready' or (has_config and has_node),
                'Ready metadata contradicts on-disk config or credentials')
        if has_config:
            cfg = read_json(config)
            require(cfg.get('endpoint') == LOCAL_ENDPOINT and
                    cfg.get('bucket') == LOCAL_BUCKET and cfg.get('region') == 'us-east-1' and
                    cfg.get('celld_version') == '0.6.1',
                    'Installer metadata contradicts host configuration; migration is manual')
        return 'local', data
    require(not has_rustfs, 'Unrecognized local credentials; manual recovery required')
    if not has_config and not has_node:
        return 'fresh', None
    require(has_config and has_node, 'Partial host configuration; manual recovery required')
    cfg = read_json(config)
    require(isinstance(cfg, dict) and isinstance(cfg.get('endpoint'), str),
            'Invalid host configuration')
    require(cfg['endpoint'].startswith('https://'),
            'Unrecognized local endpoint without installer metadata; manual recovery required')
    require(re.fullmatch(r'[0-9]+(?:\.[0-9]+)*', str(cfg.get('celld_version', ''))) is not None,
            'Invalid external native celld pin')
    require(isinstance(cfg.get('bucket'), str) and cfg['bucket'].startswith('s3://'),
            'Invalid external bucket')
    return 'external', cfg


def select(mode, existing):
    if existing == 'fresh':
        # Local default is blocked pending the disposable-host compatibility gate.
        return mode or 'external'
    if existing == 'local':
        require(mode in (None, 'local'), 'Refusing implicit local-to-external migration')
        return 'local'
    require(mode in (None, 'external'), 'Refusing implicit external-to-local migration')
    return 'external'


def sha256_file(path):
    h = hashlib.sha256()
    with path.open('rb') as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b''):
            h.update(chunk)
    return h.hexdigest()


def check_version(output, kind, version):
    require(bool(output.splitlines()) and output.splitlines()[0] == f'{kind} {version}',
            f'{kind} release version mismatch')


def unpack(archive, kind, output):
    if kind == 'celld':
        with gzip.open(archive, 'rb') as source, output.open('xb') as dest:
            shutil.copyfileobj(source, dest)
    else:
        with zipfile.ZipFile(archive) as z:
            # Never extract archive paths; accept only the one expected member.
            members = z.namelist()
            require(members.count('rustfs') == 1 and len(members) == len(set(members)),
                    'RustFS archive does not contain one unambiguous binary')
            with z.open('rustfs') as source, output.open('xb') as dest:
                shutil.copyfileobj(source, dest)
    output.chmod(0o700)


def verified_release(entry, kind, dest, temp):
    version = entry['version']
    if is_present(dest):
        safe_path(dest)
        require(dest.is_file() and os.access(dest, os.X_OK), f'Unusable existing release: {dest}')
        result = run(str(dest), '--version', capture_output=True, text=True).stdout
        check_version(result, kind, version)
        return None
    spec = entry[platform.machine()]
    archive = temp / (kind + '.archive')
    run('curl', '--fail', '--location', '--silent', '--show-error', '--proto', '=https',
        '--proto-redir', '=https', '--max-time', '180', spec['url'], '--output', str(archive))
    require(sha256_file(archive) == spec['sha256'], f'{kind} release checksum mismatch')
    candidate = temp / kind
    unpack(archive, kind, candidate)
    version_output = run(str(candidate), '--version', capture_output=True, text=True).stdout
    check_version(version_output, kind, version)
    return candidate


def mkdir(path, mode=0o755, user='root', group='root'):
    safe_path(path, owner=0 if user == 'root' else __import__('pwd').getpwnam(user).pw_uid)
    run('install', '-d', '-o', user, '-g', group, '-m', format(mode, 'o'), str(path))


def copy_new(src, dst, mode=0o755):
    safe_path(dst)
    require(not is_present(dst), f'Refusing to overwrite existing destination: {dst}')
    # Exclusive creation, with no destination symlink race under the installer lock.
    with src.open('rb') as source, dst.open('xb') as target:
        shutil.copyfileobj(source, target)
        target.flush()
        os.fsync(target.fileno())
    dst.chmod(mode)


def copy_template(src, dest, mode):
    safe_path(dest)
    # Reusable components are intentionally updated, never follow a symlink.
    run('install', '-o', 'root', '-g', 'root', '-m', format(mode, 'o'), str(src), str(dest))


def write_state(phase):
    data = {'schema': 1, 'mode': 'local', 'phase': phase,
            'rustfs_version': '1.0.1', 'celld_version': '0.6.1'}
    safe_path(STATE, secret=True)
    fd, tmp = tempfile.mkstemp(prefix='.install-state.', dir=STATE.parent)
    try:
        with os.fdopen(fd, 'w') as f:
            os.fchmod(f.fileno(), 0o600)
            json.dump(data, f, sort_keys=True)
            f.write('\n')
            f.flush()
            os.fsync(f.fileno())
        os.replace(tmp, STATE)
        dirfd = os.open(STATE.parent, os.O_DIRECTORY)
        try:
            os.fsync(dirfd)
        finally:
            os.close(dirfd)
    finally:
        if os.path.exists(tmp):
            os.unlink(tmp)


def occupied(port):
    # Linux kernel listener table, includes IPv4 and IPv6 wildcards.
    for table in ('/proc/net/tcp', '/proc/net/tcp6'):
        with open(table) as f:
            next(f)
            for line in f:
                fields = line.split()
                if fields[3] == '0A' and int(fields[1].split(':')[1], 16) == port:
                    return True
    return False


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--storage', choices=('local', 'external'))
    args = parser.parse_args()
    require(os.geteuid() == 0, 'Run as root')
    os.umask(0o077)
    require(sys.platform == 'linux' and platform.machine() in ('x86_64', 'aarch64'),
            'Supported hosts: Linux x86_64 and aarch64')
    for tool in ('curl', 'systemctl', 'sshd', 'ssh-keygen', 'useradd', 'getent', 'visudo', 'caddy'):
        require(shutil.which(tool) is not None, f'Missing prerequisite: {tool}')
    require((REPO / 'target/release/celld-ctl').is_file(),
            'Build first: cargo build --release --locked -p celld-ctl')
    safe_path(LOCK, secret=True)
    fd = os.open(LOCK, os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, 'w') as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as e:
            raise InstallError('Another host installation is running') from e
        install(args.storage)


def install(requested):
    # No changes to the host until all classified paths have been inspected.
    for p in (STATE, CONFIG, NODE_ENV, RUSTFS_ENV, RUSTFS_DATA, RUSTFS_UNIT,
              CELLD_RELEASES, RUSTFS_RELEASES, CADDY, REGISTRY,
              Path('/etc/celld-ctl/ssh-host-ed25519-key'),
              Path('/etc/celld-ctl/ssh-host-ed25519-key.pub'),
              Path('/etc/ssh/cella-deploy/authorized_keys'),
              Path('/usr/local/bin/celld-ctl'), Path('/usr/local/libexec/celld-run'),
              Path('/usr/local/sbin/celld-deploy-key'),
              Path('/etc/systemd/system/celld-cell@.service'),
              Path('/etc/celld-ctl/sshd_config'),
              Path('/etc/systemd/system/cella-sshd.service'),
              Path('/etc/systemd/journald@celld.conf.d/limits.conf'),
              Path('/etc/sudoers.d/cella-deploy'),
              Path('/var/lib/celld-ctl/public/index.html')):
        safe_path(p, secret=p in (STATE, CONFIG, NODE_ENV, RUSTFS_ENV,
                                   Path('/etc/celld-ctl/ssh-host-ed25519-key')),
                  owner=__import__('pwd').getpwnam('rustfs').pw_uid if p == RUSTFS_DATA
                  and shutil.which('getent') and run('getent', 'passwd', 'rustfs', stdout=subprocess.DEVNULL, check=False).returncode == 0 else 0)
    existing, info = classify(CONFIG, STATE, NODE_ENV, RUSTFS_ENV)
    mode = select(requested, existing)
    if existing == 'fresh':
        require(not is_present(REGISTRY),
                'Existing registry without host storage configuration; manual recovery required')
        for p in (RUSTFS_DATA, RUSTFS_UNIT, RUSTFS_RELEASES / 'v1.0.1/rustfs'):
            require(not is_present(p), f'Unrecognized storage path {p}; manual recovery required')
        require(not is_present(CADDY) or CADDY.read_bytes() ==
                (REPO / 'examples/caddy/Caddyfile.initial').read_bytes(),
                'Existing Caddy configuration; refusing to overwrite on fresh setup')
        require(not occupied(8000) or is_present(CADDY), 'Port 8000 already serves another workload')
    if mode == 'local':
        if existing == 'fresh' or run('systemctl', 'is-active', '--quiet', 'rustfs',
                                      check=False).returncode != 0:
            require(not occupied(9000), 'Port 9000 already has a listener; refusing to take over')
        if existing == 'local':
            require(is_present(RUSTFS_DATA) and is_present(RUSTFS_UNIT) and
                    RUSTFS_UNIT.read_bytes() == (REPO / 'examples/systemd/rustfs.service').read_bytes(),
                    'Managed RustFS paths are missing or modified; manual recovery required')
    else:
        require(not is_present(STATE), 'Storage metadata incompatible with external mode')
    releases = read_json(REPO / 'scripts/host-releases.json')
    version = info['celld_version'] if existing == 'external' else releases['celld']['version']
    celld_path = CELLD_RELEASES / f'v{version}/celld'
    require(version == releases['celld']['version'] or is_present(celld_path),
            f'Existing external host pins celld {version}; install its release manually before rerun')
    rustfs_path = RUSTFS_RELEASES / 'v1.0.1/rustfs'
    if not is_present(celld_path) or (mode == 'local' and not is_present(rustfs_path)):
        require(shutil.disk_usage('/var/tmp').free >= 1024 * 1024 * 1024,
                'At least 1 GiB free in /var/tmp required for release staging')
    with tempfile.TemporaryDirectory(prefix='celld-host-', dir='/var/tmp') as work:
        temp = Path(work)
        celld_entry = dict(releases['celld'], version=version)
        celld_source = verified_release(celld_entry, 'celld', celld_path, temp)
        rustfs_source = verified_release(releases['rustfs'], 'rustfs', rustfs_path, temp) if mode == 'local' else None
        # All remote bytes verified before any install action.
        install_components()
        if celld_source:
            mkdir(celld_path.parent)
            copy_new(celld_source, celld_path)
        if mode == 'local':
            local_install(existing, rustfs_source, rustfs_path)
        elif existing == 'fresh':
            print('Storage is NOT ready: configure an external HTTPS bucket and root-only host credentials. Local RustFS requires explicit --storage local until compatibility qualification.')
        if existing == 'fresh':
            init_caddy()
        run('systemctl', 'daemon-reload')
        run('systemctl', 'enable', '--now', 'cella-sshd')
        run('systemctl', 'reload', 'cella-sshd')
    print('Deployment SSH listens at 127.0.0.1:2222. Arrange private access and enroll public deploy keys.')


def install_components():
    for user, opts in (
        ('celld', ['--system', '--home-dir', '/var/lib/celld', '--shell', '/usr/sbin/nologin']),
        ('celld-publish', ['--system', '--user-group', '--home-dir', '/var/empty/celld-publish', '--shell', '/usr/sbin/nologin']),
        ('cella-deploy', ['--system', '--home-dir', '/var/empty/cella-deploy', '--shell', '/bin/sh']),
    ):
        if run('getent', 'passwd', user, stdout=subprocess.DEVNULL, check=False).returncode:
            run('useradd', *opts, user)
    for path, mode, group in (
        ('/var/empty/cella-deploy', 0o755, 'root'), ('/var/empty/celld-publish', 0o755, 'root'),
        ('/etc/ssh/cella-deploy', 0o755, 'root'), ('/etc/celld-ctl', 0o700, 'root'),
        ('/etc/celld/cells', 0o700, 'root'), ('/var/backups/celld-ctl', 0o700, 'root'),
        ('/var/lib/celld', 0o755, 'root'), ('/var/lib/celld-ctl', 0o755, 'root'),
        ('/var/lib/celld-ctl/public', 0o755, 'root'),
        ('/var/lib/celld-ctl/staging', 0o710, 'celld-publish'),
        (str(CELLD_RELEASES), 0o755, 'root'), ('/usr/local/libexec', 0o755, 'root'),
        ('/etc/systemd/journald@celld.conf.d', 0o755, 'root'),
    ):
        mkdir(Path(path), mode, group=group)
    for src, dest, mode in (
        ('target/release/celld-ctl', '/usr/local/bin/celld-ctl', 0o755),
        ('scripts/celld-run', '/usr/local/libexec/celld-run', 0o755),
        ('scripts/celld-deploy-key', '/usr/local/sbin/celld-deploy-key', 0o755),
        ('examples/systemd/celld-cell@.service', '/etc/systemd/system/celld-cell@.service', 0o644),
        ('examples/ssh/sshd_config', '/etc/celld-ctl/sshd_config', 0o600),
        ('examples/systemd/cella-sshd.service', '/etc/systemd/system/cella-sshd.service', 0o644),
        ('examples/systemd/journal-limits.conf', '/etc/systemd/journald@celld.conf.d/limits.conf', 0o644),
    ):
        copy_template(REPO / src, Path(dest), mode)
    key = Path('/etc/celld-ctl/ssh-host-ed25519-key')
    if not is_present(key):
        run('ssh-keygen', '-q', '-t', 'ed25519', '-N', '', '-C', 'cella transport host', '-f', str(key))
    run('chown', 'root:root', str(key))
    key.chmod(0o600)
    run('visudo', '-cf', str(REPO / 'examples/ssh/cella-deploy.sudoers'))
    copy_template(REPO / 'examples/ssh/cella-deploy.sudoers', Path('/etc/sudoers.d/cella-deploy'), 0o440)
    auth = Path('/etc/ssh/cella-deploy/authorized_keys')
    if not is_present(auth):
        auth.touch(mode=0o644, exist_ok=False)
    run('chown', 'root:root', str(auth))
    auth.chmod(0o644)
    index = Path('/var/lib/celld-ctl/public/index.html')
    if not is_present(index):
        copy_new(REPO / 'examples/caddy/index.html', index, 0o644)
    mkdir(Path('/run/sshd'))
    run('/usr/sbin/sshd', '-t', '-f', '/etc/celld-ctl/sshd_config')


def local_install(existing, source, dest):
    if run('getent', 'passwd', 'rustfs', stdout=subprocess.DEVNULL, check=False).returncode:
        run('useradd', '--system', '--user-group', '--home-dir', str(RUSTFS_DATA),
            '--shell', '/usr/sbin/nologin', 'rustfs')
    mkdir(RUSTFS_DATA, 0o700, 'rustfs', 'rustfs')
    mkdir(RUSTFS_ENV.parent, 0o700)
    mkdir(RUSTFS_RELEASES)
    if source:
        mkdir(dest.parent)
        copy_new(source, dest)
    if existing == 'fresh':
        copy_new(REPO / 'examples/systemd/rustfs.service', RUSTFS_UNIT, 0o644)
        write_state('preparing')
    run('/usr/local/bin/celld-ctl', 'storage', 'prepare-local')
    require(check_pair(NODE_ENV, RUSTFS_ENV), 'Local credential preparation did not complete')
    run('systemctl', 'daemon-reload')
    run('systemctl', 'enable', '--now', 'rustfs')
    # Helper retries bounded connection failures and publishes config only after bucket checks.
    run('/usr/local/bin/celld-ctl', 'storage', 'init-local')
    require(is_present(CONFIG), 'Storage initialization did not publish host config')
    cfg = read_json(CONFIG)
    require(cfg.get('endpoint') == LOCAL_ENDPOINT and cfg.get('bucket') == LOCAL_BUCKET and
            cfg.get('celld_version') == '0.6.1', 'Storage helper published unexpected host config')
    write_state('ready')
    print('Private local RustFS initialized; storage helper completed readiness checks.')


def init_caddy():
    if not is_present(CADDY):
        mkdir(CADDY.parent)
        copy_new(REPO / 'examples/caddy/Caddyfile.initial', CADDY, 0o644)
    run('caddy', 'validate', '--config', str(CADDY))
    run('systemctl', 'enable', '--now', 'caddy')
    run('systemctl', 'reload', 'caddy')


if __name__ == '__main__':
    try:
        main()
    except (InstallError, subprocess.CalledProcessError, OSError, zipfile.BadZipFile, EOFError, KeyError) as e:
        print(f'Installation stopped (not ready): {e}', file=sys.stderr)
        sys.exit(1)
