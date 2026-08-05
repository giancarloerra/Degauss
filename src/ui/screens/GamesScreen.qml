// Zaparoo Frontend
// Copyright (c) 2026 Wizzo Pty Ltd and the Zaparoo Project contributors.
// SPDX-License-Identifier: LicenseRef-PolyForm-Noncommercial-1.0.0

import QtQuick
import Zaparoo.Theme
import Zaparoo.Ui
import Zaparoo.Browse as Browse

// cxx-qt 0.8 patches `isFinal: true` on singleton properties but the
// qmltypes schema has no `isFinal` slot for Method, so every qinvokable
// call on a Zaparoo.Browse singleton (path_at, set_system, etc.) still
// trips qmllint's "Member can be shadowed" check. Until the schema grows
// method-level finality, suppress the compiler category file-wide.
// qmllint disable compiler

// Games screen — paged grid driven by `Browse.GamesModel`. Games keeps
// the folder-navigation, back-stack, and paging semantics that differ
// from the flat history lists, but it now reuses the shared
// `MediaListScreen` shell for the common list/detail rendering,
// focused-detail policy, and input plumbing.
MediaListScreen {
    id: games

    property alias gamesGrid: games.mediaGrid

    readonly property bool _portraitNonCrtList: !Theme.crtNativePath && Browse.Settings.current_orientation !== "horizontal"
    readonly property int _listPageSize: games._portraitNonCrtList ? 16 : 10
    // How many list pages before the loaded edge `_prefetchListTail`
    // starts a fetch. The background fill below does the heavy lifting;
    // this only covers the gap when the user is outrunning it and the
    // next fill tick has not fired yet.
    readonly property int _listPrefetchPages: 5
    readonly property bool _tateOrientation: Browse.Settings.current_orientation !== "horizontal"
    readonly property var _gridShape: Sizing.gamesGridShape(games._gridViewportWidth, games._gridViewportHeight)
    readonly property int _gridColumns: games._gridShape.columns
    readonly property int _gridRows: games._gridShape.rows
    readonly property int _browsePageSize: games._listLayout ? Math.max(1, games.listCard.visibleRowCount) : games.gamesGrid.pageSize
    readonly property bool _crtGridLayout: Theme.crtNativePath && !games._listLayout
    readonly property bool _tateListLayout: games._tateOrientation
    readonly property string _gridViewId: "gamesGrid"
    readonly property string _listViewId: "gamesList"
    readonly property string _tateListViewId: "gamesListTate"
    readonly property string _activeViewId: games._listLayout ? (games._tateListLayout ? games._tateListViewId : games._listViewId) : games._gridViewId
    readonly property string _browseThemeId: BrowseLayouts.currentThemeId
    readonly property var _gridProfile: BrowseLayouts.themeProfile(games._browseThemeId, games._gridViewId)
    readonly property var _viewProfile: BrowseLayouts.themeProfile(games._browseThemeId, games._activeViewId)
    readonly property var _headerProfile: games._viewProfile && games._viewProfile.header ? games._viewProfile.header : null
    readonly property var _statusProfile: games._viewProfile && games._viewProfile.status ? games._viewProfile.status : null
    readonly property var _footerProfile: games._gridProfile && games._gridProfile.footer ? games._gridProfile.footer : null

    mediaModel: Browse.GamesModel
    emptyText: qsTr("No games in this system")
    loadingText: qsTr("Loading games…")
    totalItemsOverride: Browse.GamesModel.total_dirs + Browse.GamesModel.total_files
    targetVisibleRowCount: games._listPageSize
    detailShowDescription: false
    detailShowTitle: false
    detailLoadingText: qsTr("Loading game…")
    pauseCoverRequestsDuringRapid: true
    detailIdentityForIndex: function (index) {
        if (!Browse.GamesModel.is_media_capable_at(index))
            return "";
        const systemId = Browse.GamesModel.system_id_at(index);
        const path = Browse.GamesModel.path_at(index);
        return systemId !== "" && path !== "" ? systemId + "\n" + path : "";
    }
    loadDetailForIndex: index => Browse.GamesModel.load_description_at(index)
    peekDetailForIndex: index => Browse.GamesModel.peek_description_at(index)
    clearDetailAction: () => Browse.GamesModel.clear_current_detail()
    restoreSelectionPath: () => {
        const selected = Browse.GamesState.selected_at_level;
        return selected.length > 0 ? selected[selected.length - 1] : "";
    }
    persistSelectionPath: path => games._scheduleSelectedPersist(path)
    gridMoveAction: (dx, dy) => games._performGridMove(dx, dy)
    linearMoveAction: delta => games._performLinearMove(delta)
    pageAction: delta => games._performPage(delta)
    onListLayoutEntered: () => games._fillListPage()
    gridViewId: games._gridViewId
    listViewId: games._listViewId
    tateListViewId: games._tateListViewId
    contextMenuEnabledAt: index => Browse.GamesModel.is_media_capable_at(index)
    retryAction: () => {
        if (games._atFolderLevel()) {
            const stack = Browse.GamesState.path_stack;
            const top = stack[stack.length - 1];
            Browse.GamesModel.set_path(top);
            return;
        }
        const sid = Browse.GamesModel.current_system_id;
        if (sid !== "")
            Browse.GamesModel.set_system(sid);
    }
    acceptAction: index => {
        // Debounce the whole accept path: while a prior Accept's deferred
        // leg is still pending, ignore repeats so neither the folder nor the
        // launch branch re-fires the cue or reassigns the pending target.
        if (pressCommit.running)
            return;
        const entryType = Browse.GamesModel.entry_type_at(index);
        if ((entryType === "directory" || entryType === "root") && !Browse.GamesModel.is_media_capable_at(index)) {
            // Persist synchronously (MiSTer may be killed at any time), then
            // play the cue and defer the navigation signal so the push-in
            // completes on a static scene before the model reload starts.
            games.flushSelectedPersist();
            games.pulseActivate();
            pressCommit._folderPath = Browse.GamesModel.path_at(index);
            pressCommit.arm();
            return;
        }
        // Launch is dispatched immediately so the run command races the
        // push-in cue rather than waiting it out; the deferred leg only
        // settles the tile back to rest. State persistence is synchronous.
        games._scheduleSelectedPersist(Browse.GamesModel.path_at(index));
        games.flushSelectedPersist();
        games.pulseActivate();
        Browse.GamesModel.launch_at(index);
        pressCommit._launchIndex = index;
        pressCommit.arm();
    }
    cancelAction: () => {
        // Disarm any pending accept so a press-then-back inside the deferred
        // window cannot launch/navigate after the user has backed out.
        pressCommit.stop();
        games.flushSelectedPersist();
        if (games._atFolderLevel())
            games.requestNavigateOutOfFolder();
        else
            games.requestSystemsScreen();
    }
    showTopStrip: games._statusProfile ? games._statusProfile.topStripVisible : true
    topStripTitleProvider: () => {
        const sid = Browse.GamesModel.current_system_id;
        if (sid === "")
            return "";
        const idx = Browse.SystemsModel.index_for_system_id(sid);
        return idx >= 0 ? Browse.SystemsModel.system_name_at(idx) : sid;
    }
    topStripCurrentPageProvider: () => Math.floor(games.gamesGrid.currentIndex / games._browsePageSize)
    topStripTotalPagesProvider: () => games._footerProfile && games._footerProfile.bottomStatusVisible ? 1 : Math.max(1, Math.ceil((Browse.GamesModel.total_dirs + Browse.GamesModel.total_files) / games._browsePageSize))
    topStripTotalTextProvider: () => games._listLayout || (games._footerProfile && games._footerProfile.bottomStatusVisible) ? "" : (Browse.GamesModel.total_files > 0 ? qsTr("%1 files").arg(Browse.GamesModel.total_files) : "")
    topStripRightTextProvider: () => {
        if (!games._listLayout)
            return "";
        // The background fill keeps `loading_more` true for most of a
        // large folder's first seconds, so the banner only shows when
        // the user is actually near the loaded edge waiting on rows;
        // everywhere else the position readout keeps priority.
        if (Browse.GamesModel.loading_more && games.gamesGrid.currentIndex >= games.gamesGrid.itemCount - games._browsePageSize)
            return qsTr("Loading more…");
        if (games.gamesGrid.itemCount <= 0)
            return "";
        const total = Math.max(1, Browse.GamesModel.total_dirs + Browse.GamesModel.total_files);
        return qsTr("%1 / %2").arg(games.gamesGrid.currentIndex + 1).arg(total);
    }
    gridBottomMargin: games._footerProfile ? games._footerProfile.gridBottomMargin : (Sizing.pctH(8) + Sizing.pctH(7))
    gridColumnsOverride: games._gridColumns
    gridRowsOverride: games._gridRows
    gridTotalItemsOverride: Browse.GamesModel.total_dirs + Browse.GamesModel.total_files
    gridHasMorePages: Browse.GamesModel.has_next_page
    gridLoadMoreAction: urgent => {
        // A letter jump bulk-loads to the target in one shot (overlay is up);
        // page-wrap targets and fast-scroll stay on the rapid trickle, ordinary
        // prefetch on the gentle one.
        if (games.gamesGrid.hasPendingJump)
            Browse.GamesModel.fetch_more_jump(games.gamesGrid.pendingJumpIndex);
        else if (urgent || games.detailRapidScrollActive)
            Browse.GamesModel.fetch_more_rapid();
        else
            Browse.GamesModel.fetch_more();
    }
    gridCurrentPageChangedAction: () => {
        const first = games.gamesGrid.currentPage * games.gamesGrid.pageSize;
        Browse.GamesModel.visible_first_row = first;
        if (!games._listLayout)
            Browse.GamesModel.prefetch_around(first);
    }
    activeLabelTextProvider: () => games.gamesGrid.itemCount > 0 ? Browse.GamesModel.name_at(games.gamesGrid.currentIndex) : ""
    activeLabelAtBottom: true
    activeLabelBottomMargin: games._footerProfile ? games._footerProfile.activeLabelBottomMargin : Sizing.pctH(8)
    activeLabelHeight: games._footerProfile ? games._footerProfile.activeLabelHeight : Sizing.pctH(7)
    showBottomStatusRow: games._footerProfile ? games._footerProfile.bottomStatusVisible : false
    bottomStatusLeftMargin: games._footerProfile ? games._footerProfile.bottomStatusLeftMargin : 0
    bottomStatusRightMargin: games._footerProfile ? games._footerProfile.bottomStatusRightMargin : 0
    bottomStatusLeftText: games._footerProfile && games._footerProfile.bottomStatusVisible && Browse.GamesModel.total_files > 0 ? qsTr("%1 files").arg(Browse.GamesModel.total_files) : ""
    bottomStatusRightText: games._footerProfile && games._footerProfile.bottomStatusVisible && Math.ceil((Browse.GamesModel.total_dirs + Browse.GamesModel.total_files) / games._browsePageSize) > 1 ? qsTr("%1 / %2").arg(Math.floor(games.gamesGrid.currentIndex / games._browsePageSize) + 1).arg(Math.max(1, Math.ceil((Browse.GamesModel.total_dirs + Browse.GamesModel.total_files) / games._browsePageSize))) : ""
    pageLoadingVisible: !games._listLayout && Browse.GamesModel.loading_more && games.gamesGrid.hasPendingTarget
    pageLoadingLeftMargin: games._footerProfile && games._footerProfile.bottomStatusVisible && games.bottomStatusLeftText !== "" ? Sizing.px(games.width / 3) : games.gamesGrid.leftInset

    Binding {
        target: Browse.GamesModel
        property: "cover_key_roles_enabled"
        value: !games._listLayout
    }

    // Emitted when the user presses Escape — Main.qml flips the
    // active screen back to SystemsScreen (one peer up the back-stack;
    // a second Escape from there pops to Hub).
    signal requestSystemsScreen

    // Emitted when the user accepts a directory or root entry — Main.qml
    // pushes the level onto GamesState and drives the model into the new
    // path. Stays inside the games screen (no peer flip).
    signal requestNavigateIntoFolder(string path)

    // Emitted when the user cancels from a deeper folder level — Main.qml
    // pops one level off the stack and rebrowses the parent.
    signal requestNavigateOutOfFolder

    // Persist debounce. Writing `state.toml` is an atomic
    // write+sync_all+rename (`rust/zaparoo-core/src/persist.rs`); on
    // MiSTer's SD card that's a real disk hit, and the hold-repeat tick
    // in `Main.qml` (`_repeatTickMs = 90`) means a long Down-hold would
    // fire ~11 of these per second. We coalesce them: each move stamps
    // `_pendingSelectedPath` and restarts the timer; the final flush
    // lands on hold release / Accept / Cancel via the helpers below
    // (`Main.qml`'s `_stopRepeat` calls `flushSelectedPersist`). The
    // 250 ms interval is shorter than a deliberate single tap → hold
    // gap, so isolated presses still persist quickly.
    property string _pendingSelectedPath: ""

    DeferredAction {
        id: pressCommit
        property string _folderPath: ""
        property int _launchIndex: -1
        onDeferred: {
            if (_folderPath !== "") {
                const p = _folderPath;
                _folderPath = "";
                games.requestNavigateIntoFolder(p);
            } else if (_launchIndex >= 0) {
                _launchIndex = -1;
                // Settle the push-in back to rest. Invisible when the launch
                // takes the FPGA or kills us; prevents a stuck pushed-in tile
                // when the launcher stays on the page (e.g. an Audio track).
                games.releaseActivate();
            }
        }
    }

    Timer {
        id: persistDebounce
        interval: 250
        repeat: false
        onTriggered: {
            if (games._pendingSelectedPath !== "")
                Browse.GamesState.set_selected_at_top(games._pendingSelectedPath);
            games._pendingSelectedPath = "";
        }
    }

    function _scheduleSelectedPersist(path: string): void {
        games._pendingSelectedPath = path;
        persistDebounce.restart();
    }

    // Force any pending persist to land synchronously. Called from the
    // Accept path (we hand control off to launch and the kill-resume
    // guarantee depends on the latest selection being on disk), from
    // Cancel/Escape (about to flip screens), and from `Main.qml`'s
    // `_stopRepeat` so dpad release commits the latest cell.
    function flushSelectedPersist(): void {
        if (!persistDebounce.running && games._pendingSelectedPath === "")
            return;
        persistDebounce.stop();
        if (games._pendingSelectedPath !== "")
            Browse.GamesState.set_selected_at_top(games._pendingSelectedPath);
        games._pendingSelectedPath = "";
    }

    // Move selection by (dx, dy) and commit the new selection on
    // success. Unlike HubScreen's _handleSystems, none of the games-grid
    // directions have a row-edge escape branch, so all four cardinal
    // actions share this exact body.
    function _performGridMove(dx: int, dy: int): void {
        games.gamesGrid.moveSelection(dx, dy);
    }

    function _performLinearMove(delta: int): void {
        const count = games.gamesGrid.itemCount;
        if (count <= 0)
            return;
        let next = games.gamesGrid.currentIndex + delta;
        if (next < 0)
            next = count - 1;
        else if (next >= count) {
            const knownTotal = Browse.GamesModel.total_dirs + Browse.GamesModel.total_files;
            if (games._listHasMore(count, knownTotal)) {
                Browse.GamesModel.fetch_more();
                return;
            }
            next = 0;
        }
        if (next === games.gamesGrid.currentIndex) {
            if (next >= count - 2)
                Browse.GamesModel.fetch_more();
            return;
        }
        games.gamesGrid.currentIndex = next;
        if (next >= count - 2 && Browse.GamesModel.has_next_page)
            Browse.GamesModel.fetch_more();
        games._prefetchListTail(next);
    }

    function _fillListPage(): void {
        if (!games._listLayout)
            return;
        const count = games.gamesGrid.itemCount;
        if (count > 0 && count <= games._listPageSize && Browse.GamesModel.has_next_page)
            Browse.GamesModel.fetch_more();
    }

    function _listHasMore(count: int, knownTotal: int): bool {
        return Browse.GamesModel.has_next_page || knownTotal > count;
    }

    function _prefetchListTail(index: int): void {
        if (!games._listLayout || Browse.GamesModel.loading_more)
            return;
        const count = games.gamesGrid.itemCount;
        const knownTotal = Browse.GamesModel.total_dirs + Browse.GamesModel.total_files;
        if (index >= count - games._listPageSize * games._listPrefetchPages && games._listHasMore(count, knownTotal))
            Browse.GamesModel.fetch_more();
    }

    // Background folder fill. The paged model used to fetch only on
    // demand, so paging raced a media.browse round trip and lost
    // routinely ("Loading more…" at the loaded edge). Instead, once a
    // folder's first page is up, keep pulling gentle chunks until the
    // whole folder is local. The repeat interval paces Core so other
    // requests interleave between chunks; a tick during an in-flight
    // fetch is a no-op (`fetch_more` debounces on `loading_more`) and
    // the next tick resumes. Leaving the screen stops the timer via
    // `active`; switching folders re-aims it (`has_next_page` resets
    // with the new browse, and stale responses die on the model's
    // browse generation counter).
    Timer {
        id: backgroundFolderFill
        interval: 200
        repeat: true
        // List layout only: grid delegates are not suspended, so a
        // background append there would materialize delegate cells the
        // user never asked for; grid paging keeps its own look-ahead.
        running: games.active && games._listLayout && !Browse.GamesModel.loading && Browse.GamesModel.has_next_page
        onTriggered: {
            if (!Browse.GamesModel.loading_more && Browse.GamesModel.has_next_page)
                Browse.GamesModel.fetch_more();
        }
    }

    // Page jump (L/R shoulder buttons). Wraps in both directions; same
    // post-move state-commit path as _performGridMove so the saved entry
    // tracks whichever item the user lands on.
    function _performPage(delta: int): void {
        if (games._listLayout) {
            games._performLinearMove(delta * games._browsePageSize);
            return;
        }
        games.gamesGrid.pageBy(delta);
    }

    // True when we're inside a navigated folder (path_stack length > 1).
    // Drives folder-aware cancel routing.
    function _atFolderLevel(): bool {
        return Browse.GamesState.path_stack.length > 1;
    }

    // Jump-to-letter. `itemOffset` is the cumulative count of all buckets before
    // the chosen letter (from the letter-index facet); all directories come
    // before files in the grid, so the letter's first item sits at
    // `total_dirs + itemOffset`. Driving the grid's page-jump there loads the
    // intervening pages and lands on that absolute index, so the full list stays
    // navigable both ways and the page counter reflects the real position.
    function jumpToItem(itemOffset: int): void {
        const absolute = Browse.GamesModel.total_dirs + itemOffset;
        const landed = games.gamesGrid.jumpToIndex(absolute);
        // A deferred walk (target not yet loaded) returns false; show the
        // standard centered loading cue over the stale source page until the
        // walk commits. An already-loaded backward jump lands immediately, so
        // no cue is needed.
        games.jumpLoading = !landed;
    }

    // Clear the jump cue once the grid's pending-target walk resolves - on
    // commit, or on an abort (model reset / directional clear) that flips
    // `hasPendingTarget` back to false.
    Connections {
        target: games.gamesGrid
        function onHasPendingTargetChanged(): void {
            if (!games.gamesGrid.hasPendingTarget)
                games.jumpLoading = false;
        }
    }
}
