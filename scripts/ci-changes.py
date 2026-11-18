#!/usr/bin/env python3
"""Select CI jobs conservatively from the entire PR or merge-group diff."""
import argparse
import re
import subprocess


def flags(paths):
    docs = re.compile(r'(\.md$|^docs/|^assets/|^changelog\.d/|^\.github/ISSUE_TEMPLATE/|\.(png|jpe?g|svg|webp)$)')
    rust = re.compile(r'^(crates/|Cargo\.(toml|lock)$|clippy\.toml$|rust-toolchain|\.cargo/|\.github/workflows/ci\.yml$)')
    image = re.compile(r'^(crates/(dutchman|dutchman-core|dutchman-solidcraft|dutchman-freecad|dutchman-dxf|dutchman-launcher|dutchman-serve|dutchman-web)/|Cargo\.(toml|lock)$|\.cargo/|Dockerfile$|\.dockerignore$|docker/|\.github/workflows/(ci|placements|docker)\.yml$)')
    sources = [path for path in paths if not docs.search(path)]
    return {
        'code': bool(sources),
        'rust': any(rust.search(path) for path in sources),
        'docker': any(image.search(path) for path in sources),
        'docs': any(path in ('README.md', 'docs/formats.md') for path in paths),
    }


def select(base, event, ref):
    full = event in ('merge_group', 'workflow_dispatch') or ref.startswith('refs/tags/')
    if event == 'workflow_dispatch' or ref.startswith('refs/tags/'):
        return {'full': full, **dict.fromkeys(('code', 'rust', 'docker', 'docs'), True)}
    valid = subprocess.run(['git', 'cat-file', '-e', f'{base}^{{commit}}'], capture_output=True)
    if valid.returncode != 0:
        return {'full': full, **dict.fromkeys(('code', 'rust', 'docker', 'docs'), True)}
    diff = subprocess.check_output(['git', 'diff', '--name-only', '-z', base, 'HEAD'])
    paths = [path.decode('utf-8', errors='surrogateescape') for path in diff.split(b'\0') if path]
    return {'full': full, **flags(paths)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--base', required=True)
    parser.add_argument('--event', required=True)
    parser.add_argument('--ref', required=True)
    args = parser.parse_args()
    for key, value in select(args.base, args.event, args.ref).items():
        print(f'{key}={str(value).lower()}')


if __name__ == '__main__':
    main()
