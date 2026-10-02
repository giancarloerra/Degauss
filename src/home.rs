//! Personal organisation references existing targets; it never owns their files.
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Target {
    Folder {
        children: Vec<String>,
    },
    Category {
        category: String,
    },
    System {
        system: String,
    },
    LibraryFolder {
        system: String,
        trail: Vec<crate::browse::Place>,
    },
    Game {
        system: String,
        launch: crate::browse::Launch,
    },
    Collection {
        collection: String,
    },
    Core {
        path: PathBuf,
    },
    Script {
        path: PathBuf,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub name: String,
    pub target: Target,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Store {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub entries: BTreeMap<String, Entry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub order: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub hidden: BTreeSet<String>,
    #[serde(default)]
    next: u64,
}

pub fn entry_key(id: &str) -> String {
    format!("entry:{id}")
}
pub fn category_key(category: &str) -> String {
    format!("category:{category}")
}

impl Store {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty() && self.order.is_empty() && self.hidden.is_empty() && self.next == 0
    }

    pub fn rows(
        &self,
        folder: Option<&str>,
        categories: &[String],
        show_hidden: bool,
    ) -> Vec<String> {
        let mut rows = match folder {
            Some(id) => match self.entries.get(id).map(|entry| &entry.target) {
                Some(Target::Folder { children }) => {
                    children.iter().map(|id| entry_key(id)).collect()
                }
                _ => Vec::new(),
            },
            None => {
                let mut rows: Vec<_> = self
                    .order
                    .iter()
                    .filter(|key| {
                        if let Some(id) = key.strip_prefix("entry:") {
                            self.entries.contains_key(id)
                        } else {
                            key.strip_prefix("category:")
                                .is_some_and(|name| categories.iter().any(|c| c == name))
                        }
                    })
                    .cloned()
                    .collect();
                for category in categories {
                    let key = category_key(category);
                    if !rows.contains(&key) {
                        rows.push(key);
                    }
                }
                rows
            }
        };
        if !show_hidden {
            rows.retain(|key| !self.hidden.contains(key));
        }
        rows
    }

    pub fn parent(&self, id: &str) -> Option<String> {
        self.entries
            .iter()
            .find_map(|(parent, entry)| match &entry.target {
                Target::Folder { children } if children.iter().any(|child| child == id) => {
                    Some(parent.clone())
                }
                _ => None,
            })
    }

    pub fn add(&mut self, entry: Entry, parent: Option<&str>) -> Result<String, String> {
        let mut changed = self.clone();
        changed.next = changed
            .next
            .checked_add(1)
            .ok_or("Home entry IDs are exhausted.")?;
        let id = format!("{}", changed.next);
        if changed.entries.contains_key(&id) {
            return Err("Home entry ID already exists.".into());
        }
        changed.entries.insert(id.clone(), entry);
        changed.attach(&id, parent)?;
        changed.validate()?;
        *self = changed;
        Ok(id)
    }

    fn attach(&mut self, id: &str, parent: Option<&str>) -> Result<(), String> {
        match parent {
            None => {
                self.order.push(entry_key(id));
            }
            Some(parent) => match self.entries.get_mut(parent).map(|entry| &mut entry.target) {
                Some(Target::Folder { children }) => children.push(id.into()),
                _ => return Err("The destination personal folder no longer exists.".into()),
            },
        }
        Ok(())
    }

    pub fn move_to(&mut self, id: &str, parent: Option<&str>) -> Result<(), String> {
        if !self.entries.contains_key(id) {
            return Err("The Home entry no longer exists.".into());
        }
        let mut changed = self.clone();
        changed.detach(id);
        changed.attach(id, parent)?;
        changed.validate()?;
        *self = changed;
        Ok(())
    }

    fn detach(&mut self, id: &str) {
        self.order.retain(|key| key != &entry_key(id));
        for entry in self.entries.values_mut() {
            if let Target::Folder { children } = &mut entry.target {
                children.retain(|child| child != id);
            }
        }
    }

    pub fn remove(&mut self, id: &str) {
        self.detach(id);
        self.hidden.remove(&entry_key(id));
        if let Some(Entry {
            target: Target::Folder { children },
            ..
        }) = self.entries.remove(id)
        {
            for child in children {
                self.remove(&child);
            }
        }
    }

    pub fn reorder(&mut self, folder: Option<&str>, keys: &[String]) {
        match folder {
            None => self.order = keys.to_vec(),
            Some(id) => {
                if let Some(Entry {
                    target: Target::Folder { children },
                    ..
                }) = self.entries.get_mut(id)
                {
                    *children = keys
                        .iter()
                        .filter_map(|key| key.strip_prefix("entry:").map(str::to_string))
                        .collect();
                }
            }
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        let mut visited = BTreeSet::new();
        for key in &self.order {
            if let Some(id) = key.strip_prefix("entry:") {
                self.visit(id, 0, &mut visited)?;
            }
        }
        if visited.len() != self.entries.len() {
            return Err("Home contains an unattached entry or a folder cycle.".into());
        }
        Ok(())
    }

    fn visit(&self, id: &str, depth: usize, visited: &mut BTreeSet<String>) -> Result<(), String> {
        if !visited.insert(id.into()) {
            return Err(
                "A personal folder cannot contain itself, a descendant or the same entry twice."
                    .into(),
            );
        }
        let entry = self
            .entries
            .get(id)
            .ok_or("A personal folder contains a missing entry.")?;
        if entry.name.trim().is_empty() {
            return Err("Home entries need a name.".into());
        }
        if let Target::Folder { children } = &entry.target {
            if depth >= 3 {
                return Err("Personal folders can be nested only three levels below Home.".into());
            }
            for child in children {
                self.visit(child, depth + 1, visited)?;
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Resume {
    pub folder: Option<String>,
    pub key: String,
    #[serde(default)]
    pub anchor: Option<Box<crate::state::State>>,
}

pub struct PreviewSource {
    pub system: String,
    pub pack: bool,
    pub provider: Option<crate::artwork_pack::Provider>,
    pub targets: Vec<(String, crate::browse::Launch)>,
}

#[derive(Debug)]
pub struct Preview {
    pub rows: BTreeMap<String, crate::browse::Row>,
    pub missing: Vec<String>,
}

pub struct PreviewJob {
    receiver: std::sync::mpsc::Receiver<Result<Preview, String>>,
    cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl PreviewJob {
    pub fn start(cache_dir: PathBuf, sources: Vec<PreviewSource>) -> Result<Self, String> {
        let (sender, receiver) = std::sync::mpsc::channel();
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = cancelled.clone();
        std::thread::Builder::new()
            .name("degauss-home-art".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    read_previews(&cache_dir, sources, &flag)
                }))
                .unwrap_or_else(|_| Err("Home artwork worker panicked.".into()));
                let _ = sender.send(result);
            })
            .map_err(|error| format!("Starting Home artwork reader: {error}"))?;
        Ok(Self {
            receiver,
            cancelled,
        })
    }
    pub fn poll(&self) -> Option<Result<Preview, String>> {
        match self.receiver.try_recv() {
            Ok(result) => Some(result),
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                Some(Err("Home artwork worker stopped without a result.".into()))
            }
        }
    }
}

impl Drop for PreviewJob {
    fn drop(&mut self) {
        self.cancelled
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

fn read_previews(
    cache_dir: &std::path::Path,
    sources: Vec<PreviewSource>,
    cancelled: &std::sync::atomic::AtomicBool,
) -> Result<Preview, String> {
    let mut preview = Preview {
        rows: BTreeMap::new(),
        missing: Vec::new(),
    };
    for source in sources {
        if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
            return Err("Home artwork reading cancelled.".into());
        }
        let cache = if source.pack {
            crate::cache::load_artwork_pack_data_checked(cache_dir, &source.system)
                .map(|data| data.map(|data| data.cache))
        } else {
            crate::cache::load_system_checked(cache_dir, &source.system)
        }
        .map_err(|error| format!("Home artwork for {}: {error}", source.system))?;
        for (key, launch) in source.targets {
            if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
                return Err("Home artwork reading cancelled.".into());
            }
            let row = cache.as_ref().and_then(|cache| {
                cache
                    .folders
                    .values()
                    .flat_map(|folder| &folder.rows)
                    .find(|row| row.kind == crate::browse::Kind::Play(launch.clone()))
                    .cloned()
            });
            if let Some(mut row) = row {
                if let Some(provider) = &source.provider {
                    provider.apply_prepared(std::slice::from_mut(&mut row));
                }
                row.details.desc.clear();
                preview.rows.insert(key, row);
            } else {
                preview.missing.push(key);
            }
        }
    }
    Ok(preview)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn folder(name: &str) -> Entry {
        Entry {
            name: name.into(),
            image: None,
            target: Target::Folder {
                children: Vec::new(),
            },
        }
    }
    #[test]
    fn default_order_and_hidden_home_do_not_change_library_categories() {
        let mut store = Store::default();
        let categories = vec!["Arcade".into(), "Console".into()];
        assert_eq!(
            store.rows(None, &categories, false),
            vec![category_key("Arcade"), category_key("Console")]
        );
        let id = store.add(folder("Friday"), None).unwrap();
        assert_eq!(store.rows(None, &categories, false)[0], entry_key(&id));
        store.hidden.insert(category_key("Arcade"));
        assert_eq!(store.rows(None, &categories, false).len(), 2);
        assert_eq!(store.rows(None, &categories, true).len(), 3);
        assert_eq!(categories, vec!["Arcade", "Console"]);
    }
    #[test]
    fn subtree_moves_preserve_images_and_reject_cycles_or_depth_atomically() {
        let mut store = Store::default();
        let a = store.add(folder("a"), None).unwrap();
        let b = store.add(folder("b"), Some(&a)).unwrap();
        let c = store.add(folder("c"), Some(&b)).unwrap();
        store.entries.get_mut(&a).unwrap().image = Some("/logos/a.png".into());
        let original = store.clone();
        assert!(store.move_to(&a, Some(&c)).is_err());
        assert_eq!(store, original);
        assert!(store.add(folder("d"), Some(&c)).is_err());
        assert_eq!(store, original);
        store.move_to(&b, None).unwrap();
        assert_eq!(store.parent(&c), Some(b));
        assert_eq!(store.entries[&a].image, Some("/logos/a.png".into()));
    }
    #[test]
    fn exact_targets_persist_and_removal_only_removes_organisation() {
        let mut store = Store::default();
        let folder = store.add(folder("Friday"), None).unwrap();
        let path = PathBuf::from("/usb/games/pack.zip/game.rom");
        store
            .add(
                Entry {
                    name: "Same title".into(),
                    image: None,
                    target: Target::Game {
                        system: "NES".into(),
                        launch: crate::browse::Launch::File(path),
                    },
                },
                Some(&folder),
            )
            .unwrap();
        let encoded = toml::to_string(&store).unwrap();
        assert_eq!(toml::from_str::<Store>(&encoded).unwrap(), store);
        store.remove(&folder);
        assert!(store.entries.is_empty());
    }

    #[test]
    fn preview_reads_exact_cached_targets_and_preserves_real_read_failures() {
        let dir = std::env::temp_dir().join(format!("degauss-home-preview-{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let launch = crate::browse::Launch::File(dir.join("example.nes"));
        let row = crate::browse::Row {
            name: "Cached title".into(),
            sort_key: "cached".into(),
            kind: crate::browse::Kind::Play(launch.clone()),
            cover: Some(dir.join("art.png")),
            genre: None,
            favorite: false,
            below: None,
            details: crate::browse::Details {
                desc: "Full description is read on demand".into(),
                ..Default::default()
            },
        };
        let mut cache = crate::cache::SystemCache {
            format: 1,
            ..Default::default()
        };
        cache.folders.insert(
            "d:/games".into(),
            crate::cache::Folder {
                rows: vec![row],
                games: 1,
                mtime: 0,
            },
        );
        crate::cache::save_system(&dir, "NES", &cache).unwrap();
        let sources = || {
            vec![PreviewSource {
                system: "NES".into(),
                pack: false,
                provider: None,
                targets: vec![("entry:1".into(), launch.clone())],
            }]
        };
        let flag = std::sync::atomic::AtomicBool::new(false);
        let preview = read_previews(&dir, sources(), &flag).unwrap();
        assert_eq!(preview.rows["entry:1"].name, "Cached title");
        assert!(preview.rows["entry:1"].details.desc.is_empty());
        std::fs::write(crate::cache::system_path(&dir, "NES"), b"invalid cache").unwrap();
        assert!(read_previews(&dir, sources(), &flag)
            .unwrap_err()
            .contains("NES"));
        flag.store(true, std::sync::atomic::Ordering::Relaxed);
        assert!(read_previews(&dir, sources(), &flag)
            .unwrap_err()
            .contains("cancelled"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
