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


def validate_tag(tag, version):
    if not re.fullmatch(r'\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?', version):
        raise ValueError('Invalid package version')
    if tag != 'v' + version:
        raise ValueError(f'Tag {tag!r} does not match Cargo version v{version}')


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
    parser.add_argument('--tag', required=True)
    parser.add_argument('--out-dir', type=Path, default=ROOT / 'dist')
    args = parser.parse_args()
    if platform.system() != 'Linux' or platform.machine() != 'x86_64':
        parser.error('Only native Linux x86_64 release builds are supported')
    metadata = json.loads(subprocess.check_output(
        ['cargo', 'metadata', '--locked', '--offline', '--no-deps', '--format-version', '1'], cwd=ROOT))
    package = next(p for p in metadata['packages'] if Path(p['manifest_path']) == ROOT / 'Cargo.toml')
    version = package['version']
    validate_tag(args.tag, version)
    # A workflow must build a tag, never a branch with a coincidentally equal version.
    tag_commit = subprocess.check_output(['git', 'rev-parse', f'refs/tags/{args.tag}^{{commit}}'], cwd=ROOT).strip()
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
    print(create_archive(ROOT, binary, version, args.out_dir))


if __name__ == '__main__':
    main()
