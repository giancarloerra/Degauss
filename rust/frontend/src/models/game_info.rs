// Zaparoo Frontend
// Copyright (c) 2026 Wizzo Pty Ltd and the Zaparoo Project contributors.
// SPDX-License-Identifier: LicenseRef-PolyForm-Noncommercial-1.0.0

use crate::media_image_cache::{
    global_media_image_cache, MediaImageCache, MediaImageUpdate, MediaKey,
};
use crate::models::tag_utils::tag_display_value;
use crate::models::{global_handle, global_store};
use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::QString;
use std::collections::BTreeSet;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::broadcast::error::RecvError;
use tokio::task::JoinHandle;
use tracing::debug;
use zaparoo_core::media_types::{MediaMeta, MediaMetaParams};

#[derive(Default)]
pub struct GameInfoRust {
    loading: bool,
    error_message: QString,
    title: QString,
    description: QString,
    detail_tags: QString,
    image_key: QString,
    image_index: i32,
    image_count: i32,
    image_can_prev: bool,
    image_can_next: bool,
    detail_image_keys: Vec<MediaKey>,
    seq: Arc<AtomicU64>,
    cover_subscription: Option<JoinHandle<()>>,
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
        #[qproperty(bool, loading)]
        #[qproperty(QString, error_message)]
        #[qproperty(QString, title)]
        #[qproperty(QString, description)]
        #[qproperty(QString, detail_tags)]
        #[qproperty(QString, image_key)]
        #[qproperty(i32, image_index)]
        #[qproperty(i32, image_count)]
        #[qproperty(bool, image_can_prev)]
        #[qproperty(bool, image_can_next)]
        type GameInfo = super::GameInfoRust;

        #[qinvokable]
        fn load(
            self: Pin<&mut GameInfo>,
            system_id: &QString,
            path: &QString,
            fallback_title: &QString,
        );

        #[qinvokable]
        fn clear(self: Pin<&mut GameInfo>);

        #[qinvokable]
        fn cycle_image(self: Pin<&mut GameInfo>, delta: i32);
    }

    impl cxx_qt::Threading for GameInfo {}
}

impl ffi::GameInfo {
    fn load(
        mut self: Pin<&mut Self>,
        system_id: &QString,
        path: &QString,
        fallback_title: &QString,
    ) {
        self.as_mut().ensure_cover_subscription();
        let system = system_id.to_string();
        let path = path.to_string();
        let fallback_title = fallback_title.to_string();
        self.as_mut().rust().seq.fetch_add(1, Ordering::SeqCst);
        clear_detail_images(self.as_mut());
        self.as_mut().set_error_message(QString::default());
        self.as_mut()
            .set_title(QString::from(fallback_title.trim()));
        self.as_mut().set_description(QString::default());
        self.as_mut().set_detail_tags(QString::default());
        if system.trim().is_empty() || path.trim().is_empty() {
            self.as_mut().set_loading(false);
            return;
        }
        self.as_mut().set_loading(true);
        let seq = self.rust().seq.clone();
        let ticket = seq.load(Ordering::SeqCst);
        let qt_thread = self.qt_thread();
        let store = global_store();
        global_handle().spawn(async move {
            let params = MediaMetaParams::for_media(system.clone(), path.clone());
            let result = match crate::media_meta_db::media_meta_async(params.clone()).await {
                Some(media) => Ok(zaparoo_core::media_types::MediaMetaResult { media }),
                None => store.client().media_meta(params).await,
            };
            let _ = qt_thread.queue(move |mut model| {
                if seq.load(Ordering::SeqCst) != ticket {
                    return;
                }
                model.as_mut().set_loading(false);
                match result {
                    Ok(result) => {
                        let meta_title = result.media.title.name.trim();
                        if !meta_title.is_empty() {
                            model.as_mut().set_title(QString::from(meta_title));
                        }
                        model.as_mut().set_description(QString::from(
                            description_from_meta(&result.media).as_str(),
                        ));
                        model.as_mut().set_detail_tags(QString::from(
                            detail_tags_from_meta(&result.media, &path).as_str(),
                        ));
                        install_detail_images(
                            model.as_mut(),
                            detail_image_keys_from_meta(&result.media, &system, &path),
                        );
                    }
                    Err(e) => {
                        debug!("game info fetch failed for {path}: {}", e.message);
                        model
                            .as_mut()
                            .set_error_message(QString::from(e.message.as_str()));
                    }
                }
            });
        });
    }

    fn clear(mut self: Pin<&mut Self>) {
        self.as_mut().rust().seq.fetch_add(1, Ordering::SeqCst);
        self.as_mut().set_loading(false);
        self.as_mut().set_error_message(QString::default());
        self.as_mut().set_title(QString::default());
        self.as_mut().set_description(QString::default());
        self.as_mut().set_detail_tags(QString::default());
        clear_detail_images(self);
    }

    fn cycle_image(self: Pin<&mut Self>, delta: i32) {
        if delta == 0 || self.image_count <= 1 {
            return;
        }
        let current = self.image_index;
        let next = (current + delta).clamp(0, self.image_count - 1);
        if next == current {
            return;
        }
        set_detail_image_index(self, next);
    }

    fn ensure_cover_subscription(mut self: Pin<&mut Self>) {
        if self.cover_subscription.is_some() {
            return;
        }
        let cache = global_media_image_cache();
        let mut rx = cache.subscribe();
        let qt_thread = self.qt_thread();
        let handle = global_handle().spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(update) => {
                        let _ = qt_thread.queue(move |model| {
                            notify_cover_update(model, &update);
                        });
                    }
                    Err(RecvError::Lagged(_)) => {}
                    Err(RecvError::Closed) => break,
                }
            }
        });
        self.as_mut().rust_mut().cover_subscription = Some(handle);
    }
}

fn description_from_meta(meta: &MediaMeta) -> String {
    meta.title
        .properties
        .get("property:description")
        .or_else(|| meta.properties.get("property:description"))
        .map(|property| property.text.trim().to_string())
        .filter(|text| !text.is_empty())
        .unwrap_or_default()
}

fn detail_tags_from_meta(meta: &MediaMeta, path: &str) -> String {
    let source = if meta.title.tags.is_empty() {
        meta.tags.as_slice()
    } else {
        meta.title.tags.as_slice()
    };
    let mut rows: Vec<(String, String)> = Vec::new();
    for tag_type in [
        "system",
        "platform",
        "year",
        "release date",
        "release_date",
        "genre",
        "players",
        "play mode",
        "play_mode",
        "cooperative",
        "developer",
        "publisher",
        "rating",
    ] {
        rows.extend(
            source
                .iter()
                .filter(|tag| {
                    tag.tag_type.eq_ignore_ascii_case(tag_type)
                        && !tag_display_value(tag).is_empty()
                })
                .map(|tag| (display_label(&tag.tag_type), tag_display_value(tag))),
        );
    }
    rows.extend(
        source
            .iter()
            .filter(|tag| {
                !is_ordered_tag(&tag.tag_type)
                    && !tag.tag_type.trim().is_empty()
                    && !tag_display_value(tag).is_empty()
            })
            .map(|tag| (display_label(&tag.tag_type), tag_display_value(tag))),
    );
    let filename = file_stem_or_name(path);
    if !filename.is_empty() {
        rows.push(("Filename".to_string(), filename));
    }
    rows.into_iter()
        .map(|(label, value)| format!("{label}\t{value}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_ordered_tag(tag_type: &str) -> bool {
    matches!(
        normalize_tag_type(tag_type).as_str(),
        "system"
            | "platform"
            | "year"
            | "release date"
            | "release_date"
            | "genre"
            | "players"
            | "play mode"
            | "play_mode"
            | "cooperative"
            | "developer"
            | "publisher"
            | "rating"
    )
}

fn normalize_tag_type(tag_type: &str) -> String {
    tag_type.trim().to_ascii_lowercase().replace('-', " ")
}

fn display_label(tag_type: &str) -> String {
    let normalized = normalize_tag_type(tag_type).replace('_', " ");
    match normalized.as_str() {
        "release date" => "Release date".to_string(),
        "play mode" => "Play mode".to_string(),
        other => {
            let mut words = other.split_whitespace();
            let Some(first) = words.next() else {
                return String::new();
            };
            let mut label = first.to_string();
            if let Some(ch) = label.get_mut(0..1) {
                ch.make_ascii_uppercase();
            }
            for word in words {
                label.push(' ');
                label.push_str(word);
            }
            label
        }
    }
}

fn image_type_from_property_key(key: &str) -> Option<String> {
    let suffix = key.strip_prefix("property:image")?;
    if suffix.is_empty() {
        return Some("image".to_string());
    }
    Some(suffix.trim_start_matches('-').to_string()).filter(|image_type| !image_type.is_empty())
}

fn detail_image_keys_from_meta(meta: &MediaMeta, system: &str, path: &str) -> Vec<MediaKey> {
    let mut seen = BTreeSet::<String>::new();
    let mut ordered = Vec::<String>::new();
    for image_type in meta
        .available_image_types
        .iter()
        .chain(meta.title.available_image_types.iter())
    {
        if !image_type.trim().is_empty() && seen.insert(image_type.clone()) {
            ordered.push(image_type.clone());
        }
    }
    if ordered.is_empty() {
        for key in meta
            .title
            .properties
            .keys()
            .chain(meta.properties.keys())
            .filter_map(|key| image_type_from_property_key(key))
        {
            seen.insert(key);
        }
        if seen.remove("image") {
            ordered.push("image".to_string());
        }
        ordered.extend(seen);
    }
    ordered
        .into_iter()
        .map(|image_type| MediaKey::with_image_type(system, path, image_type))
        .collect()
}

fn clear_detail_images(mut model: Pin<&mut ffi::GameInfo>) {
    model.as_mut().rust_mut().detail_image_keys.clear();
    model.as_mut().set_image_key(QString::default());
    model.as_mut().set_image_index(0);
    model.as_mut().set_image_count(0);
    model.as_mut().set_image_can_prev(false);
    model.as_mut().set_image_can_next(false);
}

fn install_detail_images(mut model: Pin<&mut ffi::GameInfo>, keys: Vec<MediaKey>) {
    model.as_mut().rust_mut().detail_image_keys = keys;
    set_detail_image_index(model, 0);
}

fn set_detail_image_index(mut model: Pin<&mut ffi::GameInfo>, index: i32) {
    let count = i32::try_from(model.detail_image_keys.len()).unwrap_or(i32::MAX);
    let clamped = if count <= 0 {
        0
    } else {
        index.clamp(0, count - 1)
    };
    model.as_mut().set_image_index(clamped);
    model.as_mut().set_image_count(count);
    model.as_mut().set_image_can_prev(clamped > 0);
    model
        .as_mut()
        .set_image_can_next(count > 0 && clamped < count - 1);
    sync_current_image_key(model);
}

fn sync_current_image_key(mut model: Pin<&mut ffi::GameInfo>) {
    let index = model.image_index;
    if index < 0 {
        model.as_mut().set_image_key(QString::default());
        return;
    }
    let Some(key) = model.detail_image_keys.get(index as usize).cloned() else {
        model.as_mut().set_image_key(QString::default());
        return;
    };
    let cache = global_media_image_cache();
    if cache.is_cached(&key) {
        model
            .as_mut()
            .set_image_key(QString::from(MediaImageCache::image_key_for(&key).as_str()));
    } else {
        cache.enqueue_with_media_id(key, None, 1);
        model.as_mut().set_image_key(QString::default());
    }
}

fn notify_cover_update(mut model: Pin<&mut ffi::GameInfo>, update: &MediaImageUpdate) {
    let current = model
        .detail_image_keys
        .get(model.image_index as usize)
        .cloned();
    if !current
        .as_ref()
        .is_some_and(|current| current == &update.key)
    {
        return;
    }
    if update.ext.is_some() {
        model.as_mut().set_image_key(QString::from(
            MediaImageCache::image_key_for(&update.key).as_str(),
        ));
    } else {
        model.as_mut().set_image_key(QString::default());
    }
}

fn file_stem_or_name(path: &str) -> String {
    let file = path
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default();
    file.rsplit_once('.')
        .map_or(file, |(stem, _)| stem)
        .trim()
        .to_string()
}
