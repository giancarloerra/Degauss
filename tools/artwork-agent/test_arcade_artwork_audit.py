"""Regression tests for scoped Arcade artwork planning and XML preservation."""

import hashlib
import tempfile
import unittest
from pathlib import Path

from arcade_artwork_audit import (
    append_entries,
    degauss_lookup,
    latest_mras,
    read_gamelist,
    review_manifest,
)


class ArcadeArtworkAuditTests(unittest.TestCase):
    def test_latest_log_scope_deduplicates_and_rejects_path_escape(self):
        log = "_Arcade/Game.mra\n_Arcade/Game.mra\n_Arcade/cores/Core.rbf\n"
        self.assertEqual(latest_mras(log), ["Game.mra"])
        with self.assertRaisesRegex(ValueError, "Unsafe MRA path"):
            latest_mras("_Arcade/../Elsewhere.mra\n")

    def test_parent_art_and_exact_path_override_filename_fallback(self):
        xml = """<gameList>
          <game id="7"><name>World Game</name><screenshot>./media/screenshot/world.png</screenshot></game>
          <game parentid="7"><path>./_alternatives/Game.mra</path></game>
          <game><path>./Game.mra</path><name>Japan Game</name><screenshot>./media/screenshot/japan.png</screenshot></game>
        </gameList>"""
        _, tables = read_gamelist(xml)
        exact, kind = degauss_lookup("Game.mra", tables)
        self.assertEqual((kind, exact["name"], exact["art"]),
                         ("exact", "Japan Game", "./media/screenshot/japan.png"))
        alternative, kind = degauss_lookup("_alternatives/Game.mra", tables)
        self.assertEqual((kind, alternative["name"], alternative["art"]),
                         ("exact", "World Game", "./media/screenshot/world.png"))
        organized, kind = degauss_lookup("_Organized/Game.mra", tables)
        self.assertEqual((kind, organized["name"]), ("filename", "World Game"))

    def test_path_only_entry_does_not_mask_stem_artwork(self):
        xml = """<gameList>
          <game><path>./Organized/Game.mra</path></game>
          <game><path>./Game.slug</path><screenshot>./media/Game.png</screenshot></game>
        </gameList>"""
        _, tables = read_gamelist(xml)

        entry, match = degauss_lookup("Organized/Game.mra", tables)

        self.assertEqual(match, "slug")
        self.assertEqual(entry["art"], "./media/Game.png")

    def test_slug_lookup_handles_region_tags_but_not_different_volumes(self):
        xml = """<gameList>
          <game><path>./finallap2.slug</path><name>Final Lap 2</name><screenshot>./media/screenshot/finalap2.png</screenshot></game>
        </gameList>"""
        _, tables = read_gamelist(xml)
        self.assertEqual(degauss_lookup("Final Lap 2 (Japan).mra", tables)[1], "slug")
        self.assertEqual(degauss_lookup("Final Lap 3 (Japan).mra", tables)[1], "none")

    def test_append_adds_regional_paths_without_rewriting_existing_xml(self):
        original = "<?xml version='1.0'?><gameList>\n  <game><path>Old.mra</path></game>\n</gameList>\n"
        added = append_entries(original, [{"path": "_alternatives/A&B (Japan).mra",
                                           "name": "A&B (Japan)",
                                           "target": "media/screenshot/ab.png"}])
        self.assertIn(original.split("</gameList>")[0], added)
        entries, tables = read_gamelist(added)
        self.assertEqual(len(entries), 1)
        game, kind = degauss_lookup("_alternatives/A&B (Japan).mra", tables)
        self.assertEqual((kind, game["name"], game["art"]),
                         ("exact", "A&B (Japan)", "./media/screenshot/ab.png"))

    def test_manifest_count_mismatch_stops_before_card_write(self):
        with tempfile.TemporaryDirectory() as directory:
            image = Path(directory) / "game.png"
            image.write_bytes(b"\x89PNG\r\n\x1a\n" + b"x" * 120)
            log = b"_Arcade/Game.mra\n"
            xml = b"<gameList></gameList>"
            manifest = {
                "log_sha256": hashlib.sha256(log).hexdigest(),
                "gamelist_sha256": hashlib.sha256(xml).hexdigest(),
                "expected_total_new_entries": 2,
                "groups": [{"setnames": ["game"], "image": str(image),
                            "target": "media/screenshot/game.png",
                            "sha256": hashlib.sha256(image.read_bytes()).hexdigest(),
                            "expected_missing_paths": 2}],
            }
            item = {"path": "Game.mra", "name": "Game", "setname": "game", "exists": True,
                    "parse": "xml", "match": "none", "art_valid": False}
            with self.assertRaisesRegex(ValueError, "Scope changed"):
                review_manifest(manifest, {"items": [item]}, log, xml)

    def test_existing_entry_edit_keeps_metadata_and_rejects_duplicate_path(self):
        original = "<gameList><game id='parent'><name>Original</name></game>\n" \
                   "<game parentid='parent'><path>./_Organized/Region.mra</path>" \
                   "<desc>Keep &amp; verify</desc></game></gameList>"
        selected = [{"path": "_Organized/Region.mra", "target": "media/screenshot/region.png",
                     "operation": "edit"}]
        updated = append_entries(original, selected)
        self.assertIn("<desc>Keep &amp; verify</desc><screenshot>./media/screenshot/region.png</screenshot>", updated)
        self.assertEqual(updated.count("<game "), original.count("<game "))
        _, tables = read_gamelist(updated)
        entry, kind = degauss_lookup("_Organized/Region.mra", tables)
        self.assertEqual((kind, entry["art"], entry["parentid"]),
                         ("exact", "./media/screenshot/region.png", "parent"))
        duplicated = original.replace("</gameList>",
                                      "<game><path>./_Organized/Region.mra</path></game></gameList>")
        with self.assertRaisesRegex(ValueError, "absent or duplicated"):
            append_entries(duplicated, selected)

    def test_review_skips_already_valid_filename_fallback(self):
        with tempfile.TemporaryDirectory() as directory:
            image = Path(directory) / "game.png"
            image.write_bytes(b"\x89PNG\r\n\x1a\n" + b"x" * 120)
            log = b"_Arcade/Game.mra\n"
            xml = b"<gameList></gameList>"
            manifest = {
                "log_sha256": hashlib.sha256(log).hexdigest(),
                "gamelist_sha256": hashlib.sha256(xml).hexdigest(),
                "expected_total_new_entries": 1,
                "groups": [{"setnames": ["game"], "image": str(image),
                            "target": "media/screenshot/game.png",
                            "sha256": hashlib.sha256(image.read_bytes()).hexdigest(),
                            "expected_missing_paths": 1}],
            }
            items = [
                {"path": "Game.mra", "name": "Game", "setname": "game", "exists": True,
                 "parse": "xml", "match": "none", "art_valid": False},
                {"path": "_Organized/Game.mra", "name": "Game", "setname": "game", "exists": True,
                 "parse": "xml", "match": "filename", "art_valid": True},
            ]
            selected, _ = review_manifest(manifest, {"items": items}, log, xml)
            self.assertEqual([item["path"] for item in selected], ["Game.mra"])


if __name__ == "__main__":
    unittest.main()
