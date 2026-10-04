#!/usr/bin/env python3
"""Explicit HTTP load probe for already-deployed representative applications.

Never provisions applications, uploads code, or obtains credentials. Requests
can mutate application data; use disposable load-test fleets, not production.
"""
import argparse
from collections import Counter
from concurrent.futures import ThreadPoolExecutor
import json
import math
import re
import time
import urllib.error
import urllib.parse
import urllib.request


def percentile(values, fraction):
    return sorted(values)[max(0, math.ceil(len(values) * fraction) - 1)]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--base-url', required=True, help='Trusted app frontend, e.g. http://127.0.0.1:8000')
    parser.add_argument('--slugs', required=True, help='Comma-separated deployed test app names')
    parser.add_argument('--requests', type=int, default=1000)
    parser.add_argument('--concurrency', type=int, default=4)
    parser.add_argument('--timeout', type=float, default=30)
    parser.add_argument('--dry-run', action='store_true')
    parser.add_argument('--allow-mutations', action='store_true', help='Acknowledge requests may change application data')
    args = parser.parse_args()
    slugs = args.slugs.split(',')
    if len(set(slugs)) != len(slugs) or not all(re.fullmatch(r'[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?', slug) for slug in slugs):
        parser.error('Provide distinct valid application slugs')
    url = urllib.parse.urlsplit(args.base_url)
    if url.scheme not in ('http', 'https') or not url.hostname or url.username or url.password or url.query or url.fragment or url.path not in ('', '/'):
        parser.error('base-url must be an HTTP(S) origin without credentials, path or query')
    if not 1 <= args.requests <= 1000000 or not 1 <= args.concurrency <= 256 or not 0 < args.timeout <= 120:
        parser.error('Invalid request count, concurrency or timeout')
    plan = {'apps': len(slugs), 'requests': args.requests, 'concurrency': args.concurrency, 'slugs': slugs}
    if args.dry_run:
        print(json.dumps(plan, indent=2))
        return
    if not args.allow_mutations:
        parser.error('Use disposable fleets and explicitly pass --allow-mutations')

    def request(index):
        # Each slot exercises a stable named DO in the reference counter fixture.
        # Other representative apps may ignore the query and exercise their own path.
        endpoint = args.base_url.rstrip('/') + '/' + slugs[index % len(slugs)] + '/?name=capacity-' + str(index % args.concurrency)
        start = time.monotonic()
        try:
            with urllib.request.urlopen(endpoint, timeout=args.timeout) as response:
                response.read(1024 * 1024)
                status = str(response.status)
        except urllib.error.HTTPError as error:
            status = str(error.code)
            error.close()
        except (urllib.error.URLError, TimeoutError, OSError):
            status = 'network_error'
        return (time.monotonic() - start) * 1000, status

    started = time.monotonic()
    with ThreadPoolExecutor(max_workers=args.concurrency) as pool:
        results = list(pool.map(request, range(args.requests)))
    elapsed = time.monotonic() - started
    latencies = [latency for latency, _ in results]
    statuses = Counter(status for _, status in results)
    print(json.dumps(dict(plan, elapsed_secs=elapsed, requests_per_sec=len(results)/elapsed,
        latency_ms=dict(p50=percentile(latencies, .5), p95=percentile(latencies, .95), p99=percentile(latencies, .99), maximum=max(latencies)),
        statuses=dict(statuses)), indent=2))
    if any(not status.startswith('2') for status in statuses):
        raise SystemExit(1)

if __name__ == '__main__':
    main()
