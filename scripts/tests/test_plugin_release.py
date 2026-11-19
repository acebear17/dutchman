"""Exercise release guards with synthetic artifacts; no downloads or GitHub calls."""
import fnmatch
import hashlib
import io
import os
from pathlib import Path
import re
import subprocess
import tarfile
import tempfile
import textwrap
import unittest

ROOT = Path(__file__).resolve().parents[2]
RELEASE = (ROOT / ".github/workflows/release.yml").read_text()
BUILD = (ROOT / ".github/workflows/release-build.yml").read_text()
VERSION = "1.2.3"
PLUGIN = f"dutchman-plugin-{VERSION}.zip"


def step(source, name):
    """Read a literal run block so the test executes the workflow's actual guard."""
    block = source.split(f"      - name: {name}\n", 1)[1]
    block = block.split("        run: |\n", 1)[1]
    lines = []
    for line in block.splitlines():
        if line and not line.startswith("          "):
            break
        lines.append(line)
    return textwrap.dedent("\n".join(lines))


class PluginRelease(unittest.TestCase):
    def setUp(self):
        self.folder = tempfile.TemporaryDirectory()
        self.addCleanup(self.folder.cleanup)
        self.root = Path(self.folder.name)
        self.env = dict(os.environ, VERSION=VERSION, DRY="true", TAG=f"v{VERSION}",
                        GITHUB_STEP_SUMMARY=str(self.root / "summary"))
        self.guard = step(RELEASE, "Flatten artifacts and check checksums and version")
        # Derive the full file set from the guard; all payloads are authored test data.
        array = self.guard.split("expected=(", 1)[1].split(")", 1)[0]
        self.names = re.findall(r'"([^"]+)"', array)
        self.names = [name.replace("$VERSION", VERSION) for name in self.names]
        artifacts = self.root / "artifacts/dutchman-1.2.3-synthetic"
        artifacts.mkdir(parents=True)
        for name in self.names:
            path = artifacts / name
            if name == f"dutchman-{VERSION}-x86_64-unknown-linux-gnu.tar.gz":
                data = f"#!/bin/sh\necho dutchman {VERSION}\n".encode()
                with tarfile.open(path, "w:gz") as archive:
                    item = tarfile.TarInfo(f"dutchman-{VERSION}-x86_64-unknown-linux-gnu/dutchman")
                    item.mode = 0o755
                    item.size = len(data)
                    archive.addfile(item, io.BytesIO(data))
            else:
                path.write_bytes(b"synthetic release payload")
            digest = hashlib.sha256(path.read_bytes()).hexdigest()
            path.with_name(name + ".sha256").write_text(f"{digest}  {name}\n")
        self.artifacts = artifacts

    def run_guard(self):
        return subprocess.run(["bash", "-c", self.guard], cwd=self.root, env=self.env,
                              capture_output=True, text=True, check=False)

    def test_build_artifact_is_downloaded_and_required(self):
        plugin = BUILD.split("\n  plugin:\n", 1)[1].split("\n  # Each Mac", 1)[0]
        name = re.search(r"name: (dutchman-\$\{\{ env.VERSION \}\}-plugin)", plugin)[1]
        pattern = re.search(r"pattern: (dutchman-\[0-9\]\*)", RELEASE)[1]
        self.assertTrue(fnmatch.fnmatchcase(name.replace("${{ env.VERSION }}", VERSION), pattern))
        self.assertIn("-p dutchman-plugin", plugin)
        self.assertIn("scripts/package-plugin.sh", plugin)
        self.assertIn("if-no-files-found: error", plugin)
        self.assertIn("needs: [plan, build, plugin, image-check]", BUILD)
        self.assertIn("needs.plugin.result == 'success'", BUILD)
        self.assertIn('sha256sum --check --strict "dutchman-plugin-$version.zip.sha256"', plugin)

    def test_latest_download_and_checksum_match_versioned_payload(self):
        result = self.run_guard()
        self.assertEqual(result.returncode, 0, result.stderr)
        dist = self.root / "dist"
        self.assertEqual((dist / PLUGIN).read_bytes(), (dist / "dutchman-plugin.zip").read_bytes())
        for name in (PLUGIN, "dutchman-plugin.zip"):
            digest, recorded = (dist / (name + ".sha256")).read_text().split()
            self.assertEqual(recorded, name)
            self.assertEqual(digest, hashlib.sha256((dist / name).read_bytes()).hexdigest())
        for document in ("README.md", "docs/solidcraft-plugin.md"):
            text = (ROOT / document).read_text()
            for name in ("dutchman-plugin.zip", "dutchman-plugin.zip.sha256"):
                self.assertIn(f"/releases/latest/download/{name})", text)

    def test_missing_plugin_fails_release(self):
        (self.artifacts / PLUGIN).unlink()
        (self.artifacts / (PLUGIN + ".sha256")).unlink()
        result = self.run_guard()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(f"Missing release file for {VERSION}: {PLUGIN}", result.stderr)

    def test_missing_plugin_checksum_fails_release(self):
        (self.artifacts / (PLUGIN + ".sha256")).unlink()
        result = self.run_guard()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(f"Missing release checksum: {PLUGIN}.sha256", result.stderr)

    def test_changed_plugin_fails_checksum(self):
        (self.artifacts / PLUGIN).write_bytes(b"changed payload")
        result = self.run_guard()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(f"{PLUGIN}: FAILED", result.stdout)

    def test_upload_missing_latest_plugin_fails_release(self):
        result = self.run_guard()
        self.assertEqual(result.returncode, 0, result.stderr)
        scripts = self.root / "scripts"
        scripts.mkdir()
        (scripts / "release-notes.py").write_text('print("Synthetic release notes")\n')
        tools = self.root / "tools"
        tools.mkdir()
        # Stand in for GitHub: accept creation but omit the stable zip in its asset listing.
        gh = tools / "gh"
        gh.write_text('''#!/bin/bash
if [[ "$1 $2" == "release view" ]]; then
  find dist -maxdepth 1 -type f -printf '%f\\n' | sed '/^dutchman-plugin.zip$/d'
fi
''')
        gh.chmod(0o755)
        self.env.update(DRY="false", GITHUB_REPOSITORY="example/synthetic",
                        STARTED="2026-11-18T00:00:00Z", PATH=str(tools) + os.pathsep + os.environ["PATH"])
        create = step(RELEASE, "Create the draft release")
        result = subprocess.run(["bash", "-c", create], cwd=self.root, env=self.env,
                                capture_output=True, text=True, check=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("draft release's files differ", result.stderr)
        self.assertIn("dutchman-plugin.zip", result.stderr)
