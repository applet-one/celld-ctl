#!/usr/bin/env python3
"""Hermetic policy, release and path-safety tests (no root/systemd/network)."""
import importlib.util
import json
from pathlib import Path
import stat
import tempfile
import unittest
from unittest.mock import patch, Mock

spec = importlib.util.spec_from_file_location('installer', Path(__file__).with_name('install_host.py'))
i = importlib.util.module_from_spec(spec)
spec.loader.exec_module(i)
smoke_spec = importlib.util.spec_from_file_location('smoke', Path(__file__).with_name('test_local_storage.py'))
smoke = importlib.util.module_from_spec(smoke_spec)
smoke_spec.loader.exec_module(smoke)


class Policy(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.config, self.state, self.node, self.rust = [self.root / p for p in ('config', 'state', 'node', 'rust')]
        self.local = {'endpoint': i.LOCAL_ENDPOINT, 'bucket': i.LOCAL_BUCKET,
                      'region': 'us-east-1', 'celld_version': '0.6.1'}
        self.metadata = {'schema': 1, 'mode': 'local', 'phase': 'ready',
                         'celld_version': '0.6.1', 'rustfs_version': '1.0.1'}

    def put(self, path, content):
        path.write_text(content)
        path.chmod(0o600)

    def credentials(self):
        self.put(self.node, f'AWS_ACCESS_KEY_ID={"a" * 48}\nAWS_SECRET_ACCESS_KEY={"b" * 64}\n')
        self.put(self.rust, f'RUSTFS_ACCESS_KEY={"a" * 48}\nRUSTFS_SECRET_KEY={"b" * 64}\n')

    def classify(self):
        return i.classify(self.config, self.state, self.node, self.rust)[0]

    def test_default_is_unqualified_external(self):
        self.assertEqual(self.classify(), 'fresh')
        self.assertEqual(i.select(None, 'fresh'), 'external')
        self.assertEqual(i.select('local', 'fresh'), 'local')

    def test_external_preservation(self):
        self.put(self.config, json.dumps({'endpoint': 'https://example.com',
                                          'bucket': 's3://existing', 'celld_version': '0.6.1'}))
        self.put(self.node, 'AWS_ACCESS_KEY_ID=xyz\nAWS_SECRET_ACCESS_KEY=operator\n')
        self.assertEqual(self.classify(), 'external')
        self.assertEqual(i.select(None, 'external'), 'external')
        with self.assertRaises(i.InstallError):
            i.select('local', 'external')

    def test_local_reuses_matching_credentials(self):
        self.credentials()
        self.put(self.config, json.dumps(self.local))
        self.put(self.state, json.dumps(self.metadata))
        self.assertEqual(self.classify(), 'local')
        self.assertEqual(i.select(None, 'local'), 'local')
        with self.assertRaises(i.InstallError):
            i.select('external', 'local')

    def test_preparing_recovery_and_contradictions(self):
        self.metadata['phase'] = 'preparing'
        self.put(self.state, json.dumps(self.metadata))
        self.assertEqual(self.classify(), 'local')  # helper not yet run
        self.credentials()
        self.assertEqual(self.classify(), 'local')  # helper can revalidate pair
        self.put(self.rust, f'RUSTFS_ACCESS_KEY={"c" * 48}\nRUSTFS_SECRET_KEY={"b" * 64}\n')
        with self.assertRaises(i.InstallError):
            self.classify()
        self.node.unlink()
        self.assertEqual(self.classify(), 'local')  # service pair can reconstruct node pair
        self.put(self.rust, 'RUSTFS_ACCESS_KEY=broken\nRUSTFS_SECRET_KEY=also_broken\n')
        with self.assertRaises(i.InstallError):
            self.classify()
        self.rust.unlink()
        self.put(self.node, 'AWS_ACCESS_KEY_ID=orphan\nAWS_SECRET_ACCESS_KEY=orphan\n')
        with self.assertRaises(i.InstallError):
            self.classify()

    def test_ready_cannot_resume_partial_service_pair(self):
        self.put(self.state, json.dumps(self.metadata))
        self.put(self.config, json.dumps(self.local))
        self.credentials()
        self.node.unlink()
        with self.assertRaises(i.InstallError):
            self.classify()

    def test_service_only_preparing_rejects_config_contradiction(self):
        self.metadata['phase'] = 'preparing'
        self.put(self.state, json.dumps(self.metadata))
        self.credentials()
        self.node.unlink()
        self.put(self.config, json.dumps(dict(self.local, endpoint='https://existing.example')))
        with self.assertRaises(i.InstallError):
            self.classify()

    def test_unknown_local_config_not_adopted(self):
        self.put(self.config, json.dumps(self.local))
        self.put(self.node, 'AWS_ACCESS_KEY_ID=x\nAWS_SECRET_ACCESS_KEY=y\n')
        with self.assertRaises(i.InstallError):
            self.classify()

    def test_symlink_and_bad_modes_and_parent(self):
        real = self.root / 'real'
        real.write_text('secret')
        (self.root / 'link').symlink_to(real)
        with self.assertRaises(i.InstallError):
            i.safe_path(self.root / 'link')
        real.chmod(0o644)
        with self.assertRaises(i.InstallError):
            i.safe_path(real, secret=True)
        self.root.chmod(0o777)
        with self.assertRaises(i.InstallError):
            i.safe_path(self.root / 'absent')

    def test_manifest_exact_archives(self):
        manifest = i.read_json(i.REPO / 'scripts/host-releases.json')
        for name in ('celld', 'rustfs'):
            for arch in ('x86_64', 'aarch64'):
                asset = manifest[name][arch]
                self.assertRegex(asset['sha256'], r'^[0-9a-f]{64}$')
                self.assertTrue(asset['url'].startswith('https://github.com/'))
                self.assertNotIn('latest', asset['url'])

    def test_rustfs_multiline_version_output(self):
        i.check_version('rustfs 1.0.1\nbuild time   : 2026-10-03\n', 'rustfs', '1.0.1')
        i.check_version('celld 0.6.1\n', 'celld', '0.6.1')
        for output in ('rustfs 1.0.2\n', 'something else\n', ''):
            with self.assertRaises(i.InstallError):
                i.check_version(output, 'rustfs', '1.0.1')

    def test_checksum_failure_never_unpacks(self):
        def bad_download(*args, **kwargs):
            Path(args[-1]).write_bytes(b'not the official asset')
            return Mock(returncode=0)
        manifest = i.read_json(i.REPO / 'scripts/host-releases.json')
        with patch.object(i, 'run', side_effect=bad_download), patch.object(i, 'unpack') as unpack:
            with self.assertRaisesRegex(i.InstallError, 'checksum mismatch'):
                i.verified_release(manifest['rustfs'], 'rustfs', self.root / 'missing',
                                   self.root)
        unpack.assert_not_called()

    def test_fake_installer_default_and_explicit_local(self):
        # Substitute filesystem, commands and downloads; absolutely no host writes.
        from contextlib import ExitStack
        paths = {'STATE': self.state, 'CONFIG': self.config, 'NODE_ENV': self.node,
                 'RUSTFS_ENV': self.rust, 'RUSTFS_DATA': self.root / 'data',
                 'RUSTFS_UNIT': self.root / 'unit', 'CADDY': self.root / 'caddy',
                 'REGISTRY': self.root / 'registry',
                 'CELLD_RELEASES': self.root / 'celld', 'RUSTFS_RELEASES': self.root / 'rustfs-release'}
        with ExitStack() as stack:
            for key, path in paths.items():
                stack.enter_context(patch.object(i, key, path))
            stack.enter_context(patch.object(i, 'safe_path'))
            stack.enter_context(patch.object(i, 'occupied', return_value=False))
            stack.enter_context(patch.object(i, 'verified_release', return_value=None))
            components = stack.enter_context(patch.object(i, 'install_components'))
            local = stack.enter_context(patch.object(i, 'local_install'))
            caddy = stack.enter_context(patch.object(i, 'init_caddy'))
            commands = stack.enter_context(patch.object(i, 'run', return_value=Mock(returncode=1)))
            i.install(None)
            components.assert_called_once()
            local.assert_not_called()
            caddy.assert_called_once()
            commands.reset_mock()
            local.reset_mock()
            components.reset_mock()
            i.install('local')
            local.assert_called_once()
            self.assertEqual(local.call_args.args[0], 'fresh')
            self.assertTrue(any(call.args[:3] == ('systemctl', 'is-active', '--quiet')
                                for call in commands.call_args_list) is False)

    def test_fake_installer_rejects_conflicting_port_before_install(self):
        self.put(self.config, json.dumps(self.local))
        self.put(self.state, json.dumps(self.metadata))
        self.credentials()
        with patch.object(i, 'STATE', self.state), patch.object(i, 'CONFIG', self.config), \
             patch.object(i, 'NODE_ENV', self.node), patch.object(i, 'RUSTFS_ENV', self.rust), \
             patch.object(i, 'RUSTFS_DATA', self.root / 'data'), \
             patch.object(i, 'RUSTFS_UNIT', self.root / 'unit'), \
             patch.object(i, 'safe_path'), patch.object(i, 'occupied', return_value=True), \
             patch.object(i, 'run', return_value=Mock(returncode=1)), \
             patch.object(i, 'install_components') as components:
            with self.assertRaisesRegex(i.InstallError, 'Port 9000'):
                i.install(None)
        components.assert_not_called()

    def test_fake_installer_refuses_orphan_registry(self):
        orphan = self.root / 'registry'
        orphan.touch()
        with patch.object(i, 'STATE', self.state), patch.object(i, 'CONFIG', self.config), \
             patch.object(i, 'NODE_ENV', self.node), patch.object(i, 'RUSTFS_ENV', self.rust), \
             patch.object(i, 'REGISTRY', orphan), patch.object(i, 'safe_path'), \
             patch.object(i, 'install_components') as components:
            with self.assertRaisesRegex(i.InstallError, 'Existing registry'):
                i.install('local')
        components.assert_not_called()

    def test_disposable_host_gate_requires_marker_and_empty_registry(self):
        marker = self.root / 'marker'
        registry = self.root / 'registry'
        marker.write_text('throwaway-vm\n')
        marker.chmod(0o600)
        with patch.object(smoke.os, 'geteuid', return_value=0), \
             patch.object(smoke, 'safe_path'), \
             patch.object(smoke.socket, 'gethostname', return_value='throwaway-vm'):
            with self.assertRaisesRegex(smoke.InstallError, 'Explicit'):
                smoke.validate_target('production', 'throwaway-vm', marker, registry)
            smoke.validate_target('throwaway-vm', 'throwaway-vm', marker, registry)
            registry.touch()
            with self.assertRaisesRegex(smoke.InstallError, 'Existing registry'):
                smoke.validate_target('throwaway-vm', 'throwaway-vm', marker, registry)


if __name__ == '__main__':
    unittest.main()
