//! Reviewed Degauss-to-ScreenScraper platform mapping.
//!
//! These are ScreenScraper's public numeric platform IDs. An absent entry
//! is intentional: choosing a merely similar platform can attach the wrong
//! game to a ROM, which is worse than reporting that a system is unsupported.

use std::collections::BTreeMap;

/// Resolve an explicit user override first, then the reviewed built-in map.
pub fn id_for(system: &str, overrides: &BTreeMap<String, u32>) -> Option<u32> {
    overrides
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(system))
        .map(|(_, id)| *id)
        .or_else(|| built_in(system))
}

fn built_in(system: &str) -> Option<u32> {
    Some(match system {
        "3DO" => 29,
        "AdventureVision" => 78,
        "Amiga" => 64,
        "AmigaCD32" => 130,
        "Amstrad" => 65,
        "AppleII" => 86,
        "AppleIIGS" => 217,
        // ScreenScraper calls the shared Tandy MC-10 / Matra Alice family
        // "Alice" and lists Degauss's .c10 format for it.
        "AliceMC10" => 311,
        "Arcade" => 75,
        "Arcadia" => 94,
        "Arduboy" => 263,
        "Atari2600" => 26,
        "Atari5200" => 40,
        "Atari7800" => 41,
        "Atari800" => 43,
        "AtariLynx" => 28,
        "AcornAtom" => 36,
        "Astrocade" => 44,
        "BBCMicro" => 37,
        "BK0011M" => 93,
        "CasioPV1000" => 74,
        "CDI" => 133,
        "ChannelF" => 80,
        "ColecoVision" => 48,
        "C16" => 99,
        "C64" => 66,
        "PET2001" => 240,
        "VIC20" => 73,
        "AcornElectron" => 85,
        "FDS" => 106,
        "Gamate" => 266,
        "GameNWatch" => 52,
        "GameGear" | "GameGear2P" => 21,
        "Gameboy" | "Gameboy2P" => 9,
        "GBA" | "GBA2P" => 12,
        "GameboyColor" => 10,
        "Genesis" => 1,
        "Sega32X" => 19,
        "Intellivision" => 115,
        "Jaguar" => 27,
        "JaguarCD" => 171,
        "Jupiter" => 126,
        // These ScreenScraper catalogues currently list no games or ROMs.
        // An explicit user override still takes priority above this map.
        "Laser" | "TomyTutor" => return None,
        "Lynx48" => 88,
        "SordM5" => 323,
        "MacPlus" => 146,
        "Odyssey2" => 104,
        "MasterSystem" => 2,
        "MegaDuck" => 90,
        "MSX" | "MSX1" => 113,
        "NeoGeo" => 142,
        "NeoGeoCD" => 70,
        // ScreenScraper's populated Neo-Geo catalogue contains both AES and
        // MVS software; its separate MVS entry currently has no games or ROMs.
        "NeoGeoMVS" => 142,
        "NeoGeoPocket" => 25,
        "NeoGeoPocketColor" => 82,
        "NES" | "NESMusic" => 3,
        "Nintendo64" => 14,
        "OpenBOR" => 214,
        "Oric" => 131,
        "DOS" => 135,
        "Pico8" => 234,
        "PSX" => 57,
        "PocketChallengeV2" => 237,
        "PokemonMini" => 211,
        "SAMCoupe" => 213,
        "Saturn" => 22,
        "MegaCD" => 20,
        "SG1000" => 109,
        "SNES" | "SNESMusic" => 4,
        "SuperGameboy" => 127,
        "SuperGrafx" => 105,
        "SuperVision" => 207,
        "SVI328" => 218,
        "TI994A" => 205,
        "CoCo2" => 144,
        "ZX81" => 77,
        "TurboGrafx16" => 31,
        "TurboGrafx16CD" => 114,
        "VC4000" => 281,
        "Vectrex" => 102,
        "VirtualBoy" => 11,
        "CreatiVision" => 241,
        "WonderSwan" => 45,
        "WonderSwanColor" => 46,
        "X68000" => 79,
        "ZXSpectrum" => 76,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::path::Path;

    #[test]
    fn representative_public_ids_are_exact() {
        let none = BTreeMap::new();
        assert_eq!(id_for("Genesis", &none), Some(1));
        assert_eq!(id_for("MasterSystem", &none), Some(2));
        assert_eq!(id_for("NES", &none), Some(3));
        assert_eq!(id_for("SNES", &none), Some(4));
        assert_eq!(id_for("Arcade", &none), Some(75));
        assert_eq!(id_for("Amiga", &none), Some(64));
        assert_eq!(id_for("X68000", &none), Some(79));
        assert_eq!(id_for("PocketChallengeV2", &none), Some(237));
        assert_eq!(id_for("Jupiter", &none), Some(126));
        assert_eq!(id_for("NeoGeo", &none), Some(142));
        assert_eq!(id_for("NeoGeoMVS", &none), Some(142));
        assert_eq!(id_for("Lynx48", &none), Some(88));
        assert_eq!(id_for("AliceMC10", &none), Some(311));
    }

    #[test]
    fn an_unreviewed_system_is_not_guessed() {
        assert_eq!(id_for("AppleLisa", &BTreeMap::new()), None);
        assert_eq!(id_for("OtherCores", &BTreeMap::new()), None);
        // This collection contains core builds, not one scrapeable game system.
        assert_eq!(id_for("UnstableCores", &BTreeMap::new()), None);
        assert_eq!(id_for("Laser", &BTreeMap::new()), None);
        assert_eq!(id_for("TomyTutor", &BTreeMap::new()), None);
    }

    #[test]
    fn an_override_is_case_insensitive_and_wins() {
        let overrides = [("genesis".to_string(), 999), ("laser".to_string(), 320)].into();
        assert_eq!(id_for("Genesis", &overrides), Some(999));
        assert_eq!(id_for("Laser", &overrides), Some(320));
    }

    #[test]
    fn every_shipped_system_is_mapped_or_explicitly_unsupported() {
        // A new shipped system must force a deliberate mapping decision.
        // These entries are intentionally unsupported rather than guessed;
        // users can still provide a reviewed numeric override locally.
        let systems = crate::systems::parse_table(
            include_str!("../../assets/systems.toml"),
            Path::new("assets/systems.toml"),
        )
        .unwrap();
        let actual: BTreeSet<_> = systems
            .iter()
            .filter(|system| id_for(&system.id, &BTreeMap::new()).is_none())
            .map(|system| system.id.as_str())
            .collect();
        let expected: BTreeSet<_> = [
            "AmstradPCW",
            "Apogee",
            "AppleI",
            "AppleLisa",
            "Aquarius",
            "Audio",
            "CasioPV2000",
            "Chip8",
            "EDSAC",
            "Favorites",
            "Galaksija",
            "Groovy",
            "Interact",
            "Laser",
            "MultiComp",
            "Orao",
            "OtherCores",
            "PCXT",
            "PDP1",
            "PMD85",
            "QL",
            "RX78",
            "Specialist",
            "TRS80",
            "TSConf",
            "TatungEinstein",
            "TomyTutor",
            "UK101",
            "UnstableCores",
            "UtilityCores",
            "Vector06C",
            "ZXNext",
        ]
        .into_iter()
        .collect();
        assert_eq!(actual, expected);
    }
}
