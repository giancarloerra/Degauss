// Zaparoo Frontend
// Copyright (c) 2026 Wizzo Pty Ltd and the Zaparoo Project contributors.
// SPDX-License-Identifier: LicenseRef-PolyForm-Noncommercial-1.0.0
//
// Process-wide, in-memory cache for `media.meta` results, keyed by the
// canonical `(system_id, path)` pair (`MediaKey`). It exists so the list +
// detail browse view can paint a focused row's metadata synchronously instead
// of blanking and re-fetching on every move:
//
//   * The detail pane reads the cache the moment the focused row changes. A
//     warm neighbor (prefetched while dwelling on the previous row) is an
//     instant hit, so the table never shows the previous row's stale values
//     and never flickers blank-then-populate.
//
//   * A `None` outcome (Core returned an error or nothing usable) is memoized
//     as a definite negative, so an item with no metadata resolves instantly
//     on revisit and never flashes.
//
// In-memory only and strictly bounded by an LRU cap — Core is the canonical
// metadata store and the frontend must not persist scraped metadata, nor grow
// without bound on MiSTer's tight RAM budget (see CLAUDE.md). Metadata is a
// handful of small strings per row, so the entry-count cap keeps the footprint
// well under a megabyte.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

use tracing::debug;
use zaparoo_core::media_types::{MediaMeta, MediaMetaParams};

use crate::media_image_cache::MediaKey;
use crate::models::{global_handle, global_store};

/// Maximum number of cached metadata entries before LRU eviction kicks in.
const META_CACHE_CAP: usize = 512;

/// Outcome of a synchronous cache probe.
pub enum MetaLookup {
    /// Metadata is cached for this key. Boxed because `MediaMeta` is large and
    /// the other variants carry no data — keeps the enum cheap to pass around.
    Hit(Box<MediaMeta>),
    /// Core was asked and returned nothing usable — a memoized negative so
    /// revisits resolve instantly and never re-fetch.
    Negative,
    /// Not in the cache; the caller should fetch.
    Miss,
}

struct Entry {
    /// `Some` = positive hit, `None` = memoized negative.
    meta: Option<MediaMeta>,
    /// LRU recency stamp, bumped on insert and on every read.
    clock: u64,
}

struct State {
    map: HashMap<MediaKey, Entry>,
    /// Keys with a prefetch fetch in flight, so concurrent prefetch passes do
    /// not double-request the same row.
    inflight: HashSet<MediaKey>,
    clock: u64,
}

pub struct MediaMetaCache {
    state: Mutex<State>,
}

impl MediaMetaCache {
    fn new() -> Self {
        Self {
            state: Mutex::new(State {
                map: HashMap::new(),
                inflight: HashSet::new(),
                clock: 0,
            }),
        }
    }

    /// Probe the cache for `key`, bumping its LRU recency on a hit.
    pub fn lookup(&self, key: &MediaKey) -> MetaLookup {
        #[allow(clippy::unwrap_used, reason = "Mutex poisoning is unrecoverable")]
        let mut guard = self.state.lock().unwrap();
        guard.clock += 1;
        let now = guard.clock;
        match guard.map.get_mut(key) {
            Some(entry) => {
                entry.clock = now;
                match &entry.meta {
                    Some(meta) => MetaLookup::Hit(Box::new(meta.clone())),
                    None => MetaLookup::Negative,
                }
            }
            None => MetaLookup::Miss,
        }
    }

    /// Insert a resolved fetch outcome. `Some` is a positive hit, `None` a
    /// negative memo. Evicts the least-recently-used entry past the cap.
    pub fn store(&self, key: MediaKey, meta: Option<MediaMeta>) {
        #[allow(clippy::unwrap_used, reason = "Mutex poisoning is unrecoverable")]
        let mut guard = self.state.lock().unwrap();
        store_locked(&mut guard, key, meta);
    }

    /// Best-effort background warm of `requests` (key + params) that are not
    /// already cached or in flight. Fire-and-forget: results land in the cache
    /// for the next synchronous `lookup`.
    pub fn prefetch(&self, requests: Vec<(MediaKey, MediaMetaParams)>) {
        let mut to_fetch: Vec<(MediaKey, MediaMetaParams)> = Vec::new();
        {
            #[allow(clippy::unwrap_used, reason = "Mutex poisoning is unrecoverable")]
            let mut guard = self.state.lock().unwrap();
            for (key, params) in requests {
                if guard.map.contains_key(&key) || guard.inflight.contains(&key) {
                    continue;
                }
                guard.inflight.insert(key.clone());
                to_fetch.push((key, params));
            }
        }
        if to_fetch.is_empty() {
            return;
        }
        global_handle().spawn(async move {
            let store = global_store();
            let cache = global_media_meta_cache();
            for (key, params) in to_fetch {
                // Direct database resolution first; the RPC remains the
                // fallback for any None, mirroring the other read paths.
                let meta = match crate::media_meta_db::media_meta_async(params.clone()).await {
                    Some(media) => Some(media),
                    None => match store.client().media_meta(params).await {
                        Ok(result) => Some(result.media),
                        Err(_) => None,
                    },
                };
                cache.store(key, meta);
            }
        });
    }
}

fn store_locked(guard: &mut State, key: MediaKey, meta: Option<MediaMeta>) {
    guard.clock += 1;
    let now = guard.clock;
    guard.inflight.remove(&key);
    guard.map.insert(key, Entry { meta, clock: now });
    while guard.map.len() > META_CACHE_CAP {
        let victim = guard
            .map
            .iter()
            .min_by_key(|(_, entry)| entry.clock)
            .map(|(k, _)| k.clone());
        match victim {
            Some(v) => {
                guard.map.remove(&v);
                debug!("media_meta_cache: evicted entry");
            }
            None => break,
        }
    }
}

static GLOBAL_MEDIA_META_CACHE: OnceLock<Arc<MediaMetaCache>> = OnceLock::new();

/// Lazily initialise the process-wide media metadata cache and return a handle.
/// Constructed on first call from any thread; subsequent calls return the same
/// `Arc`.
pub fn global_media_meta_cache() -> Arc<MediaMetaCache> {
    GLOBAL_MEDIA_META_CACHE
        .get_or_init(|| Arc::new(MediaMetaCache::new()))
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(path: &str) -> MediaKey {
        MediaKey::new("SNES", path)
    }

    fn meta_with_path(path: &str) -> MediaMeta {
        MediaMeta {
            path: path.to_string(),
            ..MediaMeta::default()
        }
    }

    #[test]
    fn positive_hit_round_trips() {
        let cache = MediaMetaCache::new();
        cache.store(key("a"), Some(meta_with_path("a")));
        assert!(matches!(cache.lookup(&key("a")), MetaLookup::Hit(meta) if meta.path == "a"));
    }

    #[test]
    fn negative_is_memoized() {
        let cache = MediaMetaCache::new();
        cache.store(key("b"), None);
        assert!(matches!(cache.lookup(&key("b")), MetaLookup::Negative));
    }

    #[test]
    fn miss_for_unknown_key() {
        let cache = MediaMetaCache::new();
        assert!(matches!(cache.lookup(&key("missing")), MetaLookup::Miss));
    }

    #[test]
    fn evicts_least_recently_used_past_cap() {
        let cache = MediaMetaCache::new();
        for i in 0..META_CACHE_CAP {
            cache.store(
                key(&format!("k{i}")),
                Some(meta_with_path(&format!("k{i}"))),
            );
        }
        // Touch k0 so it is the most-recently-used, then overflow by one.
        assert!(matches!(cache.lookup(&key("k0")), MetaLookup::Hit(_)));
        cache.store(key("overflow"), Some(meta_with_path("overflow")));
        // k1 was the least-recently-used and should have been evicted; k0
        // survived because the lookup refreshed its recency.
        assert!(matches!(cache.lookup(&key("k1")), MetaLookup::Miss));
        assert!(matches!(cache.lookup(&key("k0")), MetaLookup::Hit(_)));
        assert!(matches!(cache.lookup(&key("overflow")), MetaLookup::Hit(_)));
    }
}
