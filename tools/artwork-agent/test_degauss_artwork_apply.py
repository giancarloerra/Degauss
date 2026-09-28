import hashlib
import unittest
import xml.etree.ElementTree as ET

from degauss_artwork_apply import safe_relative, update_xml


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


if __name__ == "__main__":
    unittest.main()
