#!/usr/bin/env python3
"""Explicit HTTP load probe for already-deployed representative applications.

Never provisions applications, uploads code, or obtains credentials. Requests
can mutate application data; use disposable load-test fleets, not production.
"""
import argparse
from collections import Counter
from concurrent.futures import ThreadPoolExecutor
from html.parser import HTMLParser
import json
import math
import re
import time
import urllib.error
import urllib.parse
import urllib.request


class AppLinks(HTMLParser):
    """Read app ports from directory anchors, not their JS-populated hrefs."""

    def __init__(self):
        super().__init__()
        self.links = {}
        self.anchor = None

    def handle_starttag(self, tag, attrs):
        if tag != 'a':
            return
        attributes = dict(attrs)
        if 'app-link' in attributes.get('class', '').split():
            self.anchor = [attributes.get('data-port'), '']
        else:
            self.anchor = None

    def handle_data(self, data):
        if self.anchor is not None:
            self.anchor[1] += data

    def handle_endtag(self, tag):
        if tag == 'a' and self.anchor is not None:
            port, slug = self.anchor
            self.links.setdefault(slug, []).append(port)
            self.anchor = None


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def discover_ports(origin, slugs, timeout):
    with urllib.request.build_opener(NoRedirect()).open(origin + '/', timeout=timeout) as response:
        html = response.read(1024 * 1024 + 1)
    if len(html) > 1024 * 1024:
        raise ValueError('Directory HTML exceeds 1 MiB')
    parser = AppLinks()
    parser.feed(html.decode('utf-8'))
    parser.close()
    ports = {}
    for slug in slugs:
        matches = parser.links.get(slug, [])
        if len(matches) != 1 or not matches[0] or not re.fullmatch(r'[0-9]+', matches[0]):
            raise ValueError(f'Missing, duplicate or invalid directory port for {slug}')
        port = int(matches[0])
        if not 3000 <= port <= 9999:
            raise ValueError(f'Directory port out of range for {slug}')
        ports[slug] = port
    return ports


def percentile(values, fraction):
    return sorted(values)[max(0, math.ceil(len(values) * fraction) - 1)]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--base-url', required=True, help='Trusted directory origin, e.g. http://127.0.0.1:8000')
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
    try:
        if url.port is None:
            parser.error('base-url must include the directory port')
    except ValueError:
        parser.error('base-url has an invalid port')
    if not 1 <= args.requests <= 1000000 or not 1 <= args.concurrency <= 256 or not 0 < args.timeout <= 120:
        parser.error('Invalid request count, concurrency or timeout')
    plan = {'apps': len(slugs), 'requests': args.requests, 'concurrency': args.concurrency, 'slugs': slugs}
    if args.dry_run:
        print(json.dumps(plan, indent=2))
        return
    if not args.allow_mutations:
        parser.error('Use disposable fleets and explicitly pass --allow-mutations')

    host = f'[{url.hostname}]' if ':' in url.hostname else url.hostname
    origin = f'{url.scheme}://{host}:{url.port}'
    try:
        ports = discover_ports(origin, slugs, args.timeout)
    except (ValueError, UnicodeError, urllib.error.URLError, TimeoutError, OSError) as error:
        parser.error(f'Cannot resolve app ports from directory: {error}')

    def request(index):
        # Each slot exercises a stable named DO in the reference counter fixture.
        # Other representative apps may ignore the query and exercise their own path.
        endpoint = f'{url.scheme}://{host}:{ports[slugs[index % len(slugs)]]}/?name=capacity-{index % args.concurrency}'
        start = time.monotonic()
        try:
            with urllib.request.build_opener(NoRedirect()).open(endpoint, timeout=args.timeout) as response:
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
