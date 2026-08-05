// Zaparoo Frontend
// Copyright (c) 2026 Wizzo Pty Ltd and the Zaparoo Project contributors.
// SPDX-License-Identifier: LicenseRef-PolyForm-Noncommercial-1.0.0

use crate::models::{global_handle, global_store};
use cxx_qt::{CxxQtType, Initialize, Threading};
use cxx_qt_lib::QString;
use std::collections::HashSet;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tracing::warn;
use zaparoo_core::endpoints::run::RunMutation;
use zaparoo_core::media_types::{
    BrowseEntry, MediaBrowseParams, MediaItem, MediaMetaParams, MediaSearchParams, RunParams,
};

const ARCADE_SYSTEM_ID: &str = "Arcade";
const MAX_ALT_RESULTS: u32 = 64;

#[derive(Default)]
pub struct AlternateVersionsRust {
    count: i32,
    loading: bool,
    error_message: QString,
    entries: Vec<MediaItem>,
    seq: Arc<AtomicU64>,
}

#[cxx_qt::bridge]
pub mod ffi {
    unsafe extern "C++" {
        include!("model_includes.h");

        type QString = cxx_qt_lib::QString;
    }

    unsafe extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        #[qproperty(i32, count, READ, NOTIFY)]
        #[qproperty(bool, loading, READ, NOTIFY)]
        #[qproperty(QString, error_message, READ, NOTIFY)]
        type AlternateVersions = super::AlternateVersionsRust;

        #[qinvokable]
        fn discover_for(
            self: Pin<&mut AlternateVersions>,
            system_id: QString,
            name: QString,
            path: QString,
        );

        #[qinvokable]
        fn name_at(self: &AlternateVersions, index: i32) -> QString;

        #[qinvokable]
        fn launch_at(self: Pin<&mut AlternateVersions>, index: i32);
    }

    impl cxx_qt::Threading for AlternateVersions {}
    impl cxx_qt::Initialize for AlternateVersions {}
}

impl Initialize for ffi::AlternateVersions {
    fn initialize(self: Pin<&mut Self>) {}
}

impl ffi::AlternateVersions {
    #[allow(
        clippy::needless_pass_by_value,
        reason = "cxx-qt qinvokable signatures use by-value QString"
    )]
    fn discover_for(mut self: Pin<&mut Self>, system_id: QString, name: QString, path: QString) {
        let system_id = system_id.to_string();
        let name = name.to_string();
        let path = path.to_string();
        let ticket = self.rust().seq.fetch_add(1, Ordering::SeqCst) + 1;
        self.as_mut().rust_mut().entries.clear();
        self.as_mut().rust_mut().count = 0;
        self.as_mut().count_changed();
        self.as_mut().rust_mut().error_message = QString::default();
        self.as_mut().error_message_changed();
        self.as_mut().rust_mut().loading = true;
        self.as_mut().loading_changed();
        let qt_thread = self.qt_thread();
        let seq = self.rust().seq.clone();
        let store = global_store();
        global_handle().spawn(async move {
            let client = store.client();
            let result = discover_alternate_versions(
                &client,
                system_id.as_str(),
                name.as_str(),
                path.as_str(),
            )
            .await;
            let _ = qt_thread.queue(move |mut model| {
                if seq.load(Ordering::SeqCst) != ticket {
                    return;
                }
                match result {
                    Ok(entries) => {
                        let count = entries.len() as i32;
                        model.as_mut().rust_mut().entries = entries;
                        model.as_mut().rust_mut().count = count;
                        model.as_mut().count_changed();
                        model.as_mut().rust_mut().error_message = QString::default();
                        model.as_mut().error_message_changed();
                    }
                    Err(message) => {
                        warn!("discover alternate versions failed: {message}");
                        model.as_mut().rust_mut().entries.clear();
                        model.as_mut().rust_mut().count = 0;
                        model.as_mut().count_changed();
                        model.as_mut().rust_mut().error_message = QString::from(message.as_str());
                        model.as_mut().error_message_changed();
                    }
                }
                model.as_mut().rust_mut().loading = false;
                model.as_mut().loading_changed();
            });
        });
    }

    fn name_at(&self, index: i32) -> QString {
        if index < 0 || index >= self.count {
            return QString::default();
        }
        let entry = &self.entries[index as usize];
        let label = file_stem_or_name(&entry.path);
        QString::from(label.as_str())
    }

    fn launch_at(self: Pin<&mut Self>, index: i32) {
        if index < 0 || index >= self.count {
            return;
        }
        let entry = &self.entries[index as usize];
        let text = if entry.path.trim().is_empty() {
            entry.zap_script.clone()
        } else {
            entry.path.clone()
        };
        if text.is_empty() {
            return;
        }
        let name = entry.name.clone();
        let store = global_store();
        global_handle().spawn(async move {
            if let Err(e) = store.run_mutation::<RunMutation>(RunParams { text }).await {
                warn!("run failed for alternate {name}: {}", e.message);
            }
        });
    }
}

async fn discover_alternate_versions(
    client: &zaparoo_core::client::Client,
    system_id: &str,
    name: &str,
    selected_path: &str,
) -> Result<Vec<MediaItem>, String> {
    if system_id != ARCADE_SYSTEM_ID || name.trim().is_empty() || selected_path.trim().is_empty() {
        return Ok(Vec::new());
    }
    let meta_params = MediaMetaParams::for_media(system_id.to_string(), selected_path.to_string());
    let canonical_media = match crate::media_meta_db::media_meta_async(meta_params.clone()).await {
        Some(media) => Some(media),
        None => client
            .media_meta(meta_params)
            .await
            .ok()
            .map(|result| result.media),
    };
    let canonical_title = canonical_media
        .map(|media| media.title.name.trim().to_string())
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| name.trim().to_string());
    let result = client
        .media_search(MediaSearchParams {
            query: Some(canonical_title.clone()),
            systems: vec![ARCADE_SYSTEM_ID.to_string()],
            max_results: Some(MAX_ALT_RESULTS),
            ..MediaSearchParams::default()
        })
        .await
        .map_err(|e| e.message)?;
    let selected_norm = normalize_seed_title(&canonical_title);
    let mut alternate_folders = HashSet::new();
    for entry in result.results {
        if !is_alternate_candidate(&entry, selected_path) {
            continue;
        }
        if normalize_seed_title(&entry.name) != selected_norm {
            continue;
        }
        if let Some(folder) = alternate_folder_path(&entry.path) {
            alternate_folders.insert(folder);
        }
    }
    if alternate_folders.is_empty() {
        return Ok(Vec::new());
    }
    let mut seen_paths = HashSet::new();
    let mut discovered = Vec::new();
    for folder in alternate_folders {
        let page = client
            .media_browse(MediaBrowseParams {
                path: folder,
                systems: vec![ARCADE_SYSTEM_ID.to_string()],
                max_results: Some(1000),
                ..MediaBrowseParams::default()
            })
            .await
            .map_err(|e| e.message)?;
        for entry in page.entries {
            if entry.is_folder() || entry.path.is_empty() || entry.path == selected_path {
                continue;
            }
            if !seen_paths.insert(entry.path.clone()) {
                continue;
            }
            discovered.push(media_item_from_browse_entry(entry));
        }
    }
    Ok(discovered)
}

fn is_alternate_candidate(entry: &MediaItem, selected_path: &str) -> bool {
    if entry.path.is_empty() || entry.path == selected_path {
        return false;
    }
    entry.path.contains("_alternat")
        || entry
            .relative_path
            .as_deref()
            .is_some_and(|path| path.contains("_alternat"))
}

fn alternate_folder_path(path: &str) -> Option<String> {
    let trimmed = path.trim_matches('/');
    let parts: Vec<&str> = trimmed.split('/').filter(|part| !part.is_empty()).collect();
    for (index, part) in parts.iter().enumerate() {
        if !part.contains("_alternat") {
            continue;
        }
        let folder_end = if index + 2 <= parts.len() {
            index + 2
        } else {
            index + 1
        };
        return Some(format!("/{}", parts[..folder_end].join("/")));
    }
    None
}

fn media_item_from_browse_entry(entry: BrowseEntry) -> MediaItem {
    MediaItem {
        media_id: entry.media_id,
        name: entry.name,
        path: entry.path,
        zap_script: entry.zap_script,
        system: zaparoo_core::media_types::System {
            id: entry.system_id,
            ..Default::default()
        },
        tags: entry.tags,
        disambiguating_tags: entry.disambiguating_tags,
        relative_path: if entry.relative_path.is_empty() {
            None
        } else {
            Some(entry.relative_path)
        },
    }
}

fn normalize_name(value: &str) -> String {
    value
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .flat_map(char::to_lowercase)
        .collect()
}

fn normalize_seed_title(value: &str) -> String {
    let mut trimmed = value.trim();
    loop {
        let candidate = trimmed.trim_end();
        if let Some(prefix) = strip_trailing_group(candidate, '(', ')') {
            trimmed = prefix;
            continue;
        }
        if let Some(prefix) = strip_trailing_group(candidate, '[', ']') {
            trimmed = prefix;
            continue;
        }
        break;
    }
    normalize_name(trimmed)
}

fn strip_trailing_group(value: &str, open: char, close: char) -> Option<&str> {
    let value = value.trim_end();
    if !value.ends_with(close) {
        return None;
    }
    let mut depth = 0usize;
    for (index, ch) in value.char_indices().rev() {
        if ch == close {
            depth += 1;
        } else if ch == open {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                return Some(value[..index].trim_end());
            }
        }
    }
    None
}

fn file_stem_or_name(path: &str) -> String {
    let file = path
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default();
    let stem = file.rsplit_once('.').map_or(file, |(stem, _)| stem).trim();
    if stem.is_empty() {
        path.trim().to_string()
    } else {
        stem.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        alternate_folder_path, file_stem_or_name, is_alternate_candidate,
        media_item_from_browse_entry, normalize_name, normalize_seed_title,
    };
    use zaparoo_core::media_types::{BrowseEntry, MediaItem};

    fn media_item(name: &str, path: &str, relative_path: Option<&str>) -> MediaItem {
        MediaItem {
            name: name.into(),
            path: path.into(),
            relative_path: relative_path.map(str::to_string),
            ..Default::default()
        }
    }

    #[test]
    fn normalize_name_strips_non_alnum_and_case() {
        assert_eq!(normalize_name("Bubble Bobble!"), "bubblebobble");
        assert_eq!(normalize_name("Bubble-Bobble"), "bubblebobble");
    }

    #[test]
    fn normalize_seed_title_drops_trailing_variant_groups() {
        assert_eq!(normalize_seed_title("Bubble Bobble"), "bubblebobble");
        assert_eq!(
            normalize_seed_title("Bubble Bobble (Japan)"),
            "bubblebobble"
        );
        assert_eq!(
            normalize_seed_title("Bubble Bobble [Bootleg]"),
            "bubblebobble"
        );
        assert_eq!(
            normalize_seed_title("Bubble Bobble Lost Cave"),
            "bubblebobblelostcave"
        );
    }

    #[test]
    fn alternate_folder_path_returns_deepest_alternate_folder() {
        assert_eq!(
            alternate_folder_path(
                "/media/fat/_Arcade/_alternatives/Bubble Bobble/Bubble Bobble.mra"
            ),
            Some("/media/fat/_Arcade/_alternatives/Bubble Bobble".into())
        );
    }

    #[test]
    fn alternate_folder_path_returns_none_when_missing_marker() {
        assert_eq!(
            alternate_folder_path("/media/fat/_Arcade/Bubble Bobble/Bubble Bobble.mra"),
            None
        );
    }

    #[test]
    fn alternate_candidate_accepts_alternate_path_and_rejects_selected() {
        assert!(is_alternate_candidate(
            &media_item(
                "Bubble Bobble",
                "/media/fat/_Arcade/_alternatives/Bubble Bobble/Bubble Bobble (Japan).mra",
                Some("_alternatives/Bubble Bobble/Bubble Bobble (Japan).mra"),
            ),
            "/media/fat/_Arcade/Bubble Bobble.mra",
        ));
        assert!(!is_alternate_candidate(
            &media_item(
                "Bubble Bobble",
                "/media/fat/_Arcade/Bubble Bobble.mra",
                Some("Bubble Bobble.mra"),
            ),
            "/media/fat/_Arcade/Bubble Bobble.mra",
        ));
    }

    #[test]
    fn media_item_from_browse_entry_preserves_core_fields() {
        let item = media_item_from_browse_entry(BrowseEntry {
            media_id: Some(42),
            name: "Bubble Bobble (Japan)".into(),
            path: "/media/fat/_Arcade/_alternatives/Bubble Bobble/Bubble Bobble (Japan).mra".into(),
            system_id: "Arcade".into(),
            relative_path: "_alternatives/Bubble Bobble/Bubble Bobble (Japan).mra".into(),
            zap_script: "**launch:\"/x\"".into(),
            ..Default::default()
        });
        assert_eq!(item.media_id, Some(42));
        assert_eq!(item.system.id, "Arcade");
        assert_eq!(
            item.relative_path.as_deref(),
            Some("_alternatives/Bubble Bobble/Bubble Bobble (Japan).mra")
        );
        assert_eq!(item.zap_script, "**launch:\"/x\"");
    }

    #[test]
    fn file_stem_or_name_prefers_filename_stem() {
        assert_eq!(
            file_stem_or_name(
                "/media/fat/_Arcade/_alternatives/Bubble Bobble/Bubble Bobble (Japan).mra"
            ),
            "Bubble Bobble (Japan)"
        );
    }
}
