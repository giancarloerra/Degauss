use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{Read, Write};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, XmlVersion};

use super::{Error, ErrorKind, ImagePolicy, Metadata, MetadataPolicy, Result};

const MAX_GAMELIST_BYTES: u64 = 256 * 1024 * 1024;
const FIELDS: [&str; 8] = [
    "name",
    "desc",
    "publisher",
    "developer",
    "releasedate",
    "players",
    "genre",
    "lang",
];
const ART_FIELDS: [&str; 3] = ["image", "screenshot", "thumbnail"];
type Replacement = (Range<usize>, Vec<u8>);
type ExistingEdits = (Vec<Replacement>, Vec<u8>, Change);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Change {
    pub metadata_fields: usize,
    pub image: bool,
    pub created_entry: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Needs {
    pub image: bool,
    pub metadata: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Eligibility {
    Needs(Needs),
    Alias { representative: usize },
}

impl Needs {
    pub fn any(self) -> bool {
        self.image || self.metadata
    }
}

impl Change {
    pub fn changed(self) -> bool {
        self.metadata_fields > 0 || self.image || self.created_entry
    }
}

/// Backups made during one scrape run.
///
/// A gamelist is backed up at most once, immediately before its first
/// successful replacement. A newly-created gamelist has no original to back
/// up. Existing backups are never overwritten.
pub struct Backups {
    suffix: u128,
    made: HashSet<PathBuf>,
}

impl Backups {
    pub fn new() -> Self {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        Self {
            suffix,
            made: HashSet::new(),
        }
    }

    fn ensure(&mut self, path: &Path, original: &[u8]) -> Result<()> {
        if self.made.contains(path) || !path.exists() {
            return Ok(());
        }
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("gamelist.xml");
        let parent = path.parent().unwrap_or(Path::new("."));
        for attempt in 0..100u8 {
            let backup = parent.join(format!(
                "{file_name}.degauss-scraper-{}-{}.bak",
                self.suffix, attempt
            ));
            match write_new(&backup, original) {
                Ok(()) => {
                    self.made.insert(path.to_path_buf());
                    return Ok(());
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(Error::local(format!(
                        "could not back up {}: {error}",
                        path.display()
                    )))
                }
            }
        }
        Err(Error::local(format!(
            "could not choose a backup name beside {}",
            path.display()
        )))
    }
}

impl Default for Backups {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
struct Field {
    whole: Range<usize>,
    content: Option<Range<usize>>,
    value: String,
}

#[derive(Debug, Clone)]
struct Game {
    whole: Range<usize>,
    close_start: usize,
    id: Option<String>,
    parent_id: Option<String>,
    path: String,
    fields: BTreeMap<String, Field>,
}

struct Document {
    games: Vec<Game>,
    root_close: usize,
    empty_root: Option<EmptyRoot>,
    newline: &'static str,
}

struct EmptyRoot {
    whole: Range<usize>,
    name: String,
}

#[derive(Debug, Clone)]
struct OpenField {
    name: String,
    whole_start: usize,
    content_start: usize,
    depth: usize,
    text: String,
}

#[derive(Debug, Clone)]
struct OpenGame {
    whole_start: usize,
    depth: usize,
    id: Option<String>,
    parent_id: Option<String>,
    path: String,
    fields: BTreeMap<String, Field>,
    field: Option<OpenField>,
}

#[cfg(test)]
pub fn needs_many(
    gamelist_path: &Path,
    folder: &Path,
    relative_game_paths: &[String],
    image_policy: ImagePolicy,
    metadata_policy: MetadataPolicy,
) -> Result<Vec<Result<Needs>>> {
    let fallback: Vec<bool> = relative_game_paths
        .iter()
        .map(|path| crate::zip::split_member_path(Path::new(path)).is_none())
        .collect();
    needs_many_with_fallback(
        gamelist_path,
        folder,
        relative_game_paths,
        &fallback,
        image_policy,
        metadata_policy,
    )
}

#[cfg(test)]
pub fn needs_many_with_fallback(
    gamelist_path: &Path,
    folder: &Path,
    relative_game_paths: &[String],
    metadata_fallback: &[bool],
    image_policy: ImagePolicy,
    metadata_policy: MetadataPolicy,
) -> Result<Vec<Result<Needs>>> {
    Ok(needs_many_controlled(
        gamelist_path,
        folder,
        relative_game_paths,
        metadata_fallback,
        image_policy,
        metadata_policy,
        &mut |_| Ok(()),
    )?
    .into_iter()
    .map(|result| {
        result.and_then(|eligibility| match eligibility {
            Eligibility::Needs(needs) => Ok(needs),
            Eligibility::Alias { .. } => {
                Err(Error::local("target is a verified alias of another target"))
            }
        })
    })
    .collect::<Vec<_>>())
}

pub fn needs_many_controlled(
    gamelist_path: &Path,
    folder: &Path,
    relative_game_paths: &[String],
    metadata_fallback: &[bool],
    image_policy: ImagePolicy,
    metadata_policy: MetadataPolicy,
    progress: &mut impl FnMut(usize) -> Result<()>,
) -> Result<Vec<Result<Eligibility>>> {
    progress(0)?;
    if metadata_fallback.len() != relative_game_paths.len() {
        return Err(Error::local(
            "metadata policy count does not match scrape targets",
        ));
    }
    let Some(bytes) = read_optional(gamelist_path)? else {
        return Ok(relative_game_paths
            .iter()
            .map(|_| {
                Ok(Eligibility::Needs(Needs {
                    image: image_policy != ImagePolicy::Off,
                    metadata: metadata_policy != MetadataPolicy::Off,
                }))
            })
            .collect());
    };
    let document = parse_document(&bytes, gamelist_path)?;
    let (resolved, aliases) = resolve_games_at(
        &document,
        relative_game_paths,
        metadata_fallback,
        Some(folder),
        progress,
    )?;
    let representatives: HashMap<usize, usize> = resolved
        .iter()
        .enumerate()
        .filter(|(at, _)| !aliases.contains(at))
        .filter_map(|(at, result)| match result {
            Ok(Some(entry)) => Some((*entry, at)),
            _ => None,
        })
        .collect();
    let mut parents: HashMap<&str, Vec<&Game>> = HashMap::new();
    for (at, game) in document.games.iter().enumerate() {
        if at % 256 == 0 {
            progress(0)?;
        }
        if let Some(id) = game.id.as_deref() {
            parents.entry(id).or_default().push(game);
        }
    }
    resolved
        .into_iter()
        .enumerate()
        .map(|(at, result)| {
            progress(at)?;
            if aliases.contains(&at) {
                let representative = match &result {
                    Ok(Some(entry)) => representatives.get(entry).copied(),
                    _ => None,
                }
                .ok_or_else(|| Error::local("verified alias has no representative target"))?;
                return Ok(Ok(Eligibility::Alias { representative }));
            }
            Ok((|| {
                let game = match result? {
                    Some(index) => &document.games[index],
                    None => {
                        return Ok(Eligibility::Needs(Needs {
                            image: image_policy != ImagePolicy::Off,
                            metadata: metadata_policy != MetadataPolicy::Off,
                        }))
                    }
                };
                let parent = || -> Result<Option<&Game>> {
                    let Some(id) = game.parent_id.as_deref() else {
                        return Ok(None);
                    };
                    let Some(matches) = parents.get(id) else {
                        return Ok(None);
                    };
                    if matches.len() > 1 {
                        return Err(Error::new(
                            ErrorKind::Local,
                            format!("gamelist contains more than one parent with id {id}"),
                        ));
                    }
                    Ok(matches.first().copied())
                };
                let image = match image_policy {
                    ImagePolicy::Off => false,
                    ImagePolicy::ReplaceExisting => true,
                    ImagePolicy::MissingOnly => match if let Some(art) = own_art(game) {
                        Some(art)
                    } else {
                        parent()?.and_then(own_art)
                    } {
                        Some(field) if !field.value.trim().is_empty() => {
                            !resolve_relative(folder, field.value.trim()).is_file()
                        }
                        _ => true,
                    },
                };
                let metadata = match metadata_policy {
                    MetadataPolicy::Off => false,
                    MetadataPolicy::ReplaceExisting => true,
                    MetadataPolicy::FillMissing => {
                        let mut missing = false;
                        for name in FIELDS {
                            let own = game
                                .fields
                                .get(name)
                                .filter(|field| !field.value.trim().is_empty());
                            let effective = match own {
                                Some(field) => Some(field),
                                None => parent()?.and_then(|parent| {
                                    parent
                                        .fields
                                        .get(name)
                                        .filter(|field| !field.value.trim().is_empty())
                                }),
                            };
                            if effective.is_none() {
                                missing = true;
                                break;
                            }
                        }
                        missing
                    }
                };
                Ok(Eligibility::Needs(Needs { image, metadata }))
            })())
        })
        .collect()
}

#[cfg(test)]
pub fn needs(
    gamelist_path: &Path,
    folder: &Path,
    relative_game_path: &str,
    image_policy: ImagePolicy,
    metadata_policy: MetadataPolicy,
) -> Result<Needs> {
    needs_many(
        gamelist_path,
        folder,
        &[relative_game_path.to_string()],
        image_policy,
        metadata_policy,
    )?
    .pop()
    .unwrap_or_else(|| Err(Error::local("the gamelist inspection returned no result")))
}

#[cfg(test)]
fn needs_image(
    gamelist_path: &Path,
    folder: &Path,
    relative_game_path: &str,
    policy: ImagePolicy,
) -> Result<bool> {
    needs(
        gamelist_path,
        folder,
        relative_game_path,
        policy,
        MetadataPolicy::Off,
    )
    .map(|needs| needs.image)
}

#[derive(Debug, Clone)]
pub struct Update {
    pub relative_game_path: String,
    pub metadata: Metadata,
    pub image_path: Option<String>,
    /// The active scrape created the image file even if the gamelist already
    /// pointed at that exact path. This repairs a broken existing reference
    /// and still requires the browser cache to be refreshed.
    pub image_created: bool,
    pub image_policy: ImagePolicy,
    pub metadata_policy: MetadataPolicy,
}

#[cfg(test)]
pub fn apply_many(
    gamelist_path: &Path,
    folder: &Path,
    updates: &[Update],
    backups: &mut Backups,
) -> Result<Vec<Result<Change>>> {
    let fallback: Vec<bool> = updates
        .iter()
        .map(|update| {
            crate::zip::split_member_path(Path::new(&update.relative_game_path)).is_none()
        })
        .collect();
    apply_many_with_fallback(gamelist_path, folder, updates, &fallback, backups)
}

pub fn apply_many_with_fallback(
    gamelist_path: &Path,
    folder: &Path,
    updates: &[Update],
    metadata_fallback: &[bool],
    backups: &mut Backups,
) -> Result<Vec<Result<Change>>> {
    if metadata_fallback.len() != updates.len() {
        return Err(Error::local(
            "metadata policy count does not match scrape updates",
        ));
    }
    if updates.is_empty() {
        return Ok(Vec::new());
    }

    let original = read_optional(gamelist_path)?;
    let source = original
        .as_deref()
        .unwrap_or(b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<gameList>\n</gameList>\n");
    let document = parse_document(source, gamelist_path)?;
    let relative_paths: Vec<String> = updates
        .iter()
        .map(|update| update.relative_game_path.clone())
        .collect();
    let resolved = resolve_games(&document, &relative_paths, metadata_fallback)?;
    let mut outcomes = vec![Ok(Change::default()); updates.len()];
    let mut replacements = Vec::new();
    let mut appended = Vec::new();

    for (at, (update, resolved)) in updates.iter().zip(resolved).enumerate() {
        let edits = match resolved {
            Ok(Some(index)) => {
                let game = &document.games[index];
                if crate::zip::split_member_path(Path::new(&update.relative_game_path)).is_some()
                    && game.path != normalise_rel(&update.relative_game_path)
                {
                    append_member_from_legacy(source, &document, game, folder, update)
                } else {
                    existing_edits(source, &document, game, folder, update)
                }
            }
            Ok(None) => append_game_bytes(source, &document, update)
                .map(|(bytes, change)| (Vec::new(), bytes, change)),
            Err(error) => {
                outcomes[at] = Err(error);
                continue;
            }
        };
        match edits {
            Ok((item_replacements, insertion, change)) => {
                replacements.extend(item_replacements);
                appended.extend(insertion);
                outcomes[at] = Ok(change);
            }
            Err(error) => outcomes[at] = Err(error),
        }
    }

    let document_changed = !replacements.is_empty() || !appended.is_empty();
    if !appended.is_empty() {
        if let Some(root) = &document.empty_root {
            let mut expanded = source[root.whole.clone()].to_vec();
            let slash = expanded
                .iter()
                .rposition(|byte| *byte == b'/')
                .ok_or_else(|| Error::local("invalid self-closing gamelist root"))?;
            expanded.remove(slash);
            expanded.extend_from_slice(&appended);
            expanded.extend_from_slice(format!("</{}>", root.name).as_bytes());
            replacements.push((root.whole.clone(), expanded));
        } else {
            replacements.push((document.root_close..document.root_close, appended));
        }
    }
    if !document_changed {
        return Ok(outcomes);
    }

    let proposed = apply_replacements(source, replacements)?;
    ensure_proposed_size(proposed.len(), gamelist_path)?;
    let text = std::str::from_utf8(&proposed).map_err(|_| {
        Error::new(
            ErrorKind::Local,
            "the proposed gamelist was not valid UTF-8",
        )
    })?;
    crate::gamelist::Gamelist::parse(text, folder, gamelist_path).map_err(|error| {
        Error::new(
            ErrorKind::Local,
            format!("the proposed gamelist did not pass Degauss validation: {error}"),
        )
    })?;

    if let Some(original) = original.as_deref() {
        backups.ensure(gamelist_path, original)?;
    }
    atomic_replace(gamelist_path, &proposed)?;
    Ok(outcomes)
}

fn ensure_proposed_size(size: usize, path: &Path) -> Result<()> {
    if size as u64 > MAX_GAMELIST_BYTES {
        return Err(Error::new(
            ErrorKind::Local,
            format!(
                "the proposed {} would be {size} bytes, past the {MAX_GAMELIST_BYTES}-byte gamelist limit",
                path.display()
            ),
        ));
    }
    Ok(())
}

/// Every artwork path directly referenced by the current on-disk gamelist.
///
/// This is used only when cleaning up a download made by the active scrape.
/// A parse failure is returned to the caller so it can retain the file rather
/// than risk deleting media that may still be in use.
pub fn referenced_art_paths(gamelist_path: &Path) -> Result<HashSet<String>> {
    let Some(bytes) = read_optional(gamelist_path)? else {
        return Ok(HashSet::new());
    };
    let document = parse_document(&bytes, gamelist_path)?;
    Ok(document
        .games
        .iter()
        .flat_map(|game| {
            ART_FIELDS.iter().filter_map(|field| {
                game.fields
                    .get(*field)
                    .map(|value| normalise_rel(value.value.trim()))
                    .filter(|value| !value.is_empty())
            })
        })
        .collect())
}

#[cfg(test)]
pub struct Apply<'a> {
    pub gamelist_path: &'a Path,
    pub folder: &'a Path,
    pub relative_game_path: &'a str,
    pub metadata: &'a Metadata,
    pub image_path: Option<&'a str>,
    pub image_policy: ImagePolicy,
    pub metadata_policy: MetadataPolicy,
}

#[cfg(test)]
pub fn apply(update: Apply<'_>, backups: &mut Backups) -> Result<Change> {
    let owned = Update {
        relative_game_path: update.relative_game_path.to_string(),
        metadata: update.metadata.clone(),
        image_path: update.image_path.map(str::to_string),
        image_created: false,
        image_policy: update.image_policy,
        metadata_policy: update.metadata_policy,
    };
    apply_many(
        update.gamelist_path,
        update.folder,
        std::slice::from_ref(&owned),
        backups,
    )?
    .pop()
    .unwrap_or_else(|| Err(Error::local("the gamelist update returned no result")))
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>> {
    let size = match std::fs::metadata(path) {
        Ok(metadata) => metadata.len(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(Error::local(format!(
                "could not inspect {}: {error}",
                path.display()
            )))
        }
    };
    if size > MAX_GAMELIST_BYTES {
        return Err(Error::new(
            ErrorKind::Local,
            format!(
                "{} is {size} bytes, past the {MAX_GAMELIST_BYTES}-byte gamelist limit",
                path.display()
            ),
        ));
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|error| Error::local(format!("could not open {}: {error}", path.display())))?
        .take(MAX_GAMELIST_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| Error::local(format!("could not read {}: {error}", path.display())))?;
    if bytes.len() as u64 > MAX_GAMELIST_BYTES {
        return Err(Error::new(
            ErrorKind::Local,
            format!(
                "{} grew past the {MAX_GAMELIST_BYTES}-byte gamelist limit while it was read",
                path.display()
            ),
        ));
    }
    Ok(Some(bytes))
}

fn parse_document(bytes: &[u8], origin: &Path) -> Result<Document> {
    let text = std::str::from_utf8(bytes).map_err(|_| {
        Error::new(
            ErrorKind::Local,
            format!("{} is not UTF-8 XML", origin.display()),
        )
    })?;
    let mut reader = Reader::from_str(text);
    let mut depth = 0usize;
    let mut game: Option<OpenGame> = None;
    let mut games = Vec::new();
    let mut root_depth = None;
    let mut root_close = None;
    let mut empty_root = None;

    loop {
        let before = usize::try_from(reader.buffer_position()).unwrap_or(usize::MAX);
        match reader.read_event() {
            Ok(Event::Eof) => break,
            Err(error) => {
                return Err(Error::new(
                    ErrorKind::Local,
                    format!(
                        "{} is malformed XML at {}: {error}",
                        origin.display(),
                        reader.buffer_position()
                    ),
                ))
            }
            Ok(Event::Start(element)) => {
                let name = element.name().as_ref().to_ascii_lowercase();
                let attributes = xml_attributes(&element, origin)?;
                depth += 1;
                if name == "gamelist" && root_depth.is_none() {
                    root_depth = Some(depth);
                } else if name == "game" && game.is_none() {
                    let mut id = None;
                    let mut parent_id = None;
                    for (key, value) in attributes {
                        if key.eq_ignore_ascii_case("id") {
                            id = Some(value);
                        } else if key.eq_ignore_ascii_case("parentid") {
                            parent_id = Some(value);
                        }
                    }
                    game = Some(OpenGame {
                        whole_start: before,
                        depth,
                        id,
                        parent_id,
                        path: String::new(),
                        fields: BTreeMap::new(),
                        field: None,
                    });
                } else if let Some(open) = game.as_mut() {
                    if depth == open.depth + 1
                        && (name == "path"
                            || FIELDS.contains(&name.as_str())
                            || ART_FIELDS.contains(&name.as_str()))
                    {
                        open.field = Some(OpenField {
                            name: name.clone(),
                            whole_start: before,
                            content_start: usize::try_from(reader.buffer_position())
                                .unwrap_or(usize::MAX),
                            depth,
                            text: String::new(),
                        });
                    }
                }
            }
            Ok(Event::Empty(element)) => {
                let name = element.name().as_ref().to_ascii_lowercase();
                xml_attributes(&element, origin)?;
                if name == "gamelist" && root_depth.is_none() && root_close.is_none() {
                    let after = usize::try_from(reader.buffer_position()).unwrap_or(usize::MAX);
                    let close = after.checked_sub(2).ok_or_else(|| {
                        Error::local(format!(
                            "{} has an invalid <gameList/> root",
                            origin.display()
                        ))
                    })?;
                    root_close = Some(close);
                    empty_root = Some(EmptyRoot {
                        whole: before..after,
                        name: element.name().as_ref().to_string(),
                    });
                } else if let Some(open) = game.as_mut() {
                    if depth + 1 == open.depth + 1
                        && (name == "path"
                            || FIELDS.contains(&name.as_str())
                            || ART_FIELDS.contains(&name.as_str()))
                    {
                        let after = usize::try_from(reader.buffer_position()).unwrap_or(usize::MAX);
                        if name == "path" {
                            open.path.clear();
                        }
                        open.fields.insert(
                            name.clone(),
                            Field {
                                whole: before..after,
                                content: None,
                                value: String::new(),
                            },
                        );
                    }
                }
            }
            Ok(Event::Text(value)) => {
                if let Some(field) = game.as_mut().and_then(|game| game.field.as_mut()) {
                    field.text.push_str(&value.xml10_content());
                }
            }
            Ok(Event::CData(value)) => {
                if let Some(field) = game.as_mut().and_then(|game| game.field.as_mut()) {
                    field.text.push_str(value.as_ref());
                }
            }
            Ok(Event::GeneralRef(value)) => {
                if let Some(field) = game.as_mut().and_then(|game| game.field.as_mut()) {
                    let name = value.into_inner();
                    if let Some(resolved) = quick_xml::escape::resolve_predefined_entity(&name) {
                        field.text.push_str(resolved);
                    } else if let Some(resolved) = numeric_entity(&name) {
                        field.text.push(resolved);
                    } else {
                        return Err(Error::new(
                            ErrorKind::Local,
                            format!(
                                "{} contains the unsupported XML entity &{name}; in <{}>",
                                origin.display(),
                                field.name
                            ),
                        ));
                    }
                }
            }
            Ok(Event::End(element)) => {
                let name = element.name().as_ref().to_ascii_lowercase();
                if let Some(open) = game.as_mut() {
                    if open
                        .field
                        .as_ref()
                        .is_some_and(|field| field.depth == depth && field.name == name)
                    {
                        let field = open.field.take().expect("checked above");
                        let value = field.text.trim().to_string();
                        if name == "path" {
                            open.path = normalise_rel(&value);
                        }
                        open.fields.insert(
                            name.clone(),
                            Field {
                                whole: field.whole_start
                                    ..usize::try_from(reader.buffer_position())
                                        .unwrap_or(usize::MAX),
                                content: Some(field.content_start..before),
                                value,
                            },
                        );
                    }
                    if name == "game" && open.depth == depth {
                        let open = game.take().expect("checked above");
                        games.push(Game {
                            whole: open.whole_start
                                ..usize::try_from(reader.buffer_position()).unwrap_or(usize::MAX),
                            close_start: before,
                            id: open.id,
                            parent_id: open.parent_id,
                            path: open.path,
                            fields: open.fields,
                        });
                    }
                }
                if name == "gamelist" && root_depth == Some(depth) {
                    root_close = Some(before);
                }
                depth = depth.saturating_sub(1);
            }
            _ => {}
        }
    }

    let root_close = root_close.ok_or_else(|| {
        Error::new(
            ErrorKind::Local,
            format!("{} has no complete <gameList> root", origin.display()),
        )
    })?;
    Ok(Document {
        games,
        root_close,
        empty_root,
        newline: if text.contains("\r\n") { "\r\n" } else { "\n" },
    })
}

fn xml_attributes(element: &BytesStart<'_>, origin: &Path) -> Result<Vec<(String, String)>> {
    element
        .attributes()
        .map(|attribute| {
            let attribute = attribute.map_err(|error| {
                Error::new(
                    ErrorKind::Local,
                    format!(
                        "{} has a malformed XML attribute: {error}",
                        origin.display()
                    ),
                )
            })?;
            let key = attribute.key.as_ref().to_owned();
            let value = attribute
                .normalized_value(XmlVersion::Implicit1_0)
                .map_err(|error| {
                    Error::new(
                        ErrorKind::Local,
                        format!(
                            "{} has a malformed XML attribute value: {error}",
                            origin.display()
                        ),
                    )
                })?
                .into_owned();
            Ok((key, value))
        })
        .collect()
}

#[cfg(test)]
fn matching_game<'a>(
    document: &'a Document,
    relative: &str,
    metadata_fallback: bool,
) -> Result<Option<&'a Game>> {
    let wanted = normalise_rel(relative);
    if let Some(found) = unique_at(document, relative, |game| game.path == wanted)? {
        return Ok(Some(found));
    }

    if !metadata_fallback {
        return Ok(None);
    }

    let file_name = file_name_of(&wanted).to_ascii_lowercase();
    if let Some(found) = unique_at(document, relative, |game| {
        !game.path.ends_with(".slug") && file_name_of(&game.path).eq_ignore_ascii_case(&file_name)
    })? {
        return Ok(Some(found));
    }

    let stem = stem_of(&file_name).to_ascii_lowercase();
    if let Some(found) = unique_at(document, relative, |game| {
        !game.path.ends_with(".slug")
            && stem_of(file_name_of(&game.path)).eq_ignore_ascii_case(&stem)
    })? {
        return Ok(Some(found));
    }

    let candidates = crate::gamelist::slug_candidates(&stem);
    if let Some(found) = unique_at(document, relative, |game| {
        game.path
            .strip_suffix(".slug")
            .map(crate::gamelist::slugify)
            .is_some_and(|slug| candidates.contains(&slug))
    })? {
        return Ok(Some(found));
    }

    if let Some((archive, _)) = wanted.rsplit_once(".zip/") {
        let archive_name = file_name_of(archive);
        let archive_stem = stem_of(archive_name).to_ascii_lowercase();
        let archive_candidates = crate::gamelist::slug_candidates(&archive_stem);
        if let Some(found) = unique_at(document, relative, |game| {
            game.path
                .strip_suffix(".slug")
                .map(crate::gamelist::slugify)
                .is_some_and(|slug| archive_candidates.contains(&slug))
        })? {
            return Ok(Some(found));
        }
        if let Some(found) = unique_at(document, relative, |game| {
            !game.path.ends_with(".slug")
                && file_name_of(&game.path).eq_ignore_ascii_case(archive_name)
        })? {
            return Ok(Some(found));
        }
        if let Some(found) = unique_at(document, relative, |game| {
            !game.path.ends_with(".slug")
                && stem_of(file_name_of(&game.path)).eq_ignore_ascii_case(&archive_stem)
        })? {
            return Ok(Some(found));
        }
    }
    Ok(None)
}

#[cfg(test)]
fn matching_game_index(
    document: &Document,
    relative: &str,
    metadata_fallback: bool,
) -> Result<Option<usize>> {
    matching_game(document, relative, metadata_fallback).map(|matched| {
        matched.and_then(|matched| {
            document
                .games
                .iter()
                .position(|candidate| std::ptr::eq(candidate, matched))
        })
    })
}

/// Resolve every target against one immutable document snapshot.
///
/// A single existing entry, or a single as-yet missing path, may not receive
/// two scrape results in the same batch. Refusing both targets is safer than
/// choosing an order-dependent winner after title/slug fallback matching.
fn resolve_games(
    document: &Document,
    relative_paths: &[String],
    metadata_fallback: &[bool],
) -> Result<Vec<Result<Option<usize>>>> {
    resolve_games_controlled(document, relative_paths, metadata_fallback, &mut |_| Ok(()))
}

fn resolve_games_controlled(
    document: &Document,
    relative_paths: &[String],
    metadata_fallback: &[bool],
    progress: &mut impl FnMut(usize) -> Result<()>,
) -> Result<Vec<Result<Option<usize>>>> {
    resolve_games_at(document, relative_paths, metadata_fallback, None, progress)
        .map(|(resolved, _)| resolved)
}

type ResolvedGames = (Vec<Result<Option<usize>>>, HashSet<usize>);

fn resolve_games_at(
    document: &Document,
    relative_paths: &[String],
    metadata_fallback: &[bool],
    folder: Option<&Path>,
    progress: &mut impl FnMut(usize) -> Result<()>,
) -> Result<ResolvedGames> {
    let index = MatchIndex::new(document, progress)?;
    let mut resolved = Vec::with_capacity(relative_paths.len());
    for (at, relative) in relative_paths.iter().enumerate() {
        progress(at)?;
        resolved.push(index.find(relative, metadata_fallback[at]));
    }
    let mut existing: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    let mut missing: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (at, result) in resolved.iter().enumerate() {
        progress(at)?;
        match result {
            Ok(Some(index)) => existing.entry(*index).or_default().push(at),
            Ok(None) => missing
                // A case-sensitive USB or network filesystem may contain
                // both `Game.rom` and `game.rom`. The reader resolves their
                // exact relative paths before its case-insensitive fallback,
                // so they are safe, distinct entries when both are absent.
                .entry(normalise_rel(&relative_paths[at]))
                .or_default()
                .push(at),
            Err(_) => {}
        }
    }
    let mut aliases = HashSet::new();
    for (entry, positions) in existing.iter().filter(|(_, positions)| positions.len() > 1) {
        if let Some(folder) = folder {
            match same_file_representative(
                folder,
                &document.games[*entry].path,
                relative_paths,
                positions,
                progress,
            ) {
                Ok(keep) => {
                    aliases.extend(positions.iter().copied().filter(|at| *at != keep));
                    continue;
                }
                Err(error) if error.kind == ErrorKind::Cancelled => return Err(error),
                Err(error) => {
                    for &at in positions {
                        resolved[at] = Err(error.clone());
                    }
                    continue;
                }
            }
        }
        for &at in positions {
            progress(at)?;
            resolved[at] = Err(Error::local(format!(
                "more than one scrape target resolves to the same gamelist entry as {}",
                relative_paths[at]
            )));
        }
    }
    for positions in missing.values().filter(|positions| positions.len() > 1) {
        for &at in positions {
            progress(at)?;
            resolved[at] = Err(Error::new(
                ErrorKind::Local,
                format!(
                    "more than one scrape target resolves to the same gamelist entry as {}",
                    relative_paths[at]
                ),
            ));
        }
    }
    Ok((resolved, aliases))
}

fn same_file_representative(
    folder: &Path,
    existing_path: &str,
    relative_paths: &[String],
    positions: &[usize],
    progress: &mut impl FnMut(usize) -> Result<()>,
) -> Result<usize> {
    let mut physical = None;
    for &at in positions {
        progress(at)?;
        let path = folder.join(normalise_rel(&relative_paths[at]));
        let canonical = path.canonicalize().map_err(|error| {
            Error::local(format!(
                "could not verify scrape alias {}: {error}",
                path.display()
            ))
        })?;
        let metadata = std::fs::metadata(&canonical).map_err(|error| {
            Error::local(format!(
                "could not inspect scrape alias {}: {error}",
                path.display()
            ))
        })?;
        if !metadata.is_file() || physical.as_ref().is_some_and(|first| first != &canonical) {
            return Err(Error::local(format!("more than one scrape target resolves to the same gamelist entry, but they are not the same file: {}", relative_paths[at])));
        }
        physical = Some(canonical);
    }
    positions
        .iter()
        .copied()
        .min_by_key(|&at| {
            let path = normalise_rel(&relative_paths[at]);
            (path != existing_path, path, at)
        })
        .ok_or_else(|| Error::local("scrape alias group contains no targets"))
}

/// One immutable document's lookup tables preserve the reader's precedence.
/// All matching positions are retained: an ambiguous key must not pick a winner.
struct MatchIndex {
    exact: HashMap<String, Vec<usize>>,
    names: HashMap<String, Vec<usize>>,
    stems: HashMap<String, Vec<usize>>,
    slugs: HashMap<String, Vec<usize>>,
}

impl MatchIndex {
    fn new(document: &Document, progress: &mut impl FnMut(usize) -> Result<()>) -> Result<Self> {
        let mut index = Self {
            exact: HashMap::new(),
            names: HashMap::new(),
            stems: HashMap::new(),
            slugs: HashMap::new(),
        };
        for (at, game) in document.games.iter().enumerate() {
            if at % 256 == 0 {
                progress(0)?;
            }
            if game.path.is_empty() {
                continue;
            }
            index.exact.entry(game.path.clone()).or_default().push(at);
            if let Some(slug) = game.path.strip_suffix(".slug") {
                index
                    .slugs
                    .entry(crate::gamelist::slugify(slug))
                    .or_default()
                    .push(at);
            } else {
                let name = file_name_of(&game.path).to_ascii_lowercase();
                index
                    .stems
                    .entry(stem_of(&name).to_ascii_lowercase())
                    .or_default()
                    .push(at);
                index.names.entry(name).or_default().push(at);
            }
        }
        Ok(index)
    }

    fn unique<'a>(
        values: impl Iterator<Item = &'a usize>,
        relative: &str,
    ) -> Result<Option<usize>> {
        let mut first = None;
        for &at in values {
            if first.is_some_and(|first| first != at) {
                return Err(Error::new(
                    ErrorKind::Local,
                    format!("gamelist contains more than one matching entry for {relative}"),
                ));
            }
            first = Some(at);
        }
        Ok(first)
    }

    fn lookup(
        map: &HashMap<String, Vec<usize>>,
        key: &str,
        relative: &str,
    ) -> Result<Option<usize>> {
        Self::unique(map.get(key).into_iter().flatten(), relative)
    }

    fn slug(&self, stem: &str, relative: &str) -> Result<Option<usize>> {
        let candidates = crate::gamelist::slug_candidates(stem);
        Self::unique(
            candidates
                .iter()
                .filter_map(|key| self.slugs.get(key))
                .flatten(),
            relative,
        )
    }

    fn find(&self, relative: &str, fallback: bool) -> Result<Option<usize>> {
        let wanted = normalise_rel(relative);
        if let Some(at) = Self::lookup(&self.exact, &wanted, relative)? {
            return Ok(Some(at));
        }
        if !fallback {
            return Ok(None);
        }
        let name = file_name_of(&wanted).to_ascii_lowercase();
        if let Some(at) = Self::lookup(&self.names, &name, relative)? {
            return Ok(Some(at));
        }
        let stem = stem_of(&name).to_ascii_lowercase();
        if let Some(at) = Self::lookup(&self.stems, &stem, relative)? {
            return Ok(Some(at));
        }
        if let Some(at) = self.slug(&stem, relative)? {
            return Ok(Some(at));
        }
        if let Some((archive, _)) = wanted.rsplit_once(".zip/") {
            let name = file_name_of(archive).to_ascii_lowercase();
            let stem = stem_of(&name).to_ascii_lowercase();
            if let Some(at) = self.slug(&stem, relative)? {
                return Ok(Some(at));
            }
            if let Some(at) = Self::lookup(&self.names, &name, relative)? {
                return Ok(Some(at));
            }
            if let Some(at) = Self::lookup(&self.stems, &stem, relative)? {
                return Ok(Some(at));
            }
        }
        Ok(None)
    }
}
#[cfg(test)]
fn unique_at<'a>(
    document: &'a Document,
    relative: &str,
    matches: impl Fn(&Game) -> bool,
) -> Result<Option<&'a Game>> {
    let mut found = document
        .games
        .iter()
        .filter(|game| !game.path.is_empty() && matches(game));
    let first = found.next();
    if first.is_some() && found.next().is_some() {
        return Err(Error::new(
            ErrorKind::Local,
            format!("gamelist contains more than one matching entry for {relative}"),
        ));
    }
    Ok(first)
}

fn file_name_of(value: &str) -> &str {
    value.rsplit('/').next().unwrap_or(value)
}

fn stem_of(value: &str) -> &str {
    value
        .rfind('.')
        .filter(|index| *index > 0)
        .map(|index| &value[..index])
        .unwrap_or(value)
}

fn parent_game<'a>(document: &'a Document, game: &Game) -> Result<Option<&'a Game>> {
    let Some(parent_id) = game.parent_id.as_deref() else {
        return Ok(None);
    };
    let mut parents = document
        .games
        .iter()
        .filter(|candidate| candidate.id.as_deref() == Some(parent_id));
    let first = parents.next();
    if first.is_some() && parents.next().is_some() {
        return Err(Error::new(
            ErrorKind::Local,
            format!("gamelist contains more than one parent with id {parent_id}"),
        ));
    }
    Ok(first)
}

fn effective_field<'a>(
    document: &'a Document,
    game: &'a Game,
    name: &str,
) -> Result<Option<&'a Field>> {
    if let Some(field) = game.fields.get(name) {
        if !field.value.trim().is_empty() {
            return Ok(Some(field));
        }
    }
    Ok(parent_game(document, game)?.and_then(|parent| {
        parent
            .fields
            .get(name)
            .filter(|field| !field.value.trim().is_empty())
    }))
}

fn effective_art<'a>(document: &'a Document, game: &'a Game) -> Result<Option<&'a Field>> {
    // Match `RawGame::art().or_else(parent.art())` in the production
    // reader: any artwork on the child wins before the parent's own
    // image/screenshot/thumbnail precedence is considered.
    if let Some(field) = own_art(game) {
        return Ok(Some(field));
    }
    Ok(parent_game(document, game)?.and_then(own_art))
}

fn own_art(game: &Game) -> Option<&Field> {
    ART_FIELDS.iter().find_map(|name| {
        game.fields
            .get(*name)
            .filter(|field| !field.value.trim().is_empty())
    })
}

fn existing_edits(
    source: &[u8],
    document: &Document,
    game: &Game,
    folder: &Path,
    update: &Update,
) -> Result<ExistingEdits> {
    let mut replacements: Vec<Replacement> = Vec::new();
    let mut additions: Vec<(&str, &str)> = Vec::new();
    let mut change = Change::default();

    if update.metadata_policy != MetadataPolicy::Off {
        for (name, value) in update.metadata.fields() {
            let Some(value) = value.filter(|value| !value.trim().is_empty()) else {
                continue;
            };
            let existing = game.fields.get(name);
            let effective = effective_field(document, game, name)?;
            let should_write = match update.metadata_policy {
                MetadataPolicy::Off => false,
                MetadataPolicy::FillMissing => {
                    effective.is_none_or(|field| field.value.trim().is_empty())
                }
                MetadataPolicy::ReplaceExisting => {
                    effective.is_none_or(|field| field.value.trim() != value.trim())
                }
            };
            if should_write
                && replace_or_add(name, value, existing, &mut replacements, &mut additions)
            {
                change.metadata_fields += 1;
            }
        }
    }

    if update.image_policy != ImagePolicy::Off {
        if let Some(value) = update
            .image_path
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            let existing = game.fields.get("image");
            let effective = effective_art(document, game)?;
            let should_write = match update.image_policy {
                ImagePolicy::Off => false,
                ImagePolicy::ReplaceExisting => {
                    effective.is_none_or(|field| field.value.trim() != value.trim())
                }
                ImagePolicy::MissingOnly => effective.is_none_or(|field| {
                    field.value.trim().is_empty()
                        || !resolve_relative(folder, field.value.trim()).is_file()
                }),
            };
            if should_write
                && replace_or_add("image", value, existing, &mut replacements, &mut additions)
            {
                change.image = true;
            } else if update.image_created
                && effective
                    .is_some_and(|field| same_media_reference(field.value.trim(), value.trim()))
            {
                // The XML needs no edit, but a file that this exact effective
                // reference could not open before the scrape now exists.
                change.image = true;
            }
        }
    }

    if !additions.is_empty() {
        let indent = child_indent(source, game, document.newline);
        let mut bytes = Vec::new();
        if !line_starts_at(source, game.close_start) {
            bytes.extend_from_slice(document.newline.as_bytes());
        }
        for (name, value) in additions {
            bytes.extend_from_slice(indent.as_bytes());
            bytes.extend_from_slice(element(name, value).as_bytes());
            bytes.extend_from_slice(document.newline.as_bytes());
        }
        replacements.push((game.close_start..game.close_start, bytes));
    }
    Ok((replacements, Vec::new(), change))
}

/// Materialize a single-member legacy overlay at its exact member path before
/// editing it. Existing fields, unknown tags and parent links survive; the
/// shared archive/title row is never modified or given a duplicate id.
fn append_member_from_legacy(
    source: &[u8],
    document: &Document,
    game: &Game,
    folder: &Path,
    update: &Update,
) -> Result<ExistingEdits> {
    let (mut replacements, _, mut change) = existing_edits(source, document, game, folder, update)?;
    if !change.changed() {
        return Ok((Vec::new(), Vec::new(), change));
    }
    let path = game
        .fields
        .get("path")
        .ok_or_else(|| Error::local("legacy metadata entry has no path field"))?;
    replacements.push((
        path.whole.clone(),
        element("path", &update.relative_game_path).into_bytes(),
    ));
    let original = &source[game.whole.clone()];
    let mut reader = Reader::from_reader(original);
    if let Event::Start(start) = reader
        .read_event()
        .map_err(|error| Error::local(error.to_string()))?
    {
        let mut opening = String::from("<game");
        for (key, value) in xml_attributes(&start, folder)? {
            if !key.eq_ignore_ascii_case("id") {
                opening.push_str(&format!(
                    " {key}=\"{}\"",
                    escape(&value).replace('"', "&quot;")
                ));
            }
        }
        opening.push('>');
        replacements.push((
            game.whole.start..game.whole.start + reader.buffer_position() as usize,
            opening.into_bytes(),
        ));
    }
    for (range, _) in &mut replacements {
        range.start -= game.whole.start;
        range.end -= game.whole.start;
    }
    let cloned = apply_replacements(original, replacements)?;
    let mut insertion = Vec::new();
    insertion.extend_from_slice(document.newline.as_bytes());
    insertion.extend_from_slice(&cloned);
    insertion.extend_from_slice(document.newline.as_bytes());
    change.created_entry = true;
    Ok((Vec::new(), insertion, change))
}

fn append_game_bytes(
    source: &[u8],
    document: &Document,
    update: &Update,
) -> Result<(Vec<u8>, Change)> {
    let mut fields: Vec<(&str, &str)> = Vec::new();
    fields.push(("path", update.relative_game_path.as_str()));
    if update.metadata_policy != MetadataPolicy::Off {
        fields.extend(
            update
                .metadata
                .fields()
                .into_iter()
                .filter_map(|(name, value)| {
                    value
                        .filter(|value| !value.trim().is_empty())
                        .map(|value| (name, value))
                }),
        );
    }
    if update.image_policy != ImagePolicy::Off {
        if let Some(image) = update
            .image_path
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            fields.push(("image", image));
        }
    }
    if fields.len() == 1 {
        return Ok((Vec::new(), Change::default()));
    }

    let game_indent = document
        .games
        .first()
        .map(|game| indentation_at(source, game.whole.start))
        .filter(|indent| !indent.is_empty())
        .unwrap_or_else(|| "  ".to_string());
    let field_indent = format!("{game_indent}  ");
    let mut insertion = Vec::new();
    if !line_starts_at(source, document.root_close) {
        insertion.extend_from_slice(document.newline.as_bytes());
    }
    insertion.extend_from_slice(game_indent.as_bytes());
    insertion.extend_from_slice(b"<game>");
    insertion.extend_from_slice(document.newline.as_bytes());
    for (name, value) in fields {
        insertion.extend_from_slice(field_indent.as_bytes());
        insertion.extend_from_slice(element(name, value).as_bytes());
        insertion.extend_from_slice(document.newline.as_bytes());
    }
    insertion.extend_from_slice(game_indent.as_bytes());
    insertion.extend_from_slice(b"</game>");
    insertion.extend_from_slice(document.newline.as_bytes());

    let metadata_fields = if update.metadata_policy == MetadataPolicy::Off {
        0
    } else {
        update
            .metadata
            .fields()
            .into_iter()
            .filter(|(_, value)| value.is_some_and(|value| !value.trim().is_empty()))
            .count()
    };
    Ok((
        insertion,
        Change {
            metadata_fields,
            image: update.image_policy != ImagePolicy::Off && update.image_path.is_some(),
            created_entry: true,
        },
    ))
}

fn replace_or_add<'a>(
    name: &'a str,
    value: &'a str,
    existing: Option<&Field>,
    replacements: &mut Vec<Replacement>,
    additions: &mut Vec<(&'a str, &'a str)>,
) -> bool {
    if let Some(field) = existing {
        if field.value.trim() == value.trim() {
            return false;
        }
        let replacement = escape(value).into_bytes();
        if let Some(content) = field.content.clone() {
            replacements.push((content, replacement));
        } else {
            replacements.push((field.whole.clone(), element(name, value).into_bytes()));
        }
    } else {
        additions.push((name, value));
    }
    true
}

fn apply_replacements(source: &[u8], mut replacements: Vec<Replacement>) -> Result<Vec<u8>> {
    replacements.sort_by_key(|replacement| std::cmp::Reverse(replacement.0.start));
    let mut last_start = source.len().saturating_add(1);
    let mut output = source.to_vec();
    for (range, bytes) in replacements {
        if range.start > range.end || range.end > output.len() || range.end > last_start {
            return Err(Error::new(
                ErrorKind::Local,
                "overlapping or invalid gamelist edit",
            ));
        }
        output.splice(range.clone(), bytes);
        last_start = range.start;
    }
    Ok(output)
}

fn child_indent(source: &[u8], game: &Game, newline: &str) -> String {
    game.fields
        .values()
        .next()
        .map(|field| indentation_at(source, field.whole.start))
        .filter(|indent| !indent.is_empty())
        .unwrap_or_else(|| format!("{}  ", indentation_at(source, game.whole.start)))
        .replace('\r', "")
        .replace('\n', newline)
}

fn indentation_at(source: &[u8], at: usize) -> String {
    let line = source[..at.min(source.len())]
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map(|index| index + 1)
        .unwrap_or(0);
    source[line..at.min(source.len())]
        .iter()
        .take_while(|byte| **byte == b' ' || **byte == b'\t')
        .map(|byte| *byte as char)
        .collect()
}

fn line_starts_at(source: &[u8], at: usize) -> bool {
    source[..at.min(source.len())]
        .iter()
        .rposition(|byte| *byte == b'\n')
        .is_none_or(|line| source[line + 1..at].iter().all(u8::is_ascii_whitespace))
}

fn resolve_relative(folder: &Path, value: &str) -> PathBuf {
    // Keep this identical to the established read path in `gamelist.rs`:
    // older gamelists sometimes begin a root-relative value with `/`, and
    // Degauss has always treated that as relative to the gamelist folder.
    folder.join(normalise_rel(value))
}

fn normalise_rel(value: &str) -> String {
    let value = value.trim().replace('\\', "/");
    value
        .strip_prefix("./")
        .unwrap_or(&value)
        .trim_start_matches('/')
        .to_string()
}

/// FAT and exFAT resolve media names case-insensitively. Preserve exact-case
/// behavior for ordinary file existence checks, but recognise a reference to
/// the just-created content file when only its ASCII case differs.
fn same_media_reference(left: &str, right: &str) -> bool {
    normalise_rel(left).eq_ignore_ascii_case(&normalise_rel(right))
}

fn numeric_entity(name: &str) -> Option<char> {
    let name = name.strip_prefix('#')?;
    let value = match name.strip_prefix(['x', 'X']) {
        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
        None => name.parse().ok()?,
    };
    char::from_u32(value)
}

fn element(name: &str, value: &str) -> String {
    format!("<{name}>{}</{name}>", escape(value))
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn write_new(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

fn atomic_replace(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)
        .map_err(|error| Error::local(format!("could not create {}: {error}", parent.display())))?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("gamelist.xml");
    let temp = parent.join(format!(".{file_name}.degauss-{}.part", std::process::id()));
    if temp.exists() {
        std::fs::remove_file(&temp).map_err(|error| {
            Error::local(format!(
                "could not clear an old temporary gamelist: {error}"
            ))
        })?;
    }
    let outcome = (|| {
        write_new(&temp, bytes).map_err(|error| {
            Error::local(format!("could not write temporary gamelist: {error}"))
        })?;
        std::fs::rename(&temp, path)
            .map_err(|error| Error::local(format!("could not install {}: {error}", path.display())))
    })();
    if outcome.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    outcome
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    #[test]
    fn real_alias_groups_share_one_target_without_weakening_writer_guards() {
        use std::os::unix::fs::symlink;
        let root = temp("verified-aliases");
        for directory in ["a", "z", "separate"] {
            std::fs::create_dir_all(root.join(directory)).unwrap();
        }
        std::fs::write(root.join("Game.rom"), b"same physical game").unwrap();
        for alias in ["a/Game.rom", "z/Game.rom", "separate/Game.rom"] {
            symlink(root.join("Game.rom"), root.join(alias)).unwrap();
        }
        let xml = "<gameList><game><path>./Game.rom</path><name>Game</name></game></gameList>";
        let document = super::parse_document(xml.as_bytes(), &root.join("gamelist.xml")).unwrap();
        let paths = vec!["z/Game.rom".into(), "Game.rom".into(), "a/Game.rom".into()];
        for _ in 0..2 {
            let (resolved, aliases) = super::resolve_games_at(
                &document,
                &paths,
                &[true; 3],
                Some(&root),
                &mut |_| Ok(()),
            )
            .unwrap();
            assert!(resolved.iter().all(Result::is_ok));
            assert_eq!(aliases, std::collections::HashSet::from([0, 2]));
        }
        let scoped = vec!["z/Game.rom".into(), "a/Game.rom".into()];
        let (_, aliases) =
            super::resolve_games_at(&document, &scoped, &[true; 2], Some(&root), &mut |_| Ok(()))
                .unwrap();
        assert_eq!(aliases, std::collections::HashSet::from([0]));
        assert!(
            super::resolve_games(&document, &paths, &[true; 3])
                .unwrap()
                .iter()
                .all(Result::is_err),
            "writer still rejects duplicate results for one XML entry"
        );
        let explicit = xml.replace(
            "</gameList>",
            "<game><path>a/Game.rom</path><name>Alias entry</name></game></gameList>",
        );
        let explicit =
            super::parse_document(explicit.as_bytes(), &root.join("gamelist.xml")).unwrap();
        let (_, aliases) = super::resolve_games_at(
            &explicit,
            &["Game.rom".into(), "a/Game.rom".into()],
            &[true; 2],
            Some(&root),
            &mut |_| Ok(()),
        )
        .unwrap();
        assert!(
            aliases.is_empty(),
            "distinct exact XML entries remain separate"
        );
        for folder in [&root, &root.join("separate")] {
            std::fs::write(folder.join("gamelist.xml"), xml).unwrap();
            let outcomes = super::needs_many_controlled(
                &folder.join("gamelist.xml"),
                folder,
                &["Game.rom".into()],
                &[true],
                super::ImagePolicy::MissingOnly,
                super::MetadataPolicy::FillMissing,
                &mut |_| Ok(()),
            )
            .unwrap();
            assert!(
                matches!(outcomes[0], Ok(super::Eligibility::Needs(_))),
                "different gamelists never share planning identity"
            );
        }
        let error =
            super::same_file_representative(&root, "Game.rom", &paths, &[0, 1, 2], &mut |_| {
                Err(super::Error::new(super::ErrorKind::Cancelled, "cancelled"))
            })
            .unwrap_err();
        assert_eq!(error.kind, super::ErrorKind::Cancelled);
        std::fs::remove_file(root.join("a/Game.rom")).unwrap();
        std::fs::write(root.join("a/Game.rom"), b"different game").unwrap();
        let (resolved, aliases) =
            super::resolve_games_at(&document, &paths, &[true; 3], Some(&root), &mut |_| Ok(()))
                .unwrap();
        assert!(aliases.is_empty());
        assert!(resolved.iter().all(|result| result
            .as_ref()
            .unwrap_err()
            .to_string()
            .contains("not the same file")));
        std::fs::remove_file(root.join("a/Game.rom")).unwrap();
        symlink(root.join("missing.rom"), root.join("a/Game.rom")).unwrap();
        let (resolved, aliases) =
            super::resolve_games_at(&document, &paths, &[true; 3], Some(&root), &mut |_| Ok(()))
                .unwrap();
        assert!(aliases.is_empty());
        assert!(resolved.iter().all(|result| result
            .as_ref()
            .unwrap_err()
            .to_string()
            .contains("could not verify scrape alias")));
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn indexed_matching_preserves_linear_precedence_and_ambiguity() {
        let paths = [
            "Case.rom",
            "case.rom",
            "dir/one.rom",
            "other/one.bin",
            "two.slug",
            "two!.slug",
            "legacy.zip",
            "collection.zip/member.rom",
            "duplicate.rom",
            "duplicate.rom",
        ];
        let xml = format!(
            "<gameList>{}</gameList>",
            paths
                .iter()
                .map(|path| format!("<game><path>{path}</path></game>"))
                .collect::<String>()
        );
        let document =
            super::parse_document(xml.as_bytes(), std::path::Path::new("test.xml")).unwrap();
        let index = super::MatchIndex::new(&document, &mut |_| Ok(())).unwrap();
        for query in [
            "./Case.rom",
            "case.rom",
            "else/CASE.rom",
            "one.rom",
            "one.xyz",
            "two.rom",
            "legacy.zip/member.rom",
            "collection.zip/member.rom",
            "duplicate.rom",
            "missing.rom",
        ] {
            for fallback in [false, true] {
                let old = super::matching_game_index(&document, query, fallback)
                    .map_err(|error| error.to_string());
                let new = index
                    .find(query, fallback)
                    .map_err(|error| error.to_string());
                assert_eq!(new, old, "{query}, fallback={fallback}");
            }
        }
        let relative = vec!["dir/one.rom".into(), "else/one.rom".into()];
        let outcomes = super::resolve_games(&document, &relative, &[true, true]).unwrap();
        assert!(outcomes.iter().all(|result| result
            .as_ref()
            .unwrap_err()
            .to_string()
            .contains("more than one scrape target")));
    }

    #[test]
    fn eligibility_resolution_can_cancel_inside_one_document() {
        let xml = "<gameList><game><path>one.rom</path></game></gameList>";
        let document =
            super::parse_document(xml.as_bytes(), std::path::Path::new("test.xml")).unwrap();
        let paths = vec!["one.rom".to_string(); 1000];
        let mut calls = 0;
        let error = super::resolve_games_controlled(
            &document,
            &paths,
            &vec![true; paths.len()],
            &mut |_| {
                calls += 1;
                if calls == 12 {
                    Err(super::Error::new(
                        super::ErrorKind::Cancelled,
                        "test cancellation",
                    ))
                } else {
                    Ok(())
                }
            },
        )
        .unwrap_err();
        assert_eq!(error.kind, super::ErrorKind::Cancelled);
        assert_eq!(
            calls, 12,
            "cancellation must stop before classifying duplicate targets as failures"
        );
    }

    #[test]
    fn bounded_planning_comparison_preserves_every_result() {
        let paths: Vec<_> = (0..600).map(|at| format!("folder/Game{at}.mra")).collect();
        let xml = format!(
            "<gameList>{}</gameList>",
            paths
                .iter()
                .map(|path| format!("<game><path>{path}</path></game>"))
                .collect::<String>()
        );
        let document =
            super::parse_document(xml.as_bytes(), std::path::Path::new("timing.xml")).unwrap();
        let start = std::time::Instant::now();
        let old: Vec<_> = paths
            .iter()
            .map(|path| super::matching_game_index(&document, path, true).unwrap())
            .collect();
        let linear = start.elapsed();
        let start = std::time::Instant::now();
        let index = super::MatchIndex::new(&document, &mut |_| Ok(())).unwrap();
        let new: Vec<_> = paths
            .iter()
            .map(|path| index.find(path, true).unwrap())
            .collect();
        let indexed = start.elapsed();
        assert_eq!(old, new);
        eprintln!(
            "600 exact targets: linear={linear:?}, indexed including construction={indexed:?}"
        );
    }
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "degauss-scraper-gamelist-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn metadata(name: &str) -> Metadata {
        Metadata {
            name: Some(name.into()),
            desc: Some("New & useful <description>".into()),
            developer: Some("New developer".into()),
            ..Default::default()
        }
    }

    #[test]
    fn a_missing_gamelist_is_created_and_passes_the_real_reader() {
        let folder = temp("create");
        let path = folder.join("gamelist.xml");
        let change = apply(
            Apply {
                gamelist_path: &path,
                folder: &folder,
                relative_game_path: "./Game.rom",
                metadata: &metadata("Game"),
                image_path: Some("./media/screenscraper/Game.png"),
                image_policy: ImagePolicy::MissingOnly,
                metadata_policy: MetadataPolicy::FillMissing,
            },
            &mut Backups::new(),
        )
        .unwrap();
        assert!(change.created_entry);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("<path>./Game.rom</path>"));
        assert!(text.contains("New &amp; useful &lt;description&gt;"));
        crate::gamelist::Gamelist::parse(&text, &folder, &path).unwrap();
        let _ = std::fs::remove_dir_all(folder);
    }

    #[test]
    fn a_self_closing_empty_gamelist_accepts_its_first_game_losslessly() {
        let folder = temp("self-closing-root");
        let path = folder.join("gamelist.xml");
        std::fs::write(
            &path,
            "<?xml version=\"1.0\"?>\r\n<GameList source=\"existing\" />\r\n",
        )
        .unwrap();
        let change = apply(
            Apply {
                gamelist_path: &path,
                folder: &folder,
                relative_game_path: "./Game.rom",
                metadata: &metadata("Game"),
                image_path: None,
                image_policy: ImagePolicy::Off,
                metadata_policy: MetadataPolicy::FillMissing,
            },
            &mut Backups::new(),
        )
        .unwrap();

        assert!(change.created_entry);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("<?xml version=\"1.0\"?>\r\n<GameList source=\"existing\" >\r\n"));
        assert!(text.contains("<path>./Game.rom</path>"));
        assert!(text.ends_with("</GameList>\r\n"));
        crate::gamelist::Gamelist::parse(&text, &folder, &path).unwrap();
        let _ = std::fs::remove_dir_all(folder);
    }

    #[test]
    fn fill_missing_preserves_existing_and_unknown_xml_byte_for_byte() {
        let folder = temp("preserve");
        let path = folder.join("gamelist.xml");
        let original = "<?xml version=\"1.0\"?>\r\n<gameList custom=\"yes\">\r\n  <!--keep this-->\r\n  <game id=\"7\"><path>./Game.rom</path><name><![CDATA[Local Name]]></name><rating>0.9</rating><desc/></game>\r\n</gameList>\r\n";
        std::fs::write(&path, original).unwrap();
        apply(
            Apply {
                gamelist_path: &path,
                folder: &folder,
                relative_game_path: "Game.rom",
                metadata: &metadata("Remote Name"),
                image_path: None,
                image_policy: ImagePolicy::Off,
                metadata_policy: MetadataPolicy::FillMissing,
            },
            &mut Backups::new(),
        )
        .unwrap();
        let result = std::fs::read_to_string(&path).unwrap();
        assert!(result.contains("<name><![CDATA[Local Name]]></name>"));
        assert!(result.contains("<!--keep this-->"));
        assert!(result.contains("<rating>0.9</rating>"));
        assert!(result.contains("<desc>New &amp; useful &lt;description&gt;</desc>"));
        let _ = std::fs::remove_dir_all(folder);
    }

    #[test]
    fn replace_does_not_erase_fields_absent_upstream() {
        let folder = temp("no-erase");
        let path = folder.join("gamelist.xml");
        std::fs::write(
            &path,
            "<gameList><game><path>./Game.rom</path><publisher>Local</publisher></game></gameList>",
        )
        .unwrap();
        apply(
            Apply {
                gamelist_path: &path,
                folder: &folder,
                relative_game_path: "./Game.rom",
                metadata: &Metadata {
                    name: Some("Remote".into()),
                    ..Default::default()
                },
                image_path: None,
                image_policy: ImagePolicy::Off,
                metadata_policy: MetadataPolicy::ReplaceExisting,
            },
            &mut Backups::new(),
        )
        .unwrap();
        let result = std::fs::read_to_string(&path).unwrap();
        assert!(result.contains("<publisher>Local</publisher>"));
        assert!(result.contains("<name>Remote</name>"));
        let _ = std::fs::remove_dir_all(folder);
    }

    #[test]
    fn missing_only_repairs_a_broken_image_but_keeps_a_real_one() {
        let folder = temp("images");
        let path = folder.join("gamelist.xml");
        std::fs::write(
            &path,
            "<gameList><game><path>./Game.rom</path><image>./old.png</image></game></gameList>",
        )
        .unwrap();
        assert!(needs_image(&path, &folder, "Game.rom", ImagePolicy::MissingOnly).unwrap());
        std::fs::write(folder.join("old.png"), b"image").unwrap();
        assert!(!needs_image(&path, &folder, "Game.rom", ImagePolicy::MissingOnly).unwrap());
        let _ = std::fs::remove_dir_all(folder);
    }

    #[test]
    fn missing_only_resolves_legacy_art_paths_exactly_like_the_reader() {
        let root = temp("legacy-image-paths");
        let folder = root.join("games");
        std::fs::create_dir_all(folder.join("media")).unwrap();
        std::fs::write(folder.join("media/leading.png"), b"image").unwrap();
        std::fs::write(root.join("shared.png"), b"image").unwrap();
        let path = folder.join("gamelist.xml");

        for image in ["/media/leading.png", "../shared.png"] {
            std::fs::write(
                &path,
                format!(
                    "<gameList><game><path>./Game.rom</path><image>{image}</image></game></gameList>"
                ),
            )
            .unwrap();
            assert!(
                !needs_image(&path, &folder, "Game.rom", ImagePolicy::MissingOnly).unwrap(),
                "existing legacy path {image:?} was treated as missing"
            );
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn missing_only_respects_screenshot_and_thumbnail_precedence() {
        for field in ["screenshot", "thumbnail"] {
            let folder = temp(field);
            let path = folder.join("gamelist.xml");
            std::fs::write(
                &path,
                format!(
                    "<gameList><game><path>./Game.rom</path><{field}>./art.png</{field}></game></gameList>"
                ),
            )
            .unwrap();
            std::fs::write(folder.join("art.png"), b"image").unwrap();
            assert!(!needs_image(&path, &folder, "Game.rom", ImagePolicy::MissingOnly).unwrap());
            let _ = std::fs::remove_dir_all(folder);
        }

        let folder = temp("broken-primary");
        let path = folder.join("gamelist.xml");
        std::fs::write(
            &path,
            "<gameList><game><path>./Game.rom</path><image>./missing.png</image><screenshot>./real.png</screenshot></game></gameList>",
        )
        .unwrap();
        std::fs::write(folder.join("real.png"), b"image").unwrap();
        assert!(needs_image(&path, &folder, "Game.rom", ImagePolicy::MissingOnly).unwrap());
        let _ = std::fs::remove_dir_all(folder);
    }

    #[test]
    fn replace_image_changes_only_image_and_keeps_the_previous_file() {
        let folder = temp("replace-image");
        let path = folder.join("gamelist.xml");
        let original = "<gameList><game><path>./Game.rom</path><name>Local name</name><image>./old.png</image><video>./clip.mp4</video></game></gameList>";
        std::fs::write(&path, original).unwrap();
        std::fs::write(folder.join("old.png"), b"old image").unwrap();
        std::fs::write(folder.join("new.png"), b"new image").unwrap();

        let change = apply(
            Apply {
                gamelist_path: &path,
                folder: &folder,
                relative_game_path: "./Game.rom",
                metadata: &Metadata {
                    name: Some("Remote name".into()),
                    ..Default::default()
                },
                image_path: Some("./new.png"),
                image_policy: ImagePolicy::ReplaceExisting,
                metadata_policy: MetadataPolicy::Off,
            },
            &mut Backups::new(),
        )
        .unwrap();

        assert!(change.image);
        assert_eq!(change.metadata_fields, 0);
        let result = std::fs::read_to_string(&path).unwrap();
        assert!(result.contains("<name>Local name</name>"));
        assert!(result.contains("<image>./new.png</image>"));
        assert!(result.contains("<video>./clip.mp4</video>"));
        assert_eq!(std::fs::read(folder.join("old.png")).unwrap(), b"old image");
        let _ = std::fs::remove_dir_all(folder);
    }

    #[test]
    fn inherited_parent_data_counts_as_existing_without_changing_the_parent() {
        let folder = temp("parent");
        let path = folder.join("gamelist.xml");
        std::fs::write(
            &path,
            "<gameList><game id=\"7\"><desc>Parent description</desc><image>./parent.png</image></game><game parentid=\"7\"><path>./Game.rom</path></game></gameList>",
        )
        .unwrap();
        std::fs::write(folder.join("parent.png"), b"image").unwrap();
        assert!(!needs_image(&path, &folder, "Game.rom", ImagePolicy::MissingOnly).unwrap());

        let change = apply(
            Apply {
                gamelist_path: &path,
                folder: &folder,
                relative_game_path: "./Game.rom",
                metadata: &Metadata {
                    desc: Some("Remote description".into()),
                    developer: Some("Remote developer".into()),
                    ..Default::default()
                },
                image_path: None,
                image_policy: ImagePolicy::Off,
                metadata_policy: MetadataPolicy::FillMissing,
            },
            &mut Backups::new(),
        )
        .unwrap();
        assert_eq!(change.metadata_fields, 1);
        let result = std::fs::read_to_string(&path).unwrap();
        assert_eq!(result.matches("<desc>").count(), 1);
        assert!(result.contains("<desc>Parent description</desc>"));
        assert!(result.contains("<developer>Remote developer</developer>"));
        let _ = std::fs::remove_dir_all(folder);
    }

    #[test]
    fn child_artwork_overrides_parent_artwork_before_type_precedence() {
        let folder = temp("child-art-precedence");
        let path = folder.join("gamelist.xml");
        std::fs::write(
            &path,
            "<gameList><game id=\"7\"><image>./missing-parent.png</image></game><game parentid=\"7\"><path>./Game.rom</path><screenshot>./child.png</screenshot></game></gameList>",
        )
        .unwrap();
        std::fs::write(folder.join("child.png"), b"image").unwrap();

        assert!(
            !needs_image(&path, &folder, "Game.rom", ImagePolicy::MissingOnly).unwrap(),
            "the existing reader displays the child's screenshot"
        );

        std::fs::remove_file(folder.join("child.png")).unwrap();
        std::fs::write(folder.join("missing-parent.png"), b"image").unwrap();
        assert!(
            needs_image(&path, &folder, "Game.rom", ImagePolicy::MissingOnly).unwrap(),
            "a broken child image must not fall back to the parent's file"
        );
        let _ = std::fs::remove_dir_all(folder);
    }

    #[test]
    fn a_slug_keyed_existing_entry_is_updated_without_appending_a_duplicate() {
        let folder = temp("slug");
        let path = folder.join("gamelist.xml");
        std::fs::write(
            &path,
            "<gameList><game><path>Super Game.slug</path><desc/></game></gameList>",
        )
        .unwrap();
        let change = apply(
            Apply {
                gamelist_path: &path,
                folder: &folder,
                relative_game_path: "./Super Game (USA) [!].rom",
                metadata: &metadata("Super Game"),
                image_path: None,
                image_policy: ImagePolicy::Off,
                metadata_policy: MetadataPolicy::FillMissing,
            },
            &mut Backups::new(),
        )
        .unwrap();
        assert!(!change.created_entry);
        let result = std::fs::read_to_string(&path).unwrap();
        assert_eq!(result.matches("<game>").count(), 1);
        assert!(result.contains("<path>Super Game.slug</path>"));
        assert!(!result.contains("Super Game (USA)"));
        let _ = std::fs::remove_dir_all(folder);
    }

    #[test]
    fn replace_existing_does_not_rewrite_identical_values() {
        let folder = temp("identical");
        let path = folder.join("gamelist.xml");
        let original =
            "<gameList><game><path>./Game.rom</path><name>Remote</name></game></gameList>";
        std::fs::write(&path, original).unwrap();
        let change = apply(
            Apply {
                gamelist_path: &path,
                folder: &folder,
                relative_game_path: "./Game.rom",
                metadata: &Metadata {
                    name: Some("Remote".into()),
                    ..Default::default()
                },
                image_path: None,
                image_policy: ImagePolicy::Off,
                metadata_policy: MetadataPolicy::ReplaceExisting,
            },
            &mut Backups::new(),
        )
        .unwrap();
        assert!(!change.changed());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        assert_eq!(
            std::fs::read_dir(&folder)
                .unwrap()
                .flatten()
                .filter(|entry| entry.file_name().to_string_lossy().ends_with(".bak"))
                .count(),
            0
        );
        let _ = std::fs::remove_dir_all(folder);
    }

    #[test]
    fn malformed_or_duplicate_gamelists_are_never_replaced() {
        for (name, original) in [
            ("malformed", "<gameList><game>"),
            (
                "duplicate",
                "<gameList><game><path>./x</path></game><game><path>x</path></game></gameList>",
            ),
        ] {
            let folder = temp(name);
            let path = folder.join("gamelist.xml");
            std::fs::write(&path, original).unwrap();
            let result = apply(
                Apply {
                    gamelist_path: &path,
                    folder: &folder,
                    relative_game_path: "./x",
                    metadata: &metadata("X"),
                    image_path: None,
                    image_policy: ImagePolicy::Off,
                    metadata_policy: MetadataPolicy::FillMissing,
                },
                &mut Backups::new(),
            );
            assert!(result.is_err());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
            let _ = std::fs::remove_dir_all(folder);
        }
    }

    #[test]
    fn an_existing_file_gets_one_exact_backup_per_run() {
        let folder = temp("backup");
        let path = folder.join("gamelist.xml");
        let original = "<gameList><game><path>./x</path></game></gameList>";
        std::fs::write(&path, original).unwrap();
        let mut backups = Backups::new();
        for name in ["One", "Two"] {
            apply(
                Apply {
                    gamelist_path: &path,
                    folder: &folder,
                    relative_game_path: "./x",
                    metadata: &metadata(name),
                    image_path: None,
                    image_policy: ImagePolicy::Off,
                    metadata_policy: MetadataPolicy::ReplaceExisting,
                },
                &mut backups,
            )
            .unwrap();
        }
        let backup: Vec<_> = std::fs::read_dir(&folder)
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".bak"))
            .collect();
        assert_eq!(backup.len(), 1);
        assert_eq!(std::fs::read_to_string(backup[0].path()).unwrap(), original);
        let _ = std::fs::remove_dir_all(folder);
    }

    #[test]
    fn complete_existing_content_needs_no_scrape_request() {
        let folder = temp("complete");
        let path = folder.join("gamelist.xml");
        std::fs::write(folder.join("art.png"), b"image").unwrap();
        std::fs::write(
            &path,
            "<gameList><game><path>./Game.rom</path><name>Name</name><desc>Description</desc><publisher>Publisher</publisher><developer>Developer</developer><releasedate>19910000T000000</releasedate><players>1</players><genre>Action</genre><lang>en</lang><image>./art.png</image></game></gameList>",
        )
        .unwrap();
        let needed = needs(
            &path,
            &folder,
            "./Game.rom",
            ImagePolicy::MissingOnly,
            MetadataPolicy::FillMissing,
        )
        .unwrap();
        assert_eq!(needed, Needs::default());
        let _ = std::fs::remove_dir_all(folder);
    }

    #[test]
    fn external_parent_metadata_and_root_relative_art_skip_only_the_matching_zip_member() {
        let folder = temp("complete-inherited-zip");
        let path = folder.join("gamelist.xml");
        std::fs::write(folder.join("art.png"), b"image").unwrap();
        let xml = "<gameList><game id=\"p\"><name>Name</name><desc>Description</desc><publisher>Publisher</publisher><developer>Developer</developer><releasedate>19910000T000000</releasedate><players>1</players><genre>Action</genre><lang>en</lang><screenshot>/art.png</screenshot></game><game parentid=\"p\"><path>collection.zip/one.rom</path></game></gameList>";
        std::fs::write(&path, xml).unwrap();
        let paths = vec!["collection.zip/one.rom".into(), "other.zip/one.rom".into()];
        let results = needs_many_with_fallback(
            &path,
            &folder,
            &paths,
            &[false, false],
            ImagePolicy::MissingOnly,
            MetadataPolicy::FillMissing,
        )
        .unwrap();
        let mut results = results.into_iter();
        assert_eq!(
            results.next().unwrap().unwrap(),
            Needs {
                image: false,
                metadata: false
            }
        );
        assert_eq!(
            results.next().unwrap().unwrap(),
            Needs {
                image: true,
                metadata: true
            },
            "another archive member must not inherit the same basename's completeness"
        );
        std::fs::remove_file(folder.join("art.png")).unwrap();
        assert_eq!(
            needs_many_with_fallback(
                &path,
                &folder,
                &paths[..1],
                &[false],
                ImagePolicy::MissingOnly,
                MetadataPolicy::FillMissing
            )
            .unwrap()
            .remove(0)
            .unwrap(),
            Needs {
                image: true,
                metadata: false
            }
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            xml,
            "planning must not rewrite external metadata"
        );
        let _ = std::fs::remove_dir_all(folder);
    }

    #[test]
    fn every_image_and_metadata_policy_combination_is_independent() {
        let folder = temp("policy-matrix");
        let path = folder.join("gamelist.xml");
        std::fs::write(folder.join("art.png"), b"image").unwrap();
        std::fs::write(
            &path,
            "<gameList><game><path>./Game.rom</path><name>Name</name><desc>Description</desc><publisher>Publisher</publisher><developer>Developer</developer><releasedate>19910000T000000</releasedate><players>1</players><genre>Action</genre><lang>en</lang><image>./art.png</image></game></gameList>",
        )
        .unwrap();

        for image_policy in ImagePolicy::ALL {
            for metadata_policy in MetadataPolicy::ALL {
                let complete =
                    needs(&path, &folder, "./Game.rom", image_policy, metadata_policy).unwrap();
                assert_eq!(
                    complete,
                    Needs {
                        image: image_policy == ImagePolicy::ReplaceExisting,
                        metadata: metadata_policy == MetadataPolicy::ReplaceExisting,
                    },
                    "complete entry with {image_policy:?} and {metadata_policy:?}"
                );

                let absent =
                    needs(&path, &folder, "./Other.rom", image_policy, metadata_policy).unwrap();
                assert_eq!(
                    absent,
                    Needs {
                        image: image_policy != ImagePolicy::Off,
                        metadata: metadata_policy != MetadataPolicy::Off,
                    },
                    "absent entry with {image_policy:?} and {metadata_policy:?}"
                );
            }
        }
        let _ = std::fs::remove_dir_all(folder);
    }

    #[test]
    fn non_utf8_and_oversized_gamelists_are_never_modified() {
        let folder = temp("invalid-input-limits");
        let path = folder.join("gamelist.xml");
        std::fs::write(&path, [0xff, 0xfe]).unwrap();
        assert!(needs(
            &path,
            &folder,
            "./Game.rom",
            ImagePolicy::MissingOnly,
            MetadataPolicy::FillMissing,
        )
        .is_err());
        assert_eq!(std::fs::read(&path).unwrap(), [0xff, 0xfe]);

        let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.set_len(MAX_GAMELIST_BYTES + 1).unwrap();
        assert!(needs(
            &path,
            &folder,
            "./Game.rom",
            ImagePolicy::MissingOnly,
            MetadataPolicy::FillMissing,
        )
        .is_err());
        assert_eq!(
            std::fs::metadata(&path).unwrap().len(),
            MAX_GAMELIST_BYTES + 1
        );
        let _ = std::fs::remove_dir_all(folder);
    }

    #[test]
    fn a_proposed_gamelist_cannot_cross_the_reader_limit() {
        let path = Path::new("gamelist.xml");
        assert!(ensure_proposed_size(MAX_GAMELIST_BYTES as usize, path).is_ok());
        assert!(ensure_proposed_size(MAX_GAMELIST_BYTES as usize + 1, path).is_err());
    }

    #[test]
    fn an_unknown_entity_cannot_be_mistaken_for_missing_metadata() {
        let folder = temp("entity");
        let path = folder.join("gamelist.xml");
        let original =
            "<!DOCTYPE gameList [<!ENTITY custom 'Local'>]><gameList><game><path>./Game.rom</path><name>&custom;</name></game></gameList>";
        std::fs::write(&path, original).unwrap();
        let result = needs(
            &path,
            &folder,
            "./Game.rom",
            ImagePolicy::Off,
            MetadataPolicy::FillMissing,
        );
        assert!(result.is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        let _ = std::fs::remove_dir_all(folder);
    }

    #[test]
    fn a_batch_updates_multiple_games_with_one_original_backup() {
        let folder = temp("batch");
        let path = folder.join("gamelist.xml");
        let original = "<gameList><game><path>./One.rom</path></game><game><path>./Two.rom</path></game></gameList>";
        std::fs::write(&path, original).unwrap();
        let updates = [
            Update {
                relative_game_path: "./One.rom".into(),
                metadata: metadata("One remote"),
                image_path: None,
                image_created: false,
                image_policy: ImagePolicy::Off,
                metadata_policy: MetadataPolicy::FillMissing,
            },
            Update {
                relative_game_path: "./Two.rom".into(),
                metadata: metadata("Two remote"),
                image_path: None,
                image_created: false,
                image_policy: ImagePolicy::Off,
                metadata_policy: MetadataPolicy::FillMissing,
            },
        ];
        let outcomes = apply_many(&path, &folder, &updates, &mut Backups::new()).unwrap();
        assert!(outcomes
            .into_iter()
            .all(|outcome| outcome.unwrap().changed()));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("<name>One remote</name>"));
        assert!(text.contains("<name>Two remote</name>"));
        let backups: Vec<_> = std::fs::read_dir(&folder)
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".bak"))
            .collect();
        assert_eq!(backups.len(), 1);
        assert_eq!(
            std::fs::read_to_string(backups[0].path()).unwrap(),
            original
        );
        let _ = std::fs::remove_dir_all(folder);
    }

    #[test]
    fn a_batch_refuses_two_targets_that_fallback_to_one_entry() {
        let folder = temp("batch-collision");
        let path = folder.join("gamelist.xml");
        let original =
            "<gameList><game><path>Super Game.slug</path><desc>Local</desc></game></gameList>";
        std::fs::write(&path, original).unwrap();
        let update = |relative: &str| Update {
            relative_game_path: relative.into(),
            metadata: metadata("Remote"),
            image_path: None,
            image_created: false,
            image_policy: ImagePolicy::Off,
            metadata_policy: MetadataPolicy::ReplaceExisting,
        };
        let outcomes = apply_many(
            &path,
            &folder,
            &[
                update("./Super Game (USA).rom"),
                update("./Super Game (Europe).rom"),
            ],
            &mut Backups::new(),
        )
        .unwrap();
        assert!(outcomes.iter().all(Result::is_err));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        let _ = std::fs::remove_dir_all(folder);
    }

    #[test]
    fn a_batch_keeps_case_distinct_missing_paths_on_case_sensitive_storage() {
        let folder = temp("batch-case-sensitive");
        let path = folder.join("gamelist.xml");
        let update = |relative: &str, name: &str| Update {
            relative_game_path: relative.into(),
            metadata: metadata(name),
            image_path: None,
            image_created: false,
            image_policy: ImagePolicy::Off,
            metadata_policy: MetadataPolicy::FillMissing,
        };
        let outcomes = apply_many(
            &path,
            &folder,
            &[
                update("./Game.rom", "Uppercase path"),
                update("./game.rom", "Lowercase path"),
            ],
            &mut Backups::new(),
        )
        .unwrap();

        assert!(outcomes
            .into_iter()
            .all(|outcome| outcome.unwrap().created_entry));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("<path>./Game.rom</path>"));
        assert!(text.contains("<path>./game.rom</path>"));
        let _ = std::fs::remove_dir_all(folder);
    }

    #[test]
    fn content_media_references_follow_fat_case_semantics() {
        assert!(same_media_reference(
            "./MEDIA/ScreenScraper/3-42-ABC.PNG",
            "media/screenscraper/3-42-abc.png"
        ));
        assert!(!same_media_reference(
            "./media/screenscraper/3-42-abc.png",
            "./media/screenscraper/3-43-abc.png"
        ));
    }
    #[test]
    fn multigame_zip_scrape_never_edits_an_archive_or_basename_match() {
        let folder = temp("zip-exact");
        let path = folder.join("gamelist.xml");
        let archive_row = "<game><path>library.zip</path><name>Archive</name></game>";
        std::fs::write(&path, format!("<gameList>{archive_row}</gameList>")).unwrap();
        let update = Update {
            relative_game_path: "./library.zip/folder/Game.rom".into(),
            metadata: metadata("Member"),
            image_path: None,
            image_created: false,
            image_policy: ImagePolicy::Off,
            metadata_policy: MetadataPolicy::FillMissing,
        };
        apply_many_with_fallback(&path, &folder, &[update], &[false], &mut Backups::default())
            .unwrap()[0]
            .as_ref()
            .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains(archive_row));
        assert!(text.contains("<path>./library.zip/folder/Game.rom</path>"));
        assert!(text.contains("<name>Member</name>"));
        std::fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn single_member_fill_preserves_legacy_fields_and_materializes_exact_entry() {
        let folder = temp("zip-legacy-fill");
        let path = folder.join("gamelist.xml");
        let original = r#"<game ID="old" customFlag="Keep &amp; exact"><path>Only.zip</path><name>Keep name</name><custom>Keep custom</custom></game>"#;
        std::fs::write(&path, format!("<gameList>{original}</gameList>")).unwrap();
        let update = Update {
            relative_game_path: "./Only.zip/Game.rom".into(),
            metadata: metadata("Replacement name"),
            image_path: None,
            image_created: false,
            image_policy: ImagePolicy::Off,
            metadata_policy: MetadataPolicy::FillMissing,
        };
        apply_many_with_fallback(&path, &folder, &[update], &[true], &mut Backups::default())
            .unwrap()[0]
            .as_ref()
            .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains(original));
        assert_eq!(text.matches("ID=\"old\"").count(), 1);
        assert_eq!(text.matches("customFlag=\"Keep &amp; exact\"").count(), 2);
        assert!(
            !text.contains("customflag="),
            "XML attribute names are case-sensitive"
        );
        let list = crate::gamelist::Gamelist::load(&path, &folder).unwrap();
        let (member, _) = list.lookup_exact("./Only.zip/Game.rom").unwrap();
        assert_eq!(member.name.as_deref(), Some("Keep name"));
        assert_eq!(member.developer.as_deref(), Some("New developer"));
        assert_eq!(text.matches("<custom>Keep custom</custom>").count(), 2);
        std::fs::remove_dir_all(folder).unwrap();
    }
}
