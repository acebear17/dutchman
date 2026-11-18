"""Prove path selection for whole merge groups and conservative fallback."""
import importlib.util
from pathlib import Path
import subprocess
import unittest
import tempfile
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location('ci_changes', ROOT / 'scripts/ci-changes.py')
CHANGES = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CHANGES)


class ChangesTests(unittest.TestCase):
    def setUp(self):
        scan = patch.object(CHANGES, 'referenced_inputs', return_value=set())
        self.inputs = scan.start()
        self.addCleanup(scan.stop)

    def test_changelog_only(self):
        self.assertFalse(any(CHANGES.flags(['changelog.d/fix.md']).values()))

    def test_readme_requires_focused_tests(self):
        selected = CHANGES.flags(['README.md'], inputs={'README.md'})
        self.assertTrue(all(selected.values()))

    def test_crate_readme_needs_no_rust_build(self):
        self.assertTrue(CHANGES.flags(["crates/dutchman-core/README.md"])['code'])

    def test_other_guide_needs_no_rust_build(self):
        self.assertFalse(any(CHANGES.flags(['docs/guide.md']).values()))

    def test_gui_does_not_rebuild_the_image(self):
        selected = CHANGES.flags(['crates/dutchman-gui/src/main.rs'])
        self.assertTrue(selected['code'])
        self.assertTrue(selected['rust'])
        self.assertFalse(selected['docker'])
        self.assertFalse(selected['conversion'])

    def test_mixed_group_preserves_checks(self):
        selected = CHANGES.flags(['changelog.d/note.md', 'crates/dutchman-core/src/decode.rs'])
        self.assertTrue(selected['code'])
        self.assertTrue(selected['rust'])
        self.assertTrue(selected['docker'])
        self.assertTrue(selected['conversion'])

    def test_renamed_source_still_runs_code_checks(self):
        self.assertTrue(CHANGES.flags(['scripts/new-check.py'])['code'])

    def test_unknown_file_runs_code_checks(self):
        self.assertTrue(CHANGES.flags(['some-new-build-file'])['code'])

    def test_full_dispatch_and_tag_force_checks_without_reading_git(self):
        for event, ref in [('workflow_dispatch', 'refs/heads/topic'), ('push', 'refs/tags/v1.0.0')]:
            with patch.object(CHANGES.subprocess, 'run', side_effect=AssertionError('must not need base')):
                self.assertTrue(all(CHANGES.select('', event, ref).values()))

    def test_missing_queue_base_fails_open(self):
        with patch.object(CHANGES.subprocess, 'run', return_value=subprocess.CompletedProcess([], 1)):
            self.assertTrue(all(CHANGES.select('missing', 'merge_group', 'refs/heads/queue').values()))

    def test_complete_diff_uses_nul_separated_paths(self):
        with patch.object(CHANGES.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0)), \
             patch.object(CHANGES.subprocess, 'check_output', return_value=b'changelog.d/n.md\0crates/dutchman-core/src/lib.rs\0') as diff:
            selected = CHANGES.select('base', 'merge_group', 'refs/heads/queue')
            self.assertTrue(selected['full'])
            self.assertTrue(selected['code'])
            diff.assert_called_once_with(['git', 'diff', '--name-only', '-z', 'base', 'HEAD'])

    def test_workflow_selects_conversion_jobs_for_core_but_not_gui(self):
        workflow = (ROOT / '.github/workflows/ci.yml').read_text()
        for path, expected in [('crates/dutchman-gui/src/app.rs', False),
                               ('crates/dutchman-core/src/decode.rs', True)]:
            outputs = {'full': True, **CHANGES.flags([path])}
            for job in ('wasm', 'solidcraft-roundtrip', 'placements'):
                section = workflow[workflow.index('  ' + job + ':\n'):]
                condition = next(line.strip()[4:] for line in section.splitlines() if line.startswith('    if:'))
                condition = condition.replace('github.event_name', repr('push'))
                for key, value in outputs.items():
                    condition = condition.replace('needs.changes.outputs.' + key, repr(str(value).lower()))
                selected = eval(condition.replace('&&', 'and'), {'__builtins__': {}})
                self.assertEqual(selected, expected, (path, job, condition))

    def test_gui_manifest_keeps_conversion_and_image_checks(self):
        selected = CHANGES.flags(['crates/dutchman-gui/Cargo.toml'])
        self.assertTrue(selected['conversion'])
        self.assertTrue(selected['docker'])

    def select_group(self, paths):
        with patch.object(CHANGES.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0)), \
             patch.object(CHANGES.subprocess, 'check_output', return_value=b'\0'.join(p.encode() for p in paths) + b'\0'):
            return CHANGES.select('base', 'merge_group', 'refs/heads/queue')

    def test_asset_only_merge_group_forces_every_flag_true(self):
        self.assertTrue(all(self.select_group(['assets/logo/dutchman-app-icon.svg']).values()))

    def test_every_non_docs_group_forces_every_flag_true(self):
        for path in ('crates/dutchman-gui/src/app.rs', 'Dockerfile', '.github/ISSUE_TEMPLATE/bug.yml',
                     'image.png', 'some-new-input', 'changelog.d/metadata.json'):
            with self.subTest(path=path):
                self.assertTrue(all(self.select_group(['docs/guide.md', path]).values()))

    def test_proven_docs_only_group_skips_code_jobs(self):
        selected = self.select_group(['docs/guide.md', 'changelog.d/fix.md',
                                      'CHANGELOG', 'LICENSE', 'NOTICE', 'CONTRIBUTING.md'])
        self.assertEqual(selected, dict(full=True, code=False, rust=False, conversion=False, docker=False, docs=False))

    def test_readme_is_not_unconditionally_docs_only(self):
        selected = self.select_group(['README.md', 'docs/formats.md'])
        self.assertTrue(selected['docs'])
        self.assertTrue(selected['code'])

    def test_referenced_markdown_merge_group_forces_every_flag(self):
        self.inputs.return_value = {'docs/json.md'}
        self.assertTrue(all(self.select_group(['docs/json.md']).values()))

    def test_deleted_embedded_svg_selects_compilation_and_tests(self):
        selected = CHANGES.flags(['crates/dutchman-gui/assets/dutchman-app-icon.svg'])
        self.assertTrue(selected['code'])
        self.assertTrue(selected['rust'])

    def test_install_doc_consumed_by_readme_tests_runs_checks(self):
        selected = CHANGES.flags(['docs/install.md'], inputs={'docs/install.md'})
        self.assertTrue(selected['code'])
        self.assertTrue(selected['docs'])


class ReferenceScanTests(unittest.TestCase):
    def test_new_relative_include_and_packaging_tree_are_discovered(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            files = {
                'crates/example/src/lib.rs': 'include_str!("../../../guide.md");',
                'guide.md': 'Included at compile time',
                'tests/report.rs': 'std::fs::read_to_string("docs/json.md");',
                'docs/json.md': 'Read by tests',
                'docs/new-guide.md': 'Packaged recursively',
                'skills/new/SKILL.md': 'Packaged recursively',
                'scripts/package-docs.py': 'for tree in ("docs", "skills"): root.joinpath(tree).rglob("*.md")',
                'build.rs': 'read("build-guide.md");',
                'build-guide.md': 'Build input',
                'CONTRIBUTING.md': 'Unconsumed guidance',
            }
            for name, content in files.items():
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(content)
            subprocess.run(['git', 'init', '-q', str(root)], check=True)
            subprocess.run(['git', '-C', str(root), 'add', '.'], check=True)
            inputs = CHANGES.referenced_inputs(root)
            for name in ('guide.md', 'docs/json.md', 'docs/new-guide.md', 'skills/new/SKILL.md', 'build-guide.md'):
                self.assertIn(name, inputs)
            self.assertNotIn('CONTRIBUTING.md', inputs)
            # A newly added reference removes an otherwise allowed Markdown file.
            (root / 'tests/new.rs').write_text('read_to_string("CONTRIBUTING.md");')
            subprocess.run(['git', '-C', str(root), 'add', '.'], check=True)
            inputs = CHANGES.referenced_inputs(root)
            self.assertIn('CONTRIBUTING.md', inputs)
            with patch.object(CHANGES, 'referenced_inputs', return_value=inputs), \
                 patch.object(CHANGES.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0)), \
                 patch.object(CHANGES.subprocess, 'check_output', return_value=b'CONTRIBUTING.md\0'):
                self.assertTrue(all(CHANGES.select('base', 'merge_group', 'refs/heads/queue').values()))
            # Deletion must not lose the include target from the exclusion.
            (root / 'guide.md').unlink()
            self.assertIn('guide.md', CHANGES.referenced_inputs(root, ['guide.md']))

    def test_current_packaged_docs_and_test_inputs_are_excluded(self):
        inputs = CHANGES.referenced_inputs()
        for name in ('README.md', 'docs/json.md', 'docs/install.md',
                     'skills/dutchman-converting/SKILL.md', 'assets/install/AGENTS.md'):
            self.assertIn(name, inputs)
