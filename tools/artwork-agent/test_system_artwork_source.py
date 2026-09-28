import unittest

from system_artwork_source import (
    git_blob_sha1,
    groups_for,
    libretro_filename_key,
    libretro_manifest,
    select_libretro_candidates,
)


class SystemArtworkSourceTests(unittest.TestCase):
    def test_groups_regional_variants_by_public_identity(self):
        audit = {"unresolved": [
            {"title": "Game", "path": "USA/Game (USA).sfc"},
            {"title": "Game", "path": "Europe/Game (Europe).sfc"},
            {"title": "Game 2", "path": "USA/Game II (USA).sfc"},
        ]}
        groups = groups_for(audit)
        self.assertEqual(len(groups), 2)
        self.assertEqual(groups[0]["paths"], ["Europe/Game (Europe).sfc", "USA/Game (USA).sfc"])
        self.assertEqual(groups[1]["paths"], ["USA/Game II (USA).sfc"])

    def test_dotted_titles_remain_separate(self):
        audit = {"unresolved": [
            {"title": "Dr. J", "path": "Dr. J.crt"},
            {"title": "Dr. Who and the Mines of Terror", "path": "Dr. Who.d64"},
            {"title": "Mr. Do!", "path": "Mr. Do! (USA).sfc"},
            {"title": "Mr. Nutz", "path": "Mr. Nutz (USA).sfc"},
        ]}
        groups = groups_for(audit)
        self.assertEqual([group["title"] for group in groups], [
            "Dr. J", "Dr. Who and the Mines of Terror", "Mr. Do!", "Mr. Nutz",
        ])

    def test_libretro_invalid_characters_follow_repository_naming(self):
        self.assertEqual(libretro_filename_key('Q*Bert: Test'), libretro_filename_key('Q_Bert_ Test'))

    def test_libretro_prefers_exact_filename_and_requires_prior_miss(self):
        audit = {
            "system": "SNES",
            "unresolved": [
                {"title": "Game", "path": "USA/Game (USA).sfc"},
                {"title": "Other", "path": "USA/Other (USA).sfc"},
            ],
        }
        screen = [
            {"title": "Game", "status": "no_exact_title"},
            {"title": "Other", "status": "candidate"},
        ]
        entries = [
            {"path": "Named_Snaps/Game (USA).png", "blob_sha1": "a" * 40},
            {"path": "Named_Snaps/Other (USA).png", "blob_sha1": "b" * 40},
        ]
        candidates, summary = select_libretro_candidates(audit, screen, entries)
        self.assertEqual(len(candidates), 1)
        self.assertEqual(candidates[0]["operations"][0]["match_kind"], "exact_rom_filename")
        self.assertEqual(summary["ineligible_screenscraper_status"], 1)

    def test_libretro_identity_match_requires_one_unique_blob(self):
        audit = {"system": "Amiga", "unresolved": [
            {"title": "Hover Sprint", "path": "Games/Hover Sprint (OCS)[en]"},
            {"title": "Battleship", "path": "Games/Battleship (OCS)[en]"},
        ]}
        screen = [
            {"title": "Hover Sprint", "status": "no_exact_title"},
            {"title": "Battleship", "status": "no_screenshot"},
        ]
        entries = [
            {"path": "Named_Snaps/Hoversprint.png", "blob_sha1": "a" * 40},
            {"path": "Named_Snaps/Battleship (USA).png", "blob_sha1": "b" * 40},
            {"path": "Named_Snaps/Battleship (Europe).png", "blob_sha1": "c" * 40},
        ]
        candidates, summary = select_libretro_candidates(audit, screen, entries)
        self.assertEqual(len(candidates), 1)
        self.assertEqual(candidates[0]["operations"][0]["title"], "Hover Sprint")
        self.assertEqual(summary["ambiguous_libretro_match"], 1)

    def test_git_blob_sha1(self):
        self.assertEqual(git_blob_sha1(b"test\n"), "9daeafb9864cf43055ae93beb0afd6c7d144bfa4")

    def test_libretro_manifest_requires_complete_visual_review(self):
        audit = {
            "system": "SNES", "root": "/media/fat/games/SNES",
            "gamelist_sha256": "0" * 64,
        }
        value = {"repository": "repo", "commit": "1" * 40, "records": [{
            "status": "candidate", "blob_sha1": "a" * 40,
            "tree_path": "Named_Snaps/Game.png", "image": __file__,
            "bytes": 1, "sha256": "0" * 64, "operations": [],
        }]}
        with self.assertRaisesRegex(ValueError, "does not account"):
            libretro_manifest(audit, value, {"approved_blob_sha1": [], "rejected": {}})


if __name__ == "__main__":
    unittest.main()
