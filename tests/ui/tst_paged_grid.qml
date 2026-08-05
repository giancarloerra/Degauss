// Zaparoo Frontend
// Copyright (c) 2026 Wizzo Pty Ltd and the Zaparoo Project contributors.
// SPDX-License-Identifier: LicenseRef-PolyForm-Noncommercial-1.0.0

import QtQuick
import QtTest
import Zaparoo.Theme
import Zaparoo.Ui

// Direct moveSelection coverage. PagedGrid wraps in surprising ways
// (within-row Left/Right wrap, vertical page advance/retreat, partial
// last-page hole clamps), so each branch needs its own explicit case.
//
// Test geometry pinned to 1280×480 with an explicit 4×3 grid
// (pageSize=12). The production browse screens now choose rows/columns
// from viewport-aware sizing, so the test pins its shape directly and
// keeps the navigation assertions stable.
TestCase {
    id: testCase
    name: "UiPagedGrid"
    when: windowShown
    width: 1280
    height: 480
    visible: true

    Component.onCompleted: {
        Sizing.screenWidth = testCase.width;
        Sizing.screenHeight = testCase.height;
    }

    ListModel {
        id: model
    }

    Component {
        id: cellDelegate
        Item {
            property string name: ""
            property string coverKey: ""
            property bool isSelected: false
            property bool isFocused: false
            property int favorite: 0
        }
    }

    PagedGrid {
        id: grid
        anchors.fill: parent
        model: model
        delegate: cellDelegate
        columnsOverride: 4
        rowsOverride: 3
    }

    SignalSpy {
        id: loadMoreSpy
        target: grid
        signalName: "loadMoreRequested"
    }

    function fillModel(count: int): void {
        model.clear();
        for (let i = 0; i < count; i++)
            model.append({
                "name": "item-" + i,
                "coverKey": "",
                "favorite": 0
            });
        // Wait for Repeater itemCount to catch up before any test assertions.
        tryCompare(grid, "itemCount", count);
    }

    function init(): void {
        Sizing.screenWidth = testCase.width;
        Sizing.screenHeight = testCase.height;
        // Reset paginated-state knobs so a leak from a failed test
        // (which skips its cleanup) doesn't poison the next case's
        // pageCount/totalPageCount math.
        grid.hasMorePages = false;
        grid.totalItemsOverride = -1;
        fillModel(0);
        grid.setCurrentIndexImmediate(0);
        loadMoreSpy.clear();
    }

    ListModel {
        id: suspendModel
        ListElement { name: "a"; coverKey: ""; favorite: 0; hidden: false; disambiguatingTags: "" }
        ListElement { name: "b"; coverKey: ""; favorite: 0; hidden: false; disambiguatingTags: "" }
        ListElement { name: "c"; coverKey: ""; favorite: 0; hidden: false; disambiguatingTags: "" }
    }

    PagedGrid {
        id: suspendProbe
        suspendDelegates: true
        model: suspendModel
        delegate: Item {}
        columnsOverride: 3
        rowsOverride: 3
        width: 300
        height: 300
    }

    // Suspended delegates: the grid must keep reporting itemCount from
    // the model's own count with zero delegates materialized, so list
    // layout can use the grid as cursor authority without paying the
    // per-row instantiation cost on every insert.
    function test_suspended_delegates_track_model_count(): void {
        suspendProbe.model = suspendModel;
        compare(suspendProbe.itemCount, 3, "itemCount must come from model.count while suspended");
        suspendModel.append({"name": "d", "coverKey": "", "favorite": 0, "hidden": false, "disambiguatingTags": ""});
        compare(suspendProbe.itemCount, 4, "itemCount must track model growth while suspended");
    }

    function test_geometry_matches_pinned_resolution(): void {
        compare(grid.columns, 4, "expected 4 columns at 480px height");
        compare(grid.rows, 3, "expected 3 rows at 480px height");
        compare(grid.pageSize, 12);
    }

    function test_empty_model_refuses_movement(): void {
        compare(grid.itemCount, 0);
        compare(grid.moveSelection(1, 0), false);
        compare(grid.moveSelection(0, 1), false);
        compare(grid.currentIndex, 0);
    }

    function test_within_page_step_right(): void {
        fillModel(20);
        compare(grid.currentIndex, 0);
        compare(grid.moveSelection(1, 0), true);
        compare(grid.currentIndex, 1);
    }

    function test_within_page_step_down(): void {
        fillModel(20);
        compare(grid.moveSelection(0, 1), true);
        // (row 0, col 0) → (row 1, col 0) → index 4
        compare(grid.currentIndex, 4);
    }

    // ── Vertical paging (Up/Down crosses page boundaries) ───────────────

    function test_down_at_bottom_row_advances_to_next_page(): void {
        // 24 items, two full pages. From (page 0, row 2, col 0) = 8,
        // Down advances to (page 1, row 0, col 0) = 12.
        fillModel(24);
        grid.setCurrentIndexImmediate(8);
        compare(grid.moveSelection(0, 1), true);
        compare(grid.currentIndex, 12);
    }

    function test_up_at_top_row_retreats_to_previous_page(): void {
        // 24 items. From (page 1, row 0, col 0) = 12, Up retreats to
        // (page 0, row 2, col 0) = 8.
        fillModel(24);
        grid.setCurrentIndexImmediate(12);
        compare(grid.moveSelection(0, -1), true);
        compare(grid.currentIndex, 8);
    }

    function test_down_at_last_page_last_row_wraps_to_page_zero(): void {
        // 24 items. From (page 1, row 2, col 0) = 20, Down wraps to
        // (page 0, row 0, col 0) = 0.
        fillModel(24);
        grid.setCurrentIndexImmediate(20);
        compare(grid.moveSelection(0, 1), true);
        compare(grid.currentIndex, 0);
    }

    function test_up_at_page_zero_first_row_wraps_to_last_page(): void {
        // 24 items. From (page 0, row 0, col 0) = 0, Up wraps to
        // (page 1, row 2, col 0) = 20.
        fillModel(24);
        compare(grid.moveSelection(0, -1), true);
        compare(grid.currentIndex, 20);
    }

    function test_up_at_page_zero_wraps_to_partial_last_page_clamped(): void {
        // 20 items: page 1 has rows 0..1 (12..19). Up from index 0
        // would land on (page 1, row 2, col 0) = 20 — a hole. Clamp
        // to the last item on the partial last page (19).
        fillModel(20);
        compare(grid.moveSelection(0, -1), true);
        compare(grid.currentIndex, 19);
    }

    function test_down_overshoot_to_partial_page_clamps_to_last_existing(): void {
        // 13 items: page 1 has only index 12 (row 0, col 0). From
        // (page 0, row 2, col 3) = 11, Down would land on (page 1,
        // row 0, col 3) = 15 — a hole. Clamp to last item on the
        // partial page (12).
        fillModel(13);
        grid.setCurrentIndexImmediate(11);
        compare(grid.moveSelection(0, 1), true);
        compare(grid.currentIndex, 12);
    }

    function test_down_below_last_filled_row_on_partial_page_wraps_to_page_zero(): void {
        // 14 items: standing at (page 1, row 0, col 1) = 13 (the last
        // item; row 1 of this page is empty). Down advances off the
        // last filled row — same as overflowing the grid, so on the
        // last page it wraps to (page 0, row 0, same col) = 1.
        fillModel(14);
        grid.setCurrentIndexImmediate(13);
        compare(grid.moveSelection(0, 1), true);
        compare(grid.currentIndex, 1);
    }

    function test_up_from_partial_page_retreats_to_previous_page(): void {
        // 14 items: page 1 has indices 12, 13. From (page 1, row 0,
        // col 1) = 13, Up retreats to (page 0, row 2, col 1) = 9
        // (a real cell on the full prev page).
        fillModel(14);
        grid.setCurrentIndexImmediate(13);
        compare(grid.moveSelection(0, -1), true);
        compare(grid.currentIndex, 9);
    }

    // ── Single-page Up/Down (wraps within the page) ─────────────────────

    function test_single_page_up_wraps_to_last_row_same_page(): void {
        // 12 items, single full page. From (row 0, col 0) = 0,
        // Up wraps to (row 2, col 0) = 8.
        fillModel(12);
        compare(grid.pageCount, 1);
        compare(grid.moveSelection(0, -1), true);
        compare(grid.currentIndex, 8);
    }

    function test_single_page_down_at_partial_last_row_wraps_to_top(): void {
        // 6 items, single partial page. From (row 1, col 1) = 5, Down
        // steps below the last filled row, which on the only (=last)
        // page wraps to (row 0, same col) = 1. Mirrors the full-page
        // single-page Down-wrap so partial pages aren't a special case.
        fillModel(6);
        grid.setCurrentIndexImmediate(5);
        compare(grid.pageCount, 1);
        compare(grid.moveSelection(0, 1), true);
        compare(grid.currentIndex, 1);
    }

    // ── Horizontal within-row wrap (Left/Right never changes page) ──────

    function test_right_at_last_col_wraps_within_row(): void {
        // 24 items. From (page 0, row 0, col 3) = 3, Right wraps to
        // (page 0, row 0, col 0) = 0. No page change.
        fillModel(24);
        grid.setCurrentIndexImmediate(3);
        compare(grid.moveSelection(1, 0), true);
        compare(grid.currentIndex, 0);
    }

    function test_left_at_first_col_wraps_within_row(): void {
        // 24 items. From (page 0, row 0, col 0) = 0, Left wraps to
        // (page 0, row 0, col 3) = 3. No page change.
        fillModel(24);
        compare(grid.moveSelection(-1, 0), true);
        compare(grid.currentIndex, 3);
    }

    function test_right_on_partial_row_wraps_within_filled(): void {
        // 14 items: page 1 row 0 has (12, 13). From idx 13, Right
        // wraps within the row to col 0 = 12.
        fillModel(14);
        grid.setCurrentIndexImmediate(13);
        compare(grid.moveSelection(1, 0), true);
        compare(grid.currentIndex, 12);
    }

    function test_left_on_partial_row_wraps_within_filled(): void {
        // 14 items. From idx 12 (page 1, row 0, col 0), Left wraps
        // to last filled col on the row = idx 13.
        fillModel(14);
        grid.setCurrentIndexImmediate(12);
        compare(grid.moveSelection(-1, 0), true);
        compare(grid.currentIndex, 13);
    }

    function test_single_page_left_wrap_to_last_col_when_row_full(): void {
        // 6 items at 4-cols: row 0 is full (0..3), row 1 partial (4..5).
        // From idx 0, Left wraps to last col on full row 0 = idx 3.
        fillModel(6);
        compare(grid.pageCount, 1);
        compare(grid.moveSelection(-1, 0), true);
        compare(grid.currentIndex, 3);
    }

    function test_single_page_left_wrap_partial_row_clamps_to_last_item(): void {
        // 2 items at 4-cols: row 0 partial (0..1). From idx 0, Left
        // wraps to last filled col on this row (col 1) = idx 1.
        fillModel(2);
        compare(grid.pageCount, 1);
        compare(grid.moveSelection(-1, 0), true);
        compare(grid.currentIndex, 1);
    }

    function test_single_page_right_at_last_filled_wraps_to_row_start(): void {
        // 6 items. From (row 1, col 1) = 5, Right wraps within the
        // partial row to col 0 = idx 4.
        fillModel(6);
        grid.setCurrentIndexImmediate(5);
        compare(grid.moveSelection(1, 0), true);
        compare(grid.currentIndex, 4);
    }

    function test_no_movement_returns_false(): void {
        fillModel(20);
        compare(grid.moveSelection(0, 0), false);
        compare(grid.currentIndex, 0);
    }

    function test_item_count_clamp_keeps_current_in_bounds(): void {
        // Shrink the model directly (without an intermediate clear)
        // so the clamp at PagedGrid.onItemCountChanged is exercised
        // with a stale-but-valid currentIndex (not just 0).
        fillModel(20);
        grid.setCurrentIndexImmediate(19);
        model.remove(10, 10);
        tryCompare(grid, "itemCount", 10);
        compare(grid.currentIndex, 9);
    }

    // ── pageBy (L/R shoulder shortcut, unchanged) ────────────────────────

    function test_pageBy_advances_one_page(): void {
        fillModel(24);
        grid.setCurrentIndexImmediate(2); // (row 0, col 2)
        compare(grid.pageBy(1), true);
        compare(grid.currentPage, 1);
        // Preserves (row, col): (page 1, row 0, col 2) = 14.
        compare(grid.currentIndex, 14);
    }

    function test_pageBy_wraps_negative(): void {
        fillModel(24);
        compare(grid.pageBy(-1), true);
        compare(grid.currentPage, 1);
    }

    function test_pageBy_single_page_returns_false(): void {
        fillModel(6);
        compare(grid.pageCount, 1);
        compare(grid.pageBy(1), false);
        compare(grid.pageBy(-1), false);
    }

    function test_pageBy_partial_target_clamps_to_last_item(): void {
        // 14 items, currentIndex 5 (row 1, col 1) on page 0. pageBy(1)
        // targets (page 1, row 1, col 1) = 17 — a hole. Clamps to
        // last on page 1 (13).
        fillModel(14);
        grid.setCurrentIndexImmediate(5);
        compare(grid.pageBy(1), true);
        compare(grid.currentIndex, 13);
    }

    // ── Page-stack flags (gutter arrows / scrollbar derivations) ─────────

    function test_hasPages_flags_track_currentPage(): void {
        fillModel(36); // 3 pages
        grid.setCurrentIndexImmediate(0);
        compare(grid.hasPagesAbove, false);
        compare(grid.hasPagesBelow, true);
        grid.setCurrentIndexImmediate(12); // page 1
        compare(grid.hasPagesAbove, true);
        compare(grid.hasPagesBelow, true);
        grid.setCurrentIndexImmediate(24); // page 2
        compare(grid.hasPagesAbove, true);
        compare(grid.hasPagesBelow, false);
    }

    function test_hasPages_flags_single_page_dataset(): void {
        fillModel(6);
        compare(grid.pageCount, 1);
        compare(grid.hasPagesAbove, false);
        compare(grid.hasPagesBelow, false);
    }

    // ── Scroll thumb sizing (totalItemsOverride) ─────────────────────────

    function test_totalPageCount_uses_override(): void {
        // 24 items loaded — 2 pages on the 4×3 grid. With an override
        // saying total is 60 (5 pages), totalPageCount must reflect 5
        // so the scroll thumb sizes from the dataset's true total
        // rather than the loaded slice.
        fillModel(24);
        compare(grid.pageCount, 2);
        grid.totalItemsOverride = 60;
        compare(grid.totalPageCount, 5);
        grid.totalItemsOverride = -1;
        compare(grid.totalPageCount, 2);
    }

    // ── Pending wrap-target (paginated dataset, partial load) ───────────
    //
    // GamesScreen sets `totalItemsOverride` to the dataset's true entry
    // count and `hasMorePages` to `GamesModel.has_next_page`. When the
    // user wraps onto an unloaded page, PagedGrid must:
    //   - stash the (page, row, col) target,
    //   - fire `loadMoreRequested` (the screen wires this to fetch_more),
    //   - leave `currentIndex` untouched,
    //   - commit the jump on the next `itemCount` growth that covers
    //     the target,
    //   - drop the pending intent on a sideways move,
    //   - settle on the loaded last item if `hasMorePages` flips false
    //     before the target is reached.
    //
    // Tests below set `totalItemsOverride = 60` (5 pages) but seed the
    // model with 24 items (2 pages loaded) to mimic the partial-load
    // state the user repro'd on Genesis "1 US - A-F".

    function _setupPartialLoad(loaded: int, total: int): void {
        fillModel(loaded);
        grid.totalItemsOverride = total;
        grid.hasMorePages = true;
        grid.setCurrentIndexImmediate(0);
        compare(grid.pageCount, Math.ceil(loaded / grid.pageSize));
        compare(grid.totalPageCount, Math.ceil(total / grid.pageSize));
    }

    function _resetPartialLoadState(): void {
        grid.totalItemsOverride = -1;
        grid.hasMorePages = false;
    }

    function test_up_at_page_zero_unloaded_target_stashes_pending(): void {
        // 24/60 — Up at index 0 targets the dataset's last page (page 4),
        // which isn't loaded. Selection must not move; pending state set.
        _setupPartialLoad(24, 60);
        loadMoreSpy.clear();
        compare(grid.moveSelection(0, -1), false);
        compare(grid.currentIndex, 0, "selection must not move while waiting for fetch");
        compare(grid._pendingTargetPage, 4, "pending target page should be the dataset's last");
        compare(grid._pendingTargetRow, grid.rows - 1);
        compare(grid._pendingTargetCol, 0);
        verify(loadMoreSpy.count >= 1, "expected loadMoreRequested to fire at least once");
        _resetPartialLoadState();
    }

    function test_pending_target_commits_when_pages_load(): void {
        // Set up the partial-load wrap, then grow the model to cover
        // the target page. The itemCount-change handler must commit
        // the jump.
        _setupPartialLoad(24, 60);
        compare(grid.moveSelection(0, -1), false);
        compare(grid._pendingTargetPage, 4);
        // Append the rest of the dataset in one go — pageCount jumps
        // to 5, target page 4 is now loaded, jump commits.
        for (let i = 24; i < 60; i++)
            model.append({
                "name": "item-" + i,
                "coverKey": "",
                "favorite": 0
            });
        tryCompare(grid, "itemCount", 60);
        // Pending target row was rows-1 (=2), col 0; target index is
        // page 4 base (48) + row 2 * 4 + col 0 = 56.
        compare(grid.currentIndex, 56);
        compare(grid._pendingTargetPage, -1, "pending should clear after commit");
        _resetPartialLoadState();
    }

    function test_pending_target_clamps_when_target_partial_page(): void {
        // 24/50 — totalPageCount is 5 (last page partial: indices 48,49).
        // Up wraps to (page 4, row 2, col 0) = 56, which doesn't exist.
        // Selection waits while data loads; the model declares no more
        // pages once total is in, and the watchdog settles us on the
        // partial page's last existing item (49).
        fillModel(24);
        grid.totalItemsOverride = 50;
        grid.hasMorePages = true;
        grid.setCurrentIndexImmediate(0);
        compare(grid.totalPageCount, 5);
        compare(grid.moveSelection(0, -1), false);
        compare(grid._pendingTargetPage, 4);
        for (let i = 24; i < 50; i++)
            model.append({
                "name": "item-" + i,
                "coverKey": "",
                "favorite": 0
            });
        tryCompare(grid, "itemCount", 50);
        // Target slot (idx 56) doesn't exist on the partial last page,
        // so selection is still parked while we wait for "more". Real
        // models flip `has_next_page` false at the end of pagination;
        // the watchdog inside _commitPendingTarget then clamps to the
        // last loaded item on the target page.
        compare(grid.currentIndex, 0, "still pending — target slot 56 doesn't exist yet");
        grid.hasMorePages = false;
        compare(grid.currentIndex, 49, "clamp to last existing item on the partial page");
        _resetPartialLoadState();
    }

    function test_pending_target_chains_fetch_when_still_short(): void {
        // 24/60 — Up at index 0 stashes target page 4. Append only one
        // more page (12 items → 36 loaded, pageCount=3). Target still
        // unreached; the handler must fire another loadMoreRequested
        // and leave currentIndex untouched.
        _setupPartialLoad(24, 60);
        compare(grid.moveSelection(0, -1), false);
        loadMoreSpy.clear();
        for (let i = 24; i < 36; i++)
            model.append({
                "name": "item-" + i,
                "coverKey": "",
                "favorite": 0
            });
        tryCompare(grid, "itemCount", 36);
        compare(grid.currentIndex, 0, "still waiting on target page, selection unchanged");
        compare(grid._pendingTargetPage, 4);
        verify(loadMoreSpy.count >= 1, "expected another loadMoreRequested after partial append");
        _resetPartialLoadState();
    }

    function test_pending_target_settles_when_hasMorePages_clears(): void {
        // 24/60 — but the model later reports no more pages without
        // ever reaching pageCount=5. The watchdog branch in
        // _commitPendingTarget must settle on the loaded last item
        // (idx 23) so the user isn't stuck on the source cell.
        _setupPartialLoad(24, 60);
        compare(grid.moveSelection(0, -1), false);
        compare(grid._pendingTargetPage, 4);
        grid.hasMorePages = false;
        // No itemCount change is needed — the hasMorePages watcher
        // fires _commitPendingTarget itself.
        compare(grid._pendingTargetPage, -1, "pending should clear once hasMorePages goes false");
        compare(grid.currentIndex, grid.itemCount - 1, "settle on the loaded last item");
        _resetPartialLoadState();
    }

    function test_pending_target_cancels_on_horizontal_move(): void {
        // After stashing a pending wrap target, a sideways move means
        // the user changed intent — drop the pending jump.
        _setupPartialLoad(24, 60);
        grid.setCurrentIndexImmediate(1);
        compare(grid.moveSelection(0, -1), false);
        compare(grid._pendingTargetPage, 4);
        // Right step within row should clear pending.
        compare(grid.moveSelection(1, 0), true);
        compare(grid._pendingTargetPage, -1);
        _resetPartialLoadState();
    }

    function test_pending_target_clears_on_model_shrink(): void {
        // A model reset (system change, path change) shrinks itemCount.
        // The pending intent belongs to the previous dataset — drop it.
        _setupPartialLoad(24, 60);
        compare(grid.moveSelection(0, -1), false);
        compare(grid._pendingTargetPage, 4);
        model.clear();
        tryCompare(grid, "itemCount", 0);
        compare(grid._pendingTargetPage, -1);
        _resetPartialLoadState();
    }

    function test_down_past_last_loaded_stashes_pending(): void {
        // 24/60 — sit on the last filled row of the loaded slice
        // (page 1 row 2 col 0 = idx 20). Down would advance to page 2,
        // which isn't loaded. Stash, don't move.
        _setupPartialLoad(24, 60);
        grid.setCurrentIndexImmediate(20);
        loadMoreSpy.clear();
        compare(grid.moveSelection(0, 1), false);
        compare(grid.currentIndex, 20);
        compare(grid._pendingTargetPage, 2);
        compare(grid._pendingTargetRow, 0);
        compare(grid._pendingTargetCol, 0);
        verify(loadMoreSpy.count >= 1);
        _resetPartialLoadState();
    }

    function test_pageBy_past_loaded_stashes_pending(): void {
        // 24/60 — pageBy(2) targets page 2, unloaded. Stash, don't move.
        _setupPartialLoad(24, 60);
        grid.setCurrentIndexImmediate(2);
        compare(grid.pageBy(2), false);
        compare(grid.currentIndex, 2, "pageBy past loaded must not move synchronously");
        compare(grid._pendingTargetPage, 2);
        compare(grid._pendingTargetRow, 0);
        compare(grid._pendingTargetCol, 2);
        _resetPartialLoadState();
    }

    function test_loadAheadPages_default_is_two(): void {
        // The default `loadAheadPages` keeps two pages of buffer ahead
        // of the user before firing the prefetch. GamesScreen relies
        // on this default and overrides it only if the trade-off
        // changes.
        compare(grid.loadAheadPages, 2);
    }

    function test_loadAheadPages_two_fires_at_pageCount_minus_three(): void {
        // 60 items at pageSize 12 = 5 pages loaded. With
        // loadAheadPages=2, the trigger fires when `currentPage >=
        // pageCount - loadAheadPages - 1` = page 2. Start at idx 20
        // (page 1, row 2, col 0) and step Down: the move advances to
        // (page 2, row 0, col 0) = idx 24, which sits exactly on the
        // threshold — loadMoreRequested must fire so the next chunk is
        // in flight before the user reaches the loaded edge.
        fillModel(60);
        grid.hasMorePages = true;
        grid.setCurrentIndexImmediate(20);
        loadMoreSpy.clear();
        compare(grid.moveSelection(0, 1), true);
        compare(grid.currentPage, 2);
        verify(loadMoreSpy.count >= 1, "loadAheadPages=2 must fire prefetch on entering pageCount-3");
        grid.hasMorePages = false;
    }

    function test_loadAheadPages_two_does_not_fire_below_threshold(): void {
        // Same 60-item / 5-page setup. Step from idx 8 (page 0, row 2,
        // col 0) Down to idx 12 (page 1, row 0, col 0). currentPage=1
        // sits below the threshold (pageCount-3 = 2), so no prefetch
        // should fire yet.
        fillModel(60);
        grid.hasMorePages = true;
        grid.setCurrentIndexImmediate(8);
        loadMoreSpy.clear();
        compare(grid.moveSelection(0, 1), true);
        compare(grid.currentPage, 1);
        compare(loadMoreSpy.count, 0, "loadAheadPages=2 must not fire below pageCount-3 threshold");
        grid.hasMorePages = false;
    }

    function test_hasPendingTarget_tracks_pending_state(): void {
        // GamesScreen gates the "Loading more..." indicator on this
        // property so background prefetches stay silent. It must
        // mirror `_pendingTargetPage >= 0` exactly: false at rest,
        // true while a wrap/shoulder/hold-Down move is parked, false
        // once the target commits or the user changes intent.
        _setupPartialLoad(24, 60);
        compare(grid.hasPendingTarget, false, "no pending target at rest");
        compare(grid.moveSelection(0, -1), false);
        compare(grid._pendingTargetPage, 4);
        compare(grid.hasPendingTarget, true, "Up wrap to unloaded page must arm hasPendingTarget");
        for (let i = 24; i < 60; i++)
            model.append({
                "name": "item-" + i,
                "coverKey": "",
                "favorite": 0
            });
        tryCompare(grid, "itemCount", 60);
        compare(grid._pendingTargetPage, -1);
        compare(grid.hasPendingTarget, false, "commit must clear hasPendingTarget");
        _resetPartialLoadState();
    }

    // ── jumpToIndex (jump-to-letter position jump) ──────────────────────

    function test_jumpToIndex_loaded_target_lands_immediately(): void {
        // Fully loaded — a jump to any in-range index lands at once (the
        // backward-jump-to-already-loaded-letter case).
        fillModel(60);
        grid.setCurrentIndexImmediate(0);
        compare(grid.jumpToIndex(37), true);
        compare(grid.currentIndex, 37);
    }

    function test_jumpToIndex_clamps_to_last_item(): void {
        fillModel(20);
        compare(grid.jumpToIndex(999), true);
        compare(grid.currentIndex, 19);
    }

    function test_jumpToIndex_unloaded_stashes_absolute_index(): void {
        // 24/60 loaded. Jump to index 50 (unloaded): stash the ABSOLUTE
        // target (not a page/row/col decomposition), fire loadMore, leave
        // selection put. The page-wrap channel must stay clear so the two
        // can't be confused at commit time.
        _setupPartialLoad(24, 60);
        loadMoreSpy.clear();
        compare(grid.jumpToIndex(50), false);
        compare(grid.currentIndex, 0, "must not move while loading");
        compare(grid._pendingTargetIndex, 50, "stash the exact absolute target");
        compare(grid._pendingTargetPage, -1, "page-wrap channel stays clear for a jump");
        compare(grid.hasPendingJump, true);
        verify(loadMoreSpy.count >= 1, "expected loadMoreRequested to fire");
        _resetPartialLoadState();
    }

    function test_jumpToIndex_commits_exact_index_when_loaded(): void {
        // The pending jump commits on the exact absolute target index once
        // the data has loaded that far — never a page-aligned slot, never a
        // page early.
        _setupPartialLoad(24, 60);
        compare(grid.jumpToIndex(50), false);
        compare(grid._pendingTargetIndex, 50);
        for (let i = 24; i < 60; i++)
            model.append({
                "name": "item-" + i,
                "coverKey": "",
                "favorite": 0
            });
        tryCompare(grid, "itemCount", 60);
        compare(grid.currentIndex, 50, "lands on the exact target, mid-page");
        compare(grid._pendingTargetIndex, -1, "pending jump clears after commit");
        compare(grid.hasPendingJump, false);
        _resetPartialLoadState();
    }

    function test_jumpToIndex_commits_on_first_crossing_not_a_page_early(): void {
        // Grow the model one short of the target, then exactly across it.
        // The commit must wait until itemCount actually passes the target and
        // then land on the target itself — not settle a page (or any amount)
        // early while the intervening rows trickle in.
        _setupPartialLoad(24, 60);
        compare(grid.jumpToIndex(50), false);
        // Up to 50 rows loaded => target index 50 still not present
        // (indices 0..49). Must remain pending, selection unmoved.
        for (let i = 24; i < 50; i++)
            model.append({
                "name": "item-" + i,
                "coverKey": "",
                "favorite": 0
            });
        tryCompare(grid, "itemCount", 50);
        compare(grid.currentIndex, 0, "target index 50 not loaded yet — stay parked");
        compare(grid._pendingTargetIndex, 50);
        // One more row => index 50 exists; commit lands exactly there.
        model.append({
            "name": "item-50",
            "coverKey": "",
            "favorite": 0
        });
        tryCompare(grid, "itemCount", 51);
        compare(grid.currentIndex, 50);
        compare(grid._pendingTargetIndex, -1);
        _resetPartialLoadState();
    }

    function test_jumpToIndex_truncated_dataset_lands_on_nearest_loaded(): void {
        // Jump to 50, but the dataset turns out shorter: only 45 rows ever
        // arrive and the model declares no more pages. Settle on the nearest
        // loaded item (44), never a full page back to a page boundary.
        _setupPartialLoad(24, 60);
        compare(grid.jumpToIndex(50), false);
        compare(grid._pendingTargetIndex, 50);
        for (let i = 24; i < 45; i++)
            model.append({
                "name": "item-" + i,
                "coverKey": "",
                "favorite": 0
            });
        tryCompare(grid, "itemCount", 45);
        compare(grid.currentIndex, 0, "still pending — target 50 not loaded");
        grid.hasMorePages = false;
        compare(grid.currentIndex, 44, "settle on nearest loaded item, not a page early");
        compare(grid._pendingTargetIndex, -1);
        _resetPartialLoadState();
    }

    function test_jumpToIndex_pending_cleared_by_directional_move(): void {
        // A pending jump is the user's last expressed intent only until they
        // press a direction; a successful move must drop it.
        fillModel(60);
        grid.hasMorePages = true;
        grid.totalItemsOverride = 120;
        grid.setCurrentIndexImmediate(0);
        compare(grid.jumpToIndex(100), false);
        compare(grid.hasPendingJump, true);
        compare(grid.moveSelection(1, 0), true, "right step within row succeeds");
        compare(grid.hasPendingJump, false, "directional move clears the pending jump");
        compare(grid._pendingTargetIndex, -1);
        _resetPartialLoadState();
    }

    function test_hasPendingJump_false_for_page_wrap_target(): void {
        // A page-wrap (pageBy / vertical wrap) onto an unloaded page arms
        // hasPendingTarget but NOT hasPendingJump — only true letter jumps
        // bulk-load.
        _setupPartialLoad(24, 60);
        compare(grid.moveSelection(0, -1), false);
        compare(grid.hasPendingTarget, true, "page-wrap arms the generic pending flag");
        compare(grid.hasPendingJump, false, "page-wrap is not a jump");
        _resetPartialLoadState();
    }
}
