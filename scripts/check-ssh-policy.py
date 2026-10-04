#!/usr/bin/env python3
"""Check a reachable, enrolled deployment key's real SSH policy (no app mutations)."""
import argparse
import json
import os
import re
import subprocess


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--host', required=True)
    p.add_argument('--identity', required=True)
    p.add_argument('--port', type=int, default=2222)
    p.add_argument('--slug', required=True, help='An existing app used only for read-only status')
    args = p.parse_args()
    if not re.fullmatch(r'[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?', args.slug):
        p.error('Invalid slug')
    if args.host.startswith('-') or not re.fullmatch(r'[a-zA-Z0-9._@:\[\]-]+', args.host) or not 0 < args.port <= 65535:
        p.error('Invalid SSH destination')
    env = {key: os.environ[key] for key in ('PATH', 'HOME', 'LANG') if key in os.environ}
    flags = ['ssh', '-F', '/dev/null', '-T', '-a', '-x', '-o', 'BatchMode=yes', '-o', 'IdentitiesOnly=yes', '-o', 'IdentityAgent=none', '-o', 'StrictHostKeyChecking=yes', '-o', 'ControlMaster=no', '-o', 'ControlPath=none', '-o', 'ConnectTimeout=10', '-i', args.identity, '-p', str(args.port)]

    def require(condition, message):
        if not condition:
            raise RuntimeError(message)

    def run(command, data=b''):
        return subprocess.run(flags + ['--', args.host, command], input=data, capture_output=True, env=env, timeout=30)

    baseline = run('celld-ctl-transport', json.dumps(dict(op='status', slug=args.slug)).encode())
    if baseline.returncode or json.loads(baseline.stdout).get('ok') is not True:
        raise RuntimeError('Baseline status failed: enroll a valid key and verify host fingerprint/connectivity first')
    checks = 1
    for command in ['id', 'sh -c id', 'celld-ctl list', 'celld-ctl-transport; id', 'celld-ctl-transport --root /tmp']:
        result = run(command)
        require(result.returncode != 0 and json.loads(result.stdout).get('ok') is False, 'arbitrary SSH command was not rejected')
        checks += 1
    for data in [dict(op='remove', slug=args.slug), dict(op='target', slug='../shadow'), dict(op='target', slug=args.slug, path='/etc/shadow')]:
        result = run('celld-ctl-transport', json.dumps(data).encode())
        require(result.returncode != 0 and json.loads(result.stdout).get('ok') is False, 'invalid request was not rejected')
        checks += 1
    result = run('celld-ctl-transport', b'x' * (16 * 1024 + 1))
    require(result.returncode != 0 and json.loads(result.stdout).get('ok') is False, 'oversized request was not rejected')
    checks += 1
    forwarded = subprocess.run(flags + ['-W', '127.0.0.1:8000', '--', args.host], input=b'', capture_output=True, env=env, timeout=30)
    require(forwarded.returncode != 0 and b'administratively prohibited' in forwarded.stderr, 'TCP forwarding was not rejected')
    checks += 1
    print(json.dumps(dict(checks_passed=checks, application_mutations=False)))

if __name__ == '__main__':
    main()
