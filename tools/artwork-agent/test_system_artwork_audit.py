import unittest

from system_artwork_audit import identity_key, metadata_tag, public_title, uncertain


class SystemArtworkAuditTests(unittest.TestCase):
    def test_public_title_strips_only_known_release_metadata(self):
        self.assertEqual(public_title("Duel, The - Test Drive II (USA) (Rev 1).sfc"), "Duel, The - Test Drive II")
        self.assertEqual(public_title("Rugrats in Paris - The Movie (USA).z64"), "Rugrats in Paris - The Movie")
        self.assertEqual(public_title("Captured Dreams (The Black Lotus)(AGA)"), "Captured Dreams (The Black Lotus)")
        self.assertEqual(public_title("Dr. Mario BS (3-3)"), "Dr. Mario BS (3-3)")
        self.assertEqual(public_title("Mr. Do!"), "Mr. Do!")

    def test_identity_normalizes_articles_punctuation_and_roman_numbers(self):
        self.assertEqual(identity_key("Addams Family, The (USA).sfc"), identity_key("The Addams Family"))
        self.assertEqual(identity_key("Bubsy II (USA).sfc"), identity_key("Bubsy 2"))
        self.assertNotEqual(identity_key("Dr. J"), identity_key("Dr. Who and the Mines of Terror"))
        self.assertNotEqual(identity_key("Mr. Do!"), identity_key("Mr. Nutz"))

    def test_uncertain_variants_do_not_confuse_title_words_with_metadata(self):
        self.assertFalse(uncertain("Duel, The - Test Drive II (USA).sfc"))
        self.assertTrue(uncertain("Betas/Game (USA) (Beta).sfc"))
        self.assertTrue(uncertain("Game [h1].sfc"))
        self.assertTrue(uncertain("Games/Kaboomania Demo (OCS)[en]"))
        self.assertTrue(uncertain("Game Prototype.sfc"))
        self.assertFalse(uncertain("Game [Side B].d64"))
        self.assertFalse(uncertain("Demolition Man (USA).sfc"))

    def test_metadata_tags_are_narrow(self):
        self.assertTrue(metadata_tag("USA, Europe"))
        self.assertTrue(metadata_tag("En,Fr,De"))
        self.assertTrue(metadata_tag("Disc 2"))
        self.assertFalse(metadata_tag("The Movie"))


if __name__ == "__main__":
    unittest.main()
