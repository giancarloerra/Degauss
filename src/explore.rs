//! Cross-system discovery from selected-source indexes, never a ROM scan.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};

use serde::{Deserialize, Serialize};

use crate::browse::{Kind, Launch, Row};
use crate::error::{DegaussError, Result};
use crate::game_filter::{Criterion, Field};

pub const NAME: &str = "Explore Games";
pub const STATE_ID: &str = "__degauss_explore__";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Resume {
    pub query: Query,
    pub origin: Option<Box<crate::state::State>>,
    #[serde(default)]
    pub pivots: Vec<(Query, usize)>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Query {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub system: Option<String>,
    #[serde(default)]
    pub decade: Option<Criterion>,
    #[serde(
        default,
        serialize_with = "save_fields",
        deserialize_with = "read_fields"
    )]
    pub fields: [Option<Criterion>; 6],
}

// TOML has no null array elements. Persist only the chosen named criteria.
fn save_fields<S: serde::Serializer>(
    fields: &[Option<Criterion>; 6],
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    let values: BTreeMap<_, _> = Field::ALL
        .into_iter()
        .filter_map(|field| {
            fields[field.index()]
                .as_ref()
                .map(|criterion| (field.label(), criterion))
        })
        .collect();
    values.serialize(serializer)
}

fn read_fields<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<[Option<Criterion>; 6], D::Error> {
    let values = BTreeMap::<String, Criterion>::deserialize(deserializer)?;
    let mut fields = std::array::from_fn(|_| None);
    for (name, criterion) in values {
        let field = Field::ALL
            .into_iter()
            .find(|field| field.label() == name)
            .ok_or_else(|| serde::de::Error::custom(format!("Unknown Explore field: {name}")))?;
        fields[field.index()] = Some(criterion);
    }
    Ok(fields)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Facet {
    Category,
    System,
    Decade,
    Metadata(Field),
}

impl Facet {
    pub const ALL: [Self; 9] = [
        Self::Category,
        Self::System,
        Self::Decade,
        Self::Metadata(Field::Genre),
        Self::Metadata(Field::Year),
        Self::Metadata(Field::Players),
        Self::Metadata(Field::Language),
        Self::Metadata(Field::Developer),
        Self::Metadata(Field::Publisher),
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Category => "Category",
            Self::System => "System",
            Self::Decade => "Decade",
            Self::Metadata(field) => field.label(),
        }
    }

    fn value(self, entry: &Entry) -> Option<&str> {
        match self {
            Self::Category => Some(&entry.category),
            Self::System => Some(&entry.system),
            Self::Decade => entry.decade.as_deref(),
            Self::Metadata(field) => field.value(&entry.row),
        }
    }
}

fn normalise(text: &str) -> String {
    text.trim().to_lowercase()
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub system: String,
    pub system_name: String,
    pub category: String,
    pub row: Row,
    original_name: String,
    search_title: String,
    decade: Option<String>,
}

impl Entry {
    pub fn key(&self) -> String {
        match &self.row.kind {
            Kind::Play(Launch::File(path)) => format!("{}\0f:{}", self.system, path.display()),
            Kind::Play(Launch::AmigaVision { install, title }) => {
                format!("{}\0a:{}|{}", self.system, install.display(), title)
            }
            Kind::Enter(place) => format!("{}\0{}", self.system, place.key()),
        }
    }
}

#[derive(Default)]
pub struct Catalogue {
    pub entries: Vec<Entry>,
    /// Indexed source, folder and missing-game notices shown in the coverage report.
    pub omitted: Vec<String>,
    pub providers: BTreeMap<String, crate::artwork_pack::Provider>,
    pub signatures: BTreeMap<String, String>,
    names: crate::name_display::GameNameDisplay,
    mra_filenames: bool,
    notices: BTreeMap<String, Vec<String>>,
}

impl Catalogue {
    pub fn present_names(
        &mut self,
        names: crate::name_display::GameNameDisplay,
        mra_filenames: bool,
    ) {
        self.names = names;
        self.mra_filenames = mra_filenames;
        let mut groups: BTreeMap<(String, String), Vec<usize>> = BTreeMap::new();
        for (index, entry) in self.entries.iter_mut().enumerate() {
            entry.row.name.clone_from(&entry.original_name);
            let shown = crate::name_display::row_name(&entry.row, names, mra_filenames);
            if shown.as_ref() != entry.row.name {
                entry.row.name = shown.into_owned();
            }
            entry.search_title = entry.row.name.to_lowercase().replace(' ', "");
            groups
                .entry((entry.system.clone(), normalise(&entry.row.name)))
                .or_default()
                .push(index);
        }
        for group in groups.values().filter(|group| group.len() > 1) {
            let filenames: Vec<String> = group
                .iter()
                .map(|index| match &self.entries[*index].row.kind {
                    Kind::Play(Launch::File(path)) => path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
                    Kind::Play(Launch::AmigaVision { title, .. }) => title.clone(),
                    Kind::Enter(place) => place.path().display().to_string(),
                })
                .collect();
            for (position, index) in group.iter().enumerate() {
                let entry = &mut self.entries[*index];
                let filename = &filenames[position];
                let label = if filenames.iter().filter(|name| *name == filename).count() > 1 {
                    match &entry.row.kind {
                        Kind::Play(Launch::File(path)) => path.display().to_string(),
                        Kind::Play(Launch::AmigaVision { install, .. }) => {
                            install.display().to_string()
                        }
                        _ => filename.clone(),
                    }
                } else {
                    filename.clone()
                };
                entry.row.name.push_str(" · ");
                entry.row.name.push_str(&label);
            }
        }
        self.entries.sort_by_cached_key(|entry| {
            (
                normalise(&entry.row.name),
                entry.system.clone(),
                entry.key(),
            )
        });
    }
}

#[derive(Debug, Clone)]
pub struct Choice {
    pub criterion: Option<Criterion>,
    pub count: usize,
}

impl Choice {
    pub fn label(&self) -> &str {
        self.criterion.as_ref().map_or("Any", Criterion::label)
    }
}

impl Query {
    pub fn criterion(&self, facet: Facet) -> Option<Criterion> {
        match facet {
            Facet::Category => self.category.as_ref().map(|label| Criterion::Known {
                label: label.clone(),
                key: normalise(label),
            }),
            Facet::System => self.system.as_ref().map(|label| Criterion::Known {
                label: label.clone(),
                key: normalise(label),
            }),
            Facet::Decade => self.decade.clone(),
            Facet::Metadata(field) => self.fields[field.index()].clone(),
        }
    }

    pub fn choose(&mut self, facet: Facet, criterion: Option<Criterion>) {
        match facet {
            Facet::Category => self.category = criterion.map(|value| value.label().to_string()),
            Facet::System => self.system = criterion.map(|value| value.label().to_string()),
            Facet::Decade => self.decade = criterion,
            Facet::Metadata(field) => self.fields[field.index()] = criterion,
        }
    }

    pub fn label(&self, facet: Facet) -> String {
        self.criterion(facet)
            .map_or_else(|| "Any".into(), |value| value.label().into())
    }

    fn matches_except(&self, entry: &Entry, except: Option<Facet>, title: &str) -> bool {
        if !title
            .split_whitespace()
            .all(|word| entry.search_title.contains(word))
        {
            return false;
        }
        if except != Some(Facet::Category)
            && self
                .category
                .as_ref()
                .is_some_and(|value| value != &entry.category)
        {
            return false;
        }
        if except != Some(Facet::System)
            && self
                .system
                .as_ref()
                .is_some_and(|value| value != &entry.system)
        {
            return false;
        }
        Facet::ALL.into_iter().all(|facet| {
            if Some(facet) == except || matches!(facet, Facet::Category | Facet::System) {
                return true;
            }
            let selected = match facet {
                Facet::Decade => self.decade.as_ref(),
                Facet::Metadata(field) => self.fields[field.index()].as_ref(),
                _ => None,
            };
            match selected {
                None => true,
                Some(Criterion::Unknown) => facet.value(entry).is_none(),
                Some(Criterion::Known { key, .. }) => facet
                    .value(entry)
                    .is_some_and(|value| normalise(value) == *key),
            }
        })
    }

    /// References into the immutable projection: editing criteria never clones games.
    pub fn matching(&self, catalogue: &Catalogue) -> Vec<usize> {
        let title = self.title.to_lowercase();
        catalogue
            .entries
            .iter()
            .enumerate()
            .filter_map(|(at, entry)| self.matches_except(entry, None, &title).then_some(at))
            .collect()
    }

    /// Counts ignore this facet, while retaining every other constraint.
    pub fn choices(&self, catalogue: &Catalogue, facet: Facet) -> Vec<Choice> {
        let mut values: BTreeMap<String, (String, usize)> = BTreeMap::new();
        let mut unknown = 0;
        let mut total = 0;
        let title = self.title.to_lowercase();
        for entry in &catalogue.entries {
            if !self.matches_except(entry, Some(facet), &title) {
                continue;
            }
            total += 1;
            match facet.value(entry) {
                Some(value) => {
                    values
                        .entry(normalise(value))
                        .or_insert((value.to_string(), 0))
                        .1 += 1
                }
                None => unknown += 1,
            }
        }
        let selected = self.criterion(facet);
        if let Some(Criterion::Known { label, key }) = &selected {
            values.entry(key.clone()).or_insert((label.clone(), 0));
        }
        let mut choices = vec![Choice {
            criterion: None,
            count: total,
        }];
        choices.extend(values.into_iter().map(|(key, (label, count))| Choice {
            criterion: Some(Criterion::Known { key, label }),
            count,
        }));
        if !matches!(facet, Facet::Category | Facet::System) {
            choices.push(Choice {
                criterion: Some(Criterion::Unknown),
                count: unknown,
            });
        }
        choices
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Collection {
    pub name: String,
    pub query: Query,
}

pub fn collection_name(
    name: &str,
    collections: &BTreeMap<String, Collection>,
    excluding: Option<&str>,
) -> Result<String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 80 || name.chars().any(char::is_control) {
        return Err(DegaussError::unsupported(
            "collection name",
            "use 1 to 80 printable characters",
        ));
    }
    if collections.iter().any(|(id, value)| {
        Some(id.as_str()) != excluding && normalise(&value.name) == normalise(name)
    }) {
        return Err(DegaussError::unsupported(
            "collection name",
            "that name is already used",
        ));
    }
    Ok(name.to_string())
}

pub fn next_collection_id(collections: &BTreeMap<String, Collection>, next: u64) -> (String, u64) {
    (next.max(1)..)
        .map(|id| format!("collection-{id}"))
        .find(|id| !collections.contains_key(id))
        .map(|id| {
            let next = id.trim_start_matches("collection-").parse::<u64>().unwrap() + 1;
            (id, next)
        })
        .unwrap()
}

#[derive(Clone)]
pub struct Source {
    pub id: String,
    pub name: String,
    pub category: String,
    pub config: crate::config::SystemConfig,
    pub pack: Option<PathBuf>,
    pub error: Option<String>,
}

pub struct Request {
    pub cache_dir: PathBuf,
    pub sources: Vec<Source>,
    pub hidden: HashSet<String>,
    pub language: Option<String>,
    pub names: crate::name_display::GameNameDisplay,
    pub mra_filenames: bool,
    pub previous: Option<Catalogue>,
}

impl Request {
    /// Cache revisions and selected-source/visibility changes invalidate only
    /// affected systems. This does not enumerate or read game directories.
    fn source_signatures(&self, cancelled: &AtomicBool) -> BTreeMap<String, String> {
        self.sources
            .iter()
            .take_while(|_| !cancelled.load(Ordering::Relaxed))
            .map(|source| {
                let mut paths = vec![crate::cache::system_path(&self.cache_dir, &source.id)];
                if source.pack.is_some() {
                    paths = vec![
                        crate::cache::artwork_pack_system_path(&self.cache_dir, &source.id),
                        crate::cache::artwork_pack_source_path(&self.cache_dir, &source.id),
                        crate::cache::artwork_pack_prepared_path(&self.cache_dir, &source.id),
                    ];
                }
                let stamps: Vec<_> = paths
                    .iter()
                    .map(|path| {
                        std::fs::metadata(path)
                            .map(|data| (data.len(), data.modified().ok()))
                            .map_err(|error| error.kind())
                    })
                    .collect();
                let roots: Vec<_> = std::iter::once(&source.config.path)
                    .chain(source.config.extra_paths.iter())
                    .collect();
                let mut hidden: Vec<_> = self
                    .hidden
                    .iter()
                    .filter(|key| {
                        let Some((_, value)) = key.split_once(':') else {
                            return false;
                        };
                        let path = std::path::Path::new(value.split('|').next().unwrap_or(value));
                        roots.iter().any(|root| path.starts_with(root))
                    })
                    .collect();
                hidden.sort();
                let available = roots.iter().any(|root| std::path::Path::new(root).is_dir());
                (
                    source.id.clone(),
                    format!(
                        "{:?}",
                        (
                            &source.name,
                            &source.category,
                            &source.pack,
                            &source.error,
                            &self.language,
                            roots,
                            hidden,
                            stamps,
                            available
                        )
                    ),
                )
            })
            .collect()
    }
}

pub enum Event {
    Progress(String),
    Ready(Catalogue),
    Cancelled,
    Failed(String),
}

pub struct Job {
    receiver: mpsc::Receiver<Event>,
    cancelled: Arc<AtomicBool>,
}

impl Job {
    pub fn start(request: Request) -> Result<Self> {
        let (sender, receiver) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&cancelled);
        std::thread::Builder::new()
            .name("degauss-explore".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    load(request, &flag, |name| {
                        let _ = sender.send(Event::Progress(name));
                    })
                }));
                let event = match result {
                    Ok(Ok(Some(catalogue))) => Event::Ready(catalogue),
                    Ok(Ok(None)) => Event::Cancelled,
                    Ok(Err(error)) => Event::Failed(error.to_string()),
                    Err(_) => Event::Failed("Explore Games worker panicked".into()),
                };
                let _ = sender.send(event);
            })
            .map_err(|error| DegaussError::unsupported("Explore Games", error.to_string()))?;
        Ok(Self {
            receiver,
            cancelled,
        })
    }
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
    pub fn try_recv(&self) -> Option<Event> {
        match self.receiver.try_recv() {
            Ok(Event::Ready(_)) if self.cancelled.load(Ordering::Relaxed) => Some(Event::Cancelled),
            Ok(event) => Some(event),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => Some(Event::Failed(
                "Explore Games worker disconnected without a result".into(),
            )),
        }
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn hidden_key(row: &Row) -> String {
    match &row.kind {
        Kind::Enter(place) => place.key(),
        Kind::Play(Launch::File(path)) => format!("f:{}", path.display()),
        Kind::Play(Launch::AmigaVision { install, title }) => {
            format!("a:{}|{title}", install.display())
        }
    }
}

fn read_source(
    request: &Request,
    source: &Source,
) -> Result<(
    crate::cache::SystemCache,
    Option<crate::artwork_pack::Provider>,
)> {
    if let Some(error) = &source.error {
        return Err(DegaussError::unsupported("selected data source", error));
    }
    if !std::iter::once(&source.config.path)
        .chain(source.config.extra_paths.iter())
        .any(|root| std::path::Path::new(root).is_dir())
    {
        return Err(DegaussError::unsupported(
            "indexed storage",
            "game storage is unavailable",
        ));
    }
    let Some(root) = &source.pack else {
        return crate::cache::load_system_checked(&request.cache_dir, &source.id)?
            .map(|cache| (cache, None))
            .ok_or_else(|| {
                DegaussError::unsupported(
                    "Gamelist index",
                    "not indexed; rebuild this system list to include it",
                )
            });
    };
    let state = crate::cache::load_pack_source_state(&request.cache_dir, &source.id)?
        .ok_or_else(|| DegaussError::unsupported("Artwork Pack", "not prepared"))?;
    let accepted = state
        .accepted
        .as_ref()
        .filter(|accepted| std::path::Path::new(&accepted.docs_root) == root)
        .ok_or_else(|| {
            DegaussError::unsupported("Artwork Pack", "selected source is not prepared")
        })?;
    if !accepted.health.usable() {
        return Err(DegaussError::unsupported(
            "Artwork Pack",
            "selected source is unusable",
        ));
    }
    let prepared = crate::cache::load_pack_prepared_map(&request.cache_dir, &source.id)?
        .ok_or_else(|| DegaussError::unsupported("Artwork Pack", "prepared metadata is missing"))?;
    let data = crate::cache::load_artwork_pack_data_checked(&request.cache_dir, &source.id)?
        .ok_or_else(|| DegaussError::unsupported("Artwork Pack index", "not indexed"))?;
    let language = state
        .declined
        .as_ref()
        .filter(|declined| declined.language == request.language && declined.signature.is_some())
        .map_or(accepted.language.as_deref(), |_| {
            request.language.as_deref()
        });
    let provider = crate::artwork_pack::Provider::from_prepared_state(
        &source.id,
        root,
        language,
        accepted.health,
        accepted.diagnostics.clone(),
        accepted.signature.clone(),
        prepared,
    );
    Ok((data.cache, Some(provider)))
}

/// Resolve aliases first, then read only files with repeated indexed titles or
/// filenames. Different content stays distinct, including MGL file/core choices.
fn deduplicate(
    mut entries: Vec<Entry>,
    cancelled: &AtomicBool,
    notices: &mut Vec<String>,
) -> Result<Option<Vec<Entry>>> {
    entries.sort_by_cached_key(|entry| match &entry.row.kind {
        Kind::Play(Launch::File(path)) => (path.components().count(), entry.key()),
        _ => (0, entry.key()),
    });
    let mut references = HashSet::new();
    let mut unique = Vec::with_capacity(entries.len());
    for entry in entries {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let identity = match &entry.row.kind {
            Kind::Play(Launch::File(path)) => {
                let (file, member) = crate::zip::split_member_path(path)
                    .unwrap_or_else(|| (path.clone(), String::new()));
                let real = match std::fs::canonicalize(&file) {
                    Ok(real) => real,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        notices.push(format!(
                            "{}: missing indexed game {}: {error}",
                            entry.system_name,
                            path.display()
                        ));
                        continue;
                    }
                    Err(error) => {
                        return Err(DegaussError::io("resolving indexed game", &file, error))
                    }
                };
                format!("{}\0{}\0{}", entry.system, real.display(), member)
            }
            _ => entry.key(),
        };
        if references.insert(identity) {
            unique.push(entry);
        }
    }
    let mut filenames = HashMap::new();
    let mut titles = HashMap::new();
    for entry in &unique {
        if let Kind::Play(Launch::File(path)) = &entry.row.kind {
            *filenames
                .entry((
                    entry.system.clone(),
                    path.file_name().unwrap_or_default().to_os_string(),
                ))
                .or_insert(0usize) += 1;
            *titles
                .entry((entry.system.clone(), normalise(&entry.original_name)))
                .or_insert(0usize) += 1;
        }
    }
    let mut archives = crate::zip::ArchiveCache::default();
    let mut candidates = Vec::with_capacity(unique.len());
    let mut sizes = HashMap::new();
    for entry in unique {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let Kind::Play(Launch::File(path)) = &entry.row.kind else {
            candidates.push((entry, None));
            continue;
        };
        let extension = path
            .extension()
            .unwrap_or_default()
            .to_string_lossy()
            .to_ascii_lowercase();
        let repeated = filenames[&(
            entry.system.clone(),
            path.file_name().unwrap_or_default().to_os_string(),
        )] > 1
            || titles[&(entry.system.clone(), normalise(&entry.original_name))] > 1;
        if !repeated {
            candidates.push((entry, None));
            continue;
        }
        let (size, digest) = if let Some((archive, member)) = crate::zip::split_member_path(path) {
            let listing = match archives.read_controlled(&archive, cancelled) {
                Ok(listing) => listing,
                Err(DegaussError::Io { source, .. })
                    if source.kind() == std::io::ErrorKind::NotFound =>
                {
                    notices.push(format!(
                        "{}: missing indexed game {}: {source}",
                        entry.system_name,
                        path.display()
                    ));
                    continue;
                }
                Err(error) => return Err(error),
            };
            let Some(listing) = listing else {
                return Ok(None);
            };
            let Some(file) = listing.entries.iter().find(|file| file.name == member) else {
                notices.push(format!(
                    "{}: missing indexed game: {} no longer contains {member}",
                    entry.system_name,
                    archive.display()
                ));
                continue;
            };
            (file.size, Some(file.crc32))
        } else {
            let metadata = match std::fs::metadata(path) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    notices.push(format!(
                        "{}: missing indexed game {}: {error}",
                        entry.system_name,
                        path.display()
                    ));
                    continue;
                }
                Err(error) => return Err(DegaussError::io("checking duplicate size", path, error)),
            };
            (metadata.len(), None)
        };
        *sizes
            .entry((entry.system.clone(), extension.clone(), size))
            .or_insert(0usize) += 1;
        candidates.push((entry, Some((extension, size, digest))));
    }
    let mut content = HashSet::new();
    let mut games = Vec::with_capacity(candidates.len());
    for (entry, candidate) in candidates {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let Some((extension, size, digest)) = candidate else {
            games.push(entry);
            continue;
        };
        if sizes[&(entry.system.clone(), extension.clone(), size)] < 2 {
            games.push(entry);
            continue;
        }
        let Kind::Play(Launch::File(path)) = &entry.row.kind else {
            unreachable!("only file entries are duplicate candidates");
        };
        let digest = if let Some(digest) = digest {
            digest
        } else {
            let mut file = match std::fs::File::open(path) {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    notices.push(format!(
                        "{}: missing indexed game {}: {error}",
                        entry.system_name,
                        path.display()
                    ));
                    continue;
                }
                Err(error) => {
                    return Err(DegaussError::io("opening duplicate candidate", path, error))
                }
            };
            let mut digest = crc32fast::Hasher::new();
            let mut buffer = [0u8; 128 * 1024];
            loop {
                if cancelled.load(Ordering::Relaxed) {
                    return Ok(None);
                }
                let read = file.read(&mut buffer).map_err(|error| {
                    DegaussError::io("checking duplicate candidate", path, error)
                })?;
                if read == 0 {
                    break;
                }
                digest.update(&buffer[..read]);
            }
            digest.finalize()
        };
        if content.insert((entry.system.clone(), extension, size, digest)) {
            games.push(entry);
        }
    }
    Ok(Some(games))
}

fn load(
    mut request: Request,
    cancelled: &AtomicBool,
    mut progress: impl FnMut(String),
) -> Result<Option<Catalogue>> {
    if cancelled.load(Ordering::Relaxed) {
        return Ok(None);
    }
    let signatures = request.source_signatures(cancelled);
    if cancelled.load(Ordering::Relaxed) {
        return Ok(None);
    }
    if request
        .previous
        .as_ref()
        .is_some_and(|previous| previous.signatures == signatures)
    {
        let mut previous = request.previous.take().expect("unchanged projection");
        if previous.names != request.names || previous.mra_filenames != request.mra_filenames {
            previous.present_names(request.names, request.mra_filenames);
        }
        return Ok(Some(previous));
    }
    let mut catalogue = Catalogue::default();
    let mut previous = request.previous.take().unwrap_or_default();
    let mut previous_entries: BTreeMap<String, Vec<Entry>> = BTreeMap::new();
    for entry in previous.entries.drain(..) {
        previous_entries
            .entry(entry.system.clone())
            .or_default()
            .push(entry);
    }
    for source in &request.sources {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        progress(source.name.clone());
        if previous.signatures.get(&source.id) == signatures.get(&source.id)
            && signatures.contains_key(&source.id)
        {
            catalogue
                .entries
                .extend(previous_entries.remove(&source.id).unwrap_or_default());
            if let Some(provider) = previous.providers.remove(&source.id) {
                catalogue.providers.insert(source.id.clone(), provider);
            }
            if let Some(notices) = previous.notices.remove(&source.id) {
                catalogue.notices.insert(source.id.clone(), notices);
            }
            continue;
        }
        let mut notices = Vec::new();
        let mut entries = Vec::new();
        let (cache, provider) = match read_source(&request, source) {
            Ok(data) => data,
            Err(error) => {
                catalogue
                    .notices
                    .insert(source.id.clone(), vec![format!("{}: {error}", source.name)]);
                continue;
            }
        };
        let mut pending = vec![crate::browse::start_for(&source.config)];
        let mut visited = HashSet::new();
        while let Some(place) = pending.pop() {
            if cancelled.load(Ordering::Relaxed) {
                return Ok(None);
            }
            if !visited.insert(place.key()) {
                continue;
            }
            let Some(folder) = cache.get(&place) else {
                notices.push(format!(
                    "{}: indexed folder unavailable: {}",
                    source.name,
                    place.path().display()
                ));
                continue;
            };
            let mut rows = folder.rows.clone();
            if let Some(provider) = &provider {
                provider.apply_prepared(&mut rows);
            }
            for mut row in rows {
                if cancelled.load(Ordering::Relaxed) {
                    return Ok(None);
                }
                if request.hidden.contains(&hidden_key(&row)) {
                    continue;
                }
                match &row.kind {
                    Kind::Enter(place) => pending.push(place.clone()),
                    Kind::Play(_) => {
                        // Descriptions are read on demand by the common information worker.
                        row.details.desc.clear();
                        let decade = Field::Year
                            .value(&row)
                            .and_then(|year| year.parse::<u32>().ok())
                            .map(|year| format!("{}s", year / 10 * 10));
                        let search_title = request
                            .names
                            .apply(&row.name)
                            .to_lowercase()
                            .replace(' ', "");
                        let entry = Entry {
                            system: source.id.clone(),
                            system_name: source.name.clone(),
                            category: source.category.clone(),
                            original_name: row.name.clone(),
                            row,
                            decade,
                            search_title,
                        };
                        entries.push(entry);
                    }
                }
            }
        }
        progress(format!("{}: checking duplicate references", source.name));
        let Some(entries) = deduplicate(entries, cancelled, &mut notices)? else {
            return Ok(None);
        };
        catalogue.entries.extend(entries);
        if let Some(provider) = provider {
            catalogue.providers.insert(source.id.clone(), provider);
        }
        catalogue.notices.insert(source.id.clone(), notices);
    }
    catalogue.signatures = signatures;
    catalogue.omitted = catalogue.notices.values().flatten().cloned().collect();
    catalogue.present_names(request.names, request.mra_filenames);
    Ok((!cancelled.load(Ordering::Relaxed)).then_some(catalogue))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(system: &str, title: &str, genre: Option<&str>, developer: &str, year: &str) -> Entry {
        Entry {
            system: system.into(),
            system_name: system.into(),
            category: "Console".into(),
            original_name: title.into(),
            search_title: title.to_lowercase().replace(' ', ""),
            decade: year
                .get(..4)
                .and_then(|year| year.parse::<u32>().ok())
                .map(|year| format!("{}s", year / 10 * 10)),
            row: Row {
                name: title.into(),
                sort_key: title.into(),
                kind: Kind::Play(Launch::File(PathBuf::from(format!("/{system}/{title}")))),
                cover: None,
                genre: genre.map(str::to_string),
                favorite: false,
                below: None,
                details: crate::browse::Details {
                    developer: developer.into(),
                    released: year.into(),
                    ..Default::default()
                },
            },
        }
    }

    #[test]
    fn amiga_and_folder_variant_captions_never_expose_internal_keys() {
        let mut catalogue = Catalogue::default();
        for title in ["Game (AGA)", "Game (ECS)"] {
            let mut game = entry("Amiga", title, None, "", "");
            game.row.kind = Kind::Play(Launch::AmigaVision {
                install: PathBuf::from("/Amiga/install"),
                title: title.into(),
            });
            catalogue.entries.push(game);
        }
        for (title, path) in [("Folder (A)", "/A"), ("Folder (B)", "/B")] {
            let mut folder = entry("Folders", title, None, "", "");
            folder.row.kind = Kind::Enter(crate::browse::Place::Dir(PathBuf::from(path)));
            catalogue.entries.push(folder);
        }
        for install in ["/Amiga/First", "/Amiga/Second"] {
            let mut game = entry("Amiga", "Same game", None, "", "");
            game.row.kind = Kind::Play(Launch::AmigaVision {
                install: PathBuf::from(install),
                title: "Same game".into(),
            });
            catalogue.entries.push(game);
        }
        let identities: HashSet<_> = catalogue.entries.iter().map(Entry::key).collect();
        catalogue.present_names(
            crate::name_display::GameNameDisplay::RemoveParentheses,
            false,
        );
        let captions: HashSet<_> = catalogue
            .entries
            .iter()
            .map(|entry| entry.row.name.as_str())
            .collect();
        assert_eq!(
            captions,
            HashSet::from([
                "Game · Game (AGA)",
                "Game · Game (ECS)",
                "Folder · /A",
                "Folder · /B",
                "Same game · /Amiga/First",
                "Same game · /Amiga/Second"
            ])
        );
        assert!(captions.iter().all(|name| !name.contains('\0')));
        assert_eq!(
            identities,
            catalogue.entries.iter().map(Entry::key).collect()
        );
        catalogue.present_names(crate::name_display::GameNameDisplay::Full, false);
        assert!(catalogue
            .entries
            .iter()
            .filter(|entry| entry.original_name != "Same game")
            .all(|entry| entry.row.name == entry.original_name));
        // Full titles still need a qualifier when distinct installations
        // genuinely share a title; switching modes must not collapse them.
        assert_eq!(
            catalogue
                .entries
                .iter()
                .filter(|entry| entry.original_name == "Same game")
                .map(|entry| entry.row.name.as_str())
                .collect::<HashSet<_>>(),
            HashSet::from(["Same game · /Amiga/First", "Same game · /Amiga/Second"])
        );
    }

    #[test]
    fn variant_qualifiers_do_not_change_title_search_results() {
        let mut catalogue = Catalogue::default();
        for path in [
            "/media/fat/games/NES/First/Same Game.nes",
            "/media/usb0/games/NES/Second/Same Game.nes",
        ] {
            let mut game = entry("NES", "Same Game", None, "", "");
            game.row.kind = Kind::Play(Launch::File(PathBuf::from(path)));
            catalogue.entries.push(game);
        }
        catalogue
            .entries
            .push(entry("NES", "NES Adventure", None, "", ""));
        let identities: HashSet<_> = catalogue.entries.iter().map(Entry::key).collect();
        for names in [
            crate::name_display::GameNameDisplay::Full,
            crate::name_display::GameNameDisplay::RemoveParenthesesAndBrackets,
        ] {
            catalogue.present_names(names, false);
            let variants: Vec<_> = catalogue
                .entries
                .iter()
                .filter(|entry| entry.original_name == "Same Game")
                .collect();
            assert_ne!(variants[0].row.name, variants[1].row.name);
            for game in variants {
                assert!(Query::default().matches_except(game, None, "same game"));
                for path_word in ["fat", "usb", "games", "nes", "first", "second"] {
                    assert!(
                        !Query::default().matches_except(game, None, path_word),
                        "a distinguishing path is not part of the title query"
                    );
                }
            }
            let title = catalogue
                .entries
                .iter()
                .find(|entry| entry.original_name == "NES Adventure")
                .unwrap();
            assert!(Query::default().matches_except(title, None, "nes"));
            assert_eq!(
                identities,
                catalogue.entries.iter().map(Entry::key).collect()
            );
        }
    }

    #[test]
    fn identical_copies_collapse_but_real_variants_and_system_owners_remain() {
        let root = duplicate_fixture("copies");
        let mut entries = Vec::new();
        for (folder, bytes) in [
            ("Original", b"game".as_slice()),
            ("Copy", b"game".as_slice()),
            ("Japan", b"variant".as_slice()),
        ] {
            std::fs::create_dir(root.path().join(folder)).unwrap();
            let path = root.path().join(folder).join("Game.nes");
            std::fs::write(&path, bytes).unwrap();
            let mut row = entry("NES", "Same metadata title", None, "", "");
            row.row.kind = Kind::Play(Launch::File(path));
            entries.push(row);
        }
        let mut other = entries[0].clone();
        other.system = "Other".into();
        entries.push(other);
        let mut catalogue = Catalogue {
            entries: deduplicate(entries, &AtomicBool::new(false), &mut Vec::new())
                .unwrap()
                .unwrap(),
            ..Default::default()
        };
        assert_eq!(
            catalogue.entries.len(),
            3,
            "only identical copies belonging to the same system collapse"
        );
        let paths = catalogue
            .entries
            .iter()
            .map(Entry::key)
            .collect::<HashSet<_>>();
        catalogue.present_names(
            crate::name_display::GameNameDisplay::RemoveParenthesesAndBrackets,
            false,
        );
        let nes = catalogue
            .entries
            .iter()
            .filter(|entry| entry.system == "NES")
            .collect::<Vec<_>>();
        assert_ne!(
            nes[0].row.name, nes[1].row.name,
            "distinct contents with one metadata title have distinct captions"
        );
        assert_eq!(
            catalogue
                .entries
                .iter()
                .map(Entry::key)
                .collect::<HashSet<_>>(),
            paths,
            "presentation never changes launch identity"
        );
        assert!(
            deduplicate(catalogue.entries, &AtomicBool::new(true), &mut Vec::new())
                .unwrap()
                .is_none()
        );
    }

    #[cfg(unix)]
    #[test]
    fn different_size_candidates_do_not_read_their_content() {
        use std::os::unix::fs::PermissionsExt;
        let root = duplicate_fixture("different-sizes");
        let mut entries = Vec::new();
        for (folder, bytes) in [("A", b"a".as_slice()), ("B", b"variant".as_slice())] {
            std::fs::create_dir(root.path().join(folder)).unwrap();
            let path = root.path().join(folder).join("Game.chd");
            std::fs::write(&path, bytes).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
            let mut row = entry("PSX", "Same title", None, "", "");
            row.row.kind = Kind::Play(Launch::File(path));
            entries.push(row);
        }
        assert_eq!(
            deduplicate(entries, &AtomicBool::new(false), &mut Vec::new())
                .unwrap()
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn missing_games_are_reported_without_discarding_healthy_entries() {
        let root = duplicate_fixture("missing");
        let healthy = root.path().join("Healthy.nes");
        std::fs::write(&healthy, b"game").unwrap();
        let mut entries = Vec::new();
        for path in [
            healthy.clone(),
            root.path().join("Deleted.nes"),
            root.path().join("Deleted.zip/Game.nes"),
        ] {
            let mut game = entry("NES", "Game", None, "", "");
            game.row.kind = Kind::Play(Launch::File(path));
            entries.push(game);
        }
        let mut notices = Vec::new();
        let games = deduplicate(entries, &AtomicBool::new(false), &mut notices)
            .unwrap()
            .unwrap();
        assert_eq!(games.len(), 1);
        assert_eq!(games[0].row.kind, Kind::Play(Launch::File(healthy)));
        assert_eq!(notices.len(), 2);
        assert!(notices
            .iter()
            .all(|notice| notice.contains("missing indexed game")));
        assert!(notices.iter().any(|notice| notice.contains("Deleted.nes")));
        assert!(notices.iter().any(|notice| notice.contains("Deleted.zip")));
    }

    #[test]
    fn missing_zip_members_are_reported_but_corrupt_archives_are_not_hidden() {
        let root = duplicate_fixture("missing-member");
        let archive = root.path().join("Games.zip");
        std::fs::write(&archive, crate::zip::tests_archive(&["Healthy.nes"], false)).unwrap();
        let mut entries = Vec::new();
        for member in ["Healthy.nes", "Deleted.nes"] {
            let mut game = entry("NES", "Game", None, "", "");
            game.row.kind = Kind::Play(Launch::File(archive.join(member)));
            entries.push(game);
        }
        let mut notices = Vec::new();
        let games = deduplicate(entries.clone(), &AtomicBool::new(false), &mut notices)
            .unwrap()
            .unwrap();
        assert_eq!(games.len(), 1);
        assert_eq!(notices.len(), 1);
        assert!(notices[0].contains("no longer contains Deleted.nes"));
        std::fs::write(&archive, b"not a ZIP").unwrap();
        assert!(deduplicate(entries, &AtomicBool::new(false), &mut Vec::new()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn organised_arcade_links_do_not_duplicate_the_original_mra() {
        let root = duplicate_fixture("symlinks");
        let game = root.path().join("1941 (World).mra");
        std::fs::write(&game, b"<misterromdescription/>").unwrap();
        std::fs::create_dir(root.path().join("Organised")).unwrap();
        let alias = root.path().join("Organised/1941 (World).mra");
        std::os::unix::fs::symlink(&game, &alias).unwrap();
        let mut original = entry("Arcade", "1941", None, "", "");
        original.row.kind = Kind::Play(Launch::File(game.clone()));
        let mut linked = original.clone();
        linked.row.kind = Kind::Play(Launch::File(alias));
        let rows = deduplicate(
            vec![linked, original],
            &AtomicBool::new(false),
            &mut Vec::new(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].row.kind,
            Kind::Play(Launch::File(game)),
            "the root entry wins over its organised alias"
        );
    }

    #[test]
    fn identical_mgl_copies_collapse_but_different_launch_targets_remain() {
        let root = duplicate_fixture("mgl");
        let mut rows = Vec::new();
        for folder in ["A", "B", "Japan"] {
            std::fs::create_dir(root.path().join(folder)).unwrap();
            let path = root.path().join(folder).join("Game.mgl");
            let game = if folder == "Japan" {
                "Japan.rom"
            } else {
                "Game.rom"
            };
            std::fs::write(&path, format!("<mistergamedescription><rbf>NES</rbf><setname same_dir=\"1\">NES</setname><file delay=\"1\" type=\"f\" index=\"0\" path=\"{game}\"/></mistergamedescription>"))
                .unwrap();
            let mut row = entry("NES", "Same title", None, "", "");
            row.row.kind = Kind::Play(Launch::File(path));
            rows.push(row);
        }
        assert_eq!(
            deduplicate(rows, &AtomicBool::new(false), &mut Vec::new())
                .unwrap()
                .unwrap()
                .len(),
            2,
            "Main resolves relative files against the core home, not the MGL's directory"
        );
    }

    #[test]
    fn filters_and_counts_use_other_constraints_and_preserve_zero_selection() {
        let catalogue = Catalogue {
            entries: vec![
                entry("a", "Alpha", Some("Action"), "Dev", "19910101"),
                entry("b", "Beta", Some("Puzzle"), "Dev", "1993"),
                entry("b", "Gamma", None, "", ""),
            ],
            ..Default::default()
        };
        let mut query = Query::default();
        query.choose(
            Facet::Metadata(Field::Developer),
            Some(Criterion::Known {
                label: "Dev".into(),
                key: "dev".into(),
            }),
        );
        query.choose(
            Facet::Metadata(Field::Genre),
            Some(Criterion::Known {
                label: "Missing".into(),
                key: "missing".into(),
            }),
        );
        assert!(query.matching(&catalogue).is_empty());
        let choices = query.choices(&catalogue, Facet::Metadata(Field::Genre));
        assert_eq!(choices[0].count, 2);
        assert!(choices
            .iter()
            .any(|choice| choice.label() == "Missing" && choice.count == 0));
        query.choose(Facet::Metadata(Field::Genre), None);
        query.choose(
            Facet::Decade,
            Some(Criterion::Known {
                label: "1990s".into(),
                key: "1990s".into(),
            }),
        );
        assert_eq!(query.matching(&catalogue), [0, 1]);
        query.fields = Default::default();
        query.decade = Some(Criterion::Unknown);
        assert_eq!(query.matching(&catalogue), [2]);
    }

    #[test]
    fn title_words_match_in_any_order_and_facet_counts_use_the_same_query() {
        let catalogue = Catalogue {
            entries: vec![
                entry("NES", "Super Mario Bros.", Some("Platform"), "", ""),
                entry("NES", "Mario Kart", Some("Racing"), "", ""),
            ],
            ..Default::default()
        };
        for title in [
            "super mario",
            "mario super",
            "SUPERMARIO",
            "  mario\t super  ",
        ] {
            let query = Query {
                title: title.into(),
                ..Default::default()
            };
            assert_eq!(
                query.matching(&catalogue),
                [0],
                "all requested words must match"
            );
            assert_eq!(query.choices(&catalogue, Facet::System)[0].count, 1);
        }
    }

    #[test]
    fn unchanged_worker_refresh_returns_the_same_projection_without_decoding_or_sorting() {
        let (_root, request) = fixture();
        let mut unchanged = Request {
            sources: request.sources.clone(),
            cache_dir: request.cache_dir.clone(),
            hidden: request.hidden.clone(),
            language: request.language.clone(),
            names: request.names,
            mra_filenames: request.mra_filenames,
            previous: None,
        };
        let cancel = AtomicBool::new(false);
        let catalogue = load(request, &cancel, |_| {}).unwrap().unwrap();
        assert!(!catalogue.signatures.is_empty());
        let pointer = catalogue.entries.as_ptr();
        unchanged.previous = Some(catalogue);
        let catalogue = load(unchanged, &cancel, |_| {
            panic!("unchanged owners need no traversal")
        })
        .unwrap()
        .unwrap();
        assert_eq!(catalogue.entries.as_ptr(), pointer);
    }

    #[test]
    fn a_name_setting_changed_outside_explore_reprojects_the_reused_catalogue_only() {
        let (_root, request) = fixture();
        let mut changed = Request {
            sources: request.sources.clone(),
            cache_dir: request.cache_dir.clone(),
            hidden: request.hidden.clone(),
            language: request.language.clone(),
            names: crate::name_display::GameNameDisplay::RemoveParentheses,
            mra_filenames: false,
            previous: None,
        };
        let cancel = AtomicBool::new(false);
        let mut catalogue = load(request, &cancel, |_| {}).unwrap().unwrap();
        catalogue.entries[0].original_name = "Canonical (USA)".into();
        catalogue.present_names(crate::name_display::GameNameDisplay::Full, false);
        let query = Query {
            title: "USA".into(),
            ..Default::default()
        };
        assert_eq!(query.matching(&catalogue).len(), 1);
        changed.previous = Some(catalogue);
        let catalogue = load(changed, &cancel, |_| {
            panic!("a display change must not reread owners")
        })
        .unwrap()
        .unwrap();
        assert!(query.matching(&catalogue).is_empty());
        assert!(catalogue
            .entries
            .iter()
            .any(|entry| entry.original_name == "Canonical (USA)"));
    }

    #[test]
    fn identical_titles_and_paths_do_not_merge_different_owners() {
        let a = entry("a", "Same", None, "", "");
        let mut b = a.clone();
        b.system = "b".into();
        assert_ne!(a.key(), b.key());
        assert_eq!(a.key(), a.clone().key());
    }

    #[test]
    fn cancelling_a_ready_but_unconsumed_result_returns_to_the_origin() {
        let (sender, receiver) = mpsc::channel();
        let job = Job {
            receiver,
            cancelled: Arc::new(AtomicBool::new(false)),
        };
        sender.send(Event::Ready(Catalogue::default())).unwrap();
        job.cancel();
        assert!(matches!(job.try_recv(), Some(Event::Cancelled)));
    }

    #[test]
    fn saved_criteria_roundtrip_and_names_are_unique_without_renaming_ids() {
        let mut collections = BTreeMap::new();
        let (id, next) = next_collection_id(&collections, 1);
        let collection = Collection {
            name: "Action games".into(),
            query: Query {
                title: "alpha".into(),
                ..Default::default()
            },
        };
        collections.insert(id.clone(), collection.clone());
        assert!(collection_name(" ACTION GAMES ", &collections, None).is_err());
        assert!(collection_name("Action games", &collections, Some(&id)).is_ok());
        assert_ne!(id, next_collection_id(&collections, next).0);
        collections.remove(&id);
        assert_ne!(
            id,
            next_collection_id(&collections, next).0,
            "removed IDs must never retarget Home shortcuts"
        );
        let text = toml::to_string(&collection).unwrap();
        assert_eq!(toml::from_str::<Collection>(&text).unwrap(), collection);
        let mut query = collection.query;
        query.fields[Field::Year.index()] = Some(Criterion::Unknown);
        query.choose(
            Facet::Metadata(Field::Genre),
            Some(Criterion::Known {
                label: "Action".into(),
                key: "action".into(),
            }),
        );
        let resume = Resume {
            query: query.clone(),
            origin: None,
            pivots: vec![(query.clone(), 17)],
        };
        let text = toml::to_string(&resume).unwrap();
        let saved: Resume = toml::from_str(&text).unwrap();
        assert_eq!(saved.query, query);
        assert_eq!(saved.pivots, resume.pivots);
    }

    struct Fixture(PathBuf);
    fn duplicate_fixture(name: &str) -> Fixture {
        let root = std::env::temp_dir().join(format!(
            "degauss-explore-dedup-{name}-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        Fixture(root)
    }
    impl Fixture {
        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn fixture() -> (Fixture, Request) {
        let root = (0..)
            .find_map(|index| {
                let path = std::env::temp_dir()
                    .join(format!("degauss-explore-{}-{index}", std::process::id()));
                match std::fs::create_dir(&path) {
                    Ok(()) => Some(Fixture(path)),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => None,
                    Err(error) => panic!("Explore fixture: {error}"),
                }
            })
            .unwrap();
        let table = crate::systems::parse_table(
            include_str!("../assets/systems.toml"),
            std::path::Path::new("systems.toml"),
        )
        .unwrap();
        let mut sources = Vec::new();
        for id in ["NES", "SNES"] {
            let path = root.path().join(id);
            std::fs::create_dir_all(path.join("Hidden")).unwrap();
            let extension = if id == "NES" { "nes" } else { "sfc" };
            std::fs::write(path.join(format!("Alpha.{extension}")), b"fixture").unwrap();
            std::fs::write(path.join(format!("Hidden/Secret.{extension}")), b"fixture").unwrap();
            std::fs::write(path.join("gamelist.xml"), format!("<gameList><game><path>Alpha.{extension}</path><name>Gamelist title</name><genre>Action</genre><desc>On demand only</desc></game></gameList>")).unwrap();
            let found = crate::systems::FoundSystem {
                def: table.iter().find(|system| system.id == id).unwrap().clone(),
                paths: vec![path],
                logo_dir: None,
                menu_folder: None,
            };
            let config = found.to_config();
            let library = crate::browse::Library::open(&config).unwrap();
            crate::cache::save_system(
                &root.path().join("cache"),
                id,
                &crate::cache::build_system(&library),
            )
            .unwrap();
            sources.push(Source {
                id: id.into(),
                name: id.into(),
                category: "Consoles".into(),
                config,
                pack: None,
                error: None,
            });
        }
        let request = Request {
            cache_dir: root.path().join("cache"),
            sources,
            hidden: HashSet::new(),
            language: None,
            names: Default::default(),
            mra_filenames: false,
            previous: None,
        };
        (root, request)
    }

    #[test]
    fn indexed_traversal_respects_hidden_subtrees_reuses_unchanged_owners_and_cancels() {
        let (root, mut request) = fixture();
        let mut changed = Request {
            sources: request.sources.clone(),
            cache_dir: request.cache_dir.clone(),
            hidden: HashSet::new(),
            language: None,
            names: Default::default(),
            mra_filenames: false,
            previous: None,
        };
        request
            .hidden
            .insert(format!("d:{}/Hidden", request.sources[0].config.path));
        let cancel = AtomicBool::new(false);
        let catalogue = load(request, &cancel, |_| {}).unwrap().unwrap();
        assert_eq!(catalogue.entries.len(), 3);
        assert!(catalogue
            .entries
            .iter()
            .all(|entry| entry.row.details.desc.is_empty()));
        let pointer = catalogue
            .entries
            .iter()
            .find(|entry| entry.system == "NES")
            .unwrap()
            .row
            .name
            .as_ptr();
        changed
            .hidden
            .insert(format!("d:{}/Hidden", changed.sources[0].config.path));
        changed
            .hidden
            .insert(format!("d:{}/Hidden", changed.sources[1].config.path));
        changed.previous = Some(catalogue);
        let catalogue = load(changed, &cancel, |_| {}).unwrap().unwrap();
        assert_eq!(catalogue.entries.len(), 2);
        assert_eq!(
            catalogue
                .entries
                .iter()
                .find(|entry| entry.system == "NES")
                .unwrap()
                .row
                .name
                .as_ptr(),
            pointer,
            "unchanged system rows must move, not be decoded or cloned again"
        );
        drop(root);
        let (_root, request) = fixture();
        cancel.store(true, Ordering::Relaxed);
        assert!(load(request, &cancel, |_| {}).unwrap().is_none());
    }

    #[test]
    fn selected_pack_uses_prepared_metadata_and_never_falls_back_to_gamelist() {
        let (root, mut request) = fixture();
        request.sources.truncate(1);
        let pack = root.path().join("docs");
        std::fs::create_dir_all(&pack).unwrap();
        request.sources[0].pack = Some(pack.clone());
        let cancel = AtomicBool::new(false);
        let missing = load(request, &cancel, |_| {}).unwrap().unwrap();
        assert!(missing.entries.is_empty());
        assert_eq!(missing.omitted.len(), 1);
        assert!(missing.omitted[0].contains("not prepared"));
        let (_unused, mut request) = fixture();
        request.cache_dir = root.path().join("cache");
        request.sources.truncate(1);
        request.sources[0].config.path = root.path().join("NES").to_string_lossy().into_owned();
        request.sources[0].pack = Some(pack.clone());
        let library = crate::browse::Library::open_source_neutral(
            &request.sources[0].config,
            Default::default(),
        )
        .unwrap();
        let cache = crate::cache::build_system(&library);
        let launch = cache
            .folders
            .values()
            .flat_map(|folder| &folder.rows)
            .find_map(|row| match &row.kind {
                Kind::Play(launch @ Launch::File(path)) if path.ends_with("Alpha.nes") => {
                    Some(launch.clone())
                }
                _ => None,
            })
            .unwrap();
        crate::cache::install_transactional(
            &request.cache_dir,
            crate::cache::CacheKind::ArtworkPack,
            &[crate::cache::StagedSystemCache {
                id: "NES".into(),
                cache,
                fingerprints: Default::default(),
                fingerprints_complete: true,
            }],
        )
        .unwrap();
        let prepared = std::collections::HashMap::from([(
            launch,
            crate::artwork_pack::PackPresentation {
                name: Some("Pack title".into()),
                genre: Some("Puzzle".into()),
                ..Default::default()
            },
        )]);
        crate::cache::save_pack_state(
            &request.cache_dir,
            "NES",
            &crate::cache::PackSourceState {
                accepted: Some(crate::cache::AcceptedSource {
                    docs_root: pack.to_string_lossy().into_owned(),
                    language: None,
                    signature: None,
                    cache_marker: 0,
                    health: crate::artwork_pack::ProviderHealth::Ready,
                    diagnostics: vec![],
                    skipped_entries: 0,
                }),
                declined: None,
            },
            &prepared,
        )
        .unwrap();
        let catalogue = load(request, &cancel, |_| {}).unwrap().unwrap();
        assert!(catalogue.omitted.is_empty());
        assert!(catalogue
            .entries
            .iter()
            .any(|entry| entry.row.name == "Pack title"
                && entry.row.genre.as_deref() == Some("Puzzle")));
        assert!(!catalogue
            .entries
            .iter()
            .any(|entry| entry.row.name == "Gamelist title"));
    }
}
