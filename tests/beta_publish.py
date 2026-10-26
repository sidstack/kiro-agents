#!/usr/bin/env python3
"""Reject mismatched/untrusted beta artifacts before privileged publication."""
import copy
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import tarfile
import unittest
import zipfile

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('publish_beta', ROOT / 'scripts/publish_beta.py')
publish = importlib.util.module_from_spec(spec)
spec.loader.exec_module(publish)
VERSION = '0.3.1-beta.42.1'
PREFIX = f'kiro-agents-v{VERSION}-x86_64-unknown-linux-gnu'
RUN = dict(repository={'full_name': 'sidstack/kiro-agents'},
           head_repository={'full_name': 'sidstack/kiro-agents'},
           path='.github/workflows/beta.yml', event='push', conclusion='success',
           head_branch='fix/dependencies', head_sha='a' * 40, run_number=42, run_attempt=1)


def fixture(update=None, bad_member=None):
    compressed = io.BytesIO()
    with tarfile.open(fileobj=compressed, mode='w:gz') as tar:
        for label in ['kiro-agents', 'LICENSE', 'README.md', 'CHANGELOG.md']:
            member = tarfile.TarInfo(f'{PREFIX}/{label}')
            member.size = 3
            member.mode = 0o755 if label == 'kiro-agents' else 0o644
            if bad_member and label == 'kiro-agents':
                bad_member(member)
            tar.addfile(member, io.BytesIO(b'abc'))
    archive = PREFIX + '.tar.gz'
    data = compressed.getvalue()
    metadata = dict(version=VERSION, tag='v' + VERSION, sha='a' * 40, archive=archive)
    if update:
        metadata.update(update)
    files = {archive: data, 'beta.json': json.dumps(metadata).encode(),
             'SHA256SUMS': (hashlib.sha256(data).hexdigest() + '  ' + archive + '\n').encode()}
    return files


def bundle(files):
    output = io.BytesIO()
    with zipfile.ZipFile(output, 'w') as archive:
        for name, data in files.items():
            archive.writestr(name, data)
    return output.getvalue()


class PublicationBoundary(unittest.TestCase):
    def test_checked_same_repository_beta_is_accepted(self):
        publish.validate_run(RUN, 'sidstack/kiro-agents')
        metadata, files = publish.validate_payload(bundle(fixture()), RUN)
        self.assertEqual(metadata['version'], VERSION)
        self.assertEqual(len(files), 3)

    def test_fork_failed_main_and_other_workflows_cannot_publish(self):
        for update in [dict(head_repository={'full_name': 'fork/repo'}),
                       dict(head_branch='main'), dict(conclusion='failure'),
                       dict(event='pull_request'), dict(path='.github/workflows/checks.yml')]:
            run = copy.deepcopy(RUN)
            run.update(update)
            with self.subTest(update=update), self.assertRaises(ValueError):
                publish.validate_run(run, 'sidstack/kiro-agents')

    def test_artifact_cannot_claim_other_commit_or_stable_version(self):
        for update in [dict(sha='b' * 40), dict(version='0.3.1'),
                       dict(tag='v0.3.1'), dict(version='0.3.1-beta.41.1'),
                       dict(archive='../escape.tar.gz')]:
            with self.subTest(update=update), self.assertRaises(ValueError):
                publish.validate_payload(bundle(fixture(update)), RUN)

    def test_other_run_or_rerun_cannot_reuse_a_beta_identity(self):
        for update in [dict(run_number=43), dict(run_attempt=2), dict(head_sha='b' * 40)]:
            run = dict(RUN, **update)
            with self.subTest(update=update), self.assertRaises(ValueError):
                publish.validate_payload(bundle(fixture()), run)

    def test_checksum_extra_files_and_archive_traversal_are_rejected(self):
        files = fixture()
        files['SHA256SUMS'] = b'wrong'
        with self.assertRaises(ValueError):
            publish.validate_payload(bundle(files), RUN)
        files = fixture()
        files['../../escape'] = b'wrong'
        with self.assertRaises(ValueError):
            publish.validate_payload(bundle(files), RUN)
        for change in [lambda m: setattr(m, 'name', '../../escape'),
                       lambda m: setattr(m, 'type', tarfile.SYMTYPE),
                       lambda m: setattr(m, 'mode', 0o777)]:
            with self.assertRaises(ValueError):
                publish.validate_payload(bundle(fixture(bad_member=change)), RUN)


if __name__ == '__main__':
    unittest.main()
