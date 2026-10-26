#!/usr/bin/env python3
"""Validate release archive contents, identities, permissions and checksums."""
import hashlib
import importlib.util
from pathlib import Path
import tarfile
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('package_release', ROOT / 'scripts/package_release.py')
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)


class ReleaseArchive(unittest.TestCase):
    def test_mismatched_or_unsafe_versions_are_refused(self):
        for tag, version in [('v0.2.0', '0.3.0'), ('0.3.0', '0.3.0'), ('v../../x', '../../x')]:
            with self.assertRaises(ValueError):
                release.validate_tag(tag, version)
        release.validate_tag('v0.3.0', '0.3.0')

    def test_archive_contains_only_installable_binary_and_public_docs(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            binary = root / 'kiro-agents'
            binary.write_bytes(b'test native binary')
            for name in ['LICENSE', 'README.md', 'CHANGELOG.md', 'private.log']:
                (root / name).write_text(name)
            archive = release.create_archive(root, binary, '0.3.0', root / 'out')
            prefix = 'kiro-agents-v0.3.0-x86_64-unknown-linux-gnu/'
            with tarfile.open(archive) as tar:
                self.assertEqual(tar.getnames(), [prefix + n for n in ['kiro-agents', 'LICENSE', 'README.md', 'CHANGELOG.md']])
                self.assertEqual(tar.getmember(prefix + 'kiro-agents').mode, 0o755)
                self.assertEqual(tar.extractfile(prefix + 'kiro-agents').read(), binary.read_bytes())
            self.assertEqual((root / 'out/SHA256SUMS').read_text(), hashlib.sha256(archive.read_bytes()).hexdigest() + '  ' + archive.name + '\n')
            first = archive.read_bytes()
            release.create_archive(root, binary, '0.3.0', root / 'out')
            self.assertEqual(first, archive.read_bytes())


if __name__ == '__main__':
    unittest.main()
