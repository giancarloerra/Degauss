// Zaparoo Frontend
// Copyright (c) 2026 Wizzo Pty Ltd and the Zaparoo Project contributors.
// SPDX-License-Identifier: LicenseRef-PolyForm-Noncommercial-1.0.0
// var-typed action/callback properties (acceptAction, cancelAction, gridMoveAction,
// linearMoveAction, etc.) and layout profile bindings on QVariant-typed objects
// cannot be statically typed. cxx-qt 0.8 singleton methods also trip "can be
// shadowed". Both are structural; suppress compiler category file-wide.
// qmllint disable compiler
pragma ComponentBehavior: Bound

import QtQuick
import Zaparoo.Theme
import Zaparoo.Ui
import Zaparoo.Browse as Browse

// Shared media-list screen shell. The caller supplies the model,
// persisted selection state, and user-facing copy; interaction,
// focused-detail policy, and list/detail layout stay centralized here.
//
// Favorites and Recently Played use this directly today. Games can grow
// on top of the same shell by layering in folder navigation, richer
// pagination/status rules, and image-cycling behavior without forking
// the common list/detail mechanics again.
Item {
    id: root

    property var mediaModel: null
    property var mediaState: null
    property string screenTitle: ""
    property string emptyText: ""
    property string loadingText: ""
    property string detailPlaceholderKey: "icons/File"
    property int totalItemsOverride: -1
    property int targetVisibleRowCount: 0
    property bool detailShowDescription: true
    property bool detailShowTitle: true
    property string detailLoadingText: qsTr("Loading…")
    property bool detailCanPreviousImage: false
    property bool detailCanNextImage: false
    property bool detailReserveImageNav: false
    property var detailIdentityForIndex: null
    property var loadDetailForIndex: null
    // Optional immediate (non-debounced) detail hook. Defaults (when null) to
    // the model's `peek_detail_at`; GamesScreen overrides it with
    // `peek_description_at`. Keeps the detail table identity-correct the instant
    // the focus moves, so it never shows the previous row's metadata.
    property var peekDetailForIndex: null
    property var clearDetailAction: null
    property var retryAction: null
    property var acceptAction: null
    property var cancelAction: null
    property var gridMoveAction: null
    property var linearMoveAction: null
    property var pageAction: null
    property var onListLayoutEntered: null
    // Optional overrides for Left/Right in list layout. When null, the shell
    // pages a whole list page with the same ready-state guard as page_prev /
    // page_next. Grid layout never routes through these.
    property var listLeftAction: null
    property var listRightAction: null
    property var contextMenuEnabledAt: null
    property var restoreSelectionPath: null
    property var persistSelectionPath: null
    property var topStripTitleProvider: null
    property var topStripCurrentPageProvider: null
    property var topStripTotalPagesProvider: null
    property var topStripTotalTextProvider: null
    property var topStripRightTextProvider: null
    property var activeLabelTextProvider: null
    property var activeLabelTagsProvider: null
    property var gridCurrentPageChangedAction: null
    property var gridCurrentIndexChangedAction: null
    property var gridLoadMoreAction: null
    property string gridViewId: "gamesGrid"
    property string listViewId: "gamesList"
    property string tateListViewId: "gamesListTate"

    property alias mediaGrid: mediaGrid
    property alias topStrip: topStrip
    property alias listCard: listCard
    property alias activeLabel: activeLabel

    property bool transitioning: false
    property bool active: true
    property bool gridFocused: true
    property bool optimisticLoading: false
    // True while a jump-to-letter walk is loading the intervening pages. Folded
    // into `_loading()` so the standard centered loading cue paints over the
    // (stale) source page instead of leaving it frozen. Set/cleared by the
    // consumer that owns the jump (GamesScreen).
    property bool jumpLoading: false
    // False until the user takes control of focus (first input). Combined with
    // `_restoreDone` into `_focusReady`, which gates whether the grid tiles and
    // list rows render selection at all.
    property bool _focusArmed: false
    // Set true once the selection has been finalized from persisted state
    // (restoreSelection for favorites/recents; Main.qml's
    // `_setGamesRestoreIndex` for games). Combined with `_focusArmed` into
    // `_focusReady`, which gates whether the grid tiles and list rows render
    // selection at all - so the default index 0 never paints before restore
    // lands on a cold-start / forward entry.
    property bool _restoreDone: false
    readonly property bool _focusReady: root._focusArmed || root._restoreDone
    property bool detailRapidScrollActive: false
    property bool detailRapidIndicatorActive: detailRapidScrollActive
    property string detailRapidScrollAction: ""
    property bool pauseCoverRequestsDuringRapid: true
    property bool forceListLayout: false
    property bool renderGridLayout: true
    // Opt-in: allow the West-button page menu while the list is empty.
    // Default false so screens whose menu cannot empty the list keep the
    // stricter ready-only gate.
    property bool pageMenuEnabledWhenEmpty: false
    property bool showTopStrip: true
    property bool showBottomStatusRow: false
    property bool activeLabelAtBottom: false
    property bool suppressSelectionPersist: false
    property int gridBottomMargin: Sizing.pctH(15)
    property int activeLabelBottomMargin: 0
    property int activeLabelHeight: Sizing.pctH(7)
    property int bottomStatusLeftMargin: 0
    property int bottomStatusRightMargin: 0
    property int pageLoadingLeftMargin: 0
    readonly property int _gridViewportWidth: Math.max(1, root.width)
    readonly property int _gridViewportHeight: Math.max(1, root.height - (topStrip.y + topStrip.height) - root.gridBottomMargin)
    readonly property var _gridViewportShape: Sizing.gamesGridShape(root._gridViewportWidth, root._gridViewportHeight)
    property int gridColumnsOverride: root._gridViewportShape.columns
    property int gridRowsOverride: root._gridViewportShape.rows
    property bool pageLoadingVisible: false
    property string bottomStatusLeftText: ""
    property string bottomStatusRightText: ""
    property int gridTotalItemsOverride: -1
    property bool gridHasMorePages: false
    readonly property bool _listRapidLineMove: root._listLayout && (root.detailRapidScrollAction === "up" || root.detailRapidScrollAction === "down")
    readonly property bool _showRapidScrollIndicator: root.detailRapidIndicatorActive && !root._listRapidLineMove
    readonly property bool _listLayout: root.forceListLayout || Browse.Settings.current_browse_layout === "list"
    readonly property bool _tateListLayout: root._listLayout && Browse.Settings.current_orientation !== "horizontal"
    readonly property string _activeListViewId: root._tateListLayout ? root.tateListViewId : root.listViewId
    readonly property string _browseThemeId: BrowseLayouts.currentThemeId
    readonly property var _gridLayoutProfile: BrowseLayouts.themeProfile(root._browseThemeId, root.gridViewId)
    readonly property var _listLayoutProfile: BrowseLayouts.themeProfile(root._browseThemeId, root._activeListViewId)
    readonly property var _activeViewProfile: root._listLayout ? root._listLayoutProfile : root._gridLayoutProfile
    readonly property var _statusProfile: root._activeViewProfile && root._activeViewProfile.status ? root._activeViewProfile.status : null
    readonly property var _footerProfile: root._gridLayoutProfile && root._gridLayoutProfile.footer ? root._gridLayoutProfile.footer : null
    readonly property bool _crtListStrip: Theme.crtNativePath && root._listLayout
    readonly property int _listOverlayBottomMargin: root._listLayoutProfile && root._listLayoutProfile.list ? root._listLayoutProfile.list.overlayBottomMargin : Sizing.pctH(15)
    // Hide list/grid content as soon as the model enters Loading, but
    // let ScreenStateOverlay keep delaying the centered cue. Keep the
    // content hidden through the cue's minimum-visible tail so newly
    // loaded rows cannot paint underneath lingering loading text.
    readonly property bool _overlayLoadingVisible: stateOverlay.loadingVisible
    readonly property bool _gateHide: root.transitioning || root._loading() || root._overlayLoadingVisible

    signal requestHubScreen
    signal requestContextMenu(int index, var anchorRect)
    // Page-scoped operations entry point (West button). The router decides
    // what the menu contains; the screen just reports the press when the list
    // is in a usable state.
    signal requestPageMenu

    on_ListLayoutChanged: {
        if (!root._listLayout)
            return;
        if (typeof root.onListLayoutEntered === "function")
            root.onListLayoutEntered();
        focusedDetail.requestNow();
    }

    // Layout-aware pulse routing. In grid layout this forwards to the
    // PagedGrid tile; in list layout it increments the BrowseListDetailView
    // pulse so the selected row fires its push-in. The same push-in cue
    // serves both forward navigation and game launch.
    function pulseActivate(): void {
        if (root._listLayout)
            listCard.activatePulse++;
        else
            mediaGrid.pulseActivate();
    }

    // Settle the push-in cue back to rest after a launch that keeps the
    // frontend on the same screen (a launcher that does not take the FPGA or
    // quit us, e.g. an Audio track). Routed like pulseActivate to whichever
    // layout is live. Not called for forward navigation, which transitions the
    // screen and resets the cue off-screen on its own.
    function releaseActivate(): void {
        if (root._listLayout)
            listCard.releasePulse++;
        else
            mediaGrid.releaseActivate();
    }

    function _count(): int {
        return root.mediaModel !== null ? root.mediaModel.count : 0;
    }

    function _loading(): bool {
        return root.optimisticLoading || root.jumpLoading || (root.mediaModel !== null ? root.mediaModel.loading : false);
    }

    function _errorMessage(): string {
        return root.mediaModel !== null ? (root.mediaModel.error_message ?? "") : "";
    }

    function _detailImageKey(): string {
        return root.mediaModel !== null ? (root.mediaModel.current_detail_image_key ?? "") : "";
    }

    function _detailTags(): string {
        return root.mediaModel !== null ? (root.mediaModel.current_detail_tags ?? "") : "";
    }

    function _detailLoading(): bool {
        return root.mediaModel !== null ? root.mediaModel.current_detail_loading : false;
    }

    function restoreSelection(): void {
        if (root._count() <= 0)
            return;
        // The model is loaded; the selection is now finalized (either the
        // saved path below or the default index 0). Let the tiles/rows render.
        root._restoreDone = true;
        const path = typeof root.restoreSelectionPath === "function" ? (root.restoreSelectionPath() ?? "") : (root.mediaState !== null ? (root.mediaState.selected_path ?? "") : "");
        if (path === "")
            return;
        const idx = root.mediaModel.index_for_path(path);
        if (idx >= 0 && idx !== mediaGrid.currentIndex)
            mediaGrid.currentIndex = idx;
    }

    function _persistFocus(): void {
        if (root.suppressSelectionPersist || root.mediaModel === null)
            return;
        const idx = mediaGrid.currentIndex;
        if (idx < 0)
            return;
        const path = root.mediaModel.path_at(idx);
        if (path === "")
            return;
        if (typeof root.persistSelectionPath === "function")
            root.persistSelectionPath(path);
        else if (root.mediaState !== null)
            root.mediaState.selected_path = path;
    }

    function _focusIndex(index: int): void {
        if (index < 0 || index >= mediaGrid.itemCount)
            return;
        root._focusArmed = true;
        mediaGrid.currentIndex = index;
    }

    function _performLinearMove(delta: int): void {
        if (typeof root.linearMoveAction === "function") {
            root.linearMoveAction(delta);
            return;
        }
        const count = mediaGrid.itemCount;
        if (count <= 0)
            return;
        let next = mediaGrid.currentIndex + delta;
        if (next < 0)
            next = count - 1;
        else if (next >= count)
            next = 0;
        if (next === mediaGrid.currentIndex) {
            if (next >= count - 2)
                root.mediaModel.fetch_more();
            return;
        }
        mediaGrid.currentIndex = next;
        if (next >= count - 2)
            root.mediaModel.fetch_more();
    }

    function _coverRefreshFirstRow(): int {
        if (!root._listLayout)
            return mediaGrid.currentPage * mediaGrid.pageSize;
        return Math.max(0, mediaGrid.currentIndex - listCard.visibleRowCount);
    }

    function _coverRefreshRowCount(): int {
        if (!root._listLayout)
            return mediaGrid.pageSize * 2;
        return Math.max(1, listCard.visibleRowCount * 3);
    }

    function _resumeCoverRequests(): void {
        if (root.mediaModel === null)
            return;
        if (root.pauseCoverRequestsDuringRapid)
            root.mediaModel.cover_requests_paused = false;
        if (typeof root.mediaModel.refresh_cover_keys === "function")
            root.mediaModel.refresh_cover_keys(root._coverRefreshFirstRow(), root._coverRefreshRowCount());
        if (!root._listLayout && typeof root.gridCurrentPageChangedAction === "function")
            root.gridCurrentPageChangedAction();
    }

    function _pauseCoverRequests(): void {
        if (root.mediaModel === null || !root.pauseCoverRequestsDuringRapid)
            return;
        root.mediaModel.cover_requests_paused = true;
        if (typeof root.mediaModel.clear_pending_cover_requests === "function")
            root.mediaModel.clear_pending_cover_requests();
    }

    onDetailRapidScrollActiveChanged: {
        if (root.detailRapidScrollActive)
            root._pauseCoverRequests();
        else
            root._resumeCoverRequests();
    }

    function _performPage(delta: int): void {
        if (typeof root.pageAction === "function") {
            root.pageAction(delta);
            return;
        }
        if (root._listLayout) {
            root._performLinearMove(delta * mediaGrid.pageSize);
            return;
        }
        mediaGrid.pageBy(delta);
    }

    function _state(): string {
        if (root._loading() || root._overlayLoadingVisible)
            return "loading";
        if (root._errorMessage() !== "")
            return "error";
        if (root._count() === 0)
            return "empty";
        return "ready";
    }

    function handleAction(action: string): void {
        if ((action === "left" || action === "right" || action === "up" || action === "down" || action === "context_menu") && root._gateHide)
            return;

        root._focusArmed = true;

        // In list layout Left/Right are the fast-travel pair to Up/Down:
        // the same single-row walk through the same machinery, driven
        // faster when held (see Main.qml's smooth-scroll turbo and the
        // held-key repeat cadence). Paging lives on the page keys only.
        if (action === "left") {
            if (root._listLayout && typeof root.listLeftAction === "function")
                root.listLeftAction();
            else if (root._listLayout && root._state() === "ready")
                root._performLinearMove(-1);
            else if (!root._listLayout && typeof root.gridMoveAction === "function")
                root.gridMoveAction(-1, 0);
            else if (!root._listLayout)
                mediaGrid.moveSelection(-1, 0);
        } else if (action === "right") {
            if (root._listLayout && typeof root.listRightAction === "function")
                root.listRightAction();
            else if (root._listLayout && root._state() === "ready")
                root._performLinearMove(1);
            else if (!root._listLayout && typeof root.gridMoveAction === "function")
                root.gridMoveAction(1, 0);
            else if (!root._listLayout)
                mediaGrid.moveSelection(1, 0);
        } else if (action === "up") {
            if (root._listLayout)
                root._performLinearMove(-1);
            else if (typeof root.gridMoveAction === "function")
                root.gridMoveAction(0, -1);
            else
                mediaGrid.moveSelection(0, -1);
        } else if (action === "down") {
            if (root._listLayout)
                root._performLinearMove(1);
            else if (typeof root.gridMoveAction === "function")
                root.gridMoveAction(0, 1);
            else
                mediaGrid.moveSelection(0, 1);
        } else if (action === "page_prev") {
            if (root._state() === "ready")
                root._performPage(-1);
        } else if (action === "page_next") {
            if (root._state() === "ready")
                root._performPage(1);
        } else if (action === "page_menu") {
            // Screens whose page menu can itself cause the empty state (a
            // filter that matches nothing) must stay reachable while empty,
            // or the user is locked out of the only way to clear it.
            if (root._state() === "ready" || (root.pageMenuEnabledWhenEmpty && root._state() === "empty"))
                root.requestPageMenu();
        } else if (action === "accept") {
            const state = root._state();
            if (state === "loading")
                return;
            if (state === "error" || state === "empty") {
                if (typeof root.retryAction === "function")
                    root.retryAction();
                else
                    root.mediaModel.fetch_more();
                return;
            }
            if (typeof root.acceptAction === "function") {
                root.acceptAction(mediaGrid.currentIndex);
            } else {
                root.pulseActivate();
                defaultLaunchCommit._idx = mediaGrid.currentIndex;
                defaultLaunchCommit.arm();
            }
        } else if (action === "context_menu") {
            if (mediaGrid.itemCount > 0) {
                const idx = mediaGrid.currentIndex;
                if (typeof root.contextMenuEnabledAt === "function" && !root.contextMenuEnabledAt(idx))
                    return;
                root._persistFocus();
                const rect = root._listLayout ? listCard.currentCellRectIn(root) : mediaGrid.currentCellRectIn(root);
                root.requestContextMenu(idx, rect);
            }
        } else if (action === "cancel") {
            if (typeof root.cancelAction === "function")
                root.cancelAction();
            else
                root.requestHubScreen();
        }
    }

    // Defers the default launch_at call so the push-in cue (pulseActivate)
    // completes on a static scene before Core takes the FPGA.
    // Only used when no `acceptAction` override is provided (Favorites,
    // Recents). GamesScreen owns its own pressCommit with folder routing.
    DeferredAction {
        id: defaultLaunchCommit
        property int _idx: -1
        onDeferred: {
            const i = _idx;
            _idx = -1;
            if (i >= 0 && root.mediaModel !== null) {
                root.mediaModel.launch_at(i);
                // Settle the push-in back to rest. If the launch takes the FPGA
                // or kills us the release is never seen; if it stays on the page
                // (e.g. an Audio track) the cue does not stick pushed in.
                root.releaseActivate();
            }
        }
    }

    FocusedMediaDetailController {
        id: focusedDetail

        // With direct-database metadata the fetch costs a millisecond,
        // so the settle wait shrinks to a couple of frames — enough to
        // coalesce a fast tap burst, no longer a visible pause.
        debounceMs: Browse.AppStatus.direct_meta ? 40 : 220
        enabled: !root._gateHide && root._listLayout
        itemCount: mediaGrid.itemCount
        currentIndex: mediaGrid.currentIndex
        rapidScrollActive: root.detailRapidScrollActive
        identityForIndex: root.detailIdentityForIndex
        loadForIndex: root.loadDetailForIndex
        peekForIndex: root.peekDetailForIndex
        clearDetail: root.clearDetailAction
        mediaModel: root.mediaModel
    }

    TopStatusStrip {
        id: topStrip
        visible: !root._gateHide && (root._statusProfile ? root._statusProfile.topStripVisible : root.showTopStrip)
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: parent.top
        anchors.topMargin: Sizing.headerBottom + (root._statusProfile ? root._statusProfile.topMargin : Sizing.pctH(1))
        height: root._statusProfile ? root._statusProfile.stripHeight : (root.showTopStrip ? Sizing.pctH(7) : 0)
        slotMargin: root._statusProfile ? root._statusProfile.slotMargin : Sizing.pctW(5)
        title: typeof root.topStripTitleProvider === "function" ? root.topStripTitleProvider() : root.screenTitle
        currentPage: typeof root.topStripCurrentPageProvider === "function" ? root.topStripCurrentPageProvider() : mediaGrid.currentPage
        totalPages: typeof root.topStripTotalPagesProvider === "function" ? root.topStripTotalPagesProvider() : Math.max(1, Math.ceil(root._count() / mediaGrid.pageSize))
        totalText: typeof root.topStripTotalTextProvider === "function" ? root.topStripTotalTextProvider() : (root._listLayout ? "" : (root._count() > 0 ? qsTr("%1 entries").arg(root._count()) : ""))
        rightTextOverride: typeof root.topStripRightTextProvider === "function" ? root.topStripRightTextProvider() : (!root._listLayout || mediaGrid.itemCount <= 0 ? "" : qsTr("%1 / %2").arg(mediaGrid.currentIndex + 1).arg(Math.max(1, root._count())))
    }

    BrowseListDetailView {
        id: listCard

        rapidRenderMode: root.detailRapidScrollActive
        visible: !root._gateHide && root._listLayout
        anchors.left: parent.left
        anchors.leftMargin: root._listLayoutProfile && root._listLayoutProfile.list ? root._listLayoutProfile.list.cardSideMargin : Sizing.pctW(5)
        anchors.right: parent.right
        anchors.rightMargin: root._listLayoutProfile && root._listLayoutProfile.list ? root._listLayoutProfile.list.cardSideMargin : Sizing.pctW(5)
        anchors.top: topStrip.bottom
        anchors.topMargin: root._listLayoutProfile && root._listLayoutProfile.list ? root._listLayoutProfile.list.cardTopMargin : Sizing.pctH(2)
        anchors.bottom: parent.bottom
        anchors.bottomMargin: root._listLayoutProfile && root._listLayoutProfile.list ? root._listLayoutProfile.list.cardBottomMargin : Sizing.pctH(8)
        layoutProfile: root._listLayoutProfile
        model: root.mediaModel
        totalItemsOverride: root.totalItemsOverride
        targetVisibleRowCount: root.targetVisibleRowCount
        currentIndex: mediaGrid.currentIndex
        focusReady: root._focusReady
        detailTitle: listCard.currentName
        detailCoverKey: root.detailRapidScrollActive ? root.detailPlaceholderKey : (root._detailImageKey() !== "" ? root._detailImageKey() : listCard.currentCoverKey)
        detailShowDescription: root.detailShowDescription
        detailShowTitle: root.detailShowTitle
        detailTags: root._detailTags()
        detailLoading: root._detailLoading()
        detailSuppressed: root.detailRapidScrollActive
        screenSettling: !root.active
        detailLoadingText: root.detailLoadingText
        detailCanPreviousImage: root.detailCanPreviousImage
        detailCanNextImage: root.detailCanNextImage
        detailReserveImageNav: root.detailReserveImageNav
        onItemHovered: index => root._focusIndex(index)
        onItemClicked: index => {
            root._focusIndex(index);
            root.handleAction("accept");
        }
        onItemRightClicked: index => {
            root._focusIndex(index);
            root.handleAction("context_menu");
        }
        onEmptyRightClicked: root.handleAction("cancel")
        onPageWheelRequested: delta => root.handleAction(delta > 0 ? "page_next" : "page_prev")
    }

    PagedGrid {
        // Rapid paging crosses the default 2-page buffer faster than a
        // media.browse refill can land; a deeper look-ahead starts the
        // fetch while the user is still many pages from the loaded edge,
        // so "Loading more" stops interrupting sustained paging.
        loadAheadPages: root.detailRapidScrollActive ? 8 : 2
        // The grid is invisible in list layout but stays the row/cursor
        // authority; suspending its delegates removes the per-row
        // materialization cost from every model reset and insert.
        suspendDelegates: root._listLayout
        id: mediaGrid

        visible: !root._gateHide && !root._listLayout && root.renderGridLayout
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: topStrip.bottom
        anchors.bottom: parent.bottom
        anchors.bottomMargin: root.gridBottomMargin
        focused: root.gridFocused
        screenSettling: !root.active
        focusReady: root._focusReady
        model: root.mediaModel
        delegate: Tile {
            layoutProfile: root._gridLayoutProfile
            showCaption: true
            coverSourceSize: Sizing.gamesGridCoverSourceSize(root._gridViewportWidth, root._gridViewportHeight)
        }
        layoutProfile: root._gridLayoutProfile
        columnsOverride: root.gridColumnsOverride
        rowsOverride: root.gridRowsOverride
        totalItemsOverride: root.gridTotalItemsOverride
        hasMorePages: root.gridHasMorePages
        coverLoadingPaused: root.detailRapidScrollActive
        rapidRenderMode: root.detailRapidScrollActive
        onLoadMoreRequested: urgent => {
            if (typeof root.gridLoadMoreAction === "function")
                root.gridLoadMoreAction(urgent);
            else
                root.mediaModel.fetch_more();
        }
        onCurrentIndexChanged: {
            root._persistFocus();
            if (typeof root.gridCurrentIndexChangedAction === "function")
                root.gridCurrentIndexChangedAction();
        }
        onCurrentPageChanged: {
            if (typeof root.gridCurrentPageChangedAction === "function")
                root.gridCurrentPageChangedAction();
        }
        onItemHovered: index => root._focusIndex(index)
        onItemClicked: index => {
            root._focusIndex(index);
            root.handleAction("accept");
        }
        onItemRightClicked: index => {
            root._focusIndex(index);
            root.handleAction("context_menu");
        }
        onEmptyRightClicked: root.handleAction("cancel")
        onPageWheelRequested: delta => root.handleAction(delta > 0 ? "page_next" : "page_prev")
    }

    ActiveLabel {
        id: activeLabel
        visible: !root._gateHide && !root._listLayout && root.renderGridLayout
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: root.activeLabelAtBottom ? undefined : mediaGrid.bottom
        anchors.bottom: root.activeLabelAtBottom ? parent.bottom : undefined
        anchors.bottomMargin: root.activeLabelAtBottom ? root.activeLabelBottomMargin : 0
        height: root.activeLabelHeight
        text: typeof root.activeLabelTextProvider === "function" ? root.activeLabelTextProvider() : (mediaGrid.itemCount > 0 ? root.mediaModel.name_at(mediaGrid.currentIndex) : "")
        // Full (untrimmed) disambiguation tokens for the focused item, shown as a
        // dim suffix. Uses an explicit provider when given, else the model's
        // disambiguating_tags_at; guarded so a plain ListModel (tests) is safe.
        tags: typeof root.activeLabelTagsProvider === "function" ? root.activeLabelTagsProvider() : ((mediaGrid.itemCount > 0 && root.mediaModel && typeof root.mediaModel.disambiguating_tags_at === "function") ? root.mediaModel.disambiguating_tags_at(mediaGrid.currentIndex) : "")
    }

    Text {
        id: bottomTotalText
        visible: root.showBottomStatusRow && !root._gateHide && !root._listLayout && root.bottomStatusLeftText !== ""
        anchors.left: parent.left
        anchors.leftMargin: root.bottomStatusLeftMargin
        anchors.verticalCenter: activeLabel.verticalCenter
        width: Sizing.px(parent.width / 3) - root.bottomStatusLeftMargin
        height: Sizing.fontSize(2.9)
        elide: Text.ElideRight
        horizontalAlignment: Text.AlignLeft
        verticalAlignment: Text.AlignVCenter
        text: root.bottomStatusLeftText
        font.family: Theme.fontUi
        font.pixelSize: Sizing.fontSize(2.9)
        color: Theme.textPrimary
        renderType: Text.NativeRendering
    }

    Text {
        visible: root.showBottomStatusRow && !root._gateHide && !root._listLayout && root.bottomStatusRightText !== ""
        anchors.right: parent.right
        anchors.rightMargin: root.bottomStatusRightMargin
        anchors.verticalCenter: activeLabel.verticalCenter
        width: Sizing.px(parent.width / 3) - root.bottomStatusRightMargin
        height: Sizing.fontSize(2.9)
        elide: Text.ElideRight
        horizontalAlignment: Text.AlignRight
        verticalAlignment: Text.AlignVCenter
        text: root.bottomStatusRightText
        font.family: Theme.fontUi
        font.pixelSize: Sizing.fontSize(2.9)
        color: Theme.textPrimary
        renderType: Text.NativeRendering
    }

    LoadingIndicator {
        visible: !root._gateHide && !root._listLayout && root.pageLoadingVisible
        anchors.left: activeLabel.left
        anchors.leftMargin: root.pageLoadingLeftMargin
        anchors.verticalCenter: activeLabel.verticalCenter
    }

    RapidScrollIndicator {
        visible: !root._gateHide && root._showRapidScrollIndicator && mediaGrid.itemCount > 0 && !root._listLayout
        x: Sizing.center(parent.width, width)
        y: Sizing.center(mediaGrid.height, height) + mediaGrid.y
        title: typeof root.activeLabelTextProvider === "function" ? root.activeLabelTextProvider() : (mediaGrid.itemCount > 0 && root.mediaModel !== null ? root.mediaModel.name_at(mediaGrid.currentIndex) : "")
        z: 20
    }

    ScreenStateOverlay {
        id: stateOverlay

        x: root._listLayout ? listCard.x : mediaGrid.x
        y: root._listLayout ? listCard.y : mediaGrid.y
        width: root._listLayout ? listCard.width : mediaGrid.width
        height: root._listLayout ? Math.max(0, root.height - listCard.y - root._listOverlayBottomMargin) : mediaGrid.height
        enabled: true
        loading: root._loading()
        errorMessage: root._errorMessage()
        count: root._count()
        emptyText: root.emptyText
        loadingText: root.loadingText
    }

    // Adjacent-cover preload pool. While the user dwells on a list row
    // these hidden Images decode the next and previous rows' covers into
    // Qt's pixmap cache at the same sourceSize as the visible detail cover
    // (the shared Sizing.detailCoverSourceWidth tier), so the detail cover switch on a d-pad move is a
    // synchronous cache hit rather than an async decode. Mirrors the
    // system-cover prefetch pattern in Main.qml:2629. Active only in list
    // layout; in grid layout there is no per-row detail pane.
    // The source guard (`k.startsWith("media-image/")`) keeps the source
    // empty for placeholder keys (`icons/*`) so no decode work is done for
    // folder entries or cold-cache neighbors that haven't resolved yet.
    Image {
        id: prefetchNextCover
        visible: false
        width: 0
        height: 0
        asynchronous: true
        cache: true
        sourceSize.width: Sizing.detailCoverSourceWidth
        source: {
            if (!root._listLayout || root.mediaModel === null)
                return "";
            const k = root.mediaModel.detail_prefetch_key_next ?? "";
            return k.startsWith("media-image/") ? Resources.coverUrl(k, Theme.logoFocusPrimary, Theme.logoFocusSecondary, Theme.logoFocusShadow) : "";
        }
    }

    Image {
        id: prefetchPrevCover
        visible: false
        width: 0
        height: 0
        asynchronous: true
        cache: true
        sourceSize.width: Sizing.detailCoverSourceWidth
        source: {
            if (!root._listLayout || root.mediaModel === null)
                return "";
            const k = root.mediaModel.detail_prefetch_key_prev ?? "";
            return k.startsWith("media-image/") ? Resources.coverUrl(k, Theme.logoFocusPrimary, Theme.logoFocusSecondary, Theme.logoFocusShadow) : "";
        }
    }
}
