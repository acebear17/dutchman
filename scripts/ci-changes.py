#!/usr/bin/env python3
"""Select CI jobs conservatively from the entire PR or merge-group diff."""
import argparse
from collections import defaultdict
import re
import fnmatch
from pathlib import Path
import posixpath
import subprocess


ROOT = Path(__file__).resolve().parents[1]
DOC_FILES = frozenset(('AGENTS.md', 'CHANGELOG', 'CHANGELOG.md', 'CONTRIBUTING.md',
                       'CODE_OF_CONDUCT.md', 'SECURITY.md', 'LICENSE', 'NOTICE',
                       '.github/pull_request_template.md'))
DOC_TREES = ('docs/', 'skills/', 'changelog.d/')


def referenced_inputs(root=ROOT, extra=()):
    """Grep consumers at run time, expanding directory/glob and relative references.

    Include directories as well as literal files: package-docs.py walks docs/ and
    skills/, so all their members are inputs even without individually named paths.
    The changed paths are included to cover deleted files and renamed inputs.
    """
    tracked = subprocess.check_output(['git', '-C', str(root), 'ls-files', '-z'])
    candidates = set(tracked.decode('utf-8', errors='surrogateescape').split('\0')) | set(extra)
    candidates.discard('')
    scopes = ['crates', 'tests', 'build.rs', 'xtask', 'Cargo.toml',
              ':(glob)**/build.rs', ':(glob)scripts/package*', ':(glob)scripts/archive*',
              ':(glob)scripts/*msi*', ':(glob)scripts/*appimage*', ':(glob)scripts/*dmg*',
              '.github/workflows']
    found = subprocess.run(['git', '-C', str(root), 'grep', '-I', '-l', '-z',
                            '-e', '.', '--', *scopes], capture_output=True, check=False)
    if found.returncode not in (0, 1):
        raise OSError('Cannot scan CI build and test inputs')
    inputs = set()
    directories = defaultdict(set)
    for path in candidates:
        for parent in Path(path).parents:
            if str(parent) != '.':
                directories[str(parent)].add(path)
    seen = set()
    for name in found.stdout.decode('utf-8', errors='surrogateescape').split('\0'):
        if not name:
            continue
        text = (root / name).read_text(errors='replace')
        # Quoted paths include include_str!/include_bytes!, filesystem reads,
        # Path joins, shell copies and workflow arguments. Also find unquoted
        # paths in shell commands and comments; false positives keep coverage.
        tokens = re.findall(r"[\"']([^\"'\n]+)[\"']", text)
        tokens += re.findall(r'(?:[\w.*?{}-]+/)*[\w.*?{}-]+\.md', text)
        # Shell packaging commands commonly name directories without quotes.
        tokens += re.findall(r'[\w./{}-]+', text)
        mentioned_dirs = set()
        for token in tokens:
            for base in ('', str(Path(name).parent)):
                target = posixpath.normpath(posixpath.join(base, token))
                if target in directories:
                    mentioned_dirs.add(target)
        for token in tokens:
            if not token or token in ('.', '..', '/', '*', '**'):
                continue
            token = re.sub(r'\$\{[^}]+\}|\{[^}]+\}', '*', token)
            token = token.removeprefix('./')
            if token.strip('/') in ('*', '**'):
                continue
            bases = mentioned_dirs if '/' not in token and '*' in token and mentioned_dirs else ('', str(Path(name).parent))
            for base in bases:
                target = posixpath.normpath(posixpath.join(base, token))
                if target in seen:
                    continue
                seen.add(target)
                if target in candidates:
                    inputs.add(target)
                inputs.update(directories.get(target, ()))
                if any(char in target for char in '*?['):
                    inputs.update(path for path in candidates if fnmatch.fnmatchcase(path, target))
    return inputs


def docs_only_path(path, inputs):
    allowed = path in DOC_FILES or (path.endswith('.md') and path.startswith(DOC_TREES))
    return allowed and path not in inputs


def flags(paths, inputs=None):
    inputs = referenced_inputs(extra=paths) if inputs is None else inputs
    sources = [path for path in paths if not docs_only_path(path, inputs)]
    rust = re.compile(r'^(crates/|assets/|Cargo\.(toml|lock)$|clippy\.toml$|rust-toolchain|\.cargo/|\.github/workflows/ci\.yml$)')
    image = re.compile(r'^(crates/(dutchman|dutchman-core|dutchman-solidcraft|dutchman-freecad|dutchman-dxf|dutchman-launcher|dutchman-serve|dutchman-web)/|Cargo\.(toml|lock)$|\.cargo/|Dockerfile$|\.dockerignore$|docker/|\.github/workflows/(ci|placements|docker)\.yml$)')
    consumed = any(path in inputs for path in paths)
    return {
        'code': bool(sources),
        'rust': consumed or any(rust.search(path) for path in sources),
        'conversion': consumed or any(not path.startswith('crates/dutchman-gui/') or path.endswith(('Cargo.toml', 'Cargo.lock')) for path in sources),
        'docker': consumed or any(image.search(path) or path.endswith(('Cargo.toml', 'Cargo.lock')) for path in sources),
        'docs': consumed or any(path in ('README.md', 'docs/formats.md', 'docs/install.md') for path in paths),
    }


def select(base, event, ref):
    full = event in ('merge_group', 'workflow_dispatch') or ref.startswith('refs/tags/')
    if event == 'workflow_dispatch' or ref.startswith('refs/tags/'):
        return {'full': full, **dict.fromkeys(('code', 'rust', 'conversion', 'docker', 'docs'), True)}
    valid = subprocess.run(['git', 'cat-file', '-e', f'{base}^{{commit}}'], capture_output=True)
    if valid.returncode != 0:
        return {'full': full, **dict.fromkeys(('code', 'rust', 'conversion', 'docker', 'docs'), True)}
    diff = subprocess.check_output(['git', 'diff', '--name-only', '-z', base, 'HEAD'])
    paths = [path.decode('utf-8', errors='surrogateescape') for path in diff.split(b'\0') if path]
    try:
        inputs = referenced_inputs(extra=paths)
    except (OSError, subprocess.SubprocessError):
        return {'full': full, **dict.fromkeys(('code', 'rust', 'conversion', 'docker', 'docs'), True)}
    if event == 'merge_group':
        docs_only = all(docs_only_path(path, inputs) for path in paths)
        if not docs_only:
            return {'full': True, **dict.fromkeys(('code', 'rust', 'conversion', 'docker', 'docs'), True)}
        return {'full': True, 'code': False, 'rust': False, 'conversion': False,
                'docker': False, 'docs': any(path in ('README.md', 'docs/formats.md') for path in paths)}
    return {'full': full, **flags(paths, inputs)}


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
