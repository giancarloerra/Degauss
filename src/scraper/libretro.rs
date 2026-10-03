//! Account-free, explicit-use Libretro database and thumbnail lookup.
//!
//! The RDB container is read independently with an MIT MessagePack reader.
//! No RetroArch scanner or thumbnail-downloader implementation is included.

use std::collections::{HashMap, HashSet};
use std::io::Cursor;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use rmpv::ValueRef;
use serde::{Deserialize, Serialize};

use super::api::{HttpResponse, Lookup, LookupResponse, Transport};
use super::{hashes::Hashes, Error, ErrorKind, Match, Media, Metadata, Result};

const MAX_DATABASE_BYTES: u64 = 32 * 1024 * 1024;
const DATABASE_BASE: &str =
    "https://raw.githubusercontent.com/libretro/libretro-database/master/rdb";
const THUMBNAIL_BASE: &str = "https://thumbnails.libretro.com";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Artwork {
    #[default]
    Screenshot,
    BoxArt,
    TitleScreen,
}

impl Artwork {
    pub const ALL: [Self; 3] = [Self::Screenshot, Self::BoxArt, Self::TitleScreen];

    pub fn label(self) -> &'static str {
        match self {
            Self::Screenshot => "Screenshot",
            Self::BoxArt => "Box Art",
            Self::TitleScreen => "Title Screen",
        }
    }

    pub fn step(self, delta: isize) -> Self {
        let at = Self::ALL.iter().position(|value| *value == self).unwrap();
        Self::ALL[(at as isize + delta).rem_euclid(Self::ALL.len() as isize) as usize]
    }

    fn directory(self) -> &'static str {
        match self {
            Self::Screenshot => "Named_Snaps",
            Self::BoxArt => "Named_Boxarts",
            Self::TitleScreen => "Named_Titles",
        }
    }
}

// Explicit Degauss identities and reviewed public RDB names. The numeric key
// is used only by the shared target collector to distinguish database aliases.
const PLATFORMS: &[(&[&str], u32, &str)] = &[
    (&["3DO"], 1001, "The 3DO Company - 3DO"),
    (&["AdventureVision"], 1002, "Entex - Adventure Vision"),
    (&["Amiga"], 1003, "Commodore - Amiga"),
    (&["AmigaCD32"], 1004, "Commodore - CD32"),
    (&["Amstrad"], 1005, "Amstrad - CPC"),
    (&["AppleII"], 1006, "Apple - II"),
    (&["Arcade"], 1007, "MAME"),
    (&["Arcadia"], 1008, "Emerson - Arcadia 2001"),
    (&["Arduboy"], 1009, "Arduboy Inc - Arduboy"),
    (&["Atari2600"], 1010, "Atari - 2600"),
    (&["Atari5200"], 1011, "Atari - 5200"),
    (&["Atari7800"], 1012, "Atari - 7800"),
    (&["Atari800"], 1013, "Atari - 8-bit Family"),
    (&["AtariLynx"], 1014, "Atari - Lynx"),
    (&["CasioPV1000"], 1015, "Casio - PV-1000"),
    (&["ChannelF"], 1016, "Fairchild - Channel F"),
    (&["Chip8"], 1017, "CHIP-8"),
    (&["ColecoVision"], 1018, "Coleco - ColecoVision"),
    (&["C16"], 1019, "Commodore - Plus-4"),
    (&["C64"], 1020, "Commodore - 64"),
    (&["PET2001"], 1021, "Commodore - PET"),
    (&["VIC20"], 1022, "Commodore - VIC-20"),
    (&["FDS"], 1023, "Nintendo - Family Computer Disk System"),
    (&["GameNWatch"], 1024, "Handheld Electronic Game"),
    (&["GameGear", "GameGear2P"], 1025, "Sega - Game Gear"),
    (
        &["Gameboy", "Gameboy2P", "SuperGameboy"],
        1026,
        "Nintendo - Game Boy",
    ),
    (&["GameboyColor"], 1027, "Nintendo - Game Boy Color"),
    (&["GBA", "GBA2P"], 1028, "Nintendo - Game Boy Advance"),
    (&["Genesis"], 1029, "Sega - Mega Drive - Genesis"),
    (&["Sega32X"], 1030, "Sega - 32X"),
    (&["Intellivision"], 1031, "Mattel - Intellivision"),
    (&["Jaguar"], 1032, "Atari - Jaguar"),
    (&["Odyssey2"], 1033, "Magnavox - Odyssey2"),
    (&["MasterSystem"], 1034, "Sega - Master System - Mark III"),
    (&["MSX", "MSX1"], 1035, "Microsoft - MSX"),
    (&["NeoGeoCD"], 1036, "SNK - Neo Geo CD"),
    (&["NeoGeoPocket"], 1037, "SNK - Neo Geo Pocket"),
    (&["NeoGeoPocketColor"], 1038, "SNK - Neo Geo Pocket Color"),
    (&["NES"], 1039, "Nintendo - Nintendo Entertainment System"),
    (&["Nintendo64"], 1040, "Nintendo - Nintendo 64"),
    (&["DOS", "PCXT"], 1041, "DOS"),
    (&["PSX"], 1042, "Sony - PlayStation"),
    (&["PokemonMini"], 1043, "Nintendo - Pokemon Mini"),
    (&["Saturn"], 1044, "Sega - Saturn"),
    (&["MegaCD"], 1045, "Sega - Mega-CD - Sega CD"),
    (&["SG1000"], 1046, "Sega - SG-1000"),
    (
        &["SNES"],
        1047,
        "Nintendo - Super Nintendo Entertainment System",
    ),
    (&["SuperGrafx"], 1048, "NEC - PC Engine SuperGrafx"),
    (&["SuperVision"], 1049, "Watara - Supervision"),
    (&["TurboGrafx16"], 1050, "NEC - PC Engine - TurboGrafx 16"),
    (
        &["TurboGrafx16CD"],
        1051,
        "NEC - PC Engine CD - TurboGrafx-CD",
    ),
    (&["VC4000"], 1052, "Interton - VC 4000"),
    (&["Vectrex"], 1053, "GCE - Vectrex"),
    (&["VirtualBoy"], 1054, "Nintendo - Virtual Boy"),
    (&["WonderSwan"], 1055, "Bandai - WonderSwan"),
    (&["WonderSwanColor"], 1056, "Bandai - WonderSwan Color"),
    (&["X68000"], 1057, "Sharp - X68000"),
    (&["ZXSpectrum"], 1058, "Sinclair - ZX Spectrum"),
    (&["NeoGeo", "NeoGeoMVS"], 1059, "SNK - Neo Geo"),
];

pub fn platform(system_id: &str) -> Option<(u32, &'static str)> {
    PLATFORMS.iter().find_map(|(ids, key, database)| {
        ids.iter()
            .any(|id| id.eq_ignore_ascii_case(system_id))
            .then_some((*key, *database))
    })
}

pub fn database_for_key(key: u32) -> Option<&'static str> {
    PLATFORMS
        .iter()
        .find(|(_, id, _)| *id == key)
        .map(|(_, _, name)| *name)
}

fn malformed(detail: impl std::fmt::Display) -> Error {
    Error::new(
        ErrorKind::MalformedResponse,
        format!("Libretro database: {detail}"),
    )
}

fn check_cancelled(cancelled: &AtomicBool) -> Result<()> {
    if cancelled.load(Ordering::Relaxed) {
        Err(Error::new(
            ErrorKind::Cancelled,
            "Libretro scrape cancelled",
        ))
    } else {
        Ok(())
    }
}

fn field<'a>(record: &'a ValueRef<'a>, key: &str) -> Option<&'a ValueRef<'a>> {
    let ValueRef::Map(fields) = record else {
        return None;
    };
    fields
        .iter()
        .find(|(name, _)| string(name) == Some(key))
        .map(|(_, value)| value)
}

fn string<'a>(value: &'a ValueRef<'a>) -> Option<&'a str> {
    match value {
        ValueRef::String(text) => text.as_str(),
        _ => None,
    }
}

fn text(record: &ValueRef<'_>, key: &str) -> Option<String> {
    let value = field(record, key)?;
    match value {
        ValueRef::Array(values) => {
            let text = values
                .iter()
                .filter_map(string)
                .collect::<Vec<_>>()
                .join(", ");
            (!text.is_empty()).then_some(text)
        }
        _ => string(value)
            .filter(|text| !text.trim().is_empty())
            .map(str::to_string),
    }
}

fn fingerprint(record: &ValueRef<'_>, key: &str, bytes: usize) -> Result<Option<String>> {
    let Some(value) = field(record, key) else {
        return Ok(None);
    };
    match value {
        ValueRef::Binary(data) if data.len() == bytes => Ok(Some(hex(data))),
        _ => Err(malformed(format!("invalid {key} fingerprint"))),
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(value, "{byte:02X}");
    }
    value
}

fn year_date(value: &ValueRef<'_>) -> Option<String> {
    let year = field(value, "releaseyear")?
        .as_u64()
        .filter(|year| (1..=9999).contains(year))?;
    let mut date = format!("{year:04}");
    if let Some(month) = field(value, "releasemonth")
        .and_then(ValueRef::as_u64)
        .filter(|month| (1..=12).contains(month))
    {
        date.push_str(&format!("{month:02}"));
        if let Some(day) = field(value, "releaseday")
            .and_then(ValueRef::as_u64)
            .filter(|day| (1..=31).contains(day))
        {
            date.push_str(&format!("{day:02}"));
        }
    }
    Some(date)
}

pub struct Database {
    pub name: String,
    records: Vec<Match>,
    fingerprints: HashMap<String, Vec<usize>>,
    names: HashMap<String, Vec<usize>>,
    short_names: HashMap<String, Option<String>>,
}

impl Database {
    pub fn parse(name: &str, bytes: &[u8], cancelled: &AtomicBool) -> Result<Self> {
        if bytes.len() < 17
            || bytes.len() as u64 > MAX_DATABASE_BYTES
            || &bytes[..8] != b"RARCHDB\0"
        {
            return Err(malformed("invalid RDB header or size"));
        }
        let offset = usize::try_from(u64::from_be_bytes(bytes[8..16].try_into().unwrap()))
            .map_err(|_| malformed("metadata offset is out of range"))?;
        if !(17..bytes.len()).contains(&offset) {
            return Err(malformed("metadata offset is outside the database"));
        }
        let metadata = rmpv::decode::read_value_ref_with_max_depth(&mut &bytes[offset..], 16)
            .map_err(malformed)?;
        let count = field(&metadata, "count")
            .and_then(ValueRef::as_u64)
            .ok_or_else(|| malformed("missing record count"))?;
        if count > (offset - 16) as u64 {
            return Err(malformed("record count exceeds database size"));
        }
        let mut reader = Cursor::new(&bytes[16..offset]);
        let mut database = Self {
            name: name.into(),
            records: Vec::new(),
            fingerprints: HashMap::new(),
            names: HashMap::new(),
            short_names: HashMap::new(),
        };
        for _ in 0..count {
            check_cancelled(cancelled)?;
            let value =
                rmpv::decode::read_value_ref_with_max_depth(&mut reader, 16).map_err(malformed)?;
            if !matches!(value, ValueRef::Map(_)) {
                return Err(malformed("record is not a map"));
            }
            if field(&value, "name").is_some_and(|name| string(name).is_none()) {
                return Err(malformed("record title is not valid UTF-8 text"));
            }
            let crc = fingerprint(&value, "crc", 4)?;
            let md5 = fingerprint(&value, "md5", 16)?;
            let sha1 = fingerprint(&value, "sha1", 20)?;
            let title = text(&value, "name");
            if title.is_none()
                && sha1.is_none()
                && md5.is_none()
                && crc.is_none()
                && matches!(field(&value, "serial"), Some(ValueRef::Binary(serial)) if !serial.is_empty())
            {
                // Real disc databases contain serial-only controller flags.
                // This source intentionally does not identify discs by serial.
                // Keep validating the container, without indexing these as games.
                continue;
            }
            // Public databases contain hash-only metadata records too. They
            // can supply the fields they have after an exact fingerprint
            // match, but cannot invent a title or identify thumbnail artwork.
            let identity = title
                .as_deref()
                .or(sha1.as_deref())
                .or(md5.as_deref())
                .or(crc.as_deref())
                .ok_or_else(|| malformed("record has neither a title nor a fingerprint"))?;
            let id = format!("libretro:{name}:{identity}");
            let at = database.records.len();
            let metadata = Metadata {
                name: title.clone(),
                desc: text(&value, "description")
                    .filter(|description| Some(description) != title.as_ref()),
                publisher: text(&value, "publisher"),
                developer: text(&value, "developer"),
                releasedate: year_date(&value),
                players: field(&value, "maxusers")
                    .or_else(|| field(&value, "users"))
                    .and_then(ValueRef::as_u64)
                    .map(|players| players.to_string()),
                genre: text(&value, "genre"),
                lang: text(&value, "language"),
            };
            let matched = Match {
                id,
                name: title.clone().unwrap_or_default(),
                names: title.iter().cloned().collect(),
                metadata,
                media: None,
                rom_crc32: crc,
                rom_md5: md5,
                rom_sha1: sha1,
            };
            for (prefix, hash) in [
                ("crc", &matched.rom_crc32),
                ("md5", &matched.rom_md5),
                ("sha1", &matched.rom_sha1),
            ] {
                if let Some(hash) = hash {
                    database
                        .fingerprints
                        .entry(format!("{prefix}:{hash}"))
                        .or_default()
                        .push(at);
                }
            }
            if let Some(title) = &title {
                database
                    .names
                    .entry(exact_name(title))
                    .or_default()
                    .push(at);
                database
                    .short_names
                    .entry(short_name(title))
                    .and_modify(|existing| {
                        if existing.as_deref() != Some(title) {
                            *existing = None;
                        }
                    })
                    .or_insert_with(|| Some(title.clone()));
            }
            database.records.push(matched);
        }
        if rmpv::decode::read_value_ref_with_max_depth(&mut reader, 16).map_err(malformed)?
            != ValueRef::Nil
            || reader.position() != (offset - 16) as u64
        {
            return Err(malformed("record count or end marker does not match"));
        }
        Ok(database)
    }

    pub fn load(
        name: &str,
        cache_dir: &Path,
        transport: &dyn Transport,
        cancelled: &AtomicBool,
        activity: &mut dyn FnMut(&str),
    ) -> Result<Self> {
        check_cancelled(cancelled)?;
        let path = cache_dir.join("libretro").join(format!("{name}.rdb"));
        match std::fs::File::open(&path) {
            Ok(file) => {
                use std::io::Read;
                activity("Reading cached Libretro database");
                let mut bytes = Vec::new();
                file.take(MAX_DATABASE_BYTES + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|error| {
                        Error::local(format!("could not read cached Libretro database: {error}"))
                    })?;
                Self::parse(name, &bytes, cancelled)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                activity("Downloading Libretro database");
                let response = transport.get_libretro(
                    &format!("{DATABASE_BASE}/{}.rdb", encode(name)),
                    MAX_DATABASE_BYTES,
                )?;
                let response = checked_response(response, "database")?;
                let database = Self::parse(name, &response.body, cancelled)?;
                check_cancelled(cancelled)?;
                super::config::atomic_write(&path, &response.body, "Libretro database")?;
                Ok(database)
            }
            Err(error) => Err(Error::local(format!(
                "could not open cached Libretro database: {error}"
            ))),
        }
    }

    pub fn lookup(&self, title: &str, hashes: Option<&Hashes>, artwork: Artwork) -> LookupResponse {
        if let Some(hashes) = hashes {
            for (prefix, hash) in [
                ("sha1", &hashes.sha1),
                ("md5", &hashes.md5),
                ("crc", &hashes.crc32),
            ] {
                if let Some(indices) = self.fingerprints.get(&format!("{prefix}:{hash}")) {
                    let matches = self.matches(
                        indices.iter().copied().filter(|at| {
                            let record = &self.records[*at];
                            [&record.rom_sha1, &record.rom_md5, &record.rom_crc32]
                                .into_iter()
                                .zip([&hashes.sha1, &hashes.md5, &hashes.crc32])
                                .all(|(stored, actual)| {
                                    stored.as_ref().is_none_or(|stored| stored == actual)
                                })
                        }),
                        artwork,
                    );
                    if !matches.is_empty() {
                        return lookup_response(matches);
                    }
                }
            }
        }
        let exact = self
            .names
            .get(&exact_name(title))
            .map(|indices| self.matches(indices.iter().copied(), artwork))
            .unwrap_or_default();
        if !exact.is_empty() {
            // A known fingerprint that disagrees with the database must not
            // silently borrow another revision's metadata from its filename.
            return if hashes.is_some() {
                LookupResponse {
                    lookup: Lookup::Ambiguous(exact.len()),
                    alternatives: exact,
                    account: None,
                    server_miss: false,
                }
            } else {
                lookup_response(exact)
            };
        }
        // Region/revision tags stay in exact keys. A shortened title may show
        // candidates for confirmation, but is never an automatic match.
        let candidates = self.search(title, artwork);
        LookupResponse {
            lookup: if candidates.is_empty() {
                Lookup::NotFound
            } else {
                Lookup::Ambiguous(candidates.len())
            },
            alternatives: candidates,
            account: None,
            server_miss: false,
        }
    }

    fn matches(&self, indices: impl Iterator<Item = usize>, artwork: Artwork) -> Vec<Match> {
        let mut seen = HashSet::new();
        indices
            .filter_map(|at| {
                let record = &self.records[at];
                if !seen.insert(record.id.clone()) {
                    return None;
                }
                let mut matched = record.clone();
                if record.name.is_empty() {
                    return Some(matched);
                }
                let short = record.name.split('(').next().unwrap_or(&record.name).trim();
                if short != record.name
                    && self
                        .short_names
                        .get(&short_name(&record.name))
                        .and_then(Option::as_deref)
                        == Some(&record.name)
                {
                    matched.names.push(short.to_string());
                }
                matched.media = Some(Media {
                    url: thumbnail_url(&self.name, artwork, &record.name),
                    format: Some("png".into()),
                });
                Some(matched)
            })
            .collect()
    }

    pub fn search(&self, title: &str, artwork: Artwork) -> Vec<Match> {
        let key = short_name(title);
        if key.is_empty() {
            return Vec::new();
        }
        self.matches(
            self.records
                .iter()
                .enumerate()
                .filter(|(_, record)| short_name(&record.name).contains(&key))
                .map(|(at, _)| at),
            artwork,
        )
    }
}

fn lookup_response(matches: Vec<Match>) -> LookupResponse {
    let lookup = match matches.len() {
        0 => Lookup::NotFound,
        1 => Lookup::Found(Box::new(matches[0].clone())),
        count => Lookup::Ambiguous(count),
    };
    LookupResponse {
        lookup,
        alternatives: matches,
        account: None,
        server_miss: false,
    }
}

fn exact_name(title: &str) -> String {
    title
        .chars()
        .filter(|value| value.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn short_name(title: &str) -> String {
    exact_name(title.split(['(', '[']).next().unwrap_or(title))
}

pub fn thumbnail_url(database: &str, artwork: Artwork, title: &str) -> String {
    let filename = thumbnail_filename(title);
    format!(
        "{THUMBNAIL_BASE}/{}/{}/{}.png",
        encode(database),
        artwork.directory(),
        encode(&filename)
    )
}

/// Only confirmed names for the selected database entry are tried. The
/// shortened name is supplied by Database only when it identifies one entry.
pub fn thumbnail_candidates(matched: &Match, filename: Option<&str>) -> Vec<String> {
    let Some(media) = &matched.media else {
        return Vec::new();
    };
    let Some((directory, _)) = media.url.rsplit_once('/') else {
        return Vec::new();
    };
    let mut urls = vec![media.url.clone()];
    let names = filename
        .filter(|name| exact_name(name) == exact_name(&matched.name))
        .into_iter()
        .chain(matched.names.iter().map(String::as_str));
    for name in names {
        let name = thumbnail_filename(name);
        let url = format!("{directory}/{}.png", encode(&name));
        if !urls.contains(&url) {
            urls.push(url);
        }
    }
    urls
}

fn thumbnail_filename(title: &str) -> String {
    title
        .chars()
        .map(|character| {
            if "&*/:`<>?\\|\"".contains(character) {
                '_'
            } else {
                character
            }
        })
        .collect()
}

pub fn download_image(
    urls: &[String],
    transport: &dyn Transport,
    limit: u64,
    cancelled: &AtomicBool,
) -> Result<HttpResponse> {
    for url in urls {
        check_cancelled(cancelled)?;
        let response = checked_response(transport.get_libretro(url, limit)?, "image");
        match response {
            Err(error) if error.kind == ErrorKind::NotFound => {}
            result => return result,
        }
    }
    Err(Error::new(
        ErrorKind::NotFound,
        "No Libretro image exists for the confirmed game names",
    ))
}

pub fn checked_response(response: HttpResponse, operation: &str) -> Result<HttpResponse> {
    let kind = match response.status {
        200 => return Ok(response),
        404 => ErrorKind::NotFound,
        408 | 504 => ErrorKind::Timeout,
        429 => ErrorKind::RateLimited,
        500..=599 => ErrorKind::Server,
        _ => ErrorKind::InvalidRequest,
    };
    Err(Error::new(
        kind,
        format!("Libretro {operation} returned HTTP {}", response.status),
    ))
}

fn encode(text: &str) -> String {
    use std::fmt::Write;
    let mut encoded = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(byte as char);
        } else {
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use rmpv::Value;

    pub(crate) fn fixture(records: Vec<Value>) -> Vec<u8> {
        let count = records.len();
        let mut bytes = b"RARCHDB\0".to_vec();
        bytes.extend_from_slice(&[0; 8]);
        for record in records {
            rmpv::encode::write_value(&mut bytes, &record).unwrap();
        }
        rmpv::encode::write_value(&mut bytes, &Value::Nil).unwrap();
        let offset = bytes.len() as u64;
        bytes[8..16].copy_from_slice(&offset.to_be_bytes());
        rmpv::encode::write_value(
            &mut bytes,
            &Value::Map(vec![("count".into(), (count as u64).into())]),
        )
        .unwrap();
        bytes
    }

    pub(crate) fn record(title: &str) -> Value {
        Value::Map(vec![
            ("name".into(), title.into()),
            ("publisher".into(), "Example Publisher".into()),
            ("releaseyear".into(), 1992.into()),
        ])
    }

    fn database(records: Vec<Value>) -> Database {
        Database::parse(
            "Nintendo - Nintendo Entertainment System",
            &fixture(records),
            &AtomicBool::new(false),
        )
        .unwrap()
    }

    #[test]
    fn rdb_reads_only_present_metadata_and_valid_fingerprints() {
        let Value::Map(mut fields) = record("A Game (USA)") else {
            unreachable!()
        };
        fields.extend([
            ("crc".into(), Value::Binary(vec![0x12, 0x34, 0x56, 0x78])),
            (
                "developer".into(),
                Value::Array(vec!["First".into(), "Second".into()]),
            ),
            ("description".into(), "A Game (USA)".into()),
            ("genre".into(), "Action".into()),
            ("maxusers".into(), 2.into()),
        ]);
        let db = database(vec![Value::Map(fields)]);
        let matched = &db.records[0];
        assert_eq!(matched.rom_crc32.as_deref(), Some("12345678"));
        assert_eq!(matched.metadata.developer.as_deref(), Some("First, Second"));
        assert_eq!(matched.metadata.releasedate.as_deref(), Some("1992"));
        assert_eq!(matched.metadata.players.as_deref(), Some("2"));
        assert!(matched.metadata.desc.is_none());
        assert!(matched.metadata.lang.is_none());
    }

    #[test]
    fn hash_only_upstream_records_supply_metadata_without_inventing_a_name_or_image() {
        let db = database(vec![Value::Map(vec![
            ("crc".into(), Value::Binary(vec![0x12, 0x34, 0x56, 0x78])),
            ("publisher".into(), "Activision".into()),
            ("users".into(), 1.into()),
        ])]);
        let hashes = Hashes {
            crc32: "12345678".into(),
            md5: "00".repeat(16),
            sha1: "00".repeat(20),
            size: 32,
        };
        let Lookup::Found(matched) = db
            .lookup("Unknown local name", Some(&hashes), Artwork::Screenshot)
            .lookup
        else {
            panic!("hash-only metadata was not matched");
        };
        assert!(matched.metadata.name.is_none());
        assert!(matched.media.is_none());
        assert!(matched.names.is_empty());
        assert_eq!(matched.metadata.publisher.as_deref(), Some("Activision"));
        assert_eq!(matched.metadata.players.as_deref(), Some("1"));
        assert!(db.search("Activision", Artwork::Screenshot).is_empty());
    }

    #[test]
    fn serial_only_disc_records_are_valid_but_are_not_name_or_rom_fingerprint_matches() {
        let db = database(vec![
            record("Game (USA)"),
            Value::Map(vec![
                ("serial".into(), Value::Binary(b"SLUS-90066".to_vec())),
                ("rumble".into(), 1.into()),
            ]),
        ]);
        assert_eq!(db.records.len(), 1);
        assert!(matches!(
            db.lookup("Game (USA)", None, Artwork::Screenshot).lookup,
            Lookup::Found(_)
        ));
        assert!(matches!(
            db.lookup("SLUS-90066", None, Artwork::Screenshot).lookup,
            Lookup::NotFound
        ));
    }

    #[test]
    fn renamed_rom_is_identified_by_crc_and_conflicting_hash_is_not_accepted_by_name() {
        let Value::Map(mut fields) = record("Original (Europe)") else {
            unreachable!()
        };
        fields.push(("crc".into(), Value::Binary(vec![0x12, 0x34, 0x56, 0x78])));
        let db = database(vec![Value::Map(fields)]);
        let mut hashes = Hashes {
            crc32: "12345678".into(),
            md5: "00".repeat(16),
            sha1: "00".repeat(20),
            size: 32,
        };
        let Lookup::Found(matched) = db
            .lookup("Renamed", Some(&hashes), Artwork::Screenshot)
            .lookup
        else {
            panic!("renamed hash match failed")
        };
        assert_eq!(matched.name, "Original (Europe)");
        hashes.crc32 = "FFFFFFFF".into();
        assert!(matches!(
            db.lookup("Original (Europe)", Some(&hashes), Artwork::Screenshot)
                .lookup,
            Lookup::Ambiguous(1)
        ));
    }

    #[test]
    fn region_and_revision_tags_require_confirmation_when_not_exact() {
        let db = database(vec![
            record("Example (USA)"),
            record("Example (Europe)"),
            record("Example (USA) (Rev 1)"),
        ]);
        assert!(matches!(
            db.lookup("Example", None, Artwork::Screenshot).lookup,
            Lookup::Ambiguous(3)
        ));
        let Lookup::Found(matched) = db
            .lookup("Example (USA) (Rev 1)", None, Artwork::BoxArt)
            .lookup
        else {
            panic!("exact revision failed")
        };
        assert_eq!(matched.name, "Example (USA) (Rev 1)");
        assert!(thumbnail_candidates(&matched, None)
            .iter()
            .all(|url| url.contains("Rev%201")));
    }

    #[test]
    fn thumbnails_use_canonical_encoding_and_only_safe_flexible_names() {
        assert_eq!(thumbnail_url("Nintendo - Game Boy", Artwork::TitleScreen, "Q*Bert & Friends (USA)"), "https://thumbnails.libretro.com/Nintendo%20-%20Game%20Boy/Named_Titles/Q_Bert%20_%20Friends%20%28USA%29.png");
        assert_eq!(
            thumbnail_url("MAME", Artwork::Screenshot, "A \"Quoted\" Title"),
            "https://thumbnails.libretro.com/MAME/Named_Snaps/A%20_Quoted_%20Title.png"
        );
        let db = database(vec![record("Unique (USA)")]);
        let Lookup::Found(matched) = db.lookup("Unique (USA)", None, Artwork::Screenshot).lookup
        else {
            unreachable!()
        };
        let urls = thumbnail_candidates(&matched, Some("Completely Renamed"));
        assert_eq!(urls.len(), 2);
        assert!(urls[0].ends_with("Unique%20%28USA%29.png"));
        assert!(urls[1].ends_with("Unique.png"));
        assert!(!urls.iter().any(|url| url.contains("Renamed")));
    }

    #[test]
    fn malformed_and_cancelled_databases_are_not_empty_successes() {
        for bytes in [
            vec![],
            b"not an RDB".to_vec(),
            fixture(vec![Value::Nil]),
            fixture(vec![Value::Map(vec![
                ("name".into(), 3.into()),
                ("crc".into(), Value::Binary(vec![0; 4])),
            ])]),
            fixture(vec![Value::Map(vec![
                ("name".into(), "Bad".into()),
                ("crc".into(), Value::Binary(vec![0])),
            ])]),
        ] {
            assert_eq!(
                Database::parse("test", &bytes, &AtomicBool::new(false))
                    .err()
                    .unwrap()
                    .kind,
                ErrorKind::MalformedResponse
            );
        }
        let mut bytes = fixture(vec![record("Example")]);
        bytes[8..16].copy_from_slice(&u64::MAX.to_be_bytes());
        assert_eq!(
            Database::parse("test", &bytes, &AtomicBool::new(false))
                .err()
                .unwrap()
                .kind,
            ErrorKind::MalformedResponse
        );
        assert_eq!(
            Database::parse(
                "test",
                &fixture(vec![record("Example")]),
                &AtomicBool::new(true)
            )
            .err()
            .unwrap()
            .kind,
            ErrorKind::Cancelled
        );
    }

    #[test]
    fn explicit_platform_mapping_does_not_guess_from_display_labels() {
        assert_eq!(
            platform("nEs"),
            Some((1039, "Nintendo - Nintendo Entertainment System"))
        );
        assert_eq!(platform("Gameboy2P"), platform("Gameboy"));
        assert_ne!(platform("Arcade"), platform("NeoGeo"));
        for id in [
            "Nintendo Entertainment System",
            "AppleIIGS",
            "ZXNext",
            "My NES",
            "Unknown",
        ] {
            assert!(platform(id).is_none(), "{id}");
        }
        let ids: toml::Value = toml::from_str(include_str!("../../assets/systems.toml")).unwrap();
        for (aliases, _, _) in PLATFORMS {
            for id in *aliases {
                assert!(
                    ids["systems"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|system| system["id"].as_str() == Some(id)),
                    "missing Degauss id {id}"
                );
            }
        }
    }

    #[test]
    fn upstream_missing_http_and_database_errors_are_distinct() {
        for (status, kind) in [
            (404, ErrorKind::NotFound),
            (408, ErrorKind::Timeout),
            (429, ErrorKind::RateLimited),
            (500, ErrorKind::Server),
            (403, ErrorKind::InvalidRequest),
        ] {
            let error = checked_response(
                HttpResponse {
                    status,
                    content_type: None,
                    body: Vec::new(),
                },
                "database",
            )
            .err()
            .unwrap();
            assert_eq!(error.kind, kind);
            assert!(error.detail.contains(&status.to_string()));
            assert!(!error
                .user_message_for(super::super::ScraperSource::Libretro)
                .contains("ScreenScraper"));
        }
    }
}
