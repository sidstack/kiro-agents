#!/usr/bin/env python3
"""Archive validation for the packaged-binary harness: checksums and unsafe tars."""
import hashlib
import importlib.util
import io
from pathlib import Path
import tarfile
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('installed_binary', ROOT / 'tests/installed_binary.py')
harness = importlib.util.module_from_spec(spec)
spec.loader.exec_module(harness)

VERSION = '0.3.1-beta.42.1'
PREFIX = f'kiro-agents-v{VERSION}-x86_64-unknown-linux-gnu'
MEMBERS = [('kiro-agents', b'\x7fELF fake binary', 0o755),
           ('LICENSE', b'mit', 0o644),
           ('README.md', b'# kiro-agents', 0o644),
           ('CHANGELOG.md', b'## 0.3.1', 0o644)]


def build(archive_dir, members=MEMBERS, name=None, tweak=None):
    """Write archive plus matching SHA256SUMS, optionally with mutations."""
    archive = Path(archive_dir) / (name or f'{PREFIX}.tar.gz')
    with tarfile.open(archive, mode='w:gz') as tar:
        for label, data, mode in members:
            member = tarfile.TarInfo(f'{PREFIX}/{label}')
            member.size = len(data)
            member.mode = mode
            if tweak:
                tweak(label, member)
            tar.addfile(member, io.BytesIO(data))
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    sums = archive.parent / 'SHA256SUMS'
    sums.write_text(f'{digest}  {archive.name}\n')
    return archive


class InstalledBinary(unittest.TestCase):
    def test_accepts_a_well_formed_versioned_archive(self):
        with tempfile.TemporaryDirectory() as tmp:
            archive = build(Path(tmp))
            digest = harness.verify_checksum(archive, VERSION)
            self.assertEqual(digest, hashlib.sha256(archive.read_bytes()).hexdigest())
            binary = harness.extract(archive, Path(tmp) / 'out', VERSION)
            self.assertEqual(binary.read_bytes(), MEMBERS[0][1])
            self.assertEqual(binary.stat().st_mode & 0o7777, 0o755)
            self.assertEqual(sorted(p.name for p in (Path(tmp) / 'out').iterdir()),
                             ['CHANGELOG.md', 'LICENSE', 'README.md', 'kiro-agents'])

    def test_rejects_mismatched_or_missing_checksums(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            archive = build(root)
            (root / 'SHA256SUMS').write_text(
                f'{"0" * 64}  {archive.name}\n')
            with self.assertRaises(ValueError):
                harness.verify_checksum(archive, VERSION)
            archive = build(root)
            (root / 'SHA256SUMS').unlink()
            with self.assertRaises(ValueError):
                harness.verify_checksum(archive, VERSION)
            archive = build(root)
            (root / 'SHA256SUMS').write_text(f'{"0" * 64}  other.tar.gz\n')
            with self.assertRaises(ValueError):
                harness.verify_checksum(archive, VERSION)

    def test_rejects_duplicate_or_malformed_checksum_entries(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            archive = build(root)
            digest = hashlib.sha256(archive.read_bytes()).hexdigest()
            (root / 'SHA256SUMS').write_text(
                f'{digest}  {archive.name}\n{digest}  {archive.name}\n')
            with self.assertRaises(ValueError):
                harness.verify_checksum(archive, VERSION)
            (root / 'SHA256SUMS').write_text(f'DEADBEEF  {archive.name}\n')
            with self.assertRaises(ValueError):
                harness.verify_checksum(archive, VERSION)

    def test_rejects_archive_name_for_another_version(self):
        with tempfile.TemporaryDirectory() as tmp:
            archive = build(Path(tmp), name='kiro-agents-v9.9.9-x86_64-unknown-linux-gnu.tar.gz')
            with self.assertRaises(ValueError):
                harness.verify_checksum(archive, VERSION)

    def test_rejects_unsafe_or_unexpected_members(self):
        cases = [
            ('absolute path', lambda label, member: setattr(member, 'name', f'/etc/{label}')),
            ('parent traversal', lambda label, member: setattr(member, 'name', f'{PREFIX}/../{label}')),
            ('wrong prefix', lambda label, member: setattr(member, 'name', f'other/{label}')),
        ]
        with tempfile.TemporaryDirectory() as tmp:
            for label, tweak in cases:
                with self.subTest(label):
                    archive = build(Path(tmp), tweak=tweak)
                    with self.assertRaises(ValueError):
                        harness.inspect_archive(archive, VERSION)

    def test_rejects_non_regular_members_and_extra_files(self):
        with tempfile.TemporaryDirectory() as tmp:
            dir_archive = Path(tmp) / f'{PREFIX}.tar.gz'
            with tarfile.open(dir_archive, mode='w:gz') as tar:
                for name, data, mode in MEMBERS:
                    member = tarfile.TarInfo(f'{PREFIX}/{name}')
                    member.size, member.mode = len(data), mode
                    tar.addfile(member, io.BytesIO(data))
                directory = tarfile.TarInfo(f'{PREFIX}/extra')
                directory.type = tarfile.DIRTYPE
                directory.mode = 0o755
                tar.addfile(directory)
            (tmp := Path(tmp) / 'SHA256SUMS').write_text(
                f'{hashlib.sha256(dir_archive.read_bytes()).hexdigest()}  {dir_archive.name}\n')
            with self.assertRaises(ValueError):
                harness.inspect_archive(dir_archive, VERSION)

        with tempfile.TemporaryDirectory() as tmp:
            symlink_archive = Path(tmp) / f'{PREFIX}.tar.gz'
            with tarfile.open(symlink_archive, mode='w:gz') as tar:
                for name, data, mode in MEMBERS:
                    member = tarfile.TarInfo(f'{PREFIX}/{name}')
                    member.size, member.mode = len(data), mode
                    tar.addfile(member, io.BytesIO(data))
                link = tarfile.TarInfo(f'{PREFIX}/kiro-agents')
                link.type = tarfile.SYMTYPE
                link.linkname = '/bin/sh'
                tar.addfile(link)
            with self.assertRaises(ValueError):
                harness.inspect_archive(symlink_archive, VERSION)

        with tempfile.TemporaryDirectory() as tmp:
            archive = build(Path(tmp), members=list(MEMBERS) + [('host.log', b'secret', 0o644)])
            with self.assertRaises(ValueError):
                harness.inspect_archive(archive, VERSION)

    def test_rejects_wrong_permissions_and_bad_versions(self):
        with tempfile.TemporaryDirectory() as tmp:
            archive = build(Path(tmp), tweak=lambda label, m: setattr(m, 'mode', 0o644)
                            if label == 'kiro-agents' else None)
            with self.assertRaises(ValueError):
                harness.inspect_archive(archive, VERSION)
        with tempfile.TemporaryDirectory() as tmp:
            archive = build(Path(tmp))
            for bad in ['', '../x', '0.3', '0.3.1 ', None]:
                with self.subTest(repr(bad)):
                    with self.assertRaises(ValueError):
                        harness.verify_checksum(archive, bad)


if __name__ == '__main__':
    unittest.main()
