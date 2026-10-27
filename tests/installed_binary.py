#!/usr/bin/env python3
"""Run every PTY integration scenario against a packaged release/beta binary.

Takes the versioned archive produced by scripts/package_release.py, verifies the
adjacent SHA256SUMS entry, rejects unsafe or unexpected archive members and then
extracts it into a temporary directory. The extracted executable must report the
expected identity and pass all existing integration scripts. Only fake Kiro
and temporary state are reachable: HOME and KIRO_HOME point at a throwaway
directory so no real Kiro or application configuration is touched.
"""
import argparse
import hashlib
import os
from pathlib import Path, PurePosixPath
import shutil
import re
import subprocess
import sys
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parents[1]
TARGET = 'x86_64-unknown-linux-gnu'
EXECUTABLE = 'kiro-agents'
DOCS = ('LICENSE', 'README.md', 'CHANGELOG.md')
SCRIPTS = ('terminal_smoke.py', 'board_smoke.py', 'board_errors.py',
           'board_handoff.py', 'board_ack.py', 'board_picker.py',
           'board_search.py', 'board_naming.py', 'naming_upgrade.py',
           'frontend_lifecycle.py')
MODES = {EXECUTABLE: 0o755, **{doc: 0o644 for doc in DOCS}}
VERSION_RE = re.compile(r'(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)'
                        r'(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?\Z')
MAX_MEMBER = 64 * 1024 * 1024
MAX_TOTAL = 200 * 1024 * 1024
SCRIPT_TIMEOUT = 180
IDENTITY_TIMEOUT = 30


def archive_name(version):
    return f'kiro-agents-v{version}-{TARGET}.tar.gz'


def archive_prefix(version):
    return f'kiro-agents-v{version}-{TARGET}'


def validate_version(version):
    if not VERSION_RE.fullmatch(version or ''):
        raise ValueError(f'Invalid expected version {version!r}')
    return version


def verify_checksum(archive, expected_version):
    """Return the archive digest, or raise when SHA256SUMS disagrees."""
    archive = Path(archive)
    validate_version(expected_version)
    name = archive_name(expected_version)
    if archive.name != name:
        raise ValueError(f'Archive {archive.name!r} does not match version {expected_version}')
    sums = archive.parent / 'SHA256SUMS'
    if not sums.is_file():
        raise ValueError(f'Missing checksum file {sums}')
    entries = [line for line in sums.read_text().splitlines() if line.strip()]
    matching = []
    for line in entries:
        digest, _, label = line.partition('  ')
        if label == name:
            matching.append(digest)
    if len(matching) != 1:
        raise ValueError(f'SHA256SUMS holds {len(matching)} entries for {name}')
    if not re.fullmatch(r'[0-9a-f]{64}', matching[0]):
        raise ValueError('SHA256SUMS entry is not a lowercase SHA-256 digest')
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    if digest != matching[0]:
        raise ValueError(f'Checksum mismatch for {name}: recorded {matching[0]}, computed {digest}')
    return digest


def inspect_archive(archive, expected_version):
    """Return validated members for the versioned archive, or raise."""
    archive = Path(archive)
    prefix = archive_prefix(validate_version(expected_version)) + '/'
    allowed = {prefix + label for label in MODES}
    members = []
    seen = set()
    total = 0
    with tarfile.open(archive, mode='r:gz') as tar:
        for member in tar:
            path = PurePosixPath(member.name)
            if (not member.isfile() or member.issym() or member.islnk()
                    or path.is_absolute() or '..' in path.parts):
                raise ValueError(f'Unsafe or unexpected member {member.name!r}')
            if member.name not in allowed or member.name in seen:
                raise ValueError(f'Member {member.name!r} is outside the allowlist')
            seen.add(member.name)
            if member.size < 0 or member.size > MAX_MEMBER:
                raise ValueError(f'Member {member.name!r} has an invalid size')
            label = member.name[len(prefix):]
            if member.mode & 0o7777 != MODES[label]:
                raise ValueError(f'Member {label!r} mode is {oct(member.mode & 0o7777)}, '
                                 f'expected {oct(MODES[label])}')
            total += member.size
            if total > MAX_TOTAL:
                raise ValueError('Archive expands past the size limit')
            members.append(member)
    if len(members) != len(MODES) or {m.name for m in members} != allowed:
        raise ValueError('Archive does not hold exactly the four release files')
    return members


def extract(archive, destination, expected_version):
    """Write validated members under destination without path traversal."""
    destination = Path(destination)
    prefix = archive_prefix(validate_version(expected_version)) + '/'
    members = inspect_archive(archive, expected_version)
    destination.mkdir(parents=True, exist_ok=True)
    with tarfile.open(archive, mode='r:gz') as tar:
        for member in members:
            label = member.name[len(prefix):]
            target = destination / label
            source = tar.extractfile(member)
            if source is None:
                raise ValueError(f'Member {member.name!r} has no content')
            data = source.read()
            if len(data) != member.size:
                raise ValueError(f'Member {member.name!r} expanded past its declared size')
            target.write_bytes(data)
            target.chmod(member.mode & 0o7777)
    binary = destination / EXECUTABLE
    if not binary.is_file() or not os.access(binary, os.X_OK):
        raise ValueError(f'Extracted {EXECUTABLE} is not an executable regular file')
    return binary


def check_identity(binary, expected_version):
    version = subprocess.run([str(binary), '--version'], capture_output=True, text=True,
                             timeout=IDENTITY_TIMEOUT, check=True).stdout.strip()
    if version != f'{EXECUTABLE} {expected_version}':
        raise ValueError(f'Extracted binary identity mismatch: {version!r}')
    help_text = subprocess.run([str(binary), '--help'], capture_output=True, text=True,
                               timeout=IDENTITY_TIMEOUT, check=True).stdout
    if EXECUTABLE not in help_text:
        raise ValueError('Extracted binary --help output is unexpected')


def sandbox_env():
    """A throwaway home so the packaged binary can never reach real Kiro state."""
    home = Path(tempfile.mkdtemp(prefix='kiro-installed-home-'))
    env = dict(os.environ)
    env['HOME'] = str(home)
    env['KIRO_HOME'] = str(home / 'kiro')
    env.pop('KIRO_AGENTS_HOME', None)
    env.pop('KIRO_SESSIONS_DIR', None)
    return home, env


def run_scripts(binary, env):
    failed = []
    for script in SCRIPTS:
        command = [sys.executable, str(ROOT / 'tests' / script), str(binary)]
        result = subprocess.run(command, cwd=ROOT, env=env, timeout=SCRIPT_TIMEOUT,
                                capture_output=True, text=True)
        if result.returncode != 0:
            tail = (result.stdout + result.stderr).strip().splitlines()[-15:]
            failed.append(f'{script} exited {result.returncode}: ' + ' | '.join(tail))
    return failed


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('archive', type=Path)
    parser.add_argument('--expected-version', required=True)
    args = parser.parse_args()
    version = validate_version(args.expected_version)
    digest = verify_checksum(args.archive, version)
    print(f'Archive {args.archive.name} matches SHA-256 {digest}')
    home, env = sandbox_env()
    try:
        with tempfile.TemporaryDirectory(prefix='kiro-installed-binary-') as tmp:
            binary = extract(args.archive, Path(tmp), version)
            check_identity(binary, version)
            print(f'Extracted binary reports {EXECUTABLE} {version}')
            failed = run_scripts(binary, env)
            if failed:
                for line in failed:
                    print(line, file=sys.stderr)
                raise SystemExit(f'{len(failed)} of {len(SCRIPTS)} scenarios failed')
    finally:
        shutil.rmtree(home, ignore_errors=True)
    print(f'PASS: extracted {version} binary passed all {len(SCRIPTS)} PTY scenarios')


if __name__ == '__main__':
    main()
