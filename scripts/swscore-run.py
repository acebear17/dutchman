#!/usr/bin/env python3
"""Prepare and score released CC0 SolidWorks fixtures for swscore.yml."""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
import urllib.request

# Reuse the public manifest validation, bounded verified downloads, and process-group timeout.
SPEC = importlib.util.spec_from_file_location('fixture_score', Path(__file__).with_name('fixture-score.py'))
fixtures = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(fixtures)
SHARDS = 16
MANIFEST = 'https://progress.dutchmancad.xyz/scoreboard/public-files.json'


def prepare(out):
    out.mkdir(parents=True, exist_ok=True)
    with urllib.request.urlopen(MANIFEST, timeout=60) as response:
        data = response.read(16 * 1024 * 1024 + 1)
    if len(data) > 16 * 1024 * 1024:
        raise ValueError('Manifest exceeds 16 MiB')
    # Validate before selecting; never retain contributor metadata or the original manifest.
    with tempfile.TemporaryDirectory() as temporary:
        source = Path(temporary) / 'manifest.json'
        source.write_bytes(data)
        rows = fixtures.manifest_rows(source)
    rows = [{k: row[k] for k in ('file', 'url', 'sha256', 'bytes', 'consent', 'app')}
            for row in rows if row.get('app', '').casefold() == 'solidworks'
            and Path(row['file']).suffix.lower() in {'.sldprt', '.sldasm'}
            and row.get('consent') == 'cc0']
    if not rows:
        raise ValueError('Public manifest has no CC0 SolidWorks parts or assemblies')
    rows.sort(key=lambda row: row['file'])
    fixtures.write_json(out / 'manifest.json', dict(files=rows, shards=SHARDS))
    with Path(os.environ['GITHUB_OUTPUT']).open('a') as output:
        output.write('matrix=' + json.dumps(dict(variant=['pin', 'main'], shard=list(range(SHARDS)))) + '\n')


def invoke(command, stdout, stderr):
    try:
        return fixtures.run(command, stdout, stderr, os.environ.copy(), time.monotonic() + 900)
    except subprocess.TimeoutExpired:
        return 124


def one(row, binary, cli, out, stem):
    result = dict(file=row['file'], sha256=row['sha256'], download_ok=False,
                  convert_exit=None, reopen_exit=None, report=None, complete=False)
    record = out / (stem + '-result.json')
    fixtures.write_json(record, result)  # Preserve evidence if the runner itself times out.
    try:
        if not binary.is_file():
            raise ValueError('Dutchman build unavailable; see the build artifact and log')
        if cli is None or not cli.is_file():
            raise ValueError('SolidCraft unavailable; see solidcraft-fetch.log')
        with tempfile.TemporaryDirectory(prefix='swscore-') as temporary:
            scratch = Path(temporary)
            source = scratch / Path(row['file']).name
            design = scratch / 'converted.sc'
            fixtures.download(row, source)
            result['download_ok'] = True
            with (out / (stem + '.json')).open('w') as report, (out / (stem + '-convert.log')).open('w') as log:
                result['convert_exit'] = invoke([str(binary), 'convert', str(source), '-o', str(design),
                                                 '--json', '--quiet', '--yes'], report, log)
            result['report'] = fixtures.read_json(out / (stem + '.json'), None)
            fixtures.write_json(record, result)
            if design.is_file():
                roll = scratch / 'roll.json'
                fixtures.write_json(roll, {'commands': [{'command': 'timeline.roll_to', 'params': {'position': 'end'}}]})
                with (out / (stem + '-open.log')).open('w') as log:
                    result['reopen_exit'] = invoke([str(cli), 'run', str(roll), '--in', str(design), '--results',
                                                    '--json-out', str(out / (stem + '-open.json'))], log, subprocess.STDOUT)
    except (OSError, ValueError) as error:
        result['error'] = str(error)
        (out / (stem + '-error.log')).write_text(str(error) + '\n')
    result['complete'] = True
    fixtures.write_json(record, result)
    return result


def score(args):
    if not 0 <= args.shard < SHARDS:
        raise ValueError('Shard must be between 0 and 15')
    rows = fixtures.manifest_rows(args.manifest)
    if any(row['consent'] != 'cc0' or row.get('app', '').casefold() != 'solidworks'
           or Path(row['file']).suffix.lower() not in {'.sldprt', '.sldasm'} for row in rows):
        raise ValueError('Only released CC0 SolidWorks parts and assemblies may be scored')
    args.out.mkdir(parents=True, exist_ok=True)
    binary = args.binary.resolve()
    if binary.is_file():
        binary.chmod(binary.stat().st_mode | 0o111)
    cli = Path(os.environ['SOLIDCRAFT_CLI']).resolve() if os.environ.get('SOLIDCRAFT_CLI') else None
    fixtures.write_json(args.out / 'shard.json', dict(variant=args.variant, shard=args.shard,
                        files=len(rows[args.shard::SHARDS]), solidcraft=os.environ.get('SOLIDCRAFT_VERSION')))
    for index in range(args.shard, len(rows), SHARDS):
        one(rows[index], binary, cli, args.out, f'{index:06d}')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    prep = sub.add_parser('prepare')
    prep.add_argument('--out', type=Path, required=True)
    part = sub.add_parser('score')
    part.add_argument('--manifest', type=Path, required=True)
    part.add_argument('--variant', choices=['pin', 'main'], required=True)
    part.add_argument('--shard', type=int, required=True)
    part.add_argument('--binary', type=Path, required=True)
    part.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    if args.command == 'prepare':
        prepare(args.out)
    else:
        score(args)


if __name__ == '__main__':
    main()
