//! `gamelist.xml` reader (EmulationStation family format).
//!
//! This is the standard metadata sidecar used by EmulationStation, ES-DE,
//! Batocera and RetroPie: one file per game folder, listing per-game
//! metadata only. It never carries launch information: which core runs a
//! file is defined per system elsewhere (`es_systems.xml` upstream,
//! `systems.toml` here).
//!
//! Two entry shapes are accepted, because both exist in the wild:
//!
//! 1. Flat (the EmulationStation standard):
//!    `<game><path>./X.d64</path><name>X</name><image>./media/x.png</image></game>`
//! 2. Parent/child, as some scrapers write it: a parent
//!    entry carries `id` and the media, and children carry `parentid` plus
//!    the `<path>` they apply to. Children inherit the parent's media.
//!
//! Matching a metadata entry to a file on disk is deliberately explicit and
//! counted: `by_rel_path` (exact relative path), `by_file_name` and
//! `by_stem` (progressively looser), plus `by_slug` for entries whose path
//! is a normalised title key rather than a filename. Every entry that never
//! matches anything is counted in `unmatched`, so Degauss can display how
//! much of the metadata actually bound to media instead of quietly showing
//! a short list.
//!
//! Two things here are shaped by what a real card costs rather than by
//! taste:
//!
//! * Nothing is stat()ed. Artwork folders on a scraped card routinely hold
//!   tens of thousands of files on a FAT filesystem with no directory index,
//!   so each individual lookup rescans the whole directory.
//!   Checking each entry's art individually would spend well over a minute
//!   before the first frame. The caller reads each art directory once
//!   instead and answers from memory.
//! * Metadata is stored once in a vector and the lookup tables hold indices
//!   into it. Four maps each owning a clone would multiply that memory for
//!   a large gamelist, on a board with a few hundred megabytes of RAM.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use quick_xml::events::Event;
use quick_xml::{Reader, XmlVersion};

use crate::error::{DegaussError, Result};

/// The largest `gamelist.xml` worth reading.
const MAX_GAMELIST_BYTES: u64 = 256 * 1024 * 1024;

/// A release date as somebody would write it.
///
/// The scrapers write the ISO basic form the EmulationStation format uses,
/// `19990527T000000`, which is a timestamp where a date was wanted. A day
/// or a month of zero means the scraper only knew the year, so that is all
/// that is claimed. Anything that does not look like that is passed
/// through: a card can hold whatever it holds.
fn release_date(raw: &str) -> String {
    let digits = raw.trim();
    if digits.len() < 8 || !digits.as_bytes()[..8].iter().all(u8::is_ascii_digit) {
        return digits.to_string();
    }
    let (year, month, day) = (&digits[..4], &digits[4..6], &digits[6..8]);
    match (month, day) {
        ("00", _) | (_, "00") => year.to_string(),
        _ => format!("{year}-{month}-{day}"),
    }
}

/// A description reduced to what one line can hold.
///
/// Scraped descriptions run to paragraphs. Only the first line is ever
/// shown, and keeping the rest would hold megabytes of text that nothing
/// reads.
fn first_line(text: &str) -> String {
    const ROOM: usize = 160;
    let line = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("");
    let line = line.trim();
    if line.chars().count() > ROOM {
        let kept: String = line.chars().take(ROOM).collect();
        format!("{}...", kept.trim_end())
    } else {
        line.to_string()
    }
}

/// Metadata for one game, after parent/child resolution.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GameMeta {
    pub name: Option<String>,
    /// Absolute path to the cover art, resolved against the gamelist folder.
    pub image: Option<PathBuf>,
    pub genre: Option<String>,
    pub favorite: bool,
    /// What the scrapers write beside the name. Shown a line each under the
    /// picture, so a description is cut where a line ends rather than kept
    /// whole: the card holds tens of thousands of them and every byte is
    /// parsed and held.
    pub desc: Option<String>,
    pub publisher: Option<String>,
    pub developer: Option<String>,
    pub released: Option<String>,
    pub players: Option<String>,
    pub lang: Option<String>,
}

impl GameMeta {
    fn is_empty(&self) -> bool {
        self.name.is_none()
            && self.image.is_none()
            && self.genre.is_none()
            && !self.favorite
            && self.desc.is_none()
            && self.publisher.is_none()
            && self.developer.is_none()
            && self.released.is_none()
            && self.players.is_none()
            && self.lang.is_none()
    }
}

/// What the parse actually found. Displayed by Degauss: a metadata file
/// that parses but binds to nothing is a silent failure otherwise.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GamelistStats {
    pub games_total: usize,
    pub parents: usize,
    pub children: usize,
    pub with_image: usize,
    /// Filled in by the catalog once it has read the art directories.
    pub image_missing_on_disk: usize,
    pub slug_keyed: usize,
    pub parse_ms: u128,
}

#[derive(Debug, Default)]
pub struct Gamelist {
    /// Every entry's metadata, stored once.
    entries: Vec<GameMeta>,
    by_rel_path: HashMap<String, usize>,
    by_file_name: HashMap<String, usize>,
    by_stem: HashMap<String, usize>,
    by_slug: HashMap<String, usize>,
    pub stats: GamelistStats,
}

/// One `<game>` element as read, before parent/child resolution.
#[derive(Debug, Default)]
struct RawGame {
    id: Option<String>,
    parent_id: Option<String>,
    path: Option<String>,
    name: Option<String>,
    image: Option<String>,
    screenshot: Option<String>,
    thumbnail: Option<String>,
    genre: Option<String>,
    favorite: bool,
    desc: Option<String>,
    publisher: Option<String>,
    developer: Option<String>,
    released: Option<String>,
    players: Option<String>,
    lang: Option<String>,
}

impl RawGame {
    /// ES uses `<image>` as the primary art; some scrapers write
    /// `<screenshot>`. `<thumbnail>` is the last resort because it is a
    /// small preview in both conventions.
    fn art(&self) -> Option<&str> {
        self.image
            .as_deref()
            .or(self.screenshot.as_deref())
            .or(self.thumbnail.as_deref())
    }
}

/// Normalise a `<path>` value for lookup: `./Sub/Game.d64` -> `Sub/Game.d64`.
fn normalise_rel(raw: &str) -> String {
    let trimmed = raw.trim().replace('\\', "/");
    let stripped = trimmed.strip_prefix("./").unwrap_or(&trimmed);
    stripped.trim_start_matches('/').to_string()
}

fn file_name_of(rel: &str) -> &str {
    rel.rsplit('/').next().unwrap_or(rel)
}

fn stem_of(file_name: &str) -> &str {
    match file_name.rfind('.') {
        Some(i) if i > 0 => &file_name[..i],
        _ => file_name,
    }
}

/// `&#65;` and `&#x41;` are character references rather than named
/// entities, and a library full of accented titles uses them.
fn resolve_numeric_entity(name: &str) -> Option<char> {
    let digits = name.strip_prefix('#')?;
    let value = match digits.strip_prefix(['x', 'X']) {
        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
        None => digits.parse().ok()?,
    };
    char::from_u32(value)
}

/// Decode a value that was escaped twice, for display only.
///
/// Scrapers write `&amp;apos;` where they meant an apostrophe, so correct
/// XML decoding yields the literal text `&apos;` and the shelf reads
/// "Daley Thompson&apos;s Star Events". The file on disk is named that way
/// too, so paths must keep it exactly and only the name a person reads is
/// softened. Nothing is guessed: only the five entities XML defines and
/// numeric references are touched, and a name with no entity in it comes
/// back unchanged.
fn undouble_escape(value: &str) -> String {
    if !value.contains('&') {
        return value.to_string();
    }
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        let Some(end) = tail.find(';') else {
            out.push_str(tail);
            return out;
        };
        let name = &tail[1..end];
        if let Some(resolved) = quick_xml::escape::resolve_predefined_entity(name) {
            out.push_str(resolved);
        } else if let Some(character) = resolve_numeric_entity(name) {
            out.push(character);
        } else {
            // Not an entity, just an ampersand followed by words.
            out.push_str(&tail[..=end]);
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

/// Title normalisation for slug-keyed entries: lowercase, keep only
/// alphanumerics.
pub fn slugify(value: &str) -> String {
    value
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Drop the bracketed parts of a filename: region, language, revision and
/// dump tags. `Super Game (USA) [!].sfc` is the same game as `Super Game`,
/// and the metadata is keyed on the title, not the dump.
fn without_tags(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut depth = 0usize;
    for c in value.chars() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out.trim().to_string()
}

/// Spelled-out numbers become digits, because the metadata's own
/// normalisation does that: "Formula One Grand Prix" is keyed as
/// `formula1grandprix`.
fn digits_for_words(value: &str) -> String {
    const WORDS: [(&str, &str); 10] = [
        ("one", "1"),
        ("two", "2"),
        ("three", "3"),
        ("four", "4"),
        ("five", "5"),
        ("six", "6"),
        ("seven", "7"),
        ("eight", "8"),
        ("nine", "9"),
        ("ten", "10"),
    ];
    value
        .split_whitespace()
        .map(|word| {
            let bare: String = word
                .chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .collect::<String>()
                .to_ascii_lowercase();
            WORDS
                .iter()
                .find(|(w, _)| *w == bare)
                .map(|(_, d)| (*d).to_string())
                .unwrap_or_else(|| word.to_string())
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The slug keys a filename might be filed under, most literal first.
///
/// A scraped library keys its metadata on the game's title while the files
/// on disk carry dump tags, so one name has to be tried several ways or
/// most of a library's artwork never binds.
fn slug_candidates(stem: &str) -> Vec<String> {
    let mut out = Vec::with_capacity(4);
    let mut push = |value: String| {
        if !value.is_empty() && !out.contains(&value) {
            out.push(value);
        }
    };
    push(slugify(stem));
    let untagged = without_tags(stem);
    push(slugify(&untagged));
    push(slugify(&digits_for_words(stem)));
    push(slugify(&digits_for_words(&untagged)));
    out
}

impl Gamelist {
    /// Look up metadata for a file, from strictest to loosest key. Returns
    /// which key matched so the caller can report the mix.
    pub fn lookup(&self, rel_path: &str) -> Option<(&GameMeta, MatchKind)> {
        let rel = normalise_rel(rel_path);
        if let Some(&i) = self.by_rel_path.get(&rel) {
            return Some((&self.entries[i], MatchKind::RelPath));
        }
        let file_name = file_name_of(&rel).to_ascii_lowercase();
        if let Some(&i) = self.by_file_name.get(&file_name) {
            return Some((&self.entries[i], MatchKind::FileName));
        }
        let stem = stem_of(&file_name).to_string();
        if let Some(&i) = self.by_stem.get(&stem) {
            return Some((&self.entries[i], MatchKind::Stem));
        }
        for candidate in slug_candidates(&stem) {
            if let Some(&i) = self.by_slug.get(&candidate) {
                return Some((&self.entries[i], MatchKind::Slug));
            }
        }
        // A game inside an archive is usually filed under the archive's own
        // name, not the name of the file within it.
        if let Some(archive) = rel.rsplit_once(".zip/").map(|(before, _)| before) {
            let archive_stem = stem_of(file_name_of(archive)).to_string();
            for candidate in slug_candidates(&archive_stem) {
                if let Some(&i) = self.by_slug.get(&candidate) {
                    return Some((&self.entries[i], MatchKind::Slug));
                }
            }
            let key = format!("{archive_stem}.zip").to_ascii_lowercase();
            if let Some(&i) = self.by_file_name.get(&key) {
                return Some((&self.entries[i], MatchKind::FileName));
            }
            if let Some(&i) = self.by_stem.get(&archive_stem.to_ascii_lowercase()) {
                return Some((&self.entries[i], MatchKind::Stem));
            }
        }
        None
    }

    /// Distinct directories the metadata points art at. The caller reads
    /// each one once rather than asking about files individually.
    pub fn art_directories(&self) -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = self
            .entries
            .iter()
            .filter_map(|meta| meta.image.as_deref().and_then(|p| p.parent()))
            .map(|p| p.to_path_buf())
            .collect();
        dirs.sort();
        dirs.dedup();
        dirs
    }

    /// Parse a gamelist file. `folder` is the directory the gamelist lives
    /// in; relative media paths resolve against it.
    pub fn load(path: &Path, folder: &Path) -> Result<Self> {
        let started = std::time::Instant::now();
        // A gamelist is text; even a very large library makes tens of
        // megabytes. Past this it is not a gamelist, and reading it would
        // cost more memory than the board has.
        let size = std::fs::metadata(path)
            .map_err(|e| DegaussError::io("reading gamelist", path, e))?
            .len();
        if size > MAX_GAMELIST_BYTES {
            return Err(DegaussError::unsupported(
                "gamelist",
                format!(
                    "{} is {size} bytes, past the {MAX_GAMELIST_BYTES} limit",
                    path.display()
                ),
            ));
        }
        let text = std::fs::read_to_string(path)
            .map_err(|e| DegaussError::io("reading gamelist.xml", path, e))?;
        let mut list = Self::parse(&text, folder, path)?;
        list.stats.parse_ms = started.elapsed().as_millis();
        Ok(list)
    }

    /// Parse gamelist XML from a string. Split out from [`Gamelist::load`]
    /// so tests do not need files on disk.
    pub fn parse(text: &str, folder: &Path, origin: &Path) -> Result<Self> {
        let mut reader = Reader::from_str(text);
        // NOT trim_text: it trims each fragment, so the spaces on either
        // side of an entity reference are destroyed and "Rock &amp; Roll"
        // comes back as "Rock&Roll". The joined value is trimmed once, at
        // the closing tag.

        let mut raw_games: Vec<RawGame> = Vec::new();
        let mut current: Option<RawGame> = None;
        let mut field: Option<String> = None;
        // Text is ACCUMULATED, not assigned. A parser hands back the text
        // around an entity reference as separate pieces, so "Rock &amp;
        // Roll" arrives as "Rock ", "&", " Roll". Taking only the last piece
        // silently truncated every title and every path containing an
        // ampersand, and a truncated path then pointed the artwork index at
        // the system's own folder.
        let mut text = String::new();

        loop {
            match reader.read_event() {
                Err(e) => {
                    return Err(DegaussError::malformed(
                        "gamelist.xml",
                        origin,
                        format!("at position {}: {e}", reader.buffer_position()),
                    ))
                }
                Ok(Event::Eof) => break,
                Ok(Event::Start(e)) => {
                    let tag = e.name().as_ref().to_ascii_lowercase();
                    if tag == "game" {
                        let mut game = RawGame::default();
                        for attr in e.attributes() {
                            let attr = attr.map_err(|err| {
                                DegaussError::malformed(
                                    "gamelist.xml",
                                    origin,
                                    format!("bad attribute on <game>: {err}"),
                                )
                            })?;
                            let key = attr.key.as_ref().to_ascii_lowercase();
                            let value = attr
                                .normalized_value(XmlVersion::Implicit1_0)
                                .map_err(|err| {
                                    DegaussError::malformed(
                                        "gamelist.xml",
                                        origin,
                                        format!("bad attribute value on <game>: {err}"),
                                    )
                                })?
                                .into_owned();
                            match key.as_str() {
                                "id" => game.id = Some(value),
                                "parentid" => game.parent_id = Some(value),
                                _ => {}
                            }
                        }
                        current = Some(game);
                        field = None;
                    } else if current.is_some() {
                        // Allocating text for fields that are never read costs
                        // real time on a large gamelist.
                        field = matches!(
                            tag.as_str(),
                            "path"
                                | "name"
                                | "image"
                                | "screenshot"
                                | "thumbnail"
                                | "genre"
                                | "favorite"
                                | "desc"
                                | "publisher"
                                | "developer"
                                | "releasedate"
                                | "players"
                                | "lang"
                        )
                        .then_some(tag);
                    }
                }
                Ok(Event::Text(t)) => {
                    if current.is_some() && field.is_some() {
                        text.push_str(&t.xml10_content());
                    }
                }
                Ok(Event::CData(t)) => {
                    if current.is_some() && field.is_some() {
                        text.push_str(&t.into_inner());
                    }
                }
                // An entity reference arrives as its own event, which is
                // exactly why the pieces have to be joined rather than
                // assigned. Unknown entities are dropped rather than
                // guessed at.
                Ok(Event::GeneralRef(entity)) => {
                    if current.is_some() && field.is_some() {
                        let name = entity.into_inner();
                        if let Some(resolved) = quick_xml::escape::resolve_predefined_entity(&name)
                        {
                            text.push_str(resolved);
                        } else if let Some(character) = resolve_numeric_entity(&name) {
                            text.push(character);
                        }
                    }
                }
                Ok(Event::End(e)) => {
                    let tag = e.name().as_ref().to_ascii_lowercase();
                    // The value is complete only now, with every piece of
                    // text between the tags joined back together.
                    if let (Some(game), Some(name)) = (current.as_mut(), field.as_deref()) {
                        let value = text.trim();
                        if !value.is_empty() {
                            let value = value.to_string();
                            match name {
                                "path" => game.path = Some(value),
                                "name" => game.name = Some(undouble_escape(&value)),
                                "image" => game.image = Some(value),
                                "screenshot" => game.screenshot = Some(value),
                                "thumbnail" => game.thumbnail = Some(value),
                                "genre" => game.genre = Some(value),
                                "favorite" => game.favorite = value.eq_ignore_ascii_case("true"),
                                // Cut where it is read, not where it is
                                // drawn: this file holds tens of thousands
                                // of descriptions and they are shown on one
                                // line each.
                                "desc" => game.desc = Some(first_line(&value)),
                                "publisher" => game.publisher = Some(value),
                                "developer" => game.developer = Some(value),
                                "releasedate" => game.released = Some(release_date(&value)),
                                "players" => game.players = Some(value),
                                "lang" => game.lang = Some(value),
                                _ => {}
                            }
                        }
                    }
                    text.clear();
                    if tag == "game" {
                        if let Some(game) = current.take() {
                            raw_games.push(game);
                        }
                    }
                    field = None;
                }
                _ => {}
            }
        }

        Ok(Self::resolve(raw_games, folder))
    }

    /// Turn raw entries into lookup tables, resolving parent/child
    /// inheritance and making media paths absolute.
    fn resolve(raw_games: Vec<RawGame>, folder: &Path) -> Self {
        let mut list = Gamelist::default();
        list.stats.games_total = raw_games.len();

        let mut parents: HashMap<String, &RawGame> = HashMap::new();
        for game in &raw_games {
            if let Some(id) = &game.id {
                parents.insert(id.clone(), game);
            }
        }

        for game in &raw_games {
            // A parent entry (id, no path) only supplies media to children.
            if game.path.is_none() {
                if game.id.is_some() {
                    list.stats.parents += 1;
                }
                continue;
            }

            let inherited = game.parent_id.as_ref().and_then(|pid| parents.get(pid));
            if inherited.is_some() {
                list.stats.children += 1;
            }

            let art = game.art().or_else(|| inherited.and_then(|p| p.art()));
            let name = game
                .name
                .clone()
                .or_else(|| inherited.and_then(|p| p.name.clone()));
            let genre = game
                .genre
                .clone()
                .or_else(|| inherited.and_then(|p| p.genre.clone()));
            let favorite = game.favorite || inherited.is_some_and(|p| p.favorite);
            let inherit = |own: Option<String>, pick: fn(&RawGame) -> Option<String>| {
                own.or_else(|| inherited.and_then(|parent| pick(parent)))
            };
            let desc = inherit(game.desc.clone(), |p| p.desc.clone());
            let publisher = inherit(game.publisher.clone(), |p| p.publisher.clone());
            let developer = inherit(game.developer.clone(), |p| p.developer.clone());
            let released = inherit(game.released.clone(), |p| p.released.clone());
            let players = inherit(game.players.clone(), |p| p.players.clone());
            let lang = inherit(game.lang.clone(), |p| p.lang.clone());

            // Deliberately not checked here: see the note at the top of the
            // file about what a stat costs on this hardware.
            let image = art.map(|rel| folder.join(normalise_rel(rel)));
            if image.is_some() {
                list.stats.with_image += 1;
            }

            let meta = GameMeta {
                name,
                image,
                genre,
                favorite,
                desc,
                publisher,
                developer,
                released,
                players,
                lang,
            };
            if meta.is_empty() {
                continue;
            }

            let raw_path = game.path.as_deref().unwrap_or_default();
            let rel = normalise_rel(raw_path);

            // A `<path>` ending in `.slug` is a normalised title key, not a
            // file on disk, which is one convention in use.
            if let Some(slug_key) = rel.strip_suffix(".slug") {
                list.stats.slug_keyed += 1;
                let index = list.entries.len();
                list.entries.push(meta);
                list.by_slug.insert(slugify(slug_key), index);
                continue;
            }

            let file_name = file_name_of(&rel).to_ascii_lowercase();
            let stem = stem_of(&file_name).to_string();
            // Stored once; the three lookup tables hold indices into it.
            let index = list.entries.len();
            list.entries.push(meta);
            list.by_rel_path.insert(rel, index);
            list.by_file_name.entry(file_name).or_insert(index);
            list.by_stem.entry(stem).or_insert(index);
        }

        list
    }
}

/// Which key bound a metadata entry to a file. Reported so a run that only
/// matches loosely is visible rather than assumed correct.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchKind {
    RelPath,
    FileName,
    Stem,
    Slug,
}

#[cfg(test)]
mod tests {
    use super::*;

    const FOLDER: &str = "/games/C64";

    fn parse(text: &str) -> Gamelist {
        Gamelist::parse(
            text,
            Path::new(FOLDER),
            Path::new("/games/C64/gamelist.xml"),
        )
        .expect("fixture parses")
    }

    #[test]
    fn flat_entries_bind_by_relative_path_and_resolve_media_against_the_folder() {
        let list = parse(
            r#"<?xml version="1.0"?>
            <gameList>
              <game>
                <path>./Boulder Dash.d64</path>
                <name>Boulder Dash</name>
                <image>./media/images/boulder.png</image>
                <genre>Puzzle</genre>
                <favorite>true</favorite>
              </game>
            </gameList>"#,
        );

        let (meta, kind) = list.lookup("./Boulder Dash.d64").expect("entry binds");
        assert_eq!(kind, MatchKind::RelPath);
        assert_eq!(meta.name.as_deref(), Some("Boulder Dash"));
        assert_eq!(meta.genre.as_deref(), Some("Puzzle"));
        assert!(meta.favorite, "favorite flag must survive parsing");
        assert_eq!(
            meta.image.as_deref(),
            Some(Path::new("/games/C64/media/images/boulder.png")),
            "media paths are relative to the gamelist folder, not the process cwd"
        );
        assert_eq!(list.stats.games_total, 1);
    }

    #[test]
    fn a_child_entry_inherits_media_from_its_parent() {
        // The parent/child shape: art lives on the parent, the child only
        // names the file it applies to.
        let list = parse(
            r#"<gameList>
              <game id="7" source="scraper">
                <name>Impossible Mission</name>
                <screenshot>./media/screenshot/im.png</screenshot>
              </game>
              <game parentid="7" source="scraper">
                <path>./Impossible Mission.d64</path>
              </game>
            </gameList>"#,
        );

        let (meta, _) = list
            .lookup("./Impossible Mission.d64")
            .expect("child binds");
        assert_eq!(meta.name.as_deref(), Some("Impossible Mission"));
        assert_eq!(
            meta.image.as_deref(),
            Some(Path::new("/games/C64/media/screenshot/im.png")),
            "a child with no art of its own must inherit the parent's"
        );
        assert_eq!(list.stats.parents, 1);
        assert_eq!(list.stats.children, 1);
    }

    #[test]
    fn slug_keyed_entries_bind_to_a_filename_by_normalised_title() {
        let list = parse(
            r#"<gameList>
              <game>
                <path>impossiblemission.slug</path>
                <name>Impossible Mission</name>
                <image>./media/im.png</image>
              </game>
            </gameList>"#,
        );

        let (meta, kind) = list
            .lookup("./Impossible Mission.d64")
            .expect("slug key binds to a real filename");
        assert_eq!(kind, MatchKind::Slug);
        assert_eq!(meta.name.as_deref(), Some("Impossible Mission"));
        assert_eq!(list.stats.slug_keyed, 1);
    }

    #[test]
    fn a_slug_key_binds_a_file_that_carries_region_tags() {
        // Scraped metadata is keyed on the title; files on disk carry dump
        // tags. Without stripping them, most of a library's artwork never
        // binds to anything.
        let list = parse(
            r#"<gameList>
              <game>
                <path>supermarioworld.slug</path>
                <name>Super Mario World</name>
                <image>./media/smw.png</image>
              </game>
            </gameList>"#,
        );

        let (meta, kind) = list
            .lookup("./Super Mario World (USA) [!].sfc")
            .expect("tags must not prevent a match");
        assert_eq!(kind, MatchKind::Slug);
        assert_eq!(meta.name.as_deref(), Some("Super Mario World"));
    }

    #[test]
    fn a_spelled_out_number_matches_a_slug_that_uses_a_digit() {
        let list = parse(
            r#"<gameList>
              <game><path>formula1grandprix.slug</path><name>Formula One Grand Prix</name></game>
            </gameList>"#,
        );
        assert!(list.lookup("./Formula One Grand Prix.d64").is_some());
    }

    #[test]
    fn a_game_inside_an_archive_binds_through_the_archive_name() {
        // The metadata knows the archive, because that is what a scraper
        // sees; the launchable thing is the file inside it.
        let list = parse(
            r#"<gameList>
              <game>
                <path>sonicthehedgehog.slug</path>
                <name>Sonic The Hedgehog</name>
                <image>./media/sonic.png</image>
              </game>
            </gameList>"#,
        );

        let (meta, _) = list
            .lookup("./Sonic The Hedgehog (World).zip/Sonic The Hedgehog (W) [!].md")
            .expect("the archive's own name must be tried");
        assert_eq!(meta.name.as_deref(), Some("Sonic The Hedgehog"));
    }

    #[test]
    fn lookup_falls_back_from_exact_path_to_filename_when_folders_differ() {
        let list = parse(
            r#"<gameList>
              <game><path>./Sub/Game.d64</path><name>Game</name></game>
            </gameList>"#,
        );

        let (_, kind) = list
            .lookup("./Other/Game.d64")
            .expect("filename still binds");
        assert_eq!(
            kind,
            MatchKind::FileName,
            "a moved file should still find its metadata, but the looser match must be reported"
        );
    }

    #[test]
    fn malformed_xml_is_an_error_not_an_empty_list() {
        // The failure mode that matters: a broken metadata file must not
        // look like a folder with no metadata.
        let err = Gamelist::parse(
            "<gameList><game><path>./x.d64</path></gameList>",
            Path::new(FOLDER),
            Path::new("/games/C64/gamelist.xml"),
        );
        assert!(err.is_err(), "unclosed element must surface as an error");
    }

    #[test]
    fn entries_without_metadata_are_not_indexed() {
        let list = parse(r#"<gameList><game><path>./bare.d64</path></game></gameList>"#);
        assert!(
            list.lookup("./bare.d64").is_none(),
            "an entry carrying nothing but a path adds no metadata"
        );
    }

    #[test]
    fn a_value_containing_an_entity_survives_whole() {
        // A parser reports the text around an entity as separate pieces, so
        // taking only the last one truncated every title and path with an
        // ampersand in it. A truncated path then aimed the artwork index at
        // the system's own folder and read thousands of files for nothing.
        let list = parse(
            r#"<gameList>
              <game>
                <path>./Rock &amp; Roll.d64</path>
                <name>Rock &amp; Roll</name>
                <image>./media/rock &amp; roll.png</image>
              </game>
            </gameList>"#,
        );

        let (meta, _) = list
            .lookup("./Rock & Roll.d64")
            .expect("the whole path must survive");
        assert_eq!(meta.name.as_deref(), Some("Rock & Roll"));
        assert_eq!(
            meta.image.as_deref(),
            Some(Path::new("/games/C64/media/rock & roll.png")),
            "the artwork path must keep its leading folder"
        );
    }

    #[test]
    fn art_directories_never_include_the_system_root() {
        // If a media path loses its folder, every entry appears to keep its
        // artwork loose in the system folder, and the scan then reads the
        // whole library a second time looking for pictures.
        let list = parse(
            r#"<gameList>
              <game>
                <path>./A &amp; B.d64</path>
                <image>./media/screenshot/a &amp; b.png</image>
              </game>
            </gameList>"#,
        );
        let dirs = list.art_directories();
        assert_eq!(
            dirs,
            vec![Path::new("/games/C64/media/screenshot").to_path_buf()]
        );
    }

    #[test]
    fn slugify_matches_the_documented_normalisation() {
        assert_eq!(slugify("Impossible Mission"), "impossiblemission");
        assert_eq!(slugify("H.E.R.O. (1984)"), "hero1984");
    }

    #[test]
    fn a_name_escaped_twice_by_a_scraper_still_reads_properly() {
        // Some scrapers write a doubly-escaped entity: "&amp;apos;" must
        // decode to the literal "&apos;", because the file on disk is
        // named that way.
        let list = Gamelist::parse(
            r#"<gameList><game><path>./a.prg</path>
               <name>Daley Thompson&amp;apos;s Star Events</name>
               <screenshot>./media/Daley Thompson&amp;apos;s Star Events.png</screenshot>
               </game></gameList>"#,
            Path::new("/games/C16"),
            Path::new("test"),
        )
        .expect("parses");
        let (meta, _) = list.lookup("a.prg").expect("found");
        assert_eq!(meta.name.as_deref(), Some("Daley Thompson's Star Events"));
        // The file on disk really is named with the entity in it, so the
        // path must NOT be softened or the artwork stops being found.
        assert_eq!(
            meta.image.as_deref().and_then(|p| p.to_str()),
            Some("/games/C16/media/Daley Thompson&apos;s Star Events.png")
        );
    }

    #[test]
    fn an_ampersand_that_is_not_an_entity_is_left_alone() {
        let list = Gamelist::parse(
            r#"<gameList><game><path>./b.prg</path>
               <name>Rock &amp; Roll &amp;not an entity</name></game></gameList>"#,
            Path::new("/games/C16"),
            Path::new("test"),
        )
        .expect("parses");
        let (meta, _) = list.lookup("b.prg").expect("found");
        assert_eq!(meta.name.as_deref(), Some("Rock & Roll &not an entity"));
    }

    #[test]
    fn a_release_date_reads_as_a_date_rather_than_a_timestamp() {
        assert_eq!(release_date("19990527T000000"), "1999-05-27");
        assert_eq!(release_date("19990527"), "1999-05-27");
        // The scraper only knew the year.
        assert_eq!(release_date("19990000T000000"), "1999");
        assert_eq!(release_date("19991200T000000"), "1999");
        // Anything else is passed through rather than mangled.
        assert_eq!(release_date("1999"), "1999");
        assert_eq!(release_date("spring 1999"), "spring 1999");
        assert_eq!(release_date(""), "");
    }
}
