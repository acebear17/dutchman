"""Exercise the required status check with real event/result combinations."""
import json
import os
from pathlib import Path
import subprocess
import sys
import textwrap
import unittest

ROOT = Path(__file__).resolve().parents[2]


class CiResultTests(unittest.TestCase):
    def check_results(self, event, release=False, failed=None, docs_only=False, docs_changed=True, failed_result="skipped", gui_only=False):
        workflow = (ROOT / ".github/workflows/ci.yml").read_text()
        script = workflow.split('echo "$RESULTS" | python3 -c \'\n', 1)[1]
        script = textwrap.dedent(script.rsplit("\n          '", 1)[0])
        full = event != "pull_request"
        active = {"changes", "release-notes", "fmt", "clippy", "test"}
        if full:
            active |= {"wasm", "msrv", "solidcraft-roundtrip", "test-macos",
                       "docker", "test-arm", "placements"}
        opt_in = "RELEASE_BUILDS:" in workflow
        packaging = (event == "workflow_dispatch" and release) if opt_in else event in ("merge_group", "workflow_dispatch")
        if packaging:
            active.add("release-build")
        if gui_only:
            active -= {"wasm", "solidcraft-roundtrip", "placements", "docker"}
        if docs_only:
            active = {"changes", "release-notes"} | ({"docs"} if docs_changed else set())
        if packaging:
            active.add("release-build")
        jobs = active | {"docs", "test-arm-again", "release-build", "wasm", "msrv",
                         "solidcraft-roundtrip", "test-macos", "docker", "test-arm", "placements", "fmt", "clippy", "test"}
        needs = {job: {"result": "success" if job in active else "skipped"} for job in jobs}
        needs["release-build"]["outputs"] = {"required": "success" if packaging else "skipped"}
        needs["changes"]["outputs"] = {key: "true" for key in ("code", "rust", "conversion", "docker", "docs")}
        needs["changes"]["outputs"]["full"] = str(full).lower()
        if docs_only:
            for key in ("code", "rust", "conversion", "docker"):
                needs["changes"]["outputs"][key] = "false"
            needs["changes"]["outputs"]["docs"] = str(docs_changed).lower()
        if gui_only:
            needs["changes"]["outputs"]["conversion"] = "false"
            needs["changes"]["outputs"]["docker"] = "false"
        if failed:
            needs[failed] = {"result": failed_result}
        return subprocess.run(
            [sys.executable, "-c", script], input=json.dumps(needs), text=True,
            capture_output=True,
            env={**os.environ, "EVENT": event, "RELEASE_BUILDS": str(release).lower(),
                 "GITHUB_REF": "refs/heads/test"},
        )

    def test_queue_release_contract(self):
        result = self.check_results("merge_group")
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_queue_still_requires_tests(self):
        result = self.check_results("merge_group", failed="test")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("test", result.stderr)

    def test_dispatch_release_contract(self):
        result = self.check_results("workflow_dispatch")
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_dispatch_requires_requested_release_packaging(self):
        result = self.check_results("workflow_dispatch", release=True, failed="release-build")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("release-build", result.stderr)

    def test_dispatch_accepts_successful_release_packaging(self):
        result = self.check_results("workflow_dispatch", release=True)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_fast_pr(self):
        result = self.check_results("pull_request")
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_docs_only_queue(self):
        result = self.check_results("merge_group", docs_only=True)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_changelog_only_queue(self):
        result = self.check_results("merge_group", docs_only=True, docs_changed=False)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_docs_only_queue_requires_focused_tests(self):
        result = self.check_results("merge_group", docs_only=True, failed="docs")
        self.assertNotEqual(result.returncode, 0)

    def test_cancelled_pr_prerequisite_fails_with_rerun_instruction(self):
        for job in ("release-notes", "fmt", "clippy", "test"):
            result = self.check_results("pull_request", failed=job, failed_result="cancelled")
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("Required jobs were cancelled: " + job, result.stderr)
            self.assertIn("Rerun CI for this commit", result.stderr)

    def test_cancelled_queue_prerequisite_cannot_pass(self):
        result = self.check_results("merge_group", failed="test", failed_result="cancelled")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("cancelled (expected success)", result.stderr)

    def test_queue_cannot_skip_conversion_checks_for_gui(self):
        result = self.check_results("merge_group", gui_only=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("non-docs merge group filters must all be true", result.stderr)

    def test_gui_only_queue_still_requires_workspace_tests(self):
        result = self.check_results("merge_group", gui_only=True, failed="test")
        self.assertNotEqual(result.returncode, 0)

    def test_cancelled_selector_without_outputs_reports_a_rerun(self):
        result = self.check_results("pull_request", failed="changes", failed_result="cancelled")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Required jobs were cancelled:", result.stderr)
        self.assertIn("changes", result.stderr)
        self.assertNotIn("Traceback", result.stderr)
