"""Synthetic checks for the SolidWorks pin comparison; no CAD tools or network."""
import argparse
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest import mock


def module(name, filename):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).resolve().parents[1] / filename)
    loaded = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(loaded)
    return loaded


summary = module('swscore_summary', 'swscore-summary.py')
runner = module('swscore_run', 'swscore-run.py')


def write(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value))


def feature(kind='extrude', status='mapped', reason=None, name='Extrude1', **extra):
    return dict(kind=kind, name=name, status=status, reason=reason, **extra)


def result(features, **extra):
    return dict(complete=True, convert_exit=0, reopen_exit=0,
                report=dict(ok=True, history=dict(features=features, rebuilt=2)), **extra)


class SwscoreTests(unittest.TestCase):
    def bundle(self, root, names):
        write(root / 'swscore-fixtures/manifest.json', dict(files=[dict(file=name) for name in names]))
        for variant in summary.VARIANTS:
            write(root / f'swscore-build-{variant}/build.json', dict(ok=True, compile='success'))

    def record(self, root, variant, name, value):
        write(root / f'swscore-{variant}-0/{name}-result.json', dict(value, file=name))

    def test_metrics_blockers_and_gap_categories(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.bundle(root, ['closed', 'mapping', 'upstream', 'regression', 'missing', 'no-history'])
            native = feature('native', 'blocked', 'native is not mapped yet', name='BossExtrude12')
            mapping = feature('thread', 'blocked', 'threads are not mapped yet')
            upstream = feature(status='blocked', reason='unresolved profiles are not mapped yet')
            ignored = [feature(status='not_needed'), feature(status='left_out')]
            downstream = feature(status='blocked', reason='after the first blocking feature')
            # A later independent sketch may be mapped after a blocker. Organizational
            # and suppressed rows and downstream reasons must not distort the first blocker.
            for variant in summary.VARIANTS:
                self.record(root, variant, 'mapping', result(ignored + [downstream, mapping, feature()]))
                self.record(root, variant, 'upstream', result([upstream]))
                self.record(root, variant, 'no-history', result([]))
            self.record(root, 'pin', 'closed', result([native]))
            self.record(root, 'main', 'closed', result([feature(), feature()]))
            self.record(root, 'pin', 'regression', result([feature()]))
            self.record(root, 'main', 'regression', result([mapping]))
            self.record(root, 'pin', 'missing', result([native]))
            data = summary.summarize(root)
            self.assertEqual({g['category'] for g in data['gaps']},
                             {'closed by the pin move', 'Dutchman mapping still lacks', 'cadmpeg', 'new on main (regressions)'})
            closed = next(g for g in data['gaps'] if g['category'] == 'closed by the pin move')
            self.assertEqual((closed['kind'], closed['files']), ('BossExtrude', ['closed']))
            pin, main = (data['variants'][v]['counts'] for v in summary.VARIANTS)
            self.assertEqual(pin['files'], 6)
            self.assertEqual(pin['user_features'], 7)
            self.assertEqual(pin['mapped'], 2)
            self.assertEqual(pin['rebuilt'], 12)
            self.assertEqual(main['missing_results'], 1)
            self.assertEqual(main['conversions_ok'], 5)
            self.assertEqual(main['files_all_user_features_mapped'], 1)
            self.assertEqual(main['files_reopen_clean'], 5)
            self.assertEqual(len(data['uncomparable']), 1)
            self.assertEqual(data['variants']['pin']['files']['mapping']['first_blocker']['kind'], 'thread')
            self.assertIn('BossExtrude: native is not mapped yet', summary.render(data))

    def test_failed_build_failed_conversion_and_lost_history_never_close_gaps(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.bundle(root, ['cube'])
            self.record(root, 'pin', 'cube', result([feature(status='blocked', reason='profiles are not mapped yet')]))
            self.record(root, 'main', 'cube', result([feature()]))
            write(root / 'swscore-build-main/build.json', dict(ok=False, compile='failure'))
            data = summary.summarize(root)
            self.assertEqual(data['gaps'], [])
            self.assertIn('compilation/API drift', summary.render(data))
            write(root / 'swscore-build-main/build.json', dict(ok=True, compile='success'))
            self.record(root, 'main', 'cube', dict(result([feature()]), convert_exit=124))
            self.assertEqual(summary.summarize(root)['gaps'], [])
            self.record(root, 'main', 'cube', result([]))
            self.assertEqual(summary.summarize(root)['gaps'], [])

    def test_changed_first_blocker_keeps_both_signatures_and_missing_manifest_is_visible(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.assertIn('Manifest unavailable', summary.render(summary.summarize(root)))
            self.bundle(root, ['cube'])
            self.record(root, 'pin', 'cube', result([feature('native', 'blocked', 'native is not mapped yet', name='Sketch1')]))
            self.record(root, 'main', 'cube', result([feature('sketch'), feature('fillet', 'blocked', 'fillets in this form are not mapped yet')]))
            data = summary.summarize(root)
            self.assertEqual({g['category'] for g in data['gaps']}, {'closed by the pin move', 'new on main (regressions)'})
            self.assertIn('newly exposed later blockers', summary.render(data))

    def test_downstream_regex_native_prefix_and_markdown_escape(self):
        rows = [feature(status='blocked', reason='earlier feature Extrude1 blocked this feature'),
                feature('native', 'blocked', 'not mapped | yet\nmore', name='Cut-Extrude42')]
        self.assertEqual(summary.first_blocker(rows)['kind'], 'Cut-Extrude')
        self.assertEqual(summary.cell('a|b\n<script>'), 'a&#124;b &lt;script&gt;')

    def test_summary_command_writes_json_markdown_and_job_summary(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.bundle(root, ['cube'])
            for variant in summary.VARIANTS:
                self.record(root, variant, 'cube', result([feature()]))
            env = dict(os.environ, GITHUB_STEP_SUMMARY=str(root / 'job-summary.md'))
            subprocess.run(['python3', summary.__file__, str(root), '--out', str(root / 'out')], check=True, env=env)
            self.assertEqual((root / 'job-summary.md').read_text(), (root / 'out/swscore-summary.md').read_text())
            self.assertEqual(json.loads((root / 'out/swscore-summary.json').read_text())['variants']['pin']['counts']['mapped'], 1)

    def row(self, file='cube.SLDPRT', **extra):
        return dict(dict(file=file, app='SolidWorks', consent='cc0', bytes=7,
                         sha256=hashlib.sha256(b'fixture').hexdigest(), url='https://example.org/cube'), **extra)

    def test_prepare_filters_cc0_solidworks_sorts_and_emits_32_jobs(self):
        rows = [self.row('z.SLDASM'), self.row('a.sldprt'), self.row('other.sldprt', app='Fusion'),
                self.row('license.sldprt', consent='cc-by'), self.row('drawing.slddrw'),
                self.row('b.sldprt', app='solidworks', volunteer='DO_NOT_PUBLISH')]
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            with mock.patch.dict(os.environ, GITHUB_OUTPUT=str(root / 'output')), \
                 mock.patch.object(runner.urllib.request, 'urlopen', return_value=io.BytesIO(json.dumps(dict(files=rows)).encode())):
                runner.prepare(root / 'fixtures')
            manifest = json.loads((root / 'fixtures/manifest.json').read_text())
            self.assertEqual([r['file'] for r in manifest['files']], ['a.sldprt', 'b.sldprt', 'z.SLDASM'])
            self.assertNotIn('DO_NOT_PUBLISH', json.dumps(manifest))
            matrix = json.loads((root / 'output').read_text().split('=', 1)[1])
            self.assertEqual(matrix, dict(variant=['pin', 'main'], shard=list(range(16))))

    def test_score_only_visits_its_shard(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            rows = [self.row(f'{index}.sldprt') for index in range(35)]
            write(root / 'manifest.json', dict(files=rows))
            args = argparse.Namespace(manifest=root / 'manifest.json', shard=2, variant='pin', binary=root / 'missing', out=root / 'out')
            with mock.patch.object(runner, 'one') as one:
                runner.score(args)
            self.assertEqual([call.args[0]['file'] for call in one.call_args_list], ['2.sldprt', '18.sldprt', '34.sldprt'])

    def test_unavailable_build_records_every_selected_file(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            rows = [self.row(f'{index}.sldprt') for index in range(17)]
            write(root / 'manifest.json', dict(files=rows))
            runner.score(argparse.Namespace(manifest=root / 'manifest.json', shard=0, variant='main',
                                            binary=root / 'missing', out=root / 'out'))
            records = [json.loads(path.read_text()) for path in (root / 'out').glob('*-result.json')]
            self.assertEqual(len(records), 2)
            self.assertTrue(all(r['complete'] and r['convert_exit'] is None for r in records))
            self.assertTrue(all('build unavailable' in r['error'] for r in records))

    def test_conversion_and_gate_reopen_commands_with_timeout(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binary, cli = root / 'dutchman', root / 'cli'
            binary.touch()
            cli.touch()

            def execute(command, stdout, stderr, env, deadline):
                self.assertGreater(deadline, runner.time.monotonic() + 895)
                if command[1] == 'convert':
                    self.assertEqual(command[-3:], ['--json', '--quiet', '--yes'])
                    Path(command[command.index('-o') + 1]).touch()
                    stdout.write('{"ok":true,"history":{"features":[],"rebuilt":0}}')
                else:
                    self.assertEqual(json.loads(Path(command[2]).read_text()),
                                     {'commands': [{'command': 'timeline.roll_to', 'params': {'position': 'end'}}]})
                    self.assertEqual(command[3], '--in')
                    self.assertEqual(command[-3:-1], ['--results', '--json-out'])
                return 0

            with mock.patch.object(runner.fixtures, 'download'), mock.patch.object(runner.fixtures, 'run', side_effect=execute):
                value = runner.one(self.row(), binary, cli, root, 'test')
            self.assertEqual((value['convert_exit'], value['reopen_exit'], value['complete']), (0, 0, True))
            with mock.patch.object(runner.fixtures, 'download'), \
                 mock.patch.object(runner.fixtures, 'run', side_effect=subprocess.TimeoutExpired('convert', 900)):
                value = runner.one(self.row(), binary, cli, root, 'timeout')
            self.assertEqual(value['convert_exit'], 124)
            self.assertIsNone(value['reopen_exit'])
            self.assertTrue(value['complete'])

    def test_download_integrity_failure_is_recorded_without_conversion(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            tool = root / 'tool'
            tool.touch()
            with mock.patch.object(runner.fixtures.urllib.request, 'urlopen', return_value=io.BytesIO(b'wrong!!')), \
                 mock.patch.object(runner.fixtures, 'run') as run:
                value = runner.one(self.row(), tool, tool, root, 'bad-hash')
            run.assert_not_called()
            self.assertIn('SHA-256', value['error'])
            self.assertFalse(value['download_ok'])
            self.assertTrue(value['complete'])


if __name__ == '__main__':
    unittest.main()
