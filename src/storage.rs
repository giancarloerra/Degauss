//! Browse-triggered storage discovery. No game walk and no cache writes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;
use std::thread::JoinHandle;

use crate::browse::Place;
use crate::error::{DegaussError, Result};
use crate::systems::{CoreIndex, FoundSystem, SystemDef};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Mount {
    id: String,
    device: String,
    root: String,
    point: PathBuf,
    source: String,
}

#[derive(Debug, Clone, Default)]
pub struct Mounts(Vec<Mount>);

impl Mounts {
    pub fn read(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path)
            .map_err(|error| DegaussError::io("reading storage mounts", path, error))?;
        let text = String::from_utf8_lossy(&bytes);
        let mut mounts = Vec::new();
        for line in text.lines() {
            let Some((before, after)) = line.split_once(" - ") else {
                return Err(DegaussError::malformed(
                    "storage mount table",
                    path,
                    "missing separator",
                ));
            };
            let fields: Vec<_> = before.split_whitespace().collect();
            let tail: Vec<_> = after.split_whitespace().collect();
            if fields.len() < 6 || tail.len() < 3 {
                return Err(DegaussError::malformed(
                    "storage mount table",
                    path,
                    "incomplete mount record",
                ));
            }
            mounts.push(Mount {
                id: fields[0].into(),
                device: fields[2].into(),
                root: crate::network_wait::decode_mount_field(fields[3]),
                point: crate::network_wait::decode_mount_field(fields[4]).into(),
                source: crate::network_wait::decode_mount_field(tail[1]),
            });
        }
        Ok(Self(mounts))
    }

    fn attachment(&self, paths: &[PathBuf]) -> Result<Vec<Mount>> {
        let mut result = Vec::new();
        for path in paths {
            // Follow configured directory links only for mount identity.
            // Discovery, launch paths and persisted cache keys remain unchanged.
            let resolved = match std::fs::canonicalize(path) {
                Ok(resolved) => resolved,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(DegaussError::io("resolving game storage", path, error));
                }
            };
            if let Some(mount) = self
                .0
                .iter()
                .filter(|mount| resolved.starts_with(&mount.point))
                .max_by_key(|mount| mount.point.components().count())
            {
                if !result.contains(mount) {
                    result.push(mount.clone());
                }
            }
            // A share can mount each system or subfolder below a games root.
            for mount in self
                .0
                .iter()
                .filter(|mount| mount.point.starts_with(&resolved))
            {
                if !result.contains(mount) {
                    result.push(mount.clone());
                }
            }
        }
        result.sort_by(|a, b| a.point.cmp(&b.point).then(a.id.cmp(&b.id)));
        Ok(result)
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct OfferKey {
    pub id: String,
    paths: Vec<PathBuf>,
    pub attachment: Vec<Mount>,
}

#[derive(Clone)]
pub struct Offer {
    pub system: FoundSystem,
    pub previous: Option<FoundSystem>,
    pub key: OfferKey,
    pub previous_root: Option<String>,
}

pub struct Plan {
    pub mounts: Mounts,
    pub systems: Vec<FoundSystem>,
    pub reused: Vec<(OfferKey, crate::cache::Summary)>,
    pub offers: Vec<Offer>,
    pub sources: crate::artwork_source::Resolution,
}

pub struct State {
    pub cores: Arc<CoreIndex>,
    pub mountinfo: PathBuf,
    pub active: HashMap<String, Vec<Mount>>,
    pub declined: std::collections::HashSet<OfferKey>,
    pub job: Option<Job>,
    pub offer: Option<Plan>,
    /// Unfinished candidates retain their previous source until indexing succeeds.
    pub rebuilding: Option<Vec<Offer>>,
    pub mounts: Mounts,
    pub requested: bool,
    pub stale: bool,
}

impl State {
    pub fn new(cores: Arc<CoreIndex>, mountinfo: PathBuf, systems: &[FoundSystem]) -> Result<Self> {
        let mounts = Mounts::read(&mountinfo)?;
        let active = systems
            .iter()
            .map(|system| {
                mounts
                    .attachment(&system.paths)
                    .map(|attachment| (system.def.id.clone(), attachment))
            })
            .collect::<Result<_>>()?;
        Ok(Self {
            cores,
            mountinfo,
            active,
            declined: Default::default(),
            job: None,
            offer: None,
            rebuilding: None,
            mounts,
            requested: false,
            stale: false,
        })
    }

    pub fn indexed(&mut self, id: &str) {
        if let Some(offers) = &mut self.rebuilding {
            if let Some(offer) = offers.iter().find(|offer| offer.system.def.id == id) {
                self.active
                    .insert(id.to_string(), offer.key.attachment.clone());
            }
            offers.retain(|offer| offer.system.def.id != id);
        }
    }

    pub fn refreshed(&mut self, system: &FoundSystem) -> Result<()> {
        let baseline = Self::new(
            self.cores.clone(),
            self.mountinfo.clone(),
            std::slice::from_ref(system),
        )?;
        self.active.extend(baseline.active);
        Ok(())
    }

    pub fn observed_mounts(&mut self, mounts: Mounts) {
        self.declined.retain(|key| {
            key.attachment
                .iter()
                .all(|attachment| mounts.0.contains(attachment))
        });
        self.mounts = mounts;
    }
}

pub struct Request {
    pub mountinfo: PathBuf,
    pub roots: Vec<PathBuf>,
    pub table: Vec<SystemDef>,
    pub logo_dir: Option<PathBuf>,
    pub cores: Arc<CoreIndex>,
    pub previous: Vec<FoundSystem>,
    pub active: HashMap<String, Vec<Mount>>,
    pub declined: std::collections::HashSet<OfferKey>,
    pub settings: crate::settings::Settings,
    pub cache_dir: PathBuf,
}

fn usable_previous(
    request: &Request,
    mounts: &Mounts,
    previous: &FoundSystem,
) -> Result<Option<FoundSystem>> {
    let Some(original) = request.active.get(&previous.def.id) else {
        return Ok(None);
    };
    let mut retained = previous.clone();
    retained.paths.clear();
    for path in &previous.paths {
        let current = mounts.attachment(std::slice::from_ref(path))?;
        if current.iter().any(|mount| !original.contains(mount)) {
            continue;
        }
        match std::fs::metadata(path) {
            Ok(metadata) if metadata.is_dir() => retained.paths.push(path.clone()),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(DegaussError::io(
                    "checking previous game storage",
                    path,
                    error,
                ))
            }
        }
    }
    Ok((!retained.paths.is_empty()).then_some(retained))
}

fn upsert(systems: &mut Vec<FoundSystem>, system: FoundSystem) {
    if let Some(old) = systems.iter_mut().find(|old| old.def.id == system.def.id) {
        *old = system;
    } else {
        systems.push(system);
    }
}

fn discover(request: Request, cancelled: &AtomicBool) -> Result<Option<Plan>> {
    if cancelled.load(Ordering::Relaxed) {
        return Ok(None);
    }
    let mounts = Mounts::read(&request.mountinfo)?;
    let found = crate::systems::discover_checked(
        &request.table,
        &request.roots,
        request.logo_dir.as_deref(),
        &request.cores,
    )?;
    if cancelled.load(Ordering::Relaxed) {
        return Ok(None);
    }
    let mut changed = Vec::new();
    let mut unresolved = Vec::new();
    for system in found {
        let old = request
            .previous
            .iter()
            .find(|old| old.def.id == system.def.id);
        let attachment = mounts.attachment(&system.paths)?;
        if old.is_none_or(|old| old.paths != system.paths)
            || request.active.get(&system.def.id) != Some(&attachment)
        {
            if !request.declined.contains(&OfferKey {
                id: system.def.id.clone(),
                paths: system.paths.clone(),
                attachment,
            }) {
                unresolved.push(system.clone());
            }
            changed.push(system);
        }
    }
    let mut source_systems = request.previous.clone();
    for system in &unresolved {
        upsert(&mut source_systems, system.clone());
    }
    source_systems.retain(|system| {
        unresolved.iter().any(|candidate| {
            system.def.id == candidate.def.id
                || crate::artwork_pack::source_group(&system.def.id).is_some_and(|group| {
                    crate::artwork_pack::source_group(&candidate.def.id) == Some(group)
                })
        })
    });
    let Some(sources) = crate::artwork_source::resolve(
        &source_systems,
        &request.settings,
        &request.cache_dir,
        cancelled,
    )?
    else {
        return Ok(None);
    };
    if !sources.errors.is_empty() {
        return Err(DegaussError::unsupported(
            "storage game data sources",
            sources
                .errors
                .values()
                .cloned()
                .collect::<Vec<_>>()
                .join("\n"),
        ));
    }
    let mut systems = request.previous.clone();
    let mut reused = Vec::new();
    let mut offers = Vec::new();
    for system in changed {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let old = request
            .previous
            .iter()
            .find(|old| old.def.id == system.def.id);
        let key = OfferKey {
            id: system.def.id.clone(),
            paths: system.paths.clone(),
            attachment: mounts.attachment(&system.paths)?,
        };
        let previous = match old {
            Some(old) => usable_previous(&request, &mounts, old)?,
            None => None,
        };
        let different_paths = old.is_some_and(|old| old.paths != system.paths);
        if !request.declined.contains(&key) {
            let cache = if sources.roots.contains_key(&system.def.id) {
                crate::cache::load_artwork_pack_data_checked(&request.cache_dir, &system.def.id)?
                    .map(|data| data.cache)
            } else {
                crate::cache::load_system_checked(&request.cache_dir, &system.def.id)?
            };
            let covered = !different_paths
                && cache.as_ref().is_some_and(|cache| {
                    system
                        .paths
                        .iter()
                        .all(|path| cache.folders.contains_key(&Place::Dir(path.clone()).key()))
                        && cache
                            .summary(&crate::browse::start_for(&system.to_config()))
                            .games
                            > 0
                });
            if covered {
                reused.push((
                    key,
                    cache
                        .as_ref()
                        .expect("covered cache")
                        .summary(&crate::browse::start_for(&system.to_config())),
                ));
                upsert(&mut systems, system);
                continue;
            }
            offers.push(Offer {
                system: system.clone(),
                previous: previous.clone(),
                key,
                previous_root: None,
            });
        }
        systems.retain(|existing| existing.def.id != system.def.id);
        if let Some(previous) = previous {
            upsert(&mut systems, previous);
        }
    }
    // Match the existing table ordering, including newly discovered systems.
    systems.sort_by_key(|system| {
        request
            .table
            .iter()
            .position(|def| def.id == system.def.id)
            .unwrap_or(usize::MAX)
    });
    Ok(Some(Plan {
        mounts,
        systems,
        reused,
        offers,
        sources,
    }))
}

type DiscoveryResult = Result<Option<Plan>>;
pub struct Job {
    result: Option<Receiver<DiscoveryResult>>,
    cancelled: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Job {
    pub fn start(request: Request) -> Result<Self> {
        let (sender, result) = mpsc::sync_channel(1);
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = cancelled.clone();
        let handle = std::thread::Builder::new()
            .name("degauss-storage".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    discover(request, &worker_cancelled)
                }))
                .unwrap_or_else(|_| {
                    Err(DegaussError::unsupported(
                        "storage discovery",
                        "worker panicked",
                    ))
                });
                let _ = sender.send(result);
            })
            .map_err(|error| {
                DegaussError::unsupported("storage discovery", format!("starting worker: {error}"))
            })?;
        Ok(Self {
            result: Some(result),
            cancelled,
            handle: Some(handle),
        })
    }

    pub fn try_recv(&mut self) -> Option<DiscoveryResult> {
        let result = match self.result.as_ref()?.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => Err(DegaussError::unsupported(
                "storage discovery",
                "worker stopped without a result",
            )),
        };
        self.result = None;
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        Some(result)
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
        self.result.take();
        // This worker only reads. A blocked filesystem must not delay
        // navigation or launch; cancellation discards its eventual result.
        self.handle.take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn manual_refresh_keeps_declines_until_a_browser_observes_disconnection() {
        let fixture = Fixture::new();
        fixture.game("card/games/NES/Old.rom");
        let old = fixture.found();
        let mut state =
            State::new(Arc::new(CoreIndex::default()), fixture.mountinfo(), &old).unwrap();
        fixture.game("usb/games/NES/New.rom");
        fixture.mounts(&[(2, &fixture.path("usb"))]);
        let offered = discover(
            fixture.request(&state, old.clone()),
            &AtomicBool::new(false),
        )
        .unwrap()
        .unwrap();
        state.observed_mounts(offered.mounts);
        let key = offered.offers[0].key.clone();
        state.declined.insert(key.clone());
        fixture.mounts(&[]);
        state.refreshed(&old[0]).unwrap();
        assert!(
            state.declined.contains(&key),
            "a manual refresh must not reset another drive's declined prompt"
        );
        state.observed_mounts(Mounts::read(&fixture.mountinfo()).unwrap());
        assert!(state.declined.is_empty());
    }

    #[test]
    fn dropping_a_storage_check_does_not_wait_for_a_blocked_reader() {
        let (sender, result) = mpsc::sync_channel(1);
        let (release, blocked) = mpsc::sync_channel(0);
        let (finished, completion) = mpsc::sync_channel(1);
        let cancelled = Arc::new(AtomicBool::new(false));
        let handle = std::thread::spawn(move || {
            blocked.recv().unwrap();
            assert!(sender.send(Ok(None)).is_err());
            finished.send(()).unwrap();
        });
        let job = Job {
            result: Some(result),
            cancelled: cancelled.clone(),
            handle: Some(handle),
        };
        let (dropped, observed) = mpsc::sync_channel(1);
        let dropper = std::thread::spawn(move || {
            drop(job);
            dropped.send(()).unwrap();
        });
        let responsive = observed
            .recv_timeout(std::time::Duration::from_secs(1))
            .is_ok();
        // Release and reap the fixture worker even when the assertion fails.
        release.send(()).unwrap();
        completion
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        dropper.join().unwrap();
        assert!(cancelled.load(Ordering::Relaxed));
        assert!(responsive, "dropping the check waited for its reader");
    }

    struct Fixture {
        root: PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let root = std::env::temp_dir().join(format!(
                "degauss-storage-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&root).unwrap();
            let fixture = Self {
                root: std::fs::canonicalize(root).unwrap(),
            };
            fixture.mounts(&[]);
            fixture
        }
        fn mountinfo(&self) -> PathBuf {
            self.root.join("mountinfo")
        }
        fn mounts(&self, mounts: &[(u32, &Path)]) {
            let mut text = "1 0 8:1 / / rw - ext4 /dev/card rw\n".to_string();
            for (id, path) in mounts {
                let path = path.to_string_lossy().replace(' ', "\\040");
                text.push_str(&format!(
                    "{id} 1 0:{id} / {path} rw - cifs //server/games rw\n"
                ));
            }
            std::fs::write(self.mountinfo(), text).unwrap();
        }
        fn path(&self, relative: &str) -> PathBuf {
            self.root.join(relative)
        }
        fn game(&self, relative: &str) {
            let path = self.path(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"ROM fixture").unwrap();
        }
        fn table(&self) -> Vec<SystemDef> {
            ["NES", "SNES"]
                .into_iter()
                .map(|id| {
                    toml::from_str(&format!(
                "id={id:?}\nname={id:?}\nfolders=[{id:?}]\nrbf='_Console/Test'\nextensions=['rom']"
            ))
                    .unwrap()
                })
                .collect()
        }
        fn request(&self, state: &State, previous: Vec<FoundSystem>) -> Request {
            Request {
                mountinfo: self.mountinfo(),
                roots: vec![
                    self.path("usb/games"),
                    self.path("other/games"),
                    self.path("card/games"),
                ],
                table: self.table(),
                logo_dir: None,
                cores: state.cores.clone(),
                previous,
                active: state.active.clone(),
                declined: state.declined.clone(),
                settings: Default::default(),
                cache_dir: self.path("cache"),
            }
        }
        fn found(&self) -> Vec<FoundSystem> {
            let request = self.request(&self.state(&[]), Vec::new());
            crate::systems::discover_checked(&request.table, &request.roots, None, &request.cores)
                .unwrap()
        }
        fn state(&self, previous: &[FoundSystem]) -> State {
            State::new(Arc::new(CoreIndex::default()), self.mountinfo(), previous).unwrap()
        }
        fn cached(&self, system: &FoundSystem) -> Vec<u8> {
            let library = crate::browse::Library::open(&system.to_config()).unwrap();
            let cache = crate::cache::build_system(&library);
            let mut index = crate::cache::Index::default();
            index.systems.insert(
                system.def.id.clone(),
                cache.summary(&crate::browse::start_for(&system.to_config())),
            );
            crate::cache::save_system_with_index(
                &self.path("cache"),
                crate::cache::CacheKind::Gamelist,
                &system.def.id,
                &cache,
                &index,
            )
            .unwrap();
            std::fs::read(crate::cache::system_path(
                &self.path("cache"),
                &system.def.id,
            ))
            .unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.root).unwrap();
        }
    }

    #[test]
    fn cache_reuse_respects_the_automatic_source_of_the_whole_group() {
        let f = Fixture::new();
        let table: Vec<SystemDef> = ["NeoGeo", "NeoGeoMVS"]
            .into_iter()
            .map(|id| {
                f.game(&format!("card/games/{id}/Game.rom"));
                toml::from_str(&format!(
                    "id={id:?}\nname={id:?}\nfolders=[{id:?}]\nrbf='_Console/Test'\nextensions=['rom']"
                ))
                .unwrap()
            })
            .collect();
        let mut request = f.request(&f.state(&[]), Vec::new());
        request.table = table.clone();
        let found =
            crate::systems::discover_checked(&table, &request.roots, None, &request.cores).unwrap();
        f.cached(&found[0]);
        let state = f.state(&found);
        crate::cache::save_pack_source_state(
            &f.path("cache"),
            "NeoGeo",
            &crate::cache::PackSourceState {
                accepted: Some(crate::cache::AcceptedSource {
                    docs_root: f.path("docs").to_string_lossy().into_owned(),
                    language: None,
                    signature: None,
                    cache_marker: 0,
                    health: crate::artwork_pack::ProviderHealth::Ready,
                    diagnostics: Vec::new(),
                    skipped_entries: 0,
                }),
                declined: None,
            },
        )
        .unwrap();
        std::fs::write(f.path("card/games/NeoGeoMVS/gamelist.xml"), "<gameList/>").unwrap();
        f.mounts(&[(2, &f.path("card/games/NeoGeo"))]);
        let mut request = f.request(&state, found);
        request.table = table;
        let plan = discover(request, &AtomicBool::new(false)).unwrap().unwrap();
        assert!(
            !plan.sources.roots.contains_key("NeoGeo"),
            "a sibling's gamelist keeps the whole Automatic group on Gamelist"
        );
        assert_eq!(plan.reused.len(), 1);
        assert_eq!(plan.reused[0].0.id, "NeoGeo");
        assert!(
            plan.offers.is_empty(),
            "the existing Gamelist cache is reusable"
        );
    }

    #[test]
    fn indexed_late_storage_reuses_matching_cache_without_writing() {
        let f = Fixture::new();
        f.game("usb/games/NES/Game.rom");
        let found = f.found();
        let before = f.cached(&found[0]);
        std::fs::rename(f.path("usb"), f.path("detached")).unwrap();
        let state = f.state(&[]);
        std::fs::rename(f.path("detached"), f.path("usb")).unwrap();
        f.mounts(&[(2, &f.path("usb"))]);
        let plan = discover(f.request(&state, Vec::new()), &AtomicBool::new(false))
            .unwrap()
            .unwrap();
        assert!(plan.offers.is_empty());
        assert_eq!(
            plan.reused
                .iter()
                .map(|(key, _)| key.id.as_str())
                .collect::<Vec<_>>(),
            ["NES"]
        );
        assert_eq!(plan.systems[0].paths, found[0].paths);
        assert_eq!(
            before,
            std::fs::read(crate::cache::system_path(&f.path("cache"), "NES")).unwrap()
        );
    }

    #[test]
    fn priority_change_requires_consent_and_decline_is_attachment_specific() {
        let f = Fixture::new();
        f.game("card/games/NES/Old.rom");
        let old = f.found();
        let before = f.cached(&old[0]);
        let mut state = f.state(&old);
        f.game("usb/games/NES/New.rom");
        f.mounts(&[(2, &f.path("usb"))]);
        let plan = discover(f.request(&state, old.clone()), &AtomicBool::new(false))
            .unwrap()
            .unwrap();
        assert_eq!(plan.offers.len(), 1);
        assert_eq!(plan.offers[0].system.paths, [f.path("usb/games/NES")]);
        assert_eq!(
            plan.systems[0].paths, old[0].paths,
            "no source adoption before consent"
        );
        state.declined.insert(plan.offers[0].key.clone());
        assert!(
            discover(f.request(&state, old.clone()), &AtomicBool::new(false))
                .unwrap()
                .unwrap()
                .offers
                .is_empty()
        );
        f.game("other/games/SNES/Second.rom");
        f.mounts(&[(2, &f.path("usb")), (4, &f.path("other"))]);
        let other = discover(f.request(&state, old.clone()), &AtomicBool::new(false))
            .unwrap()
            .unwrap();
        assert_eq!(
            other
                .offers
                .iter()
                .map(|offer| offer.system.def.id.as_str())
                .collect::<Vec<_>>(),
            ["SNES"]
        );
        // A different mount identity also permits a new offer without
        // an intervening browser check.
        f.mounts(&[(3, &f.path("usb")), (4, &f.path("other"))]);
        let reattached = discover(f.request(&state, old.clone()), &AtomicBool::new(false))
            .unwrap()
            .unwrap();
        assert!(reattached
            .offers
            .iter()
            .any(|offer| offer.system.def.id == "NES"));
        std::fs::rename(f.path("usb"), f.path("detached")).unwrap();
        f.mounts(&[(4, &f.path("other"))]);
        let disconnected = discover(f.request(&state, old.clone()), &AtomicBool::new(false))
            .unwrap()
            .unwrap();
        state.observed_mounts(disconnected.mounts);
        std::fs::rename(f.path("detached"), f.path("usb")).unwrap();
        f.mounts(&[(2, &f.path("usb")), (4, &f.path("other"))]);
        let reused_id = discover(f.request(&state, old), &AtomicBool::new(false))
            .unwrap()
            .unwrap();
        assert!(
            reused_id
                .offers
                .iter()
                .any(|offer| offer.system.def.id == "NES"),
            "an observed disconnect clears the decline even when Linux reuses its mount ID"
        );
        assert_eq!(
            before,
            std::fs::read(crate::cache::system_path(&f.path("cache"), "NES")).unwrap()
        );
    }

    #[test]
    fn an_unobserved_reconnect_can_keep_the_decline_until_restart() {
        let f = Fixture::new();
        f.game("usb/games/NES/Game.rom");
        f.mounts(&[(2, &f.path("usb"))]);
        let mut state = f.state(&[]);
        let offered = discover(f.request(&state, Vec::new()), &AtomicBool::new(false))
            .unwrap()
            .unwrap();
        assert_eq!(offered.offers.len(), 1);
        state.declined.insert(offered.offers[0].key.clone());

        f.mounts(&[]);
        f.mounts(&[(2, &f.path("usb"))]);
        let same_attachment = discover(f.request(&state, Vec::new()), &AtomicBool::new(false))
            .unwrap()
            .unwrap();
        assert!(same_attachment.offers.is_empty());
        assert!(same_attachment.systems.is_empty());
        assert!(!f.path("cache").exists());

        let restarted = f.state(&[]);
        let new_offer = discover(f.request(&restarted, Vec::new()), &AtomicBool::new(false))
            .unwrap()
            .unwrap();
        assert_eq!(new_offer.offers.len(), 1);
        assert_eq!(new_offer.offers[0].system.def.id, "NES");
    }

    #[test]
    fn observing_one_disconnect_preserves_the_other_attachments_decline() {
        let f = Fixture::new();
        f.game("usb/games/NES/Game.rom");
        f.game("other/games/SNES/Game.rom");
        f.mounts(&[(2, &f.path("usb")), (4, &f.path("other"))]);
        let mut state = f.state(&[]);
        let offered = discover(f.request(&state, Vec::new()), &AtomicBool::new(false))
            .unwrap()
            .unwrap();
        assert_eq!(offered.offers.len(), 2);
        state
            .declined
            .extend(offered.offers.into_iter().map(|offer| offer.key));

        std::fs::rename(f.path("usb"), f.path("detached")).unwrap();
        f.mounts(&[(4, &f.path("other"))]);
        let disconnected = discover(f.request(&state, Vec::new()), &AtomicBool::new(false))
            .unwrap()
            .unwrap();
        state.observed_mounts(disconnected.mounts);
        assert_eq!(state.declined.len(), 1);

        std::fs::rename(f.path("detached"), f.path("usb")).unwrap();
        f.mounts(&[(2, &f.path("usb")), (4, &f.path("other"))]);
        let reconnected = discover(f.request(&state, Vec::new()), &AtomicBool::new(false))
            .unwrap()
            .unwrap();
        assert_eq!(reconnected.offers.len(), 1);
        assert_eq!(reconnected.offers[0].system.def.id, "NES");
    }

    #[cfg(unix)]
    #[test]
    fn linked_game_folders_track_the_target_attachment_without_changing_source_paths() {
        for root_link in [false, true] {
            let f = Fixture::new();
            f.game("usb/games/NES/Game.rom");
            std::fs::create_dir_all(f.path("card")).unwrap();
            if root_link {
                std::os::unix::fs::symlink(f.path("usb/games"), f.path("card/games")).unwrap();
            } else {
                std::fs::create_dir_all(f.path("card/games")).unwrap();
                std::os::unix::fs::symlink(f.path("usb/games/NES"), f.path("card/games/NES"))
                    .unwrap();
            }
            f.mounts(&[(2, &f.path("usb"))]);
            let mut state = f.state(&[]);
            let mut request = f.request(&state, Vec::new());
            request.roots = vec![f.path("card/games")];
            let offered = discover(request, &AtomicBool::new(false)).unwrap().unwrap();
            assert_eq!(offered.offers.len(), 1);
            let offer = &offered.offers[0];
            assert_eq!(offer.system.paths, [f.path("card/games/NES")]);
            assert!(
                offer
                    .key
                    .attachment
                    .iter()
                    .any(|mount| mount.point == f.path("usb")),
                "a directory link must track the target drive, not the card holding the link"
            );
            state.declined.insert(offer.key.clone());
            f.mounts(&[]);
            state.observed_mounts(Mounts::read(&f.mountinfo()).unwrap());
            assert!(state.declined.is_empty());

            f.mounts(&[(2, &f.path("usb"))]);
            let before = f.cached(&offer.system);
            let mut request = f.request(&state, Vec::new());
            request.roots = vec![f.path("card/games")];
            let reused = discover(request, &AtomicBool::new(false)).unwrap().unwrap();
            assert!(reused.offers.is_empty());
            assert_eq!(reused.reused.len(), 1);
            assert_eq!(reused.systems[0].paths, offer.system.paths);
            assert_eq!(
                before,
                std::fs::read(crate::cache::system_path(&f.path("cache"), "NES")).unwrap()
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn storage_attachment_errors_are_not_treated_as_missing_drives() {
        let f = Fixture::new();
        let link = f.path("loop");
        std::os::unix::fs::symlink(&link, &link).unwrap();
        let mounts = Mounts::read(&f.mountinfo()).unwrap();
        let error = mounts.attachment(&[link]).unwrap_err().to_string();
        assert!(error.contains("resolving game storage") && error.contains("loop"));
        assert!(mounts.attachment(&[f.path("absent")]).unwrap().is_empty());
    }

    #[test]
    fn mount_over_empty_directory_is_not_mistaken_for_its_placeholder_cache() {
        let f = Fixture::new();
        std::fs::create_dir_all(f.path("card/games/NES")).unwrap();
        let old = f.found();
        f.cached(&old[0]);
        let state = f.state(&old);
        f.game("card/games/NES/Arrived.rom");
        f.mounts(&[(2, &f.path("card/games/NES"))]);
        let plan = discover(f.request(&state, old), &AtomicBool::new(false))
            .unwrap()
            .unwrap();
        assert_eq!(plan.offers.len(), 1);
        assert!(plan.offers[0].previous.is_none());
        assert!(
            plan.systems.is_empty(),
            "the new submount is not the previous local source"
        );
    }

    #[test]
    fn declined_storage_does_not_resolve_its_unused_artwork_source() {
        let f = Fixture::new();
        f.game("card/games/NES/Old.rom");
        let old = f.found();
        let before = f.cached(&old[0]);
        let mut state = f.state(&old);
        f.game("usb/games/NES/New.rom");
        f.mounts(&[(2, &f.path("usb"))]);
        let offered = discover(f.request(&state, old.clone()), &AtomicBool::new(false))
            .unwrap()
            .unwrap();
        state.declined.insert(offered.offers[0].key.clone());
        let source = crate::cache::artwork_pack_source_path(&f.path("cache"), "NES");
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::write(source, b"unreadable source state fixture").unwrap();
        let declined = discover(f.request(&state, old.clone()), &AtomicBool::new(false))
            .unwrap()
            .unwrap();
        assert!(declined.offers.is_empty() && declined.reused.is_empty());
        assert_eq!(declined.systems[0].paths, old[0].paths);
        assert_eq!(
            before,
            std::fs::read(crate::cache::system_path(&f.path("cache"), "NES")).unwrap()
        );
    }

    #[test]
    fn custom_roots_and_mounts_below_games_reuse_the_same_source_keys() {
        let f = Fixture::new();
        f.game("custom games/NES/Game.rom");
        let mut request = f.request(&f.state(&[]), Vec::new());
        request.roots = vec![f.path("custom games")];
        let found =
            crate::systems::discover_checked(&request.table, &request.roots, None, &request.cores)
                .unwrap();
        f.cached(&found[0]);
        f.mounts(&[(2, &f.path("custom games/NES"))]);
        let plan = discover(request, &AtomicBool::new(false)).unwrap().unwrap();
        assert_eq!(
            plan.reused
                .iter()
                .map(|(key, _)| key.id.as_str())
                .collect::<Vec<_>>(),
            ["NES"]
        );
        assert!(plan.offers.is_empty());
    }

    #[test]
    fn unrelated_mount_does_not_read_cache_or_trigger_game_work() {
        let f = Fixture::new();
        f.game("card/games/NES/Old.rom");
        let old = f.found();
        let state = f.state(&old);
        let before = f.cached(&old[0]);
        std::fs::write(
            crate::cache::system_path(&f.path("cache"), "NES"),
            b"unreadable cache fixture",
        )
        .unwrap();
        f.mounts(&[(2, &f.path("unrelated"))]);
        let plan = discover(f.request(&state, old.clone()), &AtomicBool::new(false))
            .unwrap()
            .unwrap();
        assert!(plan.reused.is_empty() && plan.offers.is_empty());
        assert_eq!(plan.systems[0].paths, old[0].paths);
        f.mounts(&[(3, &f.path("card/games/NES"))]);
        let error = discover(f.request(&state, old), &AtomicBool::new(false))
            .err()
            .unwrap();
        assert!(
            error.to_string().contains("system cache"),
            "real cache failures are not missing libraries"
        );
        assert!(!before.is_empty());
    }

    #[test]
    fn an_unrelated_non_utf8_mount_record_does_not_hide_valid_storage() {
        let f = Fixture::new();
        f.game("card/games/NES/Old.rom");
        let mut bytes = std::fs::read(f.mountinfo()).unwrap();
        bytes.extend_from_slice(b"2 1 0:2 / /unrelated rw - tmpfs invalid-\xff rw\n");
        std::fs::write(f.mountinfo(), bytes).unwrap();
        let mounts = Mounts::read(&f.mountinfo()).unwrap();
        let attachments = mounts.attachment(&[f.path("card/games/NES")]).unwrap();
        assert_eq!(attachments.len(), 1);
        assert_eq!(attachments[0].point, Path::new("/"));
    }

    #[test]
    fn discovery_cancellation_and_mount_table_errors_are_explicit() {
        let f = Fixture::new();
        let state = f.state(&[]);
        assert!(
            discover(f.request(&state, Vec::new()), &AtomicBool::new(true))
                .unwrap()
                .is_none()
        );
        std::fs::write(f.mountinfo(), "not a mount table").unwrap();
        assert!(Mounts::read(&f.mountinfo())
            .unwrap_err()
            .to_string()
            .contains("missing separator"));
        let missing = Mounts::read(&f.path("missing")).unwrap_err().to_string();
        assert!(missing.contains("reading storage mounts") && missing.contains("missing"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_real_linux_mount_table_has_a_root_attachment() {
        let mounts = Mounts::read(Path::new("/proc/self/mountinfo")).unwrap();
        assert!(!mounts.attachment(&[PathBuf::from("/")]).unwrap().is_empty());
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "requires an isolated Linux mount namespace with CAP_SYS_ADMIN"]
    fn real_linux_mounts_reoffer_only_after_an_observed_disconnect() {
        struct Mounted {
            path: PathBuf,
            mounted: bool,
        }
        impl Mounted {
            fn mount(&mut self) {
                assert!(std::process::Command::new("mount")
                    .args(["-t", "tmpfs", "-o", "size=2m", "tmpfs"])
                    .arg(&self.path)
                    .status()
                    .unwrap()
                    .success());
                self.mounted = true;
            }
            fn unmount(&mut self) {
                assert!(std::process::Command::new("umount")
                    .arg(&self.path)
                    .status()
                    .unwrap()
                    .success());
                self.mounted = false;
            }
        }
        impl Drop for Mounted {
            fn drop(&mut self) {
                if self.mounted {
                    self.unmount();
                }
            }
        }

        for linked_root in [false, true] {
            let f = Fixture::new();
            f.game("card/games/NES/Old.rom");
            let old = f.found();
            let before = f.cached(&old[0]);
            let proc_mounts = PathBuf::from("/proc/self/mountinfo");
            let mut state =
                State::new(Arc::new(CoreIndex::default()), proc_mounts.clone(), &old).unwrap();
            let target = f.path("usb drive");
            std::fs::create_dir(&target).unwrap();
            let games = if linked_root {
                let alias = f.path("linked games");
                std::os::unix::fs::symlink(target.join("games"), &alias).unwrap();
                alias
            } else {
                target.join("games")
            };
            let request = |state: &State| {
                let mut request = f.request(state, old.clone());
                request.mountinfo = proc_mounts.clone();
                request.roots = vec![games.clone(), f.path("card/games")];
                request
            };
            let mut mounted = Mounted {
                path: target.clone(),
                mounted: false,
            };
            mounted.mount();
            f.game("usb drive/games/NES/New.rom");
            let offered = discover(request(&state), &AtomicBool::new(false))
                .unwrap()
                .unwrap();
            assert_eq!(offered.offers.len(), 1);
            assert_eq!(offered.offers[0].system.paths, [games.join("NES")]);
            assert!(offered.offers[0]
                .key
                .attachment
                .iter()
                .any(|mount| mount.point == target));
            assert_eq!(offered.systems[0].paths, old[0].paths);
            state.observed_mounts(offered.mounts);
            state.declined.insert(offered.offers[0].key.clone());
            let declined = discover(request(&state), &AtomicBool::new(false))
                .unwrap()
                .unwrap();
            assert!(declined.offers.is_empty());

            mounted.unmount();
            let disconnected = discover(request(&state), &AtomicBool::new(false))
                .unwrap()
                .unwrap();
            state.observed_mounts(disconnected.mounts);
            assert!(state.declined.is_empty());
            mounted.mount();
            f.game("usb drive/games/NES/New.rom");
            let reconnected = discover(request(&state), &AtomicBool::new(false))
                .unwrap()
                .unwrap();
            assert_eq!(reconnected.offers.len(), 1);
            assert_eq!(reconnected.offers[0].system.paths, [games.join("NES")]);
            assert_eq!(
                before,
                std::fs::read(crate::cache::system_path(&f.path("cache"), "NES")).unwrap()
            );
            let refreshed = reconnected.offers[0].system.clone();
            f.cached(&refreshed);
            state.refreshed(&refreshed).unwrap();
            assert_eq!(state.active["NES"], reconnected.offers[0].key.attachment);
            let mut after_refresh = request(&state);
            after_refresh.previous = vec![refreshed];
            let after_refresh = discover(after_refresh, &AtomicBool::new(false))
                .unwrap()
                .unwrap();
            assert!(after_refresh.offers.is_empty());
            assert_eq!(after_refresh.systems[0].paths, [games.join("NES")]);
            mounted.unmount();
        }
    }
}
