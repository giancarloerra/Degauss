#!/usr/bin/env python3
"""Local regression checks for RA release source and Downloader delivery."""
import hashlib
import importlib.util
import json
import os
import pathlib
import subprocess
import sys
import tarfile
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location('package_ra', ROOT / 'scripts/package-ra-main.py')
package_ra = importlib.util.module_from_spec(spec)
spec.loader.exec_module(package_ra)


class ReleaseTests(unittest.TestCase):
    def test_database_preserves_legacy_and_adds_separate_ra_assets(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            frontend = root / 'deploy/Scripts/.config/degauss/degauss'
            frontend.parent.mkdir(parents=True)
            frontend.write_bytes(b'frontend')
            normal = root / 'normal'
            normal.write_bytes(b'normal main')
            out = root / 'database.json'
            command = [sys.executable, str(ROOT / 'scripts/make-db.py'), 'v1.2.3', str(root / 'deploy'), str(normal), str(out)]
            subprocess.run(command, cwd=ROOT, check=True, capture_output=True)
            legacy = json.loads(out.read_text())
            self.assertEqual(set(legacy['files']), {'Scripts/.config/degauss/degauss', 'degauss/MiSTer_Degauss'})
            ra = root / 'ra'
            ra.mkdir()
            names = ('MiSTer_RA_Degauss', 'MiSTer_RA_Degauss.cacert.pem', 'MiSTer_RA_Degauss.SOURCE.txt')
            for name in names:
                (ra / name).write_bytes(name.encode())
            subprocess.run(command + ['--ra-dir', str(ra)], cwd=ROOT, check=True, capture_output=True)
            updated = json.loads(out.read_text())
            for path, entry in legacy['files'].items():
                self.assertEqual(updated['files'][path], entry)
            for name in names:
                entry = updated['files']['degauss/' + name]
                self.assertEqual(entry['hash'], hashlib.md5(name.encode()).hexdigest())
                self.assertEqual(entry['size'], len(name.encode()))
                self.assertEqual(entry['url'], 'https://github.com/giancarloerra/Degauss/releases/download/v1.2.3/' + name)
            self.assertNotIn('MiSTer_RA', updated['files'])
            self.assertFalse(any(path.endswith('.ini') for path in updated['files']))
            (ra / names[1]).unlink()
            out.unlink()
            failed = subprocess.run(command + ['--ra-dir', str(ra)], cwd=ROOT, capture_output=True)
            self.assertNotEqual(failed.returncode, 0)
            self.assertFalse(out.exists(), 'Incomplete RA package must not produce a successful database')

    def test_source_archive_is_identical_across_umasks_and_preserves_executability(self):
        with tempfile.TemporaryDirectory() as tmp:
            archives = []
            for mask in (0o022, 0o077):
                root = pathlib.Path(tmp) / str(mask)
                required = ('MiSTer_RA_Degauss', 'MiSTer_RA_Degauss.cacert.pem', 'SOURCE-PINS.txt',
                            'source/LICENSE', 'source/Makefile', 'source/ra_http.cpp',
                            'rebuild/scripts/build-ra-main.sh', 'rebuild/scripts/package-ra-main.py',
                            'rebuild/support/ra-main/frontend.patch', 'rebuild/support/ra-main/tls.patch',
                            'rebuild/support/ra-main/controller.patch', 'rebuild/support/ra-main/Dockerfile')
                previous_mask = os.umask(mask)
                try:
                    for name in required:
                        target = root / name
                        target.parent.mkdir(parents=True, exist_ok=True)
                        target.write_text(name)
                    executable = root / 'rebuild/scripts/build-ra-main.sh'
                    executable.chmod(0o777 & ~mask)
                    os.link(root / 'source/LICENSE', root / 'source/LICENSE-link')
                    package_ra.package(root)
                finally:
                    os.umask(previous_mask)
                archive = root / 'MiSTer_RA_Degauss-source.tar.gz'
                archives.append(archive.read_bytes())
                with tarfile.open(archive) as tar:
                    for member in tar:
                        expected = 0o755 if member.isdir() or member.name.endswith('/build-ra-main.sh') else 0o644
                        self.assertEqual(member.mode, expected, member.name)
                    self.assertTrue(tar.getmember('ra-main-source/source/LICENSE-link').islnk())
                self.assertEqual(executable.stat().st_mode & 0o777, 0o777 & ~mask,
                                 'Packaging must not change the build tree permissions')
            self.assertEqual(archives[0], archives[1],
                             'Equivalent source trees must produce byte-identical release archives')

    def test_source_archive_retains_build_inputs_and_excludes_build_products(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            required = ('MiSTer_RA_Degauss', 'MiSTer_RA_Degauss.cacert.pem', 'SOURCE-PINS.txt',
                        'source/LICENSE', 'source/Makefile', 'source/ra_http.cpp',
                        'rebuild/scripts/build-ra-main.sh', 'rebuild/scripts/package-ra-main.py',
                        'rebuild/support/ra-main/frontend.patch', 'rebuild/support/ra-main/tls.patch',
                        'rebuild/support/ra-main/controller.patch',
                        'rebuild/support/ra-main/Dockerfile', 'source/bin/MiSTer',
                        'source/lib/example/bin/build-input')
            for name in required:
                target = root / name
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_text(name)
            package_ra.package(root)
            archive = root / 'MiSTer_RA_Degauss-source.tar.gz'
            with tarfile.open(archive) as tar:
                members = tar.getnames()
                for member in tar:
                    self.assertEqual((member.uid, member.gid, member.uname, member.gname, member.mtime), (0, 0, '', '', 0))
                self.assertIn('ra-main-source/source/ra_http.cpp', members)
                self.assertIn('ra-main-source/rebuild/scripts/package-ra-main.py', members)
                self.assertIn('ra-main-source/source/lib/example/bin/build-input', members)
                self.assertNotIn('ra-main-source/source/bin/MiSTer', members)
            expected = hashlib.sha256(archive.read_bytes()).hexdigest()
            self.assertEqual((root / (archive.name + '.sha256')).read_text(), expected + '  ' + archive.name + '\n')
            self.assertIn(archive.name, (root / 'MiSTer_RA_Degauss.SOURCE.txt').read_text())
            (root / 'rebuild/scripts/package-ra-main.py').unlink()
            with self.assertRaises(SystemExit):
                package_ra.package(root)


if __name__ == '__main__':
    unittest.main()
