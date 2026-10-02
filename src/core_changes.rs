//! One acknowledged configured catalogue, held fixed throughout a visit.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{DegaussError, Result};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Catalogue {
    pub checked: u64,
    pub sources: BTreeMap<String, String>,
    pub listings: BTreeMap<String, Listing>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Listing {
    pub source: String,
    pub fingerprint: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Scope {
    #[default]
    WhatsNew,
    UpdatesAvailable,
    AllCores,
}

impl Scope {
    pub const ALL: [Self; 3] = [Self::WhatsNew, Self::UpdatesAvailable, Self::AllCores];
    pub fn label(self) -> &'static str {
        match self {
            Self::WhatsNew => "What's New",
            Self::UpdatesAvailable => "Updates Available",
            Self::AllCores => "All Cores",
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Saved {
    version: u32,
    catalogue: Catalogue,
}

pub fn path(cache_dir: &Path) -> PathBuf {
    cache_dir.join("core-updates-seen.json")
}

pub fn checked_label(checked: u64) -> String {
    let Ok(timestamp) = checked.try_into() else {
        return checked.to_string();
    };
    // SAFETY: gmtime_r writes only the owned tm and reads a valid time_t.
    let mut parts: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::gmtime_r(&timestamp, &mut parts) }.is_null() {
        return format!("Unix time {checked}");
    }
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02} UTC",
        parts.tm_year + 1900,
        parts.tm_mon + 1,
        parts.tm_mday,
        parts.tm_hour,
        parts.tm_min
    )
}

fn validate(catalogue: &Catalogue, path: &Path) -> Result<()> {
    if catalogue.sources.len() > 256
        || catalogue.listings.len() > 30_000
        || catalogue
            .sources
            .iter()
            .any(|(id, fingerprint)| id.is_empty() || id.len() > 512 || fingerprint.len() != 32)
        || catalogue.listings.iter().any(|(key, listing)| {
            key.len() > 1537
                || listing.fingerprint.len() != 32
                || !catalogue.sources.contains_key(&listing.source)
        })
    {
        return Err(DegaussError::malformed(
            "Core Updates history",
            path,
            "invalid or oversized catalogue",
        ));
    }
    Ok(())
}

pub fn load(cache_dir: &Path) -> Result<Option<Catalogue>> {
    let path = path(cache_dir);
    let metadata = match std::fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(DegaussError::io(
                "reading Core Updates history",
                &path,
                error,
            ))
        }
    };
    if metadata.len() > 64 * 1024 * 1024 {
        return Err(DegaussError::malformed(
            "Core Updates history",
            &path,
            "file too large",
        ));
    }
    let bytes = std::fs::read(&path)
        .map_err(|error| DegaussError::io("reading Core Updates history", &path, error))?;
    let saved: Saved = serde_json::from_slice(&bytes).map_err(|error| {
        DegaussError::malformed("Core Updates history", &path, error.to_string())
    })?;
    if saved.version != 1 {
        return Err(DegaussError::malformed(
            "Core Updates history",
            &path,
            "unsupported version",
        ));
    }
    validate(&saved.catalogue, &path)?;
    Ok(Some(saved.catalogue))
}

pub fn save(cache_dir: &Path, catalogue: &Catalogue) -> Result<()> {
    let path = path(cache_dir);
    validate(catalogue, &path)?;
    let bytes = serde_json::to_vec(&Saved {
        version: 1,
        catalogue: catalogue.clone(),
    })
    .map_err(|error| DegaussError::malformed("Core Updates history", &path, error.to_string()))?;
    crate::cache::write(&path, &bytes)
}

#[derive(Default)]
pub struct Visit {
    pub baseline: Option<Catalogue>,
    pub current: Catalogue,
    complete: bool,
    displayed: bool,
}

impl Visit {
    pub fn new(baseline: Option<Catalogue>) -> Self {
        Self {
            baseline,
            ..Default::default()
        }
    }
    pub fn begin_refresh(&mut self) {
        self.complete = false;
        self.displayed = false;
    }
    pub fn observe(&mut self, catalogue: Catalogue, complete: bool) {
        self.current = catalogue;
        self.complete = complete;
        self.displayed = false;
    }
    pub fn presented(&mut self) {
        self.displayed = self.complete;
    }
    pub fn acknowledge(&mut self, cache_dir: &Path) -> Result<()> {
        if self.complete && self.displayed {
            save(cache_dir, &self.current)?;
            self.complete = false;
        }
        Ok(())
    }
    pub fn label(&self, key: &str) -> Option<&'static str> {
        let baseline = self.baseline.as_ref()?;
        let listing = self.current.listings.get(key)?;
        if baseline.sources.get(&listing.source) != self.current.sources.get(&listing.source) {
            return None;
        }
        match baseline.listings.get(key) {
            None => Some("New or changed listing"),
            Some(previous) if previous != listing => Some("Changed listing"),
            Some(_) => None,
        }
    }
    pub fn source_changed(&self) -> bool {
        self.baseline
            .as_ref()
            .is_some_and(|previous| previous.sources != self.current.sources)
    }
    pub fn matches(&self, scope: Scope, item: &crate::misterzine::Item) -> bool {
        match scope {
            Scope::WhatsNew => self.baseline.is_none() || self.label(item.key()).is_some(),
            Scope::UpdatesAvailable => item.update_available(),
            Scope::AllCores => true,
        }
    }
    pub fn message(&self) -> &'static str {
        if self.baseline.is_none() {
            "First check: no previous update history"
        } else if self.source_changed() {
            "Changed source settings: affected sources have no comparison"
        } else {
            "Changes since the last complete check you viewed"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn catalogue(hash: char) -> Catalogue {
        Catalogue {
            checked: 1,
            sources: BTreeMap::from([("source".into(), "1".repeat(32))]),
            listings: BTreeMap::from([(
                "source:core.rbf".into(),
                Listing {
                    source: "source".into(),
                    fingerprint: hash.to_string().repeat(32),
                },
            )]),
        }
    }
    #[test]
    fn fixed_baseline_exact_listings_and_configuration_rebase() {
        let mut visit = Visit::new(Some(catalogue('a')));
        visit.observe(catalogue('b'), true);
        assert_eq!(visit.label("source:core.rbf"), Some("Changed listing"));
        visit.observe(catalogue('b'), true);
        assert!(
            visit.label("source:core.rbf").is_some(),
            "refresh does not erase changes"
        );
        visit.current.listings.insert(
            "source:core_20261001.rbf".into(),
            Listing {
                source: "source".into(),
                fingerprint: "b".repeat(32),
            },
        );
        assert_eq!(
            visit.label("source:core_20261001.rbf"),
            Some("New or changed listing")
        );
        visit
            .current
            .sources
            .insert("source".into(), "2".repeat(32));
        assert!(visit.label("source:core.rbf").is_none());
        assert!(visit.source_changed());
    }
    #[test]
    fn acknowledgement_requires_complete_fresh_result_actually_presented_and_leave() {
        let dir = std::env::temp_dir().join(format!("degauss-core-seen-{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let mut visit = Visit::new(None);
        visit.observe(catalogue('a'), false);
        visit.presented();
        visit.acknowledge(&dir).unwrap();
        assert!(
            load(&dir).unwrap().is_none(),
            "cache and partial results are not fresh history"
        );
        visit.observe(catalogue('a'), true);
        visit.acknowledge(&dir).unwrap();
        assert!(
            load(&dir).unwrap().is_none(),
            "undisplayed success must not be acknowledged"
        );
        visit.presented();
        visit.begin_refresh();
        visit.acknowledge(&dir).unwrap();
        assert!(
            load(&dir).unwrap().is_none(),
            "cancel or failure after refresh cannot advance history"
        );
        visit.observe(catalogue('a'), true);
        visit.presented();
        assert!(
            load(&dir).unwrap().is_none(),
            "display alone does not write"
        );
        visit.acknowledge(&dir).unwrap();
        assert_eq!(load(&dir).unwrap(), Some(catalogue('a')));
        assert!(
            visit.baseline.is_none(),
            "the visit baseline stays fixed even after saving"
        );
        std::fs::write(path(&dir), b"broken").unwrap();
        assert!(
            load(&dir).is_err(),
            "corruption is explicit, not a fresh first visit"
        );
        std::fs::remove_file(path(&dir)).unwrap();
        std::fs::remove_dir(&dir).unwrap();
        std::fs::write(&dir, b"not a directory").unwrap();
        assert!(
            save(&dir, &catalogue('a')).is_err(),
            "a failed history write is an actual error"
        );
        assert_eq!(std::fs::read(&dir).unwrap(), b"not a directory");
        std::fs::remove_file(dir).unwrap();
    }
    #[test]
    fn first_visit_and_scope_preserve_installed_state() {
        let visit = Visit::new(None);
        let current =
            crate::misterzine::Item::fixture("Core", crate::misterzine::LocalState::Current, None);
        let update = crate::misterzine::Item::fixture(
            "Core",
            crate::misterzine::LocalState::UpdateAvailable,
            None,
        );
        assert!(visit.matches(Scope::WhatsNew, &current));
        assert!(visit.label(current.key()).is_none());
        assert!(!visit.matches(Scope::UpdatesAvailable, &current));
        assert!(visit.matches(Scope::UpdatesAvailable, &update));
        assert!(visit.matches(Scope::AllCores, &current));
    }
}
