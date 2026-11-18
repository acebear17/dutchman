"""Prove path selection for whole merge groups and conservative fallback."""
import importlib.util
from pathlib import Path
import subprocess
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location('ci_changes', ROOT / 'scripts/ci-changes.py')
CHANGES = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CHANGES)


class ChangesTests(unittest.TestCase):
    def test_changelog_only(self):
        self.assertFalse(any(CHANGES.flags(['changelog.d/fix.md']).values()))

    def test_readme_requires_focused_tests(self):
        self.assertEqual(CHANGES.flags(['README.md']),
                         dict(code=False, rust=False, docker=False, docs=True))

    def test_crate_readme_needs_no_rust_build(self):
        self.assertFalse(any(CHANGES.flags(["crates/dutchman-core/README.md"]).values()))

    def test_other_guide_needs_no_rust_build(self):
        self.assertFalse(any(CHANGES.flags(['docs/guide.md']).values()))

    def test_gui_does_not_rebuild_the_image(self):
        selected = CHANGES.flags(['crates/dutchman-gui/src/main.rs'])
        self.assertTrue(selected['code'])
        self.assertTrue(selected['rust'])
        self.assertFalse(selected['docker'])

    def test_mixed_group_preserves_checks(self):
        selected = CHANGES.flags(['changelog.d/note.md', 'crates/dutchman-core/src/decode.rs'])
        self.assertTrue(selected['code'])
        self.assertTrue(selected['rust'])
        self.assertTrue(selected['docker'])

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
