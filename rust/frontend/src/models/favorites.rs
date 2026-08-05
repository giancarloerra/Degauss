// Zaparoo Frontend
// Copyright (c) 2026 Wizzo Pty Ltd and the Zaparoo Project contributors.
// SPDX-License-Identifier: LicenseRef-PolyForm-Noncommercial-1.0.0
//
// `Browse.FavoritesModel` — flat list of favorite media, surfaced
// from Core's `media.search` endpoint.
//
// Two paths into the model:
//
//   * `bind_to_endpoint!` seeds page 1 from `MediaFavoritesEndpoint` so
//     a screen flip into Favorites has data on the first paint when the
//     resource is already `Ready`. The fixed args (`maxResults = 25`)
//     match what the UI requests.
//
//   * `fetch_more()` — cursor-driven follow-ups bypass the cache and
//     call `Client::media_search` directly, just like games. The
//     model owns the cursor, the in-flight `loading_more` debounce,
//     and the seq ticket that disarms stale callbacks.
//
// Search is flat (no folder navigation, no auto-nav) so this model
// stays a fraction of the size of `GamesModel`. Card-write isn't wired
// here yet — runtime launches prefer the exact indexed path, while
// QR/card-write payloads prefer Core's portable ZapScript.

use crate::media_image_cache::{global_media_image_cache, MediaImageCache, MediaKey};
use crate::media_meta_cache::{global_media_meta_cache, MetaLookup};
use crate::models::nav_timing::NavTiming;
use crate::models::random_pick::random_index;
use crate::models::tag_utils::{
    disambiguating_tag_labels, sibling_disambiguation_displays, tag_display_value,
};
use crate::models::{global_handle, global_store};
use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::{
    QByteArray, QHash, QHashPair_i32_QByteArray, QList, QModelIndex, QString, QVariant,
};
use std::collections::HashSet;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::broadcast::error::RecvError;
use tokio::task::JoinHandle;
use tracing::{info, warn};
use zaparoo_core::client::ClientError;
use zaparoo_core::endpoints::catalog::CatalogEndpoint;
use zaparoo_core::endpoints::media_favorites::{FavoritesArgs, MediaFavoritesEndpoint};
use zaparoo_core::endpoints::media_tags_update::MediaTagsUpdateMutation;
use zaparoo_core::endpoints::readers_write::ReadersWriteMutation;
use zaparoo_core::endpoints::run::RunMutation;
use zaparoo_core::media_types::{
    MediaItem, MediaMeta, MediaMetaParams, MediaSearchParams, MediaSearchResult,
    MediaTagsUpdateParams, ReadersWriteParams, RunParams, TagInfo,
};
use zaparoo_core::remote_resource::ResourceStatus;

const NAME_ROLE: i32 = 256 + 1;
const PATH_ROLE: i32 = 256 + 2;
const SYSTEM_ID_ROLE: i32 = 256 + 3;
const COVER_KEY_ROLE: i32 = 256 + 4;
const ZAP_SCRIPT_ROLE: i32 = 256 + 5;
const FAVORITE_ROLE: i32 = 256 + 6;
const FILE_STEM_ROLE: i32 = 256 + 7;
const HIDDEN_ROLE: i32 = 256 + 8;
// Newline-joined short tokens for Core's `disambiguatingTags`, ordered by
// display priority. Empty when nothing to disambiguate. Same shape and
// rationale as the GamesModel role; the shared delegate splits on newlines.
const DISAMBIGUATING_TAGS_ROLE: i32 = 256 + 9;

// Page size for the initial load and every cursor follow-up. Core defaults
// `maxResults` to 100 when the field is absent and validates it at 1000
// (`max=1000` on SearchParams.MaxResults); note it REJECTS an over-cap value
// rather than clamping. Search rows are tiny (one tile + one caption per
// row) so 25 fills several screens of the favorites grid without
// stressing the over-the-wire payload.
const PAGE_SIZE: u32 = 25;

/// `sort_mode` value for A-Z on the displayed name. Empty string means
/// Core's own order, which is what the screen has always shown.
const SORT_NAME: &str = "name";

/// Page size for the sort/filter full load. Core validates `maxResults` at
/// 1000 and REJECTS anything larger, so this is the ceiling, not a hint. One
/// request covers any realistic favorites list; the existing cursor chain
/// still drains anything beyond it.
const FULL_LOAD_SIZE: u32 = 1000;
// How many rows ahead/behind the settled cursor to warm in list-detail
// layout. Kept small so the 2-worker byte queue stays shallow and the next
// cover is fetched first within ~250 ms.
const COVER_PREFETCH_CURSOR_NEXT: i32 = 4;
const COVER_PREFETCH_CURSOR_PREV: i32 = 2;

#[allow(
    clippy::struct_excessive_bools,
    reason = "the bools are independent qproperties surfaced to QML; collapsing them \
              into an enum would force the QML side to read a single state property \
              and re-derive each flag locally, which is exactly the work the bridge \
              avoids"
)]
pub struct FavoritesModelRust {
    // Fetch truth, always in the order Core returned it. Never reordered:
    // sorting and filtering happen in `view` so the fetch/cursor chain and
    // the append path keep working on a stable, contiguous list.
    entries: Vec<MediaItem>,
    // Row order actually shown: `view[row]` is an index into `entries`.
    // Rebuilt only by `rebuild_view`.
    view: Vec<usize>,
    // Inverse of `view`: `rev[entry_index]` is the visible row, or -1 when
    // the entry is filtered out. Lets async callbacks that hold an entry
    // index resolve the row to emit (or skip when hidden).
    rev: Vec<i32>,
    // Parallel to `view` (NOT to `entries`): the sibling-diffed disambiguation
    // display per row. Adjacency-based, so it must follow display order —
    // see `compute_favorites_disambig_displays`. Recomputed by `rebuild_view`
    // and when `show_original_filenames` changes.
    disambig_displays: Vec<String>,
    // "" = Core's order, "name" = A-Z on the displayed name.
    sort_mode: QString,
    // "" = everything, "sys:<id>" = one system, "cat:<category>" = one
    // category. A single axis keeps the state space small: every category
    // row already resolves to a set of systems.
    filter: QString,
    // True once every favorite has been fetched, which sorting and the
    // system facet both require (Core has no sort parameter and returns
    // favorites in DBID order, 25 at a time).
    full_loaded: bool,
    full_loading: bool,
    // A random launch was pressed before the full set had arrived; fired by
    // `launch_pending_random` once the load completes.
    random_pending: bool,
    // Stable reason code for a random pick that resolved nothing, so the
    // press reports itself instead of appearing to do nothing.
    random_error: QString,
    // Distinct systems present in the loaded favorites, as JSON, so the
    // picker only ever offers filters that match something.
    system_facet_json: QString,
    // Favorites held before filtering, for the "N of M" counter.
    total_count: i32,
    count: i32,
    loading: bool,
    loading_more: bool,
    error_message: QString,
    has_next_page: bool,
    next_cursor: Option<String>,
    card_write_pending: bool,
    card_write_error: QString,
    current_detail_loading: bool,
    current_detail_tags: QString,
    current_detail_image_key: QString,
    // Warm cover key for the item immediately after the current selection.
    // Empty when there is no next item or the next item has no cached bytes.
    // Exposed as a qproperty so QML can mount a hidden Image that decodes the
    // cover into Qt's pixmap cache while the user is still on the current row.
    detail_prefetch_key_next: QString,
    // Same as `detail_prefetch_key_next` but for the item immediately before.
    detail_prefetch_key_prev: QString,
    // Row whose adjacent covers are being preloaded. None when no detail
    // is active (cleared on reset or out-of-range).
    detail_prefetch_row: Option<i32>,
    cover_requests_paused: bool,
    // Mirrors the global `Show original filenames` setting; when true the
    // `name` role and `name_at()` return the original filename (sans
    // extension). Bound from QML; flipping re-emits `dataChanged(NAME_ROLE)`.
    show_original_filenames: bool,
    current_detail_media_key: Option<MediaKey>,
    current_detail_media_id: Option<i64>,
    card_write_seq: Arc<AtomicU64>,
    detail_seq: Arc<AtomicU64>,
    // Bumped whenever the cursor chain is reset by an initial
    // `apply_state` so any in-flight `fetch_more` callback can detect
    // its append no longer belongs to the current chain.
    seq: Arc<AtomicU64>,
    // Long-lived bridge from `media_image_cache` broadcast updates
    // onto `dataChanged(coverKey)` emits for matching rows. Spun up
    // lazily on the first page apply so the model singleton owns
    // exactly one subscriber for the whole process lifetime.
    cover_subscription: Option<JoinHandle<()>>,
    // Keys whose first-paint we're still waiting on. While non-empty
    // we hold `loading = true` so the screen-flip overlay covers the
    // gap between "page rendered with system logos" and "covers
    // cached". Drained by `notify_cover_update` as each cover lands;
    // force-cleared by the gate timer or a Pending/Errored transition.
    pending_first_paint_keys: HashSet<MediaKey>,
    // Safety timer that force-releases the cover gate after a bounded
    // delay, so a stalled bulk RPC can't park the user on `Loading…`
    // forever.
    cover_gate_timer: Option<JoinHandle<()>>,
    // Bumped on every cover-gate arm and on every Pending/Errored
    // disarm. The timer's queued closure compares against the current
    // value and bails on a mismatch — necessary because aborting the
    // JoinHandle doesn't cancel a callback already queued onto the Qt
    // thread between sleep-completion and abort.
    cover_gate_seq: Arc<AtomicU64>,
    nav_timing: Option<NavTiming>,
}

impl Default for FavoritesModelRust {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            view: Vec::new(),
            rev: Vec::new(),
            disambig_displays: Vec::new(),
            sort_mode: QString::default(),
            filter: QString::default(),
            full_loaded: false,
            full_loading: false,
            random_pending: false,
            random_error: QString::default(),
            system_facet_json: QString::default(),
            total_count: 0,
            count: 0,
            loading: false,
            loading_more: false,
            error_message: QString::default(),
            has_next_page: false,
            next_cursor: None,
            card_write_pending: false,
            card_write_error: QString::default(),
            current_detail_loading: false,
            current_detail_tags: QString::default(),
            current_detail_image_key: QString::default(),
            detail_prefetch_key_next: QString::default(),
            detail_prefetch_key_prev: QString::default(),
            detail_prefetch_row: None,
            cover_requests_paused: true,
            show_original_filenames: false,
            current_detail_media_key: None,
            current_detail_media_id: None,
            card_write_seq: Arc::new(AtomicU64::new(0)),
            detail_seq: Arc::new(AtomicU64::new(0)),
            seq: Arc::new(AtomicU64::new(0)),
            cover_subscription: None,
            pending_first_paint_keys: HashSet::new(),
            cover_gate_timer: None,
            cover_gate_seq: Arc::new(AtomicU64::new(0)),
            nav_timing: None,
        }
    }
}

#[cxx_qt::bridge]
pub mod ffi {
    unsafe extern "C++" {
        include!("model_includes.h");

        #[allow(non_snake_case, reason = "Qt class names are PascalCase")]
        type QAbstractListModel;

        type QModelIndex = cxx_qt_lib::QModelIndex;
        type QVariant = cxx_qt_lib::QVariant;
        type QHash_i32_QByteArray = cxx_qt_lib::QHash<cxx_qt_lib::QHashPair_i32_QByteArray>;
        type QByteArray = cxx_qt_lib::QByteArray;
        type QString = cxx_qt_lib::QString;
        type QList_i32 = cxx_qt_lib::QList<i32>;
    }

    unsafe extern "RustQt" {
        #[qobject]
        #[base = QAbstractListModel]
        #[qml_element]
        #[qml_singleton]
        #[qproperty(i32, count)]
        #[qproperty(bool, loading)]
        #[qproperty(bool, loading_more)]
        #[qproperty(QString, error_message)]
        #[qproperty(bool, has_next_page)]
        #[qproperty(bool, card_write_pending)]
        #[qproperty(QString, card_write_error)]
        #[qproperty(bool, current_detail_loading)]
        #[qproperty(QString, current_detail_tags)]
        #[qproperty(QString, current_detail_image_key)]
        #[qproperty(QString, detail_prefetch_key_next)]
        #[qproperty(QString, detail_prefetch_key_prev)]
        #[qproperty(bool, cover_requests_paused)]
        #[qproperty(bool, show_original_filenames, READ, WRITE = set_show_original_filenames, NOTIFY)]
        #[qproperty(QString, sort_mode, READ, WRITE = set_sort_mode, NOTIFY)]
        #[qproperty(QString, filter, READ, WRITE = set_filter, NOTIFY)]
        #[qproperty(bool, full_loaded)]
        #[qproperty(bool, full_loading)]
        #[qproperty(QString, system_facet_json)]
        #[qproperty(QString, random_error)]
        #[qproperty(i32, total_count)]
        type FavoritesModel = super::FavoritesModelRust;

        #[qinvokable]
        fn fetch_more(self: Pin<&mut FavoritesModel>);

        /// Fetch every remaining favorite so the view can sort and build the
        /// system facet. No-op once loaded or while a load is in flight.
        #[qinvokable]
        fn ensure_full_load(self: Pin<&mut FavoritesModel>);

        #[qinvokable]
        fn set_sort_mode(self: Pin<&mut FavoritesModel>, value: &QString);

        #[qinvokable]
        fn set_filter(self: Pin<&mut FavoritesModel>, value: &QString);

        #[qinvokable]
        fn clear_random_error(self: Pin<&mut FavoritesModel>);

        #[qinvokable]
        fn launch_at(self: Pin<&mut FavoritesModel>, index: i32);

        #[qinvokable]
        fn launch_text_at(self: &FavoritesModel, index: i32) -> QString;

        #[qinvokable]
        fn launch_random(self: Pin<&mut FavoritesModel>);

        #[qinvokable]
        fn write_card_at(self: Pin<&mut FavoritesModel>, index: i32);

        #[qinvokable]
        fn toggle_favorite_at(self: Pin<&mut FavoritesModel>, index: i32);

        #[qinvokable]
        fn is_favorite_at(self: &FavoritesModel, index: i32) -> bool;

        #[qinvokable]
        fn cancel_card_write(self: Pin<&mut FavoritesModel>);

        #[qinvokable]
        fn set_show_original_filenames(self: Pin<&mut FavoritesModel>, value: bool);

        #[qinvokable]
        fn name_at(self: &FavoritesModel, index: i32) -> QString;

        #[qinvokable]
        fn disambiguating_tags_at(self: &FavoritesModel, index: i32) -> QString;

        #[qinvokable]
        fn path_at(self: &FavoritesModel, index: i32) -> QString;

        #[qinvokable]
        fn system_id_at(self: &FavoritesModel, index: i32) -> QString;

        #[qinvokable]
        fn peek_detail_at(self: Pin<&mut FavoritesModel>, index: i32);

        #[qinvokable]
        fn load_detail_at(self: Pin<&mut FavoritesModel>, index: i32);

        #[qinvokable]
        fn clear_current_detail(self: Pin<&mut FavoritesModel>);

        #[qinvokable]
        fn refresh_cover_keys(self: Pin<&mut FavoritesModel>, first_row: i32, count: i32);

        #[qinvokable]
        fn clear_pending_cover_requests(self: Pin<&mut FavoritesModel>);

        #[qinvokable]
        fn index_for_path(self: &FavoritesModel, path: &QString) -> i32;

        #[inherit]
        #[cxx_name = "beginResetModel"]
        fn begin_reset_model(self: Pin<&mut FavoritesModel>);

        #[inherit]
        #[cxx_name = "endResetModel"]
        fn end_reset_model(self: Pin<&mut FavoritesModel>);

        #[inherit]
        #[cxx_name = "beginInsertRows"]
        fn begin_insert_rows(
            self: Pin<&mut FavoritesModel>,
            parent: &QModelIndex,
            first: i32,
            last: i32,
        );

        #[inherit]
        #[cxx_name = "endInsertRows"]
        fn end_insert_rows(self: Pin<&mut FavoritesModel>);

        // Qt signal bound as a callable so the cover-cache bridge can
        // invoke it directly from the Qt thread when an async cover
        // fetch completes for a row that is already on screen.
        #[inherit]
        #[cxx_name = "dataChanged"]
        fn data_changed(
            self: Pin<&mut FavoritesModel>,
            top_left: &QModelIndex,
            bottom_right: &QModelIndex,
            roles: &QList_i32,
        );

        #[cxx_name = "rowCount"]
        fn row_count(self: &FavoritesModel, parent: &QModelIndex) -> i32;
        fn data(self: &FavoritesModel, index: &QModelIndex, role: i32) -> QVariant;
        #[cxx_name = "roleNames"]
        fn role_names(self: &FavoritesModel) -> QHash_i32_QByteArray;

        // Materialise a `QModelIndex` for `(row, column)` so the cover-
        // cache bridge can target individual rows in `dataChanged`.
        // Forwarded to the QAbstractListModel implementation.
        #[inherit]
        fn index(self: &FavoritesModel, row: i32, column: i32, parent: &QModelIndex)
            -> QModelIndex;
    }

    impl cxx_qt::Threading for FavoritesModel {}
    impl cxx_qt::Initialize for FavoritesModel {}
}

crate::bind_to_endpoint! {
    for ffi::FavoritesModel,
    endpoint = MediaFavoritesEndpoint,
    args = FavoritesArgs::new(PAGE_SIZE),
    select = project,
    apply = apply_state,
}

/// Snapshot of a single page that `apply_state` can write onto the
/// model. Carried by value so the closure is `Send + 'static` for the
/// `qt_thread` queue.
type PageSnapshot = (Vec<MediaItem>, bool, Option<String>);

/// Project the resource status onto an `(Option<PageSnapshot>, error)`
/// tuple. `Idle`/`Loading` map to the same `(None, "")` shape so the
/// apply path can decide on its own whether to show the spinner.
fn project(status: &ResourceStatus<MediaSearchResult>) -> (Option<PageSnapshot>, String) {
    match status {
        ResourceStatus::Ready(data) => (
            Some((
                data.results.clone(),
                data.has_next_page(),
                data.next_cursor(),
            )),
            String::new(),
        ),
        ResourceStatus::Errored { message, .. } => (None, message.clone()),
        ResourceStatus::Idle | ResourceStatus::Loading => (None, String::new()),
    }
}

fn apply_state(model: Pin<&mut ffi::FavoritesModel>, (data, err): (Option<PageSnapshot>, String)) {
    if let Some(snapshot) = data {
        // Direct-first: the store's Ready is the trigger and the local
        // read is the content; the RPC page it delivered is only
        // applied when the direct read cannot answer. This keeps every
        // freshness path (heart toggles, reconnects) intact while the
        // painted list always comes from the complete local set.
        if crate::media_favorites_db::enabled() {
            spawn_direct_ready(model, snapshot);
        } else {
            apply_ready_page(model, snapshot);
        }
    } else {
        apply_pending_or_error(model, &err);
    }
}

fn apply_ready_page(mut model: Pin<&mut ffi::FavoritesModel>, snapshot: PageSnapshot) {
    let apply_started = Instant::now();
    {
        let (entries, has_next_page, next_cursor) = snapshot;
        if model.nav_timing.is_none() {
            model.as_mut().rust_mut().nav_timing = Some(NavTiming::new("cache"));
        }
        if let Some(timing) = model.as_mut().rust_mut().nav_timing.as_mut() {
            timing.mark_request_done();
        }
        // A fresh initial page resets the cursor chain — bump `seq` so
        // any in-flight `fetch_more` sees a stale ticket and bails.
        model.as_mut().rust_mut().seq.fetch_add(1, Ordering::SeqCst);
        model.as_mut().ensure_cover_subscription();
        if !model.cover_requests_paused {
            enqueue_favorites_covers(&entries);
        }
        clear_current_detail_state(model.as_mut());
        // A fresh first page invalidates any completed full load; the sort or
        // filter still applies, so rebuild_view re-derives rows from it.
        {
            let mut rust = model.as_mut().rust_mut();
            rust.entries = entries;
            rust.next_cursor = next_cursor;
            // A fresh chain invalidates any press queued against the old one.
            rust.random_pending = false;
        }
        // Through the setters, not `rust_mut`: both are qproperties, so a
        // silent write leaves any QML binding on them stale.
        model.as_mut().set_full_loaded(false);
        model.as_mut().set_full_loading(false);
        model.as_mut().rebuild_view();
        // Sorting and filtering both need the whole set. Deferred to the end
        // of this branch because `ensure_full_load` holds `loading_more` to
        // block a competing cursor fetch, and the reset below would clear it.
        let needs_full_load =
            !model.sort_mode.to_string().is_empty() || !model.filter.to_string().is_empty();
        if let Some(timing) = model.as_mut().rust_mut().nav_timing.as_mut() {
            timing.mark_apply_done();
        }
        info!(
            apply_ms = apply_started.elapsed().as_millis(),
            "favorites: apply_state timing",
        );
        if model.has_next_page != has_next_page {
            model.as_mut().set_has_next_page(has_next_page);
        }
        // Hidden startup binding can pause cover requests so Hub paints without
        // Favorites' off-screen cover gate. Screen entry resumes requests and
        // refreshes visible cover roles.
        if model.cover_requests_paused {
            disarm_cover_gate(model.as_mut());
            if model.loading {
                model.as_mut().set_loading(false);
            }
            finish_nav_timing(model.as_mut(), "covers-paused", 0);
        } else {
            // Decide whether to release `loading` immediately or hold it until
            // covers are cached. `arm_cover_gate` flips loading off itself when
            // the page has nothing to wait on; otherwise it leaves loading=true
            // and arms the safety timer.
            arm_cover_gate(model.as_mut());
        }
        if model.loading_more {
            model.as_mut().set_loading_more(false);
        }
        finish_ready_pagination(model.as_mut(), has_next_page, needs_full_load);
    }
}

/// Direct-first Ready handling: bump the chain, read the complete
/// local set (with catalog enrichment), and paint it; the RPC page in
/// `snapshot` applies only when the local read cannot answer.
fn spawn_direct_ready(mut model: Pin<&mut ffi::FavoritesModel>, snapshot: PageSnapshot) {
    // The bump invalidates in-flight cursor fetches exactly like a page
    // apply would; the callback ticket rides the post-bump value.
    model.as_mut().rust_mut().seq.fetch_add(1, Ordering::SeqCst);
    let seq = model.rust().seq.clone();
    let ticket = seq.load(Ordering::SeqCst);
    let qt_thread = model.qt_thread();
    global_handle().spawn(async move {
        let cancel_seq = seq.clone();
        let direct = tokio::task::spawn_blocking(move || {
            crate::media_favorites_db::favorites(&|| cancel_seq.load(Ordering::SeqCst) != ticket)
        })
        .await
        .ok()
        .flatten();
        let _ = qt_thread.queue(move |model| {
            if seq.load(Ordering::SeqCst) != ticket {
                return;
            }
            match direct.and_then(enrich_from_catalog) {
                Some(entries) => apply_direct_full_paint(model, entries),
                None => apply_ready_page(model, snapshot),
            }
        });
    });
}

/// Early paint: fired from the Pending branch so a cold entry shows the
/// complete local set in tens of milliseconds instead of waiting for
/// the store's first network Ready, which then simply re-confirms it.
fn spawn_direct_early_paint(model: &Pin<&mut ffi::FavoritesModel>) {
    if !crate::media_favorites_db::enabled() || model.count > 0 || model.full_loaded {
        return;
    }
    let seq = model.rust().seq.clone();
    let ticket = seq.load(Ordering::SeqCst);
    let qt_thread = model.qt_thread();
    global_handle().spawn(async move {
        let cancel_seq = seq.clone();
        let direct = tokio::task::spawn_blocking(move || {
            crate::media_favorites_db::favorites(&|| cancel_seq.load(Ordering::SeqCst) != ticket)
        })
        .await
        .ok()
        .flatten();
        let Some(entries) = direct.and_then(enrich_from_catalog) else {
            return;
        };
        let _ = qt_thread.queue(move |model| {
            if model.rust().seq.load(Ordering::SeqCst) != ticket {
                return;
            }
            apply_direct_full_paint(model, entries);
        });
    });
}

/// Land the complete local set as a first-class paint: the direct
/// counterpart of `apply_ready_page` plus terminal full-load state, so
/// it can serve both the cold entry and every store-triggered refresh.
fn apply_direct_full_paint(mut model: Pin<&mut ffi::FavoritesModel>, entries: Vec<MediaItem>) {
    let apply_started = Instant::now();
    if model.nav_timing.is_none() {
        model.as_mut().rust_mut().nav_timing = Some(NavTiming::new("direct"));
    }
    if let Some(timing) = model.as_mut().rust_mut().nav_timing.as_mut() {
        timing.set_source("direct");
        timing.mark_request_done();
    }
    model.as_mut().ensure_cover_subscription();
    if !model.cover_requests_paused {
        enqueue_favorites_covers(&entries);
    }
    clear_current_detail_state(model.as_mut());
    {
        let mut rust = model.as_mut().rust_mut();
        rust.entries = entries;
        rust.next_cursor = None;
        rust.random_pending = false;
    }
    model.as_mut().set_full_loading(false);
    model.as_mut().set_full_loaded(true);
    model.as_mut().rebuild_view();
    if let Some(timing) = model.as_mut().rust_mut().nav_timing.as_mut() {
        timing.mark_apply_done();
    }
    info!(
        apply_ms = apply_started.elapsed().as_millis(),
        "favorites: apply_direct_full_paint timing",
    );
    if model.has_next_page {
        model.as_mut().set_has_next_page(false);
    }
    if model.cover_requests_paused {
        disarm_cover_gate(model.as_mut());
        if model.loading {
            model.as_mut().set_loading(false);
        }
        finish_nav_timing(model.as_mut(), "covers-paused", 0);
    } else {
        arm_cover_gate(model.as_mut());
    }
    if model.loading_more {
        model.as_mut().set_loading_more(false);
    }
    if !model.error_message.is_empty() {
        model.as_mut().set_error_message(QString::default());
    }
    model.as_mut().launch_pending_random();
}

fn apply_pending_or_error(mut model: Pin<&mut ffi::FavoritesModel>, err: &str) {
    if err.is_empty() {
        if model.nav_timing.is_none() {
            model.as_mut().rust_mut().nav_timing = Some(NavTiming::new("network"));
        } else if let Some(timing) = model.as_mut().rust_mut().nav_timing.as_mut() {
            timing.set_source("network");
        }
        // Pending (Idle/Loading): show the spinner; don't touch results.
        // Disarm pagination so a grid scroll during a refetch doesn't
        // fire `fetch_more` against a stale cursor — `has_next_page`
        // is re-set when Ready lands. Bump `seq` and null `next_cursor`
        // so an in-flight `fetch_more` queued during the prior Ready
        // can't slip a stale append in before the next Ready arrives.
        // Disarm the cover gate too: a stale timer firing during the
        // next Ready would clear loading prematurely.
        disarm_cover_gate(model.as_mut());
        clear_current_detail_state(model.as_mut());
        model.as_mut().rust_mut().seq.fetch_add(1, Ordering::SeqCst);
        model.as_mut().rust_mut().next_cursor = None;
        // The seq bump strands any in-flight full load: its callback bails
        // on the stale ticket without clearing these. Release them here or
        // `ensure_full_load` stays wedged (sort and filter would silently
        // keep operating on page 1) for the rest of the session.
        if model.full_loading {
            model.as_mut().set_full_loading(false);
        }
        model.as_mut().rust_mut().random_pending = false;
        if model.loading_more {
            model.as_mut().set_loading_more(false);
        }
        if !model.loading {
            model.as_mut().set_loading(true);
        }
        if model.has_next_page {
            model.as_mut().set_has_next_page(false);
        }
        spawn_direct_early_paint(&model);
    } else {
        // Same disarm as the Pending branch — an Errored transition
        // doesn't reset entries, so a callback queued during the prior
        // Ready could otherwise append rows that don't belong to the
        // current chain.
        disarm_cover_gate(model.as_mut());
        clear_current_detail_state(model.as_mut());
        model.as_mut().rust_mut().seq.fetch_add(1, Ordering::SeqCst);
        model.as_mut().rust_mut().next_cursor = None;
        // The seq bump strands any in-flight full load: its callback bails
        // on the stale ticket without clearing these. Release them here or
        // `ensure_full_load` stays wedged (sort and filter would silently
        // keep operating on page 1) for the rest of the session.
        if model.full_loading {
            model.as_mut().set_full_loading(false);
        }
        model.as_mut().rust_mut().random_pending = false;
        if model.loading_more {
            model.as_mut().set_loading_more(false);
        }
        if model.loading {
            model.as_mut().set_loading(false);
        }
        finish_nav_timing(model.as_mut(), "error", 0);
        if model.has_next_page {
            model.as_mut().set_has_next_page(false);
        }
    }
    let qerr = QString::from(err);
    if model.error_message != qerr {
        model.as_mut().set_error_message(qerr);
    }
}

/// Fill `system.category` (and any missing `system.name`) from the
/// store's catalog snapshot. Returns `None` when the catalog is not
/// Ready yet — the RPC full load carries its own enrichment, so the
/// caller falls back rather than serving entries the scope filter
/// cannot match.
fn enrich_from_catalog(mut entries: Vec<MediaItem>) -> Option<Vec<MediaItem>> {
    let resource = global_store().subscribe::<CatalogEndpoint>(());
    let rx = resource.subscribe();
    let status = rx.borrow();
    let ResourceStatus::Ready(catalog) = &*status else {
        return None;
    };
    let by_id: std::collections::HashMap<&str, (&str, &str)> = catalog
        .systems
        .iter()
        .map(|s| (s.id.as_str(), (s.name.as_str(), s.category.as_str())))
        .collect();
    for entry in &mut entries {
        if let Some((name, category)) = by_id.get(entry.system.id.as_str()) {
            entry.system.category = (*category).to_string();
            if entry.system.name.is_empty() {
                entry.system.name = (*name).to_string();
            }
        }
    }
    Some(entries)
}

/// Post-Ready pagination: warm page 2 over the RPC, or run the full
/// load when the sort or filter needs the whole set. Reached only when
/// the direct read could not answer (or the layer is disabled), so the
/// paged chain must continue exactly as it always did; the always-full
/// behavior is delivered by the direct-first paint, not from here.
/// `fetch_more` is itself guarded by `has_next_page` and
/// `loading_more`.
fn finish_ready_pagination(
    mut model: Pin<&mut ffi::FavoritesModel>,
    has_next_page: bool,
    needs_full_load: bool,
) {
    if has_next_page && !model.cover_requests_paused && !needs_full_load {
        model.as_mut().fetch_more();
    }
    if needs_full_load {
        model.as_mut().ensure_full_load();
    }
}

/// Land the direct full favorite set: the local-read counterpart of the
/// RPC chain's terminal chunk. Entries replace wholesale (the local set
/// is complete and self-consistent), the cursor chain ends, and the
/// same finalize contract runs: view rebuild, `full_loaded`, and any
/// queued random press.
fn apply_direct_favorites(mut model: Pin<&mut ffi::FavoritesModel>, entries: Vec<MediaItem>) {
    model.as_mut().set_full_loading(false);
    if model.loading_more {
        model.as_mut().set_loading_more(false);
    }
    {
        let mut rust = model.as_mut().rust_mut();
        rust.entries = entries;
        rust.next_cursor = None;
    }
    if model.has_next_page {
        model.as_mut().set_has_next_page(false);
    }
    model.as_mut().rebuild_view();
    model.as_mut().set_full_loaded(true);
    model.as_mut().launch_pending_random();
}

impl ffi::FavoritesModel {
    fn row_count(&self, parent: &QModelIndex) -> i32 {
        if parent.is_valid() {
            0
        } else {
            self.count
        }
    }

    /// Resolve a visible row to its entry. Every row-space accessor goes
    /// through this: rows are view positions, not `entries` indices.
    fn entry_at(&self, row: i32) -> Option<&MediaItem> {
        self.entries.get(self.entry_index_at(row)?)
    }

    /// Resolve a visible row to its index in `entries`. Async work must carry
    /// this rather than the row, because a rebuild moves rows around while
    /// entry indices stay put.
    fn entry_index_at(&self, row: i32) -> Option<usize> {
        let row = usize::try_from(row).ok()?;
        self.view.get(row).copied()
    }

    /// Rebuild the visible order from `entries` for the current sort and
    /// filter. The single writer of `view`, `rev`, `disambig_displays`,
    /// `count` and `total_count`; a full model reset because rows can move
    /// or disappear arbitrarily.
    fn rebuild_view(mut self: Pin<&mut Self>) {
        let sort_mode = self.sort_mode.to_string();
        let filter = self.filter.to_string();
        let show_original = self.show_original_filenames;
        let view = build_view(&self.entries, &sort_mode, &filter, show_original);
        let rev = build_rev(&view, self.entries.len());
        let disambig =
            compute_favorites_disambig_displays_view(&self.entries, &view, show_original);
        let count = i32::try_from(view.len()).unwrap_or(i32::MAX);
        let total = i32::try_from(self.entries.len()).unwrap_or(i32::MAX);
        let facet = build_system_facet_json(&self.entries);

        self.as_mut().begin_reset_model();
        {
            let mut rust = self.as_mut().rust_mut();
            rust.view = view;
            rust.rev = rev;
            rust.disambig_displays = disambig;
            rust.count = count;
        }
        self.as_mut().end_reset_model();
        self.as_mut().count_changed();
        self.as_mut().set_total_count(total);
        self.as_mut()
            .set_system_facet_json(QString::from(facet.as_str()));
    }

    /// Set the row order. `""` restores Core's order, `"name"` sorts A-Z on
    /// the displayed name. Sorting needs every favorite in memory, so this
    /// kicks off the full load when one is still needed.
    fn set_sort_mode(mut self: Pin<&mut Self>, value: &QString) {
        let requested = value.to_string();
        let normalized = match requested.as_str() {
            SORT_NAME => SORT_NAME,
            "" => "",
            other => {
                warn!("ignoring unknown favorites sort mode: {other}");
                return;
            }
        };
        if self.sort_mode.to_string() == normalized {
            return;
        }
        self.as_mut().rust_mut().sort_mode = QString::from(normalized);
        self.as_mut().sort_mode_changed();
        if normalized == SORT_NAME {
            self.as_mut().ensure_full_load();
        }
        self.rebuild_view();
    }

    /// Narrow the list to one system (`sys:<id>`) or category (`cat:<name>`);
    /// `""` shows everything. Also needs the full set loaded, otherwise the
    /// filter would only see the pages fetched so far.
    fn set_filter(mut self: Pin<&mut Self>, value: &QString) {
        let requested = value.to_string();
        if self.filter.to_string() == requested {
            return;
        }
        self.as_mut().rust_mut().filter = QString::from(requested.as_str());
        self.as_mut().filter_changed();
        if !requested.is_empty() {
            self.as_mut().ensure_full_load();
        }
        self.rebuild_view();
    }

    fn data(&self, index: &QModelIndex, role: i32) -> QVariant {
        if !index.is_valid() || index.row() < 0 || index.row() >= self.count {
            return QVariant::default();
        }
        let Some(entry) = self.entry_at(index.row()) else {
            return QVariant::default();
        };
        match role {
            NAME_ROLE => QVariant::from(&QString::from(
                display_name(&entry.name, &entry.path, self.show_original_filenames).as_str(),
            )),
            PATH_ROLE => QVariant::from(&QString::from(entry.path.as_str())),
            SYSTEM_ID_ROLE => QVariant::from(&QString::from(entry.system.id.as_str())),
            COVER_KEY_ROLE => QVariant::from(&QString::from(
                cover_key_for(entry, !self.cover_requests_paused).as_str(),
            )),
            ZAP_SCRIPT_ROLE => QVariant::from(&QString::from(entry.zap_script.as_str())),
            FAVORITE_ROLE => QVariant::from(&favorite_role_value(&entry.tags)),
            FILE_STEM_ROLE => {
                QVariant::from(&QString::from(file_stem_or_name(&entry.path, &entry.name)))
            }
            HIDDEN_ROLE => QVariant::from(&false),
            // Sibling-diffed display string; precomputed in `disambig_displays`.
            DISAMBIGUATING_TAGS_ROLE => QVariant::from(&QString::from(
                self.disambig_displays
                    .get(index.row() as usize)
                    .map_or("", String::as_str),
            )),
            _ => QVariant::default(),
        }
    }

    fn role_names(&self) -> QHash<QHashPair_i32_QByteArray> {
        let mut h = QHash::<QHashPair_i32_QByteArray>::default();
        h.insert(NAME_ROLE, QByteArray::from("name"));
        h.insert(PATH_ROLE, QByteArray::from("path"));
        h.insert(SYSTEM_ID_ROLE, QByteArray::from("systemId"));
        h.insert(COVER_KEY_ROLE, QByteArray::from("coverKey"));
        h.insert(ZAP_SCRIPT_ROLE, QByteArray::from("zapScript"));
        h.insert(FAVORITE_ROLE, QByteArray::from("favorite"));
        h.insert(FILE_STEM_ROLE, QByteArray::from("fileStem"));
        h.insert(HIDDEN_ROLE, QByteArray::from("hidden"));
        h.insert(
            DISAMBIGUATING_TAGS_ROLE,
            QByteArray::from("disambiguatingTags"),
        );
        h
    }

    fn fetch_more(mut self: Pin<&mut Self>) {
        if self.loading_more || !self.has_next_page {
            return;
        }
        let cursor = self.next_cursor.clone();
        let expected_prev_cursor = cursor.clone();
        let seq = self.rust().seq.clone();
        let ticket = seq.load(Ordering::SeqCst);
        self.as_mut().set_loading_more(true);
        let qt_thread = self.qt_thread();
        let store = global_store();
        global_handle().spawn(async move {
            let result = store
                .client()
                .media_search(MediaSearchParams {
                    max_results: Some(PAGE_SIZE),
                    cursor,
                    tags: vec!["user:favorite".into()],
                    ..MediaSearchParams::default()
                })
                .await;
            let _ = qt_thread.queue(move |model| {
                if seq.load(Ordering::SeqCst) != ticket {
                    return;
                }
                apply_append_page(model, result, expected_prev_cursor.as_deref());
            });
        });
    }

    /// Pull every remaining favorite in one request so the view can sort and
    /// the facet can list systems. Core has no sort parameter and pages
    /// favorites in DBID order, so there is no way to do either without the
    /// whole set. Lazy on purpose: the untouched default screen never calls
    /// this, so it keeps the exact fetch behavior it always had.
    fn ensure_full_load(mut self: Pin<&mut Self>) {
        if self.full_loaded || self.full_loading {
            return;
        }
        if !self.has_next_page {
            // Everything already arrived in the first page. Fire a queued
            // press here: no callback is coming, so waiting for one would
            // make the first press do nothing.
            self.as_mut().set_full_loaded(true);
            self.launch_pending_random();
            return;
        }
        self.as_mut().set_full_loading(true);
        // MiSTer fast path: the whole favorite set arrives from one
        // local query (`media_favorites_db`) and upgrades the model in
        // a single apply, replacing the chained RPC drain below. Any
        // `None` falls through to that chain unchanged; the store's
        // endpoint subscription remains the freshness signal either
        // way, so favorite toggles keep refetching exactly as before.
        if crate::media_favorites_db::enabled() {
            let seq = self.rust().seq.clone();
            let ticket = seq.load(Ordering::SeqCst);
            self.as_mut().set_loading_more(true);
            let qt_thread = self.qt_thread();
            global_handle().spawn(async move {
                let cancel_seq = seq.clone();
                let direct = tokio::task::spawn_blocking(move || {
                    crate::media_favorites_db::favorites(&|| {
                        cancel_seq.load(Ordering::SeqCst) != ticket
                    })
                })
                .await
                .ok()
                .flatten();
                let _ = qt_thread.queue(move |model| {
                    if seq.load(Ordering::SeqCst) != ticket {
                        return;
                    }
                    // The scope filter matches on system category and the
                    // filter modal labels on system name; categories are
                    // Core catalog knowledge, not database rows, so the
                    // direct set must be enriched from the catalog
                    // snapshot before it can honestly replace the RPC
                    // set. No Ready catalog, no direct upgrade.
                    match direct.and_then(enrich_from_catalog) {
                        Some(entries) => apply_direct_favorites(model, entries),
                        None => model.start_full_load_rpc(),
                    }
                });
            });
            return;
        }
        self.start_full_load_rpc();
    }

    /// The RPC full-load chain: resume from the cursor and drain to the
    /// end one `FULL_LOAD_SIZE` page at a time. Kept verbatim as the
    /// fallback behind the direct read; `full_loading` is already true
    /// when this runs.
    fn start_full_load_rpc(mut self: Pin<&mut Self>) {
        // Share `seq` with the cursor chain so a fresh `apply_state` (screen
        // re-entry, favorite toggled elsewhere) invalidates this in flight.
        let seq = self.rust().seq.clone();
        let ticket = seq.load(Ordering::SeqCst);
        self.as_mut().set_loading_more(true);
        // Resume from the cursor and APPEND: page 1 is already in `entries`,
        // and above Core's per-request cap this runs again for the next
        // chunk, so a favorites list of any size drains completely.
        //
        // A scroll-driven `fetch_more` may ALREADY be in flight against this
        // same cursor (setting `loading_more` here does not unsend it), so
        // the callback re-checks the cursor before appending the way
        // `apply_append_page` does. Without that, both replies append the
        // same page and every row in it shows up twice.
        let cursor = self.next_cursor.clone();
        let expected_prev_cursor = cursor.clone();
        let qt_thread = self.qt_thread();
        let store = global_store();
        global_handle().spawn(async move {
            let result = store
                .client()
                .media_search(MediaSearchParams {
                    max_results: Some(FULL_LOAD_SIZE),
                    cursor,
                    tags: vec!["user:favorite".into()],
                    ..MediaSearchParams::default()
                })
                .await;
            let _ = qt_thread.queue(move |mut model| {
                if seq.load(Ordering::SeqCst) != ticket {
                    return;
                }
                model.as_mut().set_full_loading(false);
                model.as_mut().set_loading_more(false);
                match result {
                    Ok(result) => {
                        if model.next_cursor != expected_prev_cursor {
                            // A `fetch_more` landed first and already spliced
                            // this page. Appending it again would duplicate
                            // every row; continue from wherever the chain is
                            // now instead.
                            model.as_mut().ensure_full_load();
                            return;
                        }
                        let has_next_page = result.has_next_page();
                        let next_cursor = result.next_cursor();
                        // A page with no rows, or a cursor that did not move,
                        // means the chain cannot advance. Without this the
                        // drain would re-request forever, resetting the model
                        // on every pass.
                        let stalled = result.results.is_empty()
                            || (next_cursor.is_some() && next_cursor == model.next_cursor);
                        {
                            let mut rust = model.as_mut().rust_mut();
                            rust.entries.extend(result.results);
                            rust.next_cursor = next_cursor;
                        }
                        model.as_mut().set_has_next_page(has_next_page);
                        model.as_mut().rebuild_view();
                        if has_next_page && !stalled {
                            // More than one chunk's worth; keep draining.
                            model.as_mut().ensure_full_load();
                        } else {
                            model.as_mut().set_full_loaded(true);
                            model.as_mut().launch_pending_random();
                        }
                    }
                    Err(e) => {
                        warn!("favorites full load failed: {}", e.message);
                        // Don't strand a queued random press waiting forever.
                        if model.random_pending {
                            model.as_mut().fail_random("load-failed");
                        }
                    }
                }
            });
        });
    }

    fn clear_random_error(mut self: Pin<&mut Self>) {
        if !self.random_error.is_empty() {
            self.as_mut().set_random_error(QString::default());
        }
    }

    /// Report a random pick that resolved nothing. A one-shot action that
    /// stays silent is indistinguishable from a broken button.
    fn fail_random(mut self: Pin<&mut Self>, reason: &str) {
        warn!("random favorite pick failed: {reason}");
        self.as_mut().rust_mut().random_pending = false;
        self.as_mut().set_random_error(QString::from(reason));
    }

    /// Fire a random launch that was requested before the full set had
    /// arrived. Without this the first press is a silent no-op whenever the
    /// favorites list spans more than one page.
    fn launch_pending_random(mut self: Pin<&mut Self>) {
        if !self.random_pending {
            return;
        }
        self.as_mut().rust_mut().random_pending = false;
        self.launch_random();
    }

    fn refresh_cover_keys(mut self: Pin<&mut Self>, first_row: i32, count: i32) {
        emit_cover_key_range(self.as_mut(), first_row, count);
    }

    fn clear_pending_cover_requests(self: Pin<&mut Self>) {
        global_media_image_cache().clear_pending_requests();
    }

    fn launch_at(self: Pin<&mut Self>, index: i32) {
        let Some(entry) = self.entry_at(index) else {
            return;
        };
        let text = launch_text_for(entry);
        if text.is_empty() {
            return;
        }
        let name = entry.name.clone();
        let store = global_store();
        global_handle().spawn(async move {
            if let Err(e) = store.run_mutation::<RunMutation>(RunParams { text }).await {
                warn!("run failed for {name}: {}", e.message);
            }
        });
    }

    fn launch_text_at(&self, index: i32) -> QString {
        let Some(entry) = self.entry_at(index) else {
            return QString::default();
        };
        QString::from(portable_text_for_entry(entry).as_str())
    }

    /// Launch a random favorite from the rows currently on screen, so an
    /// active filter is respected.
    ///
    /// Picked here rather than by Core: `**launch.random:all?tags=user:favorite`
    /// returns no rows, because Core's all-systems branch does not compose
    /// with a tag filter. The frontend already holds the favorites it
    /// displays, so selecting locally is both correct and exactly the visible
    /// pool. Requires the full load, otherwise the pool would be whichever
    /// pages happen to be fetched.
    fn launch_random(mut self: Pin<&mut Self>) {
        if !self.random_error.is_empty() {
            self.as_mut().set_random_error(QString::default());
        }
        if self.count <= 0 {
            self.as_mut().fail_random("empty-scope");
            return;
        }
        if !self.full_loaded {
            // Picking from a partial list would bias the result toward the
            // first page, so pull the rest first. `ensure_full_load` fires
            // the queued press when it finishes, synchronously or not.
            self.as_mut().rust_mut().random_pending = true;
            self.as_mut().ensure_full_load();
            return;
        }
        let Ok(count) = usize::try_from(self.count) else {
            self.as_mut().fail_random("empty-scope");
            return;
        };
        let Ok(row) = i32::try_from(random_index(count)) else {
            self.as_mut().fail_random("no-launch-payload");
            return;
        };
        let Some(entry) = self.entry_at(row) else {
            self.as_mut().fail_random("no-launch-payload");
            return;
        };
        let text = launch_text_for(entry);
        if text.is_empty() {
            self.as_mut().fail_random("no-launch-payload");
            return;
        }
        let name = entry.name.clone();
        let store = global_store();
        global_handle().spawn(async move {
            if let Err(e) = store.run_mutation::<RunMutation>(RunParams { text }).await {
                warn!("random favorite run failed for {name}: {}", e.message);
            }
        });
    }

    fn write_card_at(mut self: Pin<&mut Self>, index: i32) {
        let Some(entry) = self.entry_at(index) else {
            self.as_mut()
                .set_card_write_error(QString::from("invalid selection"));
            self.as_mut().set_card_write_pending(false);
            return;
        };
        let text = portable_text_for_entry(entry);
        if text.is_empty() {
            self.as_mut()
                .set_card_write_error(QString::from("missing launch payload"));
            self.as_mut().set_card_write_pending(false);
            return;
        }
        let name = entry.name.clone();
        let store = global_store();
        let seq = self.rust().card_write_seq.clone();
        let ticket = seq.fetch_add(1, Ordering::SeqCst) + 1;
        self.as_mut().set_card_write_error(QString::default());
        self.as_mut().set_card_write_pending(true);
        let qt_thread = self.qt_thread();
        global_handle().spawn(async move {
            let result = store
                .run_mutation::<ReadersWriteMutation>(ReadersWriteParams { text })
                .await;
            let _ = qt_thread.queue(move |mut model| {
                if seq.load(Ordering::SeqCst) != ticket {
                    return;
                }
                let error = match result {
                    Ok(()) => QString::default(),
                    Err(e) => {
                        warn!("card write failed for {name}: {}", e.message);
                        QString::from(e.message.as_str())
                    }
                };
                model.as_mut().set_card_write_error(error);
                model.as_mut().set_card_write_pending(false);
            });
        });
    }

    fn toggle_favorite_at(self: Pin<&mut Self>, index: i32) {
        // Carry the entries index, not the row: by the time Core answers, a
        // sort or filter change may have moved this row somewhere else.
        let Some(entry_index) = self.entry_index_at(index) else {
            return;
        };
        let Some(entry) = self.entries.get(entry_index) else {
            return;
        };
        let Some(params) = favorite_params_for_entry(entry, !has_favorite_tag(&entry.tags)) else {
            warn!(
                "favorite update skipped: missing media identity for {}",
                entry.name
            );
            return;
        };
        let name = entry.name.clone();
        let media_id = entry.media_id;
        let system_id = entry.system.id.clone();
        let path = entry.path.clone();
        let store = global_store();
        let qt_thread = self.qt_thread();
        global_handle().spawn(async move {
            match store.run_mutation::<MediaTagsUpdateMutation>(params).await {
                Ok(result) => {
                    let _ = qt_thread.queue(move |mut model| {
                        apply_favorite_tags(
                            model.as_mut(),
                            entry_index,
                            media_id,
                            &system_id,
                            &path,
                            result.tags,
                        );
                    });
                }
                Err(e) => warn!("favorite update failed for {name}: {}", e.message),
            }
        });
    }

    fn is_favorite_at(&self, index: i32) -> bool {
        self.entry_at(index)
            .is_some_and(|entry| has_favorite_tag(&entry.tags))
    }

    fn cancel_card_write(mut self: Pin<&mut Self>) {
        self.as_mut()
            .rust()
            .card_write_seq
            .fetch_add(1, Ordering::SeqCst);
        if !self.card_write_error.is_empty() {
            self.as_mut().set_card_write_error(QString::default());
        }
        if self.card_write_pending {
            self.as_mut().set_card_write_pending(false);
        }
    }

    fn name_at(&self, index: i32) -> QString {
        let Some(entry) = self.entry_at(index) else {
            return QString::default();
        };
        QString::from(display_name(&entry.name, &entry.path, self.show_original_filenames).as_str())
    }

    // Full (untrimmed) disambiguation tokens for the focused-item readout.
    fn disambiguating_tags_at(&self, index: i32) -> QString {
        let Some(entry) = self.entry_at(index) else {
            return QString::default();
        };
        QString::from(
            disambiguating_tag_labels(&entry.disambiguating_tags)
                .join(" ")
                .as_str(),
        )
    }

    fn set_show_original_filenames(mut self: Pin<&mut Self>, value: bool) {
        if self.show_original_filenames == value {
            return;
        }
        self.as_mut().rust_mut().show_original_filenames = value;
        self.as_mut().show_original_filenames_changed();
        // The displayed name is also the A-Z sort key, so under that sort the
        // whole order changes and a full rebuild is the only correct answer.
        if self.sort_mode.to_string() == SORT_NAME {
            self.rebuild_view();
            return;
        }
        // Displayed name drives sibling grouping, so recompute the trimmed tags.
        let displays = compute_favorites_disambig_displays_view(&self.entries, &self.view, value);
        self.as_mut().rust_mut().disambig_displays = displays;
        let last_row = self.count - 1;
        if last_row >= 0 {
            let mut roles = QList::<i32>::default();
            roles.append(NAME_ROLE);
            roles.append(DISAMBIGUATING_TAGS_ROLE);
            let parent = QModelIndex::default();
            let top_left = self.as_mut().index(0, 0, &parent);
            let bottom_right = self.as_mut().index(last_row, 0, &parent);
            self.as_mut().data_changed(&top_left, &bottom_right, &roles);
        }
    }

    fn path_at(&self, index: i32) -> QString {
        let Some(entry) = self.entry_at(index) else {
            return QString::default();
        };
        QString::from(entry.path.as_str())
    }

    fn system_id_at(&self, index: i32) -> QString {
        let Some(entry) = self.entry_at(index) else {
            return QString::default();
        };
        QString::from(entry.system.id.as_str())
    }

    // Immediate, non-debounced sibling of `load_detail_at`. Called the moment
    // the focused row changes so the detail table reflects THIS row at once —
    // cached metadata (instant), a memoized blank, or a clean blank while a
    // fetch is pending — instead of holding the previous row's values through
    // the load debounce. Also warms neighboring rows' metadata so the next
    // move is a synchronous cache hit. Never issues a foreground RPC.
    fn peek_detail_at(mut self: Pin<&mut Self>, index: i32) {
        self.as_mut()
            .rust_mut()
            .detail_seq
            .fetch_add(1, Ordering::SeqCst);
        if index < 0 || index >= self.count {
            clear_current_detail_state(self.as_mut());
            return;
        }
        self.as_mut().rust_mut().detail_prefetch_row = Some(index);
        prefetch_around_cursor(
            &self.entries,
            &self.view,
            self.count,
            index,
            self.cover_requests_paused,
        );
        let Some(entry) = self.entry_at(index) else {
            clear_current_detail_state(self.as_mut());
            return;
        };
        let system = entry.system.id.clone();
        let path = entry.path.clone();
        if system.trim().is_empty() || path.trim().is_empty() {
            clear_current_detail_state(self.as_mut());
            return;
        }
        // Deliberately do NOT switch the visible cover here. The cover has its
        // own grace-window hold (BrowseDetailPane coverHold) and is settled by
        // the debounced `load_detail_at`. Re-pointing the 512px cover Image on
        // every keypress kept `_coverBusy` true through sustained navigation
        // (tripping the hourglass after the grace) and monopolized Qt's async
        // image loader so the next-row cover never got prefetched. Peek only
        // updates the metadata table and warms the prefetch hints below.
        refresh_adjacent_cover_prefetch(self.as_mut());

        let meta_key = MediaKey::new(system.clone(), path.clone());
        match global_media_meta_cache().lookup(&meta_key) {
            MetaLookup::Hit(meta) => {
                self.as_mut()
                    .set_current_detail_tags(QString::from(detail_tags_from_meta(&meta).as_str()));
                self.as_mut().set_current_detail_loading(false);
            }
            MetaLookup::Negative => {
                self.as_mut().set_current_detail_tags(QString::default());
                self.as_mut().set_current_detail_loading(false);
            }
            MetaLookup::Miss => {
                self.as_mut().set_current_detail_tags(QString::default());
                self.as_mut().set_current_detail_loading(true);
            }
        }

        enqueue_meta_prefetch(&self.entries, &self.view, self.count, index);
    }

    fn load_detail_at(mut self: Pin<&mut Self>, index: i32) {
        let ticket = self
            .as_mut()
            .rust_mut()
            .detail_seq
            .fetch_add(1, Ordering::SeqCst)
            + 1;
        if index < 0 || index >= self.count {
            clear_current_detail_state(self.as_mut());
            return;
        }
        // Record the settled row before borrowing entries.
        self.as_mut().rust_mut().detail_prefetch_row = Some(index);
        // Re-center the byte-fetch queue on the current row so it and
        // its neighbors are fetched ahead of the stale list backlog.
        prefetch_around_cursor(
            &self.entries,
            &self.view,
            self.count,
            index,
            self.cover_requests_paused,
        );
        let Some(entry) = self.entry_at(index) else {
            clear_current_detail_state(self.as_mut());
            return;
        };
        let system = entry.system.id.clone();
        let path = entry.path.clone();
        let media_id = entry.media_id;
        if system.trim().is_empty() || path.trim().is_empty() {
            clear_current_detail_state(self.as_mut());
            return;
        }
        let detail_key = match media_id {
            Some(id) => MediaKey::with_media_id(system.clone(), path.clone(), id),
            None => MediaKey::new(system.clone(), path.clone()),
        }
        .with_current_cover_preference();
        self.as_mut().rust_mut().current_detail_media_key = Some(detail_key);
        self.as_mut().rust_mut().current_detail_media_id = media_id;
        sync_current_detail_image_key(self.as_mut());
        refresh_adjacent_cover_prefetch(self.as_mut());

        // Resolve synchronously from the metadata cache when warm (a neighbor
        // prefetched while dwelling on the previous row, or a revisit), so the
        // table fills with the correct rows on this frame and never re-fetches.
        let meta_key = MediaKey::new(system.clone(), path.clone());
        match global_media_meta_cache().lookup(&meta_key) {
            MetaLookup::Hit(meta) => {
                self.as_mut()
                    .set_current_detail_tags(QString::from(detail_tags_from_meta(&meta).as_str()));
                self.as_mut().set_current_detail_loading(false);
                return;
            }
            MetaLookup::Negative => {
                self.as_mut().set_current_detail_tags(QString::default());
                self.as_mut().set_current_detail_loading(false);
                return;
            }
            MetaLookup::Miss => {}
        }

        self.as_mut().set_current_detail_loading(true);
        self.as_mut().set_current_detail_tags(QString::default());
        let seq = self.rust().detail_seq.clone();
        let qt_thread = self.qt_thread();
        let store = global_store();
        let store_key = meta_key.clone();
        global_handle().spawn(async move {
            let params = MediaMetaParams::for_media(system, path.clone());
            let result = match crate::media_meta_db::media_meta_async(params.clone()).await {
                Some(media) => Ok(zaparoo_core::media_types::MediaMetaResult { media }),
                None => store.client().media_meta(params).await,
            };
            // Cache the outcome (positive or negative) regardless of whether
            // this callback is still current, so a later revisit is instant.
            match &result {
                Ok(r) => global_media_meta_cache().store(store_key, Some(r.media.clone())),
                Err(_) => global_media_meta_cache().store(store_key, None),
            }
            let _ = qt_thread.queue(move |mut model| {
                if seq.load(Ordering::SeqCst) != ticket {
                    return;
                }
                match result {
                    Ok(result) => model.as_mut().set_current_detail_tags(QString::from(
                        detail_tags_from_meta(&result.media).as_str(),
                    )),
                    Err(e) => {
                        model.as_mut().set_current_detail_tags(QString::default());
                        warn!("favorite detail fetch failed for {path}: {}", e.message);
                    }
                }
                model.as_mut().set_current_detail_loading(false);
            });
        });
    }

    fn clear_current_detail(self: Pin<&mut Self>) {
        clear_current_detail_state(self);
    }

    /// Visible row holding `path`, or -1. QML restores the cursor by path, so
    /// this must answer in row space: an entry hidden by the current filter
    /// correctly reports -1 and the screen leaves the cursor where it is.
    fn index_for_path(&self, path: &QString) -> i32 {
        let entry = position_of_path(&self.entries, &path.to_string());
        let Ok(entry) = usize::try_from(entry) else {
            return -1;
        };
        self.rev.get(entry).copied().unwrap_or(-1)
    }

    /// Spin up the long-lived cover-cache subscriber on first use.
    /// Subsequent calls are no-ops — the model singleton owns exactly
    /// one subscriber for the whole process lifetime, decoupled from
    /// `seq` because cover updates are not tied to a particular page
    /// chain. Lagged broadcast frames are dropped silently; the
    /// `dataChanged` we'd otherwise emit is recoverable on the next
    /// scroll because `data()` re-reads the cache for every visible
    /// row's `coverKey`.
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
                            notify_cover_update(model, &update.key);
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

/// Resolve the cover URL key for a favorites row. Mirrors `GamesModel`'s
/// path: when the in-memory cache has bytes for `(systemId, mediaPath)`
/// we hand back the `media-image/<encoded>` key the
/// `QQuickImageProvider` resolves to RAM bytes; otherwise we enqueue a
/// fetch (carrying the optional `mediaId` hint) and fall back to the
/// system logo as a nicer placeholder than the generic file glyph.
fn cover_key_for(entry: &MediaItem, requests_enabled: bool) -> String {
    if entry.system.id.is_empty() {
        return "icons/File".to_string();
    }
    let media_key = media_key_for(entry).map(MediaKey::with_current_cover_preference);
    let cache = global_media_image_cache();
    let cached = media_key.as_ref().is_some_and(|k| cache.is_cached(k));
    let negative = media_key.as_ref().is_some_and(|k| cache.is_negative(k));
    let soft_no_image = media_key
        .as_ref()
        .is_some_and(|k| cache.is_soft_no_image(k));
    if requests_enabled && !cached && !negative && !soft_no_image {
        // Miss-driven re-enqueue, same rationale as GamesModel's
        // `cover_key_for`: tiles re-bound after LRU eviction or stale-
        // enqueue truncation will hit this branch and re-arm the fetch.
        if let Some(k) = media_key.as_ref() {
            cache.enqueue_search_cover_with_media_id(k.clone(), entry.media_id, PAGE_SIZE);
        }
    }
    cover_key_for_with(
        entry,
        media_key.as_ref(),
        cached,
        negative,
        soft_no_image || !requests_enabled,
    )
}

fn emit_cover_key_range(mut model: Pin<&mut ffi::FavoritesModel>, first_row: i32, count: i32) {
    if model.count <= 0 || count <= 0 {
        return;
    }
    let first = first_row.clamp(0, model.count - 1);
    let last = first.saturating_add(count - 1).min(model.count - 1);
    if last < first {
        return;
    }
    let mut roles = QList::<i32>::default();
    roles.append(COVER_KEY_ROLE);
    let parent = QModelIndex::default();
    let top_left = model.index(first, 0, &parent);
    let bottom_right = model.index(last, 0, &parent);
    model
        .as_mut()
        .data_changed(&top_left, &bottom_right, &roles);
}

/// Build the canonical `(systemId, mediaPath)` identifier for a search
/// row. Returns `None` for rows without enough info to key on.
fn media_key_for(entry: &MediaItem) -> Option<MediaKey> {
    if entry.system.id.is_empty() || entry.path.is_empty() {
        return None;
    }
    match entry.media_id {
        Some(media_id) => Some(MediaKey::with_media_id(
            entry.system.id.clone(),
            entry.path.clone(),
            media_id,
        )),
        None => Some(MediaKey::new(entry.system.id.clone(), entry.path.clone())),
    }
}

fn has_favorite_tag(tags: &[TagInfo]) -> bool {
    tags.iter()
        .any(|tag| tag.tag_type == "user" && tag.tag == "favorite")
}

fn favorite_role_value(tags: &[TagInfo]) -> i32 {
    i32::from(has_favorite_tag(tags))
}

fn detail_tags_from_meta(meta: &MediaMeta) -> String {
    let source = if meta.title.tags.is_empty() {
        meta.tags.as_slice()
    } else {
        meta.title.tags.as_slice()
    };
    let rows = [
        (
            "Year",
            detail_value_for_aliases(source, &["year", "release date", "release_date"]),
        ),
        (
            "Genre",
            detail_value_for_aliases(source, &["genre", "gamegenre"]),
        ),
        ("Players", detail_value_for_aliases(source, &["players"])),
        (
            "Developer",
            detail_value_for_aliases(source, &["developer"]),
        ),
        (
            "Publisher",
            detail_value_for_aliases(source, &["publisher"]),
        ),
        ("Rating", detail_value_for_aliases(source, &["rating"])),
    ];
    rows.into_iter()
        .map(|(label, value)| format!("{label}\t{value}"))
        .collect::<Vec<_>>()
        .join("\n")
}

// Warm the metadata cache for the rows immediately around `row` so a move to
// a neighbor is a synchronous cache hit. Best-effort and fire-and-forget;
// already-cached or in-flight keys are skipped inside the cache.
/// Warm metadata for the rows either side of the cursor. Neighbors are
/// resolved through `view`, so a sorted or filtered list warms what the user
/// will actually scroll onto rather than whatever sits next in fetch order.
fn enqueue_meta_prefetch(entries: &[MediaItem], view: &[usize], count: i32, row: i32) {
    let mut requests = Vec::new();
    for delta in [-2_i32, -1, 1, 2] {
        let i = row + delta;
        if i < 0 || i >= count {
            continue;
        }
        let Some(entry) = usize::try_from(i)
            .ok()
            .and_then(|r| view.get(r))
            .and_then(|&e| entries.get(e))
        else {
            continue;
        };
        let system = entry.system.id.clone();
        let path = entry.path.clone();
        if system.trim().is_empty() || path.trim().is_empty() {
            continue;
        }
        requests.push((
            MediaKey::new(system.clone(), path.clone()),
            MediaMetaParams::for_media(system, path),
        ));
    }
    if !requests.is_empty() {
        global_media_meta_cache().prefetch(requests);
    }
}

fn detail_value_for_aliases(source: &[TagInfo], aliases: &[&str]) -> String {
    source
        .iter()
        .filter(|tag| {
            aliases
                .iter()
                .any(|alias| tag.tag_type.eq_ignore_ascii_case(alias))
                && !tag_display_value(tag).is_empty()
        })
        .map(tag_display_value)
        .collect::<Vec<_>>()
        .join(", ")
}

fn file_stem_or_name(path: &str, name: &str) -> String {
    let file = path
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default();
    let stem = file.rsplit_once('.').map_or(file, |(stem, _)| stem).trim();
    if stem.is_empty() {
        name.to_string()
    } else {
        stem.to_string()
    }
}

/// Resolve the user-visible name, honoring the `show_original_filenames`
/// setting: the original filename (sans extension) when enabled, otherwise
/// Core's cleaned display name.
fn display_name(name: &str, path: &str, show_original_filenames: bool) -> String {
    if show_original_filenames {
        file_stem_or_name(path, name)
    } else {
        name.to_string()
    }
}

/// Sibling-diffed disambiguation displays for the visible rows, in display
/// order (see the shared `sibling_disambiguation_displays`). Grouping keys off
/// the displayed name so the original-filename toggle disables grouping
/// naturally. Adjacency-based, so it must follow the view: sorting brings
/// same-named variants next to each other that Core's order kept apart, and
/// the badges have to group with them.
fn compute_favorites_disambig_displays_view(
    entries: &[MediaItem],
    view: &[usize],
    show_original: bool,
) -> Vec<String> {
    let rows: Vec<(String, Vec<String>)> = view
        .iter()
        .filter_map(|&i| entries.get(i))
        .map(|e| {
            (
                display_name(&e.name, &e.path, show_original),
                disambiguating_tag_labels(&e.disambiguating_tags),
            )
        })
        .collect();
    sibling_disambiguation_displays(&rows)
}

/// True when `entry` passes `filter`. Empty filter keeps everything;
/// `sys:<id>` keeps one system; `cat:<category>` keeps one category
/// (case-insensitive, because Core's category casing is its own).
/// An unparseable filter keeps everything rather than emptying the screen.
fn entry_matches_filter(entry: &MediaItem, filter: &str) -> bool {
    match filter.split_once(':') {
        Some(("sys", id)) => entry.system.id == id,
        Some(("cat", category)) => entry.system.category.eq_ignore_ascii_case(category),
        _ => true,
    }
}

/// Row order for the given sort and filter. Returns indices into `entries`;
/// `entries` itself is never reordered, so the cursor chain and the append
/// path keep working on a stable list.
fn build_view(
    entries: &[MediaItem],
    sort_mode: &str,
    filter: &str,
    show_original: bool,
) -> Vec<usize> {
    let mut view: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter(|(_, e)| entry_matches_filter(e, filter))
        .map(|(i, _)| i)
        .collect();
    if sort_mode == SORT_NAME {
        // Tiebreak on path so equal names keep a stable order across
        // rebuilds instead of shuffling under the cursor.
        view.sort_by(|&a, &b| {
            let ea = &entries[a];
            let eb = &entries[b];
            display_name(&ea.name, &ea.path, show_original)
                .to_lowercase()
                .cmp(&display_name(&eb.name, &eb.path, show_original).to_lowercase())
                .then_with(|| ea.path.cmp(&eb.path))
        });
    }
    view
}

/// Inverse of `view`: entry index to visible row, `-1` when filtered out.
fn build_rev(view: &[usize], entry_count: usize) -> Vec<i32> {
    let mut rev = vec![-1; entry_count];
    for (row, &entry) in view.iter().enumerate() {
        if let Some(slot) = rev.get_mut(entry) {
            *slot = i32::try_from(row).unwrap_or(-1);
        }
    }
    rev
}

/// JSON facet of the systems present in the loaded favorites, so the filter
/// picker can only offer scopes that actually match something. Ordered by
/// display name for a stable, readable picker.
fn build_system_facet_json(entries: &[MediaItem]) -> String {
    let mut systems: Vec<(String, String, String, i32)> = Vec::new();
    for entry in entries {
        if let Some(existing) = systems.iter_mut().find(|(id, ..)| *id == entry.system.id) {
            existing.3 += 1;
        } else {
            systems.push((
                entry.system.id.clone(),
                entry.system.name.clone(),
                entry.system.category.clone(),
                1,
            ));
        }
    }
    systems.sort_by_key(|s| s.1.to_lowercase());
    let mut json = String::from("[");
    for (i, (id, name, category, count)) in systems.iter().enumerate() {
        if i > 0 {
            json.push(',');
        }
        json.push_str("{\"id\":\"");
        push_json_escaped(&mut json, id);
        json.push_str("\",\"name\":\"");
        push_json_escaped(&mut json, name);
        json.push_str("\",\"category\":\"");
        push_json_escaped(&mut json, category);
        json.push_str("\",\"count\":");
        json.push_str(&count.to_string());
        json.push('}');
    }
    json.push(']');
    json
}

/// Append `value` as the body of a JSON string literal. Hand-rolled because
/// the frontend crate has no JSON dependency and this is the only place that
/// needs one; system names come from Core, so quotes and backslashes are
/// possible and control characters must not break the payload QML parses.
fn push_json_escaped(out: &mut String, value: &str) {
    use std::fmt::Write as _;

    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                // Control characters must be escaped or the payload QML
                // parses is malformed. Writing straight into `out` avoids a
                // temporary String per character.
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
}

/// Recompute disambiguation displays for the boundary group + appended rows
/// after an `extend` starting at `insert_first`, splicing onto
/// `disambig_displays`. Returns the boundary group's start index.
fn recompute_favorites_disambig_tail(
    mut model: Pin<&mut ffi::FavoritesModel>,
    insert_first: usize,
) -> usize {
    let show_original = model.show_original_filenames;
    let (group_start, new_tail) = {
        let entries = &model.entries;
        let total = entries.len();
        let mut group_start = insert_first.min(total);
        if group_start > 0 {
            let boundary = display_name(
                &entries[group_start - 1].name,
                &entries[group_start - 1].path,
                show_original,
            );
            while group_start > 0
                && display_name(
                    &entries[group_start - 1].name,
                    &entries[group_start - 1].path,
                    show_original,
                ) == boundary
            {
                group_start -= 1;
            }
        }
        let rows: Vec<(String, Vec<String>)> = entries[group_start..total]
            .iter()
            .map(|e| {
                (
                    display_name(&e.name, &e.path, show_original),
                    disambiguating_tag_labels(&e.disambiguating_tags),
                )
            })
            .collect();
        (group_start, sibling_disambiguation_displays(&rows))
    };
    let displays = &mut model.as_mut().rust_mut().disambig_displays;
    displays.truncate(group_start);
    displays.extend(new_tail);
    group_start
}

fn clear_current_detail_state(mut model: Pin<&mut ffi::FavoritesModel>) {
    model
        .as_mut()
        .rust_mut()
        .detail_seq
        .fetch_add(1, Ordering::SeqCst);
    model.as_mut().rust_mut().current_detail_media_key = None;
    model.as_mut().rust_mut().current_detail_media_id = None;
    model.as_mut().set_current_detail_loading(false);
    model.as_mut().set_current_detail_tags(QString::default());
    model
        .as_mut()
        .set_current_detail_image_key(QString::default());
    clear_adjacent_cover_prefetch(model);
}

fn clear_adjacent_cover_prefetch(mut model: Pin<&mut ffi::FavoritesModel>) {
    model.as_mut().rust_mut().detail_prefetch_row = None;
    if !model.detail_prefetch_key_next.is_empty() {
        model
            .as_mut()
            .set_detail_prefetch_key_next(QString::default());
    }
    if !model.detail_prefetch_key_prev.is_empty() {
        model
            .as_mut()
            .set_detail_prefetch_key_prev(QString::default());
    }
}

fn refresh_adjacent_cover_prefetch(mut model: Pin<&mut ffi::FavoritesModel>) {
    let Some(row) = model.rust().detail_prefetch_row else {
        if !model.detail_prefetch_key_next.is_empty() {
            model
                .as_mut()
                .set_detail_prefetch_key_next(QString::default());
        }
        if !model.detail_prefetch_key_prev.is_empty() {
            model
                .as_mut()
                .set_detail_prefetch_key_prev(QString::default());
        }
        return;
    };
    let count = model.count;
    let requests_enabled = !model.cover_requests_paused;

    let next_key = if row + 1 < count {
        cover_key_for(&model.entries[(row + 1) as usize], requests_enabled)
    } else {
        String::new()
    };
    let prev_key = if row > 0 {
        cover_key_for(&model.entries[(row - 1) as usize], requests_enabled)
    } else {
        String::new()
    };

    if model.detail_prefetch_key_next.to_string() != next_key {
        model
            .as_mut()
            .set_detail_prefetch_key_next(QString::from(next_key.as_str()));
    }
    if model.detail_prefetch_key_prev.to_string() != prev_key {
        model
            .as_mut()
            .set_detail_prefetch_key_prev(QString::from(prev_key.as_str()));
    }
}

fn sync_current_detail_image_key(mut model: Pin<&mut ffi::FavoritesModel>) {
    let Some(key) = model.current_detail_media_key.clone() else {
        model
            .as_mut()
            .set_current_detail_image_key(QString::default());
        return;
    };
    let cache = global_media_image_cache();
    if cache.is_cached(&key) {
        model.as_mut().set_current_detail_image_key(QString::from(
            MediaImageCache::image_key_for(&key).as_str(),
        ));
    } else if cache.is_negative(&key) || cache.is_soft_no_image(&key) {
        model
            .as_mut()
            .set_current_detail_image_key(QString::default());
    } else {
        cache.enqueue_search_cover_with_media_id(key, model.current_detail_media_id, 1);
        model
            .as_mut()
            .set_current_detail_image_key(QString::from("icons/Loading"));
    }
}

fn favorite_params_for_entry(entry: &MediaItem, add: bool) -> Option<MediaTagsUpdateParams> {
    let mut params = MediaTagsUpdateParams::default();
    if add {
        params.add.push("user:favorite".to_string());
    } else {
        params.remove.push("user:favorite".to_string());
    }
    if let Some(media_id) = entry.media_id {
        params.media_id = Some(media_id);
        return Some(params);
    }

    if entry.system.id.is_empty() || entry.path.is_empty() {
        return None;
    }
    params.system.clone_from(&entry.system.id);
    params.path.clone_from(&entry.path);
    Some(params)
}

/// Write Core's updated tag list back onto the entry the toggle targeted.
/// `entry_index` indexes `entries`, not the visible rows: the row can move
/// (or vanish under a filter) between dispatch and reply.
fn apply_favorite_tags(
    mut model: Pin<&mut ffi::FavoritesModel>,
    entry_index: usize,
    media_id: Option<i64>,
    system_id: &str,
    path: &str,
    tags: Vec<TagInfo>,
) {
    let Some(entry) = model.entries.get(entry_index) else {
        return;
    };
    let same_entry = if media_id.is_some() {
        entry.media_id == media_id
    } else {
        entry.system.id == system_id && entry.path == path
    };
    if !same_entry {
        return;
    }
    model.as_mut().rust_mut().entries[entry_index].tags = tags;
    // Only repaint when the entry is actually on screen; a filtered-out row
    // has no index to emit against.
    let Some(&row) = model.rev.get(entry_index) else {
        return;
    };
    if row < 0 {
        return;
    }
    let mut roles = QList::<i32>::default();
    roles.append(FAVORITE_ROLE);
    let parent = QModelIndex::default();
    let idx = model.index(row, 0, &parent);
    model.as_mut().data_changed(&idx, &idx, &roles);
}

/// Pure helper for `cover_key_for`. Split out so tests can drive the
/// branches (cached, in-flight, negative-memoed, unattributed)
/// without spinning up the global cover cache and its tokio runtime.
///
/// In-flight (has a key, not cached, not negatively memoed) returns
/// the hourglass — same convention as `GamesModel`. Negatively memoed
/// rows fall back to the system logo, which is a friendlier "no cover
/// available" cue for the favorites/recents lists than `icons/File`.
fn cover_key_for_with(
    entry: &MediaItem,
    key: Option<&MediaKey>,
    cached: bool,
    negative: bool,
    soft_no_image: bool,
) -> String {
    if entry.system.id.is_empty() {
        return "icons/File".to_string();
    }
    match key {
        Some(k) if cached => MediaImageCache::image_key_for(k),
        Some(_) if !negative && !soft_no_image => "icons/Loading".to_string(),
        _ => format!("systems/{}", entry.system.id),
    }
}

/// Schedule a cover fetch for every search row with a non-empty
/// `(systemId, mediaPath)`. The cache enqueue is idempotent —
/// already-cached, already-pending, or negatively-memoised keys are
/// dropped — so spamming this from `apply_state` / `apply_append_page`
/// is cheap.
///
/// Iterates `entries` in reverse so the LIFO fetch queue drains in
/// visual order: the last entry pushed is `entries[0]`, which the
/// driver pops first. Forward iteration starves the top of the page.
fn enqueue_favorites_covers(results: &[MediaItem]) {
    let cache = global_media_image_cache();
    for entry in results.iter().rev() {
        if let Some(key) = media_key_for(entry).map(MediaKey::with_current_cover_preference) {
            cache.enqueue_search_cover_with_media_id(key, entry.media_id, PAGE_SIZE);
        }
    }
}

/// Re-center the byte-fetch queue on the settled cursor row so covers
/// for the current position and its immediate neighbors are fetched
/// first, ahead of the stale top-of-list backlog. See recents.rs for
/// rationale; identical logic adapted for `MediaItem` entries.
/// Warm covers around the cursor. Walks DISPLAY order (`view`), because the
/// rows next to the user are the view's neighbors; stepping through `entries`
/// would warm whatever happened to be adjacent in Core's fetch order.
fn prefetch_around_cursor(
    entries: &[MediaItem],
    view: &[usize],
    count: i32,
    row: i32,
    requests_paused: bool,
) {
    let cache = global_media_image_cache();
    if requests_paused {
        cache.clear_pending_requests();
        return;
    }
    if count <= 0 {
        return;
    }
    let row = row.clamp(0, count - 1);
    let fwd_end = (row + 1 + COVER_PREFETCH_CURSOR_NEXT).min(count);
    let back_start = (row - COVER_PREFETCH_CURSOR_PREV).max(0);
    let mut plan: Vec<(MediaKey, Option<i64>)> = Vec::new();
    let push_row = |i: i32, plan: &mut Vec<(MediaKey, Option<i64>)>| {
        let Some(e) = usize::try_from(i)
            .ok()
            .and_then(|r| view.get(r))
            .and_then(|&entry| entries.get(entry))
        else {
            return;
        };
        if let Some(key) = media_key_for(e).map(MediaKey::with_current_cover_preference) {
            plan.push((key, e.media_id));
        }
    };
    for i in row..fwd_end {
        push_row(i, &mut plan);
    }
    for i in (back_start..row).rev() {
        push_row(i, &mut plan);
    }
    cache.replace_pending_requests_ordered(plan, PAGE_SIZE);
}

fn finish_nav_timing(
    mut model: Pin<&mut ffi::FavoritesModel>,
    reason: &'static str,
    pending_remaining: usize,
) {
    if let Some(timing) = model.as_mut().rust_mut().nav_timing.take() {
        timing.log_release("favorites", reason, pending_remaining);
    }
}

/// Emit `dataChanged(coverKey)` for every row whose entry's
/// `(systemId, mediaPath)` matches `key`. Cheap walk of the current
/// `entries` vec — favorites pages top out at a few hundred rows after
/// look-ahead, and the bridge runs only when the cover-cache fetch
/// driver delivers a result.
///
/// Also drains `pending_first_paint_keys`: each cover landing during
/// the gate's hold ticks the set down, and emptying the set releases
/// the gate so the screen-flip overlay clears.
fn notify_cover_update(mut model: Pin<&mut ffi::FavoritesModel>, key: &MediaKey) {
    // The scan yields entry indices; repaint needs rows, and an entry hidden
    // by the current filter has none.
    let rows: Vec<i32> = model
        .entries
        .iter()
        .enumerate()
        .filter(|(_, e)| {
            key.is_cover_key()
                && match (key.media_id, e.media_id) {
                    (Some(a), Some(b)) => a == b,
                    _ => e.path == *key.path && e.system.id == *key.system_id,
                }
        })
        .filter_map(|(i, _)| model.rev.get(i).copied())
        .filter(|&row| row >= 0)
        .collect();
    if !rows.is_empty() {
        let mut roles = QList::<i32>::default();
        roles.append(COVER_KEY_ROLE);
        let parent = QModelIndex::default();
        for row in rows {
            let idx = model.index(row, 0, &parent);
            model.as_mut().data_changed(&idx, &idx, &roles);
        }
    }
    if model
        .current_detail_media_key
        .as_ref()
        .is_some_and(|current| current == key)
    {
        sync_current_detail_image_key(model.as_mut());
    }
    // Tick the gate's pending set down. `remove` returns false if the
    // key wasn't gated (broadcast events fire for every cache update,
    // including miss-recovery enqueues from `cover_key_for`); we only
    // try to release when a gated key was actually drained.
    let was_pending = model
        .as_mut()
        .rust_mut()
        .pending_first_paint_keys
        .remove(key);
    if was_pending && model.pending_first_paint_keys.is_empty() && model.loading {
        if let Some(handle) = model.as_mut().rust_mut().cover_gate_timer.take() {
            handle.abort();
        }
        // Bytes are cached, but QML's `MediaImageProvider` still has to
        // decode them. The hidden cover pre-warmer in
        // `FavoritesScreen.qml` dispatches all N requests at once and the
        // provider's 4-worker pool decodes them in ~75–150 ms; without
        // this settle window the gate flips `loading=false` before the
        // last few decodes complete and the grid materialises with
        // those tiles still showing the procedural fallback. Mirrors
        // the same hand-off in `games.rs::notify_cover_update`. Same
        // seq-ticket guard as the safety timer so a model reset
        // cancels the pending release.
        info!("favorites: cover gate bytes settled — entering decode-settle window");
        let seq = model.rust().cover_gate_seq.clone();
        let ticket = seq.fetch_add(1, Ordering::SeqCst) + 1;
        let qt_thread = model.qt_thread();
        let handle = global_handle().spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            let _ = qt_thread.queue(move |mut model: Pin<&mut ffi::FavoritesModel>| {
                if seq.load(Ordering::SeqCst) != ticket {
                    return;
                }
                model.as_mut().rust_mut().cover_gate_timer = None;
                if model.loading {
                    info!("favorites: cover gate released after decode-settle window");
                    model.as_mut().set_loading(false);
                }
                finish_nav_timing(model.as_mut(), "covers-ready", 0);
            });
        });
        model.as_mut().rust_mut().cover_gate_timer = Some(handle);
    }
    // Re-check the adjacent preload keys: a neighbor's bytes may have
    // just landed, upgrading its key from `icons/Loading` to
    // `media-image/...` so the hidden Image can start decoding.
    refresh_adjacent_cover_prefetch(model);
}

/// Compute the set of media keys on the current page whose covers we
/// must wait on before releasing the cover gate. Rows without enough
/// info to key on, already-cached keys, and negatively-memoised keys
/// are all excluded. Pure helper so the gate's binning logic is unit-
/// testable without spinning up the global cache + tokio runtime.
fn compute_unresolved_keys<F, G>(
    entries: &[MediaItem],
    is_cached: F,
    is_negative: G,
) -> HashSet<MediaKey>
where
    F: Fn(&MediaKey) -> bool,
    G: Fn(&MediaKey) -> bool,
{
    entries
        .iter()
        .filter_map(|entry| media_key_for(entry).map(MediaKey::with_current_cover_preference))
        .filter(|k| !is_cached(k) && !is_negative(k))
        .collect()
}

/// Decide whether to hold `loading=true` until the page's covers are
/// cached, or release immediately. Called once per Ready `apply_state`.
///
/// - If every search row's cover is already cached or negatively-
///   memoised, set loading=false right now — the screen-flip overlay
///   clears.
/// - Otherwise, store the unresolved set on the model, arm a 3 s
///   safety timer, and leave loading=true. `notify_cover_update` will
///   drain the set as covers land; whichever happens first (set
///   empties or timer fires) releases the gate.
fn arm_cover_gate(mut model: Pin<&mut ffi::FavoritesModel>) {
    if let Some(handle) = model.as_mut().rust_mut().cover_gate_timer.take() {
        handle.abort();
    }
    let cache = global_media_image_cache();
    // Only rows that are actually on screen gate the paint. A filtered-out
    // entry has no tile to wait for, and holding `loading` on it would stall
    // the screen for a cover nobody sees.
    let visible: Vec<MediaItem> = model
        .view
        .iter()
        .filter_map(|&i| model.entries.get(i))
        .cloned()
        .collect();
    let cover_keys = visible
        .iter()
        .filter_map(|entry| media_key_for(entry).map(MediaKey::with_current_cover_preference))
        .collect::<Vec<_>>();
    let cover_total = cover_keys.len();
    let cover_cache_hits = cover_keys.iter().filter(|k| cache.is_cached(k)).count();
    let unresolved = compute_unresolved_keys(
        &visible,
        |k| cache.is_cached(k),
        |k| cache.is_negative(k) || cache.is_soft_no_image(k),
    );
    if let Some(timing) = model.as_mut().rust_mut().nav_timing.as_mut() {
        timing.start_gate(cover_total, cover_cache_hits, unresolved.len());
    }
    if unresolved.is_empty() {
        model.as_mut().rust_mut().pending_first_paint_keys.clear();
        if model.loading {
            model.as_mut().set_loading(false);
        }
        finish_nav_timing(model.as_mut(), "covers-ready", 0);
        return;
    }
    info!(
        pending = unresolved.len(),
        "favorites: arm cover gate (holding loading until covers cached)"
    );
    model.as_mut().rust_mut().pending_first_paint_keys = unresolved;
    let seq = model.rust().cover_gate_seq.clone();
    let ticket = seq.fetch_add(1, Ordering::SeqCst) + 1;
    let qt_thread = model.qt_thread();
    let handle = global_handle().spawn(async move {
        tokio::time::sleep(Duration::from_secs(3)).await;
        let _ = qt_thread.queue(move |model| {
            if seq.load(Ordering::SeqCst) != ticket {
                return;
            }
            release_cover_gate_after_timeout(model);
        });
    });
    model.as_mut().rust_mut().cover_gate_timer = Some(handle);
}

/// Tear down any active cover gate. Used by Pending/Errored apply paths
/// to invalidate an in-flight timer's queued callback (via the seq
/// bump) before the next Ready installs a fresh one.
fn disarm_cover_gate(mut model: Pin<&mut ffi::FavoritesModel>) {
    if let Some(handle) = model.as_mut().rust_mut().cover_gate_timer.take() {
        handle.abort();
    }
    model.as_mut().rust_mut().pending_first_paint_keys.clear();
    model.rust().cover_gate_seq.fetch_add(1, Ordering::SeqCst);
}

/// Force-release the cover gate from the safety timer. Called only via
/// the timer's queued callback after a seq-match check; the
/// notify-driven release path lives inline in `notify_cover_update`.
fn release_cover_gate_after_timeout(mut model: Pin<&mut ffi::FavoritesModel>) {
    let pending = model.pending_first_paint_keys.len();
    info!(pending, "favorites: cover gate timed out, releasing");
    model.as_mut().rust_mut().pending_first_paint_keys.clear();
    model.as_mut().rust_mut().cover_gate_timer = None;
    if model.loading {
        model.as_mut().set_loading(false);
    }
    finish_nav_timing(model.as_mut(), "timeout", pending);
}

/// Build the `text` payload sent to Core's `run` for a search entry.
/// Runtime launches prefer exact paths to avoid title/ZapScript
/// ambiguity; portable write/QR paths prefer Core's `ZapScript`.
fn launch_text_for(entry: &MediaItem) -> String {
    if !entry.path.trim().is_empty() {
        return entry.path.clone();
    }
    entry.zap_script.clone()
}

fn portable_text_for_entry(entry: &MediaItem) -> String {
    if !entry.zap_script.trim().is_empty() {
        return entry.zap_script.clone();
    }
    entry.path.clone()
}

fn position_of_path(entries: &[MediaItem], needle: &str) -> i32 {
    if needle.is_empty() {
        return -1;
    }
    entries
        .iter()
        .position(|e| e.path == needle)
        .map_or(-1, |i| i as i32)
}

fn apply_append_page(
    mut model: Pin<&mut ffi::FavoritesModel>,
    result: Result<MediaSearchResult, ClientError>,
    expected_prev_cursor: Option<&str>,
) {
    if model.next_cursor.as_deref() != expected_prev_cursor {
        if model.loading_more {
            model.as_mut().set_loading_more(false);
        }
        return;
    }
    match result {
        Ok(result) => {
            let has_next_page = result.has_next_page();
            let next_cursor = result.next_cursor();
            let new_count = i32::try_from(result.results.len()).unwrap_or(i32::MAX - model.count);
            if !model.cover_requests_paused {
                enqueue_favorites_covers(&result.results);
            }
            if new_count > 0 {
                // Under a sort or filter the new rows land anywhere in the
                // view, so the contiguous-tail insert below would be wrong.
                // Extend the backing list quietly and rebuild the order.
                let reordered =
                    !model.sort_mode.to_string().is_empty() || !model.filter.to_string().is_empty();
                if reordered {
                    model.as_mut().rust_mut().entries.extend(result.results);
                    model.as_mut().rebuild_view();
                } else {
                    let first = model.count;
                    let last = first.saturating_add(new_count).saturating_sub(1);
                    let parent = QModelIndex::default();
                    model.as_mut().begin_insert_rows(&parent, first, last);
                    {
                        let mut rust = model.as_mut().rust_mut();
                        let appended_from = rust.entries.len();
                        rust.entries.extend(result.results);
                        let appended_to = rust.entries.len();
                        // Default view is the identity order, so the new
                        // entries are simply the next rows.
                        rust.view.extend(appended_from..appended_to);
                        rust.rev.extend(
                            (appended_from..appended_to).map(|i| i32::try_from(i).unwrap_or(-1)),
                        );
                        rust.count = first.saturating_add(new_count);
                    }
                    let group_start =
                        recompute_favorites_disambig_tail(model.as_mut(), first as usize);
                    model.as_mut().end_insert_rows();
                    model.as_mut().count_changed();
                    let total = i32::try_from(model.entries.len()).unwrap_or(i32::MAX);
                    model.as_mut().set_total_count(total);
                    let group_start = i32::try_from(group_start).unwrap_or(0);
                    if group_start < first {
                        let mut roles = QList::<i32>::default();
                        roles.append(DISAMBIGUATING_TAGS_ROLE);
                        let parent = QModelIndex::default();
                        let top_left = model.as_mut().index(group_start, 0, &parent);
                        let bottom_right = model.as_mut().index(first - 1, 0, &parent);
                        model
                            .as_mut()
                            .data_changed(&top_left, &bottom_right, &roles);
                    }
                }
            }
            model.as_mut().rust_mut().next_cursor = next_cursor;
            model.as_mut().set_has_next_page(has_next_page);
            model.as_mut().set_loading_more(false);
        }
        Err(e) => {
            warn!("media.search follow-up page failed: {}", e.message);
            model
                .as_mut()
                .set_error_message(QString::from(e.message.as_str()));
            model.as_mut().set_loading_more(false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn favorite_entry() -> MediaItem {
        MediaItem {
            name: "Favorite".to_string(),
            path: "/games/favorite.rom".to_string(),
            system: zaparoo_core::media_types::System {
                id: "SNES".to_string(),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn entry_in(name: &str, system_id: &str, category: &str) -> MediaItem {
        MediaItem {
            name: name.to_string(),
            path: format!("/games/{system_id}/{name}.rom"),
            system: zaparoo_core::media_types::System {
                id: system_id.to_string(),
                name: format!("{system_id} System"),
                category: category.to_string(),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn sample_library() -> Vec<MediaItem> {
        vec![
            entry_in("Zelda", "NES", "Consoles"),
            entry_in("Alex Kidd", "Genesis", "Consoles"),
            entry_in("mario", "NES", "Consoles"),
            entry_in("Pang", "MAME", "Arcade"),
        ]
    }

    #[test]
    fn default_view_preserves_core_order() {
        let entries = sample_library();
        let view = build_view(&entries, "", "", false);
        assert_eq!(view, vec![0, 1, 2, 3], "empty sort keeps the fetch order");
    }

    #[test]
    fn name_sort_is_alphabetical_and_case_insensitive() {
        let entries = sample_library();
        let view = build_view(&entries, SORT_NAME, "", false);
        let names: Vec<&str> = view.iter().map(|&i| entries[i].name.as_str()).collect();
        // "mario" sorts between "Alex Kidd" and "Pang" only if case is
        // ignored; a byte-wise sort would push it after every capital.
        assert_eq!(names, vec!["Alex Kidd", "mario", "Pang", "Zelda"]);
    }

    #[test]
    fn name_sort_breaks_ties_on_path_for_stability() {
        // Same displayed name twice: without a tiebreak the order could
        // differ between rebuilds and rows would swap under the cursor.
        let entries = vec![
            MediaItem {
                name: "Contra".into(),
                path: "/games/NES/b.rom".into(),
                ..Default::default()
            },
            MediaItem {
                name: "Contra".into(),
                path: "/games/NES/a.rom".into(),
                ..Default::default()
            },
        ];
        assert_eq!(build_view(&entries, SORT_NAME, "", false), vec![1, 0]);
        assert_eq!(build_view(&entries, SORT_NAME, "", false), vec![1, 0]);
    }

    #[test]
    fn filter_by_system_keeps_only_that_system() {
        let entries = sample_library();
        let view = build_view(&entries, "", "sys:NES", false);
        assert!(view.iter().all(|&i| entries[i].system.id == "NES"));
        assert_eq!(view.len(), 2);
    }

    #[test]
    fn filter_by_category_is_case_insensitive() {
        let entries = sample_library();
        // Core's casing for categories is its own; the filter must not
        // depend on matching it exactly.
        let view = build_view(&entries, "", "cat:arcade", false);
        assert_eq!(view.len(), 1);
        assert_eq!(entries[view[0]].system.id, "MAME");
    }

    #[test]
    fn unknown_filter_shows_everything_rather_than_emptying_the_screen() {
        let entries = sample_library();
        assert_eq!(
            build_view(&entries, "", "nonsense", false).len(),
            entries.len()
        );
    }

    #[test]
    fn sort_and_filter_compose() {
        let entries = sample_library();
        let view = build_view(&entries, SORT_NAME, "sys:NES", false);
        let names: Vec<&str> = view.iter().map(|&i| entries[i].name.as_str()).collect();
        assert_eq!(names, vec!["mario", "Zelda"]);
    }

    #[test]
    fn rev_maps_rows_back_and_marks_hidden_entries() {
        let entries = sample_library();
        let view = build_view(&entries, "", "sys:NES", false);
        let rev = build_rev(&view, entries.len());
        assert_eq!(rev.len(), entries.len());
        // Round-trip every visible row.
        for (row, &entry) in view.iter().enumerate() {
            assert_eq!(rev[entry], i32::try_from(row).expect("row fits"));
        }
        // Filtered-out entries report no row.
        assert_eq!(rev[1], -1, "Genesis entry is hidden");
        assert_eq!(rev[3], -1, "MAME entry is hidden");
    }

    #[test]
    fn disambiguation_follows_display_order() {
        // Sibling badges group ADJACENT equal names, so they must be computed
        // over the view. These two variants are far apart in fetch order and
        // adjacent once sorted; computing over `entries` would attach the
        // badges to the wrong rows.
        let variant = |region: &str, file: &str| MediaItem {
            name: "Sonic".to_string(),
            path: format!("/games/Genesis/{file}.md"),
            disambiguating_tags: vec![TagInfo {
                tag: region.to_string(),
                tag_type: "region".to_string(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let entries = vec![
            variant("us", "sonic_u"),
            entry_in("Alex Kidd", "Genesis", "Consoles"),
            variant("eu", "sonic_e"),
        ];
        let view = build_view(&entries, SORT_NAME, "", false);
        let displays = compute_favorites_disambig_displays_view(&entries, &view, false);
        assert_eq!(displays.len(), view.len());
        let sonic_rows: Vec<&String> = view
            .iter()
            .enumerate()
            .filter(|(_, &i)| entries[i].name == "Sonic")
            .map(|(row, _)| &displays[row])
            .collect();
        assert_eq!(sonic_rows.len(), 2);
        assert!(
            sonic_rows.iter().all(|d| !d.is_empty()),
            "adjacent same-named variants must show their distinguishing tags"
        );
    }

    #[test]
    fn system_facet_lists_present_systems_with_counts() {
        let json = build_system_facet_json(&sample_library());
        // Ordered by display name, so Genesis precedes MAME precedes NES.
        assert!(json.starts_with('['), "facet is a JSON array");
        assert!(json.contains(r#""id":"NES""#));
        assert!(json.contains(r#""category":"Arcade""#));
        assert!(json.contains(r#""count":2"#), "NES has two favorites");
        assert!(
            !json.contains(r#""id":"SNES""#),
            "systems with no favorites must not be offered as filters"
        );
    }

    #[test]
    fn system_facet_escapes_quotes_in_names() {
        // System names come from Core, so the facet must survive characters
        // that would otherwise break the JSON QML parses.
        let mut entry = favorite_entry();
        entry.system.name = "He said \"hi\"\\".to_string();
        let json = build_system_facet_json(&[entry]);
        assert!(
            json.contains(r#"He said \"hi\"\\"#),
            "quotes and backslash escaped: {json}"
        );
    }

    #[test]
    fn soft_missed_favorite_cover_uses_system_fallback() {
        let entry = favorite_entry();
        let key = MediaKey::new("SNES", "/games/favorite.rom");
        assert_eq!(
            cover_key_for_with(&entry, Some(&key), false, false, true),
            "systems/SNES"
        );
    }

    #[test]
    fn pending_favorite_cover_uses_loading_icon() {
        let entry = favorite_entry();
        let key = MediaKey::new("SNES", "/games/favorite.rom");
        assert_eq!(
            cover_key_for_with(&entry, Some(&key), false, false, false),
            "icons/Loading"
        );
    }

    #[test]
    fn compute_unresolved_keys_excludes_soft_no_image() {
        let soft_key = MediaKey::new("SNES", "/games/favorite.rom");
        let mut pending_entry = favorite_entry();
        pending_entry.path = "/games/pending.rom".to_string();
        let entries = vec![favorite_entry(), pending_entry];
        let unresolved = compute_unresolved_keys(
            &entries,
            |_| false,
            |k| {
                k.system_id.as_ref() == soft_key.system_id.as_ref()
                    && k.path.as_ref() == soft_key.path.as_ref()
            },
        );
        let expected: HashSet<MediaKey> = [MediaKey::new("SNES", "/games/pending.rom")]
            .into_iter()
            .collect();
        assert_eq!(unresolved, expected);
    }
}
