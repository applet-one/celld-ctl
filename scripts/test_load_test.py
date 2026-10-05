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


@contextlib.contextmanager
def serve(handler, dedicated=False):
    if dedicated:
        for port in range(9100, 10000):
            try:
                server = ThreadingHTTPServer(('127.0.0.1', port), handler)
                break
            except OSError:
                continue
        else:
            raise RuntimeError('No dedicated test port available')
    else:
        server = ThreadingHTTPServer(('127.0.0.1', 0), handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield server
    finally:
        server.shutdown()
        server.server_close()
        thread.join()


class QuietHandler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass


class ProbeTests(unittest.TestCase):
    def run_cli(self, *args):
        out = io.StringIO()
        with patch('sys.argv', ['load-test.py', *args]), contextlib.redirect_stdout(out), contextlib.redirect_stderr(io.StringIO()):
            load.main()
        return json.loads(out.getvalue())

    def test_plan_and_mutation_acknowledgement(self):
        args = ['--base-url', 'http://127.0.0.1:8000', '--slugs', 'a,9-app']
        with patch.object(load.urllib.request, 'build_opener', side_effect=AssertionError('network used')):
            self.assertEqual(self.run_cli(*args, '--dry-run')['apps'], 2)
            with self.assertRaises(SystemExit):
                self.run_cli(*args)
        with self.assertRaises(SystemExit):
            self.run_cli('--base-url', 'http://user:password@localhost', '--slugs', 'a', '--dry-run')
        with self.assertRaises(SystemExit):
            self.run_cli(*args, '--requests', '0', '--dry-run')

    def test_directory_ports_and_round_robin_requests(self):
        app_paths = {}
        directory_paths = []

        class App(QuietHandler):
            def do_GET(self):
                app_paths.setdefault(self.server.server_port, []).append(self.path)
                self.send_response(200)
                self.end_headers()
                self.wfile.write(b'ok')

        with serve(App, dedicated=True) as first, serve(App, dedicated=True) as second:
            class Directory(QuietHandler):
                def do_GET(self):
                    directory_paths.append(self.path)
                    self.send_response(200)
                    self.end_headers()
                    self.wfile.write(
                        f'<a class="app-link" data-port="{first.server_port}" href="/">alpha</a>'
                        f'<a class="app-link" data-port="{second.server_port}" href="/">beta</a>'.encode()
                    )

            with serve(Directory) as directory:
                result = self.run_cli('--base-url', f'http://127.0.0.1:{directory.server_port}',
                                      '--slugs', 'alpha,beta', '--requests', '4',
                                      '--concurrency', '2', '--allow-mutations')
        self.assertEqual(result['statuses'], {'200': 4})
        self.assertEqual(directory_paths, ['/'])
        self.assertCountEqual(app_paths[first.server_port], ['/?name=capacity-0', '/?name=capacity-0'])
        self.assertCountEqual(app_paths[second.server_port], ['/?name=capacity-1', '/?name=capacity-1'])

    def test_missing_invalid_and_duplicate_ports_fail_before_load(self):
        app_paths = []

        class App(QuietHandler):
            def do_GET(self):
                app_paths.append(self.path)
                self.send_response(200)
                self.end_headers()

        with serve(App, dedicated=True) as app:
            for html in (
                '<a class="app-link" data-port="9101" href="/">alpha-extra</a>',
                '<a class="other" data-port="9101" href="/">alpha</a>',
                '<a class="app-link" href="/">alpha</a>',
                '<a class="app-link" data-port="9101evil" href="/">alpha</a>',
                '<a class="app-link" data-port="2999" href="/">alpha</a>',
                '<a class="app-link" data-port="10000" href="/">alpha</a>',
                f'<a class="app-link" data-port="{app.server_port}" href="/">alpha</a>' * 2,
            ):
                class Directory(QuietHandler):
                    def do_GET(self):
                        self.send_response(200)
                        self.end_headers()
                        self.wfile.write(html.encode())

                with self.subTest(html=html), serve(Directory) as directory:
                    with self.assertRaises(SystemExit) as result:
                        self.run_cli('--base-url', f'http://127.0.0.1:{directory.server_port}',
                                     '--slugs', 'alpha', '--requests', '1', '--allow-mutations')
                    self.assertEqual(result.exception.code, 2)
        self.assertEqual(app_paths, [])

    def test_directory_redirect_is_not_followed(self):
        paths = []

        class Directory(QuietHandler):
            def do_GET(self):
                paths.append(self.path)
                self.send_response(302)
                self.send_header('Location', '/would-be-login')
                self.end_headers()

        with serve(Directory) as directory:
            with self.assertRaises(SystemExit) as result:
                self.run_cli('--base-url', f'http://127.0.0.1:{directory.server_port}',
                             '--slugs', 'alpha', '--requests', '1', '--allow-mutations')
        self.assertEqual(result.exception.code, 2)
        self.assertEqual(paths, ['/'])

    def test_app_redirect_is_failure_not_followed(self):
        paths = []

        class App(QuietHandler):
            def do_GET(self):
                paths.append(self.path)
                self.send_response(302)
                self.send_header('Location', '/would-be-login')
                self.end_headers()

        with serve(App, dedicated=True) as app:
            class Directory(QuietHandler):
                def do_GET(self):
                    self.send_response(200)
                    self.end_headers()
                    self.wfile.write(f'<a class="app-link" data-port="{app.server_port}" href="/">alpha</a>'.encode())

            with serve(Directory) as directory:
                with self.assertRaises(SystemExit) as result:
                    self.run_cli('--base-url', f'http://127.0.0.1:{directory.server_port}',
                                 '--slugs', 'alpha', '--requests', '2', '--allow-mutations')
        self.assertEqual(result.exception.code, 1)
        self.assertCountEqual(paths, ['/?name=capacity-0', '/?name=capacity-1'])

    def test_percentile(self):
        self.assertEqual(load.percentile([5, 1, 3, 2, 4], .5), 3)
        self.assertEqual(load.percentile([5, 1, 3, 2, 4], .99), 5)

if __name__ == '__main__':
    unittest.main()
