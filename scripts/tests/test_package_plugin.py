"""Check manifest selection without compiling a WebAssembly component."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
import zipfile

ROOT = Path(__file__).resolve().parents[2]


class PackagePluginTests(unittest.TestCase):
    def test_legacy_and_conditional_packages_keep_separate_manifests(self):
        with tempfile.TemporaryDirectory() as folder:
            work = Path(folder)
            module = work / 'module.wasm'
            module.write_bytes(b'synthetic module')
            tool = work / 'wasm-tools'
            # Packaging only: component construction and inspection are stubbed.
            tool.write_text('''#!/usr/bin/env python3
import pathlib, sys
args = sys.argv[1:]
if args[:2] == ['component', 'new']:
    pathlib.Path(args[args.index('-o') + 1]).write_bytes(b'synthetic component')
elif args[:2] == ['component', 'wit']:
    print('  import craft:plugin/source@0.1.0;')
''')
            tool.chmod(0o700)
            for conditional in (False, True):
                for version in (None, '0.3.1'):
                    with self.subTest(conditional=conditional, version=version):
                        out = work / f'package-{conditional}-{version}'
                        args = ['bash', str(ROOT / 'scripts/package-plugin.sh'),
                                str(module), str(out)]
                        if version:
                            args.append(version)
                        if conditional:
                            args.append('--dialog-conditions')
                        subprocess.run(args, env={**os.environ, 'WASM_TOOLS': str(tool)},
                                       check=True, capture_output=True, text=True)
                        source = 'plugin-dialog-conditions.json' if conditional else 'plugin.json'
                        expected = json.loads((ROOT / 'crates/dutchman-plugin' / source).read_text())
                        if version:
                            expected['version'] = version
                        self.assertEqual(json.loads((out / 'plugin.json').read_text()), expected)
                        archive = 'dutchman-plugin-0.3.1.zip' if version else 'dutchman-plugin.zip'
                        with zipfile.ZipFile(out / archive) as package:
                            self.assertEqual(set(package.namelist()), {'plugin.json', 'dutchman.wasm'})
                            self.assertEqual(json.loads(package.read('plugin.json')), expected)
                        if version:
                            self.assertTrue((out / (archive + '.sha256')).is_file())


if __name__ == '__main__':
    unittest.main()
