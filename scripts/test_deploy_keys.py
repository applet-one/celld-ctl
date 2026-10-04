#!/usr/bin/python3
"""Hermetic key lifecycle tests; never write installed configuration."""
import contextlib
import importlib.machinery
import importlib.util
import io
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

loader = importlib.machinery.SourceFileLoader('deploy_keys', str(Path(__file__).with_name('celld-deploy-key')))
spec = importlib.util.spec_from_loader(loader.name, loader)
keys = importlib.util.module_from_spec(spec)
loader.exec_module(keys)

class KeyTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        keys.CONFIG = self.root
        keys.AUTHORIZED = self.root / 'authorized_keys'
        keys.AUTHORIZED.write_text('')
        self.key = self.root / 'key'
        subprocess.run(['ssh-keygen', '-q', '-t', 'ed25519', '-N', '', '-f', str(self.key)], check=True)

    def tearDown(self):
        self.temp.cleanup()

    def run_cli(self, *args):
        with patch('sys.argv', ['celld-deploy-key', *map(str, args)]), patch('os.geteuid', return_value=0), contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
            keys.main()

    def test_add_list_revoke_and_modes(self):
        self.run_cli('add', 'owner-laptop', str(self.key) + '.pub', '--kind', 'owner')
        self.assertTrue(keys.AUTHORIZED.read_text().startswith('restrict ssh-ed25519 '))
        self.assertIn('cella:owner:owner-laptop', keys.AUTHORIZED.read_text())
        self.assertEqual(keys.AUTHORIZED.stat().st_mode & 0o777, 0o600)
        self.run_cli('list')
        self.run_cli('revoke', 'owner-laptop')
        self.assertEqual(keys.AUTHORIZED.read_text(), '')

    def test_duplicate_key_rejected(self):
        self.run_cli('add', 'ci-one', str(self.key) + '.pub', '--kind', 'ci')
        with self.assertRaises(SystemExit):
            self.run_cli('add', 'ci-two', str(self.key) + '.pub', '--kind', 'ci')
        self.assertEqual(len(keys.AUTHORIZED.read_text().splitlines()), 1)

    def test_invalid_label_and_key_options_rejected(self):
        with self.assertRaises(SystemExit):
            self.run_cli('add', '../bad', str(self.key) + '.pub', '--kind', 'ci')
        bad = self.root / 'bad.pub'
        bad.write_text('command="sh" ' + Path(str(self.key) + '.pub').read_text())
        with self.assertRaises(SystemExit):
            self.run_cli('add', 'bad', bad, '--kind', 'ci')
        self.assertEqual(keys.AUTHORIZED.read_text(), '')

    def test_untracked_keys_not_overwritten(self):
        keys.AUTHORIZED.write_text(Path(str(self.key) + '.pub').read_text())
        with self.assertRaises(SystemExit):
            self.run_cli('add', 'new', str(self.key) + '.pub', '--kind', 'ci')

if __name__ == '__main__':
    unittest.main()
