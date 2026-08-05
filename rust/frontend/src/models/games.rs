// Zaparoo Frontend
// Copyright (c) 2026 Wizzo Pty Ltd and the Zaparoo Project contributors.
// SPDX-License-Identifier: LicenseRef-PolyForm-Noncommercial-1.0.0
//
// `Browse.GamesModel` — directory-aware, cumulative paged browse over
// Zaparoo Core's `media.browse` endpoint.
//
// Two paths into the model:
//
//   * `set_system(id)` — entry from SystemsScreen accept. Issues
//     `media.browse({systems: [id]})` (system-scoped roots). When the
//     scoped roots collapse to a single folder entry, we auto-navigate
//     into that folder so the user never sees a 1-item list — most
//     systems have one root and showing it standalone would be empty
//     ceremony.
//
//   * `set_path(path)` — entry from a folder accept inside the games
//     screen. Issues `media.browse({path, systems: [current_system]})`.
//     No auto-navigation; explicit folder navigation is treated as
//     intent.
//
// `fetch_more()` advances pagination without resetting the model:
// `begin_insert_rows` appends entries from the next cursor onto the
// existing vec. Cursor lives in the model, not in the endpoint cache
// key, because cursor-paged follow-ups bypass the cache (one entry per
// cursor would defeat the cache).
//
// `seq` discipline: bumped on every `set_system` / `set_path`, read
// (not bumped) by `fetch_more`. Late callbacks whose ticket doesn't
// match `seq` are dropped — the standard cure for Qt-thread races
// when the user spams direction-arrow + Accept across a model swap.

use crate::media_image_cache::{global_media_image_cache, MediaImageCache, MediaKey};
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
use std::collections::{BTreeSet, HashSet};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::broadcast::error::RecvError;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};
use zaparoo_core::client::ClientError;
use zaparoo_core::endpoints::media_browse::{BrowseArgs, MediaBrowseEndpoint};
use zaparoo_core::endpoints::media_tags_update::MediaTagsUpdateMutation;
use zaparoo_core::endpoints::readers_write::ReadersWriteMutation;
use zaparoo_core::endpoints::run::RunMutation;
use zaparoo_core::media_types::{
    BrowseEntry, BrowseIndexGroup, MediaBrowseIndexParams, MediaBrowseIndexResult,
    MediaBrowseParams, MediaBrowseResult, MediaMeta, MediaMetaParams, MediaTagsUpdateParams,
    Pagination, ReadersWriteParams, RunParams, TagInfo,
};
use zaparoo_core::platform::{self, Platform};
use zaparoo_core::remote_resource::ResourceStatus;

const NAME_ROLE: i32 = 256 + 1;
const PATH_ROLE: i32 = 256 + 2;
const ZAP_SCRIPT_ROLE: i32 = 256 + 3;
const SYSTEM_ID_ROLE: i32 = 256 + 4;
// Folder tiles always use `icons/Folder`. Media tiles default to
// `icons/File` and switch to `media-image/<encoded>` once the in-memory
// media image cache has bytes for `(systemId, path)`;
// Resources.coverUrl() rewrites the `media-image/` prefix to
// `image://media-image/<encoded>` so the QQuickImageProvider serves the
// bytes from RAM.
const COVER_KEY_ROLE: i32 = 256 + 5;
const ENTRY_TYPE_ROLE: i32 = 256 + 6;
const FILE_COUNT_ROLE: i32 = 256 + 7;
const FAVORITE_ROLE: i32 = 256 + 8;
const DESCRIPTION_ROLE: i32 = 256 + 9;
const FILE_STEM_ROLE: i32 = 256 + 10;
const HIDDEN_ROLE: i32 = 256 + 11;
// Newline-joined short tokens for Core's `disambiguatingTags` (e.g.
// "US\nDisc 2"), already ordered by display priority. Empty when the title
// has nothing to disambiguate. A joined string (rather than a list role)
// keeps the QVariant simple and is robust under MiSTer's AOT QML; the
// delegate splits on newlines.
const DISAMBIGUATING_TAGS_ROLE: i32 = 256 + 12;

// Image types that Core's `media.image` endpoint can serve (per the API
// docs). The carousel tail is filtered to this set so left/right never
// lands on a type that would always return "no image".
const CORE_SERVEABLE_IMAGE_TYPES: &[&str] = &[
    "image",
    "boxart",
    "screenshot",
    "wheel",
    "titleshot",
    "map",
    "marquee",
    "fanart",
];

// Default API page size before QML binds the model's `page_size` to the
// grid's `pageSize`. 15 = 5 columns × 3 rows, the desktop default. The
// test harness sees this until it overrides explicitly. Server cap is
// 1000; grid page sizes top out at ~30 so we stay well inside bounds.
const DEFAULT_PAGE_SIZE: i32 = 15;
// `media.browse` `max_results` for cursor follow-ups. Held separate
// from `page_size` (which dictates grid layout, scroll-thumb sizing,
// and the initial-page cover gate) so wire chunks can be larger than a
// single visual page without changing the grid math. Sized for the
// MiSTer main-thread cost of `apply_append_page`: each chunk runs
// `transform_entries` and a `begin_insert_rows`/`end_insert_rows`
// pair on the Qt thread, which stalls input until it returns. 500 was Core's wire-cost optimum but
// produced a multi-second stall on ARM32 with the indicator showing
// the whole time; 100 keeps the per-chunk stall short enough to be
// invisible while still cutting an 805-entry Arcade to ~8 round-trips.
// Server caps `max_results` at 1000; 100 stays well inside that. The
// initial browse keeps the smaller `page_size` so the cover gate
// doesn't have to wait on a big first decode. Cover prefetch is no
// longer driven by metadata page size — `prefetch_around` warms only
// the visible and next pages, so a large `FETCH_MORE_CHUNK_SIZE` no
// longer floods the cover queue.
const FETCH_MORE_CHUNK_SIZE: i32 = 100;
// Rapid-scroll chunk. Bigger chunks are not free: measured on device,
// media.browse response time grows superlinearly with chunk size on a
// large folder (a max-size 1000-row request costs several seconds while
// a 100-row request returns in well under one), and a blocked pager
// waits that whole time. 300 buys ~30 list pages of runway per fetch
// while keeping the worst-case wait at the loaded edge short. The
// background folder fill (GamesScreen.qml) is what makes edge waits
// rare; this chunk only sets how painful one is when it happens.
const FETCH_MORE_RAPID_CHUNK_SIZE: i32 = 300;
// Ceiling for a jump-to-letter fetch (Core's `max_results` cap). A position
// jump must load every row up to the target before
// `PagedGrid._commitPendingTarget` can land on it; doing that as the 12-row
// frame-gapped trickle (`apply_append_page`) takes seconds on a far jump. The
// jump path instead inserts the whole chunk in one shot (`bulk` append) — the
// loading overlay is up and the appended rows sit far from `currentPage`, so
// their Tile delegates stay unmaterialised (the per-cell Loader is
// retention-gated), making the bulk insert cheap. `fetch_more_jump` sizes each
// request to the gap remaining to the target (rounded up to whole `page_size`
// pages, plus one page of margin) rather than always pulling the full
// remainder, so a near/mid-folder jump transfers far less; a gap larger than
// this ceiling chains another bulk call.
const JUMP_FETCH_CHUNK_SIZE: i32 = 1000;
// Stay within Core's `max_results` ceiling, and never smaller than the rapid
// scroll chunk (a jump must not load slower than ordinary scrolling).
const _: () = assert!(JUMP_FETCH_CHUNK_SIZE <= 1000);
const _: () = assert!(JUMP_FETCH_CHUNK_SIZE >= FETCH_MORE_RAPID_CHUNK_SIZE);
const COLLAPSED_DIRECTORY_BROWSE_PAGE_SIZE: u32 = 1000;
const COVER_PREFETCH_NEXT_PAGES: i32 = 2;
const COVER_PREFETCH_PREVIOUS_PAGES: i32 = 1;
// Per-row cursor window used in list-detail layout to keep the byte-fetch
// queue shallow. Two workers at ~250 ms/cover can drain ~6 covers/settle
// cycle well within a normal dwell period, so the next cover is always
// at the front of the queue when the user moves.
const COVER_PREFETCH_CURSOR_NEXT: i32 = 4;
const COVER_PREFETCH_CURSOR_PREV: i32 = 2;
// Bound how long navigation waits for cold visible covers. After this,
// the page becomes interactive and any remaining covers pop in via the
// normal update path. Keeps cold pages from waiting on the slowest
// `media.image` request while preserving no-pop-in for warm/cache-hit pages.
const COVER_GATE_TIMEOUT_MS: u64 = 300;

// `apply_append_page` sub-batches the model insert into chunks of this
// many rows so the Repeater's per-delegate `createObject` cost (the
// dominant Qt-thread stall on MiSTer at ~7-8 ms per Tile) is spread
// across frames instead of one ~750 ms - 1.6 s block. The first batch
// runs synchronously inside the original Qt-thread call so PagedGrid's
// `_commitPendingTarget` fires within ~200 ms of the user's press; the
// rest are posted from `global_handle()` with one frame of breathing
// room (`SUB_BATCH_FRAME_GAP_MS`) between batches so input + paint can
// drain in between. 12 is intentionally below a full dense-grid page:
// more batches trickle in, but each batch has less chance to monopolize
// the Qt thread during a held Down/Up scroll.
const APPEND_SUB_BATCH_SIZE: usize = 12;

// One frame at 30 Hz, the MiSTer software renderer's effective frame
// budget. Gives Qt one full frame to drain input + paint between
// posted sub-batches. Smaller values risk the next batch landing
// before the paint completes; larger values make the trailing rows
// trickle in too slowly.
const SUB_BATCH_FRAME_GAP_MS: u64 = 33;

#[allow(
    clippy::struct_excessive_bools,
    reason = "the bools are independent qproperties surfaced to QML; collapsing them \
              into an enum would force the QML side to read a single state property \
              and re-derive each flag locally, which is exactly the work the bridge \
              avoids"
)]
pub struct GamesModelRust {
    entries: Vec<BrowseEntry>,
    // Parallel to `entries`: the sibling-diffed disambiguation display string
    // per row (what tiles/list rows show as the inline token suffix). Recomputed
    // whenever `entries` or `show_original_filenames` changes. See
    // `compute_disambig_displays` / `recompute_disambig_tail`.
    disambig_displays: Vec<String>,
    count: i32,
    loading: bool,
    loading_more: bool,
    error_message: QString,
    has_next_page: bool,
    // Files-only count from Core (`BrowseFileCount`). Combined with
    // `total_dirs` to compute total entries for the page denominator.
    // Directories always precede files, so `total_dirs + total_files`
    // is exact, not an estimate.
    total_files: i32,
    // Total directory count for the current scope (`BrowseDirCount`),
    // carried forward by Core on every page. Directories always precede
    // files, so `total_dirs + total_files` is the exact entry total.
    total_dirs: i32,
    // API page size, bound to the QML grid's `pageSize` so the API page
    // matches the visual page exactly — no mid-page "Loading more…"
    // pause on the user's first scroll past row 0.
    page_size: i32,
    current_system_id: QString,
    current_path: QString,
    next_cursor: Option<String>,
    card_write_pending: bool,
    card_write_error: QString,
    current_description: QString,
    current_detail_tags: QString,
    current_detail_loading: bool,
    current_detail_image_key: QString,
    current_detail_image_index: i32,
    current_detail_image_count: i32,
    current_detail_image_can_prev: bool,
    current_detail_image_can_next: bool,
    // Warm cover key for the item immediately after the current selection.
    // Empty when there is no next item or the next item is not a media
    // entry with cached bytes. Exposed as a qproperty so QML can mount
    // a hidden Image that decodes the cover into Qt's pixmap cache while
    // the user is still on the current row.
    detail_prefetch_key_next: QString,
    // Same as `detail_prefetch_key_next` but for the item immediately before
    // the current selection.
    detail_prefetch_key_prev: QString,
    // The row whose adjacent covers are currently preloaded. None when no
    // detail is active (cleared on reset, non-media, or out-of-range).
    detail_prefetch_row: Option<i32>,
    cover_key_roles_enabled: bool,
    cover_requests_paused: bool,
    // When true, the `name` role and `name_at()` return the original filename
    // (without extension) instead of Core's cleaned display name. Bound from
    // QML to the `Show original filenames` setting; flipping it re-emits
    // `dataChanged(NAME_ROLE)` across every row so visible names update live.
    show_original_filenames: bool,
    detail_image_keys: Vec<MediaKey>,
    // Watcher for the current initial-page subscription. Aborted on
    // every path swap so the prior watcher stops enqueuing callbacks
    // for the old `BrowseArgs`. Callbacks already in the Qt queue are
    // disarmed by `seq` rather than by the abort.
    watcher: Option<JoinHandle<()>>,
    // Bumped on every `set_system` / `set_path`. Read but NOT bumped
    // by `fetch_more` — cursor-driven follow-ups are part of the same
    // path's load and should keep the same ticket.
    seq: Arc<AtomicU64>,
    // True only for `set_system`-driven initial loads. The single-root
    // auto-nav case is the only place we want to skip the rendered
    // intermediate state, so the flag is consumed by the apply
    // function and reset once the decision has been made.
    auto_nav_eligible: bool,
    // Set once `apply_initial_page` has installed a result for the
    // current browse target; cleared on every `start_initial_browse`.
    // Subsequent Ready transitions on the same target (cache
    // invalidation refetches, e.g. after `MediaTagsUpdateMutation`)
    // skip the full reset so the user's pagination position and
    // appended pages aren't clobbered. Local mutations
    // (`apply_favorite_tags`) keep the visible model in sync without
    // needing the refetch.
    is_seeded: bool,
    // Invalidates stale card-write completions after the user cancels
    // or starts another write before the previous RPC returns.
    card_write_seq: Arc<AtomicU64>,
    // Long-lived bridge from `media_image_cache` broadcast updates onto Qt
    // `dataChanged(coverKey)` emits. Spun up lazily on the first
    // `start_initial_browse` so the model singleton owns exactly one
    // subscriber for the whole process lifetime.
    cover_subscription: Option<JoinHandle<()>>,
    // Keys whose first-paint we're still waiting on. While non-empty we
    // hold `loading = true` so the screen-flip overlay covers the gap
    // between "page rendered with glyphs" and "covers cached". Drained
    // by `notify_cover_update` as each cover lands; force-cleared by
    // the gate timer or a subsequent `start_initial_browse`.
    pending_first_paint_keys: HashSet<MediaKey>,
    // Safety timer that force-releases the cover gate after a bounded
    // delay, so a stalled bulk RPC can't park the user on `Loading…`
    // forever.
    cover_gate_timer: Option<JoinHandle<()>>,
    // Bumped on every cover-gate arm and on every `start_initial_browse`.
    // The timer's queued closure compares against the current value and
    // bails on a mismatch — necessary because aborting the JoinHandle
    // doesn't cancel a callback that was already queued onto the Qt
    // thread between sleep-completion and abort.
    cover_gate_seq: Arc<AtomicU64>,
    description_seq: Arc<AtomicU64>,
    // Tagging ticket for in-flight append sub-batches scheduled by
    // `apply_append_page`. Bumped on every `start_initial_browse` so a
    // deferred batch from a stale dataset detects that the model has
    // moved on and bails before splicing rows from the old chain onto
    // the new one. Same race shape as `cover_gate_seq`, just for the
    // sub-batch fan-out.
    append_seq: Arc<AtomicU64>,
    // True when `apply_initial_page` wants to start the metadata
    // look-ahead `fetch_more` after the visible page is interactive.
    // Starting it before the cover gate releases can splice rows and
    // create delegates during a screen transition, which is exactly
    // the UI-thread stall the loading overlay is trying to hide.
    pending_initial_lookahead: bool,
    // First visible row in the grid. Bound from QML to
    // `gamesGrid.currentPage * gamesGrid.pageSize` so the model knows
    // which entries are on screen and can warm the next page's covers
    // explicitly. Read by `prefetch_around` and by `apply_append_page`
    // so a freshly-landed metadata chunk can re-issue the prefetch
    // window for whatever row the user is currently looking at.
    visible_first_row: i32,
    cover_max_size: i32,
    // Bounding-box size requested for detail-pane images, baked into the
    // detail keys so the pane fetches its own larger resolution instead of
    // sharing the grid's cover entry. `0` keeps the legacy shared-key
    // behavior (no separate detail fetch).
    detail_cover_max_size: i32,
    nav_timing: Option<NavTiming>,
    // When the user's cover preference is "auto" and Core's `type_tag` for
    // the index-0 key is not yet known (cover still in-flight), we stash the
    // filtered concrete type keys here. `notify_cover_update` finalizes the
    // carousel once the cover lands and the type can be looked up, then clears
    // this field.
    pending_carousel_keys: Option<Vec<MediaKey>>,
    // Latest `media.browse.index` facet for the current scope, surfaced to QML
    // for the jump-to-letter rail. `letter_index_json` is a JSON array of
    // `{key,label,count,cursor}` objects (the QML pickers consume `var` arrays,
    // so a parsed-in-QML JSON string fits that convention without a second
    // QAbstractListModel for transient modal data). `letter_index_scheme` is
    // the facet scheme (`latin`/`none`); QML omits the menu entry when no rail
    // applies. Bumped each `load_letter_index` so a stale in-flight facet from a
    // prior scope can't overwrite the current one.
    letter_index_json: QString,
    letter_index_scheme: QString,
    letter_index_seq: Arc<AtomicU64>,
    // Random-pick state. `random_seeking` is true while the walk that
    // resolves a uniformly chosen position is in flight; `random_error`
    // carries a stable reason code (never prose) so a failed pick reports
    // something instead of silently doing nothing. Superseded by
    // `random_seq` when a newer press or a scope change arrives.
    random_seeking: bool,
    random_error: QString,
    random_seq: Arc<AtomicU64>,
}

impl Default for GamesModelRust {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            disambig_displays: Vec::new(),
            count: 0,
            loading: false,
            loading_more: false,
            error_message: QString::default(),
            has_next_page: false,
            total_files: 0,
            total_dirs: 0,
            page_size: DEFAULT_PAGE_SIZE,
            current_system_id: QString::default(),
            current_path: QString::default(),
            next_cursor: None,
            card_write_pending: false,
            card_write_error: QString::default(),
            current_description: QString::default(),
            current_detail_tags: QString::default(),
            current_detail_loading: false,
            current_detail_image_key: QString::default(),
            current_detail_image_index: 0,
            current_detail_image_count: 0,
            current_detail_image_can_prev: false,
            current_detail_image_can_next: false,
            detail_prefetch_key_next: QString::default(),
            detail_prefetch_key_prev: QString::default(),
            detail_prefetch_row: None,
            cover_key_roles_enabled: true,
            cover_requests_paused: false,
            show_original_filenames: false,
            detail_image_keys: Vec::new(),
            watcher: None,
            seq: Arc::new(AtomicU64::new(0)),
            auto_nav_eligible: false,
            is_seeded: false,
            card_write_seq: Arc::new(AtomicU64::new(0)),
            cover_subscription: None,
            pending_first_paint_keys: HashSet::new(),
            cover_gate_timer: None,
            cover_gate_seq: Arc::new(AtomicU64::new(0)),
            description_seq: Arc::new(AtomicU64::new(0)),
            append_seq: Arc::new(AtomicU64::new(0)),
            pending_initial_lookahead: false,
            visible_first_row: 0,
            cover_max_size: 0,
            detail_cover_max_size: 0,
            nav_timing: None,
            pending_carousel_keys: None,
            letter_index_json: QString::default(),
            letter_index_scheme: QString::default(),
            letter_index_seq: Arc::new(AtomicU64::new(0)),
            random_seeking: false,
            random_error: QString::default(),
            random_seq: Arc::new(AtomicU64::new(0)),
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
        #[qproperty(i32, total_files)]
        #[qproperty(i32, total_dirs)]
        #[qproperty(i32, page_size)]
        #[qproperty(QString, current_system_id)]
        #[qproperty(QString, current_path)]
        #[qproperty(bool, card_write_pending)]
        #[qproperty(QString, card_write_error)]
        #[qproperty(QString, current_description)]
        #[qproperty(QString, current_detail_tags)]
        #[qproperty(bool, current_detail_loading)]
        #[qproperty(QString, current_detail_image_key)]
        #[qproperty(i32, current_detail_image_index)]
        #[qproperty(i32, current_detail_image_count)]
        #[qproperty(bool, current_detail_image_can_prev)]
        #[qproperty(bool, current_detail_image_can_next)]
        #[qproperty(QString, detail_prefetch_key_next)]
        #[qproperty(QString, detail_prefetch_key_prev)]
        #[qproperty(bool, cover_key_roles_enabled)]
        #[qproperty(bool, cover_requests_paused)]
        #[qproperty(bool, show_original_filenames, READ, WRITE = set_show_original_filenames, NOTIFY)]
        #[qproperty(i32, visible_first_row)]
        #[qproperty(i32, cover_max_size, READ, WRITE = set_cover_max_size, NOTIFY)]
        #[qproperty(i32, detail_cover_max_size, READ, WRITE = set_detail_cover_max_size, NOTIFY)]
        #[qproperty(QString, letter_index_json)]
        #[qproperty(QString, letter_index_scheme)]
        #[qproperty(bool, random_seeking)]
        #[qproperty(QString, random_error)]
        type GamesModel = super::GamesModelRust;

        #[qinvokable]
        fn set_cover_max_size(self: Pin<&mut GamesModel>, size: i32);

        #[qinvokable]
        fn set_detail_cover_max_size(self: Pin<&mut GamesModel>, size: i32);

        #[qinvokable]
        fn set_system(self: Pin<&mut GamesModel>, system_id: QString);

        #[qinvokable]
        fn set_path(self: Pin<&mut GamesModel>, path: &QString);

        #[qinvokable]
        fn browse_cached_for_path(self: &GamesModel, path: &QString) -> bool;

        #[qinvokable]
        fn fetch_more(self: Pin<&mut GamesModel>);

        #[qinvokable]
        fn fetch_more_rapid(self: Pin<&mut GamesModel>);

        #[qinvokable]
        fn fetch_more_jump(self: Pin<&mut GamesModel>, target_index: i32);

        #[qinvokable]
        fn load_letter_index(self: Pin<&mut GamesModel>);

        #[qinvokable]
        fn prefetch_around(self: Pin<&mut GamesModel>, first_visible_row: i32);

        #[qinvokable]
        fn refresh_cover_keys(self: Pin<&mut GamesModel>, first_row: i32, count: i32);

        #[qinvokable]
        fn clear_pending_cover_requests(self: Pin<&mut GamesModel>);

        #[qinvokable]
        fn launch_at(self: Pin<&mut GamesModel>, index: i32);

        #[qinvokable]
        fn launch_text_at(self: &GamesModel, index: i32) -> QString;

        #[qinvokable]
        fn launch_random(self: Pin<&mut GamesModel>);

        #[qinvokable]
        fn clear_random_error(self: Pin<&mut GamesModel>);

        #[qinvokable]
        fn write_card_at(self: Pin<&mut GamesModel>, index: i32);

        #[qinvokable]
        fn toggle_favorite_at(self: Pin<&mut GamesModel>, index: i32);

        #[qinvokable]
        fn is_favorite_at(self: &GamesModel, index: i32) -> bool;

        #[qinvokable]
        fn cancel_card_write(self: Pin<&mut GamesModel>);

        #[qinvokable]
        fn set_show_original_filenames(self: Pin<&mut GamesModel>, value: bool);

        #[qinvokable]
        fn name_at(self: &GamesModel, index: i32) -> QString;

        #[qinvokable]
        fn disambiguating_tags_at(self: &GamesModel, index: i32) -> QString;

        #[qinvokable]
        fn description_at(self: &GamesModel, index: i32) -> QString;

        #[qinvokable]
        fn peek_description_at(self: Pin<&mut GamesModel>, index: i32);

        #[qinvokable]
        fn load_description_at(self: Pin<&mut GamesModel>, index: i32);

        #[qinvokable]
        fn clear_current_detail(self: Pin<&mut GamesModel>);

        #[qinvokable]
        fn cycle_detail_image(self: Pin<&mut GamesModel>, delta: i32);

        #[qinvokable]
        fn path_at(self: &GamesModel, index: i32) -> QString;

        #[qinvokable]
        fn system_id_at(self: &GamesModel, index: i32) -> QString;

        #[qinvokable]
        fn entry_type_at(self: &GamesModel, index: i32) -> QString;

        #[qinvokable]
        fn is_media_capable_at(self: &GamesModel, index: i32) -> bool;

        #[qinvokable]
        fn index_for_game_path(self: &GamesModel, path: &QString) -> i32;

        #[inherit]
        #[cxx_name = "beginResetModel"]
        fn begin_reset_model(self: Pin<&mut GamesModel>);

        #[inherit]
        #[cxx_name = "endResetModel"]
        fn end_reset_model(self: Pin<&mut GamesModel>);

        #[inherit]
        #[cxx_name = "beginInsertRows"]
        fn begin_insert_rows(
            self: Pin<&mut GamesModel>,
            parent: &QModelIndex,
            first: i32,
            last: i32,
        );

        #[inherit]
        #[cxx_name = "endInsertRows"]
        fn end_insert_rows(self: Pin<&mut GamesModel>);

        // Qt signal bound as a callable so the cover-cache bridge can
        // invoke it directly from the Qt thread when an async cover
        // fetch completes for a row that is already on screen.
        #[inherit]
        #[cxx_name = "dataChanged"]
        fn data_changed(
            self: Pin<&mut GamesModel>,
            top_left: &QModelIndex,
            bottom_right: &QModelIndex,
            roles: &QList_i32,
        );

        #[cxx_name = "rowCount"]
        fn row_count(self: &GamesModel, parent: &QModelIndex) -> i32;
        fn data(self: &GamesModel, index: &QModelIndex, role: i32) -> QVariant;
        #[cxx_name = "roleNames"]
        fn role_names(self: &GamesModel) -> QHash_i32_QByteArray;

        // Materialise a `QModelIndex` for `(row, column)` so the cover-
        // cache bridge can target individual rows in `dataChanged`.
        // Forwarded to the QAbstractListModel implementation.
        #[inherit]
        fn index(self: &GamesModel, row: i32, column: i32, parent: &QModelIndex) -> QModelIndex;
    }

    impl cxx_qt::Threading for GamesModel {}
}

impl ffi::GamesModel {
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
                display_name_for_entry(entry, self.show_original_filenames).as_ref(),
            )),
            PATH_ROLE => QVariant::from(&QString::from(entry.path.as_str())),
            ZAP_SCRIPT_ROLE => QVariant::from(&QString::from(entry.zap_script.as_str())),
            SYSTEM_ID_ROLE => QVariant::from(&QString::from(entry_system_id(entry).as_str())),
            COVER_KEY_ROLE => QVariant::from(&QString::from(
                if self.cover_key_roles_enabled {
                    cover_key_for(
                        entry,
                        u32::try_from(self.page_size.max(1)).unwrap_or(1),
                        !self.cover_requests_paused,
                    )
                } else {
                    cover_placeholder_for(entry)
                }
                .as_str(),
            )),
            ENTRY_TYPE_ROLE => QVariant::from(&QString::from(entry.entry_type.as_str())),
            FILE_COUNT_ROLE => QVariant::from(&i32::try_from(entry.file_count).unwrap_or(i32::MAX)),
            FAVORITE_ROLE => QVariant::from(&favorite_role_value(&entry.tags)),
            DESCRIPTION_ROLE => QVariant::from(&QString::from(entry.description.as_str())),
            FILE_STEM_ROLE => {
                QVariant::from(&QString::from(file_stem_or_name(&entry.path, &entry.name)))
            }
            HIDDEN_ROLE => QVariant::from(&false),
            // Sibling-diffed display string (common-affix trimmed against
            // same-named neighbors); precomputed in `disambig_displays`.
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
        h.insert(ZAP_SCRIPT_ROLE, QByteArray::from("zapScript"));
        h.insert(SYSTEM_ID_ROLE, QByteArray::from("systemId"));
        h.insert(COVER_KEY_ROLE, QByteArray::from("coverKey"));
        h.insert(ENTRY_TYPE_ROLE, QByteArray::from("entryType"));
        h.insert(FILE_COUNT_ROLE, QByteArray::from("fileCount"));
        h.insert(FAVORITE_ROLE, QByteArray::from("favorite"));
        h.insert(DESCRIPTION_ROLE, QByteArray::from("description"));
        h.insert(FILE_STEM_ROLE, QByteArray::from("fileStem"));
        h.insert(HIDDEN_ROLE, QByteArray::from("hidden"));
        h.insert(
            DISAMBIGUATING_TAGS_ROLE,
            QByteArray::from("disambiguatingTags"),
        );
        h
    }

    fn set_cover_max_size(mut self: Pin<&mut Self>, size: i32) {
        let clamped = size.max(0);
        self.as_mut().rust_mut().cover_max_size = clamped;
        global_media_image_cache().set_max_cover_size(u32::try_from(clamped).unwrap_or(0));
    }

    // The detail size is baked into the detail keys (see `set_detail_image_keys`)
    // rather than pushed to the cache's global default, so it stays independent
    // of the grid's `set_cover_max_size`.
    fn set_detail_cover_max_size(mut self: Pin<&mut Self>, size: i32) {
        let clamped = size.max(0);
        if self.detail_cover_max_size == clamped {
            return;
        }
        self.as_mut().rust_mut().detail_cover_max_size = clamped;
        let keys = self.detail_image_keys.clone();
        if !keys.is_empty() {
            set_detail_image_keys(self.as_mut(), keys);
        }
    }

    fn set_system(mut self: Pin<&mut Self>, system_id: QString) {
        let sid = system_id.to_string();
        self.as_mut().set_current_system_id(system_id);
        // No-op short-circuit removed deliberately: SystemsScreen
        // accept always wants a "show me this system again" round
        // trip after Esc-back, even when the same system is current.
        // The Endpoint cache makes the repeat cheap (cached
        // `BrowseArgs` returns its existing `Ready` value
        // synchronously through the watcher's seed).
        let systems = if sid.is_empty() {
            Vec::new()
        } else {
            vec![sid]
        };
        self.start_initial_browse(String::new(), systems, true);
    }

    fn set_path(self: Pin<&mut Self>, path: &QString) {
        let p = path.to_string();
        let sid = self.current_system_id.to_string();
        let systems = if sid.is_empty() {
            Vec::new()
        } else {
            vec![sid]
        };
        self.start_initial_browse(p, systems, false);
    }

    fn browse_cached_for_path(&self, path: &QString) -> bool {
        let sid = self.current_system_id.to_string();
        let systems = if sid.is_empty() {
            Vec::new()
        } else {
            vec![sid]
        };
        let max_results = u32::try_from(self.page_size.max(1)).unwrap_or(u32::from(u16::MAX));
        global_store().is_ready::<MediaBrowseEndpoint>(&BrowseArgs::new(
            path.to_string(),
            systems,
            max_results,
        ))
    }

    fn fetch_more(self: Pin<&mut Self>) {
        self.fetch_more_with_limit(FETCH_MORE_CHUNK_SIZE, false);
    }

    fn fetch_more_rapid(self: Pin<&mut Self>) {
        self.fetch_more_with_limit(FETCH_MORE_RAPID_CHUNK_SIZE, false);
    }

    fn fetch_more_jump(self: Pin<&mut Self>, target_index: i32) {
        // Size the fetch to the gap remaining to the jump target instead of
        // always pulling the server max. `target_index` is the absolute grid
        // row the cursor will land on (it shares the model's row space, so it
        // includes leading directories); `count` is the rows already loaded.
        // A near/mid-folder jump transfers far less than the full remainder; an
        // under-shoot (huge folder, or the target moved) just re-enters here via
        // the pending-target loop and converges.
        let limit = jump_fetch_limit(target_index, self.count, self.page_size);
        self.fetch_more_with_limit(limit, true);
    }

    fn fetch_more_with_limit(mut self: Pin<&mut Self>, limit: i32, bulk: bool) {
        // Debounce: PagedGrid fires `loadMoreRequested` once per page
        // turn, but a fast spinner on the last loaded page can fire
        // twice before the first follow-up returns. All three guards
        // matter: `loading_more` covers in-flight, `has_next_page`
        // covers terminal pages, and `next_cursor.is_none()` covers
        // the in-between window in `apply_append_page` where the final
        // chunk has cleared the cursor before flipping
        // `has_next_page` (the flip is intentionally deferred so the
        // pending-target watchdog reads a fresh `itemCount`). Without
        // this third guard, a `_commitPendingTarget` re-fire from
        // `onItemCountChanged` would dispatch `media.browse` with
        // `cursor=None`, re-fetching from page 1 and splicing
        // duplicates onto the loaded slice.
        if self.loading_more || !self.has_next_page || self.next_cursor.is_none() {
            return;
        }
        let cursor = self.next_cursor.clone();
        let path = self.current_path.to_string();
        let sid = self.current_system_id.to_string();
        let systems = if sid.is_empty() {
            Vec::new()
        } else {
            vec![sid]
        };
        // Read the active ticket WITHOUT bumping it. `fetch_more`
        // continues the same path's load — only `set_system` /
        // `set_path` invalidate the prior cursor sequence.
        let seq = self.seq.clone();
        let ticket = seq.load(Ordering::SeqCst);
        let max_results = u32::try_from(limit.max(1)).unwrap_or(u32::from(u16::MAX));
        // Jump path: pause cover fetching for the duration of this request and
        // drop anything queued. Covers travel over the same WebSocket as
        // `media.browse`, so the cold-entry cover storm (the leaving page's
        // covers) head-of-line-blocks the jump's browse request at Core — this
        // is why the *first* jump after entering a folder is the slow one.
        // Pausing dedicates the socket to the browse call; `apply_append_page`
        // resumes covers and warms the landing page once the rows land, and
        // `start_initial_browse` resets the flag if the user leaves mid-jump
        // (the in-flight response is then ticket-rejected before it can resume).
        if bulk {
            self.as_mut().set_cover_requests_paused(true);
            global_media_image_cache().clear_pending_requests();
        }
        self.as_mut().set_loading_more(true);
        let qt_thread = self.qt_thread();
        let store = global_store();
        // Capture the cursor we're advancing from. If `next_cursor`
        // differs by the time the response arrives (e.g. a watcher
        // refetch reset the chain via `apply_initial_page`), this
        // append no longer belongs to the current page chain and
        // would corrupt the freshly-reset entries.
        let expected_prev_cursor = cursor.clone();
        global_handle().spawn(async move {
            let result = store
                .client()
                .media_browse(MediaBrowseParams {
                    path,
                    systems,
                    max_results: Some(max_results),
                    cursor,
                    letter: None,
                    sort: None,
                })
                .await;
            let _ = qt_thread.queue(move |model| {
                if seq.load(Ordering::SeqCst) != ticket {
                    return;
                }
                apply_append_page(model, result, expected_prev_cursor.as_deref(), bulk);
            });
        });
    }

    /// Fetch the `media.browse.index` facet for the current scope and surface
    /// it to QML as `letter_index_json` / `letter_index_scheme` for the
    /// jump-to-letter rail. Fire-and-forget: the router calls this when the
    /// page-ops menu opens so the buckets are likely ready by the time the
    /// user picks "Jump to letter".
    fn load_letter_index(mut self: Pin<&mut Self>) {
        let path = self.current_path.to_string();
        // A root listing has no meaningful first-character rail.
        if path.is_empty() {
            self.as_mut().set_letter_index_json(QString::from("[]"));
            self.as_mut().set_letter_index_scheme(QString::from("none"));
            return;
        }
        // A complete local listing answers the facet with no request at
        // all: the buckets fall out of the rows already in the model.
        // Beyond saving the round trip, the local facet's offsets are
        // authoritative for the exact rows on screen, including folders
        // where the direct listing's SortName order diverges from a
        // Core-side sort mode the RPC facet would describe instead. The
        // seq bump retires any in-flight RPC facet from a prior scope.
        if scope_fully_loaded(self.count, self.has_next_page, self.next_cursor.as_deref()) {
            self.letter_index_seq.fetch_add(1, Ordering::SeqCst);
            let index = local_letter_index(&self.entries);
            self.as_mut()
                .set_letter_index_json(QString::from(index.groups_json().as_str()));
            self.as_mut()
                .set_letter_index_scheme(QString::from(index.scheme.as_str()));
            return;
        }
        let sid = self.current_system_id.to_string();
        let systems = if sid.is_empty() {
            Vec::new()
        } else {
            vec![sid]
        };
        // Clear any facet from a prior scope to the loading state (empty groups,
        // empty scheme) so the rail shows "loading" rather than the previous
        // folder's buckets until the fresh fetch lands.
        self.as_mut().set_letter_index_json(QString::from("[]"));
        self.as_mut().set_letter_index_scheme(QString::default());
        // Ticket so a stale facet from a prior scope can't overwrite a newer
        // one if the user moves on before this resolves.
        let seq = self.letter_index_seq.clone();
        let ticket = seq.fetch_add(1, Ordering::SeqCst) + 1;
        let qt_thread = self.qt_thread();
        let store = global_store();
        global_handle().spawn(async move {
            let result = store
                .client()
                .media_browse_index(MediaBrowseIndexParams {
                    path,
                    systems,
                    sort: None,
                })
                .await;
            let _ = qt_thread.queue(move |mut model| {
                if seq.load(Ordering::SeqCst) != ticket {
                    return;
                }
                match result {
                    Ok(index) => {
                        let json = index.groups_json();
                        model
                            .as_mut()
                            .set_letter_index_json(QString::from(json.as_str()));
                        model
                            .as_mut()
                            .set_letter_index_scheme(QString::from(index.scheme.as_str()));
                    }
                    Err(e) => {
                        warn!("media.browse.index failed: {}", e.message);
                        model.as_mut().set_letter_index_json(QString::from("[]"));
                        model
                            .as_mut()
                            .set_letter_index_scheme(QString::from("none"));
                    }
                }
            });
        });
    }

    /// Rebuild the cover queue around the current visual page.
    ///
    /// `first_visible_row` is the row index of the topmost visible
    /// tile (`gamesGrid.currentPage * gamesGrid.pageSize` from QML).
    /// The replacement queue drains current page first, then several
    /// next pages, then the previous page. Queued stale requests are
    /// dropped so a page change immediately makes the new visible page
    /// win.
    fn prefetch_around(self: Pin<&mut Self>, first_visible_row: i32) {
        let cache = global_media_image_cache();
        if self.cover_requests_paused {
            cache.clear_pending_requests();
            return;
        }
        let plan =
            prefetch_around_plan(&self.entries, self.count, self.page_size, first_visible_row);
        let ps = u32::try_from(self.page_size.max(1)).unwrap_or(1);
        cache.replace_pending_requests_ordered(plan, ps);
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
        let entry = self.entries[index as usize].clone();
        let params = singleton_directory_needs_launch_resolution(&entry)
            .then(|| meta_params_for_entry(&entry))
            .flatten();
        let browse_params = media_capable_directory_browse_params(&entry);
        let fallback_text = run_text_for_entry(&entry);
        if params.is_none() && browse_params.is_none() && fallback_text.is_none() {
            return;
        }
        let name = entry.name.clone();
        let needs_resolution = params.is_some() || browse_params.is_some();
        let store = global_store();
        global_handle().spawn(async move {
            let text = if needs_resolution {
                let client = store.client();
                resolve_media_capable_directory_run_text(
                    client.as_ref(),
                    &entry,
                    params,
                    browse_params,
                    fallback_text,
                )
                .await
            } else {
                fallback_text
            };
            let Some(text) = text else {
                warn!("media-capable directory launch fallback unavailable for {name}; not launching container path");
                return;
            };
            if let Err(e) = store.run_mutation::<RunMutation>(RunParams { text }).await {
                warn!("run failed for {name}: {}", e.message);
            }
        });
    }

    fn launch_text_at(&self, index: i32) -> QString {
        if index < 0 || index >= self.count {
            return QString::default();
        }
        QString::from(portable_text_for_entry(&self.entries[index as usize]).as_str())
    }

    /// Launch a uniformly chosen game from the folder currently listed.
    ///
    /// Deliberately does NOT use Core's `**launch.random`: that picks a random
    /// database row id within the system's id range and takes the next row at
    /// or above it, so a game's odds are proportional to the gap in front of
    /// it. On a system whose rows were added over time (arcade sets churn
    /// constantly) one game wins nearly every draw.
    ///
    /// Instead: draw a position uniformly over the scope's launchable rows and
    /// resolve that position. Whichever row sits at a given position is
    /// irrelevant to how it got there, so gaps, deletions and re-indexing
    /// cannot skew it. Resolves locally with no request when the folder is
    /// already fully loaded, which is the common case.
    fn launch_random(mut self: Pin<&mut Self>) {
        // A new press supersedes an in-flight walk.
        let ticket = self.rust().random_seq.fetch_add(1, Ordering::SeqCst) + 1;
        if !self.random_error.is_empty() {
            self.as_mut().set_random_error(QString::default());
        }

        // Fast path: everything is already here, so the pick needs no RPC and
        // no Core-reported totals (which can disagree with what arrived).
        if scope_fully_loaded(self.count, self.has_next_page, self.next_cursor.as_deref()) {
            let pool = launchable_count(&self.entries);
            if pool == 0 {
                self.as_mut().fail_random("empty-scope");
                return;
            }
            let target = random_index(pool);
            let Some(text) = launchable_entries(&self.entries)
                .nth(target)
                .and_then(run_text_for_entry)
            else {
                self.as_mut().fail_random("no-launch-payload");
                return;
            };
            self.as_mut().set_random_seeking(false);
            spawn_random_run(text);
            return;
        }

        // Slow path: walk the listing counting launchable rows until the
        // drawn position. `total_files` bounds the draw; it is never used as
        // an index, so an inaccurate count costs at most a failed walk that
        // reports itself.
        let pool = usize::try_from(self.total_files).unwrap_or(0);
        if pool == 0 {
            self.as_mut().fail_random("empty-scope");
            return;
        }
        let target = random_index(pool);
        let path = self.current_path.to_string();
        let sid = self.current_system_id.to_string();
        let systems = if sid.is_empty() {
            Vec::new()
        } else {
            vec![sid]
        };
        let leading_dirs = self.total_dirs;
        let random_seq = self.rust().random_seq.clone();
        // Also ride the browse ticket so navigating to another folder
        // abandons this walk.
        let browse_seq = self.rust().seq.clone();
        let browse_ticket = browse_seq.load(Ordering::SeqCst);
        let qt_thread = self.qt_thread();
        self.as_mut().set_random_seeking(true);

        spawn_random_walk(RandomWalk {
            qt_thread,
            random_seq,
            ticket,
            browse_seq,
            browse_ticket,
            path,
            systems,
            leading_dirs,
            target,
        });
    }

    fn clear_random_error(mut self: Pin<&mut Self>) {
        if !self.random_error.is_empty() {
            self.as_mut().set_random_error(QString::default());
        }
    }

    /// Report a random pick that could not be resolved. Never silent: the
    /// screen has a one-shot action that visibly did nothing otherwise.
    fn fail_random(mut self: Pin<&mut Self>, reason: &str) {
        warn!("random game pick failed: {reason}");
        self.as_mut().set_random_seeking(false);
        self.as_mut().set_random_error(QString::from(reason));
    }

    fn write_card_at(mut self: Pin<&mut Self>, index: i32) {
        if index < 0 || index >= self.count {
            self.as_mut()
                .set_card_write_error(QString::from("invalid selection"));
            self.as_mut().set_card_write_pending(false);
            return;
        }
        let entry = &self.entries[index as usize];
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
        if index < 0 || index >= self.count {
            return;
        }
        let entry = &self.entries[index as usize];
        if !is_media_capable_entry(entry) {
            return;
        }
        let Some(params) = favorite_params_for_entry(entry, !has_favorite_tag(&entry.tags)) else {
            warn!(
                "favorite update skipped: missing media identity for {}",
                entry.name
            );
            return;
        };
        let name = entry.name.clone();
        let media_id = entry.media_id;
        let system_id = entry_system_id(entry);
        let path = entry.path.clone();
        let store = global_store();
        let qt_thread = self.qt_thread();
        global_handle().spawn(async move {
            match store.run_mutation::<MediaTagsUpdateMutation>(params).await {
                Ok(result) => {
                    let _ = qt_thread.queue(move |mut model| {
                        apply_favorite_tags(
                            model.as_mut(),
                            index,
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
        if index < 0 || index >= self.count {
            return false;
        }
        has_favorite_tag(&self.entries[index as usize].tags)
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
        if index < 0 || index >= self.count {
            return QString::default();
        }
        QString::from(
            display_name_for_entry(&self.entries[index as usize], self.show_original_filenames)
                .as_ref(),
        )
    }

    // Full (untrimmed) disambiguation tokens for the focused-item readout
    // (ActiveLabel). Tiles show the sibling-diffed `disambig_displays`; the
    // footer shows everything, space-joined.
    fn disambiguating_tags_at(&self, index: i32) -> QString {
        if index < 0 || index >= self.count {
            return QString::default();
        }
        QString::from(
            disambiguating_tag_labels(&self.entries[index as usize].disambiguating_tags)
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
        // The displayed name drives sibling grouping, so the trimmed tag
        // displays change too (with the toggle on, names are distinct and
        // nothing groups). Recompute them before re-emitting.
        let displays = compute_disambig_displays(&self.entries, value);
        self.as_mut().rust_mut().disambig_displays = displays;
        // Every visible name role is now stale — re-emit name + tags across all
        // rows so the grid caption, list rows, and active label refresh in place.
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

    fn description_at(&self, index: i32) -> QString {
        if index < 0 || index >= self.count {
            return QString::default();
        }
        QString::from(self.entries[index as usize].description.as_str())
    }

    // Immediate, non-debounced sibling of `load_description_at`. Called the
    // moment the focused row changes so the detail pane reflects THIS row's
    // local metadata (description + entry tags) at once, instead of holding the
    // previous row's values through the load debounce. The debounced
    // `load_description_at` then enriches it from media.meta. Games entries
    // carry their own tags, so this needs no metadata cache.
    fn peek_description_at(mut self: Pin<&mut Self>, index: i32) {
        self.as_mut()
            .rust_mut()
            .description_seq
            .fetch_add(1, Ordering::SeqCst);
        if index < 0 || index >= self.count {
            self.as_mut().set_current_detail_loading(false);
            self.as_mut().set_current_description(QString::default());
            self.as_mut().set_current_detail_tags(QString::default());
            clear_detail_images(self.as_mut());
            return;
        }
        self.as_mut().rust_mut().detail_prefetch_row = Some(index);
        prefetch_cursor_window(&self, index);

        let entry = &self.entries[index as usize];
        if !is_media_capable_entry(entry) {
            self.as_mut().set_current_detail_loading(false);
            self.as_mut().set_current_description(QString::default());
            self.as_mut().set_current_detail_tags(QString::default());
            clear_detail_images(self.as_mut());
            return;
        }

        let description = entry.description.clone();
        let detail_tags = detail_tags_from_entry(entry);
        // Local tags paint immediately, so mark loading without blanking: the
        // detail pane shows this row's values while the richer media.meta fetch
        // is still pending.
        self.as_mut().set_current_detail_loading(true);
        self.as_mut()
            .set_current_description(QString::from(description.as_str()));
        self.as_mut()
            .set_current_detail_tags(QString::from(detail_tags.as_str()));
        // Deliberately do NOT switch the visible cover here. The cover has its
        // own grace-window hold (BrowseDetailPane coverHold) and is settled by
        // the debounced `load_description_at`. Re-pointing the 512px cover Image
        // on every keypress kept `_coverBusy` true through sustained navigation
        // (tripping the hourglass after the grace) and monopolized Qt's async
        // image loader so the next-row cover never got prefetched. Peek only
        // updates the metadata table and warms the prefetch hints below.
        refresh_adjacent_cover_prefetch(self.as_mut());
    }

    #[allow(
        clippy::too_many_lines,
        reason = "Model method: closure bound to Qt-thread queue cannot easily be split further"
    )]
    fn load_description_at(mut self: Pin<&mut Self>, index: i32) {
        let ticket = self
            .as_mut()
            .rust_mut()
            .description_seq
            .fetch_add(1, Ordering::SeqCst)
            + 1;
        if index < 0 || index >= self.count {
            self.as_mut().set_current_detail_loading(false);
            self.as_mut().set_current_description(QString::default());
            self.as_mut().set_current_detail_tags(QString::default());
            clear_detail_images(self.as_mut());
            return;
        }

        // Record the settled row before borrowing entries, so
        // `refresh_adjacent_cover_prefetch` can read it once entry borrows drop.
        self.as_mut().rust_mut().detail_prefetch_row = Some(index);
        // Re-center the byte-fetch queue on the cursor using a narrow window.
        // The full-page `prefetch_around` enqueues ~120 covers per settle and
        // floods the 2-worker queue so the next cover waits seconds behind the
        // backlog. A tight window (4 forward + 2 back) keeps the queue shallow
        // enough that the next cover is fetched first within ~250 ms, reliably
        // warm before the user moves to it.
        prefetch_cursor_window(&self, index);

        let entry = &self.entries[index as usize];
        if !is_media_capable_entry(entry) {
            self.as_mut().set_current_detail_loading(false);
            self.as_mut().set_current_description(QString::default());
            self.as_mut().set_current_detail_tags(QString::default());
            clear_detail_images(self.as_mut());
            return;
        }

        let description = entry.description.clone();
        let detail_tags = detail_tags_from_entry(entry);
        // Use the cover-preference key as the synchronous primary so the detail
        // pane requests the same cache entry that `prefetch_around` already
        // warmed for the focused row — instant paint with no hourglass.
        let detail_image_key = media_key_for(entry).map(MediaKey::with_current_cover_preference);
        let Some(params) = meta_params_for_entry(entry) else {
            self.as_mut().set_current_detail_loading(false);
            self.as_mut()
                .set_current_description(QString::from(description.as_str()));
            self.as_mut()
                .set_current_detail_tags(QString::from(detail_tags.as_str()));
            set_single_detail_image_key(self.as_mut(), detail_image_key);
            refresh_adjacent_cover_prefetch(self.as_mut());
            return;
        };

        self.as_mut().set_current_detail_loading(true);
        self.as_mut()
            .set_current_description(QString::from(description.as_str()));
        self.as_mut()
            .set_current_detail_tags(QString::from(detail_tags.as_str()));
        set_single_detail_image_key(self.as_mut(), detail_image_key);
        refresh_adjacent_cover_prefetch(self.as_mut());

        let seq = self.rust().description_seq.clone();
        let qt_thread = self.qt_thread();
        let store = global_store();
        global_handle().spawn(async move {
            let result = match crate::media_meta_db::media_meta_async(params.clone()).await {
                Some(media) => Ok(zaparoo_core::media_types::MediaMetaResult { media }),
                None => store.client().media_meta(params).await,
            };
            let _ = qt_thread.queue(move |mut model| {
                if seq.load(Ordering::SeqCst) != ticket {
                    return;
                }
                match result {
                    Ok(result) => {
                        let meta = result.media;
                        let description = description_from_meta(&meta);
                        if !description.is_empty() {
                            model
                                .as_mut()
                                .set_current_description(QString::from(description.as_str()));
                        }
                        model.as_mut().set_current_detail_tags(QString::from(
                            detail_tags_from_meta(&meta).as_str(),
                        ));
                        let cover_key = media_key_for(&model.entries[index as usize])
                            .map(MediaKey::with_current_cover_preference);
                        let type_keys = detail_image_keys_from_meta(
                            &meta,
                            meta.title.system.id.as_str(),
                            meta.path.as_str(),
                        );
                        if type_keys.is_empty() {
                            // No alternate images — just the cover; clear any
                            // stale pending carousel from a previous selection.
                            model.as_mut().rust_mut().pending_carousel_keys = None;
                            let detail_keys = cover_key.into_iter().collect();
                            set_detail_image_keys(model.as_mut(), detail_keys);
                        } else if MediaImageCache::current_cover_preference_type().is_none() {
                            // Auto preference: we need Core's resolved type_tag
                            // for index-0 to drop its twin from the carousel tail.
                            // Check the cache; if the cover is already warm the
                            // type is known and we can dedup now. If not, stash
                            // the candidate keys and let notify_cover_update finish
                            // once the cover lands.
                            let cache = global_media_image_cache();
                            let resolved = cover_key
                                .as_ref()
                                .and_then(|k| cache.resolved_image_type(k));
                            if resolved.is_some() || cover_key.is_none() {
                                // Cover already fetched — dedup immediately.
                                model.as_mut().rust_mut().pending_carousel_keys = None;
                                let detail_keys = ordered_detail_image_keys(
                                    cover_key,
                                    type_keys,
                                    resolved.as_deref(),
                                );
                                set_detail_image_keys(model.as_mut(), detail_keys);
                            } else {
                                // Cover still in-flight. Publish a single-image
                                // carousel now (no arrows) and stash the candidates
                                // so notify_cover_update can finalize once the type
                                // is known.
                                model.as_mut().rust_mut().pending_carousel_keys = Some(type_keys);
                                let detail_keys = cover_key.into_iter().collect();
                                set_detail_image_keys(model.as_mut(), detail_keys);
                            }
                        } else {
                            // Explicit preference — existing dedup path.
                            model.as_mut().rust_mut().pending_carousel_keys = None;
                            let detail_keys = ordered_detail_image_keys(cover_key, type_keys, None);
                            set_detail_image_keys(model.as_mut(), detail_keys);
                        }
                    }
                    Err(e) => warn!("games detail fetch failed: {}", e.message),
                }
                model.as_mut().set_current_detail_loading(false);
            });
        });
    }

    fn clear_current_detail(mut self: Pin<&mut Self>) {
        self.as_mut()
            .rust_mut()
            .description_seq
            .fetch_add(1, Ordering::SeqCst);
        self.as_mut().set_current_detail_loading(false);
        self.as_mut().set_current_description(QString::default());
        self.as_mut().set_current_detail_tags(QString::default());
        clear_detail_images(self);
    }

    fn cycle_detail_image(self: Pin<&mut Self>, delta: i32) {
        if delta == 0 || self.current_detail_image_count <= 1 {
            return;
        }
        let current = self.current_detail_image_index;
        let next = (current + delta).clamp(0, self.current_detail_image_count - 1);
        if next == current {
            return;
        }
        set_detail_image_index(self, next);
    }

    fn path_at(&self, index: i32) -> QString {
        if index < 0 || index >= self.count {
            return QString::default();
        }
        QString::from(self.entries[index as usize].path.as_str())
    }

    fn system_id_at(&self, index: i32) -> QString {
        if index < 0 || index >= self.count {
            return QString::default();
        }
        QString::from(entry_system_id(&self.entries[index as usize]).as_str())
    }

    fn entry_type_at(&self, index: i32) -> QString {
        if index < 0 || index >= self.count {
            return QString::default();
        }
        QString::from(self.entries[index as usize].entry_type.as_str())
    }

    fn is_media_capable_at(&self, index: i32) -> bool {
        if index < 0 || index >= self.count {
            return false;
        }
        is_media_capable_entry(&self.entries[index as usize])
    }

    fn index_for_game_path(&self, path: &QString) -> i32 {
        position_of_game_path(&self.entries, &path.to_string())
    }

    /// Spin up the long-lived cover-cache subscriber on first use.
    /// Subsequent calls are no-ops — the model singleton owns exactly
    /// one subscriber for the whole process lifetime, decoupled from
    /// `seq` because cover updates are not tied to a particular browse
    /// path. Lagged broadcast frames are dropped silently; the
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

    /// Issue a fresh `media.browse` for `(path, systems)`. Bumps `seq`,
    /// aborts the prior watcher, clears entries via `beginResetModel`,
    /// subscribes to `MediaBrowseEndpoint`, and spawns a watcher whose
    /// queued callbacks bail unless the ticket still matches.
    ///
    /// `eligible_for_auto_nav` is set when this load came from
    /// `set_system`. The single-root auto-nav case in
    /// `apply_initial_page` consumes the flag to decide whether to skip
    /// rendering the 1-item roots list and dive straight into that
    /// root.
    fn start_initial_browse(
        mut self: Pin<&mut Self>,
        path: String,
        systems: Vec<String>,
        eligible_for_auto_nav: bool,
    ) {
        info!(
            ?path,
            ?systems,
            eligible_for_auto_nav,
            page_size = self.page_size,
            "games: start_initial_browse",
        );
        self.as_mut().rust_mut().nav_timing = Some(NavTiming::new("network"));
        self.as_mut().ensure_cover_subscription();
        self.as_mut().set_current_path(QString::from(path.as_str()));
        self.as_mut().set_loading(true);
        self.as_mut().set_error_message(QString::default());
        // A scope change supersedes any random walk in flight via the browse
        // ticket, and that abort deliberately writes nothing, so its flags
        // have to be released here or they stay set for the session.
        if self.random_seeking {
            self.as_mut().set_random_seeking(false);
        }
        if !self.random_error.is_empty() {
            self.as_mut().set_random_error(QString::default());
        }
        self.as_mut().set_current_detail_loading(false);
        self.as_mut().set_current_description(QString::default());
        self.as_mut().set_current_detail_tags(QString::default());
        // Stale adjacent preload keys from the prior path must not survive
        // into the new browse target. `clear_detail_images` isn't called
        // from this path, so clear explicitly here.
        clear_adjacent_cover_prefetch(self.as_mut());
        self.as_mut()
            .rust()
            .description_seq
            .fetch_add(1, Ordering::SeqCst);
        self.as_mut().set_has_next_page(false);
        self.as_mut().set_loading_more(false);
        // Clear any cover pause held by a jump fetch that's being abandoned by
        // this scope swap. The in-flight jump response is ticket-rejected below
        // (the `seq` bump), so it never reaches `apply_append_page` to resume
        // covers itself; without this the new folder would render with covers
        // stuck paused.
        self.as_mut().set_cover_requests_paused(false);
        self.as_mut().rust_mut().auto_nav_eligible = eligible_for_auto_nav;
        // Cleared on every browse target swap: a new path needs the
        // full `apply_initial_page` reset to install fresh entries.
        // The flag is set back to true at the end of that function
        // so subsequent refetches (cache invalidations) skip the
        // reset and preserve appended pages + selection.
        self.as_mut().rust_mut().is_seeded = false;
        self.as_mut().rust_mut().next_cursor = None;
        // Drop any held initial-look-ahead gate from the prior browse —
        // its append (if it lands at all) will be ticket-rejected and
        // can't re-arm the gate for this new target.
        self.as_mut().rust_mut().pending_initial_lookahead = false;
        // Invalidate any in-flight sub-batch posts from the prior
        // browse: each posted closure compares against the snapshotted
        // ticket and bails if the model has moved on. Otherwise a
        // batch from the old chain could splice rows onto the new
        // dataset.
        self.as_mut()
            .rust_mut()
            .append_seq
            .fetch_add(1, Ordering::SeqCst);
        if !self.entries.is_empty() {
            self.as_mut().begin_reset_model();
            self.as_mut().rust_mut().entries.clear();
            self.as_mut().rust_mut().disambig_displays.clear();
            self.as_mut().rust_mut().count = 0;
            self.as_mut().end_reset_model();
            self.as_mut().count_changed();
        }
        // Total-files counter resets too — the previous path's
        // denominator would be misleading until the new fetch lands.
        self.as_mut().set_total_files(0);
        // Total-dirs counter is the other denominator term; reset it for
        // the same reason.
        self.as_mut().set_total_dirs(0);

        if let Some(handle) = self.as_mut().rust_mut().watcher.take() {
            handle.abort();
        }
        // Tear down any cover gate left from the prior path. See
        // `reset_cover_gate` for the rationale; without this teardown
        // a stale timer callback could fire after the new path's
        // `set_loading(true)` and prematurely release its gate.
        reset_cover_gate(self.as_mut());

        let seq = self.rust().seq.clone();
        let ticket = seq.fetch_add(1, Ordering::SeqCst) + 1;

        // MiSTer fast path: list the folder straight from Core's media
        // database (`media_browse_db`) and hand the result to the same
        // Ready pipeline the RPC feeds. A full local listing arrives in
        // one piece with no further pages, so the paged fetch machinery
        // (background fill, tail prefetch, edge stalls) stays idle for
        // folders served this way. Root browses (empty path) always use
        // the RPC — their entries come from Core's launcher routes,
        // which do not live in the database. Any `None` (cache not
        // serveable, unknown path, query error) falls through to the
        // exact RPC subscribe this method always performed.
        if !path.is_empty() && crate::media_browse_db::enabled() {
            // Mirror the RPC path's Pending status so the query window
            // shows the loading cue instead of an empty view: the RPC
            // flow gets this from the store's status stream, the direct
            // flow must say it itself.
            apply_status(self.as_mut(), ResourceStatus::Loading);
            let qt_thread = self.qt_thread();
            let seq_direct = seq.clone();
            let path_direct = path.clone();
            let systems_direct = systems.clone();
            global_handle().spawn(async move {
                let query_path = path_direct.clone();
                let query_systems = systems_direct.clone();
                let cancel_seq = seq_direct.clone();
                let direct = tokio::task::spawn_blocking(move || {
                    crate::media_browse_db::browse_folder(&query_path, &query_systems, &|| {
                        cancel_seq.load(Ordering::SeqCst) != ticket
                    })
                })
                .await
                .ok()
                .flatten();
                let qt_thread_tail = qt_thread.clone();
                let _ = qt_thread.queue(move |mut model| {
                    if seq_direct.load(Ordering::SeqCst) != ticket {
                        return;
                    }
                    match direct {
                        Some(result) => {
                            mark_nav_source(model.as_mut(), "direct");
                            apply_direct_listing(
                                model,
                                result,
                                ticket,
                                seq_direct,
                                &qt_thread_tail,
                            );
                        }
                        None => subscribe_browse_endpoint(
                            model,
                            path_direct,
                            systems_direct,
                            ticket,
                            seq_direct,
                        ),
                    }
                });
            });
            return;
        }
        subscribe_browse_endpoint(self, path, systems, ticket, seq);
    }
}

/// Hand a complete direct listing to the model: paint first, bulk the
/// rest. A model reset costs the UI thread roughly linear time in row
/// count (measured on device at a few milliseconds per row, so a
/// two-thousand-row folder froze entry for several seconds), while
/// inserts beyond the viewport stay cheap because tile delegates are
/// retention-gated. The reset therefore carries only the first page,
/// and the remainder lands as bulk inserts queued behind the paint.
/// During the gap the model reports `has_next_page = true` (synthetic
/// pagination, no cursor) with `loading_more` held true, so the
/// saved-selection chase keeps waiting and every fetch path no-ops
/// instead of firing an RPC; the last chunk flips both off, and the
/// resulting `onCountChanged` re-runs the restore against the full
/// listing.
fn apply_direct_listing(
    mut model: Pin<&mut ffi::GamesModel>,
    mut result: MediaBrowseResult,
    ticket: u64,
    seq: Arc<AtomicU64>,
    qt_thread: &cxx_qt::CxxQtThread<ffi::GamesModel>,
) {
    let head_len = usize::try_from(model.page_size.max(1)).unwrap_or(30);
    if result.entries.len() <= head_len.saturating_mul(2) {
        apply_status(model, ResourceStatus::Ready(result));
        return;
    }
    let tail = result.entries.split_off(head_len);
    result.pagination = Some(Pagination {
        has_next_page: true,
        page_size: 0,
        next_cursor: None,
    });
    apply_status(model.as_mut(), ResourceStatus::Ready(result));
    // Same Qt-thread call as the head apply: QML cannot observe the gap
    // between the reset and this flag, so no fetch can slip in between.
    model.as_mut().set_loading_more(true);
    queue_direct_tail(qt_thread, tail, ticket, seq);
}

/// One bulk insert per event-loop turn keeps the paint responsive
/// between chunks; the jump path uses the same size for the same
/// reason. Stale chunks self-disarm on the browse ticket: a newer
/// `start_initial_browse` owns the model and resets its own flags.
const DIRECT_TAIL_CHUNK: usize = 1000;

fn queue_direct_tail(
    qt_thread: &cxx_qt::CxxQtThread<ffi::GamesModel>,
    mut tail: Vec<BrowseEntry>,
    ticket: u64,
    seq: Arc<AtomicU64>,
) {
    let rest = if tail.len() > DIRECT_TAIL_CHUNK {
        tail.split_off(DIRECT_TAIL_CHUNK)
    } else {
        Vec::new()
    };
    let qt_thread_next = qt_thread.clone();
    let _ = qt_thread.queue(move |mut model| {
        if seq.load(Ordering::SeqCst) != ticket {
            return;
        }
        let started = Instant::now();
        let entries = transform_entries(tail, platform::current().as_ref());
        insert_sub_batch(model.as_mut(), entries);
        debug!(
            insert_ms = started.elapsed().as_millis(),
            remaining = rest.len(),
            "games: direct tail chunk",
        );
        if rest.is_empty() {
            model.as_mut().set_loading_more(false);
            model.as_mut().set_has_next_page(false);
            return;
        }
        queue_direct_tail(&qt_thread_next, rest, ticket, seq);
    });
}

/// Subscribe to the store-backed `media.browse` endpoint and wire its
/// status stream into `apply_status`. Split out of
/// `start_initial_browse` so the direct-database fast path can fall
/// back to the identical RPC flow from an async context.
fn subscribe_browse_endpoint(
    mut model: Pin<&mut ffi::GamesModel>,
    path: String,
    systems: Vec<String>,
    ticket: u64,
    seq: Arc<AtomicU64>,
) {
    let max_results = u32::try_from(model.page_size.max(1)).unwrap_or(u32::from(u16::MAX));
    let resource = global_store().subscribe::<MediaBrowseEndpoint>(BrowseArgs::new(
        path,
        systems,
        max_results,
    ));
    let mut status_rx = resource.subscribe();

    // Spawn the watcher BEFORE applying the sync seed. If
    // `apply_status` recurses into `start_initial_browse` (auto-nav
    // cascade on a single-folder result), the inner call's
    // `watcher.take().abort()` cleans up this handle correctly —
    // doing this in the other order leaks the inner watcher when
    // the outer call later overwrites `self.watcher`.
    let qt_thread = model.qt_thread();
    let snapshot = status_rx.borrow_and_update().clone();
    if let Some(timing) = model.as_mut().rust_mut().nav_timing.as_mut() {
        if matches!(snapshot, ResourceStatus::Ready(_)) {
            timing.set_source("cache");
            timing.mark_request_done();
        }
    }
    let seq_for_loop = seq;
    let handle = global_handle().spawn(async move {
        while status_rx.changed().await.is_ok() {
            let snapshot = status_rx.borrow_and_update().clone();
            let seq_for_closure = seq_for_loop.clone();
            let _ = qt_thread.queue(move |model| {
                if seq_for_closure.load(Ordering::SeqCst) != ticket {
                    return;
                }
                apply_status(model, snapshot);
            });
        }
    });
    model.as_mut().rust_mut().watcher = Some(handle);

    // Sync seed runs inline on the Qt thread, so it cannot race a
    // queued callback (Qt won't pump events until set_system /
    // set_path returns). No ticket check needed; the value we read
    // is whichever the resource has right now.
    apply_status(model, snapshot);
}

/// Resolve the system id role exposed to QML. Single-system entries
/// (media items, single-system roots/directories) populate `system_id`;
/// multi-system roots fall back to the first entry of `system_ids` so
/// the tile still has *a* logo to draw rather than a blank.
fn entry_system_id(entry: &BrowseEntry) -> String {
    if !entry.system_id.is_empty() {
        return entry.system_id.clone();
    }
    entry.system_ids.first().cloned().unwrap_or_default()
}

fn is_media_capable_entry(entry: &BrowseEntry) -> bool {
    entry.entry_type == "media"
        || (entry.entry_type == "directory"
            && (entry.media_id.is_some() || !entry.zap_script.is_empty()))
}

/// Everything one random walk needs. Grouped so the walk can live outside
/// `launch_random`, which the line limit (rightly) keeps short.
struct RandomWalk {
    qt_thread: cxx_qt::CxxQtThread<ffi::GamesModel>,
    random_seq: Arc<AtomicU64>,
    ticket: u64,
    browse_seq: Arc<AtomicU64>,
    browse_ticket: u64,
    path: String,
    systems: Vec<String>,
    leading_dirs: i32,
    target: usize,
}

/// Walk the listing counting only launchable rows until the drawn position,
/// then launch that entry. Aborts without writing anything when superseded.
fn spawn_random_walk(walk: RandomWalk) {
    let RandomWalk {
        qt_thread,
        random_seq,
        ticket,
        browse_seq,
        browse_ticket,
        path,
        systems,
        leading_dirs,
        target,
    } = walk;
    let store = global_store();
    global_handle().spawn(async move {
        let mut cursor: Option<String> = None;
        let mut remaining = target;
        let mut first_page = true;
        // Bounded so a pathological listing cannot loop forever; 64 pages of
        // up to 1000 rows covers any realistic folder.
        for _ in 0..64 {
            if random_seq.load(Ordering::SeqCst) != ticket
                || browse_seq.load(Ordering::SeqCst) != browse_ticket
            {
                return; // superseded: a newer press or scope owns the flags
            }
            let max_results = walk_page_size(remaining, if first_page { leading_dirs } else { 0 });
            let result = store
                .client()
                .media_browse(MediaBrowseParams {
                    path: path.clone(),
                    systems: systems.clone(),
                    max_results: Some(max_results),
                    cursor: cursor.clone(),
                    letter: None,
                    sort: None,
                })
                .await;
            first_page = false;
            let result = match result {
                Ok(result) => result,
                Err(e) => {
                    warn!("random game walk failed: {}", e.message);
                    finish_random(
                        &qt_thread,
                        &random_seq,
                        ticket,
                        &browse_seq,
                        browse_ticket,
                        Err("browse-failed"),
                    );
                    return;
                }
            };
            let page_launchable = launchable_count(&result.entries);
            if remaining < page_launchable {
                let text = launchable_entries(&result.entries)
                    .nth(remaining)
                    .and_then(run_text_for_entry);
                let outcome = text.ok_or("no-launch-payload");
                finish_random(
                    &qt_thread,
                    &random_seq,
                    ticket,
                    &browse_seq,
                    browse_ticket,
                    outcome,
                );
                return;
            }
            remaining -= page_launchable;
            match result.next_cursor() {
                // A cursor that does not advance would loop forever.
                Some(next) if result.has_next_page() && Some(&next) != cursor.as_ref() => {
                    cursor = Some(next);
                }
                _ => {
                    finish_random(
                        &qt_thread,
                        &random_seq,
                        ticket,
                        &browse_seq,
                        browse_ticket,
                        Err("walk-exhausted"),
                    );
                    return;
                }
            }
        }
        finish_random(
            &qt_thread,
            &random_seq,
            ticket,
            &browse_seq,
            browse_ticket,
            Err("walk-too-long"),
        );
    });
}

/// Fire the run mutation for a resolved random pick.
fn spawn_random_run(text: String) {
    let store = global_store();
    global_handle().spawn(async move {
        if let Err(e) = store.run_mutation::<RunMutation>(RunParams { text }).await {
            warn!("random game run failed: {}", e.message);
        }
    });
}

/// Settle a walk back on the Qt thread: launch the resolved entry, or record
/// why nothing could be launched. Writes nothing when superseded, because a
/// newer press then owns the flags.
fn finish_random(
    qt_thread: &cxx_qt::CxxQtThread<ffi::GamesModel>,
    random_seq: &Arc<AtomicU64>,
    ticket: u64,
    browse_seq: &Arc<AtomicU64>,
    browse_ticket: u64,
    outcome: Result<String, &'static str>,
) {
    let random_seq = random_seq.clone();
    let browse_seq = browse_seq.clone();
    let outcome = outcome.map_err(str::to_string);
    let _ = qt_thread.queue(move |mut model| {
        // Both tickets, not just the random one: navigating to another folder
        // bumps only the browse seq, and launching a game the user has
        // already browsed away from would be worse than not launching.
        if random_seq.load(Ordering::SeqCst) != ticket
            || browse_seq.load(Ordering::SeqCst) != browse_ticket
        {
            return;
        }
        model.as_mut().set_random_seeking(false);
        match outcome {
            Ok(text) => spawn_random_run(text),
            Err(reason) => {
                warn!("random game pick failed: {reason}");
                model
                    .as_mut()
                    .set_random_error(QString::from(reason.as_str()));
            }
        }
    });
}

/// The launchable rows of a page, i.e. everything that is not a container.
/// Directories are navigation, not games, so they never take part in a random
/// pick and never count toward a position.
fn launchable_entries(entries: &[BrowseEntry]) -> impl Iterator<Item = &BrowseEntry> {
    entries.iter().filter(|entry| !entry.is_folder())
}

/// Number of launchable rows in a page.
fn launchable_count(entries: &[BrowseEntry]) -> usize {
    launchable_entries(entries).count()
}

/// True when `entries` already holds the scope's complete listing, so a random
/// pick can be resolved locally with no request at all. Deliberately derived
/// from the loaded rows and the cursor state only: `total_files`/`total_dirs`
/// are Core-reported and can disagree with what actually arrived.
fn scope_fully_loaded(count: i32, has_next_page: bool, next_cursor: Option<&str>) -> bool {
    count > 0 && !has_next_page && next_cursor.is_none()
}

/// How many rows to request while walking to `remaining`-th launchable entry.
/// `leading_dirs` is a sizing hint for the first page only, where the listing
/// starts with the scope's directory block; being wrong only costs an extra
/// round trip, never a wrong pick, so it is never used as an index.
fn walk_page_size(remaining: usize, leading_dirs: i32) -> u32 {
    const MAX_BROWSE_PAGE: usize = 1000;
    let hint = usize::try_from(leading_dirs.max(0)).unwrap_or(0);
    let want = remaining.saturating_add(1).saturating_add(hint);
    u32::try_from(want.min(MAX_BROWSE_PAGE)).unwrap_or(1000)
}

/// Bucket key for a browse display name, byte-exact with Core's
/// `BrowseNameFirstChar`: the uppercased first byte when it is a latin
/// letter, `"0-9"` for a digit, `"#"` for everything else — including
/// empty names and multi-byte first characters, whose first byte is not
/// ASCII, exactly as Core's byte-indexed fold treats them.
fn browse_bucket_key(name: &str) -> String {
    match name.as_bytes().first() {
        Some(b) => {
            let up = b.to_ascii_uppercase();
            if up.is_ascii_uppercase() {
                (up as char).to_string()
            } else if up.is_ascii_digit() {
                "0-9".to_string()
            } else {
                "#".to_string()
            }
        }
        None => "#".to_string(),
    }
}

/// The jump-to-letter facet computed from a complete local listing: the
/// same buckets Core's `media.browse.index` derives, over the rows the
/// user is actually looking at. Buckets appear in first-appearance
/// (listing) order; `count` folds every row of a bucket wherever it
/// appears, and `offset` is the bucket's first item position among
/// files (0-based, directories excluded), the exact contract of the RPC
/// facet. `cursor` stays empty because the router jumps by offset and
/// never seeks by cursor.
fn local_letter_index(entries: &[BrowseEntry]) -> MediaBrowseIndexResult {
    let mut groups: Vec<BrowseIndexGroup> = Vec::new();
    let mut file_pos: u32 = 0;
    for entry in launchable_entries(entries) {
        let key = browse_bucket_key(&entry.name);
        if let Some(group) = groups.iter_mut().find(|g| g.key == key) {
            group.count += 1;
        } else {
            groups.push(BrowseIndexGroup {
                key: key.clone(),
                label: key,
                count: 1,
                cursor: String::new(),
                offset: file_pos,
            });
        }
        file_pos = file_pos.saturating_add(1);
    }
    MediaBrowseIndexResult {
        scheme: "latin".to_string(),
        total_files: file_pos,
        groups,
    }
}

fn run_text_for_entry(entry: &BrowseEntry) -> Option<String> {
    if media_capable_directory_needs_child_resolution(entry) {
        return non_empty_text(&entry.zap_script);
    }
    path_then_zap_script_for_entry(entry)
}

fn path_then_zap_script_for_entry(entry: &BrowseEntry) -> Option<String> {
    non_empty_text(&entry.path).or_else(|| non_empty_text(&entry.zap_script))
}

fn non_empty_text(text: &str) -> Option<String> {
    (!text.trim().is_empty()).then(|| text.to_string())
}

async fn resolve_media_capable_directory_run_text(
    client: &zaparoo_core::client::Client,
    entry: &BrowseEntry,
    meta_params: Option<MediaMetaParams>,
    browse_params: Option<MediaBrowseParams>,
    fallback_text: Option<String>,
) -> Option<String> {
    if let Some(params) = meta_params {
        let resolved = match crate::media_meta_db::media_meta_async(params.clone()).await {
            Some(media) => Ok(zaparoo_core::media_types::MediaMetaResult { media }),
            None => client.media_meta(params).await,
        };
        match resolved {
            Ok(result) if !result.media.path.trim().is_empty() => return Some(result.media.path),
            Ok(_) => warn!(
                "singleton launch path resolve returned empty path for {}; trying folder browse",
                entry.name
            ),
            Err(e) => warn!(
                "singleton launch path resolve failed for {}: {}",
                entry.name, e.message
            ),
        }
    }

    if let Some(params) = browse_params {
        match client.media_browse(params).await {
            Ok(result) => {
                if let Some(text) = child_launch_text_from_browse_result(entry, &result) {
                    return Some(text);
                }
                warn!(
                    "media-capable directory browse found no launchable child for {}",
                    entry.name
                );
            }
            Err(e) => warn!(
                "media-capable directory browse failed for {}: {}",
                entry.name, e.message
            ),
        }
    }

    fallback_text
}

fn child_launch_text_from_browse_result(
    parent: &BrowseEntry,
    result: &MediaBrowseResult,
) -> Option<String> {
    let children = result
        .entries
        .iter()
        .filter(|entry| !entry.is_folder())
        .collect::<Vec<_>>();
    if let Some(media_id) = parent.media_id {
        let matching_media_id = children
            .iter()
            .copied()
            .filter(|entry| entry.media_id == Some(media_id))
            .collect::<Vec<_>>();
        if let Some(text) = best_launch_text_for_entries(&matching_media_id) {
            return Some(text);
        }
    }
    best_launch_text_for_entries(&children)
}

fn best_launch_text_for_entries(entries: &[&BrowseEntry]) -> Option<String> {
    entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| {
            non_empty_text(&entry.path).map(|path| (launch_path_priority(&path), index, path))
        })
        .min_by_key(|(priority, index, _)| (*priority, *index))
        .map(|(_, _, path)| path)
        .or_else(|| {
            entries
                .iter()
                .find_map(|entry| non_empty_text(&entry.zap_script))
        })
}

fn launch_path_priority(path: &str) -> u8 {
    let Some((_, extension)) = path.rsplit_once('.') else {
        return 100;
    };
    if extension.eq_ignore_ascii_case("m3u") {
        0
    } else if extension.eq_ignore_ascii_case("cue") {
        1
    } else if extension.eq_ignore_ascii_case("gdi") {
        2
    } else if extension.eq_ignore_ascii_case("chd") {
        3
    } else {
        100
    }
}

fn media_capable_directory_browse_params(entry: &BrowseEntry) -> Option<MediaBrowseParams> {
    if !media_capable_directory_needs_child_resolution(entry) || entry.path.trim().is_empty() {
        return None;
    }
    let system_id = entry_system_id(entry);
    let systems = if system_id.trim().is_empty() {
        Vec::new()
    } else {
        vec![system_id]
    };
    Some(MediaBrowseParams {
        path: entry.path.clone(),
        systems,
        max_results: Some(COLLAPSED_DIRECTORY_BROWSE_PAGE_SIZE),
        ..MediaBrowseParams::default()
    })
}

fn meta_params_for_entry(entry: &BrowseEntry) -> Option<MediaMetaParams> {
    if let Some(media_id) = entry.media_id {
        return Some(MediaMetaParams::for_media_id(media_id));
    }
    let system_id = entry_system_id(entry);
    if system_id.trim().is_empty() || entry.path.trim().is_empty() {
        return None;
    }
    Some(MediaMetaParams::for_media(system_id, entry.path.clone()))
}

fn singleton_directory_needs_launch_resolution(entry: &BrowseEntry) -> bool {
    entry.entry_type == "directory" && entry.media_id.is_some()
}

fn media_capable_directory_needs_child_resolution(entry: &BrowseEntry) -> bool {
    entry.entry_type == "directory"
        && (entry.media_id.is_some() || !entry.zap_script.trim().is_empty())
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

fn detail_tags_from_meta(meta: &MediaMeta) -> String {
    let source = if meta.title.tags.is_empty() {
        meta.tags.as_slice()
    } else {
        meta.title.tags.as_slice()
    };
    detail_tags_from_tags(source)
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
        // Filter to the types Core can actually serve so a left/right cycle
        // never lands on a key that will always return "no image".
        .filter(|t| CORE_SERVEABLE_IMAGE_TYPES.contains(&t.as_str()))
        .map(|image_type| MediaKey::with_image_type(system, path, image_type))
        .collect()
}

/// Build the ordered key list for the detail image carousel: the cover-
/// preference key at index 0 (so it reuses the prefetch-warmed cache entry)
/// followed by the specific image-type keys from `media_meta` for cycling.
///
/// For explicit preferences (e.g. "boxart"), drops the matching concrete key
/// so the same image doesn't appear twice when the user presses left/right.
///
/// For the "auto" preference (`current_cover_preference_type() == None`),
/// `resolved_type` is Core's reported `type_tag` for the index-0 cover (the
/// concrete type Core actually served). Pass `None` when it isn't known yet;
/// the caller will call again once the cover fetch completes.
fn ordered_detail_image_keys(
    cover_key: Option<MediaKey>,
    type_keys: Vec<MediaKey>,
    resolved_type: Option<&str>,
) -> Vec<MediaKey> {
    let Some(cover) = cover_key else {
        return type_keys;
    };
    // For explicit preferences use the locally-known pref string.
    // For "auto" use Core's resolved type when available.
    let explicit_pref = MediaImageCache::current_cover_preference_type();
    let drop_type: Option<&str> = if let Some(ref p) = explicit_pref {
        Some(p.as_str())
    } else {
        resolved_type
    };
    let mut keys = Vec::with_capacity(1 + type_keys.len());
    keys.push(cover);
    for k in type_keys {
        if let Some(drop) = drop_type {
            if k.image_type.as_deref() == Some(drop) {
                continue;
            }
        }
        keys.push(k);
    }
    keys
}

fn portable_text_for_entry(entry: &BrowseEntry) -> String {
    if !entry.zap_script.trim().is_empty() {
        return entry.zap_script.clone();
    }
    entry.path.clone()
}

fn detail_tags_from_entry(entry: &BrowseEntry) -> String {
    detail_tags_from_tags(entry.tags.as_slice())
}

fn detail_tags_from_tags(source: &[TagInfo]) -> String {
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

fn display_title_for_entry(entry: &BrowseEntry) -> std::borrow::Cow<'_, str> {
    if entry.name.is_empty() {
        std::borrow::Cow::Owned(file_stem_or_name(&entry.path, &entry.name))
    } else {
        std::borrow::Cow::Borrowed(entry.name.as_str())
    }
}

/// Resolve the user-visible name for an entry, honoring the
/// `show_original_filenames` setting. When enabled, the original filename
/// (without extension) is preferred over Core's cleaned display name.
fn display_name_for_entry(
    entry: &BrowseEntry,
    show_original_filenames: bool,
) -> std::borrow::Cow<'_, str> {
    if show_original_filenames {
        std::borrow::Cow::Owned(file_stem_or_name(&entry.path, &entry.name))
    } else {
        display_title_for_entry(entry)
    }
}

/// Build the per-row sibling-diffed disambiguation display strings for the full
/// `entries` slice. Grouping keys off the displayed name so the
/// original-filename toggle (distinct names) naturally disables grouping.
fn compute_disambig_displays(
    entries: &[BrowseEntry],
    show_original_filenames: bool,
) -> Vec<String> {
    let rows: Vec<(String, Vec<String>)> = entries
        .iter()
        .map(|e| {
            (
                display_name_for_entry(e, show_original_filenames).into_owned(),
                disambiguating_tag_labels(&e.disambiguating_tags),
            )
        })
        .collect();
    sibling_disambiguation_displays(&rows)
}

/// After appending rows starting at `insert_first`, recompute the disambiguation
/// displays for the boundary group (which may have grown) plus everything after
/// it, and splice the result onto `disambig_displays`. Cheaper than recomputing
/// the whole vec on every page append, and correct because sibling groups are
/// local runs of equal name. Returns the boundary group's start index so the
/// caller can re-emit `dataChanged` for the pre-existing rows it touched.
fn recompute_disambig_tail(mut model: Pin<&mut ffi::GamesModel>, insert_first: usize) -> usize {
    let show_original = model.show_original_filenames;
    let (group_start, new_tail) = {
        let entries = &model.entries;
        let total = entries.len();
        let mut group_start = insert_first.min(total);
        if group_start > 0 {
            let boundary =
                display_name_for_entry(&entries[group_start - 1], show_original).into_owned();
            while group_start > 0
                && display_name_for_entry(&entries[group_start - 1], show_original).as_ref()
                    == boundary.as_str()
            {
                group_start -= 1;
            }
        }
        let rows: Vec<(String, Vec<String>)> = entries[group_start..total]
            .iter()
            .map(|e| {
                (
                    display_name_for_entry(e, show_original).into_owned(),
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

fn clear_detail_images(mut model: Pin<&mut ffi::GamesModel>) {
    model.as_mut().rust_mut().detail_image_keys.clear();
    model.as_mut().rust_mut().pending_carousel_keys = None;
    model
        .as_mut()
        .set_current_detail_image_key(QString::default());
    model.as_mut().set_current_detail_image_index(0);
    model.as_mut().set_current_detail_image_count(0);
    model.as_mut().set_current_detail_image_can_prev(false);
    model.as_mut().set_current_detail_image_can_next(false);
    clear_adjacent_cover_prefetch(model);
}

/// Clear the adjacent-cover preload keys and mark no selection active.
/// Called from `clear_detail_images` (covers all its callers) and
/// `start_initial_browse` (which resets state without going through
/// `clear_detail_images`).
fn clear_adjacent_cover_prefetch(mut model: Pin<&mut ffi::GamesModel>) {
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

/// Recompute and push the adjacent-cover preload keys for the
/// `detail_prefetch_row` that `load_description_at` last settled on.
/// Only promotes a key to `media-image/...` when its bytes are already in
/// the cache; placeholder keys (`icons/*`) are passed through unchanged so
/// the QML-side guard (`k.startsWith("media-image/")`) keeps the hidden
/// Image source empty and does no decode work.
///
/// Called:
///   - by `load_description_at` whenever the selection settles.
///   - by `notify_cover_update` so that a neighbor's bytes landing
///     during dwell immediately warms Qt's pixmap cache.
///   - by `apply_initial_page` to start the warm-up from row 0 as soon
///     as the page is interactive.
fn refresh_adjacent_cover_prefetch(mut model: Pin<&mut ffi::GamesModel>) {
    let Some(row) = model.rust().detail_prefetch_row else {
        // No active selection — ensure both keys are empty.
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
    let page_size = u32::try_from(model.page_size.max(1)).unwrap_or(1);
    let requests_enabled = !model.cover_requests_paused;

    let next_key = if row + 1 < count {
        cover_key_for(
            &model.entries[(row + 1) as usize],
            page_size,
            requests_enabled,
        )
    } else {
        String::new()
    };
    let prev_key = if row > 0 {
        cover_key_for(
            &model.entries[(row - 1) as usize],
            page_size,
            requests_enabled,
        )
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

fn set_detail_image_keys(mut model: Pin<&mut ffi::GamesModel>, keys: Vec<MediaKey>) {
    // Bake the detail-pane size into every detail image so it fetches its own
    // (larger) resolution rather than sharing the grid's cover entry. `0`
    // clears sizing, preserving the shared-key instant-paint behavior.
    let detail_size = u32::try_from(model.detail_cover_max_size.max(0)).unwrap_or(0);
    let keys = keys
        .into_iter()
        .map(|k| k.with_max_size(detail_size))
        .collect();
    model.as_mut().rust_mut().detail_image_keys = keys;
    model.as_mut().set_current_detail_image_index(0);
    let count = i32::try_from(model.detail_image_keys.len()).unwrap_or(i32::MAX);
    model.as_mut().set_current_detail_image_count(count);
    model.as_mut().set_current_detail_image_can_prev(false);
    model.as_mut().set_current_detail_image_can_next(count > 1);
    sync_current_detail_image_key_with_page_size(model, 1);
}

fn set_single_detail_image_key(mut model: Pin<&mut ffi::GamesModel>, key: Option<MediaKey>) {
    // Discard pending carousel candidates from the previous row so they
    // cannot expand the carousel on a row whose meta_params returned None
    // (and therefore never clears pending_carousel_keys itself).
    model.as_mut().rust_mut().pending_carousel_keys = None;
    set_detail_image_keys(model, key.into_iter().collect());
}

fn set_detail_image_index(mut model: Pin<&mut ffi::GamesModel>, index: i32) {
    let count = i32::try_from(model.detail_image_keys.len()).unwrap_or(i32::MAX);
    let clamped = if count <= 0 {
        0
    } else {
        index.clamp(0, count - 1)
    };
    model.as_mut().set_current_detail_image_index(clamped);
    model.as_mut().set_current_detail_image_count(count);
    model
        .as_mut()
        .set_current_detail_image_can_prev(clamped > 0);
    model
        .as_mut()
        .set_current_detail_image_can_next(count > 0 && clamped < count - 1);
    sync_current_detail_image_key(model);
}

fn sync_current_detail_image_key(model: Pin<&mut ffi::GamesModel>) {
    let page_size = u32::try_from(model.page_size.max(1)).unwrap_or(1);
    sync_current_detail_image_key_with_page_size(model, page_size);
}

fn sync_current_detail_image_key_with_page_size(
    mut model: Pin<&mut ffi::GamesModel>,
    page_size: u32,
) {
    let index = model.current_detail_image_index;
    if index < 0 {
        model
            .as_mut()
            .set_current_detail_image_key(QString::default());
        return;
    }
    let Some(key) = model.detail_image_keys.get(index as usize).cloned() else {
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
        cache.enqueue_with_media_id(key, None, page_size.max(1));
        model
            .as_mut()
            .set_current_detail_image_key(QString::from("icons/Loading"));
    }
}

fn cover_placeholder_for(entry: &BrowseEntry) -> String {
    if !is_media_capable_entry(entry) && entry.is_folder() {
        "icons/Folder".to_string()
    } else {
        "icons/File".to_string()
    }
}

fn cover_key_for(entry: &BrowseEntry, page_size: u32, requests_enabled: bool) -> String {
    if !is_media_capable_entry(entry) && entry.is_folder() {
        return "icons/Folder".to_string();
    }
    let media_key = media_key_for(entry).map(MediaKey::with_current_cover_preference);
    let cache = global_media_image_cache();
    let cached = media_key.as_ref().is_some_and(|k| cache.is_cached(k));
    let negative = media_key.as_ref().is_some_and(|k| cache.is_negative(k));
    let soft_no_image = media_key
        .as_ref()
        .is_some_and(|k| cache.is_soft_no_image(k));
    // Browse-provided signal: Core confirmed no image property for this entry.
    // Treat as negative — show the cover placeholder and skip the
    // media.image request. This eliminates the per-entry lookup flood on
    // systems like Arcade that have very few scraped covers.
    let no_cover = !entry.has_cover;
    let effective_cached = cached && !no_cover;
    if requests_enabled && !effective_cached && !negative && !soft_no_image && !no_cover {
        // Miss-driven re-enqueue: when QML asks for the cover URL of
        // a media entry whose bytes aren't in the cache, kick a fetch
        // right here. This is the only implicit path covers reach the
        // queue — `apply_initial_page` / `apply_append_page` no longer
        // bulk-enqueue, so a tile is enqueued exactly when the grid
        // materialises its delegate. Combined with `prefetch_around`
        // for the next-page warm, the queue contents always reflect
        // the user's current view rather than whichever metadata
        // chunk Core most recently sent back.
        if let Some(k) = media_key.as_ref() {
            cache.enqueue_with_media_id(k.clone(), entry.media_id, page_size);
        }
    }
    cover_key_for_with(
        entry,
        media_key.as_ref(),
        effective_cached,
        negative || soft_no_image || no_cover || !requests_enabled,
    )
}

fn emit_cover_key_range(mut model: Pin<&mut ffi::GamesModel>, first_row: i32, count: i32) {
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

/// Build the canonical `(systemId, path)` identifier for a media
/// entry. Returns `None` for entries the cache cannot key on (folder
/// roots, unattributed entries, browse roots without a path).
fn media_key_for(entry: &BrowseEntry) -> Option<MediaKey> {
    if !is_media_capable_entry(entry) || entry.path.is_empty() {
        return None;
    }
    let system_id = entry_system_id(entry);
    if system_id.is_empty() {
        return None;
    }
    match entry.media_id {
        Some(media_id) => Some(MediaKey::with_media_id(
            system_id,
            entry.path.clone(),
            media_id,
        )),
        None => Some(MediaKey::new(system_id, entry.path.clone())),
    }
}

/// Pure ordering helper for `prefetch_around`. Returns
/// (`MediaKey`, `media_id`) pairs in desired fetch order: current page
/// top-to-bottom, then configured next pages, then configured previous
/// pages. Folders and entries without a `media_key` are skipped.
fn prefetch_around_plan(
    entries: &[BrowseEntry],
    count: i32,
    page_size: i32,
    first_visible_row: i32,
) -> Vec<(MediaKey, Option<i64>)> {
    if count <= 0 {
        return Vec::new();
    }
    let page_size = page_size.max(1);
    let first = first_visible_row.clamp(0, count.saturating_sub(1));
    let current_end = first.saturating_add(page_size).min(count);
    let next_end = current_end
        .saturating_add(page_size.saturating_mul(COVER_PREFETCH_NEXT_PAGES))
        .min(count);
    let previous_start =
        first.saturating_sub(page_size.saturating_mul(COVER_PREFETCH_PREVIOUS_PAGES));
    let mut plan: Vec<(MediaKey, Option<i64>)> =
        Vec::with_capacity(((next_end - previous_start) as usize).min(entries.len()));
    let push_range = |range: std::ops::Range<i32>, plan: &mut Vec<(MediaKey, Option<i64>)>| {
        for row in range {
            let idx = row as usize;
            if idx >= entries.len() {
                continue;
            }
            let entry = &entries[idx];
            // Skip entries Core has confirmed have no cover; they would
            // occupy fetch queue slots but always return "no image".
            if !entry.has_cover {
                continue;
            }
            if let Some(key) = media_key_for(entry) {
                plan.push((key.with_current_cover_preference(), entry.media_id));
            }
        }
    };
    push_range(first..current_end, &mut plan);
    push_range(current_end..next_end, &mut plan);
    push_range(previous_start..first, &mut plan);
    plan
}

/// Pure ordering helper for `prefetch_cursor_window`. Returns
/// (`MediaKey`, `media_id`) pairs in desired fetch order: focused row first,
/// then forward neighbors up to `COVER_PREFETCH_CURSOR_NEXT`, then reverse
/// neighbors back to `COVER_PREFETCH_CURSOR_PREV`. Folders and entries
/// without a cover or `media_key` are skipped. Split out so tests can drive
/// the ordering logic without the global cache or tokio runtime.
fn prefetch_cursor_window_plan(
    entries: &[BrowseEntry],
    count: i32,
    index: i32,
) -> Vec<(MediaKey, Option<i64>)> {
    if count <= 0 {
        return Vec::new();
    }
    let index = index.clamp(0, count - 1);
    let fwd_end = (index + 1 + COVER_PREFETCH_CURSOR_NEXT).min(count);
    let back_start = (index - COVER_PREFETCH_CURSOR_PREV).max(0);
    let mut plan: Vec<(MediaKey, Option<i64>)> = Vec::new();
    for row in index..fwd_end {
        let entry = &entries[row as usize];
        if !entry.has_cover {
            continue;
        }
        if let Some(key) = media_key_for(entry) {
            plan.push((key.with_current_cover_preference(), entry.media_id));
        }
    }
    for row in (back_start..index).rev() {
        let entry = &entries[row as usize];
        if !entry.has_cover {
            continue;
        }
        if let Some(key) = media_key_for(entry) {
            plan.push((key.with_current_cover_preference(), entry.media_id));
        }
    }
    plan
}

/// Rebuild the cover queue around the cursor, using a narrow forward-biased
/// window instead of the full-page `prefetch_around`. Called on each settled
/// row in list-detail layout so that the current cover and its immediate
/// neighbors reach the front of the 2-worker fetch queue; a full-page
/// re-enqueue would flood the queue with ~120 covers and bury the next cover
/// several seconds deep.
fn prefetch_cursor_window(model: &ffi::GamesModel, index: i32) {
    let cache = global_media_image_cache();
    if model.cover_requests_paused {
        cache.clear_pending_requests();
        return;
    }
    let count = model.count;
    let page_size = model.page_size;
    let plan = prefetch_cursor_window_plan(&model.entries, count, index);
    let ps = u32::try_from(page_size.max(1)).unwrap_or(1);
    cache.replace_pending_requests_ordered(plan, ps);
}

fn has_favorite_tag(tags: &[TagInfo]) -> bool {
    tags.iter()
        .any(|tag| tag.tag_type == "user" && tag.tag == "favorite")
}

fn favorite_role_value(tags: &[TagInfo]) -> i32 {
    i32::from(has_favorite_tag(tags))
}

fn favorite_params_for_entry(entry: &BrowseEntry, add: bool) -> Option<MediaTagsUpdateParams> {
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

    let system = entry_system_id(entry);
    if system.is_empty() || entry.path.is_empty() {
        return None;
    }
    params.system = system;
    params.path.clone_from(&entry.path);
    Some(params)
}

fn apply_favorite_tags(
    mut model: Pin<&mut ffi::GamesModel>,
    index: i32,
    media_id: Option<i64>,
    system_id: &str,
    path: &str,
    tags: Vec<TagInfo>,
) {
    if index < 0 || index >= model.count {
        return;
    }
    let entry = &model.entries[index as usize];
    let same_entry = if media_id.is_some() {
        entry.media_id == media_id
    } else {
        entry_system_id(entry) == system_id && entry.path == path
    };
    if !same_entry {
        return;
    }
    model.as_mut().rust_mut().entries[index as usize].tags = tags;
    let mut roles = QList::<i32>::default();
    roles.append(FAVORITE_ROLE);
    let parent = QModelIndex::default();
    let idx = model.index(index, 0, &parent);
    model.as_mut().data_changed(&idx, &idx, &roles);
}

/// Pure helper for `cover_key_for`. Split out so tests can drive the
/// branches (folder, cached, uncached, negative-memoed, unattributed)
/// without spinning up the global cover cache and its tokio runtime.
///
/// `icons/Loading` is the in-flight cue: an entry that has a media key
/// but no cached bytes and no negative memo is one we're actively
/// fetching (or about to). Tile.qml's cover Image renders that
/// hourglass at full size until the cache update broadcast lands and
/// `dataChanged(COVER_KEY_ROLE)` flips this to either `media-image/...`
/// (success) or `icons/File` (negative memo).
fn cover_key_for_with(
    entry: &BrowseEntry,
    key: Option<&MediaKey>,
    cached: bool,
    unavailable: bool,
) -> String {
    if !is_media_capable_entry(entry) && entry.is_folder() {
        return "icons/Folder".to_string();
    }
    match key {
        Some(k) if cached => MediaImageCache::image_key_for(k),
        Some(_) if !unavailable => "icons/Loading".to_string(),
        _ => "icons/File".to_string(),
    }
}

fn finish_nav_timing(
    mut model: Pin<&mut ffi::GamesModel>,
    reason: &'static str,
    pending_remaining: usize,
) {
    if let Some(timing) = model.as_mut().rust_mut().nav_timing.take() {
        timing.log_release("games", reason, pending_remaining);
    }
}

fn mark_nav_source(mut model: Pin<&mut ffi::GamesModel>, source: &'static str) {
    if let Some(timing) = model.as_mut().rust_mut().nav_timing.as_mut() {
        timing.set_source(source);
    }
}

fn mark_nav_request_done(mut model: Pin<&mut ffi::GamesModel>) {
    if let Some(timing) = model.as_mut().rust_mut().nav_timing.as_mut() {
        timing.mark_request_done();
    }
}

/// Emit `dataChanged(coverKey)` for every row whose entry's
/// `(systemId, path)` matches `key`. Cheap walk of the current
/// `entries` vec — pages top out at a few hundred rows after look-
/// ahead, and the bridge runs only when the cover-cache fetch driver
/// delivers a result.
///
/// Also drains `pending_first_paint_keys`: each cover landing during
/// the gate's hold ticks the set down, and emptying the set releases
/// the gate so the screen-flip overlay clears.
fn notify_cover_update(mut model: Pin<&mut ffi::GamesModel>, key: &MediaKey) {
    let rows: Vec<i32> = model
        .entries
        .iter()
        .enumerate()
        .filter(|(_, e)| {
            key.is_cover_key()
                && is_media_capable_entry(e)
                && match (key.media_id, e.media_id) {
                    (Some(a), Some(b)) => a == b,
                    _ => e.path == *key.path && entry_system_id(e) == *key.system_id,
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
        .detail_image_keys
        .get(model.current_detail_image_index.max(0) as usize)
        .is_some_and(|current| current == key)
    {
        sync_current_detail_image_key(model.as_mut());
    }
    // If this is the current detail cover key (index 0) and we have
    // pending carousel candidates (deferred because the cover was in-flight
    // when meta arrived), finalize the carousel now that the type is known.
    let is_current_cover = model
        .detail_image_keys
        .first()
        .is_some_and(|first| first == key);
    if is_current_cover && model.rust().pending_carousel_keys.is_some() {
        let cache = global_media_image_cache();
        let resolved = cache.resolved_image_type(key);
        let type_keys = model
            .as_mut()
            .rust_mut()
            .pending_carousel_keys
            .take()
            .unwrap_or_default();
        let cover_key = model.detail_image_keys.first().cloned();
        let detail_keys = ordered_detail_image_keys(cover_key, type_keys, resolved.as_deref());
        set_detail_image_keys(model.as_mut(), detail_keys);
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
        if model.loading {
            info!("games: cover gate released after visible covers cached");
            model.as_mut().set_loading(false);
            finish_nav_timing(model.as_mut(), "covers-ready", 0);
            maybe_start_initial_lookahead(model.as_mut());
        }
    }
    // Re-check the adjacent preload keys: a neighbor's bytes may have
    // just landed, which can flip its key from `icons/Loading` to
    // `media-image/...` and trigger the hidden Image's decode while the
    // user is still dwelling on the current item.
    refresh_adjacent_cover_prefetch(model);
}

/// Compute the set of media keys on the current page whose covers we
/// must wait on before releasing the cover gate. Folders, unattributed
/// entries, already-cached keys, and negatively-memoised keys are all
/// excluded. Pure helper so the gate's binning logic is unit-testable
/// without spinning up the global cache + tokio runtime.
fn compute_unresolved_keys<F, G>(
    entries: &[BrowseEntry],
    is_cached: F,
    is_negative: G,
) -> HashSet<MediaKey>
where
    F: Fn(&MediaKey) -> bool,
    G: Fn(&MediaKey) -> bool,
{
    entries
        .iter()
        // Browse-provided signal: Core confirmed no image for this entry.
        // Exclude from the gate set — these entries will never resolve to
        // cached bytes, so waiting on them would always ride the timeout.
        .filter(|entry| entry.has_cover)
        .filter_map(|entry| media_key_for(entry).map(MediaKey::with_current_cover_preference))
        .filter(|k| !is_cached(k) && !is_negative(k))
        .collect()
}

/// Abort any in-flight cover-gate timer, drop the waiting-keys set,
/// and bump `cover_gate_seq` so a callback that already queued onto
/// the Qt thread before the abort took effect sees a stale ticket and
/// bails. Used on every browse status edge that doesn't go on to call
/// `arm_cover_gate` itself (Pending, Errored, and the
/// `start_initial_browse` reset).
fn reset_cover_gate(mut model: Pin<&mut ffi::GamesModel>) {
    if let Some(handle) = model.as_mut().rust_mut().cover_gate_timer.take() {
        handle.abort();
    }
    model.as_mut().rust_mut().pending_first_paint_keys.clear();
    model.rust().cover_gate_seq.fetch_add(1, Ordering::SeqCst);
}

/// Decide whether to hold `loading=true` until the page's covers are
/// cached, or release immediately. Called once per `apply_initial_page`.
///
/// - If every media entry is already cached or negatively-memoised
///   (folder-only page, or revisit), set loading=false right now —
///   there's nothing to wait on, the screen-flip overlay clears.
/// - Otherwise, store the unresolved set on the model, arm a short
///   safety timer, and leave loading=true. `notify_cover_update` will
///   drain the set as covers land; whichever happens first (set empties
///   or timer fires) releases the gate.
///
/// The timeout is the fall-through: if visible cover fetches are cold,
/// the user sees `Loading…` only briefly before the existing "list with
/// placeholders → covers pop in" behavior resumes.
fn arm_cover_gate(mut model: Pin<&mut ffi::GamesModel>) {
    if let Some(handle) = model.as_mut().rust_mut().cover_gate_timer.take() {
        handle.abort();
    }
    let cache = global_media_image_cache();
    // Scope the waiting set to the visible page only, not all loaded
    // entries. The prefetcher queues only ~3 pages' worth; computing
    // over all entries means the set can never drain on a large folder
    // (e.g. 411 PSX dirs) and the gate always rides the full timeout.
    // Using the visible page (page_size rows starting at visible_first_row)
    // lets the set drain as soon as the on-screen covers land.
    let page_size = model.page_size.max(1) as usize;
    let first = model.rust().visible_first_row.max(0) as usize;
    let window_end = (first + page_size).min(model.entries.len());
    let visible_entries = &model.entries[first..window_end];
    let cover_keys = visible_entries
        .iter()
        .filter(|entry| entry.has_cover)
        .filter_map(|entry| media_key_for(entry).map(MediaKey::with_current_cover_preference))
        .collect::<Vec<_>>();
    let cover_total = cover_keys.len();
    let cover_cache_hits = cover_keys.iter().filter(|k| cache.is_cached(k)).count();
    let unresolved = compute_unresolved_keys(
        visible_entries,
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
            finish_nav_timing(model.as_mut(), "covers-ready", 0);
            maybe_start_initial_lookahead(model.as_mut());
        }
        return;
    }
    info!(
        pending = unresolved.len(),
        "games: arm cover gate (holding loading until covers cached)"
    );
    model.as_mut().rust_mut().pending_first_paint_keys = unresolved;
    arm_cover_gate_timeout(model);
}

fn arm_cover_gate_timeout(mut model: Pin<&mut ffi::GamesModel>) {
    let seq = model.rust().cover_gate_seq.clone();
    let ticket = seq.fetch_add(1, Ordering::SeqCst) + 1;
    let qt_thread = model.qt_thread();
    let handle = global_handle().spawn(async move {
        tokio::time::sleep(Duration::from_millis(COVER_GATE_TIMEOUT_MS)).await;
        let _ = qt_thread.queue(move |mut model: Pin<&mut ffi::GamesModel>| {
            if seq.load(Ordering::SeqCst) != ticket {
                return;
            }
            if model.loading && !model.pending_first_paint_keys.is_empty() {
                release_cover_gate_after_timeout(model);
            } else {
                model.as_mut().rust_mut().cover_gate_timer = None;
            }
        });
    });
    model.as_mut().rust_mut().cover_gate_timer = Some(handle);
}

fn maybe_start_initial_lookahead(mut model: Pin<&mut ffi::GamesModel>) {
    if !model.pending_initial_lookahead
        || model.loading
        || model.loading_more
        || !model.has_next_page
    {
        return;
    }
    model.as_mut().rust_mut().pending_initial_lookahead = false;
    model.as_mut().fetch_more();
}

/// Clear stale look-ahead state after the background prefetch lands or fails.
fn release_initial_lookahead_gate(mut model: Pin<&mut ffi::GamesModel>) {
    model.as_mut().rust_mut().pending_initial_lookahead = false;
}

/// Force-release the cover gate from the safety timer. Called only via
/// the timer's queued callback after a seq-match check; the
/// notify-driven release path lives inline in `notify_cover_update`.
fn release_cover_gate_after_timeout(mut model: Pin<&mut ffi::GamesModel>) {
    let pending = model.pending_first_paint_keys.len();
    info!(pending, "games: cover gate timed out, releasing");
    model.as_mut().rust_mut().pending_first_paint_keys.clear();
    model.as_mut().rust_mut().cover_gate_timer = None;
    // Safety timer is the hard upper bound for visible covers.
    if model.loading {
        model.as_mut().set_loading(false);
        finish_nav_timing(model.as_mut(), "timeout", pending);
        maybe_start_initial_lookahead(model.as_mut());
    }
}

/// Find `needle` in `entries` with case-sensitive path equality.
fn position_of_game_path(entries: &[BrowseEntry], needle: &str) -> i32 {
    if needle.is_empty() {
        return -1;
    }
    entries
        .iter()
        .position(|e| e.path == needle)
        .map_or(-1, |i| i as i32)
}

/// Pure projection of a `ResourceStatus<MediaBrowseResult>` onto the
/// shape `apply_status` writes into the model. Pulled out so the
/// four arms can be unit-tested without a Qt event loop.
#[derive(Debug)]
enum Projection {
    /// `Idle`/`Loading` collapse to the same view: spinner on, no
    /// error. Items are not touched.
    Pending,
    /// Successful fetch; carries the raw result so the apply layer
    /// can decide whether to auto-nav into a single root.
    Ready(MediaBrowseResult),
    /// Both `retrying` and terminal errors map here; the UI treats
    /// them the same (banner + clear loading state).
    Errored { message: String },
}

fn project_status(status: ResourceStatus<MediaBrowseResult>) -> Projection {
    match status {
        ResourceStatus::Idle | ResourceStatus::Loading => Projection::Pending,
        ResourceStatus::Ready(result) => Projection::Ready(result),
        ResourceStatus::Errored { message, .. } => Projection::Errored { message },
    }
}

/// Auto-nav decision for the initial browse result. Pure so the
/// edge cases (single folder, single media, multi-root, empty) are
/// testable without a Qt environment.
#[derive(Debug, PartialEq, Eq)]
enum InitialAction {
    /// The result has exactly one folder entry and the load was
    /// `set_system`-eligible. Skip rendering and recurse into the
    /// single root's path.
    AutoNavigate { path: String },
    /// Render the result as the visible state.
    Apply,
}

fn decide_initial(
    result: &MediaBrowseResult,
    eligible_after_set_system: bool,
    platform: Option<&Platform>,
    current_path: &str,
) -> InitialAction {
    // On MiSTer, single-folder loads also flatten on `set_path`. MiSTer
    // collections often unzip into nested single-child folders; the
    // recursion in `apply_status` calls `start_initial_browse(... false)`
    // on the auto-nav target, so a chain of single-child folders
    // collapses all the way down on each navigation step.
    let mister_set_path_flatten = matches!(platform, Some(Platform::Mister));
    let single_entry_flatten = eligible_after_set_system || mister_set_path_flatten;
    if !single_entry_flatten || result.entries.len() != 1 {
        return InitialAction::Apply;
    }
    let entry = &result.entries[0];
    // Cycle guard: refuse to auto-nav into the path we're already on.
    // Prevents stack overflow if Core erroneously returns a folder
    // whose path equals the parent.
    if entry.is_folder() && !entry.path.is_empty() && entry.path != current_path {
        return InitialAction::AutoNavigate {
            path: entry.path.clone(),
        };
    }
    InitialAction::Apply
}

/// Pre-process a freshly-decoded `media.browse` page before storing it
/// in the model. Currently only used for the `MiSTer` leading-underscore
/// strip: `MiSTer` paths use `_PrefixedFolder` to control sort order in
/// the `MiSTer` menu, which is a layout artifact not user-facing intent.
/// Path stays untouched — launching/navigation still depend on the
/// original.
/// Rows a jump fetch should request to reach `target_index` from a model
/// holding `count` rows. Pulls the gap plus one page of margin (so the landing
/// page is full and there is a little look-ahead past it), rounded up to whole
/// `page_size` pages — batch sizes always derive from the user-configurable
/// page size — and clamped to `[page_size, JUMP_FETCH_CHUNK_SIZE]` (Core's
/// `max_results` ceiling). Pure so it is unit-testable.
fn jump_fetch_limit(target_index: i32, count: i32, page_size: i32) -> i32 {
    let page = page_size.max(1);
    let gap = (target_index - count).max(0) + page;
    // Manual ceil-div: signed `i32::div_ceil` is still unstable. `gap` and
    // `page` are both >= 1, so this can't overflow for realistic folder sizes.
    let pages = (gap + page - 1) / page;
    (pages * page).clamp(page, JUMP_FETCH_CHUNK_SIZE)
}

fn transform_entries(
    mut entries: Vec<BrowseEntry>,
    platform: Option<&Platform>,
) -> Vec<BrowseEntry> {
    if matches!(platform, Some(Platform::Mister)) {
        for entry in &mut entries {
            entry.name = display_name(&entry.name, platform).into_owned();
        }
    }
    entries
}

fn display_name<'a>(raw: &'a str, platform: Option<&Platform>) -> std::borrow::Cow<'a, str> {
    if matches!(platform, Some(Platform::Mister)) {
        if let Some(stripped) = raw.strip_prefix('_') {
            return std::borrow::Cow::Owned(stripped.to_string());
        }
    }
    std::borrow::Cow::Borrowed(raw)
}

/// Drop any root-type entry whose path is a strict ancestor of another
/// root entry's path. Defends against Core occasionally surfacing the
/// shared parent dir (e.g. `/media/fat/games`) as a system root alongside
/// the actual per-system roots beneath it; the parent appears as a
/// phantom option in the frontend's roots screen, and selecting it
/// browses into a directory that contains every other system.
///
/// Only `root`-type entries participate. `directory` and `media` entries
/// pass through unchanged — a normal directory listing where a folder
/// and its subfolder appear as siblings is legitimate.
fn dedup_roots_drop_ancestors(entries: Vec<BrowseEntry>) -> Vec<BrowseEntry> {
    let root_paths: Vec<String> = entries
        .iter()
        .filter(|e| e.entry_type == "root" && !e.path.is_empty())
        .map(|e| e.path.trim_end_matches('/').to_string())
        .collect();
    entries
        .into_iter()
        .filter(|e| {
            if e.entry_type != "root" || e.path.is_empty() {
                return true;
            }
            let candidate = e.path.trim_end_matches('/');
            !root_paths
                .iter()
                .any(|other| is_strict_ancestor_path(candidate, other.as_str()))
        })
        .collect()
}

fn is_strict_ancestor_path(parent: &str, child: &str) -> bool {
    if parent.is_empty() || child.is_empty() || parent == child {
        return false;
    }
    child
        .strip_prefix(parent)
        .is_some_and(|rest| rest.starts_with('/'))
}

fn apply_status(mut model: Pin<&mut ffi::GamesModel>, status: ResourceStatus<MediaBrowseResult>) {
    match project_status(status) {
        Projection::Pending => {
            mark_nav_source(model.as_mut(), "network");
            // A new browse round started (or a Ready→Pending refetch
            // is in flight). Abort any cover gate left from the
            // previous Ready so its safety-timer callback can't race
            // with the loading=true we're about to set and clear it
            // mid-load.
            reset_cover_gate(model.as_mut());
            if !model.loading {
                model.as_mut().set_loading(true);
            }
            if !model.error_message.is_empty() {
                model.as_mut().set_error_message(QString::default());
            }
            // Pagination state is only stale on a fresh browse target;
            // a transient Pending (connection blip, post-error retry)
            // on a seeded model must preserve `has_next_page` so the
            // user's paginated view stays usable once Ready arrives —
            // the early-return in `apply_initial_page` won't restore
            // it.
            if !model.is_seeded && model.has_next_page {
                model.as_mut().set_has_next_page(false);
            }
        }
        Projection::Ready(mut result) => {
            mark_nav_request_done(model.as_mut());
            let eligible = model.rust().auto_nav_eligible;
            // Eligibility is consumed by the first Ready that follows
            // an eligible load. A subsequent refetch (mutation
            // invalidation) on the same `BrowseArgs` should NOT
            // re-auto-nav — the user is already inside the auto-nav
            // target's path stack and wouldn't expect to be teleported
            // again.
            model.as_mut().rust_mut().auto_nav_eligible = false;
            let platform = platform::current();
            let current_path = model.current_path.to_string();
            let current_system_id = model.current_system_id.to_string();
            info!(
                entries_len = result.entries.len(),
                total_files = result.total_files,
                eligible,
                ?current_path,
                "media.browse Ready",
            );
            if result.entries.is_empty() {
                warn!(
                    ?current_path,
                    ?current_system_id,
                    total_files = result.total_files,
                    "media.browse returned 0 entries",
                );
            } else {
                let mut type_counts: std::collections::BTreeMap<&str, usize> =
                    std::collections::BTreeMap::new();
                for e in &result.entries {
                    *type_counts.entry(e.entry_type.as_str()).or_insert(0) += 1;
                }
                let sample: Vec<_> = result
                    .entries
                    .iter()
                    .take(3)
                    .map(|e| {
                        format!(
                            "{}|sid={}|sids={:?}|type={}|path={}",
                            e.name, e.system_id, e.system_ids, e.entry_type, e.path
                        )
                    })
                    .collect();
                debug!(?type_counts, ?sample, "media.browse Ready entry shape",);
            }
            let pre_dedup = result.entries.len();
            result.entries = dedup_roots_drop_ancestors(result.entries);
            let removed = pre_dedup.saturating_sub(result.entries.len());
            if removed > 0 {
                warn!(
                    removed,
                    ?current_path,
                    ?current_system_id,
                    "media.browse: dropped ancestor-of-sibling root entries",
                );
            }
            match decide_initial(&result, eligible, platform.as_ref(), &current_path) {
                InitialAction::AutoNavigate { path } => {
                    info!(?path, "media.browse: auto-navigating into single folder");
                    let sid = model.current_system_id.to_string();
                    let systems = if sid.is_empty() {
                        Vec::new()
                    } else {
                        vec![sid]
                    };
                    // Recurse *without* eligibility — the auto-nav
                    // target's contents are the user's view, even if
                    // by happenstance they themselves contain a
                    // single folder.
                    model.as_mut().start_initial_browse(path, systems, false);
                }
                InitialAction::Apply => {
                    apply_initial_page(model, result);
                }
            }
        }
        Projection::Errored { message } => {
            warn!("media.browse errored: {message}");
            // Errored takes us out of Ready without going through
            // `arm_cover_gate`, so any timer left armed by the prior
            // Ready needs to be torn down explicitly.
            reset_cover_gate(model.as_mut());
            let qstr = QString::from(message.as_str());
            if model.error_message != qstr {
                model.as_mut().set_error_message(qstr);
            }
            if model.loading {
                model.as_mut().set_loading(false);
            }
            finish_nav_timing(model.as_mut(), "error", 0);
            if model.has_next_page {
                model.as_mut().set_has_next_page(false);
            }
        }
    }
}

// Already seeded: this Ready is a cache invalidation refetch on the same
// browse target (e.g. `MediaTagsUpdateMutation`'s `MediaBrowseEndpoint`
// invalidation after a favorite toggle, or a Loading→Ready cycle from a
// connection blip / post-error retry). A full model reset would clobber any
// pages the user paginated to and reset the grid to row 0; the local mutation
// paths (`apply_favorite_tags`) keep the visible model in sync. Loading was set
// true by the preceding Pending — clear it so a transient blip doesn't leave
// the spinner stuck on after the refetch lands.
fn result_total_dirs(result: &MediaBrowseResult) -> i32 {
    let total_dirs = result.total_dirs.map_or_else(
        || {
            result
                .entries
                .iter()
                .filter(|entry| entry.is_folder())
                .count()
        },
        |total_dirs| usize::try_from(total_dirs).unwrap_or(usize::MAX),
    );
    i32::try_from(total_dirs).unwrap_or(i32::MAX)
}

fn seeded_refetch_has_seed_page_only(model_count: i32, seed_entry_count: usize) -> bool {
    usize::try_from(model_count).is_ok_and(|count| count == seed_entry_count)
}

fn seeded_refetch_pagination_state(
    model_count: i32,
    seed_entry_count: usize,
    current_next_cursor: Option<String>,
    current_has_next_page: bool,
    result_next_cursor: Option<String>,
    result_has_next_page: bool,
) -> (Option<String>, bool) {
    if seeded_refetch_has_seed_page_only(model_count, seed_entry_count) {
        (result_next_cursor, result_has_next_page)
    } else {
        (current_next_cursor, current_has_next_page)
    }
}

fn apply_seeded_refetch(mut model: Pin<&mut ffi::GamesModel>, result: &MediaBrowseResult) {
    let (next_cursor, has_next_page) = seeded_refetch_pagination_state(
        model.count,
        result.entries.len(),
        model.next_cursor.clone(),
        model.has_next_page,
        result.next_cursor(),
        result.has_next_page(),
    );
    let total = i32::try_from(result.total_files).unwrap_or(i32::MAX);
    let total_dirs = result_total_dirs(result);
    if model.next_cursor != next_cursor {
        model.as_mut().rust_mut().next_cursor = next_cursor;
    }
    if model.has_next_page != has_next_page {
        model.as_mut().set_has_next_page(has_next_page);
    }
    if model.total_files != total {
        model.as_mut().set_total_files(total);
    }
    if model.total_dirs != total_dirs {
        model.as_mut().set_total_dirs(total_dirs);
    }
    if model.loading {
        model.as_mut().set_loading(false);
    }
    if !model.error_message.is_empty() {
        model.as_mut().set_error_message(QString::default());
    }
    finish_nav_timing(model.as_mut(), "already-seeded", 0);
}

fn apply_initial_page(mut model: Pin<&mut ffi::GamesModel>, result: MediaBrowseResult) {
    let apply_started = Instant::now();
    if model.is_seeded {
        apply_seeded_refetch(model, &result);
        return;
    }
    let has_next_page = result.has_next_page();
    let next_cursor = result.next_cursor();
    let total = i32::try_from(result.total_files).unwrap_or(i32::MAX);
    let total_dirs = result_total_dirs(&result);
    let platform = platform::current();
    let transform_started = Instant::now();
    let entries = transform_entries(result.entries, platform.as_ref());
    let transform_ms = transform_started.elapsed().as_millis();
    let count = i32::try_from(entries.len()).unwrap_or(i32::MAX);
    info!(
        count,
        total, total_dirs, has_next_page, "games: apply_initial_page"
    );
    let reset_started = Instant::now();
    let displays = compute_disambig_displays(&entries, model.show_original_filenames);
    model.as_mut().begin_reset_model();
    model.as_mut().rust_mut().entries = entries;
    model.as_mut().rust_mut().disambig_displays = displays;
    model.as_mut().rust_mut().count = count;
    model.as_mut().rust_mut().next_cursor = next_cursor;
    // Property setters run BEFORE `end_reset_model` so Main.qml's
    // `onModelReset` handler observes the post-load state. In
    // particular, the deep-page restore branch reads `has_next_page`
    // to decide whether to chase the saved entry across pages; if
    // we set it after `end_reset_model`, that handler sees the
    // stale `false` left by `start_initial_browse` and abandons the
    // restore (currentIndex snaps to 0). The order in
    // `apply_append_page` is the opposite (set has_next_page AFTER
    // the last insert) for a different reason: that path needs
    // PagedGrid's pending-target watchdog to read a fresh itemCount
    // when the flag flips false on the terminal chunk. A fresh
    // model reset has no pending-target watchdog to mislead, so
    // setting the flag early here is safe.
    model.as_mut().set_total_files(total);
    model.as_mut().set_total_dirs(total_dirs);
    model.as_mut().set_has_next_page(has_next_page);
    model.as_mut().end_reset_model();
    model.as_mut().count_changed();
    let reset_ms = reset_started.elapsed().as_millis();
    // Seed the cover queue from the visible row outwards instead of
    // bulk-enqueuing every entry. The grid resets to row 0 on a fresh
    // browse, so anchor the first prefetch there. Any later page turn
    // re-issues this through `onCurrentPageChanged` in QML.
    model.as_mut().rust_mut().visible_first_row = 0;
    let prefetch_started = Instant::now();
    model.as_mut().prefetch_around(0);
    let prefetch_ms = prefetch_started.elapsed().as_millis();
    // Seed the detail key for row 0 so the stale key from the previous
    // folder cannot paint the instant the cover gate releases. The gate
    // waits on the same warm cover key, so by the time `loading` flips
    // false the cache-update handler has already promoted the key to its
    // `media-image/...` form (or left it as the new folder/file chip).
    // The `is_seeded` early-return above skips this for invalidation
    // refetches; they share the same browse target and row 0 is already
    // correct.
    match model.entries.first() {
        Some(entry) if is_media_capable_entry(entry) && !entry.is_folder() => {
            let key = media_key_for(entry).map(MediaKey::with_current_cover_preference);
            set_single_detail_image_key(model.as_mut(), key);
        }
        Some(entry) => {
            let placeholder = cover_placeholder_for(entry);
            clear_detail_images(model.as_mut());
            model
                .as_mut()
                .set_current_detail_image_key(QString::from(placeholder.as_str()));
        }
        None => {
            clear_detail_images(model.as_mut());
        }
    }
    // Seed the adjacent preload keys from row 0 so hidden Images start
    // warming neighbor covers immediately rather than waiting for the
    // FocusedMediaDetailController's 220ms debounce to fire.
    model.as_mut().rust_mut().detail_prefetch_row = if model.entries.is_empty() {
        None
    } else {
        Some(0)
    };
    refresh_adjacent_cover_prefetch(model.as_mut());
    if let Some(timing) = model.as_mut().rust_mut().nav_timing.as_mut() {
        timing.mark_apply_done();
    }
    debug!(
        apply_ms = apply_started.elapsed().as_millis(),
        "games: apply_initial_page timing",
    );
    // Metadata look-ahead starts only after the visible page is
    // interactive. Otherwise the follow-up append can create delegates
    // during the transition and extend the perceived navigation stall.
    let will_lookahead = has_next_page && !model.loading_more;
    if will_lookahead {
        model.as_mut().rust_mut().pending_initial_lookahead = true;
    }
    // Decide whether to release `loading` immediately or hold it until
    // visible-page covers are cached. Background metadata look-ahead
    // does not participate in this gate.
    let gate_arm_started = Instant::now();
    arm_cover_gate(model.as_mut());
    let gate_arm_ms = gate_arm_started.elapsed().as_millis();
    debug!(
        count,
        transform_ms,
        reset_ms,
        prefetch_ms,
        gate_arm_ms,
        total_ms = apply_started.elapsed().as_millis(),
        "games: apply_initial_page detail timing",
    );
    if !model.error_message.is_empty() {
        model.as_mut().set_error_message(QString::default());
    }
    // If the visible page released synchronously (all covers cached or
    // no media covers), start look-ahead now. Otherwise the release path
    // calls `maybe_start_initial_lookahead` after loading flips false.
    maybe_start_initial_lookahead(model.as_mut());
    // Mark seeded last so any early-return from this function leaves
    // the flag in its previous state. Subsequent Ready transitions
    // on the same browse target now skip the reset above.
    model.as_mut().rust_mut().is_seeded = true;
}

/// Split a freshly-fetched chunk of `BrowseEntry` rows into sub-batches
/// of at most `size` rows each, preserving order. The first batch is
/// applied synchronously by `apply_append_page`; the rest are posted
/// one frame apart so the Repeater's per-delegate materialisation
/// cost is spread across frames instead of one big main-thread stall.
///
/// Pure helper so the partitioning is unit-testable without a Qt
/// event loop. Returns an empty Vec for an empty input or `size == 0`.
fn chunk_for_subbatching(entries: Vec<BrowseEntry>, size: usize) -> Vec<Vec<BrowseEntry>> {
    if entries.is_empty() || size == 0 {
        return Vec::new();
    }
    let mut out: Vec<Vec<BrowseEntry>> = Vec::with_capacity(entries.len().div_ceil(size));
    let mut current: Vec<BrowseEntry> = Vec::with_capacity(size);
    for entry in entries {
        if current.len() == size {
            out.push(std::mem::replace(&mut current, Vec::with_capacity(size)));
        }
        current.push(entry);
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// Single `begin_insert_rows`/`end_insert_rows` pair for one sub-batch
/// of an append. Shared between the synchronous-first-batch path and
/// the deferred-tail path so the row-count math + signal ordering live
/// in exactly one place. No-ops on an empty batch.
fn insert_sub_batch(mut model: Pin<&mut ffi::GamesModel>, batch: Vec<BrowseEntry>) {
    let new_count = i32::try_from(batch.len()).unwrap_or(i32::MAX - model.count);
    if new_count <= 0 {
        return;
    }
    let first = model.count;
    let last = first.saturating_add(new_count).saturating_sub(1);
    info!(
        "insert_sub_batch first={} last={} new_count={} count_after={}",
        first,
        last,
        new_count,
        first.saturating_add(new_count),
    );
    let parent = QModelIndex::default();
    model.as_mut().begin_insert_rows(&parent, first, last);
    model.as_mut().rust_mut().entries.extend(batch);
    model.as_mut().rust_mut().count = first.saturating_add(new_count);
    // Recompute sibling-diff displays for the boundary group + the new rows
    // before `end_insert_rows`, so the inserted delegates read fresh values on
    // first paint.
    let group_start = recompute_disambig_tail(model.as_mut(), first as usize);
    model.as_mut().end_insert_rows();
    model.as_mut().count_changed();
    // Pre-existing rows in the boundary group may now trim differently because
    // the group grew — refresh just those.
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

fn apply_append_page(
    mut model: Pin<&mut ffi::GamesModel>,
    result: Result<MediaBrowseResult, ClientError>,
    expected_prev_cursor: Option<&str>,
    bulk: bool,
) {
    // Cursor-chain check: if the model's `next_cursor` no longer
    // matches the cursor this append was advancing from, an
    // intervening `apply_initial_page` reset the chain. Appending
    // here would splice rows from the old chain onto the new one.
    if model.next_cursor.as_deref() != expected_prev_cursor {
        // Clear the in-flight cue so the UI doesn't get stuck.
        if model.loading_more {
            model.as_mut().set_loading_more(false);
        }
        // A bulk jump paused covers for this request (`fetch_more_with_limit`).
        // The intervening reset that tripped this mismatch was an `is_seeded`
        // refetch on the same scope, which doesn't resume covers, so resume
        // here or the page stays frozen with covers paused. `loading_more`
        // forbids a second concurrent bulk fetch, so no other request is
        // relying on the pause.
        if bulk {
            model.as_mut().set_cover_requests_paused(false);
        }
        return;
    }
    match result {
        Ok(result) => {
            let has_next_page = result.has_next_page();
            let next_cursor = result.next_cursor();
            let platform = platform::current();
            let entries = transform_entries(result.entries, platform.as_ref());
            let total = i32::try_from(result.total_files).unwrap_or(i32::MAX);
            // Order matters for the pending-target chain in PagedGrid:
            //
            // 1. `next_cursor` and `loading_more=false` MUST happen
            //    before the FIRST sub-batch's `end_insert_rows`. The
            //    Repeater reacts synchronously to `rowsInserted`;
            //    PagedGrid's `onItemCountChanged` runs
            //    `_commitPendingTarget`, which can re-fire
            //    `loadMoreRequested` -> `fetch_more`. If `loading_more`
            //    is still true at that moment the `fetch_more` guard
            //    early-returns and the chain stalls after one append.
            //
            // 2. `has_next_page` MUST happen after the LAST sub-batch's
            //    `end_insert_rows`. On the final chunk the value flips
            //    true -> false; if we set it first, PagedGrid's
            //    `onHasMorePagesChanged` fires `_commitPendingTarget`
            //    while `itemCount` is still stale (pre-insert), the
            //    watchdog sees a too-low itemCount, and settles the
            //    pending target on whatever was loaded BEFORE this
            //    chunk landed -- the wrap appears to land ~1 chunk
            //    short of the real last page. Setting it after the
            //    insert lets the watchdog read the fresh itemCount and
            //    clamp to the actual last item.
            //
            // Sub-batching note: the rest of the chunk is posted from
            // `global_handle()` one frame apart so the Repeater's
            // per-delegate `createObject` cost (the dominant Qt-thread
            // stall on MiSTer) is spread across frames. Stale batches
            // self-disarm via the `append_seq` ticket if a new
            // `start_initial_browse` lands during the trickle window.
            model.as_mut().rust_mut().next_cursor = next_cursor;
            model.as_mut().set_loading_more(false);
            // Jump path: insert the whole chunk in one shot, no frame-gapped
            // trickle. The appended rows sit far from `currentPage`, so their
            // Tile delegates stay unmaterialised (per-cell Loader is
            // retention-gated) and the bulk insert is cheap; the loading
            // overlay is up so a single brief stall is invisible, and it lets
            // `_commitPendingTarget` see the target as loaded in one step
            // instead of creeping toward it over dozens of frames.
            if bulk {
                let had_entries = !entries.is_empty();
                if had_entries {
                    insert_sub_batch(model.as_mut(), entries);
                }
                // Resume covers (paused by `fetch_more_with_limit` for the jump
                // request) BEFORE prefetching: the insert above drives
                // `_commitPendingTarget` synchronously, which moves
                // `visible_first_row` to the landing page, so prefetching now —
                // with covers re-enabled — warms exactly that page. Resuming
                // unconditionally (even on an empty page) guarantees the pause
                // never sticks past the request that set it.
                model.as_mut().set_cover_requests_paused(false);
                if had_entries {
                    let visible = model.visible_first_row;
                    model.as_mut().prefetch_around(visible);
                }
                model.as_mut().set_has_next_page(has_next_page);
                if model.total_files != total {
                    model.as_mut().set_total_files(total);
                }
                release_initial_lookahead_gate(model.as_mut());
                return;
            }
            let mut batches = chunk_for_subbatching(entries, APPEND_SUB_BATCH_SIZE);
            if batches.is_empty() {
                // No new rows landed (empty page). Finalise immediately
                // — there's nothing to defer.
                model.as_mut().set_has_next_page(has_next_page);
                if model.total_files != total {
                    model.as_mut().set_total_files(total);
                }
                release_initial_lookahead_gate(model.as_mut());
                return;
            }
            // First batch runs synchronously inside the existing
            // Qt-thread call so PagedGrid's `_commitPendingTarget`
            // resolves within ~200 ms of the user's press.
            let first_batch = batches.remove(0);
            insert_sub_batch(model.as_mut(), first_batch);
            // Re-arm prefetch around the user's current visible row.
            // The freshly-appended rows may now occupy the
            // "current" or "next" page window; this is the only
            // hook that gets covers warmed for them without a bulk
            // enqueue.
            let visible = model.visible_first_row;
            model.as_mut().prefetch_around(visible);
            if batches.is_empty() {
                // Single-batch chunk (<= APPEND_SUB_BATCH_SIZE rows).
                // Finalise now without scheduling deferred work.
                model.as_mut().set_has_next_page(has_next_page);
                if model.total_files != total {
                    model.as_mut().set_total_files(total);
                }
                release_initial_lookahead_gate(model.as_mut());
                return;
            }
            // Remaining batches: post one frame apart on the Qt
            // thread, with the LAST one carrying the finaliser
            // (has_next_page, total_files, look-ahead gate release).
            // total_dirs is not touched here: Core computes it once on
            // page 1 and carries the same value forward, so it never
            // changes across appended pages.
            //
            // No auto-prefetch here. apply_initial_page pre-warms
            // chunk 2 once; subsequent chunks are driven by the
            // grid's onLoadMoreRequested as the user scrolls.
            // Chaining a fetch_more here turned the look-ahead into a
            // self-driving cascade that downloaded every page
            // back-to-back, tripping Core's WebSocket rate limit on
            // huge folders (Arcade).
            let seq = model.rust().append_seq.clone();
            let ticket = seq.load(Ordering::SeqCst);
            let qt_thread = model.qt_thread();
            global_handle().spawn(async move {
                let last_idx = batches.len().saturating_sub(1);
                for (i, batch) in batches.into_iter().enumerate() {
                    tokio::time::sleep(Duration::from_millis(SUB_BATCH_FRAME_GAP_MS)).await;
                    let is_last = i == last_idx;
                    let seq = seq.clone();
                    let _ = qt_thread.queue(move |mut model: Pin<&mut ffi::GamesModel>| {
                        if seq.load(Ordering::SeqCst) != ticket {
                            return;
                        }
                        insert_sub_batch(model.as_mut(), batch);
                        let visible = model.visible_first_row;
                        model.as_mut().prefetch_around(visible);
                        if is_last {
                            model.as_mut().set_has_next_page(has_next_page);
                            if model.total_files != total {
                                model.as_mut().set_total_files(total);
                            }
                            // Clear the look-ahead gate now that the
                            // first follow-up chunk has fully landed.
                            // If the cover gate already drained
                            // (covers all cached or decode-settle
                            // fired while we held it open) we're the
                            // one releasing loading.
                            release_initial_lookahead_gate(model.as_mut());
                        }
                    });
                }
            });
        }
        Err(e) => {
            warn!("media.browse follow-up page failed: {}", e.message);
            // Surface the error so the user sees the cause; clearing
            // `loading_more` means the cue disappears even though the
            // fetch failed.
            model
                .as_mut()
                .set_error_message(QString::from(e.message.as_str()));
            model.as_mut().set_loading_more(false);
            // A bulk jump paused covers for this request; resume on the error
            // path too, mirroring the Ok path's unconditional resume, so a
            // failed jump fetch never leaves the pause stuck.
            if bulk {
                model.as_mut().set_cover_requests_paused(false);
            }
            // Even on a failed first prefetch, we have to release the
            // cover gate's hold or the user is stuck on Loading…
            // forever. The visible page is already in place from
            // `apply_initial_page`; the missing tail just won't be
            // there until the user retries.
            release_initial_lookahead_gate(model.as_mut());
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
        browse_bucket_key, child_launch_text_from_browse_result, chunk_for_subbatching,
        compute_unresolved_keys, cover_key_for_with, cover_placeholder_for, decide_initial,
        dedup_roots_drop_ancestors, detail_image_keys_from_meta, detail_tags_from_tags,
        display_name, display_title_for_entry, entry_system_id, is_media_capable_entry,
        is_strict_ancestor_path, jump_fetch_limit, launchable_count, launchable_entries,
        local_letter_index, media_capable_directory_browse_params, media_key_for,
        meta_params_for_entry, ordered_detail_image_keys, position_of_game_path,
        prefetch_around_plan, prefetch_cursor_window_plan, project_status, result_total_dirs,
        run_text_for_entry, scope_fully_loaded, seeded_refetch_pagination_state,
        singleton_directory_needs_launch_resolution, transform_entries, walk_page_size,
        InitialAction, Projection,
    };
    use super::{FETCH_MORE_RAPID_CHUNK_SIZE, JUMP_FETCH_CHUNK_SIZE};
    use crate::media_image_cache::{MediaImageCache, MediaKey};
    use std::collections::HashSet;
    use zaparoo_core::media_types::{BrowseEntry, MediaBrowseResult, Pagination, TagInfo};
    use zaparoo_core::platform::Platform;
    use zaparoo_core::remote_resource::ResourceStatus;

    fn folder(name: &str, path: &str) -> BrowseEntry {
        BrowseEntry {
            name: name.into(),
            path: path.into(),
            entry_type: "directory".into(),
            ..BrowseEntry::default()
        }
    }

    fn root(name: &str, path: &str, system_id: &str) -> BrowseEntry {
        BrowseEntry {
            name: name.into(),
            path: path.into(),
            entry_type: "root".into(),
            system_id: system_id.into(),
            system_ids: vec![system_id.into()],
            ..BrowseEntry::default()
        }
    }

    fn media(name: &str, path: &str, system_id: &str) -> BrowseEntry {
        BrowseEntry {
            name: name.into(),
            path: path.into(),
            entry_type: "media".into(),
            system_id: system_id.into(),
            zap_script: format!("@{system_id}/{name}"),
            ..BrowseEntry::default()
        }
    }

    // --- local letter index ----------------------------------------------
    // The local facet must be indistinguishable from Core's
    // media.browse.index over the same rows: same bucket vocabulary
    // (byte-exact with BrowseNameFirstChar), same 0-based file offsets,
    // same first-appearance ordering.

    #[test]
    fn bucket_key_mirrors_core_vocabulary() {
        assert_eq!(browse_bucket_key("Sonic"), "S");
        assert_eq!(browse_bucket_key("sonic"), "S");
        assert_eq!(browse_bucket_key("1942"), "0-9");
        assert_eq!(browse_bucket_key("'89 Dennou"), "#");
        assert_eq!(browse_bucket_key(""), "#");
        // Multi-byte first character: Core folds on the first BYTE,
        // which is non-ASCII, so the title lands in the symbol bucket.
        assert_eq!(browse_bucket_key("Éxito"), "#");
    }

    #[test]
    fn local_index_offsets_exclude_directories_and_fold_buckets() {
        let entries = vec![
            folder("Capcom", "/games/Arcade/Capcom"),
            media("1942", "/games/a", "Arcade"),
            media("Alien Syndrome", "/games/b", "Arcade"),
            media("Altered Beast", "/games/c", "Arcade"),
            media("Bubble Bobble", "/games/d", "Arcade"),
        ];
        let index = local_letter_index(&entries);
        assert_eq!(index.scheme, "latin");
        assert_eq!(index.total_files, 4);
        let keys: Vec<&str> = index.groups.iter().map(|g| g.key.as_str()).collect();
        assert_eq!(keys, ["0-9", "A", "B"]);
        // Offsets are 0-based positions among FILES: the leading
        // directory does not shift them.
        assert_eq!(index.groups[0].offset, 0);
        assert_eq!(index.groups[1].offset, 1);
        assert_eq!(index.groups[1].count, 2);
        assert_eq!(index.groups[2].offset, 3);
        // The router jumps by offset; no cursor is synthesized.
        assert!(index.groups.iter().all(|g| g.cursor.is_empty()));
        assert!(index.groups.iter().all(|g| g.label == g.key));
    }

    #[test]
    fn local_index_over_empty_or_dir_only_listing_has_no_groups() {
        assert!(local_letter_index(&[]).groups.is_empty());
        let dirs_only = vec![folder("Capcom", "/games/Arcade/Capcom")];
        let index = local_letter_index(&dirs_only);
        assert!(index.groups.is_empty());
        assert_eq!(index.total_files, 0);
    }

    // --- uniform random pick ---------------------------------------------
    // The whole point of picking by POSITION is that a game's odds cannot
    // depend on anything but how many games there are. These tests pin the
    // pieces that decide which position maps to which entry.

    #[test]
    fn launchable_rows_exclude_folders() {
        let entries = vec![
            folder("Konami", "/games/Arcade/Konami"),
            media("Contra", "/games/Arcade/contra.mra", "Arcade"),
            folder("Capcom", "/games/Arcade/Capcom"),
            media("Pang", "/games/Arcade/pang.mra", "Arcade"),
        ];
        assert_eq!(launchable_count(&entries), 2, "folders are not games");
        let names: Vec<&str> = launchable_entries(&entries)
            .map(|e| e.name.as_str())
            .collect();
        assert_eq!(names, vec!["Contra", "Pang"]);
    }

    #[test]
    fn launchable_position_skips_leading_directory_block() {
        // Core lists every directory before any file, so position 0 must be
        // the first FILE, not the first row. Picking by row would let
        // "random game" land on a folder.
        let entries = vec![
            folder("A", "/games/Arcade/A"),
            folder("B", "/games/Arcade/B"),
            media("Galaga", "/games/Arcade/galaga.mra", "Arcade"),
            media("Rygar", "/games/Arcade/rygar.mra", "Arcade"),
        ];
        let first = launchable_entries(&entries).next().expect("a game");
        assert_eq!(first.name, "Galaga");
        let last = launchable_entries(&entries).nth(1).expect("a game");
        assert_eq!(last.name, "Rygar");
    }

    #[test]
    fn a_folder_of_only_directories_has_no_pool() {
        let entries = vec![
            folder("A", "/games/Arcade/A"),
            folder("B", "/games/Arcade/B"),
        ];
        assert_eq!(launchable_count(&entries), 0, "nothing to launch");
    }

    #[test]
    fn scope_is_fully_loaded_only_when_nothing_is_outstanding() {
        assert!(scope_fully_loaded(10, false, None), "all rows are here");
        assert!(!scope_fully_loaded(10, true, None), "more pages advertised");
        assert!(
            !scope_fully_loaded(10, false, Some("cursor")),
            "a cursor means the chain is unfinished"
        );
        assert!(!scope_fully_loaded(0, false, None), "empty is not loaded");
    }

    #[test]
    fn walk_page_size_covers_the_target_and_the_directory_block() {
        // Must fetch at least target+1 rows to be able to land ON the target,
        // plus room for the leading directories that occupy the first page.
        assert_eq!(walk_page_size(0, 0), 1);
        assert_eq!(walk_page_size(4, 0), 5);
        assert_eq!(walk_page_size(4, 3), 8);
        // Later pages carry no directory block.
        assert_eq!(walk_page_size(9, 0), 10);
    }

    #[test]
    fn walk_page_size_is_capped_at_cores_limit() {
        assert_eq!(walk_page_size(5000, 0), 1000);
        assert_eq!(walk_page_size(999, 0), 1000);
        // A negative or absurd directory hint must not underflow.
        assert_eq!(walk_page_size(2, -5), 3);
    }

    #[test]
    fn media_entry_is_media_capable() {
        assert!(is_media_capable_entry(&media(
            "smb",
            "/games/NES/smb.nes",
            "NES"
        )));
    }

    #[test]
    fn directory_with_only_system_id_is_not_media_capable() {
        let mut entry = folder("Archive", "/games/GB/archive.zip");
        entry.system_id = "Gameboy".into();
        assert!(!is_media_capable_entry(&entry));
    }

    #[test]
    fn directory_with_media_id_is_media_capable() {
        let mut entry = folder("Single", "/games/GB/single.zip");
        entry.system_id = "Gameboy".into();
        entry.media_id = Some(42);
        assert!(is_media_capable_entry(&entry));
    }

    #[test]
    fn directory_with_zap_script_is_media_capable() {
        let mut entry = folder("Single", "/games/GB/single.zip");
        entry.system_id = "Gameboy".into();
        entry.zap_script = "@Gameboy/Single".into();
        assert!(is_media_capable_entry(&entry));
    }

    #[test]
    fn root_with_system_id_is_not_media_capable() {
        assert!(!is_media_capable_entry(&root("GB", "/games/GB", "Gameboy")));
    }

    #[test]
    fn idle_projects_to_pending() {
        match project_status(ResourceStatus::Idle) {
            Projection::Pending => {}
            other => panic!("expected Pending, got {other:?}"),
        }
    }

    #[test]
    fn loading_projects_to_pending() {
        match project_status(ResourceStatus::Loading) {
            Projection::Pending => {}
            other => panic!("expected Pending, got {other:?}"),
        }
    }

    #[test]
    fn ready_carries_full_result() {
        let result = MediaBrowseResult {
            path: "/games/NES".into(),
            entries: vec![media("smb", "/games/NES/smb.nes", "NES")],
            total_files: 1,
            total_dirs: Some(0),
            pagination: None,
        };
        match project_status(ResourceStatus::Ready(result)) {
            Projection::Ready(r) => {
                assert_eq!(r.entries.len(), 1);
                assert_eq!(r.total_files, 1);
            }
            other => panic!("expected Ready, got {other:?}"),
        }
    }

    #[test]
    fn errored_propagates_message() {
        match project_status(ResourceStatus::Errored {
            message: "rpc kaboom".into(),
            retrying: true,
        }) {
            Projection::Errored { message } => assert_eq!(message, "rpc kaboom"),
            other => panic!("expected Errored, got {other:?}"),
        }
    }

    #[test]
    fn total_dirs_uses_present_zero_without_fallback() {
        let result = MediaBrowseResult {
            entries: vec![folder("Dir", "/games/Dir")],
            total_dirs: Some(0),
            ..MediaBrowseResult::default()
        };
        assert_eq!(result_total_dirs(&result), 0);
    }

    #[test]
    fn total_dirs_falls_back_to_folder_count_when_missing() {
        let result = MediaBrowseResult {
            entries: vec![
                folder("Dir", "/games/Dir"),
                media("Game", "/games/game.rom", "NES"),
                root("NES", "/games/NES", "NES"),
            ],
            total_dirs: None,
            ..MediaBrowseResult::default()
        };
        assert_eq!(result_total_dirs(&result), 2);
    }

    #[test]
    fn seeded_refetch_after_append_keeps_pagination_cursor() {
        let (next_cursor, has_next_page) = seeded_refetch_pagination_state(
            24,
            12,
            Some("page-3".into()),
            true,
            Some("page-2".into()),
            true,
        );
        assert_eq!(next_cursor.as_deref(), Some("page-3"));
        assert!(has_next_page);
    }

    #[test]
    fn seeded_refetch_on_seed_page_refreshes_pagination_cursor() {
        let (next_cursor, has_next_page) = seeded_refetch_pagination_state(
            12,
            12,
            Some("stale".into()),
            true,
            Some("page-2".into()),
            false,
        );
        assert_eq!(next_cursor.as_deref(), Some("page-2"));
        assert!(!has_next_page);
    }

    #[test]
    fn decide_initial_with_single_root_eligible_returns_auto_nav() {
        let result = MediaBrowseResult {
            entries: vec![root("NES", "/roms/NES", "NES")],
            ..MediaBrowseResult::default()
        };
        assert_eq!(
            decide_initial(&result, true, None, ""),
            InitialAction::AutoNavigate {
                path: "/roms/NES".into()
            }
        );
    }

    #[test]
    fn decide_initial_with_single_directory_eligible_returns_auto_nav() {
        let result = MediaBrowseResult {
            entries: vec![folder("Games", "/roms/Shared/Games")],
            ..MediaBrowseResult::default()
        };
        assert_eq!(
            decide_initial(&result, true, None, ""),
            InitialAction::AutoNavigate {
                path: "/roms/Shared/Games".into()
            }
        );
    }

    #[test]
    fn decide_initial_with_single_folder_not_eligible_renders_as_apply_off_mister() {
        // `set_path`-driven loads must not auto-nav off MiSTer — the
        // user is explicitly navigating through folders and would lose
        // orientation otherwise.
        let result = MediaBrowseResult {
            entries: vec![folder("Sub", "/x/Sub")],
            ..MediaBrowseResult::default()
        };
        assert_eq!(
            decide_initial(&result, false, Some(&Platform::Linux), ""),
            InitialAction::Apply,
        );
        assert_eq!(
            decide_initial(&result, false, None, ""),
            InitialAction::Apply,
        );
    }

    #[test]
    fn decide_initial_with_single_folder_not_eligible_flattens_on_mister() {
        // On MiSTer, a `set_path` load that returns exactly one folder
        // also flattens — collections often unzip into nested
        // single-child folders.
        let result = MediaBrowseResult {
            entries: vec![folder("Sub", "/x/Sub")],
            ..MediaBrowseResult::default()
        };
        assert_eq!(
            decide_initial(&result, false, Some(&Platform::Mister), ""),
            InitialAction::AutoNavigate {
                path: "/x/Sub".into()
            },
        );
    }

    #[test]
    fn decide_initial_refuses_to_auto_nav_into_same_path() {
        // Cycle guard: if Core returns a folder whose path equals the
        // current browse path, recursing would loop forever (the cache
        // returns the same Ready). Render as-is instead.
        let result = MediaBrowseResult {
            entries: vec![folder("Self", "/x/Sub")],
            ..MediaBrowseResult::default()
        };
        assert_eq!(
            decide_initial(&result, false, Some(&Platform::Mister), "/x/Sub"),
            InitialAction::Apply,
        );
        assert_eq!(
            decide_initial(&result, true, None, "/x/Sub"),
            InitialAction::Apply,
        );
    }

    #[test]
    fn decide_initial_with_multi_root_returns_apply() {
        let result = MediaBrowseResult {
            entries: vec![
                root("primary", "/roms/A", "NES"),
                root("backup", "/roms/B", "NES"),
            ],
            ..MediaBrowseResult::default()
        };
        assert_eq!(
            decide_initial(&result, true, None, ""),
            InitialAction::Apply
        );
    }

    #[test]
    fn decide_initial_with_empty_returns_apply() {
        let result = MediaBrowseResult::default();
        assert_eq!(
            decide_initial(&result, true, None, ""),
            InitialAction::Apply
        );
    }

    #[test]
    fn decide_initial_with_single_media_returns_apply() {
        // A system with exactly one ROM at the root shouldn't be
        // treated as a folder; auto-nav into a media entry would
        // make no sense.
        let result = MediaBrowseResult {
            entries: vec![media("only", "/p/only", "NES")],
            ..MediaBrowseResult::default()
        };
        assert_eq!(
            decide_initial(&result, true, None, ""),
            InitialAction::Apply
        );
    }

    #[test]
    fn decide_initial_with_single_root_blank_path_renders_as_apply() {
        // A root with no `path` cannot be navigated into. Rather than
        // recurse infinitely on a malformed payload, render the entry
        // as-is and let the user see something usable.
        let result = MediaBrowseResult {
            entries: vec![root("ghost", "", "NES")],
            ..MediaBrowseResult::default()
        };
        assert_eq!(
            decide_initial(&result, true, None, ""),
            InitialAction::Apply
        );
    }

    #[test]
    fn display_title_prefers_core_name_with_disc_marker() {
        let entry = media("D (Disc 1)", "/roms/PSX/D.cue", "PSX");
        assert_eq!(display_title_for_entry(&entry).as_ref(), "D (Disc 1)");
    }

    #[test]
    fn display_title_prefers_singleton_directory_alias() {
        let mut entry = folder("Friendly Alias", "/roms/PSX/InternalContainer");
        entry.media_id = Some(42);
        entry.system_id = "PSX".into();
        entry.zap_script = "@PSX/Friendly Alias".into();
        assert_eq!(display_title_for_entry(&entry).as_ref(), "Friendly Alias");
    }

    #[test]
    fn display_title_falls_back_to_file_stem_when_name_empty() {
        let entry = media("", "/roms/PSX/D (Disc 2).cue", "PSX");
        assert_eq!(display_title_for_entry(&entry).as_ref(), "D (Disc 2)");
    }

    #[test]
    fn display_name_strips_single_leading_underscore_on_mister() {
        assert_eq!(
            display_name("_Arcade", Some(&Platform::Mister)).as_ref(),
            "Arcade",
        );
    }

    #[test]
    fn display_name_only_strips_one_underscore_on_mister() {
        assert_eq!(
            display_name("__deep", Some(&Platform::Mister)).as_ref(),
            "_deep",
        );
    }

    #[test]
    fn display_name_preserves_off_mister() {
        assert_eq!(
            display_name("_Arcade", Some(&Platform::Linux)).as_ref(),
            "_Arcade",
        );
        assert_eq!(display_name("_Arcade", None).as_ref(), "_Arcade");
    }

    #[test]
    fn transform_entries_strips_underscore_only_on_mister() {
        let entries = vec![folder("_Arcade", "/A"), media("_smb", "/A/_smb", "NES")];
        let on_mister = transform_entries(entries.clone(), Some(&Platform::Mister));
        assert_eq!(on_mister[0].name, "Arcade");
        assert_eq!(on_mister[1].name, "smb");
        // Path is left alone.
        assert_eq!(on_mister[0].path, "/A");
        assert_eq!(on_mister[1].path, "/A/_smb");

        let off_mister = transform_entries(entries, Some(&Platform::Linux));
        assert_eq!(off_mister[0].name, "_Arcade");
        assert_eq!(off_mister[1].name, "_smb");
    }

    #[test]
    fn is_strict_ancestor_path_matches_parent_of_child() {
        assert!(is_strict_ancestor_path(
            "/media/fat/games",
            "/media/fat/games/Genesis",
        ));
        assert!(is_strict_ancestor_path(
            "/media/fat/games",
            "/media/fat/games/MegaDrive",
        ));
    }

    #[test]
    fn is_strict_ancestor_path_rejects_equal_paths() {
        assert!(!is_strict_ancestor_path("/a/b", "/a/b"));
    }

    #[test]
    fn is_strict_ancestor_path_rejects_unrelated_paths() {
        assert!(!is_strict_ancestor_path("/a/b", "/a/bc"));
        assert!(!is_strict_ancestor_path("/a/b", "/x/b/c"));
    }

    #[test]
    fn is_strict_ancestor_path_rejects_empty_inputs() {
        assert!(!is_strict_ancestor_path("", "/a"));
        assert!(!is_strict_ancestor_path("/a", ""));
    }

    #[test]
    fn dedup_roots_drops_ancestor_root_when_sibling_is_descendant() {
        // The Genesis 3-root payload Core surfaced on a real MiSTer:
        // /media/fat/games appears alongside the per-system roots beneath
        // it. The shared parent dir must not reach the frontend's roots
        // screen as a third option.
        let entries = vec![
            root("MegaDrive", "/media/fat/games/MegaDrive", "Genesis"),
            root("Genesis", "/media/fat/games/Genesis", "Genesis"),
            root("games", "/media/fat/games", "Genesis"),
        ];
        let kept = dedup_roots_drop_ancestors(entries);
        assert_eq!(kept.len(), 2);
        assert!(kept.iter().any(|e| e.path == "/media/fat/games/MegaDrive"));
        assert!(kept.iter().any(|e| e.path == "/media/fat/games/Genesis"));
        assert!(!kept.iter().any(|e| e.path == "/media/fat/games"));
    }

    #[test]
    fn dedup_roots_passes_through_unrelated_roots() {
        let entries = vec![
            root("primary", "/roms/A", "NES"),
            root("backup", "/roms/B", "NES"),
        ];
        let kept = dedup_roots_drop_ancestors(entries);
        assert_eq!(kept.len(), 2);
    }

    #[test]
    fn dedup_roots_ignores_non_root_entries() {
        // A directory listing that contains a folder and its subfolder is
        // legitimate (e.g. inside a system browse). The filter only
        // applies to root-typed entries.
        let entries = vec![
            folder("parent", "/x"),
            folder("child", "/x/sub"),
            media("smb", "/x/sub/smb.nes", "NES"),
        ];
        let kept = dedup_roots_drop_ancestors(entries);
        assert_eq!(kept.len(), 3);
    }

    #[test]
    fn dedup_roots_keeps_root_with_blank_path() {
        // Blank-path roots cannot be navigated into (handled separately
        // by decide_initial). They never participate as ancestors and
        // never get dropped as descendants.
        let entries = vec![root("ghost", "", "NES"), root("real", "/roms/NES", "NES")];
        let kept = dedup_roots_drop_ancestors(entries);
        assert_eq!(kept.len(), 2);
    }

    #[test]
    fn dedup_roots_handles_trailing_slash_on_either_side() {
        let entries = vec![
            root("parent", "/media/fat/games/", "Genesis"),
            root("child", "/media/fat/games/Genesis", "Genesis"),
        ];
        let kept = dedup_roots_drop_ancestors(entries);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].path, "/media/fat/games/Genesis");
    }

    #[test]
    fn entry_system_id_prefers_singular_field() {
        let entry = BrowseEntry {
            system_id: "SNES".into(),
            system_ids: vec!["SNES".into(), "Stub".into()],
            ..BrowseEntry::default()
        };
        assert_eq!(entry_system_id(&entry), "SNES");
    }

    #[test]
    fn entry_system_id_falls_back_to_first_of_system_ids_for_multi_system_root() {
        let entry = BrowseEntry {
            system_id: String::new(),
            system_ids: vec!["NES".into(), "FDS".into()],
            ..BrowseEntry::default()
        };
        assert_eq!(entry_system_id(&entry), "NES");
    }

    #[test]
    fn cover_key_for_folder_returns_folder_icon() {
        let entry = folder("Games", "/x");
        assert_eq!(
            cover_key_for_with(&entry, None, false, false),
            "icons/Folder"
        );
        // Folders never carry a cached cover, but explicit assertion
        // documents that even if a cache hit somehow surfaced for a
        // folder key we keep the folder glyph.
        let stale_key = MediaKey::new("NES", "/x");
        assert_eq!(
            cover_key_for_with(&entry, Some(&stale_key), true, false),
            "icons/Folder"
        );
    }

    #[test]
    fn cover_key_for_media_in_flight_returns_loading_icon() {
        let entry = media("smb", "/p/smb", "NES");
        let key = media_key_for(&entry).expect("media has key");
        // Not cached, not negative → fetch is in flight, show hourglass.
        assert_eq!(
            cover_key_for_with(&entry, Some(&key), false, false),
            "icons/Loading"
        );
    }

    #[test]
    fn cover_placeholder_never_returns_loading_icon() {
        assert_eq!(
            cover_placeholder_for(&media("smb", "/p/smb", "NES")),
            "icons/File"
        );
        assert_eq!(
            cover_placeholder_for(&folder("Games", "/x")),
            "icons/Folder"
        );
    }

    #[test]
    fn cover_key_for_media_negatively_memoed_returns_file_icon() {
        let entry = media("smb", "/p/smb", "NES");
        let key = media_key_for(&entry).expect("media has key");
        // Not cached, but negatively memoed → there is no image to
        // wait for, fall back to the plain file icon (no stuck
        // hourglass).
        assert_eq!(
            cover_key_for_with(&entry, Some(&key), false, true),
            "icons/File"
        );
    }

    #[test]
    fn cover_key_for_media_soft_missed_returns_file_icon() {
        let entry = media("smb", "/p/smb", "NES");
        let key = media_key_for(&entry).expect("media has key");
        assert_eq!(
            cover_key_for_with(&entry, Some(&key), false, true),
            "icons/File"
        );
    }

    #[test]
    fn cover_key_for_media_with_cache_returns_media_image_key() {
        let entry = media("smb", "/p/smb", "NES");
        let key = media_key_for(&entry).expect("media has key");
        let expected = MediaImageCache::image_key_for(&key);
        assert_eq!(
            cover_key_for_with(&entry, Some(&key), true, false),
            expected
        );
        assert!(expected.starts_with("media-image/"));
    }

    #[test]
    fn cover_key_for_unattributed_media_returns_file_icon() {
        let entry = BrowseEntry {
            entry_type: "media".into(),
            ..BrowseEntry::default()
        };
        // No system id and no path → no MediaKey.
        assert!(media_key_for(&entry).is_none());
        assert_eq!(cover_key_for_with(&entry, None, false, false), "icons/File");
    }

    #[test]
    fn media_key_for_skips_folders_and_unattributed_entries() {
        assert!(media_key_for(&folder("Games", "/x")).is_none());
        assert!(media_key_for(&root("NES", "/roms/NES", "NES")).is_none());
        let unattributed = BrowseEntry {
            entry_type: "media".into(),
            path: "/p".into(),
            ..BrowseEntry::default()
        };
        assert!(media_key_for(&unattributed).is_none());
        let pathless = BrowseEntry {
            entry_type: "media".into(),
            system_id: "NES".into(),
            ..BrowseEntry::default()
        };
        assert!(media_key_for(&pathless).is_none());
        let key = media_key_for(&media("smb", "/p/smb", "NES")).expect("ok");
        assert_eq!(key.system_id.as_ref(), "NES");
        assert_eq!(key.path.as_ref(), "/p/smb");
    }

    #[test]
    fn singleton_directory_uses_media_id_for_meta_and_skips_container_run_text() {
        let entry = BrowseEntry {
            media_id: Some(42),
            name: "Game".into(),
            path: "/roms/PSX/Game".into(),
            entry_type: "directory".into(),
            system_id: "PSX".into(),
            zap_script: "@PSX/Game".into(),
            ..BrowseEntry::default()
        };
        assert!(singleton_directory_needs_launch_resolution(&entry));
        let params = meta_params_for_entry(&entry).expect("meta params");
        assert_eq!(params.media_id, Some(42));
        assert!(params.system.is_empty());
        assert!(params.path.is_empty());
        let browse_params = media_capable_directory_browse_params(&entry).expect("browse params");
        assert_eq!(browse_params.path, "/roms/PSX/Game");
        assert_eq!(browse_params.systems, vec!["PSX".to_string()]);
        assert_eq!(browse_params.max_results, Some(1000));
        assert_eq!(run_text_for_entry(&entry).as_deref(), Some("@PSX/Game"));
        assert_ne!(
            run_text_for_entry(&entry).as_deref(),
            Some("/roms/PSX/Game")
        );
    }

    #[test]
    fn singleton_directory_child_resolution_prefers_cue_path_over_zapscript() {
        let parent = BrowseEntry {
            media_id: Some(42),
            name: "Game".into(),
            path: "/roms/PSX/Game".into(),
            entry_type: "directory".into(),
            system_id: "PSX".into(),
            zap_script: "@PSX/Game".into(),
            ..BrowseEntry::default()
        };
        let result = MediaBrowseResult {
            path: parent.path.clone(),
            entries: vec![
                BrowseEntry {
                    media_id: Some(42),
                    name: "Game Track 1".into(),
                    path: "/roms/PSX/Game/track01.bin".into(),
                    entry_type: "media".into(),
                    system_id: "PSX".into(),
                    zap_script: "@PSX/Game Track 1".into(),
                    ..BrowseEntry::default()
                },
                BrowseEntry {
                    media_id: Some(42),
                    name: "Game".into(),
                    path: "/roms/PSX/Game/Game.cue".into(),
                    entry_type: "media".into(),
                    system_id: "PSX".into(),
                    zap_script: "@PSX/Game".into(),
                    ..BrowseEntry::default()
                },
            ],
            total_files: 2,
            total_dirs: Some(0),
            pagination: None,
        };
        assert_eq!(
            child_launch_text_from_browse_result(&parent, &result).as_deref(),
            Some("/roms/PSX/Game/Game.cue")
        );
    }

    #[test]
    fn singleton_directory_without_zapscript_can_still_resolve_child_path() {
        let parent = BrowseEntry {
            media_id: Some(42),
            name: "Game".into(),
            path: "/roms/PSX/Game".into(),
            entry_type: "directory".into(),
            system_id: "PSX".into(),
            ..BrowseEntry::default()
        };
        let result = MediaBrowseResult {
            path: parent.path.clone(),
            entries: vec![BrowseEntry {
                media_id: Some(42),
                name: "Game".into(),
                path: "/roms/PSX/Game/Game.cue".into(),
                entry_type: "media".into(),
                system_id: "PSX".into(),
                ..BrowseEntry::default()
            }],
            total_files: 1,
            total_dirs: Some(0),
            pagination: None,
        };
        assert!(singleton_directory_needs_launch_resolution(&parent));
        assert_eq!(run_text_for_entry(&parent), None);
        assert_eq!(
            child_launch_text_from_browse_result(&parent, &result).as_deref(),
            Some("/roms/PSX/Game/Game.cue")
        );
    }

    #[test]
    fn normal_media_does_not_need_launch_resolution() {
        let entry = BrowseEntry {
            media_id: Some(7),
            ..media("smb", "/roms/NES/smb.nes", "NES")
        };
        assert!(!singleton_directory_needs_launch_resolution(&entry));
        assert_eq!(
            run_text_for_entry(&entry).as_deref(),
            Some("/roms/NES/smb.nes")
        );
    }

    #[test]
    fn position_of_game_path_returns_index_on_case_exact_match() {
        let entries = vec![
            media("smb", "/p/smb", "NES"),
            media("zelda", "/p/zelda", "NES"),
        ];
        assert_eq!(position_of_game_path(&entries, "/p/zelda"), 1);
    }

    #[test]
    fn position_of_game_path_is_case_sensitive() {
        let entries = vec![media("smb", "/p/smb", "NES")];
        assert_eq!(position_of_game_path(&entries, "/P/SMB"), -1);
        assert_eq!(position_of_game_path(&entries, "/p/SMB"), -1);
    }

    #[test]
    fn position_of_game_path_empty_needle_returns_minus_one() {
        let entries = vec![media("smb", "/p/smb", "NES")];
        assert_eq!(position_of_game_path(&entries, ""), -1);
    }

    #[test]
    fn position_of_game_path_missing_returns_minus_one() {
        let entries = vec![media("smb", "/p/smb", "NES")];
        assert_eq!(position_of_game_path(&entries, "/p/missing"), -1);
    }

    #[test]
    fn position_of_directory_path_returns_index() {
        // The path stack restore pulls indices for directory entries
        // too, not just media; guard against future regressions
        // accidentally narrowing this to media-only.
        let entries = vec![folder("A", "/x/A"), folder("B", "/x/B")];
        assert_eq!(position_of_game_path(&entries, "/x/B"), 1);
    }

    #[test]
    fn pagination_helpers_carry_cursor_through() {
        let result = MediaBrowseResult {
            pagination: Some(Pagination {
                has_next_page: true,
                page_size: 100,
                next_cursor: Some("c".into()),
            }),
            ..MediaBrowseResult::default()
        };
        assert!(result.has_next_page());
        assert_eq!(result.next_cursor(), Some("c".into()));
    }

    #[test]
    fn pagination_helpers_default_to_no_more_pages() {
        let result = MediaBrowseResult::default();
        assert!(!result.has_next_page());
        assert_eq!(result.next_cursor(), None);
    }

    #[test]
    fn compute_unresolved_keys_folder_only_page_returns_empty() {
        let entries = vec![folder("Games", "/x/Games"), folder("Saves", "/x/Saves")];
        let unresolved = compute_unresolved_keys(&entries, |_| false, |_| false);
        assert!(unresolved.is_empty());
    }

    #[test]
    fn compute_unresolved_keys_all_cached_returns_empty() {
        let entries = vec![
            media("smb", "/p/smb", "NES"),
            media("zelda", "/p/zelda", "NES"),
        ];
        let unresolved = compute_unresolved_keys(&entries, |_| true, |_| false);
        assert!(unresolved.is_empty());
    }

    #[test]
    fn compute_unresolved_keys_mixed_returns_only_uncached() {
        let cached_path = "/p/smb";
        let entries = vec![
            media("smb", cached_path, "NES"),
            media("zelda", "/p/zelda", "NES"),
            folder("Saves", "/p/Saves"),
        ];
        let unresolved =
            compute_unresolved_keys(&entries, |k| k.path.as_ref() == cached_path, |_| false);
        let expected: HashSet<MediaKey> = [MediaKey::new("NES", "/p/zelda")].into_iter().collect();
        assert_eq!(unresolved, expected);
    }

    #[test]
    fn compute_unresolved_keys_excludes_negative_memo() {
        // Negative-memoised keys are "Core said no image" — gate must
        // not wait on them, otherwise the gate stays armed until the
        // safety timer fires every time.
        let negative_path = "/p/no-image";
        let entries = vec![
            media("smb", "/p/smb", "NES"),
            media("orphan", negative_path, "NES"),
        ];
        let unresolved =
            compute_unresolved_keys(&entries, |_| false, |k| k.path.as_ref() == negative_path);
        let expected: HashSet<MediaKey> = [MediaKey::new("NES", "/p/smb")].into_iter().collect();
        assert_eq!(unresolved, expected);
    }

    #[test]
    fn compute_unresolved_keys_skips_unattributed_entries() {
        // Browse entries without enough info to key on — folder, root,
        // unattributed media — never produce a MediaKey, so the gate
        // doesn't wait on them.
        let entries = vec![
            folder("Games", "/x/Games"),
            root("NES", "/roms/NES", "NES"),
            BrowseEntry {
                entry_type: "media".into(),
                ..BrowseEntry::default()
            },
        ];
        let unresolved = compute_unresolved_keys(&entries, |_| false, |_| false);
        assert!(unresolved.is_empty());
    }

    #[test]
    fn compute_unresolved_keys_excludes_no_cover_entries() {
        // Core sends has_cover=false for entries with no image property row.
        // These entries will never resolve to cached bytes, so they must
        // not be included in the gate set — otherwise the gate would always
        // ride the safety timer on systems like Arcade.
        let mut no_cover = media("nocovergame", "/p/nocovergame", "Arcade");
        no_cover.has_cover = false;
        let entries = vec![no_cover, media("coveredgame", "/p/coveredgame", "NES")];
        let unresolved = compute_unresolved_keys(&entries, |_| false, |_| false);
        let expected: HashSet<MediaKey> = [MediaKey::new("NES", "/p/coveredgame")]
            .into_iter()
            .collect();
        assert_eq!(unresolved, expected);
    }

    #[test]
    fn compute_unresolved_keys_all_no_cover_returns_empty() {
        // A page where Core confirmed no entry has a cover (e.g. Arcade
        // with no scraped artwork) must result in an empty unresolved set
        // so the gate releases immediately rather than timing out.
        let mut a = media("a", "/p/a", "Arcade");
        a.has_cover = false;
        let mut b = media("b", "/p/b", "Arcade");
        b.has_cover = false;
        let entries = vec![a, b];
        let unresolved = compute_unresolved_keys(&entries, |_| false, |_| false);
        assert!(unresolved.is_empty());
    }

    #[test]
    fn chunk_for_subbatching_empty_returns_empty() {
        let out = chunk_for_subbatching(Vec::new(), 25);
        assert!(out.is_empty());
    }

    #[test]
    fn chunk_for_subbatching_zero_size_returns_empty() {
        let entries = vec![media("a", "/a", "NES")];
        let out = chunk_for_subbatching(entries, 0);
        assert!(out.is_empty());
    }

    #[test]
    fn jump_fetch_limit_sizes_to_gap_plus_one_page_rounded_up() {
        // Target 570, 113 loaded, page 100: gap = 457 + 100 margin = 557 ->
        // 6 pages -> 600. Far less than the 1000 ceiling for a mid-folder jump.
        assert_eq!(jump_fetch_limit(570, 113, 100), 600);
    }

    #[test]
    fn jump_fetch_limit_clamps_to_ceiling_for_far_targets() {
        // A target past what one ceiling-sized fetch covers clamps to the cap;
        // the pending-target loop fetches the next slice afterward.
        assert_eq!(jump_fetch_limit(5000, 113, 100), JUMP_FETCH_CHUNK_SIZE);
    }

    #[test]
    fn jump_fetch_limit_floors_at_one_page_when_target_already_loaded() {
        // Target at or behind the loaded count has no gap, but the one-page
        // margin still pulls a single page (look-ahead past the landing page).
        assert_eq!(jump_fetch_limit(113, 113, 100), 100);
        assert_eq!(jump_fetch_limit(50, 113, 100), 100);
    }

    #[test]
    fn jump_fetch_limit_margin_rounds_a_small_gap_up_to_two_pages() {
        // A target just past the loaded slice: gap (7) + one-page margin (100)
        // = 107 -> rounds up to 2 pages so the landing page is fully covered.
        assert_eq!(jump_fetch_limit(120, 113, 100), 200);
    }

    #[test]
    fn jump_fetch_limit_is_a_whole_number_of_pages() {
        for target in [0, 37, 113, 251, 499, 800] {
            let limit = jump_fetch_limit(target, 113, 100);
            assert_eq!(limit % 100, 0, "limit {limit} not a page multiple");
        }
    }

    #[test]
    fn jump_fetch_limit_never_below_rapid_scroll_chunk_at_default_page() {
        // A jump must not pull a smaller batch than ordinary rapid scrolling
        // when the configured page is at least the rapid chunk.
        let limit = jump_fetch_limit(2000, 0, FETCH_MORE_RAPID_CHUNK_SIZE);
        assert!(limit >= FETCH_MORE_RAPID_CHUNK_SIZE);
    }

    #[test]
    fn jump_fetch_limit_tolerates_nonpositive_page_size() {
        // Defensive: a zero/negative page_size must not divide-by-zero; it
        // floors to a single one-row page (gap = 0 + 1 margin = 1 page).
        assert_eq!(jump_fetch_limit(0, 0, 0), 1);
    }

    #[test]
    fn chunk_for_subbatching_under_size_returns_single_batch() {
        let entries: Vec<BrowseEntry> = (0..10)
            .map(|i| media(&format!("g{i}"), &format!("/g/{i}"), "NES"))
            .collect();
        let out = chunk_for_subbatching(entries, 25);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].len(), 10);
    }

    #[test]
    fn chunk_for_subbatching_exact_multiple_splits_evenly() {
        let entries: Vec<BrowseEntry> = (0..50)
            .map(|i| media(&format!("g{i}"), &format!("/g/{i}"), "NES"))
            .collect();
        let out = chunk_for_subbatching(entries, 25);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].len(), 25);
        assert_eq!(out[1].len(), 25);
    }

    #[test]
    fn chunk_for_subbatching_remainder_lands_in_final_batch() {
        let entries: Vec<BrowseEntry> = (0..60)
            .map(|i| media(&format!("g{i}"), &format!("/g/{i}"), "NES"))
            .collect();
        let out = chunk_for_subbatching(entries, 25);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].len(), 25);
        assert_eq!(out[1].len(), 25);
        assert_eq!(out[2].len(), 10);
    }

    #[test]
    fn chunk_for_subbatching_preserves_order() {
        let entries: Vec<BrowseEntry> = (0..30)
            .map(|i| media(&format!("g{i}"), &format!("/g/{i}"), "NES"))
            .collect();
        let out = chunk_for_subbatching(entries, 25);
        assert_eq!(out[0][0].path, "/g/0");
        assert_eq!(out[0][24].path, "/g/24");
        assert_eq!(out[1][0].path, "/g/25");
        assert_eq!(out[1][4].path, "/g/29");
    }

    #[test]
    fn prefetch_around_plan_orders_current_next_pages_previous() {
        let entries: Vec<BrowseEntry> = (0..60)
            .map(|i| media(&format!("g{i}"), &format!("/g/{i}"), "NES"))
            .collect();
        let plan = prefetch_around_plan(&entries, 60, 15, 15);
        let paths: Vec<String> = plan.iter().map(|(k, _)| k.path.to_string()).collect();
        let mut expected: Vec<String> = (15..30).map(|i| format!("/g/{i}")).collect();
        expected.extend((30..60).map(|i| format!("/g/{i}")));
        expected.extend((0..15).map(|i| format!("/g/{i}")));
        assert_eq!(paths, expected);
    }

    #[test]
    fn prefetch_around_plan_skips_folders_and_unattributed() {
        // Folder in-page slot — should be silently skipped, not break
        // the order of the surrounding media entries.
        let entries: Vec<BrowseEntry> = vec![
            folder("dir", "/x/dir"),
            media("g0", "/g/0", "NES"),
            folder("dir2", "/x/dir2"),
            media("g1", "/g/1", "NES"),
        ];
        let plan = prefetch_around_plan(&entries, 4, 4, 0);
        let paths: Vec<String> = plan.iter().map(|(k, _)| k.path.to_string()).collect();
        assert_eq!(paths, vec!["/g/0".to_string(), "/g/1".to_string()]);
    }

    #[test]
    fn prefetch_around_plan_handles_short_tail() {
        // 20 rows, page 1 visible (15..20). No next page; previous
        // page follows the visible tail.
        let entries: Vec<BrowseEntry> = (0..20)
            .map(|i| media(&format!("g{i}"), &format!("/g/{i}"), "NES"))
            .collect();
        let plan = prefetch_around_plan(&entries, 20, 15, 15);
        let paths: Vec<String> = plan.iter().map(|(k, _)| k.path.to_string()).collect();
        let mut expected: Vec<String> = (15..20).map(|i| format!("/g/{i}")).collect();
        expected.extend((0..15).map(|i| format!("/g/{i}")));
        assert_eq!(paths, expected);
    }

    #[test]
    fn prefetch_around_plan_empty_for_empty_model() {
        let entries: Vec<BrowseEntry> = Vec::new();
        let plan = prefetch_around_plan(&entries, 0, 15, 0);
        assert!(plan.is_empty());
    }

    #[test]
    fn prefetch_around_plan_clamps_negative_first_visible_row() {
        let entries: Vec<BrowseEntry> = (0..30)
            .map(|i| media(&format!("g{i}"), &format!("/g/{i}"), "NES"))
            .collect();
        let plan = prefetch_around_plan(&entries, 30, 15, -100);
        let paths: Vec<String> = plan.iter().map(|(k, _)| k.path.to_string()).collect();
        // Clamped to 0: visible rows 0..15, next 15..30.
        let mut expected: Vec<String> = (0..15).map(|i| format!("/g/{i}")).collect();
        expected.extend((15..30).map(|i| format!("/g/{i}")));
        assert_eq!(paths, expected);
    }

    #[test]
    fn prefetch_cursor_window_plan_focused_first_forward_bias() {
        // 20 entries, cursor on row 5. Plan: 5,6,7,8,9 then 4,3 (fwd NEXT=4, back PREV=2).
        let entries: Vec<BrowseEntry> = (0..20)
            .map(|i| media(&format!("g{i}"), &format!("/g/{i}"), "NES"))
            .collect();
        let plan = prefetch_cursor_window_plan(&entries, 20, 5);
        let paths: Vec<String> = plan.iter().map(|(k, _)| k.path.to_string()).collect();
        let expected: Vec<String> = vec![
            "/g/5", "/g/6", "/g/7", "/g/8", "/g/9", // fwd: row+1..row+1+NEXT
            "/g/4", "/g/3", // back: row-1 down to row-PREV (reversed)
        ]
        .into_iter()
        .map(String::from)
        .collect();
        assert_eq!(paths, expected);
    }

    #[test]
    fn prefetch_cursor_window_plan_clamps_at_boundaries() {
        // Cursor near start: no back entries beyond 0; cursor near end: forward clamped.
        let entries: Vec<BrowseEntry> = (0..20)
            .map(|i| media(&format!("g{i}"), &format!("/g/{i}"), "NES"))
            .collect();
        // Cursor at 1: back window is only row 0; fwd window is 1..6.
        let plan = prefetch_cursor_window_plan(&entries, 20, 1);
        let paths: Vec<String> = plan.iter().map(|(k, _)| k.path.to_string()).collect();
        let expected: Vec<String> = vec!["/g/1", "/g/2", "/g/3", "/g/4", "/g/5", "/g/0"]
            .into_iter()
            .map(String::from)
            .collect();
        assert_eq!(paths, expected);
        // Cursor at last row (19): no forward entries.
        let plan_tail = prefetch_cursor_window_plan(&entries, 20, 19);
        let paths_tail: Vec<String> = plan_tail.iter().map(|(k, _)| k.path.to_string()).collect();
        let expected_tail: Vec<String> = vec!["/g/19", "/g/18", "/g/17"]
            .into_iter()
            .map(String::from)
            .collect();
        assert_eq!(paths_tail, expected_tail);
    }

    #[test]
    fn prefetch_cursor_window_plan_skips_no_cover_and_folders() {
        let entries: Vec<BrowseEntry> = vec![
            media("g0", "/g/0", "NES"),
            media("g1", "/g/1", "NES"),
            folder("dir", "/g/dir"),
            {
                let mut e = media("g3", "/g/3", "NES");
                e.has_cover = false;
                e
            },
            media("g4", "/g/4", "NES"),
        ];
        // Cursor on row 0; fwd 1..5. Rows 2 (folder) and 3 (no cover) skipped.
        let plan = prefetch_cursor_window_plan(&entries, 5, 0);
        let paths: Vec<String> = plan.iter().map(|(k, _)| k.path.to_string()).collect();
        assert_eq!(paths, vec!["/g/0", "/g/1", "/g/4"]);
    }

    #[test]
    fn prefetch_cursor_window_plan_empty_for_empty_model() {
        let plan = prefetch_cursor_window_plan(&[], 0, 0);
        assert!(plan.is_empty());
    }

    #[test]
    fn detail_tags_emit_fixed_rows_with_blank_values() {
        let tags = vec![TagInfo {
            tag_type: "genre".into(),
            tag: "Platformer".into(),
            label: String::new(),
        }];
        let detail = detail_tags_from_tags(&tags);
        let rows: Vec<&str> = detail.split('\n').collect();
        assert_eq!(
            rows,
            vec![
                "Year\t",
                "Genre\tPlatformer",
                "Players\t",
                "Developer\t",
                "Publisher\t",
                "Rating\t",
            ]
        );
    }

    #[test]
    fn detail_tags_match_aliases_and_join_multiple_values() {
        let tags = vec![
            TagInfo {
                tag_type: "platform".into(),
                tag: "Arcade".into(),
                label: String::new(),
            },
            TagInfo {
                tag_type: "release_date".into(),
                tag: "1984".into(),
                label: String::new(),
            },
            TagInfo {
                tag_type: "gamegenre".into(),
                tag: "Action".into(),
                label: String::new(),
            },
            TagInfo {
                tag_type: "genre".into(),
                tag: "Shooter".into(),
                label: String::new(),
            },
        ];
        let detail = detail_tags_from_tags(&tags);
        let rows: Vec<&str> = detail.split('\n').collect();
        assert_eq!(rows[0], "Year\t1984");
        assert_eq!(rows[1], "Genre\tAction, Shooter");
    }

    #[test]
    fn detail_tags_prefer_label_for_display() {
        let tags = vec![TagInfo {
            tag_type: "developer".into(),
            tag: "nintendo".into(),
            label: "Nintendo".into(),
        }];
        let detail = detail_tags_from_tags(&tags);
        let rows: Vec<&str> = detail.split('\n').collect();
        assert_eq!(rows[3], "Developer\tNintendo");
    }

    #[test]
    fn ordered_detail_image_keys_no_preference_prepends_cover_key() {
        // No resolved_type -> no dedup (auto preference, type not yet known).
        let cover = MediaKey::new("NES", "/p/smb");
        let type_keys = vec![
            MediaKey::with_image_type("NES", "/p/smb", "boxart"),
            MediaKey::with_image_type("NES", "/p/smb", "screenshot"),
        ];
        let result = ordered_detail_image_keys(Some(cover.clone()), type_keys, None);
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].encode(), cover.encode());
        assert_eq!(result[1].image_type.as_deref(), Some("boxart"));
        assert_eq!(result[2].image_type.as_deref(), Some("screenshot"));
    }

    #[test]
    fn ordered_detail_image_keys_auto_dedup_drops_resolved_twin() {
        // When resolved_type is known and matches a type key, that key is
        // dropped so left/right browses genuinely different images.
        let cover = MediaKey::new("NES", "/p/smb");
        let type_keys = vec![
            MediaKey::with_image_type("NES", "/p/smb", "boxart"),
            MediaKey::with_image_type("NES", "/p/smb", "screenshot"),
        ];
        let result = ordered_detail_image_keys(Some(cover.clone()), type_keys, Some("boxart"));
        assert_eq!(result.len(), 2, "boxart twin must be dropped");
        assert_eq!(result[0].encode(), cover.encode());
        assert_eq!(result[1].image_type.as_deref(), Some("screenshot"));
    }

    #[test]
    fn ordered_detail_image_keys_no_cover_returns_type_keys_unchanged() {
        let type_keys = vec![
            MediaKey::with_image_type("NES", "/p/smb", "boxart"),
            MediaKey::with_image_type("NES", "/p/smb", "screenshot"),
        ];
        let result = ordered_detail_image_keys(None, type_keys.clone(), None);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].image_type.as_deref(), Some("boxart"));
        assert_eq!(result[1].image_type.as_deref(), Some("screenshot"));
    }

    #[test]
    fn ordered_detail_image_keys_empty_type_keys_yields_single_cover_key() {
        let cover = MediaKey::new("NES", "/p/smb");
        let result = ordered_detail_image_keys(Some(cover.clone()), vec![], None);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].encode(), cover.encode());
    }

    #[test]
    fn detail_image_keys_from_meta_filters_unsupported_types() {
        use zaparoo_core::media_types::MediaMeta;
        let meta = MediaMeta {
            available_image_types: vec![
                "boxart".to_string(),
                "boxart3d".to_string(), // not in CORE_SERVEABLE_IMAGE_TYPES
                "screenshot".to_string(),
                "thumbnail".to_string(), // not in CORE_SERVEABLE_IMAGE_TYPES
            ],
            ..Default::default()
        };
        let keys = detail_image_keys_from_meta(&meta, "NES", "/p/smb");
        let types: Vec<_> = keys
            .iter()
            .filter_map(|k| k.image_type.as_deref())
            .collect();
        assert!(types.contains(&"boxart"), "boxart must be included");
        assert!(types.contains(&"screenshot"), "screenshot must be included");
        assert!(!types.contains(&"boxart3d"), "boxart3d must be excluded");
        assert!(!types.contains(&"thumbnail"), "thumbnail must be excluded");
    }
}
