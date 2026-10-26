#!/usr/bin/env python3
"""Build a version-checked Linux release archive; never publishes anything."""
import argparse
import gzip
import hashlib
import io
import json
import os
from pathlib import Path
import platform
import re
import subprocess
import tarfile

ROOT = Path(__file__).resolve().parents[1]
TARGET = 'x86_64-unknown-linux-gnu'
VERSION_RE = r'\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?'
FULL_SHA_RE = r'[0-9a-f]{40}'


def validate_tag(tag, version):
    if not re.fullmatch(VERSION_RE, version):
        raise ValueError('Invalid package version')
    if tag != 'v' + version:
        raise ValueError(f'Tag {tag!r} does not match Cargo version v{version}')


def validate_arguments(args):
    if args.beta:
        if args.tag:
            raise ValueError('--beta derives its own tag; do not pass --tag')
        if args.sequence is None or args.attempt is None or not args.source_sha:
            raise ValueError('--beta requires --sequence, --attempt and --source-sha')
        if not re.fullmatch(FULL_SHA_RE, args.source_sha):
            raise ValueError('--source-sha must be a full 40 character lowercase commit sha')
    elif args.tag is None:
        raise ValueError('--tag is required for stable releases')
    elif args.sequence is not None or args.attempt is not None or args.source_sha:
        raise ValueError('--sequence, --attempt and --source-sha are only valid with --beta')


def beta_version(source_version, sequence, attempt):
    """Next patch version with a beta prerelease stamped from the workflow run."""
    if not re.fullmatch(r'\d+\.\d+\.\d+', source_version):
        raise ValueError(f'Beta builds need a plain stable Cargo version, got {source_version!r}')
    if sequence <= 0 or attempt <= 0:
        raise ValueError('Beta sequence and attempt must be positive integers')
    major, minor, patch = (int(part) for part in source_version.split('.'))
    return f'{major}.{minor}.{patch + 1}-beta.{sequence}.{attempt}'


def stamp_versions(root, version, package_name):
    """Rewrite only the Cargo.toml package version and the root Cargo.lock entry."""
    if not re.fullmatch(VERSION_RE, version):
        raise ValueError('Invalid version to stamp')
    manifest = root / 'Cargo.toml'
    lines = manifest.read_text().splitlines(keepends=True)
    section = None
    stamped = 0
    for index, line in enumerate(lines):
        stripped = line.strip()
        if stripped.startswith('['):
            section = stripped
        elif section == '[package]' and stripped.startswith('version'):
            lines[index] = f'version = "{version}"\n'
            stamped += 1
            section = None
    if stamped != 1:
        raise ValueError('Expected exactly one version key in the Cargo.toml package section')
    manifest.write_text(''.join(lines))

    lock = root / 'Cargo.lock'
    lines = lock.read_text().splitlines(keepends=True)
    blocks, start = [], None
    for index, line in enumerate(lines):
        if line.strip() == '[[package]]':
            if start is not None:
                blocks.append((start, index))
            start = index
    if start is not None:
        blocks.append((start, len(lines)))
    stamped = 0
    for begin, end in blocks:
        if any(line.strip() == f'name = "{package_name}"' for line in lines[begin:end]):
            for index in range(begin, end):
                if lines[index].strip().startswith('version'):
                    lines[index] = f'version = "{version}"\n'
                    stamped += 1
    if stamped != 1:
        raise ValueError(f'Expected exactly one {package_name} package entry in Cargo.lock')
    lock.write_text(''.join(lines))


def require_clean_tracked_tree(root):
    status = subprocess.check_output(
        ['git', 'status', '--porcelain', '--untracked-files=no'], cwd=root, text=True)
    if status.strip():
        raise ValueError('Beta builds require a clean tracked working tree')


def require_source_sha(root, sha):
    if not re.fullmatch(FULL_SHA_RE, sha):
        raise ValueError('--source-sha must be a full 40 character lowercase commit sha')
    head = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=root, text=True).strip()
    if head != sha:
        raise ValueError(f'HEAD {head} does not match --source-sha {sha}')


def write_beta_metadata(output, version, tag, sha, archive):
    payload = {'version': version, 'tag': tag, 'sha': sha, 'archive': archive.name}
    (output / 'beta.json').write_text(json.dumps(payload, indent=2, sort_keys=True) + '\n')


def create_archive(root, binary, version, output):
    validate_tag('v' + version, version)
    if not binary.is_file() or binary.is_symlink():
        raise ValueError('Release binary must be a regular file')
    output.mkdir(parents=True, exist_ok=True)
    name = f'kiro-agents-v{version}-{TARGET}'
    archive = output / (name + '.tar.gz')
    files = [('kiro-agents', binary, 0o755)]
    files.extend((doc, root / doc, 0o644) for doc in ['LICENSE', 'README.md', 'CHANGELOG.md'])
    # Fixed tar and gzip headers make the archive deterministic for the same inputs.
    with archive.open('wb') as raw:
        with gzip.GzipFile(fileobj=raw, mode='wb', filename='', mtime=0) as compressed:
            with tarfile.open(fileobj=compressed, mode='w') as tar:
                for label, source, mode in files:
                    data = source.read_bytes()
                    info = tarfile.TarInfo(f'{name}/{label}')
                    info.size, info.mode, info.mtime = len(data), mode, 0
                    tar.addfile(info, io.BytesIO(data))
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    (output / 'SHA256SUMS').write_text(f'{digest}  {archive.name}\n')
    return archive


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--tag')
    parser.add_argument('--out-dir', type=Path, default=ROOT / 'dist')
    parser.add_argument('--beta', action='store_true', help='Build an unpublished branch beta')
    parser.add_argument('--sequence', type=int)
    parser.add_argument('--attempt', type=int)
    parser.add_argument('--source-sha')
    args = parser.parse_args()
    try:
        validate_arguments(args)
    except ValueError as error:
        parser.error(str(error))
    if platform.system() != 'Linux' or platform.machine() != 'x86_64':
        parser.error('Only native Linux x86_64 release builds are supported')
    metadata = json.loads(subprocess.check_output(
        ['cargo', 'metadata', '--locked', '--offline', '--no-deps', '--format-version', '1'], cwd=ROOT))
    package = next(p for p in metadata['packages'] if Path(p['manifest_path']) == ROOT / 'Cargo.toml')
    version = package['version']
    if args.beta:
        require_clean_tracked_tree(ROOT)
        require_source_sha(ROOT, args.source_sha)
        version = beta_version(version, args.sequence, args.attempt)
        tag = 'v' + version
        stamp_versions(ROOT, version, package['name'])
        stamped = json.loads(subprocess.check_output(
            ['cargo', 'metadata', '--locked', '--offline', '--no-deps', '--format-version', '1'], cwd=ROOT))
        if next(p for p in stamped['packages'] if Path(p['manifest_path']) == ROOT / 'Cargo.toml')['version'] != version:
            raise ValueError('Stamped Cargo metadata does not match the beta version')
    else:
        tag = args.tag
        validate_tag(tag, version)
        # A workflow must build a tag, never a branch with a coincidentally equal version.
        tag_commit = subprocess.check_output(['git', 'rev-parse', f'refs/tags/{tag}^{{commit}}'], cwd=ROOT).strip()
        head = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT).strip()
        if tag_commit != head:
            raise ValueError('HEAD is not the requested release tag')
    env = dict(os.environ)
    # Use a known target even if a contributor has a different default build target.
    subprocess.run(['cargo', 'build', '--release', '--locked', '--target', TARGET], cwd=ROOT, env=env, check=True)
    binary = Path(metadata['target_directory']) / TARGET / 'release' / 'kiro-agents'
    actual = subprocess.check_output([str(binary), '--version'], text=True).strip()
    if actual != f'kiro-agents {version}':
        raise ValueError(f'Built binary identity mismatch: {actual}')
    archive = create_archive(ROOT, binary, version, args.out_dir)
    if args.beta:
        write_beta_metadata(args.out_dir, version, tag, args.source_sha, archive)
    print(archive)


if __name__ == '__main__':
    main()
