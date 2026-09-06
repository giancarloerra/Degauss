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
            if request.require_usable_provider && !provider.health.usable() {
                let _ = events.send(Event::Failed {
                    error: DegaussError::unsupported("Artwork Pack", provider.status_line()),
                    progress,
                });
                return;
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
        let fingerprints = if let Some(provider) = provider
            .as_ref()
            .filter(|provider| provider.health.usable())
        {
            let base_hashed_files = progress.hashed_files;
            let base_hashed_bytes = progress.hashed_bytes;
            match provider.fingerprints_for_cache(&cache, cancelled, &mut |files, bytes| {
                progress.hashed_files = base_hashed_files.saturating_add(files);
                progress.hashed_bytes = base_hashed_bytes.saturating_add(bytes);
                let _ = events.try_send(Event::Progress(progress.clone()));
            }) {
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
            match provider.prepare_for_cache(&cache, &fingerprints, cancelled) {
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
        }
        if let Some(mut provider) = provider {
            provider.discard_catalogue();
            providers.push(provider);
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
    match crate::cache::stage_transactional(&request.cache_dir, kind, caches, cancelled) {
        Ok(Some(prepared)) if !cancelled.load(Ordering::Relaxed) => {
            let _ = events.send(Event::Staged {
                target: request.target,
                prepared,
                providers,
                progress,
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
            },
            &sender,
            &cancelled,
        );
        assert!(matches!(receiver.recv().unwrap(), Event::Cancelled(_)));
        assert!(receiver.try_recv().is_err());
        std::fs::remove_dir_all(root).ok();
    }
}
