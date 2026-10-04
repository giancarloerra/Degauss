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
    def test_messagepack_licence_is_fetchable_and_uses_repository_bytes(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            staged = root / 'deploy/Scripts/.config/degauss'
            staged.mkdir(parents=True)
            (staged / 'degauss').write_bytes(b'frontend')
            (staged / 'MessagePack-MIT.txt').write_bytes(b'staged copy')
            main = root / 'MiSTer_Degauss'
            main.write_bytes(b'main')
            out = root / 'database.json'
            subprocess.run(
                [sys.executable, str(ROOT / 'scripts/make-db.py'),
                 'v1.0.0', str(root / 'deploy'), str(main), str(out)],
                cwd=ROOT, check=True, capture_output=True,
            )
            entry = json.loads(out.read_text())['files']['Scripts/.config/degauss/MessagePack-MIT.txt']
            source = (ROOT / 'assets/licenses/MessagePack-MIT.txt').read_bytes()
            self.assertEqual(entry['hash'], hashlib.md5(source).hexdigest())
            self.assertEqual(entry['size'], len(source))
            self.assertEqual(
                entry['url'],
                'https://raw.githubusercontent.com/giancarloerra/Degauss/v1.0.0/assets/licenses/MessagePack-MIT.txt',
            )

    def test_delivery_excludes_user_owned_configuration(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            staged = root / 'deploy/Scripts/.config/degauss'
            staged.mkdir(parents=True)
            (staged / 'degauss').write_bytes(b'frontend')
            (staged / 'degauss.toml').write_bytes(b'staged defaults')
            main = root / 'MiSTer_Degauss'
            main.write_bytes(b'main')
            out = root / 'database.json'
            command = [sys.executable, str(ROOT / 'scripts/make-db.py'),
                       'v1.0.0', str(root / 'deploy'), str(main), str(out)]
            subprocess.run(command, cwd=ROOT, check=True, capture_output=True)
            files = json.loads(out.read_text())['files']
            self.assertIn('Scripts/.config/degauss/degauss.toml', files)
            self.assertFalse(any(path.endswith('/degauss-user.toml') for path in files))
            # A contaminated staging directory must not publish a personal file.
            user = staged / 'degauss-user.toml'
            user.write_text('wait_for_mounts = ["/media/fat/cifs"]\n')
            previous = user.read_bytes()
            failed = subprocess.run(command, cwd=ROOT, capture_output=True)
            self.assertNotEqual(failed.returncode, 0)
            self.assertIn(b'no source known', failed.stderr)
            self.assertEqual(user.read_bytes(), previous)

    def test_ra_core_chooser_uses_root_only_for_the_private_menu(self):
        patch = (ROOT / 'support/ra-main/frontend.patch').read_text()
        menu = patch.split('diff --git a/menu.cpp b/menu.cpp\n', 1)[1].split('diff --git', 1)[0]
        self.assertIn(
            '+\t\tif (is_menu() && !strcmp(get_rbf_path(), "/media/fat/degauss/menu.rbf")) selPath[0] = 0;',
            menu,
        )
        self.assertIn('diff --git a/tests/run-degauss-core-menu-tests.py', patch)
        build = (ROOT / 'scripts/build-ra-main.sh').read_text()
        self.assertIn('python3 tests/run-degauss-core-menu-tests.py\n', build)

    def test_database_maps_shipped_shadow_masks_to_repository_files(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            staged = root / 'deploy/Scripts/.config/degauss/masks'
            staged.mkdir(parents=True)
            name = 'Scanlines + Grille.txt'
            (staged / name).write_bytes(b'staged copy')
            frontend = staged.parent / 'degauss'
            frontend.write_bytes(b'frontend')
            main = root / 'MiSTer_Degauss'
            main.write_bytes(b'main')
            out = root / 'database.json'
            subprocess.run(
                [sys.executable, str(ROOT / 'scripts/make-db.py'),
                 'v0.9.0', str(root / 'deploy'), str(main), str(out)],
                cwd=ROOT, check=True, capture_output=True,
            )
            entry = json.loads(out.read_text())['files']['Scripts/.config/degauss/masks/' + name]
            source = (ROOT / 'assets/masks' / name).read_bytes()
            self.assertEqual(entry['hash'], hashlib.md5(source).hexdigest())
            self.assertEqual(entry['size'], len(source))
            self.assertEqual(
                entry['url'],
                'https://raw.githubusercontent.com/giancarloerra/Degauss/v0.9.0/assets/masks/Scanlines%20%2B%20Grille.txt',
            )

    def test_degauss_menu_binary_matches_recorded_checksum(self):
        menu_dir = ROOT / 'support/menu-core'
        expected = (menu_dir / 'menu.rbf.sha256').read_text().split()[0]
        actual = hashlib.sha256((menu_dir / 'menu.rbf').read_bytes()).hexdigest()
        self.assertEqual(actual, expected)

    def test_degauss_menu_native_framebuffer_window_uses_inclusive_bounds(self):
        patch = (ROOT / 'support/menu-core/degauss-menu-0.9.1.patch').read_text()
        self.assertIn("+\t.hdisp(native_custom_timing ? native_hdisp : 12'd1058),", patch)
        self.assertIn("+\t.hmax(native_custom_timing ? native_hdisp - 12'd1 : 12'd1057),", patch)
        self.assertIn("+\t.vdisp(native_custom_timing ? native_vdisp : menu_pal ? 12'd288 : 12'd240),", patch)
        self.assertIn("+\t.vmax(native_custom_timing ? native_vdisp - 12'd1 : menu_pal ? 12'd287 : 12'd239),", patch)
        self.assertNotIn("+\t.hmax(12'd1058),", patch)
        self.assertNotIn("+\t.vmax(menu_pal ? 12'd300 : 12'd240),", patch)

    def test_both_main_and_menu_ship_the_extended_analog_timing_command(self):
        main = (ROOT / 'support/ra-main/frontend.patch').read_text()
        menu = (ROOT / 'support/menu-core/degauss-menu-0.9.1.patch').read_text()
        self.assertIn('DEGAUSS_ANALOG_VIDEO_MODE', main)
        self.assertIn('+\t\t\t\tif (native && res == 0xD161)', main)
        self.assertIn('+\t\t\t\t\tfor (uint16_t value : native_timing) spi_w(value);', main)
        self.assertIn('+\t\t\t\tcase(cnt[4:0])', menu)
        self.assertIn("+\t\t\t\t\t10: native_htotal  <= io_din[11:0];", menu)
        self.assertIn("+\t\t\t\t\t17: native_vdisp   <= io_din[11:0];", menu)
        self.assertIn('diff --git a/tests/run-native-timing-tests.py', menu)
        self.assertIn('diff --git a/tests/run-degauss-analog-timing-tests.py', main)
        build = (ROOT / 'scripts/build-ra-main.sh').read_text()
        self.assertIn('python3 tests/run-degauss-analog-timing-tests.py\n', build)

    def test_degauss_menu_arbiter_holds_each_read_until_its_burst_completes(self):
        patch = (ROOT / 'support/menu-core/degauss-menu-0.9.1.patch').read_text()
        self.assertIn('+localparam [2:0] RAM_MENU_READ', patch)
        self.assertIn('+localparam [2:0] RAM_NATIVE_READ', patch)
        self.assertIn('+reg [7:0] ram_reads_remaining = 0;', patch)
        self.assertIn('+assign menu_ram_readdatavalid = ram_readdatavalid &', patch)
        self.assertIn('+assign native_fb_readdatavalid = ram_readdatavalid &', patch)
        self.assertIn('+\t.avl_readdatavalid(native_fb_readdatavalid),', patch)
        self.assertIn('+\t.DDRAM_DOUT_READY(menu_ram_readdatavalid),', patch)
        self.assertIn('+set_global_assignment -name SEED 5', patch)
        source = (ROOT / 'support/menu-core/Degauss_Menu.SOURCE.txt').read_text()
        self.assertIn('Quartus fitter seed: 5', source)

    def test_ra_preset_load_failure_restores_the_previous_video_state(self):
        patch = (ROOT / 'support/ra-main/frontend.patch').read_text()
        self.assertIn('+bool video_loadPreset(char *name, bool save)', patch)
        self.assertIn('+\tif (!video_loadPreset(path, false))', patch)
        self.assertIn('+\t\tdegauss_restore_preset_baseline(true);', patch)
        self.assertIn('+\t\tsnprintf(error, error_size, "Preset could not be opened");', patch)
        self.assertIn('+\t\treturn false;', patch)

    def test_workflow_rejects_upstream_ra_at_any_card_path(self):
        workflow = (ROOT / '.github/workflows/release.yml').read_text()
        start = workflow.index('          import posixpath\n')
        end = workflow.index('          for path, entry', start)
        guard = '\n'.join(line[10:] for line in workflow[start:end].splitlines())
        exec(guard, {'db': {'files': {'degauss/MiSTer_RA_Degauss': {}}}})
        for path in ('MiSTer_RA', 'degauss/MiSTer_RA', 'nested/core/MiSTer_RA'):
            with self.subTest(path=path), self.assertRaisesRegex(AssertionError, 'must not overwrite upstream RA Main'):
                exec(guard, {'db': {'files': {path: {}}}})

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
            self.assertEqual(set(legacy['files']), {
                'Scripts/.config/degauss/degauss',
                'degauss/MiSTer_Degauss',
                'degauss/menu.rbf',
                'degauss/Degauss_Menu.SOURCE.txt',
            })
            menu = legacy['files']['degauss/menu.rbf']
            menu_bytes = (ROOT / 'support/menu-core/menu.rbf').read_bytes()
            self.assertEqual(menu['hash'], hashlib.md5(menu_bytes).hexdigest())
            self.assertEqual(menu['size'], len(menu_bytes))
            self.assertEqual(
                menu['url'],
                'https://raw.githubusercontent.com/giancarloerra/Degauss/v1.2.3/support/menu-core/menu.rbf',
            )
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
