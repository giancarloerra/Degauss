import unittest
import xml.etree.ElementTree as ET

import arcade_artwork_source as source


class SourceMatchingTests(unittest.TestCase):
    def test_regional_revisions_share_only_their_public_title(self):
        items = [
            {"path": "Final Lap.mra", "setname": "finallap", "name": "Final Lap (Rev E)", "exists": True, "art_valid": False},
            {"path": "_alternatives/Final Lap Japan.mra", "setname": "finallapjb", "name": "Final Lap (Japan, Rev B)", "exists": True, "art_valid": False},
            {"path": "Final Lap 3.mra", "setname": "finalap3", "name": "Final Lap 3 (World)", "exists": True, "art_valid": False},
        ]
        groups = source.grouped_gaps(items)
        self.assertEqual(groups, [
            {"title": "Final Lap", "setnames": ["finallap", "finallapjb"]},
            {"title": "Final Lap 3", "setnames": ["finalap3"]},
        ])

    def test_trivial_pursuit_editions_remain_distinct(self):
        items = [
            {"path": "a.mra", "setname": "triviaes", "name": "Trivial Pursuit (Volumen III)", "exists": True, "art_valid": False},
            {"path": "b.mra", "setname": "triviaes2", "name": "Trivial Pursuit (Volumen II)", "exists": True, "art_valid": False},
        ]
        self.assertEqual(len(source.grouped_gaps(items)), 2)

    def test_search_excludes_nearby_but_distinct_games(self):
        root = ET.fromstring("<Data><jeux><jeu id='1'><noms><nom>Assault</nom></noms></jeu><jeu id='2'><noms><nom>Ninja Assault</nom></noms></jeu></jeux></Data>")
        self.assertEqual([x.get("id") for x in source.exact_games(root, "Assault")], ["1"])

    def test_authenticated_search_never_follows_redirects(self):
        self.assertIsNone(source.NoRedirect().redirect_request(None, None, 302, "redirect", {}, "https://elsewhere.invalid"))


if __name__ == "__main__":
    unittest.main()
