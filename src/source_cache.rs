//! Background construction of one newly selected game-data source.
//!
//! The worker owns immutable configuration and writes complete replacement
//! files beside the live cache. It never renames a live cache or writes
//! settings. The UI thread performs only the short final rename transaction
//! before committing the source preference.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError};
use std::sync::Arc;
use std::thread::JoinHandle;

use crate::browse::{DisplayNames, Library};
use crate::cache::{CacheKind, PreparedCacheGroup, StagedSystemCache};
use crate::config::SystemConfig;
use crate::error::{DegaussError, Result};

const EVENT_QUEUE: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Gamelist,
    ArtworkPack { docs_root: PathBuf },
}

#[derive(Clone)]
pub struct System {
    pub id: String,
    pub name: String,
    pub config: SystemConfig,
}

pub struct Request {
    pub target: Target,
    pub systems: Vec<System>,
    pub names: DisplayNames,
    pub synopsis_language: Option<String>,
    pub cache_dir: PathBuf,
    /// A new source choice must still be valid when the worker starts. A
    /// recovery for an already-selected Pack deliberately keeps its invalid
    /// provider so source-neutral rows can be rebuilt and the real health
    /// error can be shown without falling back to gamelist presentation.
    pub require_usable_provider: bool,
    /// Where the paths inside an `.mgl` point, for the Pack match.
    pub homes: Arc<crate::mgl::Homes>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Progress {
    pub current: String,
    pub system: usize,
    pub systems: usize,
    pub folders: usize,
    pub games: usize,
    pub hashed_files: usize,
    pub hashed_bytes: u64,
}

#[derive(Debug)]
pub enum Event {
    Progress(Progress),
    Staged {
        target: Target,
        prepared: PreparedCacheGroup,
        providers: Vec<crate::artwork_pack::Provider>,
        progress: Progress,
        /// Rows the preparation left without Pack data because their own
        /// descriptor could not be read: one count line per reason,
        /// prefixed with the system's name and without a path. The group
        /// still stages: one broken descriptor is not a reason to fail
        /// every row in it.
        warnings: Vec<String>,
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

pub fn start(request: Request) -> Result<Job> {
    if request.systems.is_empty() {
        return Err(DegaussError::unsupported(
            "game data source",
            "no installed system belongs to this source group",
        ));
    }
    let (sender, events) = mpsc::sync_channel(EVENT_QUEUE);
    let cancelled = Arc::new(AtomicBool::new(false));
    let worker_cancelled = Arc::clone(&cancelled);
    let handle = std::thread::Builder::new()
        .name("degauss-source-cache".to_string())
        .spawn(move || {
            let fallback = Progress {
                systems: request.systems.len(),
                ..Progress::default()
            };
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run(request, &sender, &worker_cancelled)
            }));
            if outcome.is_err() {
                let _ = sender.send(Event::Failed {
                    error: DegaussError::unsupported(
                        "game data source",
                        "the background cache worker stopped unexpectedly",
                    ),
                    progress: fallback,
                });
            }
        })
        .map_err(|error| {
            DegaussError::unsupported(
                "game data source",
                format!("could not start the background cache worker: {error}"),
            )
        })?;
    Ok(Job {
        events,
        cancelled,
        handle: Some(handle),
    })
}

fn run(request: Request, events: &SyncSender<Event>, cancelled: &Arc<AtomicBool>) {
    let mut progress = Progress {
        systems: request.systems.len(),
        ..Progress::default()
    };
    let mut caches = Vec::new();
    let mut providers = Vec::new();
    let mut warnings = Vec::new();
    // Per prepared system, the rows its preparation left without Pack
    // data, for the state written beside its cache.
    let mut skipped_counts = Vec::new();

    for (position, system) in request.systems.into_iter().enumerate() {
        if cancelled.load(Ordering::Relaxed) {
            let _ = events.send(Event::Cancelled(progress));
            return;
        }
        progress.system = position + 1;
        progress.current = system.name.clone();
        let _ = events.send(Event::Progress(progress.clone()));

        let mut provider = if let Target::ArtworkPack { docs_root } = &request.target {
            let Some(provider) = crate::artwork_pack::Provider::load_controlled(
                &system.id,
                docs_root,
                request.synopsis_language.as_deref(),
                cancelled,
            ) else {
                let _ = events.send(Event::Cancelled(progress));
                return;
            };
            if !provider.health.usable() {
                // A new choice needs a usable Pack. So does a recovery that
                // would replace a complete result: a Pack root that cannot
                // be read is a failure of the whole system, and the rows
                // prepared while it could be are kept, not replaced by
                // rows with nothing in them. Only a system with no complete
                // cache to keep is staged with its ordinary rows, so it can
                // open and show the real health error.
                let complete_before =
                    crate::cache::load_artwork_pack_data(&request.cache_dir, &system.id)
                        .is_some_and(|data| data.fingerprints_complete);
                if request.require_usable_provider || complete_before {
                    let _ = events.send(Event::Failed {
                        error: DegaussError::unsupported("Artwork Pack", provider.status_line()),
                        progress,
                    });
                    return;
                }
            }
            Some(provider)
        } else {
            None
        };

        let library = match request.target {
            Target::Gamelist => Library::open_with_names(&system.config, request.names.clone()),
            Target::ArtworkPack { .. } => {
                Library::open_source_neutral(&system.config, request.names.clone())
            }
        };
        let library = match library {
            Ok(library) => library,
            Err(error) => {
                let _ = events.send(Event::Failed { error, progress });
                return;
            }
        };
        let base_folders = progress.folders;
        let base_games = progress.games;
        let mut report = |folders: usize, games: usize| {
            progress.folders = base_folders + folders;
            progress.games = base_games + games;
            let _ = events.try_send(Event::Progress(progress.clone()));
        };
        let cache = match crate::cache::build_system_controlled(&library, cancelled, &mut report) {
            Ok(Some(cache)) => cache,
            Ok(None) => {
                let _ = events.send(Event::Cancelled(progress));
                return;
            }
            Err(error) => {
                let _ = events.send(Event::Failed { error, progress });
                return;
            }
        };
        let mut skipped = crate::artwork_pack::SkippedEntries::new();
        let fingerprints = if let Some(provider) = provider
            .as_ref()
            .filter(|provider| provider.health.usable())
        {
            let base_hashed_files = progress.hashed_files;
            let base_hashed_bytes = progress.hashed_bytes;
            match provider.fingerprints_for_cache(
                &cache,
                &request.homes,
                cancelled,
                &mut |files, bytes| {
                    progress.hashed_files = base_hashed_files.saturating_add(files);
                    progress.hashed_bytes = base_hashed_bytes.saturating_add(bytes);
                    let _ = events.try_send(Event::Progress(progress.clone()));
                },
                &mut skipped,
            ) {
                Ok(Some(fingerprints)) => fingerprints,
                Ok(None) => {
                    let _ = events.send(Event::Cancelled(progress));
                    return;
                }
                Err(error) => {
                    let _ = events.send(Event::Failed { error, progress });
                    return;
                }
            }
        } else {
            crate::cache::ContentFingerprints::new()
        };
        let fingerprints_complete = provider
            .as_ref()
            .is_some_and(|provider| provider.health.usable());
        if let Some(provider) = provider.as_mut() {
            match provider.prepare_for_cache(
                &cache,
                &fingerprints,
                &request.homes,
                cancelled,
                &mut skipped,
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
            warnings.extend(crate::artwork_pack::skipped_summary(&system.name, &skipped));
        }
        if let Some(mut provider) = provider {
            provider.discard_catalogue();
            providers.push(provider);
            skipped_counts.push(skipped.len());
        }
        caches.push(StagedSystemCache {
            id: system.id,
            cache,
            fingerprints,
            fingerprints_complete,
        });
    }

    let kind = match request.target {
        Target::Gamelist => CacheKind::Gamelist,
        Target::ArtworkPack { .. } => CacheKind::ArtworkPack,
    };
    let staged = crate::cache::stage_transactional(&request.cache_dir, kind, caches, cancelled)
        .and_then(|prepared| match prepared {
            Some(prepared) => stage_pack_state(
                prepared,
                &request.cache_dir,
                &request.target,
                &providers,
                &skipped_counts,
            )
            .map(Some),
            None => Ok(None),
        });
    match staged {
        Ok(Some(prepared)) if !cancelled.load(Ordering::Relaxed) => {
            let _ = events.send(Event::Staged {
                target: request.target,
                prepared,
                providers,
                progress,
                warnings,
            });
        }
        Ok(Some(_)) | Ok(None) => {
            let _ = events.send(Event::Cancelled(progress));
        }
        Err(error) => {
            let _ = events.send(Event::Failed { error, progress });
        }
    }
}

/// The state written beside each prepared system's rows, in the same
/// transaction: what the rows were prepared against and the presentation
/// they were given. Every Pack switch and every recovery of a usable Pack
/// leaves it behind, so the next entry into the system needs no worker. A
/// Gamelist target has nothing to write.
fn stage_pack_state(
    mut prepared: PreparedCacheGroup,
    cache_dir: &std::path::Path,
    target: &Target,
    providers: &[crate::artwork_pack::Provider],
    skipped_counts: &[usize],
) -> Result<PreparedCacheGroup> {
    let Target::ArtworkPack { docs_root } = target else {
        return Ok(prepared);
    };
    let markers = prepared.markers().to_vec();
    for ((provider, marker), skipped) in providers.iter().zip(markers).zip(skipped_counts) {
        // An unusable Pack prepares nothing worth remembering: its rows are
        // staged so the system can open and say so, and the next entry
        // reads the Pack again to see whether it has been repaired.
        if !provider.health.usable() {
            continue;
        }
        let state = crate::cache::PackSourceState {
            accepted: Some(crate::cache::AcceptedSource {
                docs_root: docs_root.to_string_lossy().into_owned(),
                language: provider.synopsis_language().map(str::to_string),
                signature: provider.snapshot().cloned(),
                cache_marker: marker,
                health: provider.health,
                diagnostics: provider.diagnostics.clone(),
                skipped_entries: u32::try_from(*skipped).unwrap_or(u32::MAX),
            }),
            declined: None,
        };
        prepared = prepared.with_pack_state(
            cache_dir,
            &provider.system_id,
            &crate::cache::encode_pack_source(&state)?,
            &crate::cache::encode_pack_prepared(&provider.prepared_pairs())?,
        )?;
    }
    Ok(prepared)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn temp(tag: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "degauss-source-cache-{tag}-{}-{:p}",
            std::process::id(),
            &tag
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn config(path: &std::path::Path, extension: &str) -> SystemConfig {
        SystemConfig {
            preserve_rbf_stem: false,
            name: "Test".to_string(),
            path: path.to_string_lossy().into_owned(),
            extensions: vec![extension.to_string()],
            rbf: "_Console/Test".to_string(),
            launch: Vec::new(),
            setname: None,
            skip_folders: Vec::new(),
            extra_paths: Vec::new(),
        }
    }

    fn terminal(job: &mut Job) -> Event {
        loop {
            let event = job
                .events
                .recv_timeout(Duration::from_secs(5))
                .expect("source worker returned a terminal event");
            if !matches!(event, Event::Progress(_)) {
                if let Some(handle) = job.handle.take() {
                    handle.join().unwrap();
                }
                return event;
            }
        }
    }

    #[test]
    fn an_empty_source_group_is_rejected_before_a_thread_starts() {
        let error = start(Request {
            target: Target::Gamelist,
            systems: Vec::new(),
            names: DisplayNames::default(),
            synopsis_language: None,
            cache_dir: temp("empty-cache"),
            require_usable_provider: true,
            homes: Arc::new(crate::mgl::Homes::default()),
        })
        .err()
        .expect("empty group is invalid");
        assert!(error.to_string().contains("no installed system"));
    }

    #[test]
    fn successful_pack_worker_marks_every_staged_cache_complete() {
        let root = temp("complete");
        let docs = root.join("docs");
        let artwork = docs.join("SuperGrafx/Artwork");
        let games = root.join("games");
        std::fs::create_dir_all(&artwork).unwrap();
        std::fs::create_dir_all(&games).unwrap();
        std::fs::write(
            artwork.join("manifest.tsv"),
            "#key\tstyle\tss_system_id\nTransient Pack Key\tbox-2D\t105\n",
        )
        .unwrap();
        std::fs::write(
            artwork.join("index.tsv"),
            "#name\tcrc\tsize\tkey\nKnown\t\t\tTransient Pack Key\n",
        )
        .unwrap();
        std::fs::write(
            artwork.join("gameinfo.tsv"),
            "#key\tname\tyear\tgenre\tdeveloper\tplayers\nTransient Pack Key\tPack Display Name\t1990\tPack Genre\tPack Studio\t2\n",
        )
        .unwrap();
        std::fs::write(artwork.join("Transient Pack Key.jpg"), b"jpeg").unwrap();
        std::fs::write(games.join("Known.pce"), b"rom").unwrap();

        let mut job = start(Request {
            target: Target::ArtworkPack { docs_root: docs },
            systems: vec![System {
                id: "SuperGrafx".to_string(),
                name: "SuperGrafx".to_string(),
                config: config(&games, "pce"),
            }],
            names: DisplayNames::default(),
            synopsis_language: Some("en".to_string()),
            cache_dir: root.join("cache"),
            require_usable_provider: true,
            homes: Arc::new(crate::mgl::Homes::default()),
        })
        .unwrap();
        let Event::Staged {
            prepared,
            providers,
            ..
        } = terminal(&mut job)
        else {
            panic!("Pack worker did not stage its cache");
        };
        assert_eq!(providers.len(), 1);
        assert_eq!(
            providers[0].health,
            crate::artwork_pack::ProviderHealth::Ready
        );
        assert!(
            !providers[0].catalogue_available(),
            "a source-switch result must not retain the parsed Pack catalogue"
        );
        let (caches, warnings) = prepared.install().unwrap();
        assert_eq!(caches.len(), 1);
        assert!(caches[0].fingerprints_complete);
        assert!(warnings.is_empty());
        let mut rows: Vec<_> = caches[0]
            .cache
            .folders
            .values()
            .flat_map(|folder| folder.rows.iter().cloned())
            .collect();
        assert_eq!(
            providers[0].apply_prepared(&mut rows),
            1,
            "the source worker must return presentation prepared for UI-only projection"
        );
        let expected_cover = artwork.join("Transient Pack Key.jpg");
        assert!(rows
            .iter()
            .any(|row| row.cover.as_deref() == Some(expected_cover.as_path())));
        assert!(crate::cache::load_artwork_pack_data(&root.join("cache"), "SuperGrafx").is_some());
        let bytes = std::fs::read(
            root.join("cache")
                .join("artwork-pack")
                .join("SuperGrafx.bin"),
        )
        .unwrap();
        for forbidden in [
            "Transient Pack Key",
            "Pack Display Name",
            "Pack Genre",
            "Pack Studio",
            "/docs/SuperGrafx/Artwork",
        ] {
            assert!(
                !bytes
                    .windows(forbidden.len())
                    .any(|window| window == forbidden.as_bytes()),
                "source cache must not persist Pack presentation: {forbidden}"
            );
        }
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn one_neogeo_job_stages_and_installs_both_installed_group_members() {
        let root = temp("neogeo-group");
        let docs = root.join("docs");
        let artwork = docs.join("NEOGEO/Artwork");
        let console_games = root.join("console-games");
        let arcade_games = root.join("arcade-games");
        std::fs::create_dir_all(&artwork).unwrap();
        std::fs::create_dir_all(&console_games).unwrap();
        std::fs::create_dir_all(&arcade_games).unwrap();
        std::fs::write(
            artwork.join("manifest.tsv"),
            "#key\tstyle\tss_system_id\nKnown\tbox-2D\t142\n",
        )
        .unwrap();
        std::fs::write(
            artwork.join("index.tsv"),
            "#name\tcrc\tsize\tkey\nKnown\t\t\tKnown\n",
        )
        .unwrap();
        std::fs::write(
            artwork.join("gameinfo.tsv"),
            "#key\tname\tyear\tgenre\tdeveloper\tplayers\n",
        )
        .unwrap();
        std::fs::write(artwork.join("Known.jpg"), b"jpeg").unwrap();
        std::fs::write(console_games.join("Known.neo"), b"rom").unwrap();
        std::fs::write(arcade_games.join("Known.neo"), b"rom").unwrap();

        let mut job = start(Request {
            target: Target::ArtworkPack { docs_root: docs },
            systems: vec![
                System {
                    id: "NeoGeo".to_string(),
                    name: "Neo Geo".to_string(),
                    config: config(&console_games, "neo"),
                },
                System {
                    id: "NeoGeoMVS".to_string(),
                    name: "Neo Geo MVS".to_string(),
                    config: config(&arcade_games, "neo"),
                },
            ],
            names: DisplayNames::default(),
            synopsis_language: Some("en".to_string()),
            cache_dir: root.join("cache"),
            require_usable_provider: true,
            homes: Arc::new(crate::mgl::Homes::default()),
        })
        .unwrap();
        let Event::Staged {
            prepared,
            providers,
            ..
        } = terminal(&mut job)
        else {
            panic!("the shared Pack worker did not stage its complete group");
        };
        assert_eq!(providers.len(), 2);
        let (caches, warnings) = prepared.install().unwrap();

        assert_eq!(
            caches
                .iter()
                .map(|cache| cache.id.as_str())
                .collect::<Vec<_>>(),
            ["NeoGeo", "NeoGeoMVS"]
        );
        assert!(caches.iter().all(|cache| cache.fingerprints_complete));
        assert!(warnings.is_empty());
        assert!(crate::cache::load_artwork_pack_data(&root.join("cache"), "NeoGeo").is_some());
        assert!(crate::cache::load_artwork_pack_data(&root.join("cache"), "NeoGeoMVS").is_some());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn failure_in_the_second_system_never_returns_a_partial_group() {
        let root = temp("second-fails");
        let first = root.join("first");
        let not_a_directory = root.join("second");
        std::fs::create_dir_all(&first).unwrap();
        std::fs::write(first.join("one.rom"), b"one").unwrap();
        std::fs::write(&not_a_directory, b"not a directory").unwrap();

        let mut job = start(Request {
            target: Target::Gamelist,
            systems: vec![
                System {
                    id: "First".to_string(),
                    name: "First".to_string(),
                    config: config(&first, "rom"),
                },
                System {
                    id: "Second".to_string(),
                    name: "Second".to_string(),
                    config: config(&not_a_directory, "rom"),
                },
            ],
            names: DisplayNames::default(),
            synopsis_language: None,
            cache_dir: root.join("cache"),
            require_usable_provider: true,
            homes: Arc::new(crate::mgl::Homes::default()),
        })
        .unwrap();
        let Event::Failed { progress, .. } = terminal(&mut job) else {
            panic!("the invalid second system must fail the complete group");
        };
        assert_eq!(progress.system, 2);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn invalid_pack_is_rejected_for_selection_but_rebuilds_neutral_rows_for_recovery() {
        let root = temp("invalid-pack-recovery");
        let docs = root.join("docs");
        let artwork = docs.join("SuperGrafx/Artwork");
        let games = root.join("games");
        let cache_dir = root.join("cache");
        std::fs::create_dir_all(&artwork).unwrap();
        std::fs::create_dir_all(&games).unwrap();
        std::fs::write(artwork.join("manifest.tsv"), "not-a-valid-manifest\n").unwrap();
        std::fs::write(games.join("Filesystem Name.pce"), b"rom").unwrap();

        let request = |require_usable_provider| Request {
            target: Target::ArtworkPack {
                docs_root: docs.clone(),
            },
            systems: vec![System {
                id: "SuperGrafx".to_string(),
                name: "SuperGrafx".to_string(),
                config: config(&games, "pce"),
            }],
            names: DisplayNames::default(),
            synopsis_language: Some("en".to_string()),
            cache_dir: cache_dir.clone(),
            require_usable_provider,
            homes: Arc::new(crate::mgl::Homes::default()),
        };

        let mut selection = start(request(true)).unwrap();
        let Event::Failed { error, .. } = terminal(&mut selection) else {
            panic!("a new invalid Pack selection must be rejected");
        };
        assert!(error.to_string().contains("invalid"));
        assert!(crate::cache::load_artwork_pack_data(&cache_dir, "SuperGrafx").is_none());

        let mut recovery = start(request(false)).unwrap();
        let Event::Staged {
            prepared,
            providers,
            ..
        } = terminal(&mut recovery)
        else {
            panic!("an invalid selected Pack must still stage source-neutral rows");
        };
        assert_eq!(providers.len(), 1);
        assert_eq!(
            providers[0].health,
            crate::artwork_pack::ProviderHealth::Invalid
        );
        let (caches, warnings) = prepared.install().unwrap();
        assert_eq!(caches.len(), 1);
        assert!(!caches[0].fingerprints_complete);
        assert!(warnings.is_empty());
        let installed = crate::cache::load_artwork_pack_data(&cache_dir, "SuperGrafx").unwrap();
        assert!(!installed.fingerprints_complete);
        assert_eq!(
            installed
                .cache
                .summary(&crate::browse::Place::Dir(games))
                .games,
            1
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_pre_cancelled_worker_returns_no_staged_cache() {
        let root = temp("cancelled");
        let cache_dir = root.join("cache");
        std::fs::create_dir_all(&cache_dir).unwrap();
        let cache_path = crate::cache::system_path(&cache_dir, "Test");
        let index_path = crate::cache::index_path(&cache_dir);
        std::fs::write(&cache_path, b"previous-cache").unwrap();
        std::fs::write(&index_path, b"previous-index").unwrap();
        let (sender, receiver) = mpsc::sync_channel(EVENT_QUEUE);
        let cancelled = Arc::new(AtomicBool::new(true));
        run(
            Request {
                target: Target::Gamelist,
                systems: vec![System {
                    id: "Test".to_string(),
                    name: "Test".to_string(),
                    config: config(&root, "rom"),
                }],
                names: DisplayNames::default(),
                synopsis_language: None,
                cache_dir: root.join("cache"),
                require_usable_provider: true,
                homes: Arc::new(crate::mgl::Homes::default()),
            },
            &sender,
            &cancelled,
        );
        assert!(matches!(receiver.recv().unwrap(), Event::Cancelled(_)));
        assert!(receiver.try_recv().is_err());
        assert_eq!(std::fs::read(&cache_path).unwrap(), b"previous-cache");
        assert_eq!(std::fs::read(&index_path).unwrap(), b"previous-index");
        assert_eq!(std::fs::read_dir(&cache_dir).unwrap().count(), 2);
        std::fs::remove_dir_all(root).ok();
    }

    /// An arcade folder for the Pack worker: a healthy descriptor and a
    /// malformed one, and a Pack that knows the healthy one.
    fn arcade_fixture(tag: &str) -> (PathBuf, PathBuf, PathBuf, PathBuf) {
        let root = temp(tag);
        let docs = root.join("docs");
        let artwork = docs.join("Arcade/Artwork");
        let games = root.join("_Arcade");
        std::fs::create_dir_all(&artwork).unwrap();
        std::fs::create_dir_all(&games).unwrap();
        std::fs::write(
            artwork.join("manifest.tsv"),
            "#key\tstyle\tss_system_id\nhealthy\tbox-2D\t75\n",
        )
        .unwrap();
        std::fs::write(
            artwork.join("index.tsv"),
            "#name\tcrc\tsize\tkey\nhealthy\t\t\thealthy\n",
        )
        .unwrap();
        std::fs::write(
            artwork.join("gameinfo.tsv"),
            "#key\tname\tyear\tgenre\tdeveloper\tplayers\nhealthy\tPack Healthy\t1990\tShooter\tStudio\t2\n",
        )
        .unwrap();
        std::fs::write(artwork.join("healthy.jpg"), b"jpeg").unwrap();
        let healthy = games.join("Healthy.mra");
        std::fs::write(
            &healthy,
            "<misterromdescription><setname>healthy</setname></misterromdescription>",
        )
        .unwrap();
        let broken = games.join("Broken.mra");
        std::fs::write(&broken, "<misterromdescription><rom></wrong>").unwrap();
        (root, docs, games, broken)
    }

    fn arcade_request(
        docs: &std::path::Path,
        games: &std::path::Path,
        cache_dir: &std::path::Path,
        require_usable_provider: bool,
    ) -> Request {
        Request {
            target: Target::ArtworkPack {
                docs_root: docs.to_path_buf(),
            },
            systems: vec![System {
                id: "Arcade".to_string(),
                name: "Arcade".to_string(),
                config: config(games, "mra"),
            }],
            names: DisplayNames::default(),
            synopsis_language: None,
            cache_dir: cache_dir.to_path_buf(),
            require_usable_provider,
            homes: Arc::new(crate::mgl::Homes::default()),
        }
    }

    /// Every prepared member of a Pack group leaves its decision and its
    /// prepared rows beside its cache, in the same transaction, so the
    /// next entry into it needs no worker; a Gamelist target leaves no
    /// such thing. The marker written down is the one the installed rows
    /// read back with.
    #[test]
    fn every_pack_member_stages_its_state_and_a_gamelist_target_stages_none() {
        let root = temp("state-per-member");
        let docs = root.join("docs");
        let artwork = docs.join("NEOGEO/Artwork");
        let console_games = root.join("console-games");
        let arcade_games = root.join("arcade-games");
        std::fs::create_dir_all(&artwork).unwrap();
        std::fs::create_dir_all(&console_games).unwrap();
        std::fs::create_dir_all(&arcade_games).unwrap();
        std::fs::write(
            artwork.join("manifest.tsv"),
            "#key\tstyle\tss_system_id\nKnown\tbox-2D\t142\n",
        )
        .unwrap();
        std::fs::write(
            artwork.join("index.tsv"),
            "#name\tcrc\tsize\tkey\nKnown\t\t\tKnown\n",
        )
        .unwrap();
        std::fs::write(
            artwork.join("gameinfo.tsv"),
            "#key\tname\tyear\tgenre\tdeveloper\tplayers\nKnown\tPack Known\t1990\tAction\tStudio\t1\n",
        )
        .unwrap();
        std::fs::write(artwork.join("Known.jpg"), b"jpeg").unwrap();
        std::fs::write(console_games.join("Known.neo"), b"rom").unwrap();
        std::fs::write(arcade_games.join("Known.neo"), b"rom").unwrap();
        let cache_dir = root.join("cache");
        let systems = vec![
            System {
                id: "NeoGeo".to_string(),
                name: "Neo Geo".to_string(),
                config: config(&console_games, "neo"),
            },
            System {
                id: "NeoGeoMVS".to_string(),
                name: "Neo Geo MVS".to_string(),
                config: config(&arcade_games, "neo"),
            },
        ];
        let mut job = start(Request {
            target: Target::ArtworkPack {
                docs_root: docs.clone(),
            },
            systems: systems.clone(),
            names: DisplayNames::default(),
            synopsis_language: Some("EN".to_string()),
            cache_dir: cache_dir.clone(),
            require_usable_provider: true,
            homes: Arc::new(crate::mgl::Homes::default()),
        })
        .unwrap();
        let Event::Staged {
            prepared,
            providers,
            warnings,
            ..
        } = terminal(&mut job)
        else {
            panic!("the Pack worker did not stage its group");
        };
        assert!(warnings.is_empty());
        let markers = prepared.markers().to_vec();
        for id in ["NeoGeo", "NeoGeoMVS"] {
            assert!(
                crate::cache::load_pack_source_state(&cache_dir, id)
                    .unwrap()
                    .is_none(),
                "nothing is live before the install"
            );
        }
        prepared.install().unwrap();
        for (position, id) in ["NeoGeo", "NeoGeoMVS"].into_iter().enumerate() {
            let state = crate::cache::load_pack_source_state(&cache_dir, id)
                .unwrap()
                .unwrap_or_else(|| panic!("{id} has its decision written down"));
            let accepted = state.accepted.expect("a preparation is an acceptance");
            assert!(state.declined.is_none());
            assert_eq!(accepted.docs_root, docs.to_str().unwrap());
            assert_eq!(accepted.language.as_deref(), Some("en"));
            assert_eq!(accepted.cache_marker, markers[position]);
            assert_eq!(
                accepted.cache_marker,
                crate::cache::load_artwork_pack_data(&cache_dir, id)
                    .unwrap()
                    .marker
            );
            assert_eq!(accepted.health, crate::artwork_pack::ProviderHealth::Ready);
            assert_eq!(accepted.skipped_entries, 0);
            assert!(accepted.signature.is_some());
            let prepared = crate::cache::load_pack_prepared_map(&cache_dir, id)
                .unwrap()
                .unwrap();
            assert_eq!(
                prepared.len(),
                1,
                "{id}: the one matched row is written down"
            );
            assert_eq!(
                prepared[0].1.name.as_deref(),
                Some("Pack Known"),
                "{id}: with the presentation the worker prepared"
            );
            assert_eq!(
                providers
                    .iter()
                    .find(|provider| provider.system_id == id)
                    .unwrap()
                    .prepared_pairs()
                    .len(),
                1
            );
        }

        let gamelist_store = root.join("gamelist-cache");
        let mut job = start(Request {
            target: Target::Gamelist,
            systems: vec![systems[0].clone()],
            names: DisplayNames::default(),
            synopsis_language: None,
            cache_dir: gamelist_store.clone(),
            require_usable_provider: true,
            homes: Arc::new(crate::mgl::Homes::default()),
        })
        .unwrap();
        let Event::Staged { prepared, .. } = terminal(&mut job) else {
            panic!("the Gamelist worker did not stage its cache");
        };
        prepared.install().unwrap();
        assert!(crate::cache::load_system(&gamelist_store, "NeoGeo").is_some());
        assert!(
            !gamelist_store.join("artwork-pack").exists(),
            "a Gamelist target has no Pack decision to write"
        );
        std::fs::remove_dir_all(root).ok();
    }

    /// A descriptor that cannot be parsed costs its own row its Pack data
    /// and nothing else: the group is staged, the healthy row is prepared,
    /// the screen is told how many rows were left out and why, and the
    /// count is written down with the decision. Repaired and run again,
    /// the row is prepared like any other. Before, the first broken file
    /// failed the whole group.
    #[test]
    fn a_broken_descriptor_does_not_fail_the_source_group() {
        let (root, docs, games, broken) = arcade_fixture("broken-descriptor");
        let cache_dir = root.join("cache");
        let mut job = start(arcade_request(&docs, &games, &cache_dir, true)).unwrap();
        let Event::Staged {
            prepared,
            providers,
            warnings,
            ..
        } = terminal(&mut job)
        else {
            panic!("a broken descriptor must not fail the group");
        };
        assert_eq!(
            warnings,
            ["Arcade: 1 game left without Pack data: malformed descriptor"]
        );
        let (caches, _) = prepared.install().unwrap();
        assert!(caches[0].fingerprints_complete);
        let mut rows: Vec<_> = caches[0]
            .cache
            .folders
            .values()
            .flat_map(|folder| folder.rows.iter().cloned())
            .collect();
        assert_eq!(rows.len(), 2, "the broken row is still listed");
        assert_eq!(providers[0].apply_prepared(&mut rows), 1);
        assert!(rows.iter().any(|row| row.name == "Pack Healthy"));
        assert!(
            rows.iter()
                .any(|row| row.name == "Broken" && row.cover.is_none()),
            "{rows:?}"
        );
        let state = crate::cache::load_pack_source_state(&cache_dir, "Arcade")
            .unwrap()
            .unwrap();
        assert_eq!(state.accepted.as_ref().unwrap().skipped_entries, 1);
        assert_eq!(
            state.accepted.as_ref().unwrap().health,
            crate::artwork_pack::ProviderHealth::Ready,
            "a broken descriptor is not a degraded Pack"
        );

        std::fs::write(
            &broken,
            "<misterromdescription><setname>healthy</setname></misterromdescription>",
        )
        .unwrap();
        let mut job = start(arcade_request(&docs, &games, &cache_dir, false)).unwrap();
        let Event::Staged {
            prepared,
            providers,
            warnings,
            ..
        } = terminal(&mut job)
        else {
            panic!("the repaired group did not stage");
        };
        assert!(warnings.is_empty());
        let (caches, _) = prepared.install().unwrap();
        let mut rows: Vec<_> = caches[0]
            .cache
            .folders
            .values()
            .flat_map(|folder| folder.rows.iter().cloned())
            .collect();
        assert_eq!(providers[0].apply_prepared(&mut rows), 2);
        assert_eq!(
            crate::cache::load_pack_source_state(&cache_dir, "Arcade")
                .unwrap()
                .unwrap()
                .accepted
                .unwrap()
                .skipped_entries,
            0
        );
        std::fs::remove_dir_all(root).ok();
    }

    /// A Pack root that is not there fails a recovery whole when there is
    /// a complete result to keep: the rows prepared while it could be read
    /// are not replaced by rows with nothing in them. Only a system with
    /// no complete result stages its ordinary rows, so it can open and say
    /// what is wrong.
    #[test]
    fn a_disconnected_pack_root_fails_a_recovery_that_has_a_complete_cache() {
        let (root, docs, games, _) = arcade_fixture("disconnected-root");
        let cache_dir = root.join("cache");
        let mut job = start(arcade_request(&docs, &games, &cache_dir, true)).unwrap();
        let Event::Staged { prepared, .. } = terminal(&mut job) else {
            panic!("the first preparation did not stage");
        };
        prepared.install().unwrap();
        let complete =
            std::fs::read(crate::cache::artwork_pack_system_path(&cache_dir, "Arcade")).unwrap();
        let state =
            std::fs::read(crate::cache::artwork_pack_source_path(&cache_dir, "Arcade")).unwrap();
        std::fs::rename(&docs, root.join("docs-away")).unwrap();

        let mut job = start(arcade_request(&docs, &games, &cache_dir, false)).unwrap();
        let Event::Failed { error, .. } = terminal(&mut job) else {
            panic!("a Pack that is gone must not replace a complete result");
        };
        assert!(error.to_string().contains("Unavailable"), "{error}");
        assert_eq!(
            std::fs::read(crate::cache::artwork_pack_system_path(&cache_dir, "Arcade")).unwrap(),
            complete
        );
        assert_eq!(
            std::fs::read(crate::cache::artwork_pack_source_path(&cache_dir, "Arcade")).unwrap(),
            state
        );

        let empty_store = root.join("cache-without-result");
        let mut job = start(arcade_request(&docs, &games, &empty_store, false)).unwrap();
        let Event::Staged { prepared, .. } = terminal(&mut job) else {
            panic!("with nothing to keep, the ordinary rows are staged so the system can open");
        };
        let (caches, _) = prepared.install().unwrap();
        assert!(!caches[0].fingerprints_complete);
        assert!(
            crate::cache::load_pack_source_state(&empty_store, "Arcade")
                .unwrap()
                .is_none(),
            "an unusable Pack is not written down as accepted"
        );
        std::fs::remove_dir_all(root).ok();
    }

    /// A cache folder that cannot be written is the whole preparation's
    /// failure, not a row's: nothing is installed and the previous result
    /// in another store is untouched.
    #[test]
    fn a_cache_write_failure_is_terminal_not_a_skip() {
        let (root, docs, games, _) = arcade_fixture("cache-write-failure");
        let cache_dir = root.join("cache");
        std::fs::create_dir_all(&cache_dir).unwrap();
        std::fs::write(cache_dir.join("artwork-pack"), b"not a directory").unwrap();
        let mut job = start(arcade_request(&docs, &games, &cache_dir, true)).unwrap();
        let Event::Failed { error, .. } = terminal(&mut job) else {
            panic!("a cache folder that is a file must fail the preparation");
        };
        assert!(
            matches!(&error, DegaussError::Io { path, .. } if path.starts_with(&cache_dir)),
            "the failure names the cache, not a row: {error}"
        );
        assert_eq!(
            std::fs::read(cache_dir.join("artwork-pack")).unwrap(),
            b"not a directory"
        );
        std::fs::remove_dir_all(root).ok();
    }
}
