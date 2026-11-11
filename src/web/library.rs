//! Browser reads have independent navigation and use the native indexes and
//! projections. The single bounded worker keeps disk and catalogue work off Slint.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};

use serde_json::{json, Value};

use crate::browse::{Kind, Place, Row};
use crate::{explore, home, settings};

use super::api::Error;

pub struct Snapshot {
    pub library: explore::Request,
    pub settings: settings::Settings,
    pub menu_root: PathBuf,
    pub history_path: PathBuf,
    pub names: crate::browse::DisplayNames,
    pub systems: Vec<crate::systems::FoundSystem>,
    pub homes: Arc<crate::mgl::Homes>,
    pub favorites: crate::favorites::Favorites,
    pub pack_roots: BTreeMap<String, String>,
}

pub enum Operation {
    HomeArtwork {
        targets: BTreeMap<String, Vec<(String, crate::browse::Launch)>>,
    },
    Browse {
        system: String,
        trail: Vec<Place>,
        offset: usize,
        limit: usize,
        search: String,
        filters: explore::Query,
        pick: Option<(u64, bool)>,
    },
    Explore {
        query: explore::Query,
        offset: usize,
        limit: usize,
        pick: Option<(u64, bool)>,
    },
    History,
    Scripts {
        folder: Option<PathBuf>,
    },
    Game {
        system: String,
        launch: crate::browse::Launch,
        details_system: String,
        details_launch: crate::browse::Launch,
    },
}

pub struct Entry {
    pub target: Option<home::Target>,
    pub row: Option<Row>,
    pub name: String,
    pub system: Option<String>,
    pub system_name: Option<String>,
    pub folder: Option<PathBuf>,
}

pub struct Page {
    pub entries: Vec<Entry>,
    pub data: Value,
}

pub enum Event {
    Progress(u64, String),
    Finished(u64, Result<Page, Error>),
}

struct Task {
    id: u64,
    snapshot: Snapshot,
    operation: Operation,
}

pub struct Worker {
    sender: Option<mpsc::SyncSender<Task>>,
    pub events: mpsc::Receiver<Event>,
    cancelled: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Worker {
    pub fn start() -> Result<Self, Error> {
        let (sender, tasks) = mpsc::sync_channel::<Task>(1);
        let (events, receiver) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let flag = cancelled.clone();
        let handle = std::thread::Builder::new()
            .name("web-library".into())
            .spawn(move || {
                let mut state = LibraryState::default();
                while let Ok(task) = tasks.recv() {
                    let progress = |message| {
                        let _ = events.send(Event::Progress(task.id, message));
                    };
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        state.read(task.snapshot, task.operation, &flag, progress)
                    }))
                    .unwrap_or_else(|_| {
                        Err(Error::new(
                            "worker",
                            "The library worker stopped unexpectedly.",
                        ))
                    });
                    let result = if flag.load(Ordering::Relaxed) {
                        Err(Error::new("cancelled", "Operation cancelled."))
                    } else {
                        result
                    };
                    if events.send(Event::Finished(task.id, result)).is_err() {
                        break;
                    }
                }
            })
            .map_err(|e| Error::new("worker", format!("Starting library worker failed: {e}")))?;
        Ok(Self {
            sender: Some(sender),
            events: receiver,
            cancelled,
            handle: Some(handle),
        })
    }

    pub fn submit(&self, id: u64, snapshot: Snapshot, operation: Operation) -> Result<(), Error> {
        self.cancelled.store(false, Ordering::Relaxed);
        self.sender
            .as_ref()
            .expect("live worker")
            .try_send(Task {
                id,
                snapshot,
                operation,
            })
            .map_err(|_| {
                Error::new(
                    "busy",
                    "The browser library worker is busy. No operation was queued.",
                )
            })
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.cancel();
        self.sender.take();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

struct SourceCache {
    signature: String,
    cache: Option<crate::cache::SystemCache>,
    live: Option<crate::browse::Library>,
    provider: Option<crate::artwork_pack::Provider>,
}

#[derive(Default)]
struct LibraryState {
    catalogue: Option<explore::Catalogue>,
    source: Option<(String, SourceCache)>,
    descriptions: crate::information_job::DescriptionCache,
    projection: Option<Projection>,
}

struct Projection {
    query: explore::Query,
    matches: Vec<usize>,
    facets: Value,
    letters: Value,
    names: crate::name_display::GameNameDisplay,
    mra_filenames: bool,
}

fn letter_offsets<'a>(names: impl Iterator<Item = &'a str>) -> Value {
    let mut letters = BTreeMap::new();
    for (offset, name) in names.enumerate() {
        let first = name.chars().next().unwrap_or(' ');
        let letter = if first.is_alphanumeric() {
            first.to_uppercase().collect::<String>()
        } else {
            "#".into()
        };
        letters.entry(letter).or_insert(offset);
    }
    json!(letters
        .into_iter()
        .map(|(letter, offset)| json!({"letter":letter,"offset":offset}))
        .collect::<Vec<_>>())
}

fn error(error: impl std::fmt::Display) -> Error {
    Error::new("library", error.to_string())
}

fn page_limit(limit: usize) -> Result<usize, Error> {
    if !(1..=200).contains(&limit) {
        return Err(Error::new("page", "Page size must be 1 to 200."));
    }
    Ok(limit)
}

fn is_favorite(favorites: &crate::favorites::Favorites, row: &Row) -> bool {
    match &row.kind {
        Kind::Play(crate::browse::Launch::File(path)) => favorites.holds(path),
        Kind::Play(crate::browse::Launch::AmigaVision { install, title }) => {
            favorites.holds(&crate::favorites::amiga_key(install, title))
        }
        _ => false,
    }
}

fn entry(system: &str, name: &str, row: Row, trail: &[Place]) -> Entry {
    let target = match &row.kind {
        Kind::Play(launch) => Some(home::Target::Game {
            system: system.into(),
            launch: launch.clone(),
        }),
        Kind::Enter(place) => {
            let mut trail = trail.to_vec();
            trail.push(place.clone());
            Some(home::Target::LibraryFolder {
                system: system.into(),
                trail,
            })
        }
        Kind::Up => None,
    };
    Entry {
        name: row.name.clone(),
        row: Some(row),
        target,
        system: Some(system.into()),
        system_name: Some(name.into()),
        folder: None,
    }
}

impl LibraryState {
    fn source(
        &mut self,
        request: &explore::Request,
        source: &explore::Source,
    ) -> Result<&mut SourceCache, Error> {
        let signature = request.source_signature(source);
        if self
            .source
            .as_ref()
            .is_none_or(|(id, cache)| id != &source.id || cache.signature != signature)
        {
            if let Some(problem) = &source.error {
                return Err(error(problem));
            }
            let (cache, provider) = if source.pack.is_some() {
                let (cache, provider) = explore::read_source(request, source).map_err(error)?;
                (Some(cache), provider)
            } else {
                (
                    crate::cache::load_system_checked(&request.cache_dir, &source.id)
                        .map_err(error)?,
                    None,
                )
            };
            self.source = Some((
                source.id.clone(),
                SourceCache {
                    signature,
                    cache,
                    provider,
                    live: None,
                },
            ));
        }
        Ok(&mut self.source.as_mut().expect("loaded source").1)
    }

    fn read(
        &mut self,
        mut snapshot: Snapshot,
        operation: Operation,
        cancelled: &AtomicBool,
        progress: impl FnMut(String),
    ) -> Result<Page, Error> {
        match operation {
            Operation::HomeArtwork { targets } => {
                let mut preview = home::Preview {
                    rows: BTreeMap::new(),
                    missing: vec![],
                };
                let mut notices = Vec::new();
                for (system, targets) in targets {
                    let Some(source) = snapshot
                        .library
                        .sources
                        .iter()
                        .find(|source| source.id == system)
                    else {
                        notices.push(format!("{system}: system is unavailable."));
                        continue;
                    };
                    match explore::read_source(&snapshot.library, source) {
                        Ok((cache, provider)) => home::append_previews(
                            Some(&cache),
                            provider.as_ref(),
                            targets,
                            cancelled,
                            &mut preview,
                        )
                        .map_err(error)?,
                        Err(problem) => notices.push(format!("{}: {problem}", source.name)),
                    }
                }
                Ok(Page {
                    entries: vec![],
                    data: json!({"home_artwork":preview.rows.into_iter().filter_map(|(key,row)|row.cover.map(|path|(key,path))).collect::<BTreeMap<_,_>>(),"notices":notices}),
                })
            }
            Operation::Game {
                system,
                launch,
                details_system,
                details_launch,
            } => {
                let source = snapshot
                    .library
                    .sources
                    .iter()
                    .find(|source| source.id == system)
                    .ok_or_else(|| error("This system is unavailable."))?;
                if !std::iter::once(&source.config.path)
                    .chain(&source.config.extra_paths)
                    .any(|root| Path::new(root).is_dir())
                {
                    return Err(error("This system's storage is unavailable."));
                }
                let cached = self.source(&snapshot.library, source)?;
                let cache = cached
                    .cache
                    .as_ref()
                    .ok_or_else(|| error("This system has no index. Rebuild its system list."))?;
                let provider = cached.provider.clone();
                let mut row=cache.folders.values().flat_map(|folder|&folder.rows).find(|row|matches!(&row.kind,Kind::Play(candidate) if candidate==&launch)).cloned().ok_or_else(||error("This game is no longer in the system index. Rebuild this system if it was moved."))?;
                if let Some(provider) = &provider {
                    provider.apply_prepared(std::slice::from_mut(&mut row));
                }
                let details_source = snapshot
                    .library
                    .sources
                    .iter()
                    .find(|source| source.id == details_system)
                    .ok_or_else(|| error("The game's data source is unavailable."))?;
                let details_provider = if details_system == system {
                    provider
                } else {
                    explore::read_source(&snapshot.library, details_source)
                        .map_err(error)?
                        .1
                };
                let details_request = crate::information_job::Request {
                    launch: details_launch,
                    config: details_source.config.clone(),
                    source: details_provider
                        .map(crate::information_job::Source::ArtworkPack)
                        .unwrap_or(crate::information_job::Source::Gamelist),
                };
                let description = self
                    .descriptions
                    .read(&details_request, cancelled)
                    .map_err(error)?;
                let mut notices = Vec::new();
                if source.category.eq_ignore_ascii_case("Favourites")
                    || source.category.eq_ignore_ascii_case("Favorites")
                {
                    crate::app::enrich_favorite_rows(
                        std::slice::from_mut(&mut row),
                        &snapshot.systems,
                        &snapshot.homes,
                        &snapshot.pack_roots,
                        &snapshot.library.cache_dir,
                        |id, _, _| {
                            let selected = snapshot
                                .library
                                .sources
                                .iter()
                                .find(|source| source.id == id)?;
                            match explore::read_source(&snapshot.library, selected) {
                                Ok((_, provider)) => provider,
                                Err(problem) => {
                                    notices.push(format!("{}: {problem}", selected.name));
                                    None
                                }
                            }
                        },
                    );
                }
                row.details.desc = description.unwrap_or_default();
                row.favorite = source.category.eq_ignore_ascii_case("Favourites")
                    || source.category.eq_ignore_ascii_case("Favorites")
                    || is_favorite(&snapshot.favorites, &row);
                Ok(Page {
                    entries: vec![entry(&system, &source.name, row, &[])],
                    data: json!({"single_game":true,"notices":notices}),
                })
            }
            Operation::Browse {
                system,
                mut trail,
                offset,
                limit,
                search,
                filters,
                pick,
            } => {
                let limit = page_limit(limit)?;
                let source = snapshot
                    .library
                    .sources
                    .iter()
                    .find(|source| source.id == system)
                    .ok_or_else(|| error("This system is not available."))?;
                let cache = self.source(&snapshot.library, source)?;
                let start = crate::browse::start_for(&source.config);
                if trail.is_empty() {
                    trail.push(
                        cache
                            .cache
                            .as_ref()
                            .map(|cache| cache.navigation_start(&start))
                            .unwrap_or(start),
                    );
                }
                let place = trail.last().expect("folder trail");
                let mut notices = Vec::new();
                let mut rows = if let Some(folder) =
                    cache.cache.as_ref().and_then(|cache| cache.get(place))
                {
                    folder.rows.clone()
                } else if source.pack.is_none() {
                    // Native Gamelist browsing can list a folder before indexing.
                    // A malformed index is an error above, not a live-read fallback.
                    if cache.live.is_none() {
                        cache.live = Some(
                            crate::browse::Library::open_with_names(
                                &source.config,
                                snapshot.names.clone(),
                            )
                            .map_err(error)?,
                        );
                    }
                    let (rows, _, skipped) = cache
                        .live
                        .as_ref()
                        .expect("live library")
                        .list_reporting(place, snapshot.settings.show_empty.unwrap_or(false))
                        .map_err(error)?;
                    notices.extend(skipped.iter().map(|skipped| format!("{skipped:?}")));
                    rows
                } else {
                    return Err(error("The selected Artwork Pack folder has not been prepared. Rebuild this system list."));
                };
                if let Some(provider) = &cache.provider {
                    provider.apply_prepared(&mut rows);
                }
                if source.category.eq_ignore_ascii_case("Favourites")
                    || source.category.eq_ignore_ascii_case("Favorites")
                {
                    crate::app::enrich_favorite_rows(
                        &mut rows,
                        &snapshot.systems,
                        &snapshot.homes,
                        &snapshot.pack_roots,
                        &snapshot.library.cache_dir,
                        |id, _, _| {
                            let selected = snapshot
                                .library
                                .sources
                                .iter()
                                .find(|source| source.id == id)?;
                            match explore::read_source(&snapshot.library, selected) {
                                Ok((_, provider)) => provider,
                                Err(problem) => {
                                    notices.push(format!("{}: {problem}", selected.name));
                                    None
                                }
                            }
                        },
                    );
                    for row in &mut rows {
                        if !row.is_folder() {
                            row.favorite = true;
                        }
                    }
                } else {
                    for row in &mut rows {
                        row.favorite = is_favorite(&snapshot.favorites, row);
                    }
                }
                let preferences = snapshot
                    .settings
                    .folder_preferences
                    .get(&system)
                    .and_then(|places| places.get(&place.key()));
                let names = preferences
                    .and_then(|p| p.game_name_display)
                    .unwrap_or(snapshot.library.names);
                let search = search.to_lowercase();
                rows.retain(|row| {
                    !matches!(row.kind, Kind::Up)
                        && !snapshot.library.hidden.contains(&crate::app::row_key(row))
                });
                for row in &mut rows {
                    row.name =
                        crate::name_display::row_name(row, names, snapshot.library.mra_filenames)
                            .into_owned();
                }
                rows.sort_by_cached_key(|row| (row.name.to_lowercase(), row.sort_key.clone()));
                let folders_last = preferences
                    .and_then(|p| p.folders_last)
                    .unwrap_or(snapshot.settings.folders_last.unwrap_or(false));
                crate::app::group_rows(
                    &mut rows,
                    folders_last,
                    snapshot.settings.favorites_first.unwrap_or(true),
                );
                let metadata_choices: BTreeMap<_,_> = crate::game_filter::Field::ALL.into_iter().filter_map(|field| {
                    crate::game_filter::choices(&rows, field).map(|choices| (field.label(), choices.into_iter().map(|choice| json!({"label":choice.label(),"criterion":match choice {crate::game_filter::Choice::Any=>None,crate::game_filter::Choice::Unknown=>Some(crate::game_filter::Criterion::Unknown),crate::game_filter::Choice::Known{label,key}=>Some(crate::game_filter::Criterion::Known{label,key})}})).collect::<Vec<_>>()))
                }).collect();
                let mut selected = crate::game_filter::Filters::default();
                for field in crate::game_filter::Field::ALL {
                    let choice = match &filters.fields[field.index()] {
                        None => crate::game_filter::Choice::Any,
                        Some(crate::game_filter::Criterion::Unknown) => {
                            crate::game_filter::Choice::Unknown
                        }
                        Some(crate::game_filter::Criterion::Known { label, key }) => {
                            crate::game_filter::Choice::Known {
                                label: label.clone(),
                                key: key.clone(),
                            }
                        }
                    };
                    selected.choose(field, &choice);
                }
                rows.retain(|row| {
                    row.name.to_lowercase().contains(&search)
                        && (row.is_folder() || selected.matches(row))
                });
                if let Some((seed, favorites_only)) = pick {
                    let candidates: Vec<_> = rows
                        .iter()
                        .filter(|row| {
                            matches!(row.kind, Kind::Play(_)) && (!favorites_only || row.favorite)
                        })
                        .collect();
                    if let Some(row) = candidates.get(seed as usize % candidates.len().max(1)) {
                        return Ok(Page {
                            entries: vec![entry(&system, &source.name, (*row).clone(), &trail)],
                            data: json!({"notices":notices}),
                        });
                    }
                    // Match native folder picks: only descend when no game is
                    // eligible and there is no active search/metadata filter.
                    if search.is_empty()
                        && !selected.is_active()
                        && trail.len() < crate::browse::MAX_DEPTH
                    {
                        let folders: Vec<_> = rows
                            .iter()
                            .filter_map(|row| match &row.kind {
                                Kind::Enter(place) => Some(place.clone()),
                                _ => None,
                            })
                            .collect();
                        if let Some(place) = folders.get(seed as usize % folders.len().max(1)) {
                            if cancelled.load(Ordering::Relaxed) {
                                return Err(Error::new("cancelled", "Operation cancelled."));
                            }
                            trail.push(place.clone());
                            let mut seed = seed;
                            let seed = crate::app::next_random(&mut seed);
                            let mut page = self.read(
                                snapshot,
                                Operation::Browse {
                                    system,
                                    trail,
                                    offset: 0,
                                    limit: 1,
                                    search,
                                    filters,
                                    pick: Some((seed, favorites_only)),
                                },
                                cancelled,
                                progress,
                            )?;
                            if let Some(existing) = page.data["notices"].as_array_mut() {
                                existing.extend(notices.into_iter().map(Value::String));
                            }
                            return Ok(page);
                        }
                    }
                    return Err(Error::new(
                        "empty",
                        if favorites_only {
                            "No matching favourites in this folder."
                        } else {
                            "No games match this folder selection."
                        },
                    ));
                }
                let total = rows.len();
                let letters = letter_offsets(rows.iter().map(|row| row.name.as_str()));
                let entries = rows
                    .into_iter()
                    .skip(offset)
                    .take(limit)
                    .map(|row| entry(&system, &source.name, row, &trail))
                    .collect();
                Ok(Page {
                    entries,
                    data: json!({"system":system,"name":source.name,"total":total,"offset":offset,"limit":limit,"letters":letters,"facets":metadata_choices,"source":if source.pack.is_some(){"Artwork Pack"}else{"Gamelist"},"trail":trail,"notices":notices}),
                })
            }
            Operation::Explore {
                query,
                offset,
                limit,
                pick,
            } => {
                let limit = page_limit(limit)?;
                let names = snapshot.library.names;
                let mra_filenames = snapshot.library.mra_filenames;
                snapshot.library.previous = self.catalogue.take();
                self.catalogue =
                    explore::load(snapshot.library, cancelled, progress).map_err(error)?;
                let catalogue = self
                    .catalogue
                    .as_ref()
                    .ok_or_else(|| Error::new("cancelled", "Operation cancelled."))?;
                if catalogue.rebuilt
                    || self.projection.as_ref().is_none_or(|projection| {
                        projection.query != query
                            || projection.names != names
                            || projection.mra_filenames != mra_filenames
                    })
                {
                    let matches = query.matching(catalogue);
                    let facets: BTreeMap<_,_> = explore::Facet::ALL.into_iter().map(|facet| (facet.label(),query.choices(catalogue,facet).into_iter().map(|choice| json!({"label":choice.label(),"count":choice.count,"criterion":choice.criterion})).collect::<Vec<_>>())).collect();
                    let letters = letter_offsets(
                        matches
                            .iter()
                            .map(|at| catalogue.entries[*at].row.name.as_str()),
                    );
                    self.projection = Some(Projection {
                        query: query.clone(),
                        matches,
                        facets: json!(facets),
                        letters,
                        names,
                        mra_filenames,
                    });
                }
                let projection = self.projection.as_ref().expect("current projection");
                let matches = &projection.matches;
                let shown: Vec<_> = if let Some((seed, favorites_only)) = pick {
                    let mut eligible = matches.iter().copied().filter(|at| {
                        !favorites_only
                            || is_favorite(&snapshot.favorites, &catalogue.entries[*at].row)
                    });
                    let count = eligible.clone().count();
                    let at = eligible.nth(seed as usize % count.max(1)).ok_or_else(|| {
                        Error::new(
                            "empty",
                            if favorites_only {
                                "No matching favourites in this selection."
                            } else {
                                "No games match this selection."
                            },
                        )
                    })?;
                    vec![at]
                } else {
                    matches.iter().copied().skip(offset).take(limit).collect()
                };
                let entries = shown
                    .iter()
                    .map(|at| {
                        let item = &catalogue.entries[*at];
                        let mut row = item.row.clone();
                        if pick.is_some() {
                            row.favorite = is_favorite(&snapshot.favorites, &row);
                        }
                        entry(&item.system, &item.system_name, row, &[])
                    })
                    .collect();
                Ok(Page {
                    entries,
                    data: json!({"total":matches.len(),"offset":offset,"limit":limit,"letters":projection.letters,"facets":projection.facets,"notices":catalogue.omitted,"query":query}),
                })
            }
            Operation::History => {
                let history =
                    crate::history::History::load(&snapshot.history_path).map_err(error)?;
                let mut entries = Vec::new();
                let mut notices = Vec::new();
                let mut systems = BTreeMap::<_, Vec<_>>::new();
                for (position, game) in history.entries.into_iter().enumerate() {
                    systems
                        .entry(game.system.clone())
                        .or_default()
                        .push((position, game));
                }
                // As in native Last Played, read each system once per visit.
                // Process one at a time rather than retaining several large indexes.
                for (system, games) in systems {
                    if cancelled.load(Ordering::Relaxed) {
                        break;
                    }
                    let Some(source) = snapshot
                        .library
                        .sources
                        .iter()
                        .find(|source| source.id == system)
                    else {
                        notices.extend(
                            games
                                .iter()
                                .map(|(_, game)| format!("{}: system is unavailable", game.name)),
                        );
                        continue;
                    };
                    let (cache, provider) =
                        explore::read_source(&snapshot.library, source).map_err(error)?;
                    for (position, game) in games {
                        let row = cache.folders.values().flat_map(|folder| &folder.rows).find(
                            |row| matches!(&row.kind,Kind::Play(launch) if launch==&game.launch),
                        );
                        if let Some(row) = row {
                            let mut row = row.clone();
                            if let Some(provider) = &provider {
                                provider.apply_prepared(std::slice::from_mut(&mut row));
                            }
                            entries.push((position, entry(&source.id, &source.name, row, &[])));
                        } else {
                            notices.push(format!("{}: game is no longer indexed", game.name));
                        }
                    }
                }
                entries.sort_by_key(|(position, _)| *position);
                Ok(Page {
                    entries: entries.into_iter().map(|(_, entry)| entry).collect(),
                    data: json!({"notices":notices}),
                })
            }
            Operation::Scripts { folder } => {
                let browser = crate::scripts::Browser::open(&snapshot.menu_root).map_err(error)?;
                let directory = folder.as_deref().unwrap_or(browser.root());
                let entries = browser
                    .list(directory)
                    .map_err(error)?
                    .into_iter()
                    .map(|item| Entry {
                        target: (!item.is_directory).then(|| home::Target::Script {
                            path: item.path.clone(),
                        }),
                        folder: item.is_directory.then_some(item.path),
                        name: item.name,
                        row: None,
                        system: None,
                        system_name: None,
                    })
                    .collect();
                let trail: Vec<_> = directory
                    .ancestors()
                    .take_while(|path| path.starts_with(browser.root()))
                    .map(Path::to_path_buf)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect();
                Ok(Page {
                    entries,
                    data: json!({"root":directory==browser.root(),"script_trail":trail}),
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letter_jumps_use_first_position_in_the_complete_display_order() {
        // Folder/favourite ordering may start a second alphabetical run.
        let names = [
            "Zoo folder",
            "Arcade folder",
            "!Special",
            "1941",
            "alpha",
            "Apple",
            "Beta",
            "Éclair",
        ];
        assert_eq!(
            letter_offsets(names.into_iter()),
            json!([
                {"letter":"#","offset":2}, {"letter":"1","offset":3},
                {"letter":"A","offset":1}, {"letter":"B","offset":6},
                {"letter":"Z","offset":0}, {"letter":"É","offset":7}
            ])
        );
        assert_eq!(letter_offsets(std::iter::empty()), json!([]));
    }
}
