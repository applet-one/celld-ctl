#!/usr/bin/env python3
"""Hermetic safety checks for test_local_deploy.py; never starts host services."""

import json
from pathlib import Path
import sqlite3
import tempfile
import unittest
import urllib.error
from unittest import mock

import test_local_deploy as smoke


class SafetyGateTests(unittest.TestCase):
    def test_marker_requires_root_hostname_ack_and_marker_content(self):
        with tempfile.TemporaryDirectory() as tmp:
            marker = Path(tmp) / 'marker'
            marker.write_text('disposable-vm\n')
            with mock.patch.object(smoke, 'private_file'):
                with mock.patch.object(smoke.socket, 'gethostname',
                                       return_value='disposable-vm'):
                    smoke.marker_gate('disposable-vm', 'disposable-vm', marker, euid=0)
                    for ack, hostname, uid in (
                        ('disposable-vm', 'disposable-vm', 1000),
                        ('other', 'disposable-vm', 0),
                        ('disposable-vm', 'other', 0),
                    ):
                        with self.assertRaises(smoke.InstallError):
                            smoke.marker_gate(ack, hostname, marker, euid=uid)
                    marker.write_text('other-vm\n')
                    with self.assertRaises(smoke.InstallError):
                        smoke.marker_gate('disposable-vm', 'disposable-vm', marker, euid=0)

    def test_missing_marker_fails_closed(self):
        with tempfile.TemporaryDirectory() as tmp:
            with mock.patch.object(smoke.socket, 'gethostname', return_value='vm'):
                with self.assertRaises(smoke.InstallError):
                    smoke.marker_gate('vm', 'vm', Path(tmp) / 'missing', euid=0)

    def test_local_install_requires_ready_exact_pin_target_and_paired_credentials(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            state, config, node, rustfs = (root / n for n in
                                            ('state', 'config', 'node', 'rustfs'))
            data, unit = root / 'data', root / 'rustfs.service'
            data.mkdir(mode=0o700)
            data.chmod(0o700)
            state.write_text(json.dumps({'schema': 1, 'mode': 'local',
                                         'phase': 'ready', 'rustfs_version': '1.0.1',
                                         'celld_version': '0.6.1'}))
            config.write_text(json.dumps({'endpoint': 'http://127.0.0.1:9000',
                                          'bucket': 's3://celld-dev',
                                          'region': 'us-east-1', 'celld_version': '0.6.1'}))
            node.touch()
            rustfs.touch()
            unit.write_bytes((smoke.REPO / 'examples/systemd/rustfs.service').read_bytes())
            mock_uid = mock.Mock(pw_uid=data.stat().st_uid)
            with (mock.patch.object(smoke, 'private_file'),
                  mock.patch.object(smoke, 'safe_path'),
                  mock.patch.object(smoke, 'root_owned_unit', return_value=True),
                  mock.patch.object(smoke.pwd, 'getpwnam', return_value=mock_uid),
                  mock.patch.object(smoke, 'check_pair', return_value=True) as pair):
                smoke.local_state_gate(state, config, node, rustfs, data, unit)
                pair.return_value = False
                with self.assertRaises(smoke.InstallError):
                    smoke.local_state_gate(state, config, node, rustfs, data, unit)
                pair.return_value = True
                original = json.loads(state.read_text())
                state.write_text(json.dumps({**original, 'phase': 'preparing'}))
                with self.assertRaises(smoke.InstallError):
                    smoke.local_state_gate(state, config, node, rustfs, data, unit)
                state.write_text(json.dumps(original))
                config.write_text(json.dumps({'endpoint': 'http://example.org',
                                              'bucket': 's3://celld-dev',
                                              'region': 'us-east-1', 'celld_version': '0.6.1'}))
                with self.assertRaises(smoke.InstallError):
                    smoke.local_state_gate(state, config, node, rustfs, data, unit)
                unit.write_text('unexpected service definition')
                with self.assertRaises(smoke.InstallError):
                    smoke.local_state_gate(state, config, node, rustfs, data, unit)

    def test_existing_registry_allowed_only_if_empty_and_no_target_slug(self):
        with tempfile.TemporaryDirectory() as tmp:
            registry = Path(tmp) / 'registry.sqlite'
            with mock.patch.object(smoke, 'safe_path'):
                smoke.registry_gate(registry)
                with sqlite3.connect(registry) as conn:
                    conn.execute('CREATE TABLE apps(slug TEXT PRIMARY KEY)')
                    conn.execute('INSERT INTO apps VALUES (?)', (smoke.SLUG,))
                with mock.patch.object(smoke, 'private_file'):
                    with self.assertRaisesRegex(smoke.InstallError, 'already exists'):
                        smoke.registry_gate(registry)
                    with sqlite3.connect(registry) as conn:
                        conn.execute('UPDATE apps SET slug=?', ('other-app',))
                    with self.assertRaisesRegex(smoke.InstallError, 'Existing apps'):
                        smoke.registry_gate(registry)
                    with sqlite3.connect(registry) as conn:
                        conn.execute('DELETE FROM apps')
                    smoke.registry_gate(registry)

    def test_namespace_rejects_existing_cache_env_unit_and_active_service(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            cache, env, unit_base = root / 'cache', root / 'app.env', root / 'units'
            unit_base.mkdir()
            inactive = mock.Mock(returncode=3)
            with mock.patch.object(smoke.subprocess, 'run', return_value=inactive) as run:
                kw = {'cache': cache, 'cell_env': env, 'unit_bases': [unit_base]}
                smoke.namespace_gate(**kw)
                cache.mkdir()
                with self.assertRaisesRegex(smoke.InstallError, 'local state'):
                    smoke.namespace_gate(**kw)
                cache.rmdir()
                env.touch()
                with self.assertRaises(smoke.InstallError):
                    smoke.namespace_gate(**kw)
                env.unlink()
                (unit_base / smoke.UNIT).touch()
                with self.assertRaises(smoke.InstallError):
                    smoke.namespace_gate(**kw)
                (unit_base / smoke.UNIT).unlink()
                run.return_value = mock.Mock(returncode=0)
                with self.assertRaisesRegex(smoke.InstallError, 'already'):
                    smoke.namespace_gate(**kw)

    def test_fixture_and_client_do_not_inherit_host_credentials(self):
        with tempfile.TemporaryDirectory() as tmp:
            user = mock.Mock(pw_uid=1234, pw_gid=1234, pw_name='client',
                             pw_dir='/home/client')
            with (mock.patch.object(smoke.tempfile, 'mkdtemp', return_value=tmp),
                  mock.patch.object(smoke.os, 'chown')):
                project = smoke.make_fixture(user)
                cfg = json.loads((project / 'wrangler.jsonc').read_text())
                self.assertEqual(cfg['migrations'][0]['new_sqlite_classes'], ['Counter'])
                self.assertEqual((cfg['name'], cfg['no_bundle']), (smoke.SLUG, True))
                self.assertIn('await this.state.storage.put("n", n)',
                              (project / 'index.js').read_text())
            with mock.patch.object(smoke.subprocess, 'run',
                                   return_value=mock.Mock(returncode=0, stdout='ok',
                                                          stderr='')) as run:
                self.assertEqual(smoke.client(user, Path('/private/key'),
                                              Path('/bin/cella'), project,
                                              ['deploy'], 660), 'ok')
                cmd = run.call_args.args[0]
                self.assertIn('env', cmd)
                self.assertIn('-i', cmd)
                self.assertIn('CELLA_HOST=cella-deploy@127.0.0.1', cmd)
                self.assertFalse(any(s.startswith('AWS_') or s.startswith('S3_') for s in cmd))
                self.assertEqual(run.call_args.kwargs['timeout'], 660)

    def test_status_requires_active_observed_published_version(self):
        good = {'target': {'slug': smoke.SLUG, 'enabled': True},
                'active': True, 'version_id': 'abcdef0123456789',
                'observed_version_id': 'abcdef0123456789'}
        with mock.patch.object(smoke, 'client', return_value=json.dumps(good)) as cli:
            args = (mock.Mock(), Path('/private/key'), Path('/bin/cella'), Path('/tmp/app'))
            self.assertEqual(smoke.verified_status(*args), 'abcdef0123456789')
            cli.return_value = json.dumps({**good, 'observed_version_id': None})
            with self.assertRaises(smoke.InstallError):
                smoke.verified_status(*args)
            cli.return_value = json.dumps({**good, 'target':
                                           {'slug': 'other-app', 'enabled': True}})
            with self.assertRaises(smoke.InstallError):
                smoke.verified_status(*args)

    def test_http_response_is_never_retried_for_mutating_counter_get(self):
        opener = mock.Mock()
        opener.open.side_effect = urllib.error.HTTPError(
            'http://127.0.0.1:8000/smoke-do/', 503, 'unavailable', {}, None)
        with (mock.patch.object(smoke.urllib.request, 'build_opener', return_value=opener),
              mock.patch.object(smoke.time, 'sleep') as sleep):
            with self.assertRaisesRegex(smoke.InstallError, 'HTTP 503'):
                smoke.counter('unique-name', 1, timeout=1)
        opener.open.assert_called_once()
        sleep.assert_not_called()


if __name__ == '__main__':
    unittest.main()
