#!/usr/bin/env python3
import importlib.util
import os
from pathlib import Path
import socket
import subprocess
import sys
import threading
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('ssh_relay', Path(__file__).with_name('ssh-relay.py'))
relay = importlib.util.module_from_spec(spec)
spec.loader.exec_module(relay)


class RelayTests(unittest.TestCase):
    def test_arguments_are_data_and_command_is_fixed(self):
        args = relay.ssh_command('vm+sample@vm.example', 2222, '/tmp/key with spaces')
        self.assertEqual(args[-3:], ['--', 'vm+sample@vm.example', '/usr/bin/nc -N 127.0.0.1 2222'])
        self.assertIn('ClearAllForwardings=yes', args)
        self.assertIn('ForwardAgent=no', args)
        self.assertIn('/tmp/key with spaces', args)
        for value in ('-oProxyCommand=evil', 'host;id', 'host\nother', ''):
            with self.assertRaises(Exception):
                relay.ssh_command(value, 2222)
        for value in ('0', '-1', '65536', '2222;id'):
            with self.assertRaises(Exception):
                relay.port(value)

    def test_environment_keeps_local_agent_not_storage(self):
        with patch.dict(os.environ, {'PATH': '/bin', 'HOME': '/tmp', 'SSH_AUTH_SOCK': '/tmp/agent',
                                     'AWS_ACCESS_KEY_ID': 'secret', 'S3_ENDPOINT': 'secret',
                                     'SECRET': 'secret'}, clear=True):
            self.assertEqual(relay.ssh_environment(), {'PATH': '/bin', 'HOME': '/tmp',
                                                       'SSH_AUTH_SOCK': '/tmp/agent'})

    def test_duplex_large_bytes_and_multiple_connections(self):
        # A subprocess on socket stdin/stdout models the outer ssh byte stream.
        command = [sys.executable, '-c', 'import os\nwhile True:\n b=os.read(0,8192)\n if not b:break\n os.write(1,b)']
        server = relay.RelayServer(0, command)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            self.assertEqual(server.server_address[0], '127.0.0.1')
            for _ in range(2):
                with socket.create_connection(server.server_address, timeout=5) as sock:
                    data = bytes(range(256)) * 4096
                    writer = threading.Thread(target=lambda: (sock.sendall(data), sock.shutdown(socket.SHUT_WR)))
                    writer.start()
                    result = bytearray()
                    while True:
                        chunk = sock.recv(8192)
                        if not chunk:
                            break
                        result.extend(chunk)
                    writer.join(timeout=5)
                    self.assertFalse(writer.is_alive())
                    self.assertEqual(result, data)
        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=5)

    def test_closing_server_reaps_active_children(self):
        server = relay.RelayServer(0, [sys.executable, '-c', 'import time;time.sleep(60)'])
        left, right = socket.socketpair()
        try:
            child = server.launch(left)
            server.server_close()
            self.assertIsNotNone(child.poll())
            self.assertIsNone(server.launch(left))
        finally:
            left.close()
            right.close()


if __name__ == '__main__':
    unittest.main()
