#!/usr/bin/env python3
"""Exercise the real shell installer with local release fixtures and mocked HTTPS."""
import hashlib
import io
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class Installer(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix='kiro-installer-')
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.bin = self.root / 'commands'
        self.bin.mkdir()
        self.destination = self.root / 'install with spaces'
        self.env = dict(os.environ, PATH=str(self.bin) + ':' + os.environ['PATH'],
                        INSTALL_FIXTURES=str(self.root), INSTALL_ARCH='x86_64')
        self.mock('uname', 'case "$1" in -s) echo Linux;; -m) echo "${INSTALL_ARCH}";; esac')
        self.mock('getconf', 'echo "glibc 2.35"')
        self.mock('sleep', ':')
        self.mock('curl', '''
out=''
while [ "$#" -gt 0 ]; do
 case "$1" in
  --output) out=$2; shift 2;;
  --write-out|--proto|--retry|--connect-timeout|--max-time) shift 2;;
  https:*) url=$1; shift;;
  *) shift;;
 esac
done
case "$url" in
 */releases/latest) printf 'https://github.com/sidstack/kiro-agents/releases/tag/v1.2.3';;
 */releases/tag/*) :;;
 *.tar.gz)
   count=0
   if [ -f "$INSTALL_FIXTURES/pending-count" ]; then count=$(cat "$INSTALL_FIXTURES/pending-count"); fi
   if [ "$count" -lt "${INSTALL_PENDING_COUNT:-0}" ]; then
     echo $((count + 1)) > "$INSTALL_FIXTURES/pending-count"
     printf '404'
     echo 'fixture: assets not ready' >&2
     exit 22
   fi
   cp "$INSTALL_FIXTURES/${url##*/}" "$out";;
 *) cp "$INSTALL_FIXTURES/${url##*/}" "$out";;
esac
''')

    def mock(self, name, body):
        path = self.bin / name
        path.write_text('#!/bin/sh\nset -eu\n' + body + '\n')
        path.chmod(0o755)

    def fixture(self, version='1.2.3', kind='normal', identity=None):
        prefix = f'kiro-agents-v{version}-x86_64-unknown-linux-gnu'
        archive = self.root / (prefix + '.tar.gz')
        with tarfile.open(archive, 'w:gz') as tar:
            for label in ['kiro-agents', 'LICENSE', 'README.md', 'CHANGELOG.md']:
                content = (f'#!/bin/sh\nprintf "kiro-agents {identity or version}\\n"\n' if label == 'kiro-agents' else label).encode()
                info = tarfile.TarInfo(prefix + '/' + label)
                info.mode = 0o755 if label == 'kiro-agents' else 0o644
                info.size = len(content)
                if kind == 'traversal' and label == 'kiro-agents':
                    info.name = '../../outside'
                if kind == 'symlink' and label == 'kiro-agents':
                    info.type = tarfile.SYMTYPE
                    info.linkname = '/tmp/outside'
                    info.size = 0
                tar.addfile(info, io.BytesIO(content))
        (self.root / 'SHA256SUMS').write_text(hashlib.sha256(archive.read_bytes()).hexdigest() + '  ' + archive.name + '\n')
        return archive

    def run_install(self, *args, success=True):
        result = subprocess.run(['sh', str(ROOT / 'install.sh'), '--install-dir', str(self.destination), *args],
                                env=self.env, capture_output=True, text=True, timeout=15)
        if success:
            self.assertEqual(result.returncode, 0, result.stderr)
        else:
            self.assertNotEqual(result.returncode, 0, result.stdout)
        return result

    def test_latest_stable_exact_beta_upgrade_and_uninstall(self):
        self.fixture()
        self.run_install()
        binary = self.destination / 'kiro-agents'
        self.assertEqual(subprocess.check_output([str(binary)], text=True).strip(), 'kiro-agents 1.2.3')
        self.fixture('1.2.4-beta.4.1')
        self.run_install('--version', 'v1.2.4-beta.4.1')
        self.assertEqual(subprocess.check_output([str(binary)], text=True).strip(), 'kiro-agents 1.2.4-beta.4.1')
        self.assertEqual(binary.stat().st_mode & 0o777, 0o755)
        self.assertEqual(list(self.destination.glob('.kiro-agents.*')), [])
        self.run_install('--uninstall')
        self.assertFalse(binary.exists())

    def test_failed_checksum_or_identity_does_not_replace_existing_binary(self):
        self.destination.mkdir()
        binary = self.destination / 'kiro-agents'
        binary.write_text('keep existing binary')
        self.fixture()
        (self.root / 'SHA256SUMS').write_text('0' * 64 + '  kiro-agents-v1.2.3-x86_64-unknown-linux-gnu.tar.gz\n')
        self.run_install('--version', '1.2.3', success=False)
        self.assertEqual(binary.read_text(), 'keep existing binary')
        self.fixture(identity='9.9.9')
        self.run_install('--version', '1.2.3', success=False)
        self.assertEqual(binary.read_text(), 'keep existing binary')

    def test_pending_release_assets_are_retried_before_installation(self):
        self.fixture()
        self.env['INSTALL_PENDING_COUNT'] = '2'
        result = self.run_install()
        self.assertIn('waiting up to three minutes', result.stderr)
        self.assertTrue((self.destination / 'kiro-agents').exists())
        self.assertEqual((self.root / 'pending-count').read_text().strip(), '2')

    def test_missing_assets_eventually_fail_without_replacing_existing_binary(self):
        self.fixture()
        self.destination.mkdir()
        binary = self.destination / 'kiro-agents'
        binary.write_text('keep existing binary')
        self.env['INSTALL_PENDING_COUNT'] = '100'
        self.run_install(success=False)
        self.assertEqual(binary.read_text(), 'keep existing binary')
        self.assertEqual((self.root / 'pending-count').read_text().strip(), '19')

    def test_unsafe_archives_bad_versions_and_unsupported_hosts_are_rejected(self):
        for kind in ['traversal', 'symlink']:
            self.fixture(kind=kind)
            self.run_install('--version', '1.2.3', success=False)
            self.assertFalse(self.destination.exists())
        for version in ['../../bad', 'v1.2', 'v1.2.3;touch bad']:
            self.run_install('--version', version, success=False)
        self.env['INSTALL_ARCH'] = 'aarch64'
        self.run_install(success=False)
        self.env['INSTALL_ARCH'] = 'x86_64'
        self.mock('getconf', 'echo "glibc 2.31"')
        self.run_install(success=False)


if __name__ == '__main__':
    unittest.main()
