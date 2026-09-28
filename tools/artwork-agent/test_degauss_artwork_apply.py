import hashlib
import unittest
import xml.etree.ElementTree as ET
from unittest.mock import patch

from degauss_artwork_apply import copy_local_batch, plan, safe_relative, update_xml


class DegaussArtworkApplyTests(unittest.TestCase):
    def test_existing_entry_preserves_metadata_and_replaces_first_art_field(self):
        original = b"""<gameList><!--keep--><game><path>./Game.rom</path><name>Game</name><image source="old">./old.png</image><favorite>true</favorite></game></gameList>"""
        updated = update_xml(original, [{"path": "./Game.rom", "artwork": "./media/screenshot/Game.png"}])
        game = ET.fromstring(updated).find("game")
        self.assertEqual(game.findtext("image"), "./media/screenshot/Game.png")
        self.assertEqual(game.find("image").get("source"), "old")
        self.assertEqual(game.findtext("favorite"), "true")
        self.assertIsNone(game.find("screenshot"))
        self.assertIn(b"<!--keep-->", updated)

    def test_existing_entry_without_art_gets_screenshot(self):
        original = b"<gameList><game><path>./Game.rom</path><name>Game</name></game></gameList>"
        updated = update_xml(original, [{"path": "./Game.rom", "artwork": "./media/screenshot/Game.png"}])
        self.assertEqual(ET.fromstring(updated).find("game").findtext("screenshot"), "./media/screenshot/Game.png")

    def test_missing_entry_is_appended(self):
        original = b"<gameList />"
        updated = update_xml(original, [{"path": "./Game.rom", "name": "Game", "artwork": "./media/screenshot/Game.png"}])
        game = ET.fromstring(updated).find("game")
        self.assertEqual(game.findtext("path"), "./Game.rom")
        self.assertEqual(game.findtext("name"), "Game")
        self.assertEqual(game.findtext("screenshot"), "./media/screenshot/Game.png")

    def test_duplicate_manifest_path_is_rejected(self):
        operation = {"path": "./Game.rom", "name": "Game", "artwork": "./media/screenshot/Game.png"}
        with self.assertRaisesRegex(ValueError, "Duplicate manifest"):
            update_xml(b"<gameList />", [operation, operation])

    def test_path_escape_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "Unsafe"):
            safe_relative("./../outside.png", label="artwork path")

    def test_plan_verifies_a_shared_image_only_once(self):
        original = b"<gameList />"
        image = {
            "kind": "existing",
            "path": "./media/screenshot/Shared.png",
            "size": 10,
            "sha256": "a" * 64,
        }
        manifest = {
            "root": "/media/fat/games/Test",
            "gamelist_sha256": hashlib.sha256(original).hexdigest(),
            "operations": [
                {"path": "./One.rom", "name": "One", "artwork": image["path"], "image": image},
                {"path": "./Two.rom", "name": "Two", "artwork": image["path"], "image": image},
            ],
        }
        verified_image = ("/media/fat/games/Test/media/screenshot/Shared.png", (10, "a" * 64))
        with patch("degauss_artwork_apply.ssh", return_value=original), \
             patch("degauss_artwork_apply.remote_digests", return_value={verified_image[0]: verified_image[1]}) as digests, \
             patch("degauss_artwork_apply.verify_image", return_value=verified_image) as verify:
            _, _, verified = plan(manifest)
        self.assertEqual(len(verified), 2)
        verify.assert_not_called()
        digests.assert_called_once_with([verified_image[0]])

    def test_batch_copy_rejects_conflicting_sources_for_one_target(self):
        target = "/media/fat/games/Test/media/screenshot/Game.png"
        with self.assertRaisesRegex(RuntimeError, "Conflicting local images"):
            copy_local_batch([
                ("/tmp/one.png", target, (10, "a" * 64)),
                ("/tmp/two.png", target, (11, "b" * 64)),
            ])


if __name__ == "__main__":
    unittest.main()
