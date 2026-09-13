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
        if request.validate_location_only {
            provider.discard_catalogue();
            snapshots.push(Snapshot {
                provider,
                source_changed,
                cache_needs_recovery: false,
            });
            continue;
        }
        let cached = crate::cache::load_artwork_pack_data(&request.cache_dir, &request.system_id);
        let (fingerprints, cache_needs_recovery) = match cached.as_ref() {
            Some(_) if !provider.health.usable() => {
                (crate::cache::ContentFingerprints::new(), false)
            }
            Some(cached) => match provider.cached_fingerprints_are_current(
                &cached.cache,
                &cached.fingerprints,
                cached.fingerprints_complete,
                &request.homes,
                cancelled,
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
        if !cache_needs_recovery && provider.health.usable() {
            let Some(cache) = cached.as_ref().map(|cached| &cached.cache) else {
                let _ = events.send(Event::Failed {
                    error: DegaussError::unsupported(
                        "Artwork Pack",
                        "provider validation finished without its source cache",
                    ),
                    progress,
                });
                return;
            };
            match provider.prepare_for_cache(cache, &fingerprints, &request.homes, cancelled) {
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
        provider.discard_catalogue();
        snapshots.push(Snapshot {
            provider,
            source_changed,
            cache_needs_recovery,
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
}
