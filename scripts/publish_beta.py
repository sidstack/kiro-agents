#!/usr/bin/env python3
"""Publish checked branch artifacts using trusted default-branch code only."""
import argparse
import hashlib
import io
import json
import os
from pathlib import Path
import re
import subprocess
import tarfile
import tempfile
import zipfile

MAX_ZIP = 25 * 1024 * 1024
MAX_BINARY = 100 * 1024 * 1024


def api(path):
    return json.loads(subprocess.check_output(['gh', 'api', path]))


def validate_run(run, repo):
    if (run['repository']['full_name'] != repo
            or run['head_repository']['full_name'] != repo
            or run['path'] != '.github/workflows/beta.yml'
            or run['event'] not in ('push', 'workflow_dispatch')
            or run['conclusion'] != 'success'
            or run['head_branch'] == 'main'
            or not re.fullmatch(r'[0-9a-f]{40}', run['head_sha'])):
        raise ValueError('Run is not a successful same-repository branch beta')


def validate_payload(payload, run):
    if len(payload) > MAX_ZIP:
        raise ValueError('Artifact exceeds size limit')
    with zipfile.ZipFile(io.BytesIO(payload)) as bundle:
        members = bundle.infolist()
        if len(members) != 3 or sum(m.file_size for m in members) > MAX_ZIP:
            raise ValueError('Unexpected artifact contents or size')
        if len({m.filename for m in members}) != 3:
            raise ValueError('Duplicate artifact members')
        files = {m.filename: bundle.read(m) for m in members}
    metadata = json.loads(files['beta.json'])
    version = metadata['version']
    suffix = f"-beta.{run['run_number']}.{run['run_attempt']}"
    if (not re.fullmatch(r'(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)-beta\.[1-9]\d*\.[1-9]\d*', version)
            or not version.endswith(suffix)
            or metadata['tag'] != 'v' + version or metadata['sha'] != run['head_sha']):
        raise ValueError('Beta identity does not match workflow run')
    name = f'kiro-agents-v{version}-x86_64-unknown-linux-gnu'
    archive = name + '.tar.gz'
    if metadata['archive'] != archive or set(files) != {'beta.json', 'SHA256SUMS', archive}:
        raise ValueError('Unexpected beta filenames')
    expected = hashlib.sha256(files[archive]).hexdigest() + '  ' + archive + '\n'
    if files['SHA256SUMS'].decode('ascii') != expected:
        raise ValueError('Beta checksum mismatch')
    allowed = {f'{name}/{n}' for n in ['kiro-agents', 'LICENSE', 'README.md', 'CHANGELOG.md']}
    with tarfile.open(fileobj=io.BytesIO(files[archive]), mode='r:gz') as tar:
        members = []
        for member in tar:
            members.append(member)
            if (len(members) > 4 or member.name not in allowed
                    or not member.isfile() or member.size > MAX_BINARY
                    or member.mode != (0o755 if member.name.endswith('/kiro-agents') else 0o644)):
                raise ValueError('Unsafe or unexpected archive member')
        if len(members) != 4 or {m.name for m in members} != allowed:
            raise ValueError('Incomplete or duplicate archive members')
    return metadata, files


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--run-id', required=True, type=int)
    args = parser.parse_args()
    if args.run_id <= 0:
        parser.error('run-id must be positive')
    repo = os.environ['GH_REPO']
    if not re.fullmatch(r'[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+', repo):
        raise ValueError('Invalid repository')
    run = api(f'repos/{repo}/actions/runs/{args.run_id}')
    validate_run(run, repo)
    artifacts = api(f'repos/{repo}/actions/runs/{args.run_id}/artifacts')['artifacts']
    artifacts = [a for a in artifacts if a['name'] == 'beta-linux' and not a['expired']]
    if len(artifacts) != 1 or artifacts[0]['size_in_bytes'] > MAX_ZIP:
        raise ValueError('Missing, duplicate or oversized beta artifact')
    artifact_id = int(artifacts[0]['id'])
    payload = subprocess.check_output(['gh', 'api', f'repos/{repo}/actions/artifacts/{artifact_id}/zip'])
    metadata, files = validate_payload(payload, run)
    tag = metadata['tag']
    refs = api(f'repos/{repo}/git/matching-refs/tags/{tag}')
    refs = [ref for ref in refs if ref['ref'] == f'refs/tags/{tag}']
    if refs and (refs[0]['object']['type'] != 'commit' or refs[0]['object']['sha'] != run['head_sha']):
        raise ValueError('Existing beta tag points to a different source')
    existing = subprocess.run(['gh', 'release', 'view', tag, '--json', 'isPrerelease'], capture_output=True, text=True)
    if existing.returncode == 0:
        ref = api(f'repos/{repo}/git/ref/tags/{tag}')['object']
        if ref['type'] != 'commit' or ref['sha'] != run['head_sha'] or not json.loads(existing.stdout)['isPrerelease']:
            raise ValueError('Existing release has a different source or is stable')
    with tempfile.TemporaryDirectory(prefix='kiro-beta-publish-') as tmp:
        root = Path(tmp)
        for name, data in files.items():
            (root / name).write_bytes(data)
        notes = root / 'notes.md'
        notes.write_text(f"Branch beta for `{run['head_branch']}` at `{run['head_sha']}`.\n\n"
                         f"[Passed integration and security checks]({run['html_url']}). "
                         "The extracted optimized binary passed all fake-Kiro PTY scenarios. "
                         "This prerelease does not replace the latest stable release.\n\n"
                         "Verify SHA256SUMS before installation. Kiro CLI is required separately.\n")
        if existing.returncode != 0:
            subprocess.run(['gh', 'release', 'create', tag, '--target', run['head_sha'],
                            '--prerelease', '--latest=false', '--title', tag,
                            '--notes-file', str(notes)], check=True)
        subprocess.run(['gh', 'release', 'upload', tag, str(root / metadata['archive']),
                        str(root / 'SHA256SUMS'), str(root / 'beta.json'), '--clobber'], check=True)


if __name__ == '__main__':
    main()
