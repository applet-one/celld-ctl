#!/usr/bin/python3
import contextlib
import importlib.util
import io
import json
from pathlib import Path
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('load_test', Path(__file__).with_name('load-test.py'))
load = importlib.util.module_from_spec(spec)
spec.loader.exec_module(load)

class ProbeTests(unittest.TestCase):
    def run_cli(self, *args):
        out = io.StringIO()
        with patch('sys.argv', ['load-test.py', *args]), contextlib.redirect_stdout(out), contextlib.redirect_stderr(io.StringIO()):
            load.main()
        return json.loads(out.getvalue())

    def test_plan_and_mutation_acknowledgement(self):
        args = ['--base-url', 'http://127.0.0.1:8000', '--slugs', 'a,9-app']
        self.assertEqual(self.run_cli(*args, '--dry-run')['apps'], 2)
        with self.assertRaises(SystemExit):
            self.run_cli(*args)
        with self.assertRaises(SystemExit):
            self.run_cli('--base-url', 'http://user:password@localhost', '--slugs', 'a', '--dry-run')

    def test_redirect_is_failure_not_followed(self):
        class Handler(BaseHTTPRequestHandler):
            def do_GET(self):
                self.send_response(302)
                self.send_header('Location', '/would-be-login')
                self.end_headers()
            def log_message(self, *args):
                pass
        server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            with self.assertRaises(SystemExit) as result:
                self.run_cli('--base-url', f'http://127.0.0.1:{server.server_port}', '--slugs', 'a', '--requests', '2', '--allow-mutations')
            self.assertEqual(result.exception.code, 1)
        finally:
            server.shutdown()
            server.server_close()
            thread.join()

    def test_percentile(self):
        self.assertEqual(load.percentile([5, 1, 3, 2, 4], .5), 3)
        self.assertEqual(load.percentile([5, 1, 3, 2, 4], .99), 5)

if __name__ == '__main__':
    unittest.main()
