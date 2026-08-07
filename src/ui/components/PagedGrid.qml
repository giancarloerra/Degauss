// Zaparoo Frontend
// Copyright (c) 2026 Wizzo Pty Ltd and the Zaparoo Project contributors.
// SPDX-License-Identifier: LicenseRef-PolyForm-Noncommercial-1.0.0
// layoutProfile and its sub-properties (_gridProfile.leftInset etc.) are
// QVariant-typed JS objects; cannot be statically typed. Structural; suppress compiler.
// qmllint disable compiler

// Bound component behavior is required because the inner Repeater +
// Loader bind to root.* properties (delegate, focused, cellWidth, …)
// across component boundaries. Keep all enclosing-scope reads explicit
// via the file-scope `root` id so qmllint can verify them; do not
// introduce intermediate Items that rely on implicit lookups.
pragma ComponentBehavior: Bound

import QtQuick
import Zaparoo.Theme

// Paged grid of tiles. Items flow row-major within a page; pages stack
// **vertically** — pressing Down at the bottom row swaps in the next
// page instantly (same column, top row), Up at the top row swaps in
// the previous page (same column, bottom row). Crossing past the last
// page wraps to page 0; mirror for Up at page 0. Left/Right wrap
// **within the current row** and never change pages, so a partial last
// row cycles among its own filled cells.
//
// Selection is `currentIndex` over the source model; (page, row, col)
// are derived. Cells size themselves to the available container minus
// reserved chrome — callers pass a model and delegate, the grid
// handles layout. The right gutter renders a scroll indicator (up/down
// arrows + free-floating thumb) sized and positioned from
// `totalItemsOverride` (with `itemCount` fallback), so paginated
// callers like `GamesScreen` get a thumb that reflects the dataset's
// true total page count rather than the loaded slice.
//
// Page changes are instant cuts (no fade, no slide). On Qt Quick's
// Software adaptation the renderer cannot keep up with a per-frame
// alpha ramp over a busy grid — translucent overlays don't subtract
// from the dirty region, so every cell underneath re-rasterizes per
// frame (cover images, card bodies). See docs/qml-gotchas.md →
// "Software-renderer animation costs" for the full reasoning.
Item {
    id: root

    required property var model
    required property Component delegate

    property int currentIndex: 0
    // List layout never renders the grid, but the Repeater still
    // materializes one cell per model row (five role reads each) —
    // measured at roughly three milliseconds per row on the DE10, which
    // turned every large-folder reset or insert into seconds of
    // UI-thread stall. Suspending swaps the Repeater's model out and
    // serves `itemCount` straight from the model's own count property,
    // so the list cursor, page math, and jump logic keep working with
    // zero delegates alive.
    property bool suspendDelegates: false
    readonly property int itemCount: root.suspendDelegates ? (root.model && root.model.count !== undefined ? root.model.count : 0) : itemRepeater.count

    // Whether this section currently owns user focus. Tile uses this to
    // gate the selection card so only one section shows the focus cue
    // at a time on screens that host more than one tile section.
    // Defaults to true so call sites that don't care keep working
    // untouched.
    property bool focused: true
    property bool coverLoadingPaused: false
    property bool rapidRenderMode: false
    readonly property int _coverRetentionPages: Math.max(1, Math.ceil(Sizing.visibleCovers))
    // Pulse counter for the one-shot tile push-in. Callers increment via
    // pulseActivate(); TileLoader forwards the value to Tile where only the
    // focused+selected delegate fires its cue. The same cue serves both
    // forward navigation and game launch.
    property int activatePulse: 0
    function pulseActivate(): void {
        root.activatePulse++;
    }
    // Release counter for the push-in cue. Callers increment via
    // releaseActivate() to settle the focused tile back to rest after a launch
    // that keeps the frontend on the same screen (e.g. an Audio track). Forward
    // navigation never calls this — the screen transition + `settling` reset
    // handle the held scale off-screen.
    property int releasePulse: 0
    function releaseActivate(): void {
        root.releasePulse++;
    }
    // When true, Tile delegates reset their push-in scale back to 1.0
    // so a held push-in from the previous visit does not persist when
    // the screen is shown again. Set by the host screen to `!active`
    // (i.e. true while the screen is off-screen).
    property bool screenSettling: false
    // Forwarded to each Tile's focus-visibility gate. Host screens leave this
    // false until the grid's selection is finalized (restore or first input)
    // so the default tile 0 never paints a ring before restore lands; default
    // true keeps focus rendering on for hosts that do not wire it.
    property bool focusReady: true
    property var layoutProfile: null
    readonly property var _gridProfile: root.layoutProfile && root.layoutProfile.grid ? root.layoutProfile.grid : null

    // Emitted when the user is sitting on the last loaded page after a
    // selection move. Models with more data fetch the next page in
    // response; models without ignore it. The grid does not know
    // whether the model has more data — that is a model concern, kept
    // out of this component so it stays generic.
    signal loadMoreRequested(bool urgent)
    // Mouse entry points. Screens own persistence and activation side
    // effects, so the grid only updates its focused index and reports the
    // row the pointer targeted.
    signal itemHovered(int index)
    signal itemClicked(int index)
    signal itemRightClicked(int index)
    signal emptyRightClicked
    signal pageWheelRequested(int delta)

    // Per-instance shape overrides. -1 means "use the shared browse-grid
    // default". Real browse screens now override explicitly, but the
    // fallback stays useful for generic callers and tests.
    property int columnsOverride: -1
    property int rowsOverride: -1
    readonly property int columns: columnsOverride > 0 ? columnsOverride : Sizing.systemsGridColumns
    readonly property int rows: rowsOverride > 0 ? rowsOverride : Sizing.systemsGridRows

    // Pages of buffer to keep ahead of the user's current page before
    // firing `loadMoreRequested`. With `loadAheadPages: 2` the trigger
    // fires when the user enters the second-to-last loaded page,
    // overlapping the RPC + model insert with a full page of selection
    // travel so the new chunk lands before they reach the loaded edge.
    // The model's `loading_more` debounce collapses repeated emissions
    // while a fetch is in flight, so firing earlier doesn't fan out.
    property int loadAheadPages: 2
    readonly property int pageSize: columns * rows
    // Snap to the last fully-loaded page boundary while more chunks are
    // still on the way. Otherwise the user reaches a half-full trailing
    // page, sees "Loading more…", then watches the rest pop in mid-page
    // when the chunk lands. Once `hasMorePages` flips false the partial
    // last page becomes legitimate (it's the real end of the dataset)
    // and we ceil to include it. The `Math.max(1, ...)` floor guard
    // keeps the very first sub-page-sized load from collapsing pageCount
    // to 0 while the initial chunk is still landing.
    readonly property int pageCount: {
        if (itemCount <= 0)
            return 1;
        if (root.hasMorePages)
            return Math.max(1, Math.floor(itemCount / pageSize));
        return Math.ceil(itemCount / pageSize);
    }
    readonly property int currentPage: Math.floor(currentIndex / pageSize)
    readonly property int currentColumn: (currentIndex % pageSize) % columns
    readonly property int currentRow: Math.floor((currentIndex % pageSize) / columns)

    // Page-stack indicators. Used by the right-gutter scroll cue (and
    // by callers that want to drive their own indicator) to gate the
    // up/down arrows. Both are false on a single-page dataset.
    // These intentionally track loaded `pageCount` — they reflect what
    // the user can actually navigate to right now, not a paginated
    // model's reported total.
    readonly property bool hasPagesAbove: currentPage > 0
    readonly property bool hasPagesBelow: currentPage < pageCount - 1

    // Caller-supplied total item count, used by the scroll thumb so its
    // size and position reflect the full dataset rather than the loaded
    // slice. Default -1 means "fall back to itemCount" — fine for
    // non-paginated models (Systems, Categories, Recents) where the
    // loaded count IS the total. Paginated callers (GamesScreen) bind
    // this to their model's authoritative total so the thumb stays
    // stable while `fetch_more` grows the slice in the background.
    property int totalItemsOverride: -1
    readonly property int totalItems: totalItemsOverride >= 0 ? totalItemsOverride : itemCount
    readonly property int totalPageCount: Math.max(1, Math.ceil(totalItems / pageSize))

    // Caller-supplied "more pages exist" flag for paginated models.
    // Drives the pending-target watchdog: if the model says no more pages
    // are coming but the pending target still isn't loaded, we settle on
    // whatever's loaded rather than spinning forever. Non-paginated
    // callers leave this false; their pending targets always resolve
    // immediately because totalPageCount === pageCount.
    property bool hasMorePages: false

    // Pending wrap-target state. Set by Up-at-page-0, Down-past-last-
    // loaded, and pageBy when the destination page hasn't been fetched
    // yet. The grid fires `loadMoreRequested` and waits for `itemCount`
    // to grow; once the target page is loaded, it commits the move and
    // clears these. Cleared on any directional move that doesn't match
    // the pending intent (Left/Right, opposite-direction Up/Down) and on
    // model resets (itemCount shrink). -1 means "no pending jump".
    property int _pendingTargetPage: -1
    property int _pendingTargetRow: 0
    property int _pendingTargetCol: 0

    // Pending jump-to-index state. Set by `jumpToIndex` when the absolute
    // target isn't loaded yet; the grid fetches until `itemCount` passes it
    // and `_commitPendingTarget` lands on this exact index. Distinct from the
    // page/row/col wrap-target above: a jump preserves the absolute position
    // (pages stay fixed/global, the cursor lands mid-page on the true target),
    // so it must not be re-derived from page/row/col. The two are mutually
    // exclusive at any moment. -1 means "no pending jump".
    property int _pendingTargetIndex: -1

    // True while a wrap / shoulder-jump / hold-Down-past-edge / jump-to-index
    // move is stashed waiting on a fetch. Screens use this to gate the
    // "Loading more..." indicator so background prefetches stay
    // silent — the indicator only paints when the user is genuinely
    // waiting on input they've already given.
    readonly property bool hasPendingTarget: _pendingTargetPage >= 0 || _pendingTargetIndex >= 0

    // True only while a jump-to-index (letter jump) is waiting on a fetch, as
    // opposed to a page-wrap target. Lets callers bulk-load aggressively for a
    // jump (loading overlay up, target far away) while keeping ordinary
    // page-turn prefetches on the gentle trickle.
    readonly property bool hasPendingJump: _pendingTargetIndex >= 0

    // Absolute row the pending jump will land on, or -1 when no jump is
    // pending. Lets the model size its jump fetch to the gap remaining rather
    // than always pulling the full remainder.
    readonly property int pendingJumpIndex: _pendingTargetIndex

    // Clear every pending-target channel at once. Used by all the
    // "intent changed / resolved / reset" sites so neither the page/row/col
    // wrap-target nor the jump-to-index target can leak across.
    function _clearPendingTarget(): void {
        root._pendingTargetPage = -1;
        root._pendingTargetIndex = -1;
    }

    // Reserved chrome around the cell area. Vertical insets must be
    // large enough to contain the focused tile's 1.06× scale bleed
    // (~3% of cellHeight per side) — without them, top-row and
    // bottom-row tiles get clipped when focused. The scrollbar gutter
    // (`gutterWidth`) is treated as part of the grid for edge-spacing
    // purposes: it sits inboard of `rightInset` (which matches
    // `leftInset` so the cells+gutter block has equal margin on each
    // screen edge) with `gutterGap` of breathing room between the
    // rightmost cell and the gutter. `gutterGap` is intentionally
    // tighter than `cellSpacingX` — the scrollbar reads as chrome,
    // not as another cell, so a full inter-cell gap looks like wasted
    // space next to it. The gutter stays reserved on a single page
    // (just hidden) so cells don't reflow when paging activates.
    readonly property int leftInset: root._gridProfile ? root._gridProfile.leftInset : Sizing.pctW(5)
    readonly property int rightInset: root._gridProfile ? root._gridProfile.rightInset : Sizing.pctW(5)
    readonly property int gutterWidth: root._gridProfile ? root._gridProfile.gutterWidth : Sizing.pctW(3)
    readonly property int gutterGap: root._gridProfile ? root._gridProfile.gutterGap : Sizing.pctW(1.5)
    readonly property int scrollThumbWidth: root._gridProfile ? root._gridProfile.scrollThumbWidth : Sizing.pctW(1.2)
    readonly property int scrollThumbRightInset: root._gridProfile ? root._gridProfile.scrollThumbRightInset : 0
    readonly property bool scrollThumbRightAligned: root._gridProfile && root._gridProfile.scrollThumbRightAligned !== undefined ? root._gridProfile.scrollThumbRightAligned : false
    readonly property int scrollArrowSize: root._gridProfile ? root._gridProfile.scrollArrowSize : Math.min(gutterWidth, Sizing.pctH(4))
    readonly property int topInset: root._gridProfile ? root._gridProfile.topInset : Sizing.pctH(2)
    readonly property int bottomInset: root._gridProfile ? root._gridProfile.bottomInset : Sizing.pctH(2)
    readonly property int cellSpacingX: root._gridProfile ? root._gridProfile.columnGap : Sizing.pctW(3)
    readonly property int cellSpacingY: root._gridProfile ? root._gridProfile.rowGap : Sizing.pctH(4)
    readonly property int _contentWidth: root.columns * root.cellWidth + (root.columns - 1) * root.cellSpacingX
    readonly property int _scrollGutterX: root._gridProfile && root._gridProfile.gutterFollowsContentWidth ? root.leftInset + root._contentWidth + root.gutterGap : width - root.rightInset - root.gutterWidth

    // Computed cell dimensions — fill the available area, divided by
    // columns × rows. The cell area
    // also reserves `gutterGap + gutterWidth` on the right for the
    // tight-gap-then-scrollbar layout described above.
    readonly property int _availableWidth: Math.max(0, width - leftInset - rightInset - gutterGap - gutterWidth)
    readonly property int _availableHeight: Math.max(0, height - topInset - bottomInset)
    readonly property int cellWidth: Math.max(0, Math.floor((root._availableWidth - (root.columns - 1) * root.cellSpacingX) / root.columns))
    readonly property int cellHeight: Math.max(0, Math.floor((root._availableHeight - (root.rows - 1) * root.cellSpacingY) / root.rows))

    function setCurrentIndexImmediate(idx: int): void {
        root.currentIndex = idx;
    }

    function _handleWheel(wheel: WheelEvent): void {
        const amount = wheel.angleDelta.y !== 0 ? wheel.angleDelta.y : wheel.pixelDelta.y;
        if (amount === 0)
            return;
        root.pageWheelRequested(amount < 0 ? 1 : -1);
        wheel.accepted = true;
    }

    function currentCellRectIn(target: Item): rect {
        if (root.itemCount <= 0)
            return Qt.rect(0, 0, 0, 0);
        const local = root.currentIndex % root.pageSize;
        const row = Math.floor(local / root.columns);
        const col = local % root.columns;
        const p = root.mapToItem(target, root.leftInset + col * (root.cellWidth + root.cellSpacingX), root.topInset + row * (root.cellHeight + root.cellSpacingY));
        return Qt.rect(p.x, p.y, root.cellWidth, root.cellHeight);
    }

    // Jump the selection by `delta` whole pages. Wraps in both
    // directions over the dataset's `totalPageCount`, not just the
    // loaded slice, so paginated callers (Games) wrap to the true last
    // page rather than the last *loaded* page. If the target page
    // isn't loaded yet, the move is deferred via `_pendingTargetPage`:
    // the grid fires `loadMoreRequested` and commits the jump once
    // `itemCount` grows enough to cover the target. The target lands on
    // (targetPage, currentRow, currentColumn) when that slot exists;
    // on a partial last page it clamps to the last existing item.
    // Returns true if the index changed synchronously, false if a
    // pending-jump was stashed or the dataset is single-page.
    function pageBy(delta: int): bool {
        if (root.itemCount <= 0 || root.totalPageCount <= 1 || delta === 0)
            return false;
        const total = root.totalPageCount;
        // JS `%` keeps sign on negatives — normalise into [0, total).
        const targetPage = ((root.currentPage + delta) % total + total) % total;
        if (targetPage === root.currentPage)
            return false;
        if (targetPage > root.pageCount - 1) {
            // Target page hasn't been fetched yet. Stash the intent and
            // let the itemCount-change watcher commit it. A fresh page-nav
            // supersedes any pending jump-to-index.
            root._pendingTargetIndex = -1;
            root._pendingTargetPage = targetPage;
            root._pendingTargetRow = root.currentRow;
            root._pendingTargetCol = root.currentColumn;
            root.loadMoreRequested(true);
            return false;
        }
        root._clearPendingTarget();
        const targetSlot = targetPage * root.pageSize + root.currentRow * root.columns + root.currentColumn;
        const lastIdxOnPage = Math.min((targetPage + 1) * root.pageSize, root.itemCount) - 1;
        if (lastIdxOnPage < 0)
            return false;
        const newIndex = Math.min(targetSlot, lastIdxOnPage);
        if (newIndex === root.currentIndex)
            return false;
        root.currentIndex = newIndex;
        // Mirror moveSelection's pre-fetch: when we cross within
        // `loadAheadPages` of the loaded edge, kick a fetch so the next
        // page boundary lands on freshly loaded rows.
        if (root.currentPage >= root.pageCount - root.loadAheadPages - 1)
            root.loadMoreRequested(false);
        return true;
    }

    // Jump the selection to an exact absolute index over the full dataset
    // (`totalItems`), loading the intervening pages if the target hasn't been
    // fetched yet — the basis for "jump to letter". Mirrors `pageBy`'s
    // pending-target machinery but lands on the exact index rather than
    // preserving the current row/col. A target that is already loaded (e.g. a
    // backward jump to an earlier letter) lands immediately; an unrealised
    // target stashes the exact (page,row,col) slot and `_commitPendingTarget`
    // commits it once `fetch_more` has loaded that far. Returns true if the
    // index changed synchronously, false if a pending-jump was stashed.
    function jumpToIndex(targetIndex: int): bool {
        if (root.itemCount <= 0 || root.totalItems <= 0)
            return false;
        const target = Math.max(0, Math.min(targetIndex, root.totalItems - 1));
        if (target < root.itemCount) {
            // Already loaded — land immediately (no walk needed).
            root._clearPendingTarget();
            if (target !== root.currentIndex)
                root.currentIndex = target;
            if (root.currentPage >= root.pageCount - root.loadAheadPages - 1)
                root.loadMoreRequested(false);
            return true;
        }
        // Not fetched yet — stash the absolute target and load until the
        // model has grown past it, then commit on the exact index.
        root._pendingTargetPage = -1;
        root._pendingTargetIndex = target;
        root.loadMoreRequested(true);
        return false;
    }

    // Commit the pending target move once the destination slot is
    // loaded, or settle on the loaded last when the model says no more
    // pages are coming. Wired into `onItemCountChanged` so every
    // fetch-more append re-evaluates it; also fires from the
    // `hasMorePages` watcher so a final empty append still resolves
    // a chain. Waiting on the exact target index (not just `pageCount`
    // catching up) avoids an early commit while the Repeater is mid-
    // materialization: the target page may report `pageCount` reached
    // while the row/col slot itself isn't realised yet.
    function _commitPendingTarget(): void {
        // Jump-to-index: land on the exact absolute target. Pages stay
        // fixed/global; the cursor lands mid-page on the true target and
        // never a page early. No page-clamp and no settle-back fallback —
        // those only existed to cope with a slow page-at-a-time walk.
        if (root._pendingTargetIndex >= 0) {
            const want = root._pendingTargetIndex;
            if (want < root.itemCount) {
                root._clearPendingTarget();
                if (want !== root.currentIndex)
                    root.currentIndex = want;
                return;
            }
            if (root.hasMorePages) {
                // Keep loading; `fetch_more_*` is debounced model-side via
                // `loading_more`, so a redundant emit is cheap.
                root.loadMoreRequested(true);
                return;
            }
            // Dataset genuinely can't reach the target (Core revised the
            // total down, or the listing is shorter than expected). Land on
            // the nearest loaded item to the target — never a full page back.
            root._clearPendingTarget();
            const nearest = Math.min(want, root.itemCount - 1);
            if (nearest >= 0 && nearest !== root.currentIndex)
                root.currentIndex = nearest;
            return;
        }
        if (root._pendingTargetPage < 0)
            return;
        // Total may have shrunk under us (e.g. Core revised total_files
        // downward); clamp the target to whatever the dataset reports
        // now so we never overshoot.
        const totalLast = root.totalPageCount - 1;
        const targetPage = Math.min(root._pendingTargetPage, totalLast);
        if (targetPage < 0) {
            root._pendingTargetPage = -1;
            return;
        }
        const targetIdx = targetPage * root.pageSize + root._pendingTargetRow * root.columns + root._pendingTargetCol;
        if (targetIdx >= root.itemCount) {
            // Specific (page, row, col) slot not realised yet.
            if (root.hasMorePages) {
                // Keep the chain going; `fetch_more` is debounced
                // model-side via `loading_more`, so a redundant emit
                // is cheap.
                root.loadMoreRequested(true);
                return;
            }
            // Model says no more pages are coming. Settle on the
            // target page's last loaded item if it has any; otherwise
            // fall back to the dataset's overall loaded last so the
            // user's "go to end" intent isn't ignored.
            root._pendingTargetPage = -1;
            const pageStart = targetPage * root.pageSize;
            const lastLoadedOnPage = Math.min((targetPage + 1) * root.pageSize, root.itemCount) - 1;
            if (lastLoadedOnPage >= pageStart) {
                if (lastLoadedOnPage !== root.currentIndex)
                    root.currentIndex = lastLoadedOnPage;
                return;
            }
            const overall = root.itemCount - 1;
            if (overall >= 0 && overall !== root.currentIndex)
                root.currentIndex = overall;
            return;
        }
        root._pendingTargetPage = -1;
        if (targetIdx !== root.currentIndex)
            root.currentIndex = targetIdx;
    }

    // Step the selection by (dCol, dRow). Returns true if the index
    // actually moved. Cardinal moves only — diagonals (dCol and dRow
    // both nonzero) are not produced by any caller and behaviour for
    // them is undefined.
    //
    // Horizontal axis — within-row wrap, never changes page or row:
    // - dCol > 0 at the row's last filled column: wrap to col 0.
    // - dCol < 0 at column 0: wrap to the row's last filled column.
    //   On a partial last row, the "last filled column" is bounded by
    //   the row's actual item count so wraps land on real cells.
    //
    // Vertical axis — page advance/retreat with wrap and partial-page
    // clamp:
    // - dRow > 0 past the last *filled* row on the current page:
    //   advance to (next page, row 0, same col). On the last page, wrap
    //   to (page 0, row 0, same col). On a partial last page this
    //   triggers as soon as Down would step into the empty rows below
    //   the content, not just when stepping off the grid grid-shape.
    // - dRow < 0 above row 0: retreat to (previous page, last row,
    //   same col). On page 0, wrap to (last page, last row, same col).
    // - Landing on a hole on a partial target page (column doesn't
    //   exist there): clamp to the last existing item on the target
    //   page so the user always moves rather than sticking on a hole.
    function moveSelection(dCol: int, dRow: int): bool {
        if (root.itemCount <= 0)
            return false;

        let newPage = root.currentPage;
        let newRow = root.currentRow;
        let newCol = root.currentColumn;

        // Horizontal wrap stays on the source row; clamp the wrap target
        // to the row's actual filled span so a partial last row cycles
        // among its own items rather than walking through a hole.
        if (dCol !== 0) {
            // Sideways step changes the user's intent — drop any
            // pending wrap-target / jump we were waiting on.
            root._clearPendingTarget();
            const rowFirstIndex = root.currentPage * root.pageSize + root.currentRow * root.columns;
            const rowLastIndex = Math.min(root.itemCount - 1, rowFirstIndex + root.columns - 1);
            const maxColOnRow = rowLastIndex - rowFirstIndex;
            const colCandidate = root.currentColumn + dCol;
            if (colCandidate < 0)
                newCol = maxColOnRow;
            else if (colCandidate > maxColOnRow)
                newCol = 0;
            else
                newCol = colCandidate;
        }

        // Vertical wrap crosses page boundaries against the dataset's
        // `totalPageCount`, not the loaded slice. When the destination
        // page hasn't been fetched yet (Up at page 0 with partial load,
        // Down past last-loaded but more pages exist), stash a
        // pending-target jump and ask the model to fetch — the
        // `onItemCountChanged` handler commits the jump once the page
        // lands. The partial-page hole clamp below covers the column-
        // doesn't-exist-on-destination case once the data is present.
        //
        // A Down step that lands in an empty row of a partial last
        // page (rowCandidate fits inside `rows` but is past the page's
        // last filled row) must trigger the same page advance as
        // stepping off the grid — otherwise the hole-clamp at the end
        // of this function lands on the user's current cell and the
        // press appears to do nothing.
        if (dRow !== 0) {
            const rowCandidate = root.currentRow + dRow;
            const itemsOnPage = Math.min(root.pageSize, root.itemCount - root.currentPage * root.pageSize);
            const lastFilledRowOnPage = Math.floor((itemsOnPage - 1) / root.columns);
            if (rowCandidate < 0) {
                const targetPage = root.currentPage === 0 ? root.totalPageCount - 1 : root.currentPage - 1;
                if (targetPage > root.pageCount - 1) {
                    root._pendingTargetIndex = -1;
                    root._pendingTargetPage = targetPage;
                    root._pendingTargetRow = root.rows - 1;
                    root._pendingTargetCol = root.currentColumn;
                    root.loadMoreRequested(true);
                    return false;
                }
                newPage = targetPage;
                newRow = root.rows - 1;
            } else if (rowCandidate >= root.rows || rowCandidate > lastFilledRowOnPage) {
                const lastPage = root.totalPageCount - 1;
                const targetPage = root.currentPage === lastPage ? 0 : root.currentPage + 1;
                if (targetPage > root.pageCount - 1) {
                    root._pendingTargetIndex = -1;
                    root._pendingTargetPage = targetPage;
                    root._pendingTargetRow = 0;
                    root._pendingTargetCol = root.currentColumn;
                    root.loadMoreRequested(true);
                    return false;
                }
                newPage = targetPage;
                newRow = 0;
            } else {
                newRow = rowCandidate;
            }
        }

        let newIndex = newPage * root.pageSize + newRow * root.columns + newCol;
        if (newIndex < 0)
            return false;
        if (newIndex >= root.itemCount) {
            // Target slot is a hole on a partial page. Clamp to the
            // page's last existing item.
            const lastIdxOnPage = Math.min((newPage + 1) * root.pageSize, root.itemCount) - 1;
            if (lastIdxOnPage < 0)
                return false;
            newIndex = lastIdxOnPage;
        }
        if (newIndex === root.currentIndex) {
            // Selection didn't move because the user is at an edge with
            // no data beyond it on this side. If they're within
            // `loadAheadPages` of the loaded edge, ask the model to
            // fetch more so a subsequent press can land on freshly-
            // loaded rows.
            if (root.currentPage >= root.pageCount - root.loadAheadPages - 1)
                root.loadMoreRequested(false);
            return false;
        }
        // Successful directional move clears any pending wrap-target / jump;
        // the user is no longer waiting on it.
        root._clearPendingTarget();
        root.currentIndex = newIndex;
        // Pre-fetch early - when the user enters within `loadAheadPages`
        // of the loaded edge, kick off the next fetch so the network
        // round-trip and model insert overlap with selection travel,
        // and the new chunk lands before they cross the boundary. The
        // model's own debounce (`loading_more` guard) collapses
        // repeated emissions while a fetch is in flight.
        if (root.currentPage >= root.pageCount - root.loadAheadPages - 1)
            root.loadMoreRequested(false);
        return true;
    }

    // Defensive clamp on shrinkage only: if the model shed rows below
    // the saved index, keep us in-bounds. Don't clamp on growth — page
    // appends from cumulative pagination must leave the user's
    // currentIndex untouched.
    property int _previousItemCount: 0

    // If `hasMorePages` flips false while a pending target is still
    // ahead of the loaded slice, the watchdog branch in
    // `_commitPendingTarget` settles us on the loaded last item.
    // itemCount-change usually fires this path first, but this handler
    // covers the case where the flag is updated without an item delta
    // (e.g. a final empty append).
    onHasMorePagesChanged: {
        if (root.hasPendingTarget)
            root._commitPendingTarget();
    }

    onItemCountChanged: {
        if (root.itemCount < root._previousItemCount) {
            // Model shed rows (reset, system change, path change). The
            // pending-target context no longer applies — drop it before
            // the row-count check below moves currentIndex.
            root._clearPendingTarget();
            if (root.currentIndex >= root.itemCount)
                root.currentIndex = Math.max(0, root.itemCount - 1);
        } else if (root.itemCount > root._previousItemCount) {
            // Pages were appended. If a wrap-target jump is pending,
            // try to commit it now; the helper chains another
            // `loadMoreRequested` if the target is still ahead, or
            // settles on the loaded last item if `hasMorePages`
            // says no more pages are coming.
            root._commitPendingTarget();
        }
        root._previousItemCount = root.itemCount;
    }

    clip: true

    MouseArea {
        anchors.fill: parent
        hoverEnabled: true
        acceptedButtons: Qt.RightButton
        onClicked: root.emptyRightClicked()
        onWheel: wheel => root._handleWheel(wheel)
    }

    Item {
        id: track

        // One page wide. Cells whose `cellPage !== root.currentPage`
        // gate themselves invisible; nothing slides or scales.
        anchors.fill: parent

        Repeater {
            id: itemRepeater

            model: root.suspendDelegates ? null : root.model

            Item {
                id: cellItem

                required property int index
                required property string name
                // Every Browse model exposes `coverKey` — the relative path
                // under `resources/images/` without extension (e.g.
                // `systems/snes`, `categories/Consoles`). Tile resolves an
                // embedded PNG from the key or shows the procedural
                // fallback with `name` rendered large.
                required property string coverKey
                required property int favorite
                required property bool hidden
                // Newline-joined disambiguating-tag tokens (empty for models
                // without variants). Every Browse model exposes this role.
                required property string disambiguatingTags

                readonly property int cellPage: Math.floor(index / root.pageSize)
                readonly property int cellLocal: index % root.pageSize
                readonly property int cellRow: Math.floor(cellLocal / root.columns)
                readonly property int cellCol: cellLocal % root.columns
                readonly property bool isSelected: index === root.currentIndex

                // Cover-decode gate AND delegate-materialization gate.
                // PagedGrid's Repeater creates one cellItem per model
                // row at construction. Two-tier gate, both anchored on
                // distance from `root.currentPage`:
                //
                //   - decode range (current + next page): cells hand
                //     their real coverKey to Tile, forcing hidden
                //     next-page Image decode/QPixmapCache warm before
                //     the page cut. Rust still owns byte-fetch priority
                //     via `prefetch_around`; this range only makes QML
                //     consume already-warmed bytes early enough.
                //   - retention range (±5 pages): cells inside this
                //     radius keep their TileLoader.active=true so the
                //     Tile delegate stays materialized, AND cells that
                //     have already requested keep their coverKey set
                //     so Tile's Image keeps the decoded texture
                //     referenced. The active gate is what prevents
                //     per-press binding cost from growing with the
                //     dataset - only ~110 Tile delegates exist at any
                //     time regardless of N. Retention doesn't trigger
                //     new cover requests; only the decode range does.
                //
                // Off-radius cells (outside retention) set
                // `TileLoader.active=false`, which destroys the
                // loaded Tile delegate (Image, name Text, focus ring,
                // favorite indicator) and detaches its binding tree.
                // Cells inside retention but never requested keep the
                // delegate alive but with coverKey="", so the cover
                // collapses to the procedural fallback and the
                // texture reference drops.
                //
                // Memory ceiling tracks visible cover density: ±
                // _coverRetentionPages around currentPage keeps enough
                // decoded pages warm for the current UI scale. Re-decode
                // after crossing past the retention edge runs at
                // nice +10 (see media_image_provider.cpp) and is
                // invisible to the renderer.
                readonly property bool _coverInRange: !root.rapidRenderMode && cellPage >= root.currentPage && cellPage <= root.currentPage + 1
                readonly property bool _coverInRetentionRange: !root.rapidRenderMode && Math.abs(cellPage - root.currentPage) <= (root.coverLoadingPaused ? 1 : root._coverRetentionPages)
                property bool _coverEverRequested: false
                Binding on _coverEverRequested {
                    when: cellItem._coverInRange
                    value: true
                    restoreMode: Binding.RestoreNone
                }
                readonly property string _gatedCoverKey: (_coverInRange || (_coverEverRequested && _coverInRetentionRange)) ? coverKey : ""

                width: root.cellWidth
                height: root.cellHeight
                x: root.leftInset + cellCol * (root.cellWidth + root.cellSpacingX)
                y: root.topInset + cellRow * (root.cellHeight + root.cellSpacingY)
                // Selected tile draws on top so its scale-up tween isn't
                // clipped by neighbours below/right of it.
                z: isSelected ? 1 : 0
                visible: cellPage === root.currentPage

                // Card-shaped placeholder painted behind the
                // TileLoader. When the loader's `active` is false
                // (cell is outside the retention window) or the Tile
                // is still incubating asynchronously after a
                // retention-edge crossing, the user sees this flat
                // card slot instead of an empty pit. Once the Tile
                // finishes incubating it paints opaque on top with
                // the same color and radius, so the silhouette is
                // hidden for free without an explicit visibility
                // gate. The only selection-dependent work here is the
                // focused placeholder ring below; it stays limited to
                // the current page by the parent visibility gate.
                Rectangle {
                    id: placeholderCard

                    anchors.fill: parent
                    radius: Sizing.cornerRadius
                    color: Theme.surfaceCard
                    border.color: Theme.borderMid
                    border.width: Sizing.stroke(1)
                    // Track the loaded Tile's push-in scale so the card
                    // silhouette and the Tile surface shrink together.
                    // Falls back to 1.0 when no Tile is loaded (skeleton
                    // state), so the placeholder stays full-size until
                    // the Tile appears. `cardScale` is a readonly alias
                    // for `_activateScale` exposed by Tile for exactly
                    // this cross-sibling binding.
                    transformOrigin: Item.Center
                    // qmllint disable missing-property
                    scale: tileLoader.status === Loader.Ready && tileLoader.item ? tileLoader.item.cardScale : 1.0
                    // qmllint enable missing-property
                }

                // Standalone selected-cell ring for skeleton/rapid mode.
                // Tile.qml owns the normal ring, but rapidRenderMode
                // deliberately disables TileLoader to keep held d-pad
                // navigation cheap. Draw the same filled-rect ring on
                // the placeholder so selection never disappears while
                // covers/delegates are paused.
                Rectangle {
                    id: placeholderFocusRingOuter

                    anchors.fill: parent
                    anchors.margins: Sizing.pctH(0.4)
                    color: Theme.focusContainer
                    radius: Math.max(0, Sizing.cornerRadius - Sizing.pctH(0.4))
                    antialiasing: true
                    visible: cellItem.isSelected && root.focused && root.focusReady && (root.rapidRenderMode || tileLoader.status !== Loader.Ready)
                }

                Rectangle {
                    anchors.fill: placeholderFocusRingOuter
                    anchors.margins: Sizing.stroke(Sizing.pctH(0.6))
                    color: placeholderCard.color
                    radius: Math.max(0, placeholderFocusRingOuter.radius - Sizing.stroke(Sizing.pctH(0.6)))
                    antialiasing: true
                    visible: placeholderFocusRingOuter.visible
                }

                TileLoader {
                    id: tileLoader

                    anchors.fill: parent
                    sourceComponent: root.delegate
                    // Bound delegate materialization to the retention
                    // window. Cells outside +/-5 pages keep their
                    // cellItem (Repeater contract - it owns one item
                    // per model row) but don't construct a Tile, so
                    // the loaded delegate's binding tree (cover Image,
                    // focus ring, name Text, favorite indicator)
                    // doesn't fan out on every selection move. With
                    // ~110 active tiles instead of N, per-press
                    // binding cost stays roughly constant as the
                    // dataset grows.
                    active: cellItem._coverInRetentionRange && !root.rapidRenderMode
                    // Current-page delegates complete synchronously so
                    // tile content appears with the page instead of
                    // revealing icon/logo Images one-by-one as the
                    // Loader incubates across frames. Off-page retained
                    // delegates still incubate asynchronously so
                    // retention-edge warmup does not block input.
                    asynchronous: cellItem.cellPage !== root.currentPage
                    isSelected: cellItem.isSelected
                    isFocused: root.focused
                    name: cellItem.name
                    coverKey: cellItem._gatedCoverKey
                    favorite: cellItem.favorite
                    hidden: cellItem.hidden
                    disambiguatingTags: cellItem.disambiguatingTags
                    activatePulse: root.activatePulse
                    releasePulse: root.releasePulse
                    settling: root.screenSettling
                    focusReady: root.focusReady
                }

                MouseArea {
                    anchors.fill: parent
                    hoverEnabled: true
                    acceptedButtons: Qt.LeftButton | Qt.RightButton
                    cursorShape: Qt.PointingHandCursor
                    enabled: cellItem.visible

                    onEntered: {
                        if (root.currentIndex !== cellItem.index)
                            root.currentIndex = cellItem.index;
                        root.itemHovered(cellItem.index);
                    }

                    onClicked: mouse => {
                        if (root.currentIndex !== cellItem.index)
                            root.currentIndex = cellItem.index;
                        if (mouse.button === Qt.RightButton)
                            root.itemRightClicked(cellItem.index);
                        else
                            root.itemClicked(cellItem.index);
                    }

                    onWheel: wheel => root._handleWheel(wheel)
                }
            }
        }
    }

    // ── Right-gutter scroll indicator ────────────────────────────────────
    // Up arrow at the top, down arrow at the bottom, free-floating thumb
    // in between. No painted track — the indicator is just the arrows
    // and the thumb. Hidden when the dataset fits on a single page.
    // The thumb snaps to each page position with no glide animation:
    // scrolling is a hot path (cover prefetch + data load run on the same
    // frames), so an animated thumb would repaint while the system is
    // already busy.
    // Sits after the cell `track` in the visual tree so the indicator
    // paints on top of any brief scale-animation bleed at the right edge.
    Item {
        id: scrollGutter

        x: root._scrollGutterX
        anchors.top: parent.top
        anchors.topMargin: root.topInset
        anchors.bottom: parent.bottom
        anchors.bottomMargin: root.bottomInset
        width: root.gutterWidth
        visible: root.totalPageCount > 1

        Image {
            id: upArrow
            source: Resources.iconUrl("ScrollUp")
            width: root.scrollArrowSize
            height: root.scrollArrowSize
            anchors.top: parent.top
            anchors.horizontalCenter: parent.horizontalCenter
            fillMode: Image.PreserveAspectFit
            smooth: true
            visible: root.hasPagesAbove

            MouseArea {
                anchors.fill: parent
                acceptedButtons: Qt.LeftButton
                cursorShape: Qt.PointingHandCursor
                enabled: upArrow.visible
                onClicked: root.pageWheelRequested(-1)
            }
        }

        Image {
            id: downArrow
            source: Resources.iconUrl("ScrollDown")
            width: root.scrollArrowSize
            height: root.scrollArrowSize
            anchors.bottom: parent.bottom
            anchors.horizontalCenter: parent.horizontalCenter
            fillMode: Image.PreserveAspectFit
            smooth: true
            visible: root.hasPagesBelow

            MouseArea {
                anchors.fill: parent
                acceptedButtons: Qt.LeftButton
                cursorShape: Qt.PointingHandCursor
                enabled: downArrow.visible
                onClicked: root.pageWheelRequested(1)
            }
        }

        // Geometry-only Item between the arrows; nothing paints.
        // `scrollThumb` derives its size and position from this region's
        // height and the grid's `totalPageCount` / `currentPage`.
        Item {
            id: scrollRegion
            anchors.top: parent.top
            anchors.topMargin: root.scrollArrowSize + Sizing.pctH(1)
            anchors.bottom: parent.bottom
            anchors.bottomMargin: root.scrollArrowSize + Sizing.pctH(1)
            anchors.right: root.scrollThumbRightAligned ? parent.right : undefined
            anchors.rightMargin: root.scrollThumbRightAligned ? root.scrollThumbRightInset : 0
            anchors.horizontalCenter: root.scrollThumbRightAligned ? undefined : parent.horizontalCenter
            width: root.scrollThumbWidth

            // Standard paginated-scrollbar formulas (cf. Qt
            // `ScrollBar.size`/`position`, GTK `Gtk.Scrollbar`, Apple
            // HIG): thumb length = trackLen * (visible / total) with one
            // page visible at a time, position = (page / (total - 1)) *
            // remaining range. Floor on thumb height keeps it visible
            // when `totalPageCount` is large.
            readonly property int _minThumbHeight: Sizing.pctH(4)
            readonly property int _thumbHeight: Math.max(_minThumbHeight, Math.round(scrollRegion.height / root.totalPageCount))
            readonly property int _thumbY: root.totalPageCount <= 1 ? 0 : Sizing.px((root.currentPage / (root.totalPageCount - 1)) * (scrollRegion.height - _thumbHeight))

            Rectangle {
                id: scrollThumb
                width: root.scrollThumbWidth
                height: scrollRegion._thumbHeight
                anchors.right: root.scrollThumbRightAligned ? parent.right : undefined
                anchors.horizontalCenter: root.scrollThumbRightAligned ? undefined : parent.horizontalCenter
                y: scrollRegion._thumbY
                color: Theme.textPrimary
                radius: Sizing.half(width)
            }
        }
    }
}
