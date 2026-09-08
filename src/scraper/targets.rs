use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::browse::{DisplayNames, Kind, Launch, Library, Place, MAX_DEPTH};
use crate::systems::FoundSystem;

use super::{platforms, Error, ErrorKind, Result};

#[derive(Debug, Clone)]
pub enum Scope {
    All,
    System {
        system_id: String,
        place: Place,
        display_name: String,
    },
    Folder {
        system_id: String,
        place: Place,
        display_name: String,
    },
    Game {
        system_id: String,
        launch: Launch,
        title: String,
    },
}

impl Scope {
    pub fn label(&self) -> &str {
        match self {
            Self::All => "All Systems",
            Self::System { display_name, .. } | Self::Folder { display_name, .. } => display_name,
            Self::Game { title, .. } => title,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub system_id: String,
    /// Every Degauss system whose cache reads this same game and gamelist.
    /// Two-player aliases often share both a root and a ScreenScraper id.
    pub affected_system_ids: Vec<String>,
    pub system_name: String,
    pub screen_scraper_system_id: u32,
    pub title: String,
    pub query: String,
    pub match_path: Option<PathBuf>,
    pub folder: PathBuf,
    pub relative_path: String,
    /// Only a single supported ZIP member can inherit archive-level metadata.
    pub metadata_fallback: bool,
}

impl Target {
    pub fn gamelist_path(&self) -> PathBuf {
        self.folder.join("gamelist.xml")
    }

    pub fn file_name(&self) -> &str {
        self.match_path
            .as_deref()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .unwrap_or(&self.query)
    }

    fn entry_key(&self, roots: &mut HashMap<PathBuf, PathBuf>) -> (PathBuf, String) {
        // MiSTer storage is not necessarily FAT. Preserve case so two real
        // files on a case-sensitive USB or network filesystem cannot be
        // collapsed, while resolving root symlinks so two configured aliases
        // of the same physical gamelist are updated only once. Cache this per
        // system root so a large or remote library pays for one filesystem
        // resolution rather than one for every game.
        let root = roots
            .entry(self.folder.clone())
            .or_insert_with(|| {
                std::fs::canonicalize(&self.folder).unwrap_or_else(|_| self.folder.clone())
            })
            .clone();
        (root, normalise_rel(&self.relative_path))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetIssue {
    pub system: String,
    pub detail: String,
}

#[derive(Debug, Clone, Default)]
pub struct TargetBatch {
    pub targets: Vec<Target>,
    /// Systems deliberately omitted because their selected data source is
    /// Artwork Pack. These are expected skips, not unsupported systems or
    /// failures.
    pub skipped_artwork_pack: Vec<String>,
    pub unsupported_systems: Vec<String>,
    pub issues: Vec<TargetIssue>,
    pub ambiguous_targets: usize,
}

pub fn collect(
    systems: &[FoundSystem],
    names: &DisplayNames,
    scope: &Scope,
    overrides: &BTreeMap<String, u32>,
    artwork_pack_system_ids: &HashSet<String>,
    cancelled: &AtomicBool,
    on_progress: &mut dyn FnMut(&str),
) -> Result<TargetBatch> {
    check_cancelled(cancelled)?;
    let excluded = |system_id: &str| {
        artwork_pack_system_ids
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(system_id))
    };
    let mut batch = match scope {
        Scope::All => collect_all(
            systems,
            names,
            overrides,
            artwork_pack_system_ids,
            cancelled,
            on_progress,
        ),
        Scope::System {
            system_id, place, ..
        }
        | Scope::Folder {
            system_id, place, ..
        } => {
            let system = find_system(systems, system_id)?;
            if excluded(system_id) {
                return Ok(TargetBatch {
                    skipped_artwork_pack: vec![system.def.name.clone()],
                    ..Default::default()
                });
            }
            collect_system(
                system,
                names,
                place.clone(),
                overrides,
                cancelled,
                on_progress,
            )
        }
        Scope::Game {
            system_id,
            launch,
            title,
        } => {
            let system = find_system(systems, system_id)?;
            if excluded(system_id) {
                return Ok(TargetBatch {
                    skipped_artwork_pack: vec![system.def.name.clone()],
                    ..Default::default()
                });
            }
            let platform = platform_id(system, overrides)?;
            on_progress(&system.def.name);
            check_cancelled(cancelled)?;
            Ok(TargetBatch {
                targets: vec![target_for(
                    system,
                    platform,
                    launch,
                    title,
                    &mut crate::zip::ArchiveCache::default(),
                )?],
                ..Default::default()
            })
        }
    }?;
    expand_affected_systems(&mut batch.targets, systems, artwork_pack_system_ids);
    Ok(batch)
}

/// Include every system cache that reads the same physical gamelist.
///
/// Global collection can discover aliases while deduplicating targets, but
/// system, folder and game scopes start from only one system. Without this
/// expansion, another alias of the same root keeps stale metadata until a
/// manual rebuild even though its on-disk gamelist was already changed.
fn expand_affected_systems(
    targets: &mut [Target],
    systems: &[FoundSystem],
    artwork_pack_system_ids: &HashSet<String>,
) {
    let mut readers: HashMap<PathBuf, Vec<String>> = HashMap::new();
    for system in systems {
        if is_favorites_target(system)
            || artwork_pack_system_ids
                .iter()
                .any(|id| id.eq_ignore_ascii_case(&system.def.id))
        {
            continue;
        }
        for path in &system.paths {
            let root = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
            let ids = readers.entry(root).or_default();
            if !ids.contains(&system.def.id) {
                ids.push(system.def.id.clone());
            }
        }
    }
    for target in targets {
        let root = std::fs::canonicalize(&target.folder).unwrap_or_else(|_| target.folder.clone());
        let Some(ids) = readers.get(&root) else {
            continue;
        };
        for id in ids {
            if !target.affected_system_ids.contains(id) {
                target.affected_system_ids.push(id.clone());
            }
        }
    }
}

fn collect_all(
    systems: &[FoundSystem],
    names: &DisplayNames,
    overrides: &BTreeMap<String, u32>,
    artwork_pack_system_ids: &HashSet<String>,
    cancelled: &AtomicBool,
    on_progress: &mut dyn FnMut(&str),
) -> Result<TargetBatch> {
    let mut batch = TargetBatch::default();
    for system in systems {
        check_cancelled(cancelled)?;
        if is_favorites_target(system) {
            continue;
        }
        if artwork_pack_system_ids
            .iter()
            .any(|id| id.eq_ignore_ascii_case(&system.def.id))
        {
            batch.skipped_artwork_pack.push(system.def.name.clone());
            continue;
        }
        on_progress(&system.def.name);
        check_cancelled(cancelled)?;
        let Some(platform) = platforms::id_for(&system.def.id, overrides) else {
            batch.unsupported_systems.push(system.def.name.clone());
            continue;
        };
        let config = system.to_config();
        let library = match Library::open_with_names(&config, names.clone()) {
            Ok(library) => library,
            Err(error) => {
                batch.issues.push(TargetIssue {
                    system: system.def.name.clone(),
                    detail: error.to_string(),
                });
                continue;
            }
        };
        let mut walked = walk(system, platform, &library, library.start(), cancelled)?;
        batch.targets.append(&mut walked.targets);
        batch.issues.append(&mut walked.issues);
    }
    remove_ambiguous(&mut batch);
    batch.skipped_artwork_pack.sort();
    batch.skipped_artwork_pack.dedup();
    Ok(batch)
}

fn collect_system(
    system: &FoundSystem,
    names: &DisplayNames,
    place: Place,
    overrides: &BTreeMap<String, u32>,
    cancelled: &AtomicBool,
    on_progress: &mut dyn FnMut(&str),
) -> Result<TargetBatch> {
    let platform = platform_id(system, overrides)?;
    on_progress(&system.def.name);
    check_cancelled(cancelled)?;
    let library =
        Library::open_with_names(&system.to_config(), names.clone()).map_err(|error| {
            Error::local(format!(
                "could not read {} before scraping: {error}",
                system.def.name
            ))
        })?;
    let walked = walk(system, platform, &library, place, cancelled)?;
    Ok(TargetBatch {
        targets: walked.targets,
        issues: walked.issues,
        ..Default::default()
    })
}

#[derive(Default)]
struct Walked {
    targets: Vec<Target>,
    issues: Vec<TargetIssue>,
}

fn walk(
    system: &FoundSystem,
    platform: u32,
    library: &Library,
    start: Place,
    cancelled: &AtomicBool,
) -> Result<Walked> {
    let mut walked = Walked::default();
    let mut archives = crate::zip::ArchiveCache::default();
    let mut queue = VecDeque::from([(start, 0usize)]);
    let mut seen = HashSet::new();
    while let Some((place, depth)) = queue.pop_front() {
        check_cancelled(cancelled)?;
        if depth > MAX_DEPTH {
            walked.issues.push(TargetIssue {
                system: system.def.name.clone(),
                detail: format!(
                    "{} is deeper than Degauss's {MAX_DEPTH}-level scan limit",
                    place.path().display()
                ),
            });
            continue;
        }
        if !seen.insert(place.key()) {
            continue;
        }
        let (rows, _) = match library.list(&place, true) {
            Ok(listing) => listing,
            Err(error) => {
                walked.issues.push(TargetIssue {
                    system: system.def.name.clone(),
                    detail: format!(
                        "could not enumerate {} while preparing the scrape: {error}",
                        place.path().display()
                    ),
                });
                continue;
            }
        };
        for row in rows {
            check_cancelled(cancelled)?;
            match &row.kind {
                Kind::Enter(next) => queue.push_back((next.clone(), depth + 1)),
                Kind::Play(launch) => {
                    match target_for(system, platform, launch, &row.name, &mut archives) {
                        Ok(target) => walked.targets.push(target),
                        Err(error) => walked.issues.push(TargetIssue {
                            system: system.def.name.clone(),
                            detail: error.to_string(),
                        }),
                    }
                }
            }
        }
    }
    Ok(walked)
}

fn check_cancelled(cancelled: &AtomicBool) -> Result<()> {
    if cancelled.load(Ordering::Relaxed) {
        Err(Error::new(ErrorKind::Cancelled, "scrape cancelled"))
    } else {
        Ok(())
    }
}

fn target_for(
    system: &FoundSystem,
    platform: u32,
    launch: &Launch,
    title: &str,
    archives: &mut crate::zip::ArchiveCache,
) -> Result<Target> {
    let (folder, relative_path, match_path, query) = match launch {
        Launch::File(path) => {
            let folder = root_for(system, path).ok_or_else(|| {
                Error::local(format!(
                    "{} is outside every configured folder for {}",
                    path.display(),
                    system.def.name
                ))
            })?;
            let relative = path.strip_prefix(&folder).map_err(|_| {
                Error::local(format!(
                    "could not make {} relative to {}",
                    path.display(),
                    folder.display()
                ))
            })?;
            let relative_path = dot_relative(relative);
            let match_path = hashable(path).then(|| path.clone());
            let query = query_for(title, path, &system.def.extensions);
            (folder, relative_path, match_path, query)
        }
        Launch::AmigaVision { install, title } => {
            let folder = root_for(system, install).ok_or_else(|| {
                Error::local(format!(
                    "the AmigaVision install is outside every configured folder for {}",
                    system.def.name
                ))
            })?;
            (folder, format!("./Games/{title}"), None, title.to_string())
        }
    };
    let metadata_fallback = if let Launch::File(path) = launch {
        if let Some((archive, _)) = crate::zip::split_member_path(path) {
            let contents = archives
                .read(&archive)
                .map_err(|error| Error::local(error.to_string()))?;
            let config = system.to_config();
            contents
                .entries
                .iter()
                .filter(|entry| config.accepts(Path::new(&entry.name)))
                .count()
                == 1
        } else {
            true
        }
    } else {
        true
    };
    Ok(Target {
        metadata_fallback,
        system_id: system.def.id.clone(),
        affected_system_ids: vec![system.def.id.clone()],
        system_name: system.def.name.clone(),
        screen_scraper_system_id: platform,
        title: title.to_string(),
        query,
        match_path,
        folder,
        relative_path,
    })
}

fn find_system<'a>(systems: &'a [FoundSystem], id: &str) -> Result<&'a FoundSystem> {
    systems
        .iter()
        .find(|system| system.def.id == id)
        .ok_or_else(|| Error::new(ErrorKind::Configuration, format!("unknown system {id}")))
}

// Retain the reserved-ID exclusion for older custom tables without a category.
pub(super) fn is_favorites_target(system: &FoundSystem) -> bool {
    crate::systems::is_favorites(system.category())
        || system.def.id.eq_ignore_ascii_case("Favorites")
}

fn platform_id(system: &FoundSystem, overrides: &BTreeMap<String, u32>) -> Result<u32> {
    if is_favorites_target(system) {
        return Err(Error::new(
            ErrorKind::Configuration,
            "the master Favourites shelf is not a scrape target",
        ));
    }
    platforms::id_for(&system.def.id, overrides).ok_or_else(|| {
        Error::new(
            ErrorKind::Configuration,
            format!(
                "{} has no reviewed ScreenScraper platform mapping",
                system.def.name
            ),
        )
    })
}

fn root_for(system: &FoundSystem, path: &Path) -> Option<PathBuf> {
    system
        .paths
        .iter()
        .filter(|root| path.starts_with(root))
        .max_by_key(|root| root.as_os_str().len())
        .cloned()
}

fn hashable(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    !matches!(
        path.extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| extension.to_ascii_lowercase())
            .as_deref(),
        Some("cdi" | "chd" | "cue" | "iso" | "mra" | "mgl" | "neo" | "rbf" | "zip")
    )
}

fn query_for(title: &str, path: &Path, extensions: &[String]) -> String {
    let trimmed = title.trim();
    let lower = trimmed.to_ascii_lowercase();
    for extension in extensions {
        let suffix = format!(".{extension}");
        if lower.ends_with(&suffix.to_ascii_lowercase()) {
            return without_dump_tags(&trimmed[..trimmed.len() - suffix.len()]);
        }
    }
    let query = if trimmed.is_empty() {
        path.file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default()
    } else {
        trimmed.to_string()
    };
    let cleaned = without_dump_tags(&query);
    if cleaned.is_empty() {
        query
    } else {
        cleaned
    }
}

fn without_dump_tags(value: &str) -> String {
    let mut cleaned = String::with_capacity(value.len());
    let mut depth = 0usize;
    for character in value.chars() {
        match character {
            '(' | '[' => {
                depth += 1;
                if depth == 1 {
                    cleaned.push(' ');
                }
            }
            ')' | ']' => depth = depth.saturating_sub(1),
            _ if depth == 0 => cleaned.push(character),
            _ => {}
        }
    }
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn dot_relative(path: &Path) -> String {
    format!("./{}", path.to_string_lossy().replace('\\', "/"))
}

fn normalise_rel(value: &str) -> String {
    value
        .trim()
        .replace('\\', "/")
        .trim_start_matches("./")
        .trim_start_matches('/')
        .to_string()
}

fn remove_ambiguous(batch: &mut TargetBatch) {
    let mut roots = HashMap::new();
    let mut platforms: HashMap<(PathBuf, String), u32> = HashMap::new();
    let mut ambiguous = HashSet::new();
    for target in &batch.targets {
        let key = target.entry_key(&mut roots);
        match platforms.insert(key.clone(), target.screen_scraper_system_id) {
            Some(existing) if existing != target.screen_scraper_system_id => {
                ambiguous.insert(key);
            }
            _ => {}
        }
    }
    batch.ambiguous_targets = ambiguous.len();
    batch
        .targets
        .retain(|target| !ambiguous.contains(&target.entry_key(&mut roots)));

    let mut merged: Vec<Target> = Vec::new();
    let mut positions: HashMap<((PathBuf, String), u32), usize> = HashMap::new();
    for target in std::mem::take(&mut batch.targets) {
        let key = (
            target.entry_key(&mut roots),
            target.screen_scraper_system_id,
        );
        if let Some(existing) = positions.get(&key).copied() {
            merged[existing].metadata_fallback &= target.metadata_fallback;
            for system_id in target.affected_system_ids {
                if !merged[existing].affected_system_ids.contains(&system_id) {
                    merged[existing].affected_system_ids.push(system_id);
                }
            }
        } else {
            positions.insert(key, merged.len());
            merged.push(target);
        }
    }
    batch.targets = merged;
    batch.unsupported_systems.sort();
    batch.unsupported_systems.dedup();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect_now(
        systems: &[FoundSystem],
        names: &DisplayNames,
        scope: &Scope,
        overrides: &BTreeMap<String, u32>,
    ) -> Result<TargetBatch> {
        let cancelled = AtomicBool::new(false);
        collect(
            systems,
            names,
            scope,
            overrides,
            &HashSet::new(),
            &cancelled,
            &mut |_| {},
        )
    }

    fn temp(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "degauss-scraper-targets-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn system(id: &str, name: &str, root: &Path, extensions: &[&str]) -> FoundSystem {
        let folders = root
            .file_name()
            .map(|name| vec![name.to_string_lossy().into_owned()])
            .unwrap_or_default();
        let text = format!(
            "name = {name:?}\nid = {id:?}\nfolders = {folders:?}\nrbf = '_Console/Test'\nextensions = {:?}\n",
            extensions
        );
        FoundSystem {
            def: toml::from_str(&text).unwrap(),
            paths: vec![root.to_path_buf()],
            logo_dir: None,
            menu_folder: Some("Console".into()),
        }
    }

    #[test]
    fn a_system_walk_uses_real_browser_rows_and_nested_paths() {
        let root = temp("nested");
        std::fs::create_dir_all(root.join("RPG")).unwrap();
        std::fs::write(root.join("One.rom"), b"one").unwrap();
        std::fs::write(root.join("RPG/Two.rom"), b"two").unwrap();
        let system = system("NES", "Nintendo", &root, &["rom"]);
        let batch = collect_now(
            std::slice::from_ref(&system),
            &DisplayNames::default(),
            &Scope::System {
                system_id: "NES".into(),
                place: Place::Dir(root.clone()),
                display_name: "Nintendo".into(),
            },
            &BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(batch.targets.len(), 2);
        assert!(batch
            .targets
            .iter()
            .any(|target| { target.relative_path == "./RPG/Two.rom" && target.query == "Two" }));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn enumeration_reports_the_current_system_and_honours_cancellation() {
        let root = temp("cancelled-enumeration");
        std::fs::write(root.join("One.rom"), b"one").unwrap();
        let system = system("NES", "Nintendo", &root, &["rom"]);
        let cancelled = AtomicBool::new(false);
        let mut visited = Vec::new();
        let error = collect(
            &[system],
            &DisplayNames::default(),
            &Scope::All,
            &BTreeMap::new(),
            &HashSet::new(),
            &cancelled,
            &mut |name| {
                visited.push(name.to_string());
                cancelled.store(true, Ordering::Relaxed);
            },
        )
        .unwrap_err();
        assert_eq!(visited, vec!["Nintendo"]);
        assert_eq!(error.kind, ErrorKind::Cancelled);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_folder_scope_does_not_escape_to_its_parent() {
        let root = temp("scope");
        std::fs::create_dir_all(root.join("Only")).unwrap();
        std::fs::write(root.join("Outside.rom"), b"outside").unwrap();
        std::fs::write(root.join("Only/Inside.rom"), b"inside").unwrap();
        let system = system("NES", "Nintendo", &root, &["rom"]);
        let batch = collect_now(
            std::slice::from_ref(&system),
            &DisplayNames::default(),
            &Scope::Folder {
                system_id: "NES".into(),
                place: Place::Dir(root.join("Only")),
                display_name: "Only".into(),
            },
            &BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(batch.targets.len(), 1);
        assert_eq!(batch.targets[0].relative_path, "./Only/Inside.rom");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn amiga_vision_uses_the_existing_special_gamelist_key() {
        let root = temp("amigavision");
        let system = system("Amiga", "Amiga", &root, &["hdf"]);
        let batch = collect_now(
            std::slice::from_ref(&system),
            &DisplayNames::default(),
            &Scope::Game {
                system_id: "Amiga".into(),
                launch: Launch::AmigaVision {
                    install: root.clone(),
                    title: "Zool 2 (AGA)[en]".into(),
                },
                title: "Zool 2 (AGA)[en]".into(),
            },
            &BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(batch.targets[0].relative_path, "./Games/Zool 2 (AGA)[en]");
        assert!(batch.targets[0].match_path.is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn neo_geo_aliases_share_one_request_and_refresh_every_shared_cache() {
        let root = temp("ambiguous");
        std::fs::write(root.join("Game.neo"), b"game").unwrap();
        let neo = system("NeoGeo", "Neo Geo", &root, &["neo"]);
        let mvs = system("NeoGeoMVS", "Neo Geo MVS", &root, &["neo"]);
        let unknown = system("FutureSystem", "Future", &root, &["neo"]);
        let batch = collect_now(
            &[neo, mvs, unknown],
            &DisplayNames::default(),
            &Scope::All,
            &BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(batch.targets.len(), 1);
        assert_eq!(batch.ambiguous_targets, 0);
        assert_eq!(
            batch.targets[0].affected_system_ids,
            vec![
                "NeoGeo".to_string(),
                "NeoGeoMVS".to_string(),
                "FutureSystem".to_string(),
            ]
        );
        assert_eq!(batch.unsupported_systems, vec!["Future"]);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn same_platform_aliases_share_one_request_and_refresh_both_caches() {
        let root = temp("same-platform");
        std::fs::write(root.join("Tune.nsf"), b"tune").unwrap();
        let nes = system("NES", "NES", &root, &["nsf"]);
        let music = system("NESMusic", "NES Music", &root, &["nsf"]);
        let batch = collect_now(
            &[nes, music],
            &DisplayNames::default(),
            &Scope::All,
            &BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(batch.targets.len(), 1);
        assert_eq!(
            batch.targets[0].affected_system_ids,
            vec!["NES".to_string(), "NESMusic".to_string()]
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_pack_alias_never_enters_the_gamelist_refresh_set() {
        let root = temp("pack-shared-root");
        std::fs::write(root.join("Tune.nsf"), b"tune").unwrap();
        let nes = system("NES", "NES", &root, &["nsf"]);
        let music = system("NESMusic", "NES Music", &root, &["nsf"]);
        let cancelled = AtomicBool::new(false);

        let batch = collect(
            &[nes, music],
            &DisplayNames::default(),
            &Scope::All,
            &BTreeMap::new(),
            &HashSet::from(["NES".to_string()]),
            &cancelled,
            &mut |_| {},
        )
        .unwrap();

        assert_eq!(batch.skipped_artwork_pack, vec!["NES"]);
        assert_eq!(batch.targets.len(), 1);
        assert_eq!(batch.targets[0].system_id, "NESMusic");
        assert_eq!(batch.targets[0].affected_system_ids, vec!["NESMusic"]);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_scoped_scrape_refreshes_every_system_that_reads_the_same_gamelist() {
        let root = temp("scoped-shared-root");
        let game = root.join("Game.rom");
        std::fs::write(&game, b"game").unwrap();
        let nes = system("NES", "NES", &root, &["rom"]);
        let genesis = system("Genesis", "Genesis", &root, &["rom"]);

        let batch = collect_now(
            &[nes, genesis],
            &DisplayNames::default(),
            &Scope::Game {
                system_id: "NES".into(),
                launch: Launch::File(game),
                title: "Game".into(),
            },
            &BTreeMap::new(),
        )
        .unwrap();

        assert_eq!(batch.targets.len(), 1);
        assert_eq!(batch.targets[0].screen_scraper_system_id, 3);
        assert_eq!(batch.targets[0].affected_system_ids, vec!["NES", "Genesis"]);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn case_sensitive_storage_keeps_distinct_paths_distinct() {
        let target = |folder: &str, system: &str| Target {
            metadata_fallback: true,
            system_id: system.into(),
            affected_system_ids: vec![system.into()],
            system_name: system.into(),
            screen_scraper_system_id: 3,
            title: "Game".into(),
            query: "Game".into(),
            match_path: None,
            folder: PathBuf::from(folder),
            relative_path: "./Game.rom".into(),
        };
        let mut batch = TargetBatch {
            targets: vec![
                target("/unmounted/Games", "One"),
                target("/unmounted/games", "Two"),
            ],
            ..Default::default()
        };

        remove_ambiguous(&mut batch);

        assert_eq!(batch.targets.len(), 2);
        assert_eq!(batch.ambiguous_targets, 0);
    }

    #[cfg(unix)]
    #[test]
    fn root_symlink_aliases_share_one_physical_target() {
        let parent = temp("symlink-alias");
        let root = parent.join("real");
        let alias = parent.join("alias");
        std::fs::create_dir(&root).unwrap();
        std::os::unix::fs::symlink(&root, &alias).unwrap();
        let target = |folder: PathBuf, system: &str| Target {
            metadata_fallback: true,
            system_id: system.into(),
            affected_system_ids: vec![system.into()],
            system_name: system.into(),
            screen_scraper_system_id: 3,
            title: "Game".into(),
            query: "Game".into(),
            match_path: None,
            folder,
            relative_path: "./Game.rom".into(),
        };
        let mut batch = TargetBatch {
            targets: vec![target(root, "One"), target(alias, "Two")],
            ..Default::default()
        };

        remove_ambiguous(&mut batch);

        assert_eq!(batch.targets.len(), 1);
        assert_eq!(batch.targets[0].affected_system_ids, vec!["One", "Two"]);
        let _ = std::fs::remove_dir_all(parent);
    }

    #[test]
    fn scope_labels_name_the_actual_scrape_target() {
        assert_eq!(Scope::All.label(), "All Systems");
        assert_eq!(
            Scope::System {
                system_id: "NES".into(),
                place: Place::Roots,
                display_name: "Nintendo Entertainment System".into(),
            }
            .label(),
            "Nintendo Entertainment System"
        );
        assert_eq!(
            Scope::Folder {
                system_id: "NES".into(),
                place: Place::Roots,
                display_name: "Platform Games".into(),
            }
            .label(),
            "Platform Games"
        );
        assert_eq!(
            Scope::Game {
                system_id: "NES".into(),
                launch: Launch::File(PathBuf::from("Game.nes")),
                title: "The Actual Game".into(),
            }
            .label(),
            "The Actual Game"
        );
    }

    #[test]
    fn name_fallback_removes_dump_tags_without_losing_the_title() {
        assert_eq!(
            query_for(
                "Super Game (USA) [Rev 1].rom",
                Path::new("Super Game (USA) [Rev 1].rom"),
                &["rom".into()]
            ),
            "Super Game"
        );
    }

    #[test]
    fn containers_descriptors_and_wrappers_never_use_the_file_hash_path() {
        let root = temp("non-hashable");
        for (name, extension) in [
            ("Arcade.mra", "mra"),
            ("Disc.cdi", "cdi"),
            ("Disc.chd", "chd"),
            ("Disc.cue", "cue"),
            ("Disc.iso", "iso"),
            ("Neo Geo.neo", "neo"),
            ("Playlist.mgl", "mgl"),
            ("Core.rbf", "rbf"),
            ("Archive.zip", "zip"),
        ] {
            let path = root.join(name);
            std::fs::write(&path, b"not the underlying ROM bytes").unwrap();
            assert!(!hashable(&path), "{extension} unexpectedly used hashes");
        }
        let ordinary = root.join("Game.rom");
        std::fs::write(&ordinary, b"rom bytes").unwrap();
        assert!(hashable(&ordinary));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn renamed_favourites_are_excluded_from_all_scopes_and_shared_refreshes() {
        let root = temp("renamed-favorites");
        let game = root.join("Game.rom");
        std::fs::write(&game, b"game").unwrap();
        for category in ["Favorites", "favorites", "FAVORITES"] {
            let mut favorite = system("MyShelf", "My shelf", &root, &["rom"]);
            favorite.menu_folder = None;
            favorite.def.category = Some(category.into());
            let ordinary = system("NES", "Nintendo", &root, &["rom"]);
            let systems = [ordinary, favorite];
            let overrides = BTreeMap::from([("MyShelf".into(), 3)]);
            let batch =
                collect_now(&systems, &DisplayNames::default(), &Scope::All, &overrides).unwrap();
            assert_eq!(batch.targets.len(), 1);
            assert_eq!(batch.targets[0].affected_system_ids, vec!["NES"]);
            assert!(batch.unsupported_systems.is_empty());
            for scope in [
                Scope::System {
                    system_id: "MyShelf".into(),
                    place: Place::Dir(root.clone()),
                    display_name: "My shelf".into(),
                },
                Scope::Folder {
                    system_id: "MyShelf".into(),
                    place: Place::Dir(root.clone()),
                    display_name: "My shelf".into(),
                },
                Scope::Game {
                    system_id: "MyShelf".into(),
                    launch: Launch::File(game.clone()),
                    title: "Game".into(),
                },
            ] {
                let error = collect_now(&systems, &DisplayNames::default(), &scope, &overrides)
                    .unwrap_err();
                assert_eq!(error.kind, ErrorKind::Configuration);
                assert!(error.to_string().contains("not a scrape target"));
            }
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_master_favourites_shelf_is_never_a_write_target() {
        let root = temp("favorites");
        let favorite = system("Favorites", "Favourites", &root, &["mgl"]);
        let error = collect_now(
            &[favorite],
            &DisplayNames::default(),
            &Scope::System {
                system_id: "Favorites".into(),
                place: Place::Dir(root.clone()),
                display_name: "Favourites".into(),
            },
            &BTreeMap::new(),
        )
        .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Configuration);
        let _ = std::fs::remove_dir_all(root);
    }
}
