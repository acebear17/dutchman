#!/usr/bin/env python3
"""Summarize swscore artifacts without mistaking missing evidence for a closed gap."""
import argparse
from collections import defaultdict
import json
import os
from pathlib import Path
import re

VARIANTS = ('pin', 'main')
DOWNSTREAM = re.compile(r'after the first blocking|earlier feature .* blocked', re.I)
UPSTREAM = re.compile(
    r'cadmpeg|\bnative\b|unresolved|not resolved|does not resolve|did not resolve|'
    r'missing|no resolved|not bound|does not bind|not supplied', re.I)
MAPPING = re.compile(r'not mapped|not supported|not implemented|mapping|solidcraft|requires|needs', re.I)


def load(path, default=None):
    try:
        return json.loads(path.read_text())
    except (OSError, ValueError):
        return default


def user_features(report):
    history = report.get('history') or {}
    return [feature for feature in history.get('features', [])
            if feature.get('status') not in {'not_needed', 'left_out'}]


def first_blocker(features):
    for feature in features:
        reason = feature.get('reason') or ''
        if feature.get('status') == 'mapped' or DOWNSTREAM.search(reason):
            continue
        kind = feature.get('kind', 'unknown')
        if kind == 'native':
            prefix = re.match(r'\D+', feature.get('name') or '')
            kind = prefix.group().strip() if prefix else 'native'
        return dict(kind=kind, reason=reason, name=feature.get('name'),
                    ordinal=feature.get('ordinal'), native=feature.get('kind') == 'native',
                    upstream=feature.get('upstream'))
    return None


def owner(blocker):
    # The report has no universal owner field. Be conservative about native/missing data;
    # classify explicit target mapping limitations only after excluding those upstream clues.
    reason = blocker['reason']
    if blocker['native'] or blocker.get('upstream') == 'cadmpeg' or UPSTREAM.search(reason):
        return 'cadmpeg'
    return 'Dutchman mapping still lacks' if MAPPING.search(reason) else 'cadmpeg'


def file_result(record):
    report = record.get('report') or {}
    features = user_features(report)
    history = report.get('history') or {}
    converted = record.get('convert_exit') == 0 and report.get('ok') is True
    return dict(conversion_ok=converted, reopen_clean=record.get('reopen_exit') == 0,
                complete=record.get('complete', False), error=record.get('error'),
                convert_exit=record.get('convert_exit'), reopen_exit=record.get('reopen_exit'),
                user_features=len(features), mapped=sum(f.get('status') == 'mapped' for f in features),
                rebuilt=history.get('rebuilt', 0),
                all_user_features_mapped=converted and bool(features) and all(f.get('status') == 'mapped' for f in features),
                history_available=isinstance(history.get('features'), list),
                first_blocker=first_blocker(features))


def summarize(root):
    manifest = load(root / 'swscore-fixtures' / 'manifest.json', {})
    rows = manifest.get('files', [])
    expected = {row['file'] for row in rows}
    variants = {}
    for variant in VARIANTS:
        build = load(root / f'swscore-build-{variant}' / 'build.json', dict(ok=False, compile='missing', update='missing'))
        files = {}
        for path in sorted(root.glob(f'swscore-{variant}-*/*-result.json')):
            record = load(path, {})
            key = record.get('file')
            if key in expected:
                if key in files:
                    raise ValueError(f'Duplicate result for {variant}: {key}')
                files[key] = file_result(record)
        counts = dict(files=len(expected), results=len(files), missing_results=len(expected - files.keys()),
                      conversions_ok=0, user_features=0, mapped=0, rebuilt=0,
                      files_all_user_features_mapped=0, files_reopen_clean=0,
                      files_without_user_features=0, incomplete_results=0)
        for result in files.values():
            for field in ('user_features', 'mapped', 'rebuilt'):
                counts[field] += result[field]
            counts['conversions_ok'] += result['conversion_ok']
            counts['files_all_user_features_mapped'] += result['all_user_features_mapped']
            counts['files_reopen_clean'] += result['reopen_clean']
            counts['files_without_user_features'] += result['history_available'] and result['user_features'] == 0
            counts['incomplete_results'] += not result['complete']
        variants[variant] = dict(build=build, counts=counts,
                                 files={key: files.get(key) for key in sorted(expected)})

    groups = defaultdict(list)
    uncomparable = []

    def add(category, blocker, key):
        groups[(category, blocker['kind'], blocker['reason'])].append(key)

    for key in sorted(expected):
        pin, main = (variants[v]['files'][key] for v in VARIANTS)
        if not all(variants[v]['build'].get('ok') for v in VARIANTS):
            uncomparable.append(dict(file=key, reason='One or both builds unavailable'))
            continue
        if not all(r and r['complete'] and r['conversion_ok'] and r['history_available'] for r in (pin, main)):
            uncomparable.append(dict(file=key, reason='Missing, incomplete, failed conversion, or unavailable history'))
            continue
        if pin['user_features'] and not main['user_features']:
            uncomparable.append(dict(file=key, reason='User history present on pin but absent on main'))
            continue
        before, after = pin['first_blocker'], main['first_blocker']
        signature = lambda blocker: (blocker['kind'], blocker['reason']) if blocker else None
        if before and signature(before) == signature(after):
            add(owner(after), after, key)
        else:
            if before:
                add('closed by the pin move', before, key)
            if after:
                add('new on main (regressions)', after, key)
    gaps = [dict(category=category, kind=kind, reason=reason, count=len(files), files=files)
            for (category, kind, reason), files in sorted(groups.items())]
    return dict(manifest_available=bool(rows), variants=variants, gaps=gaps, uncomparable=uncomparable)


def cell(value):
    return str(value if value is not None else '—').replace('&', '&amp;').replace('<', '&lt;').replace('>', '&gt;').replace('|', '&#124;').replace('\r', ' ').replace('\n', ' ')


def render(summary):
    lines = ['# SolidWorks pin gap', '']
    if not summary['manifest_available']:
        lines += ['Manifest unavailable: no fixture counts or gap conclusions can be drawn.', '']
    for variant, data in summary['variants'].items():
        build = data['build']
        status = 'compiled' if build.get('ok') else 'unavailable; compilation/API drift or setup failure (see build logs)'
        lines += [f"- **{variant}**: {status}. Update: {cell(build.get('update'))}; compile: {cell(build.get('compile'))}; cadmpeg: {cell(build.get('cadmpeg'))}."]
    lines += ['', '| Count | pin | main |', '|---|---:|---:|']
    fields = [('files', 'Files in manifest'), ('results', 'Files with results'), ('conversions_ok', 'Conversions ok'),
              ('user_features', 'User timeline features'), ('mapped', 'User features mapped'), ('rebuilt', 'Rebuilt (history.rebuilt)'),
              ('files_all_user_features_mapped', 'Files with every user feature mapped'), ('files_reopen_clean', 'Files reopening clean'),
              ('files_without_user_features', 'Files with no user features'), ('missing_results', 'Missing results'),
              ('incomplete_results', 'Incomplete results')]
    for field, label in fields:
        lines.append(f"| {label} | {summary['variants']['pin']['counts'][field]} | {summary['variants']['main']['counts'][field]} |")
    lines += ['', 'User features exclude not_needed and left_out. Full mapping requires at least one user feature and a successful conversion. Rebuilt uses the report count, which can differ from mapped (for example, reapplied features).',
              '', '## Gap table', '',
              'Compared only where both builds and conversions succeeded and history is available. A closed first-blocker signature no longer leads that file; another blocker may now lead it. New signatures are regression candidates and can also be newly exposed later blockers. Missing evidence never closes a gap.',
              '', 'Ownership is a reason-based heuristic: native, unresolved, missing-data or explicit cadmpeg reasons remain cadmpeg; other explicit mapping/SolidCraft limitations are labelled Dutchman mapping still lacks. Unclassified reasons remain cadmpeg for triage.',
              '', '| Classification | Kind | Reason | Files |', '|---|---|---|---:|']
    for gap in summary['gaps']:
        lines.append(f"| {cell(gap['category'])} | {cell(gap['kind'])} | {cell(gap['reason'])} | {gap['count']} |")
    if not summary['gaps']:
        lines.append('| No comparable blockers | — | — | 0 |')
    lines += ['', f"Files without comparable evidence: {len(summary['uncomparable'])}.", '', '## First blocker per file', '',
              '| File | pin | main |', '|---|---|---|']

    def describe(result):
        if result is None:
            return 'Missing result'
        blocker = result['first_blocker']
        if blocker:
            return f"{blocker['kind']}: {blocker['reason']}"
        if not result['conversion_ok']:
            return result['error'] or f"Conversion failed/unavailable (exit {result['convert_exit']})"
        if not result['history_available'] or not result['user_features']:
            return 'No user history reported'
        if result['mapped'] != result['user_features']:
            return 'Only downstream blockers reported'
        return 'Every user feature mapped'

    for key in summary['variants']['pin']['files']:
        lines.append('| ' + ' | '.join([cell(key)] + [cell(describe(summary['variants'][v]['files'][key])) for v in VARIANTS]) + ' |')
    return '\n'.join(lines) + '\n'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('artifacts', type=Path)
    parser.add_argument('--out', type=Path, default=Path('.'))
    args = parser.parse_args()
    summary = summarize(args.artifacts)
    markdown = render(summary)
    args.out.mkdir(parents=True, exist_ok=True)
    (args.out / 'swscore-summary.json').write_text(json.dumps(summary, indent=2) + '\n')
    (args.out / 'swscore-summary.md').write_text(markdown)
    if os.environ.get('GITHUB_STEP_SUMMARY'):
        with Path(os.environ['GITHUB_STEP_SUMMARY']).open('a') as output:
            output.write(markdown)


if __name__ == '__main__':
    main()
