// Zaparoo Frontend
// Copyright (c) 2026 Wizzo Pty Ltd and the Zaparoo Project contributors.
// SPDX-License-Identifier: LicenseRef-PolyForm-Noncommercial-1.0.0
//
// `Browse.RecentsModel` — flat list of recently-played media, surfaced
// from Core's `media.history` endpoint.
//
// Two paths into the model:
//
//   * `ensure_loaded()` starts the initial page fetch lazily when the
//     Recents screen is requested. Hub boot does not need the paginated
//     history list, so it stays off the startup RPC burst.
//
//   * `fetch_more()` — cursor-driven follow-ups call
//     `Client::media_history` directly, just like games. The model owns
//     the cursor, the in-flight `loading_more` debounce, and the seq
//     ticket that disarms stale callbacks.
//
// History is flat (no folder navigation, no auto-nav) so this model
// stays a fraction of the size of `GamesModel`. Rows are deduplicated
// by exact `mediaPath`; Core returns newest-first history, so the first
// row for a path is the one shown. Card-write isn't wired here yet —
// recents launches by `run`-ing the entry's launcher route.

use crate::media_image_cache::{global_media_image_cache, MediaImageCache, MediaKey};
use crate::media_meta_cache::{global_media_meta_cache, MetaLookup};
use crate::models::nav_timing::NavTiming;
use crate::models::tag_utils::tag_display_value;
use crate::models::{global_handle, global_store};
use cxx_qt::{CxxQtType, Initialize, Threading};
use cxx_qt_lib::{
    QByteArray, QHash, QHashPair_i32_QByteArray, QList, QModelIndex, QString, QVariant,
};
use std::collections::HashSet;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use time::{format_description::well_known::Rfc3339, Duration as TimeDuration, OffsetDateTime};
use tokio::sync::broadcast::error::RecvError;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};
use zaparoo_core::client::{ClientError, ConnectionState};
use zaparoo_core::endpoints::run::RunMutation;
use zaparoo_core::media_types::{
    MediaHistoryEntry, MediaHistoryLatestEntry, MediaHistoryParams, MediaHistoryResult, MediaMeta,
    MediaMetaParams, RunParams, TagInfo,
};

const NAME_ROLE: i32 = 256 + 1;
const PATH_ROLE: i32 = 256 + 2;
const SYSTEM_ID_ROLE: i32 = 256 + 3;
const COVER_KEY_ROLE: i32 = 256 + 4;
const LAUNCHER_ID_ROLE: i32 = 256 + 5;
const FAVORITE_ROLE: i32 = 256 + 6;
const FILE_STEM_ROLE: i32 = 256 + 7;
const HIDDEN_ROLE: i32 = 256 + 8;
// History entries carry no tags; the role exists only so the shared
// grid/list delegates (which require it for media rows) bind cleanly here.
const DISAMBIGUATING_TAGS_ROLE: i32 = 256 + 9;

// Page size for the initial load and every cursor follow-up. Core caps
// `limit` at 100; history rows are tiny (one tile + one caption per row)
// so 25 fills several screens of the recents grid without stressing the
// over-the-wire payload. Bumping this only saves a round trip — it
// doesn't change the UI cap.
const PAGE_SIZE: u32 = 25;
const RESUME_MAX_AGE_DAYS: i64 = 7;
// How many rows ahead/behind the settled cursor to warm when the user
// dwells on a row in list-detail layout. Kept small so the 2-worker byte
// queue stays shallow and the next cover is fetched first within ~250 ms.
const COVER_PREFETCH_CURSOR_NEXT: i32 = 4;
const COVER_PREFETCH_CURSOR_PREV: i32 = 2;
const RESUME_FALLBACK_COVER_KEY: &str = "icons/PlayOutline";

#[allow(
    clippy::struct_excessive_bools,
    reason = "the bools are independent qproperties surfaced to QML; collapsing them \
              into an enum would force the QML side to read a single state property \
              and re-derive each flag locally, which is exactly the work the bridge \
              avoids"
)]
pub struct RecentsModelRust {
    entries: Vec<MediaHistoryEntry>,
    count: i32,
    loading: bool,
    loading_more: bool,
    error_message: QString,
    has_next_page: bool,
    next_cursor: Option<String>,
    resume_available: bool,
    resume_loading: bool,
    resume_name: QString,
    resume_cover_key: QString,
    resume_entry: Option<MediaHistoryEntry>,
    resume_requested: bool,
    resume_seq: Arc<AtomicU64>,
    history_requested: bool,
    history_fetching: bool,
    history_subscription: Option<JoinHandle<()>>,
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
    // `name` role, `name_at()`, and the resume banner show the original
    // filename (sans extension). Bound from QML; flipping re-emits
    // `dataChanged(NAME_ROLE)` and re-syncs the resume banner.
    show_original_filenames: bool,
    current_detail_media_key: Option<MediaKey>,
    current_detail_media_id: Option<i64>,
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

impl Default for RecentsModelRust {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            count: 0,
            loading: false,
            loading_more: false,
            error_message: QString::default(),
            has_next_page: false,
            next_cursor: None,
            resume_available: false,
            resume_loading: false,
            resume_name: QString::default(),
            resume_cover_key: QString::default(),
            resume_entry: None,
            resume_requested: false,
            resume_seq: Arc::new(AtomicU64::new(0)),
            history_requested: false,
            history_fetching: false,
            history_subscription: None,
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
        #[qproperty(bool, resume_available)]
        #[qproperty(bool, resume_loading)]
        #[qproperty(QString, resume_name)]
        #[qproperty(QString, resume_cover_key)]
        #[qproperty(bool, current_detail_loading)]
        #[qproperty(QString, current_detail_tags)]
        #[qproperty(QString, current_detail_image_key)]
        #[qproperty(QString, detail_prefetch_key_next)]
        #[qproperty(QString, detail_prefetch_key_prev)]
        #[qproperty(bool, cover_requests_paused)]
        #[qproperty(bool, show_original_filenames, READ, WRITE = set_show_original_filenames, NOTIFY)]
        type RecentsModel = super::RecentsModelRust;

        #[qinvokable]
        fn ensure_loaded(self: Pin<&mut RecentsModel>);

        #[qinvokable]
        fn fetch_more(self: Pin<&mut RecentsModel>);

        #[qinvokable]
        fn launch_at(self: Pin<&mut RecentsModel>, index: i32);

        #[qinvokable]
        fn launch_resume(self: Pin<&mut RecentsModel>);

        #[qinvokable]
        fn set_show_original_filenames(self: Pin<&mut RecentsModel>, value: bool);

        #[qinvokable]
        fn name_at(self: &RecentsModel, index: i32) -> QString;

        // Always empty — media history carries no disambiguating tags. Present
        // so the shared MediaListScreen active-label tags provider can call it
        // uniformly across models.
        #[qinvokable]
        fn disambiguating_tags_at(self: &RecentsModel, index: i32) -> QString;

        #[qinvokable]
        fn path_at(self: &RecentsModel, index: i32) -> QString;

        #[qinvokable]
        fn system_id_at(self: &RecentsModel, index: i32) -> QString;

        #[qinvokable]
        fn peek_detail_at(self: Pin<&mut RecentsModel>, index: i32);

        #[qinvokable]
        fn load_detail_at(self: Pin<&mut RecentsModel>, index: i32);

        #[qinvokable]
        fn clear_current_detail(self: Pin<&mut RecentsModel>);

        #[qinvokable]
        fn refresh_cover_keys(self: Pin<&mut RecentsModel>, first_row: i32, count: i32);

        #[qinvokable]
        fn clear_pending_cover_requests(self: Pin<&mut RecentsModel>);

        #[qinvokable]
        fn index_for_path(self: &RecentsModel, path: &QString) -> i32;

        #[inherit]
        #[cxx_name = "beginResetModel"]
        fn begin_reset_model(self: Pin<&mut RecentsModel>);

        #[inherit]
        #[cxx_name = "endResetModel"]
        fn end_reset_model(self: Pin<&mut RecentsModel>);

        #[inherit]
        #[cxx_name = "beginInsertRows"]
        fn begin_insert_rows(
            self: Pin<&mut RecentsModel>,
            parent: &QModelIndex,
            first: i32,
            last: i32,
        );

        #[inherit]
        #[cxx_name = "endInsertRows"]
        fn end_insert_rows(self: Pin<&mut RecentsModel>);

        // Qt signal bound as a callable so the cover-cache bridge can
        // invoke it directly from the Qt thread when an async cover
        // fetch completes for a row that is already on screen.
        #[inherit]
        #[cxx_name = "dataChanged"]
        fn data_changed(
            self: Pin<&mut RecentsModel>,
            top_left: &QModelIndex,
            bottom_right: &QModelIndex,
            roles: &QList_i32,
        );

        #[cxx_name = "rowCount"]
        fn row_count(self: &RecentsModel, parent: &QModelIndex) -> i32;
        fn data(self: &RecentsModel, index: &QModelIndex, role: i32) -> QVariant;
        #[cxx_name = "roleNames"]
        fn role_names(self: &RecentsModel) -> QHash_i32_QByteArray;

        // Materialise a `QModelIndex` for `(row, column)` so the cover-
        // cache bridge can target individual rows in `dataChanged`.
        // Forwarded to the QAbstractListModel implementation.
        #[inherit]
        fn index(self: &RecentsModel, row: i32, column: i32, parent: &QModelIndex) -> QModelIndex;
    }

    impl cxx_qt::Threading for RecentsModel {}
    impl cxx_qt::Initialize for RecentsModel {}
}

impl Initialize for ffi::RecentsModel {
    fn initialize(mut self: Pin<&mut Self>) {
        self.as_mut().bind_resume_to_connection();
    }
}

/// Snapshot of a single page that `apply_state` can write onto the
/// model. Carried by value so the closure is `Send + 'static` for the
/// `qt_thread` queue.
type PageSnapshot = (Vec<MediaHistoryEntry>, bool, Option<String>);

fn page_snapshot(result: &MediaHistoryResult) -> PageSnapshot {
    (
        result.entries.clone(),
        result.has_next_page(),
        result.next_cursor(),
    )
}

/// Post-Ready pagination: warm page 2 over the RPC. Reached only when
/// the direct read could not answer (or the layer is disabled), so the
/// paged chain must continue exactly as it always did. `fetch_more` is
/// itself guarded by `has_next_page` and `loading_more`.
fn finish_ready_pagination(mut model: Pin<&mut ffi::RecentsModel>, has_next_page: bool) {
    if has_next_page && !model.cover_requests_paused {
        model.as_mut().fetch_more();
    }
}

fn apply_state(model: Pin<&mut ffi::RecentsModel>, (data, err): (Option<PageSnapshot>, String)) {
    if let Some(snapshot) = data {
        // Direct-first: the store's Ready is the trigger and the local
        // read is the content; the RPC page it delivered is only
        // applied when the local read cannot answer.
        if crate::media_history_db::enabled() {
            spawn_direct_ready(model, snapshot);
        } else {
            apply_ready_page(model, snapshot);
        }
    } else {
        apply_pending_or_error(model, &err);
    }
}

fn apply_ready_page(mut model: Pin<&mut ffi::RecentsModel>, snapshot: PageSnapshot) {
    let apply_started = Instant::now();
    {
        let (entries, has_next_page, next_cursor) = snapshot;
        if let Some(timing) = model.as_mut().rust_mut().nav_timing.as_mut() {
            timing.mark_request_done();
        }
        model.as_mut().rust_mut().history_requested = true;
        model.as_mut().rust_mut().history_fetching = false;
        model.as_mut().rust_mut().history_subscription = None;
        // A fresh initial page resets the cursor chain — bump `seq` so
        // any in-flight `fetch_more` sees a stale ticket and bails.
        model.as_mut().rust_mut().seq.fetch_add(1, Ordering::SeqCst);
        model.as_mut().ensure_cover_subscription();
        let raw_len = entries.len();
        let entries = dedupe_latest_by_path(entries);
        info!(
            raw_len,
            deduped_len = entries.len(),
            has_next_page,
            "recents-diag: apply_state Ready branch"
        );
        if !model.cover_requests_paused {
            enqueue_recents_covers(&entries);
        }
        let count = i32::try_from(entries.len()).unwrap_or(i32::MAX);
        clear_current_detail_state(model.as_mut());
        model.as_mut().begin_reset_model();
        model.as_mut().rust_mut().entries = entries;
        model.as_mut().rust_mut().count = count;
        model.as_mut().rust_mut().next_cursor = next_cursor;
        model.as_mut().end_reset_model();
        model.as_mut().count_changed();
        sync_resume_state(model.as_mut());
        if let Some(timing) = model.as_mut().rust_mut().nav_timing.as_mut() {
            timing.mark_apply_done();
        }
        debug!(
            apply_ms = apply_started.elapsed().as_millis(),
            "recents: apply_state timing",
        );
        if model.has_next_page != has_next_page {
            model.as_mut().set_has_next_page(has_next_page);
        }
        // Hidden startup binding can pause cover requests so Hub paints without
        // Recents' off-screen cover gate. Screen entry resumes requests and
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
        finish_ready_pagination(model.as_mut(), has_next_page);
    }
}

/// Direct-first Ready handling: bump the chain, read the complete
/// deduplicated local history, and paint it; the RPC page in
/// `snapshot` applies only when the local read cannot answer.
fn spawn_direct_ready(mut model: Pin<&mut ffi::RecentsModel>, snapshot: PageSnapshot) {
    model.as_mut().rust_mut().seq.fetch_add(1, Ordering::SeqCst);
    let seq = model.rust().seq.clone();
    let ticket = seq.load(Ordering::SeqCst);
    let qt_thread = model.qt_thread();
    global_handle().spawn(async move {
        let cancel_seq = seq.clone();
        let direct = tokio::task::spawn_blocking(move || {
            crate::media_history_db::history(&|| cancel_seq.load(Ordering::SeqCst) != ticket)
        })
        .await
        .ok()
        .flatten();
        let _ = qt_thread.queue(move |model| {
            if seq.load(Ordering::SeqCst) != ticket {
                return;
            }
            match direct {
                Some(entries) => apply_direct_full_paint(model, entries),
                None => apply_ready_page(model, snapshot),
            }
        });
    });
}

/// Early paint: fired from the Pending branch so a cold entry shows the
/// complete local history in tens of milliseconds instead of waiting
/// for the store's first network Ready, which then re-confirms it.
fn spawn_direct_early_paint(model: &Pin<&mut ffi::RecentsModel>) {
    if !crate::media_history_db::enabled() || model.count > 0 {
        return;
    }
    let seq = model.rust().seq.clone();
    let ticket = seq.load(Ordering::SeqCst);
    let qt_thread = model.qt_thread();
    global_handle().spawn(async move {
        let cancel_seq = seq.clone();
        let direct = tokio::task::spawn_blocking(move || {
            crate::media_history_db::history(&|| cancel_seq.load(Ordering::SeqCst) != ticket)
        })
        .await
        .ok()
        .flatten();
        let Some(entries) = direct else {
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

/// Land the complete local history as a first-class paint: the direct
/// counterpart of `apply_ready_page` with terminal pagination, serving
/// both the cold entry and every store-triggered refresh.
fn apply_direct_full_paint(
    mut model: Pin<&mut ffi::RecentsModel>,
    entries: Vec<MediaHistoryEntry>,
) {
    let apply_started = Instant::now();
    if model.nav_timing.is_none() {
        model.as_mut().rust_mut().nav_timing = Some(NavTiming::new("direct"));
    }
    if let Some(timing) = model.as_mut().rust_mut().nav_timing.as_mut() {
        timing.set_source("direct");
        timing.mark_request_done();
    }
    model.as_mut().rust_mut().history_requested = true;
    model.as_mut().rust_mut().history_fetching = false;
    model.as_mut().rust_mut().history_subscription = None;
    model.as_mut().ensure_cover_subscription();
    if !model.cover_requests_paused {
        enqueue_recents_covers(&entries);
    }
    let count = i32::try_from(entries.len()).unwrap_or(i32::MAX);
    clear_current_detail_state(model.as_mut());
    model.as_mut().begin_reset_model();
    {
        let mut rust = model.as_mut().rust_mut();
        rust.entries = entries;
        rust.count = count;
        rust.next_cursor = None;
    }
    model.as_mut().end_reset_model();
    model.as_mut().count_changed();
    sync_resume_state(model.as_mut());
    if let Some(timing) = model.as_mut().rust_mut().nav_timing.as_mut() {
        timing.mark_apply_done();
    }
    debug!(
        apply_ms = apply_started.elapsed().as_millis(),
        "recents: apply_direct_full_paint timing",
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
}

fn apply_pending_or_error(mut model: Pin<&mut ffi::RecentsModel>, err: &str) {
    if err.is_empty() {
        info!(
            count = model.count,
            "recents-diag: apply_state Pending branch (loading, entries untouched)"
        );
        // Pending (Idle/Loading): show the spinner; don't touch entries.
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
        sync_resume_state(model.as_mut());
        if let Some(timing) = model.as_mut().rust_mut().nav_timing.as_mut() {
            timing.set_source("network");
        }
        if !model.loading {
            model.as_mut().set_loading(true);
        }
        if model.has_next_page {
            model.as_mut().set_has_next_page(false);
        }
        spawn_direct_early_paint(&model);
    } else {
        info!(
            error = err,
            count = model.count,
            "recents-diag: apply_state Errored branch (history_requested reset, entries untouched)"
        );
        model.as_mut().rust_mut().history_requested = false;
        model.as_mut().rust_mut().history_fetching = false;
        model.as_mut().rust_mut().history_subscription = None;
        // Same disarm as the Pending branch — an Errored transition
        // doesn't reset entries, so a callback queued during the prior
        // Ready could otherwise append rows that don't belong to the
        // current chain.
        disarm_cover_gate(model.as_mut());
        clear_current_detail_state(model.as_mut());
        model.as_mut().rust_mut().seq.fetch_add(1, Ordering::SeqCst);
        model.as_mut().rust_mut().next_cursor = None;
        sync_resume_state(model.as_mut());
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

impl ffi::RecentsModel {
    fn row_count(&self, parent: &QModelIndex) -> i32 {
        if parent.is_valid() {
            0
        } else {
            self.count
        }
    }

    fn data(&self, index: &QModelIndex, role: i32) -> QVariant {
        if !index.is_valid() || index.row() < 0 || index.row() >= self.count {
            return QVariant::default();
        }
        let entry = &self.entries[index.row() as usize];
        match role {
            NAME_ROLE => QVariant::from(&QString::from(
                display_name(
                    &entry.media_name,
                    &entry.media_path,
                    self.show_original_filenames,
                )
                .as_str(),
            )),
            PATH_ROLE => QVariant::from(&QString::from(entry.media_path.as_str())),
            SYSTEM_ID_ROLE => QVariant::from(&QString::from(entry.system_id.as_str())),
            COVER_KEY_ROLE => QVariant::from(&QString::from(
                cover_key_for(entry, !self.cover_requests_paused).as_str(),
            )),
            LAUNCHER_ID_ROLE => QVariant::from(&QString::from(entry.launcher_id.as_str())),
            FAVORITE_ROLE => QVariant::from(&0_i32),
            FILE_STEM_ROLE => QVariant::from(&QString::from(file_stem_or_name(
                &entry.media_path,
                &entry.media_name,
            ))),
            HIDDEN_ROLE => QVariant::from(&false),
            DISAMBIGUATING_TAGS_ROLE => QVariant::from(&QString::default()),
            _ => QVariant::default(),
        }
    }

    fn role_names(&self) -> QHash<QHashPair_i32_QByteArray> {
        let mut h = QHash::<QHashPair_i32_QByteArray>::default();
        h.insert(NAME_ROLE, QByteArray::from("name"));
        h.insert(PATH_ROLE, QByteArray::from("path"));
        h.insert(SYSTEM_ID_ROLE, QByteArray::from("systemId"));
        h.insert(COVER_KEY_ROLE, QByteArray::from("coverKey"));
        h.insert(LAUNCHER_ID_ROLE, QByteArray::from("launcherId"));
        h.insert(FAVORITE_ROLE, QByteArray::from("favorite"));
        h.insert(FILE_STEM_ROLE, QByteArray::from("fileStem"));
        h.insert(HIDDEN_ROLE, QByteArray::from("hidden"));
        h.insert(
            DISAMBIGUATING_TAGS_ROLE,
            QByteArray::from("disambiguatingTags"),
        );
        h
    }

    fn bind_resume_to_connection(mut self: Pin<&mut Self>) {
        self.as_mut().ensure_resume_fetch();
        let mut rx = global_store().client().connection.subscribe();
        let qt_thread = self.qt_thread();
        global_handle().spawn(async move {
            while rx.changed().await.is_ok() {
                let _ = qt_thread.queue(|model| {
                    model.ensure_resume_fetch();
                });
            }
        });
    }

    fn ensure_loaded(mut self: Pin<&mut Self>) {
        info!(
            history_requested = self.history_requested,
            history_fetching = self.history_fetching,
            count = self.count,
            loading = self.loading,
            "recents-diag: ensure_loaded called"
        );
        if self.history_requested {
            info!("recents-diag: ensure_loaded short-circuit (already-loaded), NO refetch");
            NavTiming::new("cache").log_release("recents", "already-loaded", 0);
            return;
        }
        if self.history_fetching {
            info!("recents-diag: ensure_loaded short-circuit (already-fetching)");
            return;
        }
        self.as_mut().rust_mut().history_fetching = true;
        if !self.loading {
            self.as_mut().set_loading(true);
        }
        let store = global_store();
        if matches!(
            *store.client().connection.borrow(),
            ConnectionState::Connected
        ) {
            self.as_mut().start_initial_history_fetch();
            return;
        }
        let mut rx = store.client().connection.subscribe();
        let qt_thread = self.qt_thread();
        let handle = global_handle().spawn(async move {
            loop {
                if rx.changed().await.is_err() {
                    let _ = qt_thread.queue(|mut model| {
                        model.as_mut().rust_mut().history_fetching = false;
                        model.as_mut().rust_mut().history_subscription = None;
                        if model.loading {
                            model.as_mut().set_loading(false);
                        }
                    });
                    break;
                }
                let state = rx.borrow_and_update().clone();
                match state {
                    ConnectionState::Connected => {
                        let _ = qt_thread.queue(|model| {
                            model.start_initial_history_fetch();
                        });
                        break;
                    }
                    ConnectionState::Unreachable(message) => {
                        let _ = qt_thread.queue(move |mut model| {
                            let qerr = QString::from(message.as_str());
                            if model.error_message != qerr {
                                model.as_mut().set_error_message(qerr);
                            }
                            if model.loading {
                                model.as_mut().set_loading(false);
                            }
                            if model.has_next_page {
                                model.as_mut().set_has_next_page(false);
                            }
                        });
                    }
                    ConnectionState::Disconnected
                    | ConnectionState::Connecting
                    | ConnectionState::Reconnecting => {}
                }
            }
        });
        self.as_mut().rust_mut().history_subscription = Some(handle);
    }

    fn start_initial_history_fetch(mut self: Pin<&mut Self>) {
        self.as_mut().rust_mut().nav_timing = Some(NavTiming::new("network"));
        self.as_mut().rust_mut().history_subscription = None;
        if !self.loading {
            self.as_mut().set_loading(true);
        }
        if !self.error_message.is_empty() {
            self.as_mut().set_error_message(QString::default());
        }
        self.as_mut().ensure_cover_subscription();
        self.as_mut().rust_mut().seq.fetch_add(1, Ordering::SeqCst);
        clear_current_detail_state(self.as_mut());
        let seq = self.rust().seq.clone();
        let ticket = seq.load(Ordering::SeqCst);
        let qt_thread = self.qt_thread();
        let store = global_store();
        global_handle().spawn(async move {
            let result = store
                .client()
                .media_history(MediaHistoryParams {
                    limit: Some(PAGE_SIZE),
                    cursor: None,
                    systems: Vec::new(),
                })
                .await;
            match &result {
                Ok(r) => info!(
                    entries_len = r.entries.len(),
                    has_next_page = r.has_next_page(),
                    next_cursor_set = r.next_cursor().is_some(),
                    "recents-diag: initial media.history returned"
                ),
                Err(e) => info!(
                    error = e.message.as_str(),
                    "recents-diag: initial media.history failed"
                ),
            }
            let projected = match result {
                Ok(result) => (Some(page_snapshot(&result)), String::new()),
                Err(e) => (None, e.message),
            };
            let _ = qt_thread.queue(move |model| {
                if seq.load(Ordering::SeqCst) != ticket {
                    return;
                }
                apply_state(model, projected);
            });
        });
    }

    fn ensure_resume_fetch(mut self: Pin<&mut Self>) {
        if self.resume_requested {
            return;
        }
        let store = global_store();
        if !matches!(
            *store.client().connection.borrow(),
            ConnectionState::Connected
        ) {
            if !self.resume_loading {
                self.as_mut().set_resume_loading(true);
            }
            sync_resume_state(self);
            return;
        }
        self.as_mut().rust_mut().resume_requested = true;
        self.as_mut()
            .rust()
            .resume_seq
            .fetch_add(1, Ordering::SeqCst);
        self.as_mut().set_resume_loading(true);
        sync_resume_state(self.as_mut());
        let seq = self.rust().resume_seq.clone();
        let ticket = seq.load(Ordering::SeqCst);
        let qt_thread = self.qt_thread();
        global_handle().spawn(async move {
            let result = store.client().media_history_latest().await;
            let _ = qt_thread.queue(move |model| {
                if seq.load(Ordering::SeqCst) != ticket {
                    return;
                }
                apply_resume_latest_result(model, result);
            });
        });
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
                .media_history(MediaHistoryParams {
                    limit: Some(PAGE_SIZE),
                    cursor,
                    systems: Vec::new(),
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

    fn refresh_cover_keys(mut self: Pin<&mut Self>, first_row: i32, count: i32) {
        emit_cover_key_range(self.as_mut(), first_row, count);
    }

    fn clear_pending_cover_requests(self: Pin<&mut Self>) {
        global_media_image_cache().clear_pending_requests();
    }

    fn launch_at(self: Pin<&mut Self>, index: i32) {
        if index < 0 || index >= self.count {
            return;
        }
        launch_entry(&self.entries[index as usize]);
    }

    fn launch_resume(self: Pin<&mut Self>) {
        let Some(entry) = self.resume_entry.as_ref() else {
            return;
        };
        launch_entry(entry);
    }

    fn name_at(&self, index: i32) -> QString {
        if index < 0 || index >= self.count {
            return QString::default();
        }
        {
            let entry = &self.entries[index as usize];
            QString::from(
                display_name(
                    &entry.media_name,
                    &entry.media_path,
                    self.show_original_filenames,
                )
                .as_str(),
            )
        }
    }

    #[allow(
        clippy::unused_self,
        reason = "matches the shared model invokable signature; history has no tags"
    )]
    fn disambiguating_tags_at(&self, _index: i32) -> QString {
        QString::default()
    }

    fn set_show_original_filenames(mut self: Pin<&mut Self>, value: bool) {
        if self.show_original_filenames == value {
            return;
        }
        self.as_mut().rust_mut().show_original_filenames = value;
        self.as_mut().show_original_filenames_changed();
        let last_row = self.count - 1;
        if last_row >= 0 {
            let mut roles = QList::<i32>::default();
            roles.append(NAME_ROLE);
            let parent = QModelIndex::default();
            let top_left = self.as_mut().index(0, 0, &parent);
            let bottom_right = self.as_mut().index(last_row, 0, &parent);
            self.as_mut().data_changed(&top_left, &bottom_right, &roles);
        }
        // The resume banner is a computed property, not a row role — re-derive
        // it so "Continue playing" reflects the new naming immediately.
        sync_resume_state(self.as_mut());
    }

    fn path_at(&self, index: i32) -> QString {
        if index < 0 || index >= self.count {
            return QString::default();
        }
        QString::from(self.entries[index as usize].media_path.as_str())
    }

    fn system_id_at(&self, index: i32) -> QString {
        if index < 0 || index >= self.count {
            return QString::default();
        }
        QString::from(self.entries[index as usize].system_id.as_str())
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
        prefetch_around_cursor(&self.entries, self.count, index, self.cover_requests_paused);
        let entry = &self.entries[index as usize];
        let system = entry.system_id.clone();
        let path = entry.media_path.clone();
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

        enqueue_meta_prefetch(&self.entries, self.count, index);
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
        prefetch_around_cursor(&self.entries, self.count, index, self.cover_requests_paused);
        let entry = &self.entries[index as usize];
        let system = entry.system_id.clone();
        let path = entry.media_path.clone();
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
                        warn!("recent detail fetch failed for {path}: {}", e.message);
                    }
                }
                model.as_mut().set_current_detail_loading(false);
            });
        });
    }

    fn clear_current_detail(self: Pin<&mut Self>) {
        clear_current_detail_state(self);
    }

    fn index_for_path(&self, path: &QString) -> i32 {
        position_of_path(&self.entries, &path.to_string())
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

/// Resolve the cover URL key for a recents row. Mirrors `GamesModel`'s
/// path: when the in-memory cache has bytes for `(systemId, mediaPath)`
/// we hand back the `media-image/<encoded>` key the
/// `QQuickImageProvider` resolves to RAM bytes; otherwise we enqueue a
/// fetch (carrying the optional `mediaId` hint) and fall back to the
/// system logo as a nicer placeholder than the generic file glyph.
fn cover_key_for(entry: &MediaHistoryEntry, requests_enabled: bool) -> String {
    if entry.system_id.is_empty() {
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

fn resume_cover_key_for(_entry: &MediaHistoryEntry, _requests_enabled: bool) -> String {
    RESUME_FALLBACK_COVER_KEY.to_string()
}

fn apply_resume_latest_result(
    mut model: Pin<&mut ffi::RecentsModel>,
    result: Result<zaparoo_core::media_types::MediaHistoryLatestResult, ClientError>,
) {
    let latest = match result {
        Ok(result) => result.entry,
        Err(e) => {
            debug!("media.history.latest failed: {}", e.message);
            model.as_mut().rust_mut().resume_requested = false;
            if model.resume_loading {
                model.as_mut().set_resume_loading(false);
            }
            sync_resume_state(model);
            return;
        }
    };
    model.as_mut().rust_mut().resume_entry =
        latest.map(history_entry_from_latest).filter(|entry| {
            !launch_text_for(entry).is_empty()
                && resume_entry_is_fresh(entry, OffsetDateTime::now_utc())
        });
    if model.resume_loading {
        model.as_mut().set_resume_loading(false);
    }
    sync_resume_state(model);
}

fn history_entry_from_latest(entry: MediaHistoryLatestEntry) -> MediaHistoryEntry {
    MediaHistoryEntry {
        system_id: entry.system_id,
        system_name: entry.system_name,
        media_name: entry.media_name,
        media_path: entry.media_path,
        launcher_id: entry.launcher_id,
        started_at: entry.started_at,
        ..MediaHistoryEntry::default()
    }
}

fn sync_resume_state(mut model: Pin<&mut ffi::RecentsModel>) {
    let (available, name, cover_key) = match model.resume_entry.as_ref() {
        Some(entry) => (
            true,
            QString::from(
                display_name(
                    &entry.media_name,
                    &entry.media_path,
                    model.show_original_filenames,
                )
                .as_str(),
            ),
            QString::from(resume_cover_key_for(entry, !model.cover_requests_paused).as_str()),
        ),
        None => (
            false,
            QString::default(),
            QString::from(RESUME_FALLBACK_COVER_KEY),
        ),
    };
    if model.resume_available != available {
        model.as_mut().set_resume_available(available);
    }
    if model.resume_name != name {
        model.as_mut().set_resume_name(name);
    }
    if model.resume_cover_key != cover_key {
        model.as_mut().set_resume_cover_key(cover_key);
    }
}

fn emit_cover_key_range(mut model: Pin<&mut ffi::RecentsModel>, first_row: i32, count: i32) {
    if model.count <= 0 || count <= 0 || first_row >= model.count {
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

/// Build the canonical `(systemId, mediaPath)` identifier for a history
/// row. Returns `None` for rows without enough info to key on.
fn media_key_for(entry: &MediaHistoryEntry) -> Option<MediaKey> {
    if entry.system_id.is_empty() || entry.media_path.is_empty() {
        return None;
    }
    match entry.media_id {
        Some(media_id) => Some(MediaKey::with_media_id(
            entry.system_id.clone(),
            entry.media_path.clone(),
            media_id,
        )),
        None => Some(MediaKey::new(
            entry.system_id.clone(),
            entry.media_path.clone(),
        )),
    }
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
    entry: &MediaHistoryEntry,
    key: Option<&MediaKey>,
    cached: bool,
    negative: bool,
    soft_no_image: bool,
) -> String {
    if entry.system_id.is_empty() {
        return "icons/File".to_string();
    }
    match key {
        Some(k) if cached => MediaImageCache::image_key_for(k),
        Some(_) if !negative && !soft_no_image => "icons/Loading".to_string(),
        _ => format!("systems/{}", entry.system_id),
    }
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
fn enqueue_meta_prefetch(entries: &[MediaHistoryEntry], count: i32, row: i32) {
    let mut requests = Vec::new();
    for delta in [-2_i32, -1, 1, 2] {
        let i = row + delta;
        if i < 0 || i >= count {
            continue;
        }
        let entry = &entries[i as usize];
        let system = entry.system_id.clone();
        let path = entry.media_path.clone();
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

fn clear_current_detail_state(mut model: Pin<&mut ffi::RecentsModel>) {
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

fn clear_adjacent_cover_prefetch(mut model: Pin<&mut ffi::RecentsModel>) {
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

fn refresh_adjacent_cover_prefetch(mut model: Pin<&mut ffi::RecentsModel>) {
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

fn sync_current_detail_image_key(mut model: Pin<&mut ffi::RecentsModel>) {
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

/// Schedule a cover fetch for every history row with a non-empty
/// `(systemId, mediaPath)`. The cache enqueue is idempotent —
/// already-cached, already-pending, or negatively-memoised keys are
/// dropped — so spamming this from `apply_state` / `apply_append_page`
/// is cheap.
///
/// Iterates `entries` in reverse so the LIFO fetch queue drains in
/// visual order: the last entry pushed is `entries[0]`, which the
/// driver pops first. Forward iteration starves the top of the page.
fn enqueue_recents_covers(entries: &[MediaHistoryEntry]) {
    let cache = global_media_image_cache();
    for entry in entries.iter().rev() {
        if let Some(key) = media_key_for(entry).map(MediaKey::with_current_cover_preference) {
            cache.enqueue_search_cover_with_media_id(key, entry.media_id, PAGE_SIZE);
        }
    }
}

/// Re-center the byte-fetch queue on the settled cursor row so covers
/// for the current position and its immediate neighbors are fetched
/// first, ahead of the stale top-of-list backlog.
///
/// Without this, the queue fills in row order at list load and drains
/// monotonically regardless of where the user scrolls; every Down press
/// finds the next cover still deep in the queue. Called from
/// `load_detail_at` after the debounce fires.
fn prefetch_around_cursor(
    entries: &[MediaHistoryEntry],
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
    for i in row..fwd_end {
        let e = &entries[i as usize];
        if let Some(key) = media_key_for(e).map(MediaKey::with_current_cover_preference) {
            plan.push((key, e.media_id));
        }
    }
    for i in (back_start..row).rev() {
        let e = &entries[i as usize];
        if let Some(key) = media_key_for(e).map(MediaKey::with_current_cover_preference) {
            plan.push((key, e.media_id));
        }
    }
    cache.replace_pending_requests_ordered(plan, PAGE_SIZE);
}

fn finish_nav_timing(
    mut model: Pin<&mut ffi::RecentsModel>,
    reason: &'static str,
    pending_remaining: usize,
) {
    if let Some(timing) = model.as_mut().rust_mut().nav_timing.take() {
        timing.log_release("recents", reason, pending_remaining);
    }
}

/// Emit `dataChanged(coverKey)` for every row whose entry's
/// `(systemId, mediaPath)` matches `key`. Cheap walk of the current
/// `entries` vec — recents pages top out at a few hundred rows after
/// look-ahead, and the bridge runs only when the cover-cache fetch
/// driver delivers a result.
///
/// Also drains `pending_first_paint_keys`: each cover landing during
/// the gate's hold ticks the set down, and emptying the set releases
/// the gate so the screen-flip overlay clears.
fn notify_cover_update(mut model: Pin<&mut ffi::RecentsModel>, key: &MediaKey) {
    let rows: Vec<i32> = model
        .entries
        .iter()
        .enumerate()
        .filter(|(_, e)| {
            key.is_cover_key()
                && match (key.media_id, e.media_id) {
                    (Some(a), Some(b)) => a == b,
                    _ => e.media_path == *key.path && e.system_id == *key.system_id,
                }
        })
        .filter_map(|(i, _)| i32::try_from(i).ok())
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
    if resume_entry(&model.entries)
        .and_then(media_key_for)
        .map(MediaKey::with_current_cover_preference)
        .as_ref()
        .is_some_and(|current| current == key)
    {
        sync_resume_state(model.as_mut());
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
        // `RecentsScreen.qml` dispatches all N requests at once and the
        // provider's 4-worker pool decodes them in ~75–150 ms; without
        // this settle window the gate flips `loading=false` before the
        // last few decodes complete and the grid materialises with
        // those tiles still showing the procedural fallback. Mirrors
        // the same hand-off in `games.rs::notify_cover_update`. Same
        // seq-ticket guard as the safety timer so a model reset
        // cancels the pending release.
        info!("recents: cover gate bytes settled — entering decode-settle window");
        let seq = model.rust().cover_gate_seq.clone();
        let ticket = seq.fetch_add(1, Ordering::SeqCst) + 1;
        let qt_thread = model.qt_thread();
        let handle = global_handle().spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            let _ = qt_thread.queue(move |mut model: Pin<&mut ffi::RecentsModel>| {
                if seq.load(Ordering::SeqCst) != ticket {
                    return;
                }
                model.as_mut().rust_mut().cover_gate_timer = None;
                if model.loading {
                    info!("recents: cover gate released after decode-settle window");
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
    entries: &[MediaHistoryEntry],
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
/// - If every history row's cover is already cached or negatively-
///   memoised, set loading=false right now — the screen-flip overlay
///   clears.
/// - Otherwise, store the unresolved set on the model, arm a 3 s
///   safety timer, and leave loading=true. `notify_cover_update` will
///   drain the set as covers land; whichever happens first (set
///   empties or timer fires) releases the gate.
fn arm_cover_gate(mut model: Pin<&mut ffi::RecentsModel>) {
    if let Some(handle) = model.as_mut().rust_mut().cover_gate_timer.take() {
        handle.abort();
    }
    let cache = global_media_image_cache();
    let cover_keys = model
        .entries
        .iter()
        .filter_map(|entry| media_key_for(entry).map(MediaKey::with_current_cover_preference))
        .collect::<Vec<_>>();
    let cover_total = cover_keys.len();
    let cover_cache_hits = cover_keys.iter().filter(|k| cache.is_cached(k)).count();
    let unresolved = compute_unresolved_keys(
        &model.entries,
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
        "recents: arm cover gate (holding loading until covers cached)"
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
fn disarm_cover_gate(mut model: Pin<&mut ffi::RecentsModel>) {
    if let Some(handle) = model.as_mut().rust_mut().cover_gate_timer.take() {
        handle.abort();
    }
    model.as_mut().rust_mut().pending_first_paint_keys.clear();
    model.rust().cover_gate_seq.fetch_add(1, Ordering::SeqCst);
}

/// Force-release the cover gate from the safety timer. Called only via
/// the timer's queued callback after a seq-match check; the
/// notify-driven release path lives inline in `notify_cover_update`.
fn release_cover_gate_after_timeout(mut model: Pin<&mut ffi::RecentsModel>) {
    let pending = model.pending_first_paint_keys.len();
    info!(pending, "recents: cover gate timed out, releasing");
    model.as_mut().rust_mut().pending_first_paint_keys.clear();
    model.as_mut().rust_mut().cover_gate_timer = None;
    if model.loading {
        model.as_mut().set_loading(false);
    }
    finish_nav_timing(model.as_mut(), "timeout", pending);
}

fn launch_entry(entry: &MediaHistoryEntry) {
    let text = launch_text_for(entry);
    if text.is_empty() {
        return;
    }
    let store = global_store();
    global_handle().spawn(async move {
        if let Err(e) = store.run_mutation::<RunMutation>(RunParams { text }).await {
            warn!("run failed: {}", e.message);
        }
    });
}

/// Build the `text` payload sent to Core's `run` for a history entry.
/// Runtime relaunches prefer the exact path Core recorded; portable
/// `ZapScript` is not available on history rows and launcher affinity is
/// less important than avoiding title-resolution ambiguity.
fn launch_text_for(entry: &MediaHistoryEntry) -> String {
    entry.media_path.clone()
}

fn resume_entry(entries: &[MediaHistoryEntry]) -> Option<&MediaHistoryEntry> {
    let now = OffsetDateTime::now_utc();
    entries
        .iter()
        .find(|entry| !launch_text_for(entry).is_empty() && resume_entry_is_fresh(entry, now))
}

fn resume_entry_is_fresh(entry: &MediaHistoryEntry, now: OffsetDateTime) -> bool {
    let timestamp = entry
        .ended_at
        .as_deref()
        .unwrap_or(entry.started_at.as_str());
    if timestamp.trim().is_empty() {
        return false;
    }
    let Ok(played_at) = OffsetDateTime::parse(timestamp, &Rfc3339) else {
        return false;
    };
    if played_at > now {
        return false;
    }
    now - played_at <= TimeDuration::days(RESUME_MAX_AGE_DAYS)
}

fn position_of_path(entries: &[MediaHistoryEntry], needle: &str) -> i32 {
    if needle.is_empty() {
        return -1;
    }
    entries
        .iter()
        .position(|e| e.media_path == needle)
        .map_or(-1, |i| i as i32)
}

/// Keep the newest history row for each exact, non-empty path. Core
/// returns history newest-first, so preserving the first occurrence
/// implements latest-wins without parsing timestamps. Empty paths are
/// malformed/unlaunchable and stay as-is instead of all collapsing into
/// one bucket.
fn dedupe_latest_by_path(entries: Vec<MediaHistoryEntry>) -> Vec<MediaHistoryEntry> {
    filter_entries_by_path(std::iter::empty::<&MediaHistoryEntry>(), entries)
}

fn filter_entries_by_path<'a, I>(
    existing_entries: I,
    incoming_entries: Vec<MediaHistoryEntry>,
) -> Vec<MediaHistoryEntry>
where
    I: IntoIterator<Item = &'a MediaHistoryEntry>,
{
    let mut seen = existing_entries
        .into_iter()
        .filter_map(|entry| {
            if entry.media_path.is_empty() {
                None
            } else {
                Some(entry.media_path.clone())
            }
        })
        .collect::<HashSet<_>>();
    incoming_entries
        .into_iter()
        .filter(|entry| entry.media_path.is_empty() || seen.insert(entry.media_path.clone()))
        .collect()
}

fn apply_append_page(
    mut model: Pin<&mut ffi::RecentsModel>,
    result: Result<MediaHistoryResult, ClientError>,
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
            let entries = filter_entries_by_path(model.entries.iter(), result.entries);
            let new_count = i32::try_from(entries.len()).unwrap_or(i32::MAX - model.count);
            if !model.cover_requests_paused {
                enqueue_recents_covers(&entries);
            }
            if new_count > 0 {
                let first = model.count;
                let last = first.saturating_add(new_count).saturating_sub(1);
                let parent = QModelIndex::default();
                model.as_mut().begin_insert_rows(&parent, first, last);
                model.as_mut().rust_mut().entries.extend(entries);
                model.as_mut().rust_mut().count = first.saturating_add(new_count);
                model.as_mut().end_insert_rows();
                model.as_mut().count_changed();
                sync_resume_state(model.as_mut());
            }
            model.as_mut().rust_mut().next_cursor = next_cursor;
            model.as_mut().set_has_next_page(has_next_page);
            model.as_mut().set_loading_more(false);
        }
        Err(e) => {
            warn!("media.history follow-up page failed: {}", e.message);
            model
                .as_mut()
                .set_error_message(QString::from(e.message.as_str()));
            model.as_mut().set_loading_more(false);
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::expect_used,
        clippy::unwrap_used,
        clippy::panic,
        reason = "tests should fail-fast on unexpected errors"
    )]

    use super::{
        compute_unresolved_keys, cover_key_for_with, dedupe_latest_by_path, filter_entries_by_path,
        launch_text_for, media_key_for, page_snapshot, position_of_path, resume_cover_key_for,
        resume_entry, resume_entry_is_fresh, RESUME_FALLBACK_COVER_KEY,
    };
    use crate::media_image_cache::{MediaImageCache, MediaKey};
    use std::collections::HashSet;
    use time::{
        format_description::well_known::Rfc3339, macros::datetime, Duration as TimeDuration,
        OffsetDateTime,
    };
    use zaparoo_core::media_types::{MediaHistoryEntry, MediaHistoryResult, Pagination};

    fn entry(name: &str, path: &str, system_id: &str, launcher_id: &str) -> MediaHistoryEntry {
        MediaHistoryEntry {
            media_name: name.into(),
            media_path: path.into(),
            system_id: system_id.into(),
            launcher_id: launcher_id.into(),
            ..MediaHistoryEntry::default()
        }
    }

    #[test]
    fn resume_entry_requires_launchable_entry() {
        assert!(resume_entry(&[]).is_none());
        let e = entry("ghost", "", "NES", "NES");
        assert!(resume_entry(&[e]).is_none());
    }

    #[test]
    fn resume_entry_skips_stale_or_malformed_entries() {
        let now = OffsetDateTime::now_utc();
        let mut stale = entry("old", "/p/old", "NES", "NES");
        stale.started_at = (now - TimeDuration::days(30)).format(&Rfc3339).unwrap();
        let mut malformed = entry("bad", "/p/bad", "NES", "NES");
        malformed.started_at = "not-a-date".into();
        let mut recent = entry("smb", "/p/smb", "NES", "NES");
        recent.started_at = (now - TimeDuration::days(1)).format(&Rfc3339).unwrap();
        assert_eq!(
            resume_entry(&[stale, malformed, recent]).map(|entry| entry.media_name.as_str()),
            Some("smb")
        );
    }

    #[test]
    fn resume_entry_freshness_accepts_recent_started_at() {
        let mut e = entry("smb", "/p/smb", "NES", "NES");
        e.started_at = "2026-06-01T00:00:00Z".into();
        assert!(resume_entry_is_fresh(&e, datetime!(2026-06-03 00:00 UTC)));
    }

    #[test]
    fn resume_entry_freshness_rejects_old_started_at() {
        let mut e = entry("smb", "/p/smb", "NES", "NES");
        e.started_at = "2026-05-01T00:00:00Z".into();
        assert!(!resume_entry_is_fresh(&e, datetime!(2026-06-03 00:00 UTC)));
    }

    #[test]
    fn resume_entry_freshness_prefers_ended_at() {
        let mut e = entry("smb", "/p/smb", "NES", "NES");
        e.started_at = "2026-05-01T00:00:00Z".into();
        e.ended_at = Some("2026-06-01T00:00:00Z".into());
        assert!(resume_entry_is_fresh(&e, datetime!(2026-06-03 00:00 UTC)));
    }

    #[test]
    fn resume_entry_freshness_rejects_missing_or_malformed_timestamp() {
        let mut e = entry("smb", "/p/smb", "NES", "NES");
        assert!(!resume_entry_is_fresh(&e, datetime!(2026-06-03 00:00 UTC)));
        e.started_at = "not-a-date".into();
        assert!(!resume_entry_is_fresh(&e, datetime!(2026-06-03 00:00 UTC)));
    }

    #[test]
    fn resume_entry_freshness_rejects_future_timestamp() {
        let mut e = entry("smb", "/p/smb", "NES", "NES");
        e.started_at = "2026-06-04T00:00:00Z".into();
        assert!(!resume_entry_is_fresh(&e, datetime!(2026-06-03 00:00 UTC)));
    }

    #[test]
    fn resume_cover_key_always_uses_play_outline() {
        let e = entry("smb", "/p/smb", "NES", "NES");
        assert_eq!(resume_cover_key_for(&e, true), RESUME_FALLBACK_COVER_KEY);
        assert_eq!(resume_cover_key_for(&e, false), RESUME_FALLBACK_COVER_KEY);
    }

    #[test]
    fn cover_key_uses_loading_icon_when_in_flight() {
        let e = entry("smb", "/p/smb", "NES", "NES");
        let key = media_key_for(&e).expect("media has key");
        // Has a key, not cached, not negative → fetch is in flight,
        // show the hourglass over the tile.
        assert_eq!(
            cover_key_for_with(&e, Some(&key), false, false, false),
            "icons/Loading"
        );
    }

    #[test]
    fn cover_key_falls_back_to_system_logo_when_negatively_memoed() {
        let e = entry("smb", "/p/smb", "NES", "NES");
        let key = media_key_for(&e).expect("media has key");
        // Has a key, not cached, negatively memoed → no image to wait
        // for. Fall back to the system logo (friendlier than icons/File
        // for favorites/recents lists).
        assert_eq!(
            cover_key_for_with(&e, Some(&key), false, true, false),
            "systems/NES"
        );
    }

    #[test]
    fn cover_key_falls_back_to_system_logo_when_soft_missed() {
        let e = entry("smb", "/p/smb", "NES", "NES");
        let key = media_key_for(&e).expect("media has key");
        assert_eq!(
            cover_key_for_with(&e, Some(&key), false, false, true),
            "systems/NES"
        );
    }

    #[test]
    fn cover_key_returns_media_image_key_when_cached() {
        let e = entry("smb", "/p/smb", "NES", "NES");
        let key = media_key_for(&e).expect("media has key");
        let expected = MediaImageCache::image_key_for(&key);
        assert_eq!(
            cover_key_for_with(&e, Some(&key), true, false, false),
            expected
        );
        assert!(expected.starts_with("media-image/"));
    }

    #[test]
    fn cover_key_falls_back_to_file_glyph_when_system_missing() {
        let e = entry("orphan", "/p/orphan", "", "");
        assert_eq!(
            cover_key_for_with(&e, None, false, false, false),
            "icons/File"
        );
    }

    #[test]
    fn media_key_for_skips_rows_without_path_or_system() {
        let pathless = entry("ghost", "", "NES", "NES");
        assert!(media_key_for(&pathless).is_none());
        let unattributed = entry("ghost", "/p", "", "");
        assert!(media_key_for(&unattributed).is_none());
        let key =
            media_key_for(&entry("smb", "/p/smb", "NES", "NES")).expect("complete entry has key");
        assert_eq!(key.system_id.as_ref(), "NES");
        assert_eq!(key.path.as_ref(), "/p/smb");
    }

    #[test]
    fn launch_text_uses_raw_media_path_when_launcher_known() {
        let e = entry("smb", "/p/smb.nes", "NES", "NES");
        assert_eq!(launch_text_for(&e), "/p/smb.nes");
    }

    #[test]
    fn launch_text_uses_raw_media_path_when_launcher_missing() {
        let e = entry("smb", "/p/smb.nes", "NES", "");
        assert_eq!(launch_text_for(&e), "/p/smb.nes");
    }

    #[test]
    fn launch_text_preserves_path_with_spaces_and_metacharacters() {
        let path = "/media/fat/cifs/games/Genesis/1 US - A-F/B.O.B. (USA,Europe) (Rev A).md";
        let e = entry("bob", path, "Genesis", "Genesis");
        assert_eq!(launch_text_for(&e), path);
    }

    #[test]
    fn launch_text_preserves_backslashes_and_quotes_in_path() {
        let path = r#"C:\Games\say "hi".rom"#;
        let e = entry("weird", path, "DOS", "DOS");
        assert_eq!(launch_text_for(&e), path);
    }

    #[test]
    fn launch_text_is_empty_when_path_missing_and_launcher_present() {
        // A history row with a launcher id but no path is malformed.
        // Empty here suppresses the run entirely.
        let e = entry("ghost", "", "NES", "NES");
        assert_eq!(launch_text_for(&e), "");
    }

    #[test]
    fn dedupe_latest_by_path_keeps_first_matching_path() {
        let entries = dedupe_latest_by_path(vec![
            entry("latest smb", "/p/smb", "NES", "NES"),
            entry("zelda", "/p/zelda", "NES", "NES"),
            entry("older smb", "/p/smb", "NES", "NES"),
            entry("metroid", "/p/metroid", "NES", "NES"),
        ]);
        let names = entries
            .iter()
            .map(|entry| entry.media_name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["latest smb", "zelda", "metroid"]);
    }

    #[test]
    fn filter_entries_by_path_skips_existing_and_later_incoming_duplicates() {
        let existing = [entry("latest smb", "/p/smb", "NES", "NES")];
        let entries = filter_entries_by_path(
            existing.iter(),
            vec![
                entry("older smb", "/p/smb", "NES", "NES"),
                entry("latest zelda", "/p/zelda", "NES", "NES"),
                entry("older zelda", "/p/zelda", "NES", "NES"),
                entry("metroid", "/p/metroid", "NES", "NES"),
            ],
        );
        let names = entries
            .iter()
            .map(|entry| entry.media_name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["latest zelda", "metroid"]);
    }

    #[test]
    fn dedupe_latest_by_path_preserves_empty_paths() {
        let entries = dedupe_latest_by_path(vec![
            entry("ghost one", "", "NES", "NES"),
            entry("smb", "/p/smb", "NES", "NES"),
            entry("ghost two", "", "NES", "NES"),
            entry("older smb", "/p/smb", "NES", "NES"),
        ]);
        let names = entries
            .iter()
            .map(|entry| entry.media_name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["ghost one", "smb", "ghost two"]);
    }

    #[test]
    fn position_of_path_returns_index_on_match() {
        let entries = vec![
            entry("smb", "/p/smb", "NES", "NES"),
            entry("zelda", "/p/zelda", "NES", "NES"),
        ];
        assert_eq!(position_of_path(&entries, "/p/zelda"), 1);
    }

    #[test]
    fn position_of_path_empty_needle_returns_minus_one() {
        let entries = vec![entry("smb", "/p/smb", "NES", "NES")];
        assert_eq!(position_of_path(&entries, ""), -1);
    }

    #[test]
    fn position_of_path_missing_returns_minus_one() {
        let entries = vec![entry("smb", "/p/smb", "NES", "NES")];
        assert_eq!(position_of_path(&entries, "/missing"), -1);
    }

    #[test]
    fn page_snapshot_carries_entries_and_pagination() {
        let result = MediaHistoryResult {
            entries: vec![entry("smb", "/p/smb", "NES", "NES")],
            pagination: Some(Pagination {
                has_next_page: true,
                page_size: 25,
                next_cursor: Some("cursor-2".into()),
            }),
        };
        let (entries, has_next, cursor) = page_snapshot(&result);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].media_path, "/p/smb");
        assert!(has_next);
        assert_eq!(cursor.as_deref(), Some("cursor-2"));
    }

    #[test]
    fn page_snapshot_without_pagination_disarms_next_page() {
        // Core docs say pagination is omitted when no entries are
        // returned. The snapshot must surface that as `has_next_page
        // = false` so the model disarms `fetch_more` instead of looping
        // on a stale cursor.
        let result = MediaHistoryResult::default();
        let (entries, has_next, cursor) = page_snapshot(&result);
        assert!(entries.is_empty());
        assert!(!has_next);
        assert!(cursor.is_none());
    }

    #[test]
    fn compute_unresolved_keys_empty_entries_returns_empty() {
        let unresolved = compute_unresolved_keys(&[], |_| false, |_| false);
        assert!(unresolved.is_empty());
    }

    #[test]
    fn compute_unresolved_keys_all_cached_returns_empty() {
        let entries = vec![
            entry("smb", "/p/smb", "NES", "NES"),
            entry("zelda", "/p/zelda", "NES", "NES"),
        ];
        let unresolved = compute_unresolved_keys(&entries, |_| true, |_| false);
        assert!(unresolved.is_empty());
    }

    #[test]
    fn compute_unresolved_keys_mixed_returns_only_uncached() {
        let cached_path = "/p/smb";
        let entries = vec![
            entry("smb", cached_path, "NES", "NES"),
            entry("zelda", "/p/zelda", "NES", "NES"),
        ];
        let unresolved =
            compute_unresolved_keys(&entries, |k| k.path.as_ref() == cached_path, |_| false);
        let expected: HashSet<MediaKey> = [MediaKey::new("NES", "/p/zelda")].into_iter().collect();
        assert_eq!(unresolved, expected);
    }

    #[test]
    fn compute_unresolved_keys_excludes_negative_memo() {
        let negative_path = "/p/no-image";
        let entries = vec![
            entry("smb", "/p/smb", "NES", "NES"),
            entry("orphan", negative_path, "NES", "NES"),
        ];
        let unresolved =
            compute_unresolved_keys(&entries, |_| false, |k| k.path.as_ref() == negative_path);
        let expected: HashSet<MediaKey> = [MediaKey::new("NES", "/p/smb")].into_iter().collect();
        assert_eq!(unresolved, expected);
    }

    #[test]
    fn compute_unresolved_keys_excludes_soft_no_image() {
        let soft_path = "/p/soft-no-image";
        let entries = vec![
            entry("smb", "/p/smb", "NES", "NES"),
            entry("orphan", soft_path, "NES", "NES"),
        ];
        let unresolved =
            compute_unresolved_keys(&entries, |_| false, |k| k.path.as_ref() == soft_path);
        let expected: HashSet<MediaKey> = [MediaKey::new("NES", "/p/smb")].into_iter().collect();
        assert_eq!(unresolved, expected);
    }

    #[test]
    fn compute_unresolved_keys_skips_unattributed_rows() {
        // Rows without a system id or path never key into the cache —
        // gate must not wait on them.
        let entries = vec![
            entry("smb", "/p/smb", "NES", "NES"),
            entry("orphan", "", "NES", "NES"),
            entry("ghost", "/p/ghost", "", ""),
        ];
        let unresolved = compute_unresolved_keys(&entries, |_| false, |_| false);
        let expected: HashSet<MediaKey> = [MediaKey::new("NES", "/p/smb")].into_iter().collect();
        assert_eq!(unresolved, expected);
    }
}
