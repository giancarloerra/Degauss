//! Cross-system discovery from selected-source indexes, never a ROM scan.

use std::collections::{BTreeMap, HashSet};
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
    /// Visible systems omitted because their selected source is not indexed or readable.
    pub omitted: Vec<String>,
    pub providers: BTreeMap<String, crate::artwork_pack::Provider>,
    pub signatures: BTreeMap<String, String>,
    names: crate::name_display::GameNameDisplay,
    notices: BTreeMap<String, Vec<String>>,
}

impl Catalogue {
    pub fn present_names(&mut self, names: crate::name_display::GameNameDisplay) {
        self.names = names;
        for entry in &mut self.entries {
            entry.search_title = names.apply(&entry.row.name).to_lowercase().replace(' ', "");
        }
        self.entries.sort_by_cached_key(|entry| {
            (
                normalise(names.apply(&entry.row.name).as_ref()),
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
        if previous.names != request.names {
            previous.present_names(request.names);
        }
        return Ok(Some(previous));
    }
    let mut catalogue = Catalogue::default();
    let mut identities = HashSet::new();
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
                            row,
                            decade,
                            search_title,
                        };
                        if identities.insert(entry.key()) {
                            catalogue.entries.push(entry);
                        }
                    }
                }
            }
        }
        if let Some(provider) = provider {
            catalogue.providers.insert(source.id.clone(), provider);
        }
        catalogue.notices.insert(source.id.clone(), notices);
    }
    catalogue.signatures = signatures;
    catalogue.omitted = catalogue.notices.values().flatten().cloned().collect();
    catalogue.present_names(request.names);
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
            previous: None,
        };
        let cancel = AtomicBool::new(false);
        let mut catalogue = load(request, &cancel, |_| {}).unwrap().unwrap();
        catalogue.entries[0].row.name = "Canonical (USA)".into();
        catalogue.present_names(crate::name_display::GameNameDisplay::Full);
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
            .any(|entry| entry.row.name == "Canonical (USA)"));
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
