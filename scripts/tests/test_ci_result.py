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
    def check_results(self, event, release=False, failed=None):
        workflow = (ROOT / ".github/workflows/ci.yml").read_text()
        script = workflow.split('echo "$RESULTS" | python3 -c \'\n', 1)[1]
        script = textwrap.dedent(script.rsplit("\n          '", 1)[0])
        full = event != "pull_request"
        active = {"changes", "release-notes", "fmt", "clippy", "test"}
        if full:
            active |= {"wasm", "msrv", "solidcraft-roundtrip", "test-macos",
                       "docker", "test-arm", "placements"}
        if event == "workflow_dispatch" and release:
            active.add("release-build")
        jobs = active | {"docs", "test-arm-again", "release-build", "wasm", "msrv",
                         "solidcraft-roundtrip", "test-macos", "docker", "test-arm", "placements"}
        needs = {job: {"result": "success" if job in active else "skipped"} for job in jobs}
        needs["changes"]["outputs"] = {key: "true" for key in ("code", "rust", "docker")}
        needs["changes"]["outputs"]["full"] = str(full).lower()
        if failed:
            needs[failed] = {"result": "skipped"}
        return subprocess.run(
            [sys.executable, "-c", script], input=json.dumps(needs), text=True,
            capture_output=True,
            env={**os.environ, "EVENT": event, "RELEASE_BUILDS": str(release).lower(),
                 "GITHUB_REF": "refs/heads/test"},
        )

    def test_queue_does_not_require_release_packaging(self):
        result = self.check_results("merge_group")
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_queue_still_requires_tests(self):
        result = self.check_results("merge_group", failed="test")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("test", result.stderr)

    def test_dispatch_without_release_packaging(self):
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
