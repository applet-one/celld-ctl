#!/usr/bin/env python3
"""Loopback-only SSH stdio relay for VM owners when SSH TCP forwarding is denied.

Uses a normal authenticated SSH remote command, never -L/-W/direct-tcpip.
Requires Python 3 locally and OpenBSD nc at /usr/bin/nc on the VM.
This uses administrator SSH access; it is NOT a restricted CI access solution.
"""
import argparse
import os
import re
import socketserver
import subprocess
import sys
import threading


MAX_CONNECTIONS = 8
CONNECTION_TIMEOUT = 600


def destination(value):
    if not value or value.startswith('-') or not re.fullmatch(r'[A-Za-z0-9_.@:+\[\]-]+', value):
        raise argparse.ArgumentTypeError('use a literal SSH destination, not a command/options')
    return value


def port(value):
    try:
        number = int(value)
    except ValueError as error:
        raise argparse.ArgumentTypeError('port must be an integer') from error
    if not 1 <= number <= 65535:
        raise argparse.ArgumentTypeError('port must be between 1 and 65535')
    return number


def ssh_command(via, remote_port, identity=None):
    destination(via)
    remote_port = port(str(remote_port))
    args = ['ssh', '-T', '-a', '-x']
    for option in ('BatchMode=yes', 'ForwardAgent=no', 'ForwardX11=no',
                   'RequestTTY=no', 'ClearAllForwardings=yes', 'PermitLocalCommand=no',
                   'StrictHostKeyChecking=yes', 'ConnectTimeout=15',
                   'ServerAliveInterval=30', 'ServerAliveCountMax=3',
                   'ControlMaster=no', 'ControlPath=none'):
        args.extend(['-o', option])
    if identity:
        args.extend(['-o', 'IdentitiesOnly=yes', '-i', identity])
    # The destination is argv, and the only remote command is fixed apart from
    # a validated numeric port. No client shell or arbitrary command template.
    args.extend(['--', via, f'/usr/bin/nc -N 127.0.0.1 {remote_port}'])
    return args


def ssh_environment():
    # The outer owner connection may authenticate with the LOCAL agent; -a
    # prevents forwarding that agent to the VM. No cloud credentials survive.
    return {name: os.environ[name] for name in ('PATH', 'HOME', 'LANG', 'SSH_AUTH_SOCK')
            if name in os.environ}


def stop_process(process):
    if process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=2)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()


class RelayServer(socketserver.ThreadingMixIn, socketserver.TCPServer):
    allow_reuse_address = True
    daemon_threads = True
    block_on_close = False

    def __init__(self, listen_port, command):
        self.command = command
        self.slots = threading.BoundedSemaphore(MAX_CONNECTIONS)
        self.lock = threading.Lock()
        self.children = set()
        self.stopping = False
        super().__init__(('127.0.0.1', listen_port), RelayHandler)

    def launch(self, connection):
        with self.lock:
            if self.stopping:
                return None
            process = subprocess.Popen(self.command, stdin=connection, stdout=connection,
                                       stderr=None, env=ssh_environment(), close_fds=True)
            self.children.add(process)
            return process

    def server_close(self):
        with self.lock:
            self.stopping = True
            processes = tuple(self.children)
        super().server_close()
        for process in processes:
            stop_process(process)


class RelayHandler(socketserver.BaseRequestHandler):
    def handle(self):
        if not self.server.slots.acquire(blocking=False):
            print('Relay connection limit reached', file=sys.stderr)
            return
        process = None
        try:
            # Give both ends of the accepted socket directly to ssh: no Python
            # buffering or competing pipe pumps, so duplex SSH bytes stay intact.
            process = self.server.launch(self.request)
            if process is None:
                return
            try:
                code = process.wait(timeout=CONNECTION_TIMEOUT)
                if code:
                    print(f'Outer SSH relay exited with code {code}', file=sys.stderr)
            except subprocess.TimeoutExpired:
                print('SSH relay connection timed out', file=sys.stderr)
        except OSError as error:
            print(f'Cannot start outer SSH relay: {error}', file=sys.stderr)
        finally:
            if process is not None:
                stop_process(process)
                with self.server.lock:
                    self.server.children.discard(process)
            self.server.slots.release()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--via', required=True, type=destination,
                        help='Your existing administrator SSH destination for the VM')
    parser.add_argument('--listen-port', type=port, default=2222)
    parser.add_argument('--remote-port', type=port, default=2222)
    parser.add_argument('--identity', help='Optional outer administrator key, NOT the deploy key')
    args = parser.parse_args()
    command = ssh_command(args.via, args.remote_port, args.identity)
    try:
        with RelayServer(args.listen_port, command) as server:
            print(f'Listening on 127.0.0.1:{args.listen_port}; SSH stdio through {args.via} '
                  f'to VM loopback:{args.remote_port}. Keep this terminal open; Ctrl-C stops it.',
                  file=sys.stderr, flush=True)
            server.serve_forever(poll_interval=0.2)
    except KeyboardInterrupt:
        return 0
    except OSError as error:
        parser.exit(1, f'ssh-relay: {error}\n')
    return 0


if __name__ == '__main__':
    sys.exit(main())
