//! Resolve source choices once per lifecycle, on a process-owned worker.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;
use std::thread::JoinHandle;

use crate::artwork_pack;
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
    /// The Pack root in use, keyed by system: an explicit choice is spelled
    /// out for every member of its group, an Automatic acceptance stands
    /// for the one system that made it.
    pub roots: BTreeMap<String, String>,
    /// Keyed by source group, as the interface reports them.
    pub errors: BTreeMap<String, String>,
}

type Outcome = Result<Option<Resolution>>;

/// Where an Automatic Pack is looked for when a system is entered: SD, then
/// USB0 through USB7, in that order.
pub fn production_bases() -> Vec<PathBuf> {
    std::iter::once(PathBuf::from("/media/fat"))
        .chain((0..=7).map(|index| PathBuf::from(format!("/media/usb{index}"))))
        .collect()
}

/// The roots in use at startup, from what was saved and what was decided
/// before: explicit choices from the settings, Automatic acceptances from
/// each system's state file. Nothing is probed for a Pack and no Pack is
/// read here: an Automatic system that was never entered has no state and
/// costs nothing, and one that was accepted keeps its root unless a
/// `gamelist.xml` has appeared under its group's folders since.
pub fn resolve(
    systems: &[FoundSystem],
    settings: &Settings,
    cache_dir: &Path,
    cancelled: &AtomicBool,
) -> Outcome {
    let mut resolved = Resolution::default();
    let mut accepted: BTreeMap<&str, Vec<(&FoundSystem, String)>> = BTreeMap::new();
    for system in systems {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let Some(group) = artwork_pack::source_group(&system.def.id) else {
            continue;
        };
        if let Some(root) = settings.artwork_pack_roots.get(group) {
            resolved.roots.insert(system.def.id.clone(), root.clone());
        } else if mode(settings, &system.def.id) == Mode::Automatic {
            // A state file that cannot be read is the group's problem to
            // show, as an unreadable gamelist probe is: what was decided
            // for the system is unknown, not undecided.
            let state = match crate::cache::load_pack_source_state(cache_dir, &system.def.id) {
                Ok(Some(state)) => state,
                Ok(None) => continue,
                Err(error) => {
                    record_error(&mut resolved.errors, group, &error);
                    continue;
                }
            };
            if let Some(root) = state.accepted.map(|accepted| accepted.docs_root) {
                accepted.entry(group).or_default().push((system, root));
            }
        }
    }
    // A root gamelist keeps the whole group on Gamelist, accepted or not:
    // the members of a group share their folders on the card.
    let mut probes = BTreeMap::new();
    for (group, members) in accepted {
        let gamelist = (|| -> Result<Option<bool>> {
            let mut checked = BTreeSet::new();
            for system in systems {
                if artwork_pack::source_group(&system.def.id) != Some(group) {
                    continue;
                }
                for root in &system.paths {
                    if cancelled.load(Ordering::Relaxed) {
                        return Ok(None);
                    }
                    if checked.insert(root)
                        && exists_checked(&root.join("gamelist.xml"), &mut probes)?
                    {
                        return Ok(Some(true));
                    }
                }
            }
            Ok(Some(false))
        })();
        match gamelist {
            Ok(Some(true)) => {}
            Ok(Some(false)) => {
                for (system, root) in members {
                    resolved.roots.insert(system.def.id.clone(), root);
                }
            }
            Ok(None) => return Ok(None),
            Err(error) => record_error(&mut resolved.errors, group, &error),
        }
    }
    Ok((!cancelled.load(Ordering::Relaxed)).then_some(resolved))
}

/// A group's errors, one under the other: a member's unreadable state
/// and the group's gamelist probe can both fail, and the second is not
/// allowed to write over the first.
fn record_error(errors: &mut BTreeMap<String, String>, group: &str, error: &DegaussError) {
    errors
        .entry(group.to_string())
        .and_modify(|recorded| {
            recorded.push('\n');
            recorded.push_str(&error.to_string());
        })
        .or_insert_with(|| error.to_string());
}

/// Whether any of the system's own folders holds a `gamelist.xml`. A
/// dangling link or an unreadable folder is an error, never a missing
/// gamelist: it must not let a Pack in by default.
pub fn gamelist_present(system: &FoundSystem) -> Result<bool> {
    let mut probes = BTreeMap::new();
    for root in &system.paths {
        if exists_checked(&root.join("gamelist.xml"), &mut probes)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The installed Pack a system would use under Automatic, if one is
/// there: the first base, in priority order, whose `docs` holds a mapped
/// Artwork directory of the system. Only those exact directories are
/// stat'd; nothing under a base is listed and nothing in the Pack is read.
pub fn candidate_root(system_id: &str, bases: &[PathBuf]) -> Result<Option<PathBuf>> {
    let folders = artwork_pack::expected_folders(system_id);
    let mut probes = BTreeMap::new();
    for base in bases {
        let docs = base.join("docs");
        for folder in folders {
            if exists_checked(&docs.join(folder).join("Artwork"), &mut probes)? {
                return Ok(Some(docs));
            }
        }
    }
    Ok(None)
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
    pub fn start(
        systems: Vec<FoundSystem>,
        settings: Settings,
        cache_dir: PathBuf,
    ) -> Result<Self> {
        let (sender, result) = mpsc::sync_channel(1);
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&cancelled);
        let handle = std::thread::Builder::new()
            .name("degauss-art-source".to_string())
            .spawn(move || {
                let resolved = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    resolve(&systems, &settings, &cache_dir, &worker_cancelled)
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

        fn cache_dir(&self) -> PathBuf {
            self.0.join("cache")
        }

        fn accept(&self, id: &str, root: &Path) {
            crate::cache::save_pack_source_state(
                &self.cache_dir(),
                id,
                &crate::cache::PackSourceState {
                    accepted: Some(crate::cache::AcceptedSource {
                        docs_root: root.to_string_lossy().into_owned(),
                        language: None,
                        signature: None,
                        cache_marker: 0,
                        health: artwork_pack::ProviderHealth::Ready,
                        diagnostics: Vec::new(),
                        skipped_entries: 0,
                    }),
                    declined: None,
                },
            )
            .unwrap();
        }

        fn resolve(&self, systems: &[FoundSystem], settings: &Settings) -> Resolution {
            super::resolve(
                systems,
                settings,
                &self.cache_dir(),
                &AtomicBool::new(false),
            )
            .unwrap()
            .unwrap()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }

    /// Startup reads decisions, not Packs: an installed Pack nobody has
    /// accepted is not found, not opened and not held against any system,
    /// however many are installed. The Pack directory is made unreadable
    /// to prove nothing under it is even stat'd.
    #[cfg(unix)]
    #[test]
    fn startup_reads_only_recorded_decisions_and_never_probes_a_pack() {
        use std::os::unix::fs::PermissionsExt;
        let fixture = Fixture::new();
        let systems = [
            fixture.system("SuperGrafx", &["games"]),
            fixture.system("NES", &["nes"]),
        ];
        let sd = fixture.pack("sd", "SuperGrafx");
        fixture.pack("sd", "NES");
        let docs = sd.join("docs");
        std::fs::set_permissions(&docs, std::fs::Permissions::from_mode(0o000)).unwrap();
        let resolved = fixture.resolve(&systems, &Settings::default());
        assert!(
            resolved.roots.is_empty() && resolved.errors.is_empty(),
            "an installed Pack without a decision is nothing to the startup: {resolved:?}"
        );

        fixture.accept("NES", &docs);
        let resolved = fixture.resolve(&systems, &Settings::default());
        std::fs::set_permissions(&docs, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            resolved.roots.get("NES").map(String::as_str),
            Some(docs.to_str().unwrap()),
            "an accepted root is known from its state file alone"
        );
        assert!(
            !resolved.roots.contains_key("SuperGrafx"),
            "the system that was never entered gets no root from its neighbour's Pack"
        );
        assert!(resolved.errors.is_empty());
    }

    /// A gamelist that appears after acceptance wins, as it does before:
    /// the accepted root is not used while the group's folders hold one,
    /// and a broken gamelist link is an error rather than a missing file.
    #[cfg(unix)]
    #[test]
    fn a_root_gamelist_keeps_an_accepted_group_on_gamelist() {
        let fixture = Fixture::new();
        let systems = [
            fixture.system("NeoGeo", &["first", "extra"]),
            fixture.system("NeoGeoMVS", &["mvs"]),
        ];
        let docs = fixture.pack("sd", "NEOGEO").join("docs");
        fixture.accept("NeoGeo", &docs);
        assert_eq!(
            fixture
                .resolve(&systems, &Settings::default())
                .roots
                .keys()
                .collect::<Vec<_>>(),
            ["NeoGeo"],
            "the other member of the group has made no decision of its own"
        );
        for location in [
            systems[0].paths[0].clone(),
            systems[0].paths[1].clone(),
            systems[1].paths[0].clone(),
        ] {
            std::fs::write(location.join("gamelist.xml"), "<gameList/>").unwrap();
            let resolved = fixture.resolve(&systems, &Settings::default());
            assert!(resolved.roots.is_empty(), "{resolved:?}");
            assert!(resolved.errors.is_empty());
            std::fs::remove_file(location.join("gamelist.xml")).unwrap();
        }
        let xml = systems[1].paths[0].join("gamelist.xml");
        std::os::unix::fs::symlink(fixture.0.join("missing.xml"), &xml).unwrap();
        let resolved = fixture.resolve(&systems, &Settings::default());
        assert!(resolved.roots.is_empty());
        assert!(resolved.errors["NeoGeo"].contains("gamelist.xml"));
    }

    /// An accepted system whose state file is there but cannot be read
    /// is reported for its group, not started as undecided: reading it as
    /// no state would drop the accepted root and let the next entry ask a
    /// question whose answer writes over the decision that still exists.
    #[test]
    fn an_unreadable_state_file_is_the_groups_error_not_no_decision() {
        let fixture = Fixture::new();
        let systems = [fixture.system("NeoGeo", &["first"])];
        let docs = fixture.pack("sd", "NEOGEO").join("docs");
        fixture.accept("NeoGeo", &docs);
        let state_path = crate::cache::artwork_pack_source_path(&fixture.cache_dir(), "NeoGeo");
        std::fs::remove_file(&state_path).unwrap();
        std::fs::create_dir(&state_path).unwrap();
        let resolved = fixture.resolve(&systems, &Settings::default());
        assert!(resolved.roots.is_empty());
        assert!(
            resolved.errors["NeoGeo"].contains("reading the Artwork Pack state"),
            "{resolved:?}"
        );
        // The other member accepted and its gamelist probe failing as
        // well: both causes reach the group's report, neither writes over
        // the other.
        #[cfg(unix)]
        {
            let systems = [systems[0].clone(), fixture.system("NeoGeoMVS", &["second"])];
            fixture.accept("NeoGeoMVS", &docs);
            let xml = systems[1].paths[0].join("gamelist.xml");
            std::os::unix::fs::symlink(fixture.0.join("missing.xml"), &xml).unwrap();
            let resolved = fixture.resolve(&systems, &Settings::default());
            assert!(resolved.roots.is_empty());
            let error = &resolved.errors["NeoGeo"];
            assert!(
                error.contains("reading the Artwork Pack state") && error.contains("gamelist.xml"),
                "{resolved:?}"
            );
        }
    }

    /// An explicit choice is spelled out for every member of its group and
    /// probes nothing; a Pack choice beats a stale Gamelist entry.
    #[test]
    fn explicit_choices_do_not_probe_and_pack_wins_legacy_contradiction() {
        let fixture = Fixture::new();
        let systems = [
            fixture.system("GBA", &["gba"]),
            fixture.system("GBA2P", &["gba2p"]),
        ];
        let mut settings = Settings::default();
        settings.gamelist_sources.insert("GBA".into());
        assert_eq!(mode(&settings, "GBA"), Mode::Gamelist);
        let resolved = fixture.resolve(&systems, &settings);
        assert!(resolved.roots.is_empty());
        assert!(resolved.errors.is_empty());
        settings
            .artwork_pack_roots
            .insert("GBA".into(), "/chosen/docs".into());
        settings
            .artwork_pack_roots
            .insert("NES".into(), "/unrelated/docs".into());
        assert_eq!(mode(&settings, "GBA"), Mode::ArtworkPack);
        let resolved = fixture.resolve(&systems, &settings);
        assert_eq!(resolved.roots["GBA"], "/chosen/docs");
        assert!(
            !resolved.roots.contains_key("GBA2P"),
            "a shared catalogue is not a shared choice"
        );
        assert_eq!(resolved.roots.len(), 1);
        assert!(resolved.errors.is_empty());
    }

    /// The Neo Geo pair shares one saved choice: both members get it.
    #[test]
    fn a_shared_group_choice_is_spelled_out_for_every_member() {
        let fixture = Fixture::new();
        let systems = [
            fixture.system("NeoGeo", &["neo"]),
            fixture.system("NeoGeoMVS", &["mvs"]),
        ];
        let mut settings = Settings::default();
        settings
            .artwork_pack_roots
            .insert("NeoGeo".into(), "/chosen/docs".into());
        let resolved = fixture.resolve(&systems, &settings);
        assert_eq!(resolved.roots["NeoGeo"], "/chosen/docs");
        assert_eq!(resolved.roots["NeoGeoMVS"], "/chosen/docs");
    }

    /// The candidate a system is asked about: SD before USB, only the
    /// mapped Artwork directories looked at, a missing base passed over.
    #[test]
    fn candidate_root_follows_the_base_priority_and_looks_only_at_mapped_folders() {
        let fixture = Fixture::new();
        let sd = fixture.pack("sd", "SuperGrafx");
        let usb = fixture.pack("usb", "SuperGrafx");
        assert_eq!(
            candidate_root("SuperGrafx", &[sd.clone(), usb.clone()]).unwrap(),
            Some(sd.join("docs"))
        );
        assert_eq!(
            candidate_root("SuperGrafx", &[fixture.0.join("missing"), usb.clone()]).unwrap(),
            Some(usb.join("docs"))
        );
        assert_eq!(
            candidate_root("SuperGrafx", &[fixture.0.join("absent")]).unwrap(),
            None
        );
        assert_eq!(
            candidate_root("NES", &[sd.clone(), usb]).unwrap(),
            None,
            "another system's Pack is not this system's candidate"
        );
        assert_eq!(candidate_root("Unknown", &[sd]).unwrap(), None);
    }

    /// A candidate is found by its directory, not by reading it: a Pack
    /// whose manifest is broken is still the candidate, so the question is
    /// asked and the preparation is what fails, visibly.
    #[test]
    fn candidate_root_does_not_read_the_pack() {
        let fixture = Fixture::new();
        let base = fixture.0.join("sd");
        std::fs::create_dir_all(base.join("docs/SuperGrafx/Artwork")).unwrap();
        std::fs::write(
            base.join("docs/SuperGrafx/Artwork/manifest.tsv"),
            "not-a-valid-manifest\n",
        )
        .unwrap();
        assert_eq!(
            candidate_root("SuperGrafx", std::slice::from_ref(&base)).unwrap(),
            Some(base.join("docs"))
        );
    }

    /// A dangling link where the Pack or the gamelist should be is an
    /// error: installed source material whose target is gone must not let
    /// another source in.
    #[cfg(unix)]
    #[test]
    fn dangling_gamelist_and_artwork_links_are_errors_not_missing_sources() {
        let fixture = Fixture::new();
        let system = fixture.system("SuperGrafx", &["games"]);
        let xml = system.paths[0].join("gamelist.xml");
        std::os::unix::fs::symlink(fixture.0.join("missing.xml"), &xml).unwrap();
        let error = gamelist_present(&system).unwrap_err();
        assert!(error.to_string().contains("gamelist.xml"), "{error}");
        std::fs::remove_file(&xml).unwrap();
        assert!(!gamelist_present(&system).unwrap());
        std::fs::write(&xml, "<gameList/>").unwrap();
        assert!(gamelist_present(&system).unwrap());

        let broken_base = fixture.0.join("broken");
        let folder = broken_base.join("docs/SuperGrafx");
        std::fs::create_dir_all(&folder).unwrap();
        std::os::unix::fs::symlink(fixture.0.join("missing-artwork"), folder.join("Artwork"))
            .unwrap();
        let valid = fixture.pack("valid", "SuperGrafx");
        let error = candidate_root("SuperGrafx", &[broken_base, valid]).unwrap_err();
        assert!(error.to_string().contains("Artwork"), "{error}");
    }

    #[test]
    fn metadata_failure_is_not_a_missing_gamelist() {
        let fixture = Fixture::new();
        let mut system = fixture.system("SuperGrafx", &["games"]);
        let file = fixture.0.join("file");
        std::fs::write(&file, b"file").unwrap();
        system.paths.push(file.clone());
        let error = gamelist_present(&system).unwrap_err();
        assert!(error.to_string().contains("probing artwork source"));
        let error = candidate_root("SuperGrafx", &[file]).unwrap_err();
        assert!(error.to_string().contains("probing artwork source"));
    }

    #[test]
    fn cancellation_discards_the_entire_partial_resolution() {
        let fixture = Fixture::new();
        let systems = [fixture.system("SuperGrafx", &["games"])];
        assert!(super::resolve(
            &systems,
            &Settings::default(),
            &fixture.cache_dir(),
            &AtomicBool::new(true)
        )
        .unwrap()
        .is_none());
        assert!(super::resolve(
            &[],
            &Settings::default(),
            &fixture.cache_dir(),
            &AtomicBool::new(true)
        )
        .unwrap()
        .is_none());
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
            let error = candidate_root("SuperGrafx", &[base, valid_base.clone()]).unwrap_err();
            assert!(error.to_string().contains(link.to_str().unwrap()));
        }
        let library_link = fixture.0.join("broken-library");
        std::os::unix::fs::symlink(fixture.0.join("missing-library"), &library_link).unwrap();
        system.paths = vec![library_link.clone()];
        let error = gamelist_present(&system).unwrap_err();
        assert!(error.to_string().contains(library_link.to_str().unwrap()));

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
        let fixture = Fixture::new();
        let mut job = Job::start(Vec::new(), Settings::default(), fixture.cache_dir()).unwrap();
        job.handle.take().unwrap().join().unwrap();
        let resolved = job.try_recv().unwrap().unwrap().unwrap();
        assert!(resolved.roots.is_empty());
        assert!(resolved.errors.is_empty());
        assert!(job.try_recv().is_none());
    }
}
