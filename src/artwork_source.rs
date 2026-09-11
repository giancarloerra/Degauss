//! Resolve source choices once per lifecycle, on a process-owned worker.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;
use std::thread::JoinHandle;

use crate::artwork_pack::{self, Provider};
use crate::error::{DegaussError, Result};
use crate::settings::Settings;
use crate::systems::FoundSystem;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Automatic,
    Gamelist,
    ArtworkPack,
}

pub fn mode(settings: &Settings, system_id: &str) -> Mode {
    let Some(group) = artwork_pack::source_group(system_id) else {
        return Mode::Gamelist;
    };
    if settings.artwork_pack_roots.contains_key(group) {
        Mode::ArtworkPack
    } else if settings.gamelist_sources.contains(group) {
        Mode::Gamelist
    } else {
        Mode::Automatic
    }
}

#[derive(Debug, Default)]
pub struct Resolution {
    pub roots: BTreeMap<String, String>,
    pub errors: BTreeMap<String, String>,
}

type Outcome = Result<Option<Resolution>>;

pub fn resolve(systems: &[FoundSystem], settings: &Settings, cancelled: &AtomicBool) -> Outcome {
    let bases: Vec<_> = std::iter::once(PathBuf::from("/media/fat"))
        .chain((0..=7).map(|index| PathBuf::from(format!("/media/usb{index}"))))
        .collect();
    resolve_with_bases(systems, settings, cancelled, &bases)
}

/// Bases are mount roots, in priority order. Production uses SD then USB0..7.
/// Only exact mapped Artwork directories are probed, never whole-card scans.
fn resolve_with_bases(
    systems: &[FoundSystem],
    settings: &Settings,
    cancelled: &AtomicBool,
    bases: &[PathBuf],
) -> Outcome {
    let mut resolved = Resolution::default();
    let mut groups: BTreeMap<&str, Vec<&FoundSystem>> = BTreeMap::new();
    for system in systems {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        if let Some(group) = artwork_pack::source_group(&system.def.id) {
            if let Some(root) = settings.artwork_pack_roots.get(group) {
                resolved.roots.insert(group.to_string(), root.clone());
            } else if mode(settings, &system.def.id) == Mode::Automatic {
                groups.entry(group).or_default().push(system);
            }
        }
    }
    // Identical catalogue mappings (such as two-player variants) share one
    // validation result even when their saved source choices are independent.
    let mut validated = BTreeMap::<(Vec<&str>, PathBuf), std::result::Result<bool, String>>::new();
    let mut probes = BTreeMap::new();
    for (group, members) in groups {
        let outcome = (|| -> Result<Option<String>> {
            let mut gamelist = false;
            let mut checked = BTreeSet::new();
            for system in &members {
                for root in &system.paths {
                    if cancelled.load(Ordering::Relaxed) {
                        return Ok(None);
                    }
                    if checked.insert(root) {
                        gamelist |= exists_checked(&root.join("gamelist.xml"), &mut probes)?;
                    }
                }
            }
            if gamelist {
                return Ok(None);
            }
            let id = &members[0].def.id;
            let folders = artwork_pack::expected_folders(id);
            for base in bases {
                if cancelled.load(Ordering::Relaxed) {
                    return Ok(None);
                }
                let docs = base.join("docs");
                let key = (folders.to_vec(), docs.clone());
                let usable = if let Some(result) = validated.get(&key) {
                    result.clone().map_err(|detail| {
                        DegaussError::malformed("automatic Artwork Pack source", &docs, detail)
                    })?
                } else {
                    let mut present = false;
                    for folder in folders {
                        if cancelled.load(Ordering::Relaxed) {
                            return Ok(None);
                        }
                        present |= exists_checked(&docs.join(folder).join("Artwork"), &mut probes)?;
                    }
                    if present {
                        let Some(provider) = Provider::load_controlled(id, &docs, None, cancelled)
                        else {
                            return Ok(None);
                        };
                        if !provider.health.usable() {
                            let detail = format!(
                                "{}: {}",
                                provider.health.label(),
                                provider.diagnostics.join("; ")
                            );
                            validated.insert(key, Err(detail.clone()));
                            return Err(DegaussError::malformed(
                                "automatic Artwork Pack source",
                                &docs,
                                detail,
                            ));
                        }
                    }
                    validated.insert(key, Ok(present));
                    present
                };
                if usable {
                    let root = docs.to_str().ok_or_else(|| {
                        DegaussError::unsupported(
                            "automatic Artwork Pack source",
                            "docs path is not UTF-8",
                        )
                    })?;
                    return Ok(Some(root.to_string()));
                }
            }
            Ok(None)
        })();
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        match outcome {
            Ok(Some(root)) => {
                resolved.roots.insert(group.to_string(), root);
            }
            Ok(None) => {}
            Err(error) => {
                resolved.errors.insert(group.to_string(), error.to_string());
            }
        }
    }
    Ok((!cancelled.load(Ordering::Relaxed)).then_some(resolved))
}

fn exists_checked(path: &Path, probes: &mut BTreeMap<PathBuf, bool>) -> Result<bool> {
    if let Some(present) = probes.get(path) {
        return Ok(*present);
    }
    let present = match std::fs::metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match std::fs::symlink_metadata(path) {
                // A dangling link is installed source material whose target
                // failed to resolve. It must not authorize another source.
                Ok(_) => Err(DegaussError::io("probing artwork source", path, error)),
                Err(link_error) if link_error.kind() == std::io::ErrorKind::NotFound => {
                    // A missing child also follows a dangling intermediate
                    // link. Check ancestors up to the first existing path;
                    // shared mount/docs and library roots are cached for this
                    // resolution, without enumerating any directory.
                    if let Some(parent) = path.parent() {
                        exists_checked(parent, probes)?;
                    }
                    Ok(false)
                }
                Err(link_error) => {
                    Err(DegaussError::io("probing artwork source", path, link_error))
                }
            }
        }
        Err(error) => Err(DegaussError::io("probing artwork source", path, error)),
    }?;
    probes.insert(path.to_path_buf(), present);
    Ok(present)
}

pub struct Job {
    result: Option<Receiver<Outcome>>,
    cancelled: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Job {
    pub fn start(systems: Vec<FoundSystem>, settings: Settings) -> Result<Self> {
        let (sender, result) = mpsc::sync_channel(1);
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&cancelled);
        let handle = std::thread::Builder::new()
            .name("degauss-art-source".to_string())
            .spawn(move || {
                let resolved = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    resolve(&systems, &settings, &worker_cancelled)
                }))
                .unwrap_or_else(|_| {
                    Err(DegaussError::unsupported(
                        "artwork source resolution",
                        "source worker panicked",
                    ))
                });
                let _ = sender.send(resolved);
            })
            .map_err(|error| {
                DegaussError::unsupported(
                    "artwork source resolution",
                    format!("starting worker: {error}"),
                )
            })?;
        Ok(Self {
            result: Some(result),
            cancelled,
            handle: Some(handle),
        })
    }

    pub fn try_recv(&mut self) -> Option<Outcome> {
        let result = match self.result.as_ref()?.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => Err(DegaussError::unsupported(
                "artwork source resolution",
                "source worker stopped without returning its result",
            )),
        };
        self.result = None;
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        Some(result)
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancel();
        self.result.take();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
pub(crate) fn resolved_job_at_bases(
    systems: &[FoundSystem],
    settings: &Settings,
    bases: &[PathBuf],
) -> Job {
    let cancelled = Arc::new(AtomicBool::new(false));
    let (sender, result) = mpsc::sync_channel(1);
    sender
        .send(resolve_with_bases(systems, settings, &cancelled, bases))
        .unwrap();
    Job {
        result: Some(result),
        cancelled,
        handle: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let root = std::env::temp_dir().join(format!(
                "degauss-auto-source-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&root).unwrap();
            Self(root)
        }

        fn system(&self, id: &str, paths: &[&str]) -> FoundSystem {
            let def = toml::from_str(&format!(
                "id = {id:?}\nname = {id:?}\nfolders = []\nrbf = '_Console/Test'\nextensions = ['rom']"
            )).unwrap();
            let paths = paths
                .iter()
                .map(|path| {
                    let path = self.0.join(path);
                    std::fs::create_dir_all(&path).unwrap();
                    path
                })
                .collect();
            FoundSystem {
                def,
                paths,
                logo_dir: None,
                menu_folder: None,
            }
        }

        fn pack(&self, base: &str, folder: &str) -> PathBuf {
            let base = self.0.join(base);
            let art = base.join("docs").join(folder).join("Artwork");
            std::fs::create_dir_all(&art).unwrap();
            std::fs::write(
                art.join("manifest.tsv"),
                "#key\tstyle\tss_system_id\nGame\tbox-2D\t105\n",
            )
            .unwrap();
            std::fs::write(art.join("Game.jpg"), b"jpeg").unwrap();
            base
        }

        fn resolve(
            &self,
            systems: &[FoundSystem],
            settings: &Settings,
            bases: &[PathBuf],
        ) -> Resolution {
            resolve_with_bases(systems, settings, &AtomicBool::new(false), bases)
                .unwrap()
                .unwrap()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn any_root_gamelist_presence_keeps_the_entire_group_on_gamelist() {
        let fixture = Fixture::new();
        let systems = [
            fixture.system("NeoGeo", &["first", "extra"]),
            fixture.system("NeoGeoMVS", &["mvs"]),
        ];
        let base = fixture.pack("sd", "NEOGEO");
        for location in [
            systems[0].paths[0].clone(),
            systems[0].paths[1].clone(),
            systems[1].paths[0].clone(),
        ] {
            for content in ["", "<broken", "<gameList/>"] {
                std::fs::write(location.join("gamelist.xml"), content).unwrap();
                let resolved =
                    fixture.resolve(&systems, &Settings::default(), std::slice::from_ref(&base));
                assert!(resolved.roots.is_empty());
                assert!(resolved.errors.is_empty());
                std::fs::remove_file(location.join("gamelist.xml")).unwrap();
            }
        }
        assert_eq!(
            fixture
                .resolve(&systems, &Settings::default(), std::slice::from_ref(&base))
                .roots["NeoGeo"],
            base.join("docs").to_str().unwrap()
        );
    }

    #[test]
    fn explicit_choices_do_not_probe_and_pack_wins_legacy_contradiction() {
        let fixture = Fixture::new();
        let system = fixture.system("SuperGrafx", &["games"]);
        let mut settings = Settings::default();
        settings.gamelist_sources.insert("SuperGrafx".into());
        let broken_base = fixture.0.join("not-directory");
        std::fs::write(&broken_base, b"file").unwrap();
        assert_eq!(mode(&settings, "SuperGrafx"), Mode::Gamelist);
        assert!(fixture
            .resolve(
                std::slice::from_ref(&system),
                &settings,
                std::slice::from_ref(&broken_base)
            )
            .errors
            .is_empty());
        settings
            .artwork_pack_roots
            .insert("SuperGrafx".into(), "/chosen/docs".into());
        settings
            .artwork_pack_roots
            .insert("NES".into(), "/unrelated/docs".into());
        assert_eq!(mode(&settings, "SuperGrafx"), Mode::ArtworkPack);
        let resolved = fixture.resolve(&[system], &settings, &[broken_base]);
        assert_eq!(resolved.roots.len(), 1);
        assert_eq!(resolved.roots["SuperGrafx"], "/chosen/docs");
        assert!(resolved.errors.is_empty());
    }

    #[test]
    fn priority_missing_pack_and_unsupported_systems() {
        let fixture = Fixture::new();
        let systems = [
            fixture.system("SuperGrafx", &["games"]),
            fixture.system("Unknown", &["other"]),
        ];
        let sd = fixture.pack("sd", "SuperGrafx");
        let usb = fixture.pack("usb", "SuperGrafx");
        let resolved = fixture.resolve(&systems, &Settings::default(), &[sd.clone(), usb.clone()]);
        assert_eq!(
            resolved.roots["SuperGrafx"],
            sd.join("docs").to_str().unwrap()
        );
        assert_eq!(resolved.roots.len(), 1);
        let resolved = fixture.resolve(
            &systems,
            &Settings::default(),
            &[fixture.0.join("missing"), usb.clone()],
        );
        assert_eq!(
            resolved.roots["SuperGrafx"],
            usb.join("docs").to_str().unwrap()
        );
        let resolved = fixture.resolve(&systems, &Settings::default(), &[fixture.0.join("absent")]);
        assert!(resolved.roots.is_empty());
        assert!(resolved.errors.is_empty());
    }

    #[test]
    fn broken_candidate_reports_group_error_without_hiding_healthy_group() {
        let fixture = Fixture::new();
        let systems = [
            fixture.system("SuperGrafx", &["games"]),
            fixture.system("NES", &["nes"]),
        ];
        let bad = fixture.0.join("sd");
        std::fs::create_dir_all(bad.join("docs/SuperGrafx/Artwork")).unwrap();
        let usb = fixture.pack("usb", "SuperGrafx");
        fixture.pack("sd", "NES");
        let resolved = fixture.resolve(&systems, &Settings::default(), &[bad.clone(), usb]);
        assert!(!resolved.roots.contains_key("SuperGrafx"));
        assert!(resolved.errors["SuperGrafx"].contains("Invalid"));
        assert_eq!(resolved.roots["NES"], bad.join("docs").to_str().unwrap());
    }

    #[test]
    fn metadata_failure_is_not_a_missing_gamelist_or_pack() {
        let fixture = Fixture::new();
        let mut system = fixture.system("SuperGrafx", &["games"]);
        let file = fixture.0.join("file");
        std::fs::write(&file, b"file").unwrap();
        system.paths.push(file.clone());
        let resolved = fixture.resolve(std::slice::from_ref(&system), &Settings::default(), &[]);
        assert!(resolved.errors["SuperGrafx"].contains("probing artwork source"));
        system.paths.pop();
        let resolved = fixture.resolve(&[system], &Settings::default(), &[file]);
        assert!(resolved.errors["SuperGrafx"].contains("probing artwork source"));
    }

    #[test]
    fn cancellation_discards_the_entire_partial_resolution() {
        let fixture = Fixture::new();
        let systems = [fixture.system("SuperGrafx", &["games"])];
        assert!(
            resolve_with_bases(&systems, &Settings::default(), &AtomicBool::new(true), &[])
                .unwrap()
                .is_none()
        );
        assert!(
            resolve_with_bases(&[], &Settings::default(), &AtomicBool::new(true), &[])
                .unwrap()
                .is_none()
        );
    }

    #[cfg(unix)]
    #[test]
    fn dangling_gamelist_and_artwork_links_are_errors_not_missing_sources() {
        let fixture = Fixture::new();
        let system = fixture.system("SuperGrafx", &["games"]);
        let base = fixture.pack("sd", "SuperGrafx");
        let xml = system.paths[0].join("gamelist.xml");
        std::os::unix::fs::symlink(fixture.0.join("missing.xml"), &xml).unwrap();
        let resolved = fixture.resolve(
            std::slice::from_ref(&system),
            &Settings::default(),
            std::slice::from_ref(&base),
        );
        assert!(resolved.roots.is_empty());
        assert!(resolved.errors["SuperGrafx"].contains("gamelist.xml"));
        std::fs::remove_file(&xml).unwrap();

        let broken_base = fixture.0.join("broken");
        let folder = broken_base.join("docs/SuperGrafx");
        std::fs::create_dir_all(&folder).unwrap();
        std::os::unix::fs::symlink(fixture.0.join("missing-artwork"), folder.join("Artwork"))
            .unwrap();
        let resolved = fixture.resolve(&[system], &Settings::default(), &[broken_base, base]);
        assert!(resolved.roots.is_empty());
        assert!(resolved.errors["SuperGrafx"].contains("Artwork"));
    }

    #[test]
    fn shared_catalogues_preserve_independent_source_choices() {
        let fixture = Fixture::new();
        let systems = [
            fixture.system("GBA", &["gba"]),
            fixture.system("GBA2P", &["gba2p"]),
        ];
        let base = fixture.pack("sd", "GBA");
        let mut settings = Settings::default();
        let resolved = fixture.resolve(&systems, &settings, std::slice::from_ref(&base));
        assert_eq!(resolved.roots["GBA"], resolved.roots["GBA2P"]);
        settings.gamelist_sources.insert("GBA2P".into());
        let resolved = fixture.resolve(&systems, &settings, &[base]);
        assert!(resolved.roots.contains_key("GBA"));
        assert!(!resolved.roots.contains_key("GBA2P"));
        assert!(resolved.errors.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn dangling_intermediate_links_fail_while_ordinary_missing_paths_stay_missing() {
        let fixture = Fixture::new();
        let mut system = fixture.system("SuperGrafx", &["games"]);
        let valid_base = fixture.pack("valid", "SuperGrafx");
        for (base_name, link_suffix) in [
            ("broken-docs", "docs"),
            ("broken-system", "docs/SuperGrafx"),
        ] {
            let base = fixture.0.join(base_name);
            let link = base.join(link_suffix);
            std::fs::create_dir_all(link.parent().unwrap()).unwrap();
            std::os::unix::fs::symlink(fixture.0.join("missing-directory"), &link).unwrap();
            let resolved = fixture.resolve(
                std::slice::from_ref(&system),
                &Settings::default(),
                &[base, valid_base.clone()],
            );
            assert!(resolved.roots.is_empty());
            assert!(resolved.errors["SuperGrafx"].contains(link.to_str().unwrap()));
        }
        let library_link = fixture.0.join("broken-library");
        std::os::unix::fs::symlink(fixture.0.join("missing-library"), &library_link).unwrap();
        system.paths = vec![library_link.clone()];
        let resolved = fixture.resolve(&[system], &Settings::default(), &[valid_base]);
        assert!(resolved.roots.is_empty());
        assert!(resolved.errors["SuperGrafx"].contains(library_link.to_str().unwrap()));

        let mut probes = BTreeMap::new();
        let absent = fixture.0.join("ordinary-missing/docs/SuperGrafx/Artwork");
        assert!(!exists_checked(&absent, &mut probes).unwrap());
        assert_eq!(
            probes.get(&fixture.0.join("ordinary-missing/docs")),
            Some(&false)
        );
        assert_eq!(probes.get(&fixture.0), Some(&true));
        let count = probes.len();
        assert!(!exists_checked(&absent, &mut probes).unwrap());
        assert_eq!(probes.len(), count);
    }

    #[test]
    fn worker_returns_one_result_and_joins() {
        let mut job = Job::start(Vec::new(), Settings::default()).unwrap();
        job.handle.take().unwrap().join().unwrap();
        let resolved = job.try_recv().unwrap().unwrap().unwrap();
        assert!(resolved.roots.is_empty());
        assert!(resolved.errors.is_empty());
        assert!(job.try_recv().is_none());
    }
}
