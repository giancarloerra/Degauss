//! Background loading of read-only Artwork Pack provider snapshots.
//!
//! Parsing Pack tables and indexing their JPEG filenames can be substantial on
//! MiSTer. The UI therefore requests immutable snapshots here and keeps drawing
//! and polling input while this worker reads them.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;
use std::thread::JoinHandle;

use crate::error::{DegaussError, Result};

#[derive(Debug, Clone)]
pub struct Request {
    pub system_id: String,
    pub system_name: String,
    pub docs_root: PathBuf,
    pub synopsis_language: Option<String>,
    pub cache_dir: PathBuf,
    /// Location discovery needs only a bounded provider-health check. It must
    /// not inspect or prepare a source cache for a location the user has not
    /// selected.
    pub validate_location_only: bool,
    /// A parsed snapshot may be reused, but only after its filesystem
    /// identity has been rechecked on this worker.
    pub cached_provider: Option<crate::artwork_pack::Provider>,
    /// Where the paths inside an `.mgl` point, for the Pack match.
    pub homes: Arc<crate::mgl::Homes>,
    /// Write the prepared state beside the cache once the cache validates:
    /// the one-time adoption of a system prepared before the state existed.
    /// Off for location discovery, which prepares nothing.
    pub write_state: bool,
    pub manual_matches: crate::artwork_pack::ManualMatches,
    /// An explicit picker operation, never part of startup or discovery.
    pub matching: Option<Matching>,
}

#[derive(Debug, Clone)]
pub enum Matching {
    Search(String),
    Save(Box<MatchSave>),
}

#[derive(Debug, Clone)]
pub struct MatchSave {
    pub launch: crate::browse::Launch,
    /// None restores the existing automatic matcher for this game.
    pub selected: Option<crate::artwork_pack::ManualMatch>,
    pub settings: crate::settings::Settings,
    pub settings_path: PathBuf,
}

#[derive(Debug)]
pub struct Snapshot {
    pub provider: crate::artwork_pack::Provider,
    /// The Pack directory/table fingerprint changed, so decoded artwork at
    /// otherwise identical paths must be invalidated for this source group.
    pub source_changed: bool,
    /// No cache, incomplete coverage, or a changed ROM requires the existing
    /// transactional source-cache worker before this system can open.
    pub cache_needs_recovery: bool,
    /// Rows the preparation left without Pack data, one count line per
    /// reason, prefixed with the system's name.
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Progress {
    pub current: String,
    pub system: usize,
    pub systems: usize,
}

#[derive(Debug)]
pub enum Event {
    Progress(Progress),
    MatchChoices {
        provider: Box<crate::artwork_pack::Provider>,
        choices: Vec<crate::artwork_pack::MatchChoice>,
    },
    MatchStaged {
        provider: Box<crate::artwork_pack::Provider>,
        prepared: crate::cache::PreparedCacheGroup,
        settings: Box<crate::settings::Settings>,
    },
    Loaded {
        snapshots: Vec<Snapshot>,
        progress: Progress,
    },
    Cancelled(Progress),
    Failed {
        error: DegaussError,
        progress: Progress,
    },
}

pub struct Job {
    events: Receiver<Event>,
    cancelled: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Job {
    pub fn try_recv(&mut self) -> Option<Event> {
        match self.events.try_recv() {
            Ok(event) => {
                if !matches!(event, Event::Progress(_)) {
                    if let Some(handle) = self.handle.take() {
                        let _ = handle.join();
                    }
                }
                Some(event)
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                if let Some(handle) = self.handle.take() {
                    let _ = handle.join();
                }
                None
            }
        }
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancel();
    }
}

pub fn start(requests: Vec<Request>) -> Result<Job> {
    if requests.is_empty() {
        return Err(DegaussError::unsupported(
            "Artwork Pack",
            "no provider snapshot was requested",
        ));
    }
    let (sender, events) = mpsc::sync_channel(16);
    let cancelled = Arc::new(AtomicBool::new(false));
    let worker_cancelled = Arc::clone(&cancelled);
    let systems = requests.len();
    let handle = std::thread::Builder::new()
        .name("degauss-artwork-provider".to_string())
        .spawn(move || {
            let fallback = Progress {
                systems,
                ..Progress::default()
            };
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run(requests, &sender, &worker_cancelled)
            }));
            if outcome.is_err() {
                let _ = sender.send(Event::Failed {
                    error: DegaussError::unsupported(
                        "Artwork Pack",
                        "the background provider reader stopped unexpectedly",
                    ),
                    progress: fallback,
                });
            }
        })
        .map_err(|error| {
            DegaussError::unsupported(
                "Artwork Pack",
                format!("could not start the background provider reader: {error}"),
            )
        })?;
    Ok(Job {
        events,
        cancelled,
        handle: Some(handle),
    })
}

fn run(
    requests: Vec<Request>,
    events: &std::sync::mpsc::SyncSender<Event>,
    cancelled: &AtomicBool,
) {
    let mut progress = Progress {
        systems: requests.len(),
        ..Progress::default()
    };
    let mut snapshots = Vec::with_capacity(requests.len());
    for (position, request) in requests.into_iter().enumerate() {
        if cancelled.load(Ordering::Relaxed) {
            let _ = events.send(Event::Cancelled(progress));
            return;
        }
        progress.current = request.system_name;
        progress.system = position + 1;
        let _ = events.try_send(Event::Progress(progress.clone()));
        let mut cached_provider = request.cached_provider;
        let source_is_current = cached_provider.as_ref().is_some_and(|provider| {
            provider.still_current(&request.docs_root, request.synopsis_language.as_deref())
        });
        let source_changed = !source_is_current;
        let can_reuse_catalogue = source_is_current
            && cached_provider
                .as_ref()
                .is_some_and(crate::artwork_pack::Provider::catalogue_available);
        let provider = if !can_reuse_catalogue {
            crate::artwork_pack::Provider::load_controlled(
                &request.system_id,
                &request.docs_root,
                request.synopsis_language.as_deref(),
                cancelled,
            )
        } else {
            cached_provider.take()
        };
        let Some(mut provider) = provider else {
            let _ = events.send(Event::Cancelled(progress));
            return;
        };
        provider.clear_prepared();
        if let Some(matching) = request.matching {
            let outcome = run_matching(
                provider,
                matching,
                &request.cache_dir,
                &request.homes,
                cancelled,
            );
            let event = match outcome {
                Ok(Some(event)) => event,
                Ok(None) => Event::Cancelled(progress),
                Err(error) => Event::Failed { error, progress },
            };
            let _ = events.send(event);
            return;
        }
        if request.validate_location_only {
            provider.discard_catalogue();
            snapshots.push(Snapshot {
                provider,
                source_changed,
                cache_needs_recovery: false,
                warnings: Vec::new(),
            });
            continue;
        }
        // Read with its marker: the state written below records the
        // rows the mapping was prepared from. A cache that is there and
        // cannot be read is not a missing one: taken as missing, it would
        // be recovered over, so it is a failure instead.
        let cached = match crate::cache::load_artwork_pack_data_marked(
            &request.cache_dir,
            &request.system_id,
        ) {
            Ok(cached) => cached,
            Err(error) => {
                let _ = events.send(Event::Failed { error, progress });
                return;
            }
        };
        let mut skipped = crate::artwork_pack::SkippedEntries::new();
        let (fingerprints, cache_needs_recovery) = match cached.as_ref() {
            Some(_) if !provider.health.usable() => {
                (crate::cache::ContentFingerprints::new(), false)
            }
            Some((cached, _)) => match provider.cached_fingerprints_are_current(
                &cached.cache,
                &cached.fingerprints,
                cached.fingerprints_complete,
                &request.homes,
                cancelled,
                &mut skipped,
            ) {
                Ok(Some(true)) => (cached.fingerprints.clone(), false),
                Ok(Some(false)) => (crate::cache::ContentFingerprints::new(), true),
                Ok(None) => {
                    let _ = events.send(Event::Cancelled(progress));
                    return;
                }
                Err(error) => {
                    let _ = events.send(Event::Failed { error, progress });
                    return;
                }
            },
            None => (crate::cache::ContentFingerprints::new(), true),
        };
        let mut warnings = Vec::new();
        if !cache_needs_recovery && provider.health.usable() {
            let Some((cached, marker)) = cached.as_ref() else {
                let _ = events.send(Event::Failed {
                    error: DegaussError::unsupported(
                        "Artwork Pack",
                        "provider validation finished without its source cache",
                    ),
                    progress,
                });
                return;
            };
            match provider.prepare_for_cache_with_matches(
                &cached.cache,
                &fingerprints,
                &request.homes,
                cancelled,
                &mut skipped,
                &request.manual_matches,
            ) {
                Ok(Some(_)) => {}
                Ok(None) => {
                    let _ = events.send(Event::Cancelled(progress));
                    return;
                }
                Err(error) => {
                    let _ = events.send(Event::Failed { error, progress });
                    return;
                }
            }
            warnings = crate::artwork_pack::skipped_summary(&progress.current, &skipped);
            if request.write_state {
                // The cache validated against the live files and the rows
                // are prepared: from here the system opens on this state
                // alone. A state that cannot be written is a failure the
                // user sees, not a silent return to validating every time.
                let state = crate::cache::PackSourceState {
                    accepted: Some(crate::cache::AcceptedSource {
                        docs_root: request.docs_root.to_string_lossy().into_owned(),
                        language: provider.synopsis_language().map(str::to_string),
                        signature: provider.snapshot().cloned(),
                        cache_marker: *marker,
                        health: provider.health,
                        diagnostics: provider.diagnostics.clone(),
                        skipped_entries: u32::try_from(skipped.len()).unwrap_or(u32::MAX),
                    }),
                    declined: None,
                };
                let Some(map) = provider.prepared_map() else {
                    let _ = events.send(Event::Failed {
                        error: DegaussError::unsupported(
                            "Artwork Pack",
                            "provider validation finished without a prepared map",
                        ),
                        progress,
                    });
                    return;
                };
                if let Err(error) = crate::cache::save_pack_state(
                    &request.cache_dir,
                    &request.system_id,
                    &state,
                    map,
                ) {
                    let _ = events.send(Event::Failed { error, progress });
                    return;
                }
                crate::note(&format!(
                    "artwork pack {}: state written",
                    request.system_id
                ));
            }
        }
        provider.discard_catalogue();
        snapshots.push(Snapshot {
            provider,
            source_changed,
            cache_needs_recovery,
            warnings,
        });
    }
    if cancelled.load(Ordering::Relaxed) {
        let _ = events.send(Event::Cancelled(progress));
    } else {
        let _ = events.send(Event::Loaded {
            snapshots,
            progress,
        });
    }
}

fn run_matching(
    mut provider: crate::artwork_pack::Provider,
    matching: Matching,
    cache_dir: &std::path::Path,
    homes: &crate::mgl::Homes,
    cancelled: &AtomicBool,
) -> Result<Option<Event>> {
    if !provider.health.usable() {
        return Err(DegaussError::unsupported(
            "Artwork Pack match",
            provider.status_line(),
        ));
    }
    let save = match matching {
        Matching::Search(query) => {
            return provider.match_choices(&query, cancelled).map(|choices| {
                choices.map(|choices| Event::MatchChoices {
                    provider: Box::new(provider),
                    choices,
                })
            });
        }
        Matching::Save(save) => save,
    };
    let id = provider.system_id.clone();
    let state = crate::cache::load_pack_source_state(cache_dir, &id)?.ok_or_else(|| {
        DegaussError::unsupported(
            "Artwork Pack match",
            "Reopen the system to prepare its Artwork Pack first.",
        )
    })?;
    let accepted = state.accepted.as_ref().ok_or_else(|| {
        DegaussError::unsupported(
            "Artwork Pack match",
            "Reopen the system to prepare its Artwork Pack first.",
        )
    })?;
    if !provider.configuration_matches(
        std::path::Path::new(&accepted.docs_root),
        accepted.language.as_deref(),
    ) || provider.snapshot() != accepted.signature.as_ref()
    {
        return Err(DegaussError::unsupported(
            "Artwork Pack match",
            "Artwork Pack changed. Reopen the system and update its prepared data before matching.",
        ));
    }
    let (cached, marker) = crate::cache::load_artwork_pack_data_marked(cache_dir, &id)?
        .ok_or_else(|| {
            DegaussError::unsupported(
                "Artwork Pack match",
                "The system cache is missing. Rebuild this system list first.",
            )
        })?;
    if marker != accepted.cache_marker {
        return Err(DegaussError::unsupported(
            "Artwork Pack match",
            "The system list changed. Reopen the system before matching.",
        ));
    }
    let mut map = crate::cache::load_pack_prepared_map(cache_dir, &id)?.ok_or_else(|| {
        DegaussError::unsupported(
            "Artwork Pack match",
            "The prepared artwork is missing. Reopen the system to prepare it first.",
        )
    })?;
    let key = crate::game_launch_cores::key(&save.launch);
    let presentation = if let Some(selected) = &save.selected {
        Some(provider.presentation_for_match(selected)?)
    } else {
        let mut fingerprints = cached.fingerprints;
        // Restore Automatic using the current ROM, not a possibly old CRC.
        if let Some((path, fingerprint)) =
            provider.fingerprint_for_launch(&save.launch, homes, cancelled, &mut |_| {})?
        {
            fingerprints.insert(path, fingerprint);
        }
        provider.presentation_for_launch_with_fingerprints(&save.launch, &fingerprints, homes)?
    };
    if cancelled.load(Ordering::Relaxed) {
        return Ok(None);
    }
    let targets: Vec<_> = cached
        .cache
        .folders
        .values()
        .flat_map(|folder| &folder.rows)
        .filter_map(|row| match &row.kind {
            crate::browse::Kind::Play(launch) if crate::game_launch_cores::key(launch) == key => {
                Some(launch.clone())
            }
            _ => None,
        })
        .collect();
    if targets.is_empty() {
        return Err(DegaussError::unsupported(
            "Artwork Pack match",
            "The game is no longer in this system's cache. Rebuild this system list.",
        ));
    }
    for launch in targets {
        if let Some(presentation) = &presentation {
            map.insert(launch, presentation.clone());
        } else {
            map.remove(&launch);
        }
    }
    let mut settings = save.settings;
    if let Some(selected) = save.selected {
        settings
            .artwork_pack_matches
            .entry(id.clone())
            .or_default()
            .insert(key, selected);
    } else if let Some(matches) = settings.artwork_pack_matches.get_mut(&id) {
        matches.remove(&key);
        if matches.is_empty() {
            settings.artwork_pack_matches.remove(&id);
        }
    }
    if !provider.still_current(&provider.docs_root, provider.synopsis_language()) {
        return Err(DegaussError::unsupported(
            "Artwork Pack match",
            "Artwork Pack changed while matching. Reopen the system before retrying.",
        ));
    }
    let prepared =
        crate::cache::prepare_pack_match(cache_dir, &id, &map, &save.settings_path, &settings)?;
    if cancelled.load(Ordering::Relaxed) {
        return Ok(None);
    }
    provider.set_prepared_map(map);
    provider.discard_catalogue();
    Ok(Some(Event::MatchStaged {
        provider: Box::new(provider),
        prepared,
        settings: Box::new(settings),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::Path;
    use std::time::Duration;

    use crate::browse::{Details, Kind, Launch, Row};

    fn temp(tag: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "degauss-provider-job-{tag}-{}-{:p}",
            std::process::id(),
            &tag
        ));
        std::fs::remove_dir_all(&path).ok();
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn terminal(job: &mut Job) -> Event {
        loop {
            let event = job
                .events
                .recv_timeout(Duration::from_secs(5))
                .expect("provider worker returned a terminal event");
            if !matches!(event, Event::Progress(_)) {
                if let Some(handle) = job.handle.take() {
                    handle.join().unwrap();
                }
                return event;
            }
        }
    }

    fn cache_with_rom(rom: &Path) -> crate::cache::SystemCache {
        crate::cache::SystemCache {
            format: 1,
            folders: BTreeMap::from([(
                "root".to_string(),
                crate::cache::Folder {
                    mtime: 0,
                    rows: vec![Row {
                        name: "Different Name".to_string(),
                        sort_key: "different name".to_string(),
                        kind: Kind::Play(Launch::File(rom.to_path_buf())),
                        cover: None,
                        genre: None,
                        favorite: false,
                        below: None,
                        details: Details::default(),
                    }],
                    games: 1,
                },
            )]),
        }
    }

    #[test]
    fn provider_snapshots_load_on_the_worker_and_keep_invalid_health_explicit() {
        let root = temp("loaded");
        let ready_docs = root.join("ready/docs");
        let art = ready_docs.join("SuperGrafx/Artwork");
        std::fs::create_dir_all(&art).unwrap();
        std::fs::write(
            art.join("manifest.tsv"),
            "#key\tstyle\tss_system_id\nKnown\tbox-2D\t105\n",
        )
        .unwrap();
        std::fs::write(
            art.join("index.tsv"),
            "#name\tcrc\tsize\tkey\nKnown\t\t\tKnown\n",
        )
        .unwrap();
        std::fs::write(
            art.join("gameinfo.tsv"),
            "#key\tname\tyear\tgenre\tdeveloper\tplayers\n",
        )
        .unwrap();
        std::fs::write(art.join("Known.jpg"), b"jpeg").unwrap();

        let mut job = start(vec![
            Request {
                system_id: "SuperGrafx".to_string(),
                system_name: "SuperGrafx".to_string(),
                docs_root: ready_docs,
                synopsis_language: Some("en".to_string()),
                cache_dir: root.join("cache"),
                validate_location_only: false,
                cached_provider: None,
                homes: Arc::new(crate::mgl::Homes::default()),
                write_state: false,
                manual_matches: Default::default(),
                matching: None,
            },
            Request {
                system_id: "NES".to_string(),
                system_name: "NES".to_string(),
                docs_root: root.join("missing/docs"),
                synopsis_language: Some("en".to_string()),
                cache_dir: root.join("cache"),
                validate_location_only: false,
                cached_provider: None,
                homes: Arc::new(crate::mgl::Homes::default()),
                write_state: false,
                manual_matches: Default::default(),
                matching: None,
            },
        ])
        .unwrap();
        let Event::Loaded {
            snapshots,
            progress,
        } = terminal(&mut job)
        else {
            panic!("provider worker did not return its snapshots");
        };
        assert_eq!(progress.system, 2);
        assert_eq!(snapshots.len(), 2);
        assert_eq!(
            snapshots[0].provider.health,
            crate::artwork_pack::ProviderHealth::Ready
        );
        assert_eq!(
            snapshots[1].provider.health,
            crate::artwork_pack::ProviderHealth::Unavailable
        );
        assert!(snapshots.iter().all(|snapshot| snapshot.source_changed));
        assert!(snapshots
            .iter()
            .all(|snapshot| snapshot.cache_needs_recovery));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn location_validation_requires_a_usable_manifest_but_no_source_cache() {
        let root = temp("location-validation");
        let ready_docs = root.join("ready/docs");
        let ready_art = ready_docs.join("SuperGrafx/Artwork");
        let invalid_docs = root.join("invalid/docs");
        let invalid_art = invalid_docs.join("SuperGrafx/Artwork");
        std::fs::create_dir_all(&ready_art).unwrap();
        std::fs::create_dir_all(&invalid_art).unwrap();
        std::fs::write(
            ready_art.join("manifest.tsv"),
            "#key\tstyle\tss_system_id\nKnown\tbox-2D\t105\n",
        )
        .unwrap();
        std::fs::write(ready_art.join("Known.jpg"), b"jpeg").unwrap();
        std::fs::write(invalid_art.join("Orphan.jpg"), b"jpeg").unwrap();

        let request = |docs_root: PathBuf| Request {
            system_id: "SuperGrafx".to_string(),
            system_name: docs_root.to_string_lossy().into_owned(),
            docs_root,
            synopsis_language: Some("en".to_string()),
            cache_dir: root.join("cache-that-does-not-exist"),
            validate_location_only: true,
            cached_provider: None,
            homes: Arc::new(crate::mgl::Homes::default()),
            write_state: false,
            manual_matches: Default::default(),
            matching: None,
        };
        let mut job = start(vec![request(ready_docs.clone()), request(invalid_docs)]).unwrap();
        let Event::Loaded { snapshots, .. } = terminal(&mut job) else {
            panic!("location validation did not return provider health");
        };

        assert_eq!(snapshots.len(), 2);
        assert!(snapshots[0].provider.health.usable());
        assert_eq!(
            snapshots[1].provider.health,
            crate::artwork_pack::ProviderHealth::Invalid
        );
        assert!(snapshots
            .iter()
            .all(|snapshot| !snapshot.cache_needs_recovery));
        assert!(!root.join("cache-that-does-not-exist").exists());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn an_unusable_provider_does_not_walk_an_existing_source_cache() {
        let root = temp("unusable-with-cache");
        let cache_dir = root.join("cache");
        crate::cache::install_transactional(
            &cache_dir,
            crate::cache::CacheKind::ArtworkPack,
            &[crate::cache::StagedSystemCache {
                id: "NES".to_string(),
                cache: cache_with_rom(&root.join("game.nes")),
                fingerprints: crate::cache::ContentFingerprints::new(),
                fingerprints_complete: true,
            }],
        )
        .unwrap();

        let mut job = start(vec![Request {
            system_id: "NES".to_string(),
            system_name: "NES".to_string(),
            docs_root: root.join("missing/docs"),
            synopsis_language: None,
            cache_dir,
            validate_location_only: false,
            cached_provider: None,
            homes: Arc::new(crate::mgl::Homes::default()),
            write_state: false,
            manual_matches: Default::default(),
            matching: None,
        }])
        .unwrap();
        let Event::Loaded { mut snapshots, .. } = terminal(&mut job) else {
            panic!("an unavailable provider did not return its health snapshot");
        };
        let snapshot = snapshots.remove(0);
        assert_eq!(
            snapshot.provider.health,
            crate::artwork_pack::ProviderHealth::Unavailable
        );
        assert!(!snapshot.cache_needs_recovery);
        assert!(!snapshot.provider.prepared_available());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_pre_cancelled_provider_batch_reads_nothing() {
        let root = temp("cancelled");
        let (sender, receiver) = mpsc::sync_channel(2);
        let cancelled = AtomicBool::new(true);
        run(
            vec![Request {
                system_id: "SuperGrafx".to_string(),
                system_name: "SuperGrafx".to_string(),
                docs_root: root.clone(),
                synopsis_language: None,
                cache_dir: root.join("cache"),
                validate_location_only: false,
                cached_provider: None,
                homes: Arc::new(crate::mgl::Homes::default()),
                write_state: false,
                manual_matches: Default::default(),
                matching: None,
            }],
            &sender,
            &cancelled,
        );
        assert!(matches!(receiver.recv().unwrap(), Event::Cancelled(_)));
        assert!(receiver.try_recv().is_err());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn worker_validates_persisted_crc_snapshot_before_the_ui_can_use_it() {
        let root = temp("validated-fingerprints");
        let docs = root.join("docs");
        let art = docs.join("SuperGrafx/Artwork");
        let games = root.join("games");
        std::fs::create_dir_all(&art).unwrap();
        std::fs::create_dir_all(&games).unwrap();
        let rom = games.join("Different Name.pce");
        let bytes = vec![0x5a; 512 * 1024];
        std::fs::write(&rom, &bytes).unwrap();
        let crc = crc32fast::hash(&bytes);
        std::fs::write(
            art.join("manifest.tsv"),
            "#key\tstyle\tss_system_id\nKnown\tbox-2D\t105\n",
        )
        .unwrap();
        std::fs::write(
            art.join("index.tsv"),
            format!(
                "#name\tcrc\tsize\tkey\nIndexed source\t{crc:08x}\t{}\tKnown\n",
                bytes.len()
            ),
        )
        .unwrap();
        std::fs::write(
            art.join("gameinfo.tsv"),
            "#key\tname\tyear\tgenre\tdeveloper\tplayers\n",
        )
        .unwrap();
        std::fs::write(art.join("Known.jpg"), b"jpeg").unwrap();

        let provider = crate::artwork_pack::Provider::load("SuperGrafx", &docs, None);
        let cancelled = AtomicBool::new(false);
        let fingerprint = provider
            .fingerprint_for_launch(
                &Launch::File(rom.clone()),
                &crate::mgl::Homes::default(),
                &cancelled,
                &mut |_| {},
            )
            .unwrap()
            .expect("CRC-only match requires a loose-file fingerprint");
        let fingerprints = crate::cache::ContentFingerprints::from([fingerprint]);
        let cache = cache_with_rom(&rom);
        let cache_dir = root.join("cache");
        crate::cache::install_transactional(
            &cache_dir,
            crate::cache::CacheKind::ArtworkPack,
            &[crate::cache::StagedSystemCache {
                id: "SuperGrafx".to_string(),
                cache,
                fingerprints: fingerprints.clone(),
                fingerprints_complete: true,
            }],
        )
        .unwrap();

        let request = |cached_provider| Request {
            system_id: "SuperGrafx".to_string(),
            system_name: "SuperGrafx".to_string(),
            docs_root: docs.clone(),
            synopsis_language: None,
            cache_dir: cache_dir.clone(),
            validate_location_only: false,
            cached_provider,
            homes: Arc::new(crate::mgl::Homes::default()),
            write_state: false,
            manual_matches: Default::default(),
            matching: None,
        };
        let mut job = start(vec![request(None)]).unwrap();
        let Event::Loaded { mut snapshots, .. } = terminal(&mut job) else {
            panic!("current cache did not produce a provider snapshot");
        };
        let current = snapshots.remove(0);
        assert!(current.source_changed);
        assert!(!current.cache_needs_recovery);
        assert!(
            !current.provider.catalogue_available(),
            "a worker result retains only prepared presentation"
        );
        let mut rows = cache_with_rom(&rom)
            .folders
            .into_values()
            .flat_map(|folder| folder.rows)
            .collect::<Vec<_>>();
        assert_eq!(current.provider.apply_prepared(&mut rows), 1);
        assert_eq!(
            rows[0].cover.as_deref(),
            Some(art.join("Known.jpg").as_path())
        );

        std::fs::write(&rom, [bytes.as_slice(), b"changed"].concat()).unwrap();
        let mut job = start(vec![request(Some(current.provider))]).unwrap();
        let Event::Loaded { mut snapshots, .. } = terminal(&mut job) else {
            panic!("changed ROM did not produce a provider snapshot");
        };
        let changed = snapshots.remove(0);
        assert!(
            !changed.source_changed,
            "a ROM change invalidates only its prepared cache, not unchanged Pack files"
        );
        assert!(
            !changed.provider.catalogue_available(),
            "reparsing an unchanged compact snapshot must compact it again"
        );
        assert!(changed.cache_needs_recovery);
        let mut rows = cache_with_rom(&rom)
            .folders
            .into_values()
            .flat_map(|folder| folder.rows)
            .collect::<Vec<_>>();
        assert_eq!(
            changed.provider.apply_prepared(&mut rows),
            0,
            "a stale CRC must never cross the worker/UI boundary"
        );
        std::fs::remove_dir_all(root).ok();
    }

    /// A complete cache from before the state existed, with a Pack whose
    /// rows are matched by name and a broken descriptor beside them.
    fn legacy_arcade(tag: &str) -> (PathBuf, PathBuf, PathBuf) {
        let root = temp(tag);
        let docs = root.join("docs");
        let art = docs.join("Arcade/Artwork");
        let games = root.join("_Arcade");
        std::fs::create_dir_all(&art).unwrap();
        std::fs::create_dir_all(&games).unwrap();
        std::fs::write(
            art.join("manifest.tsv"),
            "#key\tstyle\tss_system_id\nhealthy\tbox-2D\t75\n",
        )
        .unwrap();
        std::fs::write(
            art.join("index.tsv"),
            "#name\tcrc\tsize\tkey\nhealthy\t\t\thealthy\n",
        )
        .unwrap();
        std::fs::write(
            art.join("gameinfo.tsv"),
            "#key\tname\tyear\tgenre\tdeveloper\tplayers\nhealthy\tPack Healthy\t1990\tShooter\tStudio\t2\n",
        )
        .unwrap();
        std::fs::write(art.join("healthy.jpg"), b"jpeg").unwrap();
        let healthy = games.join("Healthy.mra");
        std::fs::write(
            &healthy,
            "<misterromdescription><setname>healthy</setname></misterromdescription>",
        )
        .unwrap();
        let broken = games.join("Broken.mra");
        std::fs::write(&broken, "<misterromdescription><rom></wrong>").unwrap();
        let row = |path: &Path| Row {
            name: path.file_stem().unwrap().to_string_lossy().into_owned(),
            sort_key: String::new(),
            kind: Kind::Play(Launch::File(path.to_path_buf())),
            cover: None,
            genre: None,
            favorite: false,
            below: None,
            details: Details::default(),
        };
        let cache_dir = root.join("cache");
        crate::cache::install_transactional(
            &cache_dir,
            crate::cache::CacheKind::ArtworkPack,
            &[crate::cache::StagedSystemCache {
                id: "Arcade".to_string(),
                cache: crate::cache::SystemCache {
                    format: 1,
                    folders: BTreeMap::from([(
                        "root".to_string(),
                        crate::cache::Folder {
                            mtime: 0,
                            rows: vec![row(&healthy), row(&broken)],
                            games: 2,
                        },
                    )]),
                },
                fingerprints: crate::cache::ContentFingerprints::new(),
                fingerprints_complete: true,
            }],
        )
        .unwrap();
        (root, docs, cache_dir)
    }

    /// A cache that is there and cannot be read must not pass for a
    /// missing one: taken as missing, the worker would report the rows
    /// as needing recovery and the previous complete result would be
    /// rebuilt over instead of kept.
    #[test]
    fn an_unreadable_source_cache_is_a_failure_not_a_recovery() {
        let (root, docs, cache_dir) = legacy_arcade("unreadable-cache");
        let cache = crate::cache::artwork_pack_system_path(&cache_dir, "Arcade");
        std::fs::remove_file(&cache).unwrap();
        std::fs::create_dir(&cache).unwrap();
        let mut job = start(vec![adoption(&docs, &cache_dir, true)]).unwrap();
        let Event::Failed { error, progress } = terminal(&mut job) else {
            panic!("a cache that cannot be read must be a visible failure");
        };
        assert!(
            error.to_string().contains("reading the Artwork Pack cache"),
            "{error}"
        );
        assert_eq!(progress.current, "Arcade");
        assert!(
            crate::cache::load_pack_source_state(&cache_dir, "Arcade")
                .unwrap()
                .is_none(),
            "nothing is decided on a cache that could not be read"
        );
        assert!(cache.is_dir(), "the unreadable cache is left as it is");
        std::fs::remove_dir_all(root).ok();
    }

    fn adoption(docs: &Path, cache_dir: &Path, write_state: bool) -> Request {
        Request {
            system_id: "Arcade".to_string(),
            system_name: "Arcade".to_string(),
            docs_root: docs.to_path_buf(),
            synopsis_language: Some("EN".to_string()),
            cache_dir: cache_dir.to_path_buf(),
            validate_location_only: false,
            cached_provider: None,
            homes: Arc::new(crate::mgl::Homes::default()),
            write_state,
            manual_matches: Default::default(),
            matching: None,
        }
    }

    #[test]
    fn manual_match_worker_stages_persists_restores_and_preserves_pack_and_source_cache() {
        let (root, docs, cache_dir) = legacy_arcade("manual-worker");
        let art = docs.join("Arcade/Artwork");
        std::fs::write(
            art.join("manifest.tsv"),
            "#key\tstyle\tss_system_id\nhealthy\tbox-2D\t75\nsecond\tbox-2D\t75\n",
        )
        .unwrap();
        std::fs::write(
            art.join("index.tsv"),
            "#name\tcrc\tsize\tkey\nhealthy\t\t\thealthy\nSecond Stable Name\t\t\tsecond\n",
        )
        .unwrap();
        std::fs::write(art.join("gameinfo.tsv"), "#key\tname\tyear\tgenre\tdeveloper\tplayers\nhealthy\tPack Healthy\t1990\tShooter\tStudio\t2\nsecond\tManual title\t1992\tPuzzle\tOther Studio\t1\n").unwrap();
        std::fs::write(art.join("second.jpg"), b"second jpeg").unwrap();
        let settings_path = root.join("settings.toml");
        let mut settings = crate::settings::Settings::default();
        settings.launch_cores.insert("NES".into(), "nes".into());
        settings.save(&settings_path).unwrap();
        let pack_before: BTreeMap<_, _> = std::fs::read_dir(&art)
            .unwrap()
            .map(|entry| {
                let path = entry.unwrap().path();
                let bytes = std::fs::read(&path).unwrap();
                (path, bytes)
            })
            .collect();
        let mut job = start(vec![adoption(&docs, &cache_dir, true)]).unwrap();
        assert!(matches!(terminal(&mut job), Event::Loaded { .. }));
        let files = [
            crate::cache::artwork_pack_system_path(&cache_dir, "Arcade"),
            cache_dir.join("artwork-pack/Arcade.source.bin"),
            cache_dir.join("artwork-pack/Arcade.prepared.bin"),
            settings_path.clone(),
        ];
        let before: Vec<_> = files
            .iter()
            .map(|path| std::fs::read(path).unwrap())
            .collect();
        let launch = Launch::File(root.join("_Arcade/Healthy.mra"));
        let selected = crate::artwork_pack::ManualMatch {
            folder: "Arcade".into(),
            name: "second stable name".into(),
        };
        let request = |selected, settings, settings_path| {
            let mut request = adoption(&docs, &cache_dir, false);
            request.matching = Some(Matching::Save(Box::new(MatchSave {
                launch: launch.clone(),
                selected,
                settings,
                settings_path,
            })));
            request
        };
        let mut search = adoption(&docs, &cache_dir, false);
        search.matching = Some(Matching::Search("Second".into()));
        let mut job = start(vec![search]).unwrap();
        let Event::MatchChoices { choices, .. } = terminal(&mut job) else {
            panic!("search did not return choices");
        };
        assert_eq!(choices[0].reference, selected);
        assert_eq!(
            before,
            files
                .iter()
                .map(|path| std::fs::read(path).unwrap())
                .collect::<Vec<_>>(),
            "search changes no files"
        );
        let mut job = start(vec![request(
            Some(selected.clone()),
            settings.clone(),
            settings_path.clone(),
        )])
        .unwrap();
        let Event::MatchStaged { prepared, .. } = terminal(&mut job) else {
            panic!("match was not staged");
        };
        assert_eq!(
            before,
            files
                .iter()
                .map(|path| std::fs::read(path).unwrap())
                .collect::<Vec<_>>()
        );
        drop(prepared); // Late cancellation never publishes the worker's staged files.
        assert_eq!(
            before,
            files
                .iter()
                .map(|path| std::fs::read(path).unwrap())
                .collect::<Vec<_>>()
        );

        let mut job = start(vec![request(
            Some(selected.clone()),
            settings.clone(),
            root.join("missing/settings.toml"),
        )])
        .unwrap();
        let Event::Failed { error, .. } = terminal(&mut job) else {
            panic!("a settings write failure was concealed");
        };
        assert!(
            error.to_string().contains("creating staged cache"),
            "{error}"
        );
        assert_eq!(
            before,
            files
                .iter()
                .map(|path| std::fs::read(path).unwrap())
                .collect::<Vec<_>>(),
            "failed save preserves installed files"
        );

        let mut job = start(vec![request(
            Some(selected.clone()),
            settings,
            settings_path.clone(),
        )])
        .unwrap();
        let Event::MatchStaged {
            prepared,
            provider,
            settings: updated,
        } = terminal(&mut job)
        else {
            panic!("retry was not staged");
        };
        assert!(
            !provider.catalogue_available(),
            "only compact presentation returns after saving"
        );
        assert_eq!(
            provider.prepared_map().unwrap()[&launch].name.as_deref(),
            Some("Manual title")
        );
        prepared.install().unwrap();
        assert_eq!(
            std::fs::read(&files[0]).unwrap(),
            before[0],
            "no full reindex for a manual match"
        );
        let persisted = crate::settings::Settings::load(&settings_path).unwrap();
        assert_eq!(persisted.artwork_pack_matches, updated.artwork_pack_matches);
        assert_eq!(persisted.launch_cores["NES"], "nes");
        assert_eq!(
            persisted.artwork_pack_matches["Arcade"][&crate::game_launch_cores::key(&launch)],
            selected
        );
        assert_eq!(
            crate::cache::load_pack_prepared_map(&cache_dir, "Arcade")
                .unwrap()
                .unwrap()[&launch]
                .cover,
            Some(art.join("second.jpg"))
        );
        let mut reopen = adoption(&docs, &cache_dir, true);
        reopen.manual_matches = persisted.artwork_pack_matches["Arcade"].clone();
        let mut job = start(vec![reopen]).unwrap();
        let Event::Loaded { snapshots, .. } = terminal(&mut job) else {
            panic!("saved choice could not be reapplied");
        };
        assert_eq!(
            snapshots[0].provider.prepared_map().unwrap()[&launch]
                .name
                .as_deref(),
            Some("Manual title")
        );
        let mut job = start(vec![request(None, persisted, settings_path.clone())]).unwrap();
        let Event::MatchStaged { prepared, .. } = terminal(&mut job) else {
            panic!("Automatic could not be restored");
        };
        prepared.install().unwrap();
        assert!(crate::settings::Settings::load(&settings_path)
            .unwrap()
            .artwork_pack_matches
            .is_empty());
        assert_eq!(
            crate::cache::load_pack_prepared_map(&cache_dir, "Arcade")
                .unwrap()
                .unwrap()[&launch]
                .name
                .as_deref(),
            Some("Pack Healthy")
        );
        for (path, bytes) in pack_before {
            assert_eq!(std::fs::read(path).unwrap(), bytes, "the Pack is read-only");
        }
        assert_eq!(
            std::fs::read_dir(&art).unwrap().count(),
            5,
            "no image copies"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn manual_match_refuses_unprepared_changed_or_corrupt_data_and_honours_cancellation() {
        let (root, docs, cache_dir) = legacy_arcade("manual-worker-failures");
        let request = || {
            let mut request = adoption(&docs, &cache_dir, false);
            request.matching = Some(Matching::Save(Box::new(MatchSave {
                launch: Launch::File(root.join("_Arcade/Healthy.mra")),
                selected: Some(crate::artwork_pack::ManualMatch {
                    folder: "Arcade".into(),
                    name: "healthy".into(),
                }),
                settings: Default::default(),
                settings_path: root.join("settings.toml"),
            })));
            request
        };
        let mut job = start(vec![request()]).unwrap();
        let Event::Failed { error, .. } = terminal(&mut job) else {
            panic!("unprepared source accepted");
        };
        assert!(
            error.to_string().contains("prepare its Artwork Pack"),
            "{error}"
        );
        let mut job = start(vec![adoption(&docs, &cache_dir, true)]).unwrap();
        assert!(matches!(terminal(&mut job), Event::Loaded { .. }));
        let provider = crate::artwork_pack::Provider::load("Arcade", &docs, Some("EN"));
        assert!(run_matching(
            provider.clone(),
            request().matching.unwrap(),
            &cache_dir,
            &crate::mgl::Homes::default(),
            &AtomicBool::new(true)
        )
        .unwrap()
        .is_none());
        assert!(!root.join("settings.toml").exists());
        let prepared = cache_dir.join("artwork-pack/Arcade.prepared.bin");
        let bytes = std::fs::read(&prepared).unwrap();
        std::fs::write(&prepared, b"broken prepared data").unwrap();
        let mut job = start(vec![request()]).unwrap();
        assert!(matches!(terminal(&mut job), Event::Failed { .. }));
        assert_eq!(std::fs::read(&prepared).unwrap(), b"broken prepared data");
        std::fs::write(&prepared, bytes).unwrap();
        std::fs::write(docs.join("Arcade/Artwork/gameinfo.tsv"), "#key\tname\tyear\tgenre\tdeveloper\tplayers\nhealthy\tChanged Title\t1990\tShooter\tStudio\t2\n").unwrap();
        let mut job = start(vec![request()]).unwrap();
        let Event::Failed { error, .. } = terminal(&mut job) else {
            panic!("changed Pack accepted without preparation");
        };
        assert!(
            error.to_string().contains("Artwork Pack changed"),
            "{error}"
        );
        assert!(!root.join("settings.toml").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    /// The one-time adoption of a cache from before the state existed:
    /// once the cache validates against the live files and the rows are
    /// prepared, the state is written beside it, with the broken
    /// descriptor counted and reported and the healthy one matched. A
    /// state that cannot be written is a failure the user sees; a
    /// location check writes nothing.
    #[cfg(unix)]
    #[test]
    fn adoption_writes_the_state_after_validation_and_reports_a_broken_descriptor() {
        use std::os::unix::fs::PermissionsExt;
        let (root, docs, cache_dir) = legacy_arcade("adoption");
        let mut job = start(vec![adoption(&docs, &cache_dir, true)]).unwrap();
        let Event::Loaded { mut snapshots, .. } = terminal(&mut job) else {
            panic!("the adoption did not return its snapshot");
        };
        let snapshot = snapshots.remove(0);
        assert!(!snapshot.cache_needs_recovery);
        assert_eq!(
            snapshot.warnings,
            ["Arcade: 1 game left without Pack data: malformed descriptor"]
        );
        let state = crate::cache::load_pack_source_state(&cache_dir, "Arcade")
            .unwrap()
            .expect("the validated adoption is written down");
        let accepted = state.accepted.unwrap();
        assert_eq!(accepted.docs_root, docs.to_str().unwrap());
        assert_eq!(accepted.language.as_deref(), Some("en"));
        assert_eq!(accepted.skipped_entries, 1);
        assert_eq!(
            accepted.cache_marker,
            crate::cache::load_artwork_pack_data_marked(&cache_dir, "Arcade")
                .unwrap()
                .unwrap()
                .1
        );
        assert_eq!(accepted.health, crate::artwork_pack::ProviderHealth::Ready);
        let prepared = crate::cache::load_pack_prepared_map(&cache_dir, "Arcade")
            .unwrap()
            .unwrap();
        assert_eq!(prepared.len(), 1);
        assert_eq!(
            prepared.values().next().unwrap().name.as_deref(),
            Some("Pack Healthy")
        );
        assert_eq!(snapshot.provider.prepared_map().unwrap().len(), 1);

        let store = cache_dir.join("artwork-pack");
        for file in ["Arcade.source.bin", "Arcade.prepared.bin"] {
            std::fs::remove_file(store.join(file)).unwrap();
        }
        let mode = std::fs::metadata(&store).unwrap().permissions().mode();
        std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o555)).unwrap();
        let can_write = std::fs::write(store.join("probe"), b"").is_ok();
        let mut job = start(vec![adoption(&docs, &cache_dir, true)]).unwrap();
        let event = terminal(&mut job);
        std::fs::set_permissions(&store, std::fs::Permissions::from_mode(mode)).unwrap();
        if can_write {
            // Root writes through the mode; the failure cannot be produced.
            std::fs::remove_file(store.join("probe")).unwrap();
            assert!(matches!(event, Event::Loaded { .. }));
        } else {
            let Event::Failed { error, .. } = event else {
                panic!("a state that cannot be written must be a visible failure");
            };
            assert!(error.to_string().contains("writing the cache"), "{error}");
            assert!(
                crate::cache::load_pack_source_state(&cache_dir, "Arcade")
                    .unwrap()
                    .is_none(),
                "no decision is written without its rows"
            );
        }

        let mut job = start(vec![Request {
            validate_location_only: true,
            ..adoption(&docs, &cache_dir, false)
        }])
        .unwrap();
        assert!(matches!(terminal(&mut job), Event::Loaded { .. }));
        assert!(
            crate::cache::load_pack_source_state(&cache_dir, "Arcade")
                .unwrap()
                .is_none(),
            "a location check decides nothing"
        );
        std::fs::remove_dir_all(root).ok();
    }
}
