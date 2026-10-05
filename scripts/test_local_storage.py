#!/usr/bin/env python3
"""Opt-in disposable-Linux-host smoke. NOT the full native deploy/persistence gate.

Before use: dedicate a throwaway VM with no real fleet; as root create
/etc/celld-ctl/disposable-storage-test (root-owned mode 0600) containing the
VM's hostname on one line. Explicit hostname confirmation is also required.
No reset, deletion, reboot or service stop is performed.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import socket
import stat
import subprocess
import sys

from install_host import InstallError, REPO, CONFIG, NODE_ENV, RUSTFS_ENV, RUSTFS_DATA, STATE, safe_path, check_pair

MARKER = Path('/etc/celld-ctl/disposable-storage-test')
REGISTRY = Path('/var/lib/celld-ctl/registry.sqlite')


def validate_target(ack, hostname, marker, registry):
    if os.geteuid() != 0:
        raise InstallError('Run as root on an isolated disposable host')
    if not ack or not hostname or ack != hostname or hostname != socket.gethostname():
        raise InstallError('Explicit --ack-disposable-host matching this VM hostname required')
    safe_path(marker, secret=True)
    if not marker.exists() or not marker.is_file() or marker.read_text().strip() != hostname:
        raise InstallError('Dedicated 0600 disposable-host marker must contain this hostname')
    if registry.exists() or registry.is_symlink():
        raise InstallError('Existing registry detected; refusing to run on a fleet host')


def fingerprint():
    return hashlib.sha256(NODE_ENV.read_bytes() + b'\0' + RUSTFS_ENV.read_bytes()).digest()


def check_loopback_only():
    # /proc sockets use little-endian hexadecimal addresses on Linux.
    found = False
    for table in ('/proc/net/tcp', '/proc/net/tcp6'):
        with open(table) as f:
            next(f)
            for line in f:
                fields = line.split()
                address, port = fields[1].split(':')
                if fields[3] == '0A' and int(port, 16) == 9000:
                    if table != '/proc/net/tcp' or address != '0100007F':
                        raise InstallError('Unexpected non-loopback storage listener')
                    found = True
    if not found:
        raise InstallError('No local storage listener')


def smoke():
    subprocess.run([str(REPO / 'scripts/install-host.sh'), '--storage', 'local'], check=True)
    for p in (CONFIG, NODE_ENV, RUSTFS_ENV, STATE):
        safe_path(p, secret=True)
        if not p.is_file():
            raise InstallError(f'Missing {p}')
    config = json.loads(CONFIG.read_text())
    if config.get('endpoint') != 'http://127.0.0.1:9000' or config.get('bucket') != 's3://celld-dev':
        raise InstallError('Unexpected storage config')
    if not check_pair(NODE_ENV, RUSTFS_ENV):
        raise InstallError('Missing local credentials')
    if stat.S_IMODE(RUSTFS_DATA.stat().st_mode) != 0o700:
        raise InstallError('RustFS data permissions are not private')
    check_loopback_only()
    original = fingerprint()
    subprocess.run([str(REPO / 'scripts/install-host.sh'), '--storage', 'local'], check=True)
    if fingerprint() != original:
        raise InstallError('Reinstall changed credential identity')
    check_loopback_only()
    print('Disposable-host smoke passed: storage install, loopback bind, credential reuse.')
    print('NOT TESTED: native celld diagnose/deploy, conditional writes, durability, restart/cache restore, cold snapshot, arm64. Do not enable local default on this evidence.')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--ack-disposable-host', metavar='HOSTNAME', required=True)
    args = parser.parse_args()
    validate_target(args.ack_disposable_host, socket.gethostname(), MARKER, REGISTRY)
    smoke()


if __name__ == '__main__':
    try:
        main()
    except (InstallError, OSError, subprocess.CalledProcessError, ValueError) as e:
        print(f'Smoke stopped (not qualified): {e}', file=sys.stderr)
        sys.exit(1)
