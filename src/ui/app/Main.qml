// Zaparoo Frontend
// Copyright (c) 2026 Wizzo Pty Ltd and the Zaparoo Project contributors.
// SPDX-License-Identifier: LicenseRef-PolyForm-Noncommercial-1.0.0

import QtQuick
import QtQuick.Window
import Zaparoo.Theme
import Zaparoo.Screens
import Zaparoo.Ui
import Zaparoo.Browse as Browse

// cxx-qt 0.8 patches `isFinal: true` on singleton properties but the
// qmltypes schema has no `isFinal` slot for Method, so every qinvokable
// call on a Zaparoo.Browse singleton still trips qmllint's "Member can
// be shadowed" check. Until the schema grows method-level finality,
// suppress the compiler category file-wide.
// qmllint disable compiler

// Runtime wrapper around MainLayout. The visual tree lives in
// MainLayout.qml (editable by designers in Qt Design Studio) and the
// individual screens in Zaparoo.Screens; this file is a thin router
// that translates raw Qt key events into actions, dispatches them to
// the active screen (or topmost modal), and persists user-visible
// navigation state across kills.
MainLayout {
    id: root

    // Fullscreen builds (MiSTer) fill the screen; desktop windowed
    // builds inherit MainLayout's 1280x720 design defaults so the user
    // can resize freely. MainLayout binds width/height to Screen.* when
    // fullScreen is true so the first paint is at the correct dims;
    // doing it imperatively here would land after the first frame and
    // leave a 1280x720 slice on screen for one frame.

    readonly property string modalCardWrite: "card_write"
    readonly property string modalContextMenu: "context_menu"
    readonly property string modalGameInfo: "game_info"
    readonly property string modalQrCode: "qr_code"
    readonly property string modalCommercialNotice: "commercial_notice"
    readonly property string modalCoreVersion: "core_version_warning"
    readonly property string modalRandomFailed: "random_failed"
    // Sentinel id for the favorites "show everything" row; maps to an empty
    // filter. Must never be "" — see openFavoritesFilterMenu.
    readonly property string _favoritesFilterAll: "all"
    // Sentinel id for the favorites "default order" row; maps to an empty
    // sort mode. Must never be "" — see openFavoritesSortMenu.
    readonly property string _favoritesSortDefault: "default"
    readonly property string modalFirstRunIndex: "first_run_index"
    readonly property string modalLogUpload: "log_upload"
    readonly property string modalQuitConfirm: "quit_confirm"
    readonly property string modalListPicker: "list_picker"
    readonly property string modalLetterJump: "letter_jump"
    readonly property string modalSettingNeedsRestart: "restart_confirm"
    readonly property string modalCrtCalibration: "crt_calibration"

    // One-shot session flag: the first-run modal is shown at most
    // once per frontend process, even if the WS link drops and the
    // mediadb-empty condition would otherwise be satisfied again.
    property bool _firstRunIndexShown: false
    // One-shot guard for the Core-version warning, same lifetime as
    // _firstRunIndexShown: show it at most once per process even if the
    // link drops and reconnects to the same old Core.
    property bool _coreVersionWarningShown: false
    property string _pendingLanguageSelection: ""
    property string _pendingResolutionSelection: ""
    property string _pendingCrtStandardSelection: ""
    // Staged CRT-mode toggle awaiting the restart-confirm modal:
    // "" (none), "on", or "off". Confirming writes the 1-byte enable
    // file and exits with code 42 so Main_MiSTer respawns the frontend
    // with the new mode (see Browse.CrtVideo).
    property string _pendingCrtToggle: ""
    property bool _discoverMenuPending: false
    property bool _pendingResumeLaunch: false
    property bool _startupRestorePending: false
    property bool _startupRestoreStarted: false
    property string _startupRestoreScreen: ""
    property var _screenReadyCallbacks: ({})
    property var _discoverParentEntries: []
    property string _pendingLauncherSystemId: ""
    property string _pendingLauncherSelectionId: ""
    property string cardWriteOwner: ""
    property string contextMenuMode: "main"
    property string contextMenuOwner: ""
    property int contextMenuIndex: -1
    readonly property bool activeCardWritePending: root.cardWriteOwner === "systems" ? Browse.SystemsModel.card_write_pending : root.cardWriteOwner === "games" ? Browse.GamesModel.card_write_pending : root.cardWriteOwner === "favorites" ? Browse.FavoritesModel.card_write_pending : false
    readonly property string activeCardWriteError: root.cardWriteOwner === "systems" ? Browse.SystemsModel.card_write_error : root.cardWriteOwner === "games" ? Browse.GamesModel.card_write_error : root.cardWriteOwner === "favorites" ? Browse.FavoritesModel.card_write_error : ""

    // Feed the Motion singleton's master switch from the persisted
    // reduce-motion setting. Keeping Motion dependency-free (no
    // Browse import) means Zaparoo.Theme stays independent; the app
    // layer is the only place that crosses the module boundary.
    Binding {
        target: Motion
        property: "enabled"
        value: !Browse.Settings.current_reduce_motion
    }

    // Mirror the "Show original filenames" setting onto every model that
    // surfaces a game name. Bound centrally (not per-screen) so browse,
    // favorites, recents, the resume banner, and launch/now-playing titles
    // all flip together regardless of which screen is mounted. Each model's
    // setter re-emits dataChanged so already-built delegates refresh in place.
    Binding {
        target: (root.gamesScreenRequested || root.activeScreen === root.screenGames) ? Browse.GamesModel : null
        property: "show_original_filenames"
        value: Browse.Settings.current_show_original_filenames
    }
    Binding {
        target: (root.favoritesScreenRequested || root.activeScreen === root.screenFavorites) ? Browse.FavoritesModel : null
        property: "show_original_filenames"
        value: Browse.Settings.current_show_original_filenames
    }
    Binding {
        target: (root._firstFrameSeen || root.recentsScreenRequested || root.activeScreen === root.screenRecents) ? Browse.RecentsModel : null
        property: "show_original_filenames"
        value: Browse.Settings.current_show_original_filenames
    }

    // Bound here (not in GamesScreen.qml) because `set_system` can fire
    // from the accept handler before the games screen mounts; binding
    // inside the screen fires only on `Component.onCompleted`, after the
    // first fetch has already gone out with the model's default
    // page_size. That made the first cursor page misaligned with the
    // visual grid pageSize and produced half-loaded pages on every
    // subsequent cursor advance.
    readonly property int _gamesListFetchSize: 30
    readonly property var _gamesGridProfile: BrowseLayouts.themeProfile(BrowseLayouts.currentThemeId, "gamesGrid")
    readonly property var _gamesGridStatusProfile: root._gamesGridProfile && root._gamesGridProfile.status ? root._gamesGridProfile.status : null
    readonly property var _gamesGridFooterProfile: root._gamesGridProfile && root._gamesGridProfile.footer ? root._gamesGridProfile.footer : null
    readonly property int _gamesGridTopStripBottom: Sizing.headerBottom + (root._gamesGridStatusProfile ? root._gamesGridStatusProfile.topMargin : Sizing.pctH(1)) + (root._gamesGridStatusProfile ? root._gamesGridStatusProfile.stripHeight : Sizing.pctH(7))
    readonly property int _gamesGridBottomMargin: root._gamesGridFooterProfile ? root._gamesGridFooterProfile.gridBottomMargin : (Sizing.pctH(8) + Sizing.pctH(7))
    readonly property int _gamesGridViewportWidth: Math.max(1, Sizing.screenWidth)
    readonly property int _gamesGridViewportHeight: Math.max(1, Sizing.screenHeight - root._gamesGridTopStripBottom - root._gamesGridBottomMargin)
    readonly property var _gamesGridShape: Sizing.gamesGridShape(root._gamesGridViewportWidth, root._gamesGridViewportHeight)
    readonly property int _gamesGridColumns: root._gamesGridShape.columns
    readonly property int _gamesGridRows: root._gamesGridShape.rows
    readonly property int _gamesPageSize: Browse.Settings.current_browse_layout === "list" ? root._gamesListFetchSize : root._gamesGridColumns * root._gamesGridRows
    on_GamesPageSizeChanged: {
        if (root.gamesScreenRequested || root.activeScreen === root.screenGames)
            root._syncGamesModelLayout();
    }
    // Cover size requested from Core for grid tiles. Same per-view, snapped tier
    // (128/256/512/768) the grid tile decodes at (Tile.qml drives sourceSize off
    // the identical helper), so the request matches what is shown and Core caches
    // one resized WebP per tier, keeping the constantly-browsed grid small in RAM.
    readonly property int _gamesCoverMaxSize: Sizing.gamesGridCoverSourceSize(root._gamesGridViewportWidth, root._gamesGridViewportHeight)
    // The detail pane shows the cover larger than a grid tile, so it requests its
    // own snapped tier (a step above the grid). Baked into the detail keys, this
    // gives the pane an independent, crisper cache entry instead of sharing the
    // grid's smaller image.
    readonly property int _gamesDetailCoverMaxSize: Sizing.detailCoverSourceSize(root._gamesGridViewportWidth, root._gamesGridViewportHeight)
    // _gamesDetailCoverMaxSize derives from _gamesCoverMaxSize, so this one
    // handler re-syncs both sizes whenever the grid shape changes.
    on_GamesCoverMaxSizeChanged: {
        if (root.gamesScreenRequested || root.activeScreen === root.screenGames)
            root._syncGamesModelLayout();
    }

    // Bind Sizing to the scene's logical dimensions, not the
    // ApplicationWindow's. Outside CRT preview the scene fills the
    // window so the values are identical to the prior imperative
    // writes; in preview the scene is fixed at videoWidth x
    // videoHeight and the bindings keep Sizing reading logical
    // pixels for pctW/pctH/px/etc.
    Binding {
        target: Sizing
        property: "screenWidth"
        value: root.scene.width
    }
    Binding {
        target: Sizing
        property: "screenHeight"
        value: root.scene.height
    }
    // Keep the detail-cover tier's inputs identical to the fetch-size
    // inputs (`_gamesDetailCoverMaxSize` below), so request tier and
    // decode tier can never diverge.
    Binding {
        target: Sizing
        property: "detailCoverViewportWidth"
        value: root._gamesGridViewportWidth
    }
    Binding {
        target: Sizing
        property: "detailCoverViewportHeight"
        value: root._gamesGridViewportHeight
    }

    function _requestScreen(screen: string): void {
        if (screen === root.screenSystems)
            root.systemsScreenRequested = true;
        else if (screen === root.screenGames) {
            root.gamesScreenRequested = true;
            root._syncGamesModelLayout();
        } else if (screen === root.screenFavorites)
            root.favoritesScreenRequested = true;
        else if (screen === root.screenRecents)
            root.recentsScreenRequested = true;
        else if (screen === root.screenSettings)
            root.settingsScreenRequested = true;
        else if (screen === root.screenAbout)
            root.aboutScreenRequested = true;
    }

    function _syncGamesModelLayout(): void {
        Browse.GamesModel.page_size = root._gamesPageSize;
        Browse.GamesModel.set_cover_max_size(root._gamesCoverMaxSize);
        Browse.GamesModel.set_detail_cover_max_size(root._gamesDetailCoverMaxSize);
    }

    function _screenItem(screen: string): var {
        if (screen === root.screenSystems)
            return root.systemsScreen;
        if (screen === root.screenGames)
            return root.gamesScreen;
        if (screen === root.screenFavorites)
            return root.favoritesScreen;
        if (screen === root.screenRecents)
            return root.recentsScreen;
        if (screen === root.screenSettings)
            return root.settingsScreen;
        if (screen === root.screenAbout)
            return root.aboutScreen;
        return root.hubScreen;
    }

    function _whenScreenReady(screen: string, callback): void {
        root._requestScreen(screen);
        const item = root._screenItem(screen);
        if (item !== null && item !== undefined) {
            callback(item);
            return;
        }
        const pending = root._screenReadyCallbacks[screen] || [];
        pending.push(callback);
        root._screenReadyCallbacks[screen] = pending;
    }

    function _flushScreenReady(screen: string): void {
        const item = root._screenItem(screen);
        if (item === null || item === undefined)
            return;
        const pending = root._screenReadyCallbacks[screen] || [];
        if (pending.length === 0)
            return;
        delete root._screenReadyCallbacks[screen];
        for (let i = 0; i < pending.length; i++)
            pending[i](item);
    }

    function _requestModal(modal: string): void {
        if (modal === root.modalCardWrite)
            root.cardWriteModalRequested = true;
        else if (modal === root.modalContextMenu)
            root.contextMenuRequested = true;
        else if (modal === root.modalGameInfo)
            root.gameInfoModalRequested = true;
        else if (modal === root.modalQrCode)
            root.qrCodeModalRequested = true;
        else if (modal === root.modalCommercialNotice)
            root.commercialNoticeModalRequested = true;
        else if (modal === root.modalCoreVersion)
            root.coreVersionModalRequested = true;
        else if (modal === root.modalRandomFailed)
            root.randomFailedModalRequested = true;
        else if (modal === root.modalFirstRunIndex)
            root.firstRunIndexModalRequested = true;
        else if (modal === root.modalLogUpload)
            root.logUploadModalRequested = true;
        else if (modal === root.modalQuitConfirm)
            root.quitConfirmModalRequested = true;
        else if (modal === root.modalListPicker)
            root.listPickerModalRequested = true;
        else if (modal === root.modalLetterJump)
            root.letterJumpModalRequested = true;
        else if (modal === root.modalSettingNeedsRestart)
            root.settingNeedsRestartModalRequested = true;
        else if (modal === root.modalCrtCalibration)
            root.crtCalibrationModalRequested = true;
    }

    Component.onCompleted: {
        // Desktop CRT preview applies one initial integer scale here,
        // then MainLayout snaps later user resizes to the supported
        // 3x..5x steps. Fullscreen embedded sizing is handled by
        // MainLayout's width/height bindings so first paint matches
        // the FB layout.
        if (!root.fullScreen && root._crtPreviewActive) {
            root.applyCrtPreviewScale(root._crtPreviewInitialScale);
        }
        const savedScreen = root._validStartupScreen(Browse.AppState.active_screen);
        root.activeScreen = root.screenHub;
        if (savedScreen !== "" && savedScreen !== root.screenHub) {
            root._startupRestorePending = true;
            root._startupRestoreScreen = savedScreen;
            // MainLayout seeds this before Component.onCompleted so no first-frame
            // ghost Hub can leak during non-Hub restores; assign here to break the
            // initial binding and keep the router-owned curtain explicit.
            root.startupRestoreCurtainVisible = true;
        } else {
            root.startupRestoreCurtainVisible = false;
        }
        root._startupTrace("startup/qml Component.onCompleted", "savedScreen=" + savedScreen, "initialActiveScreen=" + root.activeScreen, "startupRestorePending=" + root._startupRestorePending, "connectionState=" + Browse.AppStatus.connection_state);
        // Fire the focus restore here so Hub focus is seated and marked ready
        // before first paint. Do not cascade into SystemsModel yet: first
        // paint stays Hub-only, and the post-frame handler below runs the
        // cascade needed by saved-screen restore and later drill-downs.
        root.hubScreen.restoreFromCategoriesReset(false);
        root._maybeArmHubResumeFocus();
        // Open the commercial-use notice on first paint of an unacked
        // install. Sits in front of the media-DB first-run modal in the
        // routing order — `_maybeOpenFirstRunIndex` early-returns until
        // `Browse.Notice.commercial_ack` flips true, at which point the
        // notice's close handler retriggers the media-DB check.
        root._maybeOpenCommercialNotice();
        // Kick the first-run check in case both READY and a seeded
        // empty-mediadb snapshot landed before our Connections wired up
        // (e.g. an unusually fast warm-cache reconnect).
        root._maybeCompleteBoot();
        root._maybeOpenFirstRunIndex();
        root._maybeStartStartupRestore();
    }

    on_FirstFrameSeenChanged: {
        if (root._firstFrameSeen) {
            Browse.ImageOverrides.load_hub_overrides();
            if (Browse.CategoriesModel.count > 0)
                root.hubScreen.restoreFromCategoriesReset(true);
            root._maybeStartStartupRestore();
        }
    }

    Connections {
        target: Browse.ImageOverrides
        function onHub_loadedChanged(): void {
            if (Browse.ImageOverrides.hub_loaded)
                Browse.ImageOverrides.load_system_overrides();
        }
        function onSystems_loadedChanged(): void {
            if (Browse.ImageOverrides.systems_loaded && Browse.CategoriesModel.count > 0)
                Browse.SystemsModel.reproject();
        }
    }

    function _isStableNavigationScreen(screen: string): bool {
        if (screen === root.screenHub || screen === root.screenSystems || screen === root.screenGames || screen === root.screenFavorites || screen === root.screenRecents || screen === root.screenSettings || screen === root.screenAbout)
            return true;
        return false;
    }

    function _validStartupScreen(screen: string): string {
        if (root._isStableNavigationScreen(screen))
            return screen;
        return "";
    }

    // Seed row/grid indices from persisted state when models deliver new
    // data. A miss (category renamed, ROM deleted) falls back to index 0
    // and leaves the saved identifier untouched on disk — so the user's
    // intent survives a transient catalog gap. State writes only happen
    // in each screen's handleAction (user navigation); these programmatic
    // seeds are inert.
    //
    // Always cascade into set_category (even on a miss or first-launch empty
    // HubState.category): SystemsModel is the only way to drive the next
    // onModelReset handler, and a games-screen restore depends on that chain
    // firing so GamesModel.set_system runs.
    Connections {
        target: Browse.CategoriesModel
        function onModelReset(): void {
            root.hubScreen.restoreFromCategoriesReset(root._firstFrameSeen);
            root._maybeStartStartupRestore();
            root._maybeContinueOptimisticTransitions();
        }
        function onLoadedChanged(): void {
            root._maybeContinueOptimisticTransitions();
        }
        function onError_messageChanged(): void {
            root._maybeContinueOptimisticTransitions();
        }
    }
    Connections {
        target: root._systemsModelConnectionsEnabled ? Browse.SystemsModel : null
        // On a games-screen restore, GamesState.system_id is authoritative;
        // fall back to SystemsState.system_id only if it's empty (edge case:
        // user pressed Enter on an empty systems grid and we flipped the
        // screen without ever committing a system). On a hub or systems
        // restore, SystemsState.system_id is authoritative — don't peek at
        // GamesState, or we'd override the user's position with a stale
        // games target from a prior escape-back-up-the-stack.
        function onModelReset(): void {
            if (root.systemsScreen === null) {
                root._whenScreenReady(root.screenSystems, function () {
                    root._restoreSystemsScreenSelection();
                });
                return;
            }
            root._restoreSystemsScreenSelection();
        }
    }
    Connections {
        target: root._gamesModelConnectionsEnabled ? Browse.GamesModel : null
        function onModelReset(): void {
            if (root.gamesScreen === null) {
                root._whenScreenReady(root.screenGames, function () {
                    root._restoreGamesScreenSelection();
                });
                return;
            }
            root._restoreGamesScreenSelection();
        }
        // Pages 2+ append rows via begin_insert_rows / end_insert_rows
        // (no model reset), so we can't piggy-back on onModelReset to
        // retry the lookup. `count` bumps on every append, giving us a
        // stable per-page edge to resume the deep-page restore on.
        function onCountChanged(): void {
            if (root.gamesScreen === null) {
                root._whenScreenReady(root.screenGames, function () {
                    if (root._pendingGameRestorePath !== "")
                        root._restoreGamesScreenSelection();
                });
                return;
            }
            const path = root._pendingGameRestorePath;
            if (path === "")
                return;
            // User backed out to Hub/Systems before pagination caught
            // up — selected_at_level isn't touched by a peer-screen
            // exit, so without this gate the loop would keep hammering
            // fetch_more in the background until the folder exhausts.
            if (root.activeScreen !== root.screenGames && !(root._startupRestorePending && root._startupRestoreScreen === root.screenGames)) {
                root._pendingGameRestorePath = "";
                return;
            }
            // User input updates `selected_at_level` on every move,
            // so a divergence between the pending path and the top
            // of stack means the user navigated during the restore
            // — drop the auto-restore and let them stay where they
            // landed.
            const sels = Browse.GamesState.selected_at_level;
            const currentTop = sels.length > 0 ? sels[sels.length - 1] : "";
            if (currentTop !== path) {
                root._pendingGameRestorePath = "";
                root._maybeFinishStartupGamesRestore();
                return;
            }
            const idx = Browse.GamesModel.index_for_game_path(path);
            if (idx >= 0) {
                root._setGamesRestoreIndex(idx);
                root._pendingGameRestorePath = "";
                root._maybeFinishStartupGamesRestore();
                return;
            }
            if (Browse.GamesModel.has_next_page) {
                // fetch_more is itself debounced by `loading_more` and
                // `has_next_page`, so a redundant call here is a cheap
                // no-op rather than a duplicate request.
                Browse.GamesModel.fetch_more();
                return;
            }
            root._pendingGameRestorePath = "";
            root._maybeFinishStartupGamesRestore();
        }
    }

    // Cross-screen transitions: each screen signals its intent and this
    // router flips ScreenManager, then applies route policy such as
    // launch-resume persistence. Keeps the screens themselves ignorant
    // of AppState so they can be reused in test harnesses that don't
    // wire the full persistence layer.
    function _goto(screen: string): void {
        root._requestScreen(screen);
        root._startupTrace("startup/qml goto", "from=" + root.activeScreen, "to=" + screen, "pendingTransition=" + root.pendingTransition);
        ScreenManager.activeScreen = screen;
        if (root._isLaunchResumeScreen(screen))
            Browse.AppState.active_screen = screen;
    }

    // Long-running operational screens should not be restored after the
    // process is killed. Resume only stable navigation destinations.
    function _isLaunchResumeScreen(screen: string): bool {
        return root._isStableNavigationScreen(screen);
    }

    function _allowsScreensaver(screen: string): bool {
        if (screen === root.screenUpdate)
            return root.updateScreen !== null && root.updateScreen.allowsScreensaver;
        return true;
    }

    function _backTargetReady(screen: string): bool {
        const item = root._screenItem(screen);
        if (item === null || item === undefined)
            return false;
        if (screen === root.screenHub)
            return true;
        if (screen === root.screenSystems)
            return !Browse.SystemsModel.loading;
        if (screen === root.screenGames)
            return !Browse.GamesModel.loading;
        if (screen === root.screenFavorites)
            return !Browse.FavoritesModel.loading;
        if (screen === root.screenRecents)
            return !Browse.RecentsModel.loading;
        return true;
    }

    function _maybeCompleteBackTransition(): void {
        if (root.pendingTransition !== "back")
            return;
        const target = root._backTransitionTarget;
        if (target === "" || !root._backTargetReady(target))
            return;
        root._backTransitionTarget = "";
        backTransitionTimer.stop();
        root._completeTransition(target);
    }

    // Back navigation is usually a return to an already-mounted, already-filled
    // screen. Cut immediately in that case. Keep the delayed Loading cue only
    // when real work is pending: lazy screen mount, catalog boot, or a model
    // fill still in flight.
    function _navigateBackToScreen(screen: string): void {
        if (screen === root.activeScreen)
            return;
        root._backTransitionTarget = "";
        backTransitionTimer.stop();
        if (root._backTargetReady(screen)) {
            root._goto(screen);
            root._resetIdle();
            return;
        }
        root._backTransitionTarget = screen;
        root.pendingTransition = "back";
        root._whenScreenReady(screen, function () {
            if (root.pendingTransition !== "back" || root._backTransitionTarget !== screen)
                return;
            root._maybeCompleteBackTransition();
            if (root.pendingTransition === "back")
                backTransitionTimer.restart();
        });
    }

    // Single-shot callback slots fired by the loadingChanged
    // listeners below. Only one transition is in flight at a time
    // (input gate guarantees this), so two scalars are enough.
    // `pendingTransition` itself lives in MainLayout — the source
    // screen's content-hiding bindings (row/grid `visible`) resolve
    // there, so the property has to be declared at that level for
    // the QML lint pass to be happy.
    property var _categoryReadyCallback: null
    property var _systemReadyCallback: null
    property var _favoritesReadyCallback: null
    property var _recentsReadyCallback: null
    property string _catalogWaitCategory: ""
    // Set when `_ensureCategory` arms `deferredCategorySetTimer` and
    // cleared inside the timer's `onTriggered` after `set_category`
    // actually fires. Gates `_categoryReadyCallback` consumption so a
    // stale `SystemsModel.loading` false-edge from an unrelated in-flight
    // fill (e.g. `restoreFromCategoriesReset` already running) can't
    // complete the transition before our own `set_category` has been
    // issued.
    property bool _deferredCategoryPending: false
    property bool _deferredSystemPending: false
    readonly property bool _systemsModelConnectionsEnabled: root.systemsScreenRequested || (root._firstFrameSeen && root._startupRestorePending) || root._categoryReadyCallback !== null || root._deferredCategoryPending || root._catalogWaitCategory !== ""
    readonly property bool _gamesModelConnectionsEnabled: root.gamesScreenRequested || root._systemReadyCallback !== null || root._deferredSystemPending
    readonly property bool _favoritesModelConnectionsEnabled: root.favoritesScreenRequested || root._favoritesReadyCallback !== null
    readonly property bool _recentsModelConnectionsEnabled: root.recentsScreenRequested || root._recentsReadyCallback !== null || root._pendingResumeLaunch
    // Saved games-screen entry path that wasn't on the freshly seeded
    // page 1 of MediaBrowse. The GamesModel.onCountChanged watcher
    // below paginates forward via fetch_more until the path is found
    // or `has_next_page` goes false. Cleared on resolution and on
    // any navigation that starts a new browse target so a stale
    // restore can't keep paginating after the user moves on.
    property string _pendingGameRestorePath: ""
    property string _backTransitionTarget: ""
    property string _pendingFolderBackTargetPath: ""
    property string _pendingFolderBackSystemId: ""
    property var _folderBackReadyCallback: null
    // System-cover prefetch gate. `_prefetchSystemCovers` populates
    // `_systemCoverPrefetchUrls` with the first-page logos and stores the
    // completion callback in `_systemCoverPrefetchCallback`. When every
    // Image signals Ready/Error (or `systemCoverPrefetchTimer` expires),
    // `_completePrefetchSystemCovers` fires the callback and clears state.
    property var _systemCoverPrefetchUrls: []
    property var _systemCoverPrefetchCallback: null
    property int _systemCoverPrefetchPending: 0

    function _catalogStillBooting(): bool {
        return !Browse.CategoriesModel.loaded && (Browse.CategoriesModel.error_message ?? "") === "";
    }

    function _completeDeferredCategoryIfReady(targetCategory: string): bool {
        if (root._categoryReadyCallback === null)
            return false;
        if (Browse.SystemsModel.loading)
            return false;
        if (Browse.SystemsModel.current_category !== targetCategory)
            return false;
        if (root._catalogStillBooting())
            return false;
        root._startupTrace("startup/qml deferred category ready", "category=" + targetCategory + " count=" + Browse.SystemsModel.count);
        const cb = root._categoryReadyCallback;
        root._categoryReadyCallback = null;
        cb();
        return true;
    }

    function _completeDeferredSystemIfReady(targetSystemId: string): bool {
        if (root._systemReadyCallback === null)
            return false;
        if (Browse.GamesModel.loading)
            return false;
        if (Browse.GamesModel.current_system_id !== targetSystemId)
            return false;
        root._startupTrace("startup/qml deferred system ready", "systemId=" + targetSystemId + " count=" + Browse.GamesModel.count);
        const cb = root._systemReadyCallback;
        root._systemReadyCallback = null;
        cb();
        return true;
    }

    function _completeFolderBackRebrowseIfReady(): bool {
        if (root._folderBackReadyCallback === null)
            return false;
        if (Browse.GamesModel.loading)
            return false;
        const cb = root._folderBackReadyCallback;
        root._folderBackReadyCallback = null;
        cb();
        return true;
    }

    function _maybeContinueOptimisticTransitions(): void {
        if (root._catalogStillBooting())
            return;
        if (root._catalogWaitCategory !== "" && root._categoryReadyCallback !== null) {
            const category = root._catalogWaitCategory;
            const cb = root._categoryReadyCallback;
            root._catalogWaitCategory = "";
            root._startupTrace("startup/qml catalog wait continue", "category=" + category);
            root._ensureCategory(category, cb, false);
        }
        if (root.pendingTransition === "favorites")
            favoritesTransitionTimer.restart();
        else if (root.pendingTransition === "recents")
            recentsTransitionTimer.restart();
        else if (root.pendingTransition === "settings")
            root._whenScreenReady(root.screenSettings, function () {
                if (root.pendingTransition === "settings")
                    root._completeTransition(root.screenSettings);
            });
        root._maybeCompleteBackTransition();
    }

    // Listen for SystemsModel fills owned by an in-flight transition.
    // `loading` flips true at the start of set_category and false when
    // the async tokio worker posts the filled rows back. Listening to
    // the false edge gives us a single, unambiguous "fill complete"
    // signal — onModelReset would also fire on the synchronous clear
    // (count=0) at the start of set_category, indistinguishable from a
    // category that legitimately fills with 0 rows. The callback slot
    // is consumed at most once per transition; a stray fire when no
    // transition is pending is a no-op.
    Connections {
        target: root._systemsModelConnectionsEnabled ? Browse.SystemsModel : null
        function onLoadingChanged(): void {
            if (Browse.SystemsModel.loading)
                return;
            // Deferred set_category hasn't fired yet — this false-edge
            // belongs to a prior in-flight fill, not our transition.
            if (root._deferredCategoryPending) {
                root._startupTrace("startup/qml category loading edge ignored", "reason=deferred-pending category=" + Browse.SystemsModel.current_category + " count=" + Browse.SystemsModel.count);
                return;
            }
            // Optimistic Hub can issue set_category before the catalog
            // exists. That worker legitimately resolves empty; keep the
            // normal loading cue up until CategoriesModel delivers an
            // authoritative loaded/error edge, then retry the category.
            if (root._catalogWaitCategory !== "" && root._catalogStillBooting())
                return;
            const cb = root._categoryReadyCallback;
            if (cb === null) {
                root._maybeCompleteBackTransition();
                return;
            }
            root._categoryReadyCallback = null;
            cb();
            root._maybeCompleteBackTransition();
        }
    }
    Connections {
        target: root._gamesModelConnectionsEnabled ? Browse.GamesModel : null
        function onLoadingChanged(): void {
            if (Browse.GamesModel.loading)
                return;
            if (root._deferredSystemPending) {
                root._startupTrace("startup/qml system loading edge ignored", "reason=deferred-pending systemId=" + Browse.GamesModel.current_system_id + " count=" + Browse.GamesModel.count);
                return;
            }
            const cb = root._systemReadyCallback;
            if (cb !== null) {
                root._systemReadyCallback = null;
                cb();
            }
            root._completeFolderBackRebrowseIfReady();
            root._maybeCompleteBackTransition();
        }
    }
    Connections {
        target: root._recentsModelConnectionsEnabled ? Browse.RecentsModel : null
        function onLoadingChanged(): void {
            if (Browse.RecentsModel.loading)
                return;
            root._maybeCompletePendingResumeLaunch();
            const cb = root._recentsReadyCallback;
            if (cb === null) {
                root._maybeCompleteBackTransition();
                return;
            }
            root._recentsReadyCallback = null;
            cb();
            root._maybeCompleteBackTransition();
        }

        function onResume_availableChanged(): void {
            root._maybeCompletePendingResumeLaunch();
        }
    }
    Connections {
        target: root._favoritesModelConnectionsEnabled ? Browse.FavoritesModel : null
        function onLoadingChanged(): void {
            if (Browse.FavoritesModel.loading)
                return;
            const cb = root._favoritesReadyCallback;
            if (cb === null) {
                root._maybeCompleteBackTransition();
                return;
            }
            root._favoritesReadyCallback = null;
            cb();
            root._maybeCompleteBackTransition();
        }
    }

    // Ensure SystemsModel is filled with `category`, then call cb().
    // Synchronous on the no-op return path (same category already
    // populated — a re-Accept after Esc-back); the set_category call
    // is still made for parity with the prior behaviour even though
    // Rust early-returns when the category already matches. Async
    // path waits for loadingChanged. The timer keeps the loadingChanged
    // bookkeeping on the same asynchronous path without deliberately
    // holding the transition long enough to show the global loading cue.
    function _ensureCategory(category: string, cb, waitForCatalog): void {
        if (waitForCatalog && root._catalogStillBooting()) {
            root._startupTrace("startup/qml catalog wait arm", "category=" + category);
            root._categoryReadyCallback = cb;
            root._catalogWaitCategory = category;
            return;
        }
        if (Browse.SystemsModel.current_category === category && Browse.SystemsModel.count > 0) {
            root._categoryReadyCallback = null;
            Browse.SystemsModel.set_category(category);
            cb();
            return;
        }
        root._startupTrace("startup/qml defer category set", "category=" + category);
        root._categoryReadyCallback = cb;
        root._deferredCategoryPending = true;
        deferredCategorySetTimer.targetCategory = category;
        deferredCategorySetTimer.interval = 1;
        deferredCategorySetTimer.restart();
    }

    // Ensure GamesModel is filled with `systemId`, then call cb().
    // When the system is already current and populated (re-Accept
    // after Esc-back), set_system still re-issues start_initial_browse,
    // but the cached result applies inline through the watcher's seed
    // — loading flips back to false before set_system returns — so we
    // can call cb() synchronously on this path. Cold-load goes through
    // the systemReadyCallback waiter below; when replacing populated
    // games rows during a transition, defer to the delayed loading cue
    // for the same pre-feedback-freeze reason as _ensureCategory.
    function _ensureSystem(systemId: string, cb): void {
        if (Browse.GamesModel.current_system_id === systemId && Browse.GamesModel.count > 0) {
            Browse.GamesModel.set_system(systemId);
            cb();
            return;
        }
        root._systemReadyCallback = cb;
        root._deferredSystemPending = true;
        deferredSystemSetTimer.targetSystemId = systemId;
        deferredSystemSetTimer.interval = root.pendingTransition !== "" && Browse.GamesModel.count > 0 ? root.loadingIndicatorDelayMs + 50 : 1;
        deferredSystemSetTimer.restart();
    }

    // Hub Accept routing. Empty-row passthrough preserves the committed
    // "Enter on empty hub goes to Systems" behaviour and
    // keeps the navigation test synchronous. The Resume action is a
    // hub payload rather than a category and launches the latest
    // resumable history row. Otherwise: tentatively pin the
    // destination to Systems, fill the chosen category, then either
    // bypass to Games (MiSTer Arcade singleton) or fall through to
    // Systems with a cover-prefetch warmup so the destination paints
    // with logos already in QPixmapCache.
    function _navigateFromHub(category: string): void {
        if (category === "") {
            root._goto(root.screenSystems);
            return;
        }
        if (category === "resume") {
            root._navigateResumeFromHub();
            return;
        }
        Browse.HubState.category = category;
        root._requestScreen(root.screenSystems);
        root.pendingTransition = "systems";
        root._ensureCategory(category, function () {
            const arcadeBypass = Browse.Platform.is_mister && Browse.Platform.ready && category === CategoryIds.arcadeId && Browse.SystemsModel.count === 1;
            console.log("arcade-bypass eval:", "category=" + JSON.stringify(category), "platform.is_mister=" + Browse.Platform.is_mister, "platform.ready=" + Browse.Platform.ready, "systems.count=" + Browse.SystemsModel.count, "→ bypass=" + arcadeBypass);
            if (arcadeBypass) {
                root._requestScreen(root.screenGames);
                const systemId = Browse.SystemsModel.system_id_at(0);
                Browse.SystemsState.system_id = systemId;
                Browse.GamesState.system_id = systemId;
                root.pendingTransition = "games";
                root._ensureSystem(systemId, function () {
                    root._completeTransition(root.screenGames);
                });
            } else {
                root._prefetchSystemCovers(function () {
                    root._completeTransition(root.screenSystems);
                });
            }
        }, true);
    }

    function _cancelResumeLaunch(): void {
        root._pendingResumeLaunch = false;
        if (root.pendingTransition === "resume")
            root.pendingTransition = "";
        if (root.activeScreen !== root.screenHub)
            root._goto(root.screenHub);
    }

    // Fire the actual resume launch and arm the desktop safety-clear. The
    // "Loading game…" cue (pendingTransition === "resume") stays up through
    // the launch: on MiSTer the process is replaced by the game before the
    // timer fires, so the cue covers the core swap; on desktop nothing
    // replaces us, so resumeLaunchCueTimer clears the cue and restores input.
    // Started only here, at dispatch — never while still waiting on the
    // connection — so a slow coalesce keeps the cue for as long as it needs.
    function _dispatchResumeLaunch(): void {
        Browse.RecentsModel.launch_resume();
        resumeLaunchCueTimer.restart();
    }

    function _maybeCompletePendingResumeLaunch(): void {
        if (!root._pendingResumeLaunch || root.pendingTransition !== "resume")
            return;
        if (Browse.RecentsModel.resume_loading)
            return;
        if (Browse.RecentsModel.resume_available) {
            root._pendingResumeLaunch = false;
            root._dispatchResumeLaunch();
            return;
        }
        if (Browse.AppStatus.connection_state === 2 || Browse.AppStatus.connection_state === 3)
            root._cancelResumeLaunch();
    }

    function _startResumeLaunch(): void {
        if (root.pendingTransition !== "resume")
            return;
        root._pendingResumeLaunch = true;
        root._maybeCompletePendingResumeLaunch();
    }

    function _navigateResumeFromHub(): void {
        // Optimistic loader, same contract as the other Hub actions: paint
        // the "Loading game…" cue (and hide the ghost-Hub tiles / gate input)
        // immediately, before we know whether the launch can proceed.
        // _cancelResumeLaunch clears it on the no-resumable-game branch.
        root.pendingTransition = "resume";
        if (!Browse.RecentsModel.resume_loading && Browse.RecentsModel.resume_available) {
            root._dispatchResumeLaunch();
            return;
        }
        if (Browse.RecentsModel.resume_loading || Browse.AppStatus.connection_state !== 2) {
            resumeLaunchTimer.restart();
            return;
        }
        root._cancelResumeLaunch();
    }

    function _completeFavoritesTransition(): void {
        if (root.pendingTransition !== "favorites")
            return;
        root.favoritesScreen.restoreSelection();
        root._completeTransition(root.screenFavorites);
    }

    function _startFavoritesTransitionLoad(): void {
        if (root.pendingTransition !== "favorites")
            return;
        root._whenScreenReady(root.screenFavorites, function () {
            if (root.pendingTransition !== "favorites")
                return;
            root._resumeFavoritesCovers();
            if (root._catalogStillBooting())
                return;
            if (!Browse.FavoritesModel.loading) {
                root._completeFavoritesTransition();
                return;
            }
            root._favoritesReadyCallback = root._completeFavoritesTransition;
        });
    }

    function _navigateToFavorites(): void {
        root.pendingTransition = "favorites";
        favoritesTransitionTimer.restart();
    }

    function _completeRecentsTransition(): void {
        if (root.pendingTransition !== "recents")
            return;
        root.recentsScreen.restoreSelection();
        root._completeTransition(root.screenRecents);
    }

    // Hub → Recents transition. The paginated history load is lazy so
    // Hub Resume does not pay for `media.history` during startup. Start
    // it only once the Recents screen is actually requested, then wait
    // on `loadingChanged` so the user sees the centred "Loading…" cue
    // rather than an empty grid.
    function _startRecentsTransitionLoad(): void {
        if (root.pendingTransition !== "recents")
            return;
        root._whenScreenReady(root.screenRecents, function () {
            if (root.pendingTransition !== "recents")
                return;
            Browse.RecentsModel.ensure_loaded();
            root._resumeRecentsCovers();
            if (root._catalogStillBooting())
                return;
            if (!Browse.RecentsModel.loading) {
                root._completeRecentsTransition();
                return;
            }
            root._recentsReadyCallback = root._completeRecentsTransition;
        });
    }

    function _navigateToRecents(): void {
        root.pendingTransition = "recents";
        recentsTransitionTimer.restart();
    }

    // Hub → Update transition. Placeholder screen with no async data,
    // so the flip is instant.
    function _navigateToUpdate(): void {
        if (!root.updateEnabled)
            return;
        root._goto(root.screenUpdate);
    }

    function _resumeFavoritesCovers(): void {
        Browse.FavoritesModel.cover_requests_paused = false;
        if (root.favoritesScreen === null)
            return;
        const first = root.favoritesScreen.mediaGrid.currentPage * root.favoritesScreen.mediaGrid.pageSize;
        Browse.FavoritesModel.refresh_cover_keys(first, root.favoritesScreen.mediaGrid.pageSize * 2);
    }

    function _resumeRecentsCovers(): void {
        Browse.RecentsModel.cover_requests_paused = false;
        if (root.recentsScreen === null)
            return;
        const first = root.recentsScreen.mediaGrid.currentPage * root.recentsScreen.mediaGrid.pageSize;
        Browse.RecentsModel.refresh_cover_keys(first, root.recentsScreen.mediaGrid.pageSize * 2);
    }

    // Hub → Settings transition. During optimistic boot, keep the same
    // centered Loading cue as other Hub actions until the catalog has
    // reached an authoritative state; after that Settings can flip
    // instantly because its singleton seeds from persisted state.
    function _navigateToSettings(): void {
        root._requestScreen(root.screenSettings);
        if (root._catalogStillBooting()) {
            root.pendingTransition = "settings";
            return;
        }
        root._whenScreenReady(root.screenSettings, function () {
            root._goto(root.screenSettings);
        });
    }

    // Settings → About transition. Static info screen, no async data,
    // so the flip is instant — same shape as _navigateToSettings above.
    function _navigateToAbout(): void {
        root._whenScreenReady(root.screenAbout, function () {
            root._goto(root.screenAbout);
        });
    }

    function _restoreSystemsScreenSelection(): void {
        const savedSystem = root.activeScreen === root.screenGames ? (Browse.GamesState.system_id !== "" ? Browse.GamesState.system_id : Browse.SystemsState.system_id) : Browse.SystemsState.system_id;
        const idx = savedSystem === "" ? -1 : Browse.SystemsModel.index_for_system_id(savedSystem);
        root.systemsScreen.systemsGrid.setCurrentIndexImmediate(idx >= 0 ? idx : 0);
        // Focus is now finalized from persisted state; let the grid render
        // focus (snapped, since the screen's _focusArmed is still false until
        // the first user input).
        root.systemsScreen._restoreDone = true;
        if (idx >= 0) {
            Browse.GamesModel.set_system(savedSystem);
            const stack = Browse.GamesState.path_stack;
            const top = stack.length > 0 ? stack[stack.length - 1] : "";
            if (top !== "")
                Browse.GamesModel.set_path(top);
        } else if (root.activeScreen === root.screenGames && Browse.SystemsModel.count > 0) {
            Browse.GamesModel.set_system(Browse.SystemsModel.system_id_at(0));
        }
    }

    function _setGamesRestoreIndex(index: int): void {
        if (root.gamesScreen === null)
            return;
        root.gamesScreen.suppressSelectionPersist = true;
        root.gamesScreen.gamesGrid.setCurrentIndexImmediate(index);
        root.gamesScreen.suppressSelectionPersist = false;
        // Selection finalized from persisted state; let the grid render focus
        // (snapped, since _focusArmed is still false until the first input).
        root.gamesScreen._restoreDone = true;
    }

    function _restoreGamesScreenSelection(): bool {
        const sels = Browse.GamesState.selected_at_level;
        const savedPath = sels.length > 0 ? sels[sels.length - 1] : "";
        const idx = savedPath === "" ? -1 : Browse.GamesModel.index_for_game_path(savedPath);
        if (idx >= 0) {
            root._setGamesRestoreIndex(idx);
            root._pendingGameRestorePath = "";
            return true;
        }
        if (savedPath !== "" && Browse.GamesModel.has_next_page) {
            root._pendingGameRestorePath = savedPath;
            root._setGamesRestoreIndex(0);
            Browse.GamesModel.fetch_more();
            return false;
        }
        root._pendingGameRestorePath = "";
        root._setGamesRestoreIndex(0);
        return true;
    }

    function _maybeFinishStartupGamesRestore(): void {
        if (!root._startupRestorePending || root._startupRestoreScreen !== root.screenGames)
            return;
        if (root._pendingGameRestorePath !== "")
            return;
        root._finishStartupRestore();
        root._goto(root.screenGames);
    }

    // Systems Accept routing. Pin destination to Games, fill the
    // chosen system, then flip. The Games→back routing decision is
    // re-evaluated live from current state at B-press time (see
    // gamesScreen.onRequestSystemsScreen below) so this path needs
    // no per-transition flag.
    function _navigateFromSystems(systemId: string): void {
        root._requestScreen(root.screenGames);
        Browse.SystemsState.system_id = systemId;
        // Setting system_id on GamesState resets path_stack/selected_at_level
        // to root level — the new system's browse always starts at the
        // initial games-screen view, regardless of where the user was in
        // a prior system's folder tree.
        Browse.GamesState.system_id = systemId;
        root.pendingTransition = "games";
        root._ensureSystem(systemId, function () {
            root._completeTransition(root.screenGames);
        });
    }

    // Folder drill-down inside the games screen. Stays on screenGames
    // — no pendingTransition flip — so the in-screen ScreenStateOverlay
    // handles the loading/empty/error cue while the new browse settles.
    // Pushes the new level onto GamesState before issuing the browse so
    // a kill mid-load still resumes inside the folder.
    function _navigateIntoFolder(path: string): void {
        if (path === "")
            return;
        Browse.GamesState.push_level(path, "");
        Browse.GamesModel.set_path(path);
    }

    function _rebrowseGamesFolderTarget(path: string, systemId: string): void {
        if (path === "") {
            if (systemId !== "")
                Browse.GamesModel.set_system(systemId);
        } else {
            Browse.GamesModel.set_path(path);
        }
    }

    // Folder pop-up inside the games screen. Cached parents take the same direct
    // path as folder drill-down, so Back does not show fake global Loading for a
    // browse result we can seed synchronously. Cold parents keep the delayed cue
    // before rebrowse so the UI does not look dead if the reset/RPC stalls.
    function _navigateOutOfFolder(): void {
        const stack = Browse.GamesState.path_stack;
        if (stack.length <= 1)
            return;
        Browse.GamesState.pop_level();
        const newStack = Browse.GamesState.path_stack;
        const target = newStack[newStack.length - 1];
        const systemId = target === "" ? Browse.GamesState.system_id : "";
        root._pendingFolderBackTargetPath = "";
        root._pendingFolderBackSystemId = "";
        folderBackTransitionTimer.stop();
        if (Browse.GamesModel.browse_cached_for_path(target)) {
            root._rebrowseGamesFolderTarget(target, systemId);
            root._resetIdle();
            return;
        }
        root.pendingTransition = "folder_back";
        root._pendingFolderBackTargetPath = target;
        root._pendingFolderBackSystemId = systemId;
        folderBackTransitionTimer.restart();
    }

    function _completeFolderBackTransition(): void {
        const target = root._pendingFolderBackTargetPath;
        const systemId = root._pendingFolderBackSystemId;
        root._pendingFolderBackTargetPath = "";
        root._pendingFolderBackSystemId = "";
        if (root.pendingTransition !== "folder_back")
            return;
        root._folderBackReadyCallback = root._finishFolderBackTransition;
        root._rebrowseGamesFolderTarget(target, systemId);
        root._completeFolderBackRebrowseIfReady();
    }

    function _finishFolderBackTransition(): void {
        if (root.pendingTransition !== "folder_back")
            return;
        root.pendingTransition = "";
        root._resetIdle();
    }

    // Clear the pending flag, then flip. Order matters: clearing
    // first lets the destination screen paint without the overlay
    // still drawing over it, and lets bindings dependent on
    // pendingTransition (source screen visibility) settle to the
    // post-transition state in the same frame as the screen swap.
    function _completeTransition(screen: string): void {
        root._startupTrace("startup/qml completeTransition", "to=" + screen, "from=" + root.activeScreen);
        root.pendingTransition = "";
        root._goto(screen);
        // Restart the idle countdown so the screensaver gate (which
        // skips activation while a transition is in flight) does not
        // leave the timer dead after the gate opens. No-op when the
        // screensaver setting is "off".
        root._resetIdle();
    }

    function _finishStartupRestore(): void {
        const restoredScreen = root._startupRestoreScreen;
        root._startupTrace("startup/qml finishStartupRestore", "target=" + restoredScreen, "activeScreen=" + root.activeScreen);
        startupRestoreKickTimer.stop();
        root._startupRestorePending = false;
        root._startupRestoreStarted = false;
        root._startupRestoreScreen = "";
        root.startupRestoreCurtainVisible = false;
        if (restoredScreen === "" || restoredScreen === root.screenHub)
            root._maybeArmHubResumeFocus();
    }

    function _maybeArmHubResumeFocus(): void {
        if (root.activeScreen !== root.screenHub || root._startupRestorePending)
            return;
        root.hubScreen.focusResumeIfVisible();
    }

    function _maybeStartStartupRestore(): void {
        if (!root._startupRestorePending || root._startupRestoreStarted)
            return;
        if (!root._firstFrameSeen)
            return;
        if (startupRestoreKickTimer.running)
            return;
        const targetScreen = root._startupRestoreScreen;
        if (targetScreen !== root.screenSettings && targetScreen !== root.screenAbout && Browse.AppStatus.connection_state !== 2)
            return;
        root._startupTrace("startup/qml maybeStartStartupRestore", "target=" + targetScreen, "categories=" + Browse.CategoriesModel.count, "systems=" + Browse.SystemsModel.count, "recentsLoading=" + Browse.RecentsModel.loading, "favoritesLoading=" + Browse.FavoritesModel.loading);
        if (targetScreen === "") {
            root._finishStartupRestore();
            return;
        }
        root._startupRestoreStarted = true;
        if (targetScreen === root.screenSettings || targetScreen === root.screenAbout) {
            root._whenScreenReady(targetScreen, function () {
                root._finishStartupRestore();
                root._goto(targetScreen);
            });
            return;
        }
        if (targetScreen === root.screenFavorites) {
            root._whenScreenReady(root.screenFavorites, function () {
                root._resumeFavoritesCovers();
                if (Browse.FavoritesModel.loading) {
                    root._favoritesReadyCallback = function () {
                        root.favoritesScreen.restoreSelection();
                        root._finishStartupRestore();
                        root._goto(root.screenFavorites);
                    };
                } else {
                    root.favoritesScreen.restoreSelection();
                    root._finishStartupRestore();
                    root._goto(root.screenFavorites);
                }
            });
            return;
        }
        if (targetScreen === root.screenRecents) {
            root._whenScreenReady(root.screenRecents, function () {
                Browse.RecentsModel.ensure_loaded();
                root._resumeRecentsCovers();
                if (Browse.RecentsModel.loading) {
                    root._recentsReadyCallback = function () {
                        root.recentsScreen.restoreSelection();
                        root._finishStartupRestore();
                        root._goto(root.screenRecents);
                    };
                } else {
                    root.recentsScreen.restoreSelection();
                    root._finishStartupRestore();
                    root._goto(root.screenRecents);
                }
            });
            return;
        }
        if (Browse.CategoriesModel.count <= 0) {
            const catalogError = Browse.CategoriesModel.error_message ?? "";
            if (!Browse.CategoriesModel.loaded && catalogError === "") {
                root._startupRestoreStarted = false;
                root._startupTrace("startup/qml startupRestore waitingForCatalog");
                return;
            }
            root._startupTrace("startup/qml startupRestore emptyCatalog", "loaded=" + Browse.CategoriesModel.loaded, "error=" + catalogError);
            root._finishStartupRestore();
            root._goto(targetScreen);
            return;
        }
        if (targetScreen === root.screenHub) {
            root.hubScreen.restoreFromCategoriesReset(true);
            root._finishStartupRestore();
            root._goto(root.screenHub);
            return;
        }
        const category = Browse.HubState.category;
        if (category === "") {
            root._startupTrace("startup/qml startupRestore missingCategory");
            root._finishStartupRestore();
            return;
        }
        if (targetScreen === root.screenSystems || targetScreen === root.screenGames)
            root._requestScreen(root.screenSystems);
        if (targetScreen === root.screenGames)
            root._requestScreen(root.screenGames);
        root._ensureCategory(category, function () {
            const arcadeBypass = Browse.Platform.is_mister && Browse.Platform.ready && category === CategoryIds.arcadeId && Browse.SystemsModel.count === 1;
            const arcadeSystemId = arcadeBypass ? Browse.SystemsModel.system_id_at(0) : "";
            root._startupTrace("startup/qml startupRestore categoryReady", "category=" + category, "target=" + targetScreen, "arcadeBypass=" + arcadeBypass, "systemsCount=" + Browse.SystemsModel.count);
            if (targetScreen === root.screenSystems) {
                if (arcadeBypass) {
                    Browse.SystemsState.system_id = arcadeSystemId;
                    Browse.GamesState.system_id = arcadeSystemId;
                    root._startupRestoreScreen = root.screenGames;
                    root.activeScreen = root.screenGames;
                    root._ensureSystem(arcadeSystemId, function () {
                        root._whenScreenReady(root.screenGames, function () {
                            if (root._restoreGamesScreenSelection())
                                root._maybeFinishStartupGamesRestore();
                        });
                    });
                    return;
                }
                root._whenScreenReady(root.screenSystems, function () {
                    root._restoreSystemsScreenSelection();
                    root._finishStartupRestore();
                    root._goto(root.screenSystems);
                });
                return;
            }
            const systemId = Browse.GamesState.system_id !== "" ? Browse.GamesState.system_id : (Browse.SystemsState.system_id !== "" ? Browse.SystemsState.system_id : arcadeSystemId);
            if (systemId === "") {
                root._startupTrace("startup/qml startupRestore missingSystemId", "category=" + category, "target=" + targetScreen);
                root._finishStartupRestore();
                return;
            }
            root._whenScreenReady(root.screenSystems, function () {
                root._restoreSystemsScreenSelection();
                root._systemReadyCallback = function () {
                    root._startupTrace("startup/qml startupRestore systemReady", "systemId=" + Browse.GamesModel.current_system_id, "target=" + targetScreen);
                    root._whenScreenReady(root.screenGames, function () {
                        if (root._restoreGamesScreenSelection())
                            root._maybeFinishStartupGamesRestore();
                    });
                };
                if (!Browse.GamesModel.loading) {
                    const cb = root._systemReadyCallback;
                    root._systemReadyCallback = null;
                    cb();
                }
            });
        });
    }

    Timer {
        id: startupRestoreKickTimer
        interval: 120
        repeat: false
        onTriggered: root._maybeStartStartupRestore()
    }

    onSystemsScreenChanged: root._flushScreenReady(root.screenSystems)
    onGamesScreenChanged: root._flushScreenReady(root.screenGames)
    onFavoritesScreenChanged: root._flushScreenReady(root.screenFavorites)
    onRecentsScreenChanged: root._flushScreenReady(root.screenRecents)
    onSettingsScreenChanged: root._flushScreenReady(root.screenSettings)
    onAboutScreenChanged: root._flushScreenReady(root.screenAbout)

    Connections {
        target: root.hubScreen
        function onRequestAccept(category: string): void {
            root._navigateFromHub(category);
        }
        function onRequestQuit(): void {
            root.openQuitConfirmModal();
        }
        function onRequestFavoritesScreen(): void {
            root._navigateToFavorites();
        }
        function onRequestRecentsScreen(): void {
            root._navigateToRecents();
        }
        function onRequestUpdateScreen(): void {
            root._navigateToUpdate();
        }
        function onRequestSettingsScreen(): void {
            root._navigateToSettings();
        }
        function onRequestContextMenu(index: int, anchorRect): void {
            root.openContextMenu("categories", index, anchorRect);
        }
    }
    Connections {
        target: root.favoritesScreen
        function onRequestHubScreen(): void {
            root._navigateBackToScreen(root.screenHub);
        }
        function onRequestContextMenu(index: int, anchorRect): void {
            root.openContextMenu("favorites", index, anchorRect);
        }
        function onRequestPageMenu(): void {
            root.openFavoritesPageMenu();
        }
    }
    Connections {
        target: root.recentsScreen
        function onRequestHubScreen(): void {
            root._navigateBackToScreen(root.screenHub);
        }
        function onRequestContextMenu(index: int, anchorRect): void {
            root.openContextMenu("recents", index, anchorRect);
        }
    }
    Connections {
        target: root.updateScreen
        function onRequestHubScreen(): void {
            root._goto(root.screenHub);
        }
    }
    Connections {
        target: root.settingsScreen
        function onRequestHubScreen(): void {
            root._goto(root.screenHub);
        }
        function onRequestAccept(actionId: string): void {
            if (actionId === "uploadLog")
                root.openLogUploadModal();
            else if (actionId === "aboutLicense")
                root._navigateToAbout();
            else if (actionId === "crtEnable" || actionId === "crtDisable")
                root.stageCrtToggle(actionId === "crtEnable");
            else if (actionId === "crtCalibration")
                root.openCrtCalibrationModal();
        }
        function onRequestListPicker(title: string, entries: var, initialId: string, fieldId: string): void {
            root.openListPickerModal(title, entries, initialId, fieldId);
        }
    }
    Connections {
        target: root.aboutScreen
        function onRequestSettingsScreen(): void {
            root._goto(root.screenSettings);
        }
    }
    Connections {
        target: root.systemsScreen
        function onRequestAccept(systemId: string): void {
            // Empty payload is the [OK] RETRY contract from the help
            // bar — Empty/Error states re-fire the current load
            // rather than drilling. Loading swallows the press at the
            // screen layer (no signal emitted), so this branch only
            // sees user intent on a non-Ready state.
            if (systemId === "") {
                const cat = Browse.SystemsModel.current_category;
                if (cat !== "")
                    Browse.SystemsModel.set_category(cat);
                return;
            }
            // Launch-only (virtual) systems carry a zapScript and have no
            // browsable media. Run them directly and stay on the systems
            // grid; never route into an empty games browse.
            if (Browse.SystemsModel.is_launchable_system(systemId)) {
                Browse.SystemsModel.launch_system_id(systemId);
                return;
            }
            root._navigateFromSystems(systemId);
        }
        function onRequestHubScreen(): void {
            root._navigateBackToScreen(root.screenHub);
        }
        function onRequestContextMenu(index: int, anchorRect): void {
            root.openContextMenu("systems", index, anchorRect);
        }
    }
    Connections {
        target: root.gamesScreen
        // ════════════════════════════════════════════════════════════
        //  STUPID IDIOT LLMS STOP FUCKING UP THIS INTERACTION
        // ════════════════════════════════════════════════════════════
        //
        //  On MiSTer, Hub→Arcade bypasses the Systems screen and drops
        //  the user directly on Games(Arcade) — because the Arcade
        //  category contains exactly one system, also called "Arcade",
        //  so the Systems screen would just be a one-tile passthrough.
        //
        //  THE ONLY CORRECT BACK BEHAVIOUR FROM GAMES(ARCADE) ON MISTER
        //  IS HUB. Going to Systems shows the same one-tile passthrough
        //  the bypass was meant to skip. Do not "fix" this by routing
        //  back through Systems "for consistency". Do not introduce a
        //  per-session flag and forget to persist it. Do not gate on
        //  Runtime instead of Platform — Platform is where Core runs;
        //  Runtime is where the frontend runs; a desktop frontend
        //  pointed at a remote MiSTer Core MUST still bypass.
        //
        //  Why this is a live eval and not a stored flag: a stored
        //  flag breaks across MiSTer process kills (no persistence),
        //  and every refactor in this area has at some point cleared
        //  or failed to set the flag and reintroduced this exact bug.
        //  The "should back skip Systems?" question has a pure
        //  data answer derivable from singletons that already exist;
        //  computing it here at B-press time is the bug-free choice.
        //
        //  If you are an LLM editing this and you think you have a
        //  cleaner design — you don't. Read the user's frustration in
        //  the commit history. Leave it alone.
        // ════════════════════════════════════════════════════════════
        function onRequestSystemsScreen(): void {
            const arcadeBypassActive = Browse.Platform.is_mister && Browse.Platform.ready && Browse.SystemsModel.current_category === CategoryIds.arcadeId && Browse.SystemsModel.count === 1 && Browse.GamesModel.current_system_id === CategoryIds.arcadeId;
            if (arcadeBypassActive) {
                root._navigateBackToScreen(root.screenHub);
                return;
            }
            root._navigateBackToScreen(root.screenSystems);
        }
        function onRequestNavigateIntoFolder(path: string): void {
            root._navigateIntoFolder(path);
        }
        function onRequestNavigateOutOfFolder(): void {
            root._navigateOutOfFolder();
        }
        function onRequestContextMenu(index: int, anchorRect): void {
            root.openContextMenu("games", index, anchorRect);
        }
        function onRequestPageMenu(): void {
            root.openPageMenu();
        }
    }

    onActiveCardWritePendingChanged: root.handleCardWriteStatus()
    onActiveCardWriteErrorChanged: root.handleCardWriteStatus()
    onCancelCardWriteRequested: root.cancelCardWrite()
    onCloseGameInfoRequested: root.closeGameInfoModal()
    onCloseQrCodeRequested: root.closeQrCodeModal()
    onContextMenuCloseRequested: root.handleContextMenuCloseRequested()
    onContextMenuAccepted: id => root.handleContextMenuAccepted(id)
    Connections {
        target: Browse.AlternateVersions
        function onLoadingChanged(): void {
            if (Browse.AlternateVersions.loading || !root._discoverMenuPending)
                return;
            root._discoverMenuPending = false;
            if (!root.contextMenuVisible || root.contextMenuMode !== "main")
                return;
            if (Browse.AlternateVersions.count <= 0)
                root._replaceContextMenuEntryLabel("discover_loading", "No alternates found", "discover_unavailable");
            if (Browse.AlternateVersions.count <= 0)
                return;
            const entries = [];
            for (let i = 0; i < Browse.AlternateVersions.count; i++) {
                entries.push({
                    id: "alternate_version:" + i,
                    label: Browse.AlternateVersions.name_at(i)
                });
            }
            if (entries.length === 0)
                return;
            root._discoverParentEntries = root.contextMenuEntries;
            root.contextMenuEntries = entries;
            root.contextMenuMode = "alternate_versions";
            if (root.contextMenu !== null)
                root.contextMenu.currentIndex = 0;
        }
    }

    // Pure helper — owner/entryType/mediaCapable/hasNfc/isFavorite → list of `{id,label}` entries.
    // Empty list = no menu (caller bails out of openContextMenu).
    //
    // Annotated as `: var` (not `list<var>`): MiSTer's AOT-compiled
    // static QML build coerces the JS array through `list<var>` and the
    // caller saw `entries.length === 0` despite the function pushing 3
    // items in. Plain `var` round-trips cleanly and silences the
    // "insufficiently annotated" coercion warning at the call site.
    function buildContextMenuEntries(owner: string, entryType: string, mediaCapable: bool, hasNfc: bool, isFavorite: bool, systemId: string, isHidden: bool, category: string) {
        if (owner === "systems") {
            const entries = [
                {
                    id: "launch_system",
                    label: qsTr("Launch core")
                }
            ];
            // Launch-only (virtual) systems have no media and no launcher
            // choice, so omit launcher/index/scrape actions for them.
            const launchable = Browse.SystemsModel.is_launchable_system(systemId);
            if (!launchable && !Browse.SystemLaunchers.loading && Browse.SystemLaunchers.error_message === "" && Browse.SystemLaunchers.launcher_count_for_system(systemId) > 0) {
                entries.push({
                    id: "change_launcher",
                    label: qsTr("Change launcher")
                });
            }
            const mediaBusy = Browse.MediaStatus.indexing || Browse.MediaStatus.optimizing || Browse.MediaStatus.scraping;
            if (!launchable && !mediaBusy) {
                entries.push({
                    id: "index_system",
                    label: qsTr("Update media database")
                }, {
                    id: "scrape_system",
                    label: qsTr("Scrape metadata")
                });
            }
            entries.push({
                id: "toggle_hide_system",
                label: isHidden ? qsTr("Unhide") : qsTr("Hide")
            });
            return entries;
        }
        if (owner === "categories") {
            const mediaBusy = Browse.MediaStatus.indexing || Browse.MediaStatus.optimizing || Browse.MediaStatus.scraping;
            const entries = [
                {
                    id: "toggle_hide_category",
                    label: isHidden ? qsTr("Unhide") : qsTr("Hide")
                }
            ];
            // Index/scrape act on the category's indexable systems, which
            // excludes launch-only ones. Show the actions only when the
            // category has at least one indexable system; a category whose
            // members are all launch-only yields none and the actions would
            // no-op, so omit them rather than show dead entries.
            const hasIndexable = category !== "" && Browse.SystemsModel.system_ids_for_category(category).length > 0;
            if (!mediaBusy && hasIndexable) {
                entries.push({
                    id: "index_category",
                    label: qsTr("Update media database")
                }, {
                    id: "scrape_category",
                    label: qsTr("Scrape metadata")
                });
            }
            return entries;
        }
        if (owner === "recents") {
            const entries = [
                {
                    id: "launch_game",
                    label: qsTr("Launch game")
                }
            ];
            if (Browse.Settings.current_discover_arcade_alternate_versions)
                entries.unshift({
                    id: "discover",
                    label: "Discover alt. versions"
                });
            return entries;
        }
        if (owner === "games" || owner === "favorites") {
            if ((entryType === "directory" || entryType === "root") && !mediaCapable)
                return [];
            const entries = [];
            entries.push({
                id: "toggle_favorite",
                label: isFavorite ? qsTr("Remove from favorites") : qsTr("Add to favorites")
            });
            if (hasNfc)
                entries.push({
                    id: "write_card",
                    label: qsTr("Write to NFC token")
                });
            entries.push({
                id: "qr_code",
                label: qsTr("QR code")
            });
            if (Browse.Settings.current_discover_arcade_alternate_versions) {
                entries.push({
                    id: "discover",
                    label: "Discover alt. versions"
                });
            }
            entries.push({
                id: "launch_game",
                label: qsTr("Launch game")
            });
            return entries;
        }
        return [];
    }

    // Pure helper — wrap a zapscript in the zaparoo.app deep-link template.
    // The QR code points the scanning device at this URL; the web app
    // hands the scanned zapscript back to a Core/frontend pairing.
    function _buildQrPayload(zapscript: string): string {
        return "https://zaparoo.app/write?v=" + encodeURIComponent(zapscript);
    }

    function _replaceContextMenuEntryLabel(targetId: string, nextLabel: string, nextId: string): void {
        const entries = [];
        for (let i = 0; i < root.contextMenuEntries.length; i++) {
            const entry = root.contextMenuEntries[i];
            if (entry.id === targetId) {
                entries.push({
                    id: nextId === undefined ? entry.id : nextId,
                    label: nextLabel
                });
            } else {
                entries.push(entry);
            }
        }
        root.contextMenuEntries = entries;
    }

    function _restoreDiscoverContextMenuEntry(entriesIn: var): var {
        const entries = [];
        for (let i = 0; i < entriesIn.length; i++) {
            const entry = entriesIn[i];
            if (entry.id === "discover_loading" || entry.id === "discover_unavailable") {
                entries.push({
                    id: "discover",
                    label: "Discover alt. versions"
                });
            } else {
                entries.push(entry);
            }
        }
        return entries;
    }

    function openContextMenu(owner: string, index: int, anchorRect): void {
        if (index < 0)
            return;
        let entryType = "";
        let isFavorite = false;
        let systemId = "";
        let mediaCapable = false;
        let isHidden = false;
        let category = "";
        if (owner === "systems") {
            if (index >= Browse.SystemsModel.count)
                return;
            systemId = Browse.SystemsModel.system_id_at(index);
            isHidden = Browse.SystemsModel.is_hidden_at(index);
        } else if (owner === "categories") {
            if (index >= Browse.CategoriesModel.count)
                return;
            category = Browse.CategoriesModel.category_at(index);
            isHidden = Browse.CategoriesModel.is_hidden_at(index);
        } else if (owner === "games") {
            if (index >= Browse.GamesModel.count)
                return;
            entryType = Browse.GamesModel.entry_type_at(index);
            mediaCapable = Browse.GamesModel.is_media_capable_at(index);
            isFavorite = Browse.GamesModel.is_favorite_at(index);
        } else if (owner === "favorites") {
            if (index >= Browse.FavoritesModel.count)
                return;
            mediaCapable = true;
            isFavorite = Browse.FavoritesModel.is_favorite_at(index);
        } else if (owner === "recents") {
            if (index >= Browse.RecentsModel.count)
                return;
        }
        const entries = root.buildContextMenuEntries(owner, entryType, mediaCapable, Browse.SystemStatus.has_nfc, isFavorite, systemId, isHidden, category);
        if (entries.length === 0)
            return;
        root.contextMenuEntries = entries;
        root.contextMenuOwner = owner;
        root.contextMenuIndex = index;
        root.contextMenuMode = "main";
        root._discoverParentEntries = [];
        root._discoverMenuPending = false;
        root.contextMenuAnchor = anchorRect;
        root._requestModal(root.modalContextMenu);
        root.contextMenuVisible = true;
        if (ScreenManager.topModal !== root.modalContextMenu)
            ScreenManager.pushModal(root.modalContextMenu);
    }

    function handleContextMenuCloseRequested(): void {
        if (root.contextMenuMode === "alternate_versions") {
            root.contextMenuEntries = root._restoreDiscoverContextMenuEntry(root._discoverParentEntries);
            root._discoverParentEntries = [];
            root.contextMenuMode = "main";
            root._discoverMenuPending = false;
            return;
        }
        root.closeContextMenu();
    }

    function closeContextMenu(): void {
        root.contextMenuVisible = false;
        root.contextMenuOwner = "";
        root.contextMenuIndex = -1;
        root.contextMenuMode = "main";
        root._discoverParentEntries = [];
        root._discoverMenuPending = false;
        root.contextMenuEntries = [];
        if (ScreenManager.topModal === root.modalContextMenu)
            ScreenManager.popModal();
    }

    function handleContextMenuAccepted(id: string): void {
        const owner = root.contextMenuOwner;
        const targetIndex = root.contextMenuIndex;
        if (targetIndex < 0)
            return;
        if (id === "discover") {
            let systemId = "";
            let name = "";
            let path = "";
            if (owner === "games") {
                systemId = Browse.GamesModel.system_id_at(targetIndex);
                name = Browse.GamesModel.name_at(targetIndex);
                path = Browse.GamesModel.path_at(targetIndex);
            } else if (owner === "favorites") {
                systemId = Browse.FavoritesModel.system_id_at(targetIndex);
                name = Browse.FavoritesModel.name_at(targetIndex);
                path = Browse.FavoritesModel.path_at(targetIndex);
            } else if (owner === "recents") {
                systemId = Browse.RecentsModel.system_id_at(targetIndex);
                name = Browse.RecentsModel.name_at(targetIndex);
                path = Browse.RecentsModel.path_at(targetIndex);
            }
            root._discoverMenuPending = true;
            root._replaceContextMenuEntryLabel("discover", "Searching....", "discover_loading");
            Browse.AlternateVersions.discover_for(systemId, name, path);
            return;
        }
        root.closeContextMenu();
        if (id === "change_launcher") {
            const systemId = Browse.SystemsModel.system_id_at(targetIndex);
            if (systemId === "")
                return;
            Browse.SystemLaunchers.prepare_system(systemId);
            const entries = [];
            for (let i = 0; i < Browse.SystemLaunchers.picker_ids.length; i++) {
                const launcherId = Browse.SystemLaunchers.picker_ids[i];
                const label = Browse.SystemLaunchers.picker_labels[i];
                entries.push({
                    id: launcherId,
                    label: launcherId === "__default__" ? qsTr("Default") : (label.indexOf("Current: ") === 0 ? qsTr("Current: %1").arg(launcherId) : label)
                });
            }
            if (entries.length > 0)
                root.openListPickerModal(qsTr("Change launcher"), entries, Browse.SystemLaunchers.current_launcher, "system_launcher:" + systemId);
        } else if (id.startsWith("alternate_version:")) {
            const altIndex = Number(id.slice("alternate_version:".length));
            if (!Number.isNaN(altIndex))
                Browse.AlternateVersions.launch_at(altIndex);
        } else if (id === "launch_system") {
            Browse.SystemsModel.launch_at(targetIndex);
        } else if (id === "index_system") {
            const systemId = Browse.SystemsModel.system_id_at(targetIndex);
            if (systemId !== "")
                Browse.MediaStatus.start_index_for_system(systemId);
        } else if (id === "scrape_system") {
            const systemId = Browse.SystemsModel.system_id_at(targetIndex);
            if (systemId !== "")
                Browse.MediaStatus.start_scrape_for_system(systemId);
        } else if (id === "toggle_hide_system") {
            const systemId = Browse.SystemsModel.system_id_at(targetIndex);
            if (systemId !== "") {
                if (Browse.SystemsState.is_system_hidden(systemId))
                    Browse.SystemsState.unhide_system(systemId);
                else
                    Browse.SystemsState.hide_system(systemId);
                Browse.SystemsModel.reproject();
            }
        } else if (id === "toggle_hide_category") {
            const categoryName = Browse.CategoriesModel.category_at(targetIndex);
            if (categoryName !== "") {
                if (Browse.HubState.is_category_hidden(categoryName))
                    Browse.HubState.unhide_category(categoryName);
                else
                    Browse.HubState.hide_category(categoryName);
                Browse.CategoriesModel.reproject();
            }
        } else if (id === "index_category") {
            const categoryName = Browse.CategoriesModel.category_at(targetIndex);
            if (categoryName !== "") {
                const systemIds = Browse.SystemsModel.system_ids_for_category(categoryName);
                if (systemIds.length > 0)
                    Browse.MediaStatus.start_index_for_systems(systemIds);
            }
        } else if (id === "scrape_category") {
            const categoryName = Browse.CategoriesModel.category_at(targetIndex);
            if (categoryName !== "") {
                const systemIds = Browse.SystemsModel.system_ids_for_category(categoryName);
                if (systemIds.length > 0)
                    Browse.MediaStatus.start_scrape_for_systems(systemIds);
            }
        } else if (id === "launch_game") {
            if (owner === "favorites")
                Browse.FavoritesModel.launch_at(targetIndex);
            else if (owner === "recents")
                Browse.RecentsModel.launch_at(targetIndex);
            else
                Browse.GamesModel.launch_at(targetIndex);
        } else if (id === "toggle_favorite") {
            if (owner === "games")
                Browse.GamesModel.toggle_favorite_at(targetIndex);
            else if (owner === "favorites")
                Browse.FavoritesModel.toggle_favorite_at(targetIndex);
        } else if (id === "more_info") {
            root.openGameInfo(owner, targetIndex);
        } else if (id === "write_card") {
            if (owner === "systems") {
                root.beginCardWrite("systems");
                Browse.SystemsModel.write_card_at(targetIndex);
            } else if (owner === "games") {
                root.beginCardWrite("games");
                Browse.GamesModel.write_card_at(targetIndex);
            } else if (owner === "favorites") {
                root.beginCardWrite("favorites");
                Browse.FavoritesModel.write_card_at(targetIndex);
            }
        } else if (id === "qr_code") {
            const text = owner === "systems" ? Browse.SystemsModel.launch_text_at(targetIndex) : owner === "games" ? Browse.GamesModel.launch_text_at(targetIndex) : owner === "favorites" ? Browse.FavoritesModel.launch_text_at(targetIndex) : "";
            if (text !== "") {
                Browse.QrCode.generate(root._buildQrPayload(text));
                root.openQrCodeModal();
            }
        } else if (id === "discover_unavailable" || id === "discover_loading") {
            return;
        }
    }

    function openGameInfo(owner: string, index: int): void {
        let systemId = "";
        let path = "";
        let title = "";
        if (owner === "games") {
            systemId = Browse.GamesModel.system_id_at(index);
            path = Browse.GamesModel.path_at(index);
            title = Browse.GamesModel.name_at(index);
        } else if (owner === "favorites") {
            systemId = Browse.FavoritesModel.system_id_at(index);
            path = Browse.FavoritesModel.path_at(index);
            title = Browse.FavoritesModel.name_at(index);
        } else if (owner === "recents") {
            systemId = Browse.RecentsModel.system_id_at(index);
            path = Browse.RecentsModel.path_at(index);
            title = Browse.RecentsModel.name_at(index);
        }
        if (systemId === "" || path === "")
            return;
        Browse.GameInfo.load(systemId, path, title);
        root._requestModal(root.modalGameInfo);
        root.gameInfoModalVisible = true;
        if (ScreenManager.topModal !== root.modalGameInfo)
            ScreenManager.pushModal(root.modalGameInfo);
    }

    function closeGameInfoModal(): void {
        root.gameInfoModalVisible = false;
        Browse.GameInfo.clear();
        if (ScreenManager.topModal === root.modalGameInfo)
            ScreenManager.popModal();
    }

    function openQrCodeModal(): void {
        root._requestModal(root.modalQrCode);
        root.qrCodeModalVisible = true;
        if (ScreenManager.topModal !== root.modalQrCode)
            ScreenManager.pushModal(root.modalQrCode);
    }

    function closeQrCodeModal(): void {
        root.qrCodeModalVisible = false;
        if (ScreenManager.topModal === root.modalQrCode)
            ScreenManager.popModal();
    }

    // First-run modal lifecycle. Push exactly once per session, the
    // moment the catalog resolves Ready and reports zero *indexed*
    // systems (`CategoriesModel.loaded === true && indexed_count === 0`).
    // We gate on `indexed_count`, not `count`: since Core's launchables
    // feature, a device with no mediadb still returns launch-only virtual
    // systems (non-empty `zapScript`) that land in the `Other` category,
    // so `count`/`raw_count` are non-zero even when nothing is indexed.
    // `indexed_count` ignores launchables, so it answers "are there
    // indexed games to show?" — which is exactly the first-run question.
    // The `loaded` gate is critical: the singleton's Default state has
    // `indexed_count: 0` before the catalog fetch lands, so without it
    // we'd fire the modal on cold launch before Core has answered. Gating
    // on the catalog instead of MediaStatus.exists/seeded avoids the case
    // where Core reports `database.exists: true` for an empty file.
    function _maybeOpenFirstRunIndex(): void {
        if (root._firstRunIndexShown)
            return;
        // Defer to the commercial-use notice. The notice's close handler
        // calls back into here once acked, so chaining is automatic and
        // we avoid stacking two modals at the same time.
        if (!Browse.Notice.commercial_ack)
            return;
        // Never open while the Core-version warning is still on screen —
        // `_coreVersionWarningShown` flips true when the warning *opens*, so
        // without this guard a model signal arriving before the user
        // dismisses it would stack the first-run modal on top.
        if (root.coreVersionModalVisible)
            return;
        // Defer to the Core-version warning, which sits between the notice
        // and this modal in the chain. Until that gate has resolved (shown
        // or skipped, flipping `_coreVersionWarningShown`), hand off to it
        // and let it call back here — so the two never stack and the
        // warning always comes first.
        if (!root._coreVersionWarningShown) {
            root._maybeOpenCoreVersionWarning();
            return;
        }
        if (Browse.AppStatus.connection_state !== 2)
            return;
        if (!Browse.CategoriesModel.loaded)
            return;
        if (Browse.CategoriesModel.indexed_count > 0)
            return;
        root._firstRunIndexShown = true;
        root._requestModal(root.modalFirstRunIndex);
        root.firstRunIndexModalVisible = true;
        if (ScreenManager.topModal !== root.modalFirstRunIndex)
            ScreenManager.pushModal(root.modalFirstRunIndex);
    }

    function closeFirstRunIndexModal(): void {
        root.firstRunIndexModalVisible = false;
        if (ScreenManager.topModal === root.modalFirstRunIndex)
            ScreenManager.popModal();
    }

    // Commercial-use first-run notice. Persisted ack lives in
    // `frontend.toml` (not state.toml — MiSTer's tmpfs would re-show
    // the notice on every reboot). The router opens the modal on first
    // paint when the flag is false, and the modal's close handler is
    // what advances to the next first-run gate (mediadb index).
    function _maybeOpenCommercialNotice(): void {
        if (Browse.Notice.commercial_ack)
            return;
        if (root.commercialNoticeModalVisible)
            return;
        // Defer until the cold-launch curtain has lifted. Otherwise
        // the modal paints over the BootOverlay's "Connecting…" cue,
        // and the user perceives the frontend as stuck — they can't
        // tell whether dismissing the notice will reveal a working
        // app or an actual connection failure. Waiting for boot means
        // every "I understand" press lands on a hub that's already
        // ready to use.
        if (!root.bootComplete)
            return;
        root._requestModal(root.modalCommercialNotice);
        root.commercialNoticeModalVisible = true;
        if (ScreenManager.topModal !== root.modalCommercialNotice)
            ScreenManager.pushModal(root.modalCommercialNotice);
    }

    function closeCommercialNoticeModal(): void {
        root.commercialNoticeModalVisible = false;
        if (ScreenManager.topModal === root.modalCommercialNotice)
            ScreenManager.popModal();
        // Now that the notice is dismissed, advance the first-run chain:
        // commercial notice → Core-version warning → media-DB first run.
        // Each gate early-returns until its own condition holds, so this
        // is safe to call unconditionally.
        root._maybeOpenCoreVersionWarning();
    }

    // Core-version warning lifecycle. Shown once per session, behind the
    // commercial notice, when Core reports a version older than the
    // frontend's minimum. Warn-only: the OK button dismisses it and the
    // chain advances to the media-DB first-run check. `core_version_checked`
    // gates the open so we don't evaluate before the async `version` fetch
    // has answered; `core_version_supported` defaults true so we never
    // flash the warning pre-check.
    function _maybeOpenCoreVersionWarning(): void {
        if (root._coreVersionWarningShown) {
            // Already handled this session — make sure the next gate still
            // runs so a re-entry from another trigger doesn't stall the chain.
            root._maybeOpenFirstRunIndex();
            return;
        }
        if (!Browse.Notice.commercial_ack)
            return;
        if (!Browse.AppStatus.core_version_checked)
            return;
        if (Browse.AppStatus.core_version_supported) {
            // Version is fine — skip straight to the media-DB gate.
            root._coreVersionWarningShown = true;
            root._maybeOpenFirstRunIndex();
            return;
        }
        root._coreVersionWarningShown = true;
        root._requestModal(root.modalCoreVersion);
        root.coreVersionModalVisible = true;
        if (ScreenManager.topModal !== root.modalCoreVersion)
            ScreenManager.pushModal(root.modalCoreVersion);
    }

    // A random pick that resolves nothing must say so: it is a one-shot
    // action with no other visible result, and staying silent is
    // indistinguishable from the button being broken.
    function openRandomFailedModal(): void {
        root._requestModal(root.modalRandomFailed);
        root.randomFailedModalVisible = true;
        if (ScreenManager.topModal !== root.modalRandomFailed)
            ScreenManager.pushModal(root.modalRandomFailed);
    }

    function closeRandomFailedModal(): void {
        root.randomFailedModalVisible = false;
        // Either list can raise it; clearing both is harmless and keeps the
        // notice from re-opening on the next property read.
        Browse.GamesModel.clear_random_error();
        Browse.FavoritesModel.clear_random_error();
        if (ScreenManager.topModal === root.modalRandomFailed)
            ScreenManager.popModal();
    }

    Connections {
        target: Browse.GamesModel
        function onRandom_errorChanged(): void {
            if ((Browse.GamesModel.random_error ?? "") !== "")
                root.openRandomFailedModal();
        }
    }

    Connections {
        target: Browse.FavoritesModel
        function onRandom_errorChanged(): void {
            if ((Browse.FavoritesModel.random_error ?? "") !== "")
                root.openRandomFailedModal();
        }
    }

    function closeCoreVersionModal(): void {
        root.coreVersionModalVisible = false;
        if (ScreenManager.topModal === root.modalCoreVersion)
            ScreenManager.popModal();
        // Advance to the media-DB first-run check.
        root._maybeOpenFirstRunIndex();
    }

    // Log-upload modal lifecycle. Triggered from the Settings "Upload
    // log" action; the modal kicks off `Browse.LogUpload.upload()` on
    // its own when `open` flips true. The modal owns its three-phase
    // view; the router only owns push/pop and stack bookkeeping.
    function openLogUploadModal(): void {
        // Reset before showing so a previous success/error from earlier
        // in the session doesn't paint stale state behind the new
        // upload's "Uploading…" copy.
        Browse.LogUpload.reset();
        root._requestModal(root.modalLogUpload);
        root.logUploadModalVisible = true;
        if (ScreenManager.topModal !== root.modalLogUpload)
            ScreenManager.pushModal(root.modalLogUpload);
    }

    function closeLogUploadModal(): void {
        root.logUploadModalVisible = false;
        if (ScreenManager.topModal === root.modalLogUpload)
            ScreenManager.popModal();
    }

    onCloseLogUploadRequested: root.closeLogUploadModal()

    // Quit-confirm lifecycle. Hub's cancel signal lands on
    // `openQuitConfirmModal` instead of `Qt.quit()` so a stray B / Esc
    // can't kill the frontend; the modal owns the actual decision.
    function openQuitConfirmModal(): void {
        root._requestModal(root.modalQuitConfirm);
        root.quitConfirmModalVisible = true;
        if (ScreenManager.topModal !== root.modalQuitConfirm)
            ScreenManager.pushModal(root.modalQuitConfirm);
    }

    function closeQuitConfirmModal(): void {
        root.quitConfirmModalVisible = false;
        if (ScreenManager.topModal === root.modalQuitConfirm)
            ScreenManager.popModal();
    }

    onCloseQuitConfirmRequested: root.closeQuitConfirmModal()
    onQuitConfirmAccepted: Qt.quit()

    onAcceptRestart: root.confirmPendingRestart()
    onCancelRestart: root.cancelPendingRestart()

    // List-picker lifecycle. Settings screens emit requestListPicker
    // with a fieldId that round-trips through the modal so the accept
    // handler can dispatch the chosen id back to the matching
    // Browse.Settings.set_X without re-parsing the title.
    function openListPickerModal(title: string, entries: var, initialId: string, fieldId: string): void {
        root.listPickerTitle = title;
        root.listPickerEntries = entries;
        root.listPickerInitialId = initialId;
        root.listPickerFieldId = fieldId;
        root._requestModal(root.modalListPicker);
        root.listPickerModalVisible = true;
        if (ScreenManager.topModal !== root.modalListPicker)
            ScreenManager.pushModal(root.modalListPicker);
    }

    function closeListPickerModal(): void {
        root.listPickerModalVisible = false;
        root.listPickerTitle = "";
        root.listPickerEntries = [];
        root.listPickerInitialId = "";
        root.listPickerFieldId = "";
        if (ScreenManager.topModal === root.modalListPicker)
            ScreenManager.popModal();
    }

    // Open the page/list-scoped operations menu (West button), the "View"
    // counterpart to North's item-scoped "Options". Go to... stays
    // pre-focused so the common path is a fixed West-then-Accept chord;
    // future list ops (sort/filter/layout) append here. The facet fetch is
    // kicked off here so the buckets are likely ready by the time the user
    // advances into the grid.
    function openPageMenu(): void {
        Browse.GamesModel.load_letter_index();
        const entries = [
            {
                "id": "jump_letter",
                "label": qsTr("Go to...")
            }
        ];
        // The pick is uniform over the games listed in THIS folder, so the
        // label says so rather than implying the whole system. Omitted when
        // the folder holds no games (only subfolders), because there would be
        // nothing to pick.
        if (Browse.GamesModel.total_files > 0)
            entries.push({
                "id": "launch_random",
                "label": qsTr("Random game in this folder")
            });
        root.openListPickerModal(qsTr("View"), entries, "jump_letter", "page_menu");
    }

    // Favorites' West-button menu: how the list is ordered and scoped, plus
    // the random pick. Sort and filter both need every favorite loaded (Core
    // returns them in database order, 25 at a time, with no sort parameter),
    // so opening the menu warms that load rather than making the user wait
    // after choosing.
    function openFavoritesPageMenu(): void {
        Browse.FavoritesModel.ensure_full_load();
        const entries = [
            {
                "id": "favorites_sort",
                "label": qsTr("Sort: %1").arg(root._favoritesSortLabel())
            },
            {
                "id": "favorites_filter",
                "label": qsTr("Show: %1").arg(root._favoritesFilterLabel())
            },
            {
                "id": "launch_random_favorite",
                "label": qsTr("Random favorite")
            }
        ];
        root.openListPickerModal(qsTr("View"), entries, "favorites_sort", "page_menu_favorites");
    }

    function _favoritesSortLabel(): string {
        return Browse.FavoritesModel.sort_mode === "name" ? qsTr("A-Z") : qsTr("Default");
    }

    // Human-readable form of the active scope, resolved against the facet so
    // the label matches the picker row that set it.
    function _favoritesFilterLabel(): string {
        const active = Browse.FavoritesModel.filter ?? "";
        if (active === "")
            return qsTr("All");
        if (active.startsWith("cat:"))
            return active.slice(4);
        const id = active.startsWith("sys:") ? active.slice(4) : active;
        const systems = root._favoritesSystemFacet();
        for (let i = 0; i < systems.length; i++) {
            if (systems[i].id === id)
                return systems[i].name;
        }
        return id;
    }

    function _favoritesSystemFacet(): var {
        let parsed = [];
        try {
            parsed = JSON.parse(Browse.FavoritesModel.system_facet_json || "[]");
        } catch (e) {
            parsed = [];
        }
        return Array.isArray(parsed) ? parsed : [];
    }

    function openFavoritesSortMenu(): void {
        // Same trap as the filter picker: ListPickerModal treats an empty id
        // as "nothing pending" and never emits an accept for it, so the
        // default row needs a real id mapped back on accept. Without this,
        // choosing A-Z once left no way back to Core's order.
        const entries = [
            {
                "id": root._favoritesSortDefault,
                "label": qsTr("Default")
            },
            {
                "id": "name",
                "label": qsTr("A-Z")
            }
        ];
        const active = Browse.FavoritesModel.sort_mode === "" ? root._favoritesSortDefault : Browse.FavoritesModel.sort_mode;
        root.openListPickerModal(qsTr("Sort"), entries, active, "favorites_sort_pick");
    }

    // One flat list: everything, then the categories present, then the
    // individual systems. Built from the loaded favorites, so every row here
    // is guaranteed to match at least one entry.
    // Rebuild the open scope picker when the facet grows. The full load can
    // still be in flight when the user opens "Show", and without this the
    // picker would silently omit systems that arrive on later pages.
    Connections {
        target: Browse.FavoritesModel
        function onSystem_facet_jsonChanged(): void {
            if (root.listPickerModalVisible && root.listPickerFieldId === "favorites_filter_pick")
                root.openFavoritesFilterMenu();
        }
    }

    function openFavoritesFilterMenu(): void {
        const systems = root._favoritesSystemFacet();
        // NOT the empty string: ListPickerModal uses "" as its own
        // "nothing pending" sentinel and refuses to emit an accept for it,
        // so an empty id would make this row silently do nothing.
        const entries = [
            {
                "id": root._favoritesFilterAll,
                "label": qsTr("All (%1)").arg(Browse.FavoritesModel.total_count)
            }
        ];
        const categories = [];
        for (let i = 0; i < systems.length; i++) {
            const category = systems[i].category ?? "";
            if (category === "")
                continue;
            const existing = categories.find(c => c.name === category);
            if (existing) {
                existing.count += systems[i].count;
                existing.systems += 1;
            } else
                categories.push({
                    "name": category,
                    "count": systems[i].count,
                    "systems": 1
                });
        }
        for (let c = 0; c < categories.length; c++) {
            // A category holding a single system says exactly what that
            // system's own row says — and when the system is named after its
            // category (Core's Arcade system is literally "Arcade") the two
            // rows are indistinguishable. Offer the narrower row only.
            if (categories[c].systems < 2)
                continue;
            entries.push({
                "id": "cat:" + categories[c].name,
                // Category names come from Core untranslated; only the
                // surrounding count frame is localized.
                "label": qsTr("%1 (%2)").arg(categories[c].name).arg(categories[c].count)
            });
        }
        for (let s = 0; s < systems.length; s++) {
            entries.push({
                "id": "sys:" + systems[s].id,
                "label": qsTr("%1 (%2)").arg(systems[s].name).arg(systems[s].count)
            });
        }
        const active = Browse.FavoritesModel.filter === "" ? root._favoritesFilterAll : Browse.FavoritesModel.filter;
        root.openListPickerModal(qsTr("Show"), entries, active, "favorites_filter_pick");
    }

    // Re-parse the model's facet JSON into the live grid entries. Bound through
    // a Connections below so the grid populates the instant the fetch lands.
    function _refreshLetterJumpEntries(): void {
        const scheme = Browse.GamesModel.letter_index_scheme;
        let parsed = [];
        try {
            parsed = JSON.parse(Browse.GamesModel.letter_index_json);
        } catch (e) {
            parsed = [];
        }
        root.letterJumpEntries = Array.isArray(parsed) ? parsed : [];
        // Empty scheme = facet still resolving; anything else is final.
        root.letterJumpLoading = scheme === "";
    }

    function openLetterJumpModal(): void {
        root._refreshLetterJumpEntries();
        root._requestModal(root.modalLetterJump);
        root.letterJumpModalVisible = true;
        if (ScreenManager.topModal !== root.modalLetterJump)
            ScreenManager.pushModal(root.modalLetterJump);
    }

    function closeLetterJumpModal(): void {
        root.letterJumpModalVisible = false;
        root.letterJumpEntries = [];
        root.letterJumpLoading = false;
        if (ScreenManager.topModal === root.modalLetterJump)
            ScreenManager.popModal();
    }

    function openSettingNeedsRestartModal(): void {
        root._requestModal(root.modalSettingNeedsRestart);
        root.settingNeedsRestartModalVisible = true;
        if (ScreenManager.topModal !== root.modalSettingNeedsRestart)
            ScreenManager.pushModal(root.modalSettingNeedsRestart);
    }

    function closeSettingNeedsRestartModal(): void {
        root.settingNeedsRestartModalVisible = false;
        if (ScreenManager.topModal === root.modalSettingNeedsRestart)
            ScreenManager.popModal();
    }

    function stageSettingRestart(fieldId: string, selectedId: string): void {
        if (fieldId === "language")
            root._pendingLanguageSelection = selectedId;
        else if (fieldId === "resolution")
            root._pendingResolutionSelection = selectedId;
        else if (fieldId === "crtVideoStandard")
            root._pendingCrtStandardSelection = selectedId;
        root.openSettingNeedsRestartModal();
    }

    function stageCrtToggle(enable: bool): void {
        root._pendingCrtToggle = enable ? "on" : "off";
        root.openSettingNeedsRestartModal();
    }

    function cancelPendingRestart(): void {
        root._pendingLanguageSelection = "";
        root._pendingResolutionSelection = "";
        root._pendingCrtStandardSelection = "";
        root._pendingCrtToggle = "";
        root.closeSettingNeedsRestartModal();
    }

    function confirmPendingRestart(): void {
        // CRT-mode toggle takes a different exit: Main_MiSTer owns the
        // respawn (the new mode needs a different fb setup and --crt
        // flag), so persist the flag byte and exit with the reserved
        // reload code instead of the in-process execvp restart. If the
        // flag write fails, stay up rather than respawn into the old
        // mode and let the user retry.
        if (root._pendingCrtToggle !== "") {
            const enable = root._pendingCrtToggle === "on";
            root._pendingCrtToggle = "";
            root.closeSettingNeedsRestartModal();
            if (Browse.CrtVideo.write_crt_enable_file(enable))
                Qt.exit(42);
            return;
        }
        const language = root._pendingLanguageSelection;
        const resolution = root._pendingResolutionSelection;
        const crtStandard = root._pendingCrtStandardSelection;
        root._pendingLanguageSelection = "";
        root._pendingResolutionSelection = "";
        root._pendingCrtStandardSelection = "";
        root.closeSettingNeedsRestartModal();
        if (language !== "")
            Browse.Settings.set_language(language);
        if (resolution !== "")
            Browse.Settings.set_resolution(resolution);
        if (crtStandard !== "") {
            Browse.CrtVideo.set_video_standard(crtStandard);
            // A standard change must respawn through Main_MiSTer (exit
            // 42), not the in-process execvp restart: Main owns the fb
            // geometry (programs it pre-spawn and re-asserts ~1 s in,
            // reading the mode byte from the CRT state file), so it
            // has to re-read the new mode before the next frontend
            // boots. The desktop preview has no Main; the execvp
            // restart below re-reads frontend.toml and resizes the
            // preview canvas.
            if (Browse.Settings.is_mister) {
                if (Browse.CrtVideo.write_crt_enable_file(true))
                    Qt.exit(42);
                return;
            }
        }
        root.restartApp();
    }

    function restartApp() {
        Qt.exit(1000);
    }

    // CRT calibration lifecycle. Full-bleed test pattern mounted
    // outside the safe-area inset (see MainLayout); arrows nudge the
    // centering trims live through Browse.CrtVideo.
    function openCrtCalibrationModal(): void {
        root._requestModal(root.modalCrtCalibration);
        root.crtCalibrationModalVisible = true;
        if (ScreenManager.topModal !== root.modalCrtCalibration)
            ScreenManager.pushModal(root.modalCrtCalibration);
    }

    function closeCrtCalibrationModal(): void {
        root.crtCalibrationModalVisible = false;
        if (ScreenManager.topModal === root.modalCrtCalibration)
            ScreenManager.popModal();
    }

    onCloseCrtCalibrationRequested: root.closeCrtCalibrationModal()

    function beginSystemLauncherUpdate(systemId: string, selectedId: string): void {
        root._pendingLauncherSystemId = systemId;
        root._pendingLauncherSelectionId = selectedId;
        root.listPickerTitle = qsTr("Saving launcher");
        root.listPickerEntries = [
            {
                id: "saving",
                label: qsTr("Saving…")
            }
        ];
        root.listPickerInitialId = "saving";
        root.listPickerFieldId = "system_launcher_pending";
        Browse.SystemLaunchers.set_system_launcher(systemId, selectedId);
    }

    function clearPendingLauncherUpdate(): void {
        root._pendingLauncherSystemId = "";
        root._pendingLauncherSelectionId = "";
    }

    function showSystemLauncherUpdateError(): void {
        root.listPickerTitle = qsTr("Launcher update failed");
        root.listPickerEntries = [
            {
                id: "error",
                label: qsTr("Error: %1").arg(Browse.SystemLaunchers.update_error)
            },
            {
                id: "retry",
                label: qsTr("Retry")
            },
            {
                id: "cancel",
                label: qsTr("Cancel")
            }
        ];
        root.listPickerInitialId = "retry";
        root.listPickerFieldId = "system_launcher_error";
    }

    function handleListPickerCloseRequested(): void {
        if (root.listPickerFieldId === "system_launcher_pending")
            return;
        if (root.listPickerFieldId === "system_launcher_error")
            root.clearPendingLauncherUpdate();
        root.closeListPickerModal();
    }

    onListPickerAccepted: (fieldId, selectedId) => {
        if (fieldId === "page_menu") {
            root.closeListPickerModal();
            if (selectedId === "jump_letter")
                root.openLetterJumpModal();
            else if (selectedId === "launch_random")
                Browse.GamesModel.launch_random();
            return;
        }
        if (fieldId === "page_menu_favorites") {
            root.closeListPickerModal();
            if (selectedId === "launch_random_favorite")
                Browse.FavoritesModel.launch_random();
            else if (selectedId === "favorites_sort")
                root.openFavoritesSortMenu();
            else if (selectedId === "favorites_filter")
                root.openFavoritesFilterMenu();
            return;
        }
        if (fieldId === "favorites_sort_pick") {
            root.closeListPickerModal();
            const mode = selectedId === root._favoritesSortDefault ? "" : selectedId;
            Browse.FavoritesModel.set_sort_mode(mode);
            Browse.FavoritesState.sort = mode;
            return;
        }
        if (fieldId === "favorites_filter_pick") {
            root.closeListPickerModal();
            const value = selectedId === root._favoritesFilterAll ? "" : selectedId;
            Browse.FavoritesModel.set_filter(value);
            Browse.FavoritesState.filter = value;
            return;
        }
        if (fieldId === "system_launcher_pending")
            return;
        if (fieldId === "system_launcher_error") {
            if (selectedId === "error")
                return;
            if (selectedId === "retry" && root._pendingLauncherSystemId !== "")
                root.beginSystemLauncherUpdate(root._pendingLauncherSystemId, root._pendingLauncherSelectionId);
            else {
                root.clearPendingLauncherUpdate();
                root.closeListPickerModal();
            }
            return;
        }
        if (fieldId.startsWith("system_launcher:")) {
            root.beginSystemLauncherUpdate(fieldId.slice("system_launcher:".length), selectedId);
            return;
        }
        if (fieldId === "language") {
            root.closeListPickerModal();
            if (selectedId !== Browse.Settings.current_language)
                root.stageSettingRestart(fieldId, selectedId);
            return;
        } else if (fieldId === "orientation") {
            Browse.Settings.set_orientation(selectedId);
        } else if (fieldId === "clockFormat")
            Browse.Settings.set_clock_format(selectedId);
        else if (fieldId === "region") {
            Browse.Settings.set_region(selectedId);
            Browse.SystemsModel.reproject();
            Browse.CategoriesModel.reproject();
        } else if (fieldId === "browseLayout")
            Browse.Settings.set_browse_layout(selectedId);
        else if (fieldId === "systemLogoStyle")
            Browse.Settings.set_system_logo_style(selectedId);
        else if (fieldId === "buttonLayout")
            Browse.Settings.set_button_layout(selectedId);
        else if (fieldId === "resolution") {
            root.closeListPickerModal();
            if (selectedId !== Browse.Settings.current_resolution)
                root.stageSettingRestart(fieldId, selectedId);
            return;
        } else if (fieldId === "screensaverTimeout")
            Browse.Settings.set_screensaver_timeout(selectedId);
        else if (fieldId === "mediaImageType")
            Browse.Settings.set_media_image_type(selectedId);
        else if (fieldId === "crtVideoStandard") {
            root.closeListPickerModal();
            if (selectedId !== Browse.CrtVideo.current_video_standard)
                root.stageSettingRestart(fieldId, selectedId);
            return;
        }
        root.closeListPickerModal();
    }
    onListPickerCloseRequested: root.handleListPickerCloseRequested()

    onLetterJumpAccepted: offset => {
        root.closeLetterJumpModal();
        if (root.gamesScreen !== null)
            root.gamesScreen.jumpToItem(offset);
    }
    onLetterJumpCloseRequested: root.closeLetterJumpModal()

    // Keep the open grid in sync with the facet as it lands. The model clears
    // its facet to the loading state on `load_letter_index`, then fills it; this
    // re-parses into the live grid entries each time either changes.
    Connections {
        target: root.letterJumpModalRequested ? Browse.GamesModel : null
        enabled: root.letterJumpModalRequested
        function onLetter_index_jsonChanged(): void {
            if (root.letterJumpModalVisible)
                root._refreshLetterJumpEntries();
        }
        function onLetter_index_schemeChanged(): void {
            if (root.letterJumpModalVisible)
                root._refreshLetterJumpEntries();
        }
    }

    Connections {
        target: Browse.SystemLaunchers
        function onUpdate_pendingChanged(): void {
            if (root._pendingLauncherSystemId === "" || Browse.SystemLaunchers.update_pending)
                return;
            if (Browse.SystemLaunchers.update_error === "") {
                root.clearPendingLauncherUpdate();
                root.closeListPickerModal();
            } else {
                root.showSystemLauncherUpdateError();
            }
        }
    }

    Connections {
        target: Browse.AppStatus
        function onConnection_stateChanged(): void {
            root._maybeOpenFirstRunIndex();
            root._maybeCompleteBoot();
            root._maybeStartStartupRestore();
            root._maybeCompletePendingResumeLaunch();
        }
        // The version fetch resolves asynchronously after connect; this is
        // the edge that lets the chain advance past the version-warning gate.
        function onCore_version_checkedChanged(): void {
            root._maybeOpenCoreVersionWarning();
        }
    }

    // One-shot dismiss for the cold-launch curtain. The first time the
    // catalog reports READY we flip `bootComplete` and never reset it
    // — a later disconnect surfaces only via the status pill so the
    // user keeps their cached catalog.
    function _maybeCompleteBoot(): void {
        if (root.bootComplete)
            return;
        if (Browse.AppStatus.connection_state === 2) {
            root.bootComplete = true;
            // Curtain just lifted — fire the notice gate now that the
            // hub is paintable. _maybeOpenCommercialNotice early-returns
            // until bootComplete is true, so this is the natural edge.
            root._maybeOpenCommercialNotice();
            // The screensaver gate also early-returns until bootComplete
            // — restart the idle countdown so the timer fires again on
            // the post-boot quiet period. No-op when the setting is
            // "off".
            root._resetIdle();
        }
    }

    Connections {
        target: Browse.CategoriesModel
        function onLoadedChanged(): void {
            root._maybeOpenFirstRunIndex();
            root._maybeStartStartupRestore();
            root._maybeContinueOptimisticTransitions();
        }
        function onCountChanged(): void {
            root._maybeOpenFirstRunIndex();
            root._maybeStartStartupRestore();
            root._maybeContinueOptimisticTransitions();
        }
    }

    onCloseFirstRunIndexRequested: root.closeFirstRunIndexModal()
    onCloseCommercialNoticeRequested: root.closeCommercialNoticeModal()
    onCloseCoreVersionRequested: root.closeCoreVersionModal()
    onCloseRandomFailedRequested: root.closeRandomFailedModal()

    function beginCardWrite(owner: string): void {
        if (owner === "systems")
            Browse.SystemsModel.cancel_card_write();
        else if (owner === "games")
            Browse.GamesModel.cancel_card_write();
        else if (owner === "favorites")
            Browse.FavoritesModel.cancel_card_write();
        root.cardWriteOwner = owner;
        root.cardWriteFailed = false;
        root._requestModal(root.modalCardWrite);
        root.cardWriteModalVisible = true;
        cardWriteFailureTimer.stop();
        if (ScreenManager.topModal !== root.modalCardWrite)
            ScreenManager.pushModal(root.modalCardWrite);
    }

    function handleCardWriteStatus(): void {
        if (!root.cardWriteModalVisible || root.cardWriteOwner === "")
            return;
        if (root.activeCardWritePending)
            return;
        if (root.activeCardWriteError !== "") {
            root.cardWriteFailed = true;
            cardWriteFailureTimer.restart();
        } else {
            root.hideCardWriteModal();
        }
    }

    function cancelCardWrite(): void {
        if (root.cardWriteOwner === "systems")
            Browse.SystemsModel.cancel_card_write();
        else if (root.cardWriteOwner === "games")
            Browse.GamesModel.cancel_card_write();
        else if (root.cardWriteOwner === "favorites")
            Browse.FavoritesModel.cancel_card_write();
        root.hideCardWriteModal();
    }

    function hideCardWriteModal(): void {
        cardWriteFailureTimer.stop();
        root.cardWriteModalVisible = false;
        root.cardWriteFailed = false;
        root.cardWriteOwner = "";
        if (ScreenManager.topModal === root.modalCardWrite)
            ScreenManager.popModal();
    }

    // Action router. Called from handleKey (which translates Qt key
    // codes via Browse.Input.action_for_key) and directly from tests.
    // Dispatches to the top modal if any, otherwise the active screen.
    function handleAction(action: string): void {
        root._startupTrace("input/qml handleAction", "action=" + action, "activeScreen=" + root.activeScreen, "pendingTransition=" + root.pendingTransition, "hasModal=" + ScreenManager.hasModal, "heldAction=" + root._heldAction);
        // Screensaver eats the first input cleanly: dismiss the
        // overlay and DO NOT forward the press anywhere. The next
        // press goes through the normal routing below.
        if (root._maybeDismissScreensaver())
            return;
        // Swallow input while the warm-resume curtain is up. The target
        // screen restores behind a hidden Hub (activeScreen stays Hub), so
        // any press here would drive the ghost Hub the user can't see.
        if (root._startupRestorePending && root.startupRestoreCurtainVisible && !ScreenManager.hasModal)
            return;
        root._resetIdle();
        // Input gate. While a forward transition is in flight, swallow
        // every press so a user mashing buttons during the loading
        // wait can't queue a second transition or kick a half-cancel
        // through cancel handlers — the in-flight model call has to
        // settle on its own. Modal handling below still has to run
        // first so an Accept/Esc on a card-write modal isn't
        // accidentally swallowed if a transition is pending behind
        // it (the modal owns input regardless).
        if ((root.pendingTransition !== "" || root.transitionCueVisible) && !ScreenManager.hasModal) {
            root._startupTrace("input/qml drop", "reason=pending-transition", "action=" + action, "pendingTransition=" + root.pendingTransition, "transitionCueVisible=" + root.transitionCueVisible);
            return;
        }
        if (ScreenManager.hasModal) {
            // Single-consumer dispatch. When a second modal lands
            // (action_error variant for game launch / settings reset
            // / etc.), generalise into a per-modal handler table
            // rather than chaining ifs.
            // Only "cancel" aborts an in-flight card write. Treating
            // "accept" the same way would let a fat-fingered OK during
            // pending kill the write the user actually wanted; on
            // success/error the modal auto-dismisses via
            // handleCardWriteStatus, so accept has nothing to do here.
            if (ScreenManager.topModal === root.modalCardWrite && action === "cancel") {
                root.cancelCardWrite();
            } else if (ScreenManager.topModal === root.modalQrCode && action === "cancel") {
                root.closeQrCodeModal();
            } else if (ScreenManager.topModal === root.modalGameInfo) {
                if (root.gameInfoModal !== null)
                    root.gameInfoModal.handleAction(action);
            } else if (ScreenManager.topModal === root.modalContextMenu) {
                if (root.contextMenu !== null)
                    root.contextMenu.handleAction(action);
            } else if (ScreenManager.topModal === root.modalFirstRunIndex) {
                if (root.firstRunIndexModal !== null)
                    root.firstRunIndexModal.handleAction(action);
            } else if (ScreenManager.topModal === root.modalCommercialNotice) {
                if (root.commercialNoticeModal !== null)
                    root.commercialNoticeModal.handleAction(action);
            } else if (ScreenManager.topModal === root.modalCoreVersion) {
                if (root.coreVersionModal !== null)
                    root.coreVersionModal.handleAction(action);
            } else if (ScreenManager.topModal === root.modalRandomFailed) {
                // Accept or Back both dismiss a one-button notice.
                if (action === "accept" || action === "cancel")
                    root.closeRandomFailedModal();
            } else if (ScreenManager.topModal === root.modalLogUpload) {
                if (root.logUploadModal !== null)
                    root.logUploadModal.handleAction(action);
            } else if (ScreenManager.topModal === root.modalQuitConfirm) {
                if (root.quitConfirmModal !== null)
                    root.quitConfirmModal.handleAction(action);
            } else if (ScreenManager.topModal === root.modalSettingNeedsRestart) {
                if (root.settingNeedsRestartModal !== null)
                    root.settingNeedsRestartModal.handleAction(action);
            } else if (ScreenManager.topModal === root.modalListPicker) {
                if (root.listPickerModal !== null)
                    root.listPickerModal.handleAction(action);
            } else if (ScreenManager.topModal === root.modalLetterJump) {
                if (root.letterJumpModal !== null)
                    root.letterJumpModal.handleAction(action);
            } else if (ScreenManager.topModal === root.modalCrtCalibration) {
                if (root.crtCalibrationModal !== null)
                    root.crtCalibrationModal.handleAction(action);
            }
            // While a modal owns input, swallow everything not handled
            // above rather than leak it to the root screen.
            return;
        }
        root._noteRapidNavigationAction(action, false);
        if (root.activeScreen === root.screenGames) {
            if (root.gamesScreen !== null)
                root.gamesScreen.handleAction(action);
        } else if (root.activeScreen === root.screenSystems) {
            if (root.systemsScreen !== null)
                root.systemsScreen.handleAction(action);
        } else if (root.activeScreen === root.screenFavorites) {
            if (root.favoritesScreen !== null)
                root.favoritesScreen.handleAction(action);
        } else if (root.activeScreen === root.screenRecents) {
            if (root.recentsScreen !== null)
                root.recentsScreen.handleAction(action);
        } else if (root.activeScreen === root.screenUpdate) {
            if (root.updateScreen !== null)
                root.updateScreen.handleAction(action);
        } else if (root.activeScreen === root.screenSettings) {
            if (root.settingsScreen !== null)
                root.settingsScreen.handleAction(action);
        } else if (root.activeScreen === root.screenAbout) {
            if (root.aboutScreen !== null)
                root.aboutScreen.handleAction(action);
        } else {
            root.hubScreen.handleAction(action);
        }
    }

    // Hold-to-repeat for navigation actions. Qt's OS-level auto-repeat is
    // dropped (see Keys.onPressed below) because it bursts unpredictably
    // on heavy UI loads and isn't tunable on MiSTer's framebuffer build.
    // Instead, on a real press of a repeatable action we start an
    // initial-delay timer; on its first fire we hand over to a steady
    // tick. Both fire `handleAction(heldAction)`, which means the existing
    // transition gate, modal routing, and screen dispatch all apply
    // unchanged — repeats land on whichever screen / modal is currently
    // active, just like fresh presses.
    readonly property int _repeatInitialMs: 350
    readonly property int _repeatTickMs: 90
    readonly property int _rapidNavigationQuietMs: 260
    // Window for collapsing a second delivery of the same key into one
    // press — hardware contact bounce or input-stack double send. Far
    // below _repeatInitialMs and the repeat tick so it never touches
    // hold-repeat, and below the floor for a deliberate human re-tap.
    readonly property int _duplicateInputWindowMs: 40
    property int _lastPressedKey: 0
    property string _heldAction: ""
    property int _heldKey: 0
    property bool rapidNavigationActive: false
    property bool rapidNavigationIndicatorActive: false
    property string rapidNavigationAction: ""
    property int _rapidNavigationTapCount: 0
    // Aliased so tst_navigation.qml can observe the repeat state machine
    // — child Timer ids are file-scoped and aren't reachable otherwise.
    property alias _repeatPending: repeatInitial.running
    property alias _repeatTicking: repeatTick.running

    function _stopRepeat(): void {
        if (root._heldAction !== "" || repeatInitial.running || repeatTick.running)
            root._startupTrace("input/qml repeat stop", "heldAction=" + root._heldAction, "heldKey=" + root._heldKey, "initial=" + repeatInitial.running, "ticking=" + repeatTick.running);
        repeatInitial.stop();
        repeatTick.stop();
        root._heldAction = "";
        root._heldKey = 0;
        // Hold-release commits whatever cell the user landed on. Games
        // screen debounces its `set_selected_at_top` writes (one atomic
        // disk write per move would batter MiSTer's SD card on a Down-
        // hold through 20+ pages); the flush here lands the final
        // selection so a kill during launch resumes on the right entry.
        // No-op when no persist is pending or when another screen is
        // active.
        if (root.gamesScreen !== null)
            root.gamesScreen.flushSelectedPersist();
    }

    Binding {
        target: root.gamesScreen
        property: "detailRapidScrollActive"
        value: root.activeScreen === root.screenGames && root.rapidNavigationActive
    }

    Binding {
        target: root.gamesScreen
        property: "detailRapidIndicatorActive"
        value: root.activeScreen === root.screenGames && root.rapidNavigationIndicatorActive
    }

    Binding {
        target: root.gamesScreen
        property: "detailRapidScrollAction"
        value: root.activeScreen === root.screenGames ? root.rapidNavigationAction : ""
    }

    Binding {
        target: root.favoritesScreen
        property: "detailRapidScrollActive"
        value: root.activeScreen === root.screenFavorites && root.rapidNavigationActive
    }

    Binding {
        target: root.favoritesScreen
        property: "detailRapidIndicatorActive"
        value: root.activeScreen === root.screenFavorites && root.rapidNavigationIndicatorActive
    }

    Binding {
        target: root.favoritesScreen
        property: "detailRapidScrollAction"
        value: root.activeScreen === root.screenFavorites ? root.rapidNavigationAction : ""
    }

    Binding {
        target: root.recentsScreen
        property: "detailRapidScrollActive"
        value: root.activeScreen === root.screenRecents && root.rapidNavigationActive
    }

    Binding {
        target: root.recentsScreen
        property: "detailRapidIndicatorActive"
        value: root.activeScreen === root.screenRecents && root.rapidNavigationIndicatorActive
    }

    Binding {
        target: root.recentsScreen
        property: "detailRapidScrollAction"
        value: root.activeScreen === root.screenRecents ? root.rapidNavigationAction : ""
    }

    function _isRapidNavigationAction(action: string): bool {
        return action === "up" || action === "down" || action === "page_prev" || action === "page_next";
    }

    function _noteRapidNavigationAction(action: string, forceActive: bool): void {
        if (!root._isRapidNavigationAction(action))
            return;
        const sameBurst = rapidNavigationQuiet.running && root.rapidNavigationAction === action;
        root._rapidNavigationTapCount = sameBurst ? root._rapidNavigationTapCount + 1 : 1;
        root.rapidNavigationAction = action;
        if (forceActive || rapidNavigationQuiet.running)
            root.rapidNavigationActive = true;
        if (forceActive || root._rapidNavigationTapCount >= 3)
            root.rapidNavigationIndicatorActive = true;
        rapidNavigationQuiet.restart();
    }

    function _resetRapidNavigation(): void {
        rapidNavigationQuiet.stop();
        root.rapidNavigationActive = false;
        root.rapidNavigationIndicatorActive = false;
        root.rapidNavigationAction = "";
        root._rapidNavigationTapCount = 0;
    }

    function _isRepeatableAction(action: string): bool {
        return action === "up" || action === "down" || action === "left" || action === "right" || action === "page_prev" || action === "page_next";
    }

    // Drop a second delivery of the same key while the guard window is
    // open — hardware contact bounce or input-stack double send. A
    // different key, or the same key after the window closes, is not a
    // duplicate. Pure so tst_navigation can assert it without a clock.
    function _isDuplicateInput(key: int, lastKey: int, withinWindow: bool): bool {
        return withinWindow && key === lastKey;
    }

    // State-machine half of handleKey: records the held key/action and
    // arms the initial-delay timer. Pulled out of handleKey so unit
    // tests can drive the repeat state machine without also routing
    // through handleAction → real screens. No-op for non-dpad actions.
    function _armRepeat(action: string, key: int): void {
        if (!root._isRepeatableAction(action))
            return;
        root._startupTrace("input/qml repeat arm", "action=" + action, "key=" + key, "previousAction=" + root._heldAction, "previousKey=" + root._heldKey);
        root._heldAction = action;
        root._heldKey = key;
        repeatTick.stop();
        repeatInitial.restart();
    }

    // Press handler. Single entry point for both Keys.onPressed and the
    // existing tst_navigation.qml harness (which can't drive Keys events
    // on offscreen windows reliably). Fires the action immediately, then
    // arms the dpad-repeat state machine.
    function handleKey(key: int): void {
        root._startupTrace("input/qml handleKey", "key=" + key, "activeScreen=" + root.activeScreen, "pendingTransition=" + root.pendingTransition, "hasModal=" + ScreenManager.hasModal, "heldAction=" + root._heldAction);
        // Screensaver swallows raw key events ahead of the action map,
        // so the dismissing key is never armed for repeat.
        if (root._maybeDismissScreensaver())
            return;
        const action = Browse.Input.action_for_key(key);
        root._startupTrace("input/qml key mapped", "key=" + key, "action=" + action);
        if (action === "")
            return;
        root.handleAction(action);
        root._armRepeat(action, key);
    }

    // Screen-burn protection. After `_idleScreensaverMs` of input
    // silence (key, gamepad, mouse motion or click) the frontend
    // captures the live scene with an 80%-black scrim baked in once
    // and bounces a copy of the brand mark across the window. Any
    // further input dismisses the overlay; the dismissing press is
    // eaten so the user does not accidentally navigate. The active
    // flag is in-memory only; the timeout itself is persisted
    // through `Browse.Settings.current_screensaver_timeout` (values
    // are seconds as strings, with "off" disabling the feature).
    readonly property int _idleScreensaverMs: {
        const v = Browse.Settings.current_screensaver_timeout;
        if (!v || v === "off")
            return 0;
        const n = parseInt(v, 10);
        return Number.isFinite(n) && n > 0 ? n * 1000 : 0;
    }

    on_IdleScreensaverMsChanged: {
        idleTimer.stop();
        if (root._idleScreensaverMs <= 0) {
            // Switching to "off" while the screensaver is up should
            // tear it down right away — leaving the user staring at a
            // bouncing logo after they explicitly disabled the feature
            // would be confusing.
            if (screensaverOverlay.armed)
                screensaverOverlay.deactivate();
            return;
        }
        idleTimer.start();
    }

    function _resetIdle(): void {
        if (root._idleScreensaverMs <= 0 || !root._allowsScreensaver(root.activeScreen)) {
            idleTimer.stop();
            return;
        }
        idleTimer.restart();
    }

    function _maybeDismissScreensaver(): bool {
        if (!screensaverOverlay.armed)
            return false;
        screensaverOverlay.deactivate();
        // A held key dismissed mid-repeat would otherwise keep ticking
        // against an empty target screen.
        root._stopRepeat();
        idleTimer.restart();
        return true;
    }

    function _activateScreensaver(): void {
        if (screensaverOverlay.armed)
            return;
        // Skip while the cold-launch curtain is up or a forward
        // transition is in flight: the BootOverlay and the transition
        // "Loading…" cue are not screen-burn targets, and a screensaver
        // arm during them would race the user-visible animation.
        // `_maybeCompleteBoot` and `_completeTransition` call
        // `_resetIdle()` so the countdown restarts cleanly the moment
        // the gate clears.
        if (!root.bootComplete || root.pendingTransition !== "" || root.transitionCueVisible || !root._allowsScreensaver(root.activeScreen))
            return;
        const lg = root.headerBar.logoItem;
        if (!lg)
            return;
        const pt = lg.mapToItem(root.scene, 0, 0);
        // PreserveAspectFit means the painted region is narrower than
        // the Image item; using painted{Width,Height} starts the copy
        // flush with the visible logo rather than the Image's full
        // bounding box.
        const w = lg.paintedWidth > 0 ? lg.paintedWidth : lg.width;
        const h = lg.paintedHeight > 0 ? lg.paintedHeight : lg.height;
        screensaverOverlay.activate("qrc:/qt/qml/Zaparoo/App/resources/images/logo.png", Qt.rect(pt.x, pt.y, w, h));
    }

    Timer {
        id: idleTimer
        interval: root._idleScreensaverMs > 0 ? root._idleScreensaverMs : 60000
        repeat: false
        running: root._idleScreensaverMs > 0 && root._allowsScreensaver(root.activeScreen)
        onTriggered: root._activateScreensaver()
    }

    Connections {
        target: screensaverOverlay
        function onUserDismissed(): void {
            root._maybeDismissScreensaver();
        }
    }

    // Mouse-motion idle reset. `Qt.NoButton` lets click and release
    // events fall through to the screensaver overlay's own MouseArea
    // (when armed) or to whatever clickable sits underneath in normal
    // operation. `hoverEnabled: true` is what gets us positionChanged
    // on bare cursor moves without a button being pressed. Disable
    // this root-level hover area when mouse support is off so the
    // scene-level blank-cursor blocker owns both cursor and clicks.
    MouseArea {
        anchors.fill: parent
        z: 9001
        visible: Browse.Settings.current_mouse_enabled
        enabled: visible
        hoverEnabled: true
        acceptedButtons: Qt.NoButton
        onPositionChanged: {
            if (root._maybeDismissScreensaver())
                return;
            root._resetIdle();
        }
    }

    // Release handler. Only the key that started the repeat cancels it;
    // a release of any other key in flight (a chord, an unrelated press
    // mid-hold) is ignored.
    function handleKeyRelease(key: int): void {
        root._startupTrace("input/qml handleKeyRelease", "key=" + key, "heldAction=" + root._heldAction, "heldKey=" + root._heldKey);
        if (root._heldAction !== "" && key === root._heldKey)
            root._stopRepeat();
    }

    function _handleRepeatAction(): void {
        root._noteRapidNavigationAction(root._heldAction, true);
        root.handleAction(root._heldAction);
    }

    Timer {
        id: cardWriteFailureTimer
        interval: 1500
        repeat: false
        onTriggered: root.hideCardWriteModal()
    }

    Timer {
        id: rapidNavigationQuiet
        interval: root._rapidNavigationQuietMs
        repeat: false
        onTriggered: {
            root.rapidNavigationActive = false;
            root.rapidNavigationIndicatorActive = false;
            root.rapidNavigationAction = "";
            root._rapidNavigationTapCount = 0;
        }
    }

    // Open while a duplicate-input guard window is active; see the
    // Keys.onPressed handler below.
    Timer {
        id: duplicateInputGuard
        interval: root._duplicateInputWindowMs
        repeat: false
    }

    Timer {
        id: repeatInitial
        interval: root._repeatInitialMs
        repeat: false
        onTriggered: {
            if (root._heldAction === "")
                return;
            root._handleRepeatAction();
            repeatTick.start();
        }
    }

    Timer {
        id: repeatTick
        interval: root._repeatTickMs
        repeat: true
        onTriggered: {
            if (root._heldAction === "") {
                repeatTick.stop();
                return;
            }
            root._handleRepeatAction();
        }
    }

    // Cancel a stuck repeat if the window loses focus mid-hold; without
    // this, a missed Keys.onReleased (alt-tab, modal grab, compositor
    // quirk) would leave the timer ticking forever. `root.active` is
    // ApplicationWindow's own active property.
    onActiveChanged: {
        if (!root.active)
            root._stopRepeat();
    }

    Item {
        focus: true
        // Drop auto-repeated key events. A held Escape — or a brief
        // stuck press while the main thread is blocked on a model
        // reset — would otherwise queue a burst of `cancel` actions
        // that walk back through games → systems → hub → quit on
        // a single press. Our own controlled repeat (above) takes
        // over for dpad directions only.
        //
        // Then drop a second delivery of the same key inside the
        // duplicate-input window: some controllers / input stacks
        // double-send a single press, which would otherwise act
        // twice. Returning before restart() keeps the window anchored
        // to the first accepted press so a bounce can't extend it.
        Keys.onPressed: event => {
            if (event.isAutoRepeat)
                return;
            if (root._isDuplicateInput(event.key, root._lastPressedKey, duplicateInputGuard.running))
                return;
            root._lastPressedKey = event.key;
            duplicateInputGuard.restart();
            root.handleKey(event.key);
        }
        Keys.onReleased: event => {
            if (event.isAutoRepeat)
                return;
            root.handleKeyRelease(event.key);
        }
    }

    // Transition cue. Item, not Rectangle — the source screen's existing
    // background and circuit-trace texture stay visible underneath; never
    // paint a full-screen fill. The delayed indicator suppresses flashes
    // for quick loads; once it appears, screen `transitioning` bindings hide
    // primary content so the centered "Loading…" reads alone in the cleared
    // band. Do not apply a minimum-visible tail here: when the work completes
    // near the delay threshold, the destination must not paint underneath a
    // stale loading label. Sized to the full window so anchors.centerIn parks
    // the row in the geometric center regardless of which screen is the source.
    Item {
        anchors.fill: parent
        visible: transitionCueActive || transitionCue.showing
        z: 100

        readonly property bool startupRestoreCueActive: root.bootComplete && root.startupRestoreCurtainVisible && root._startupRestoreScreen !== ""
        readonly property bool transitionCueActive: (root.pendingTransition !== "" && !root.startupRestoreCurtainVisible) || startupRestoreCueActive
        readonly property string cueScreen: root.pendingTransition !== "" ? root.pendingTransition : root._startupRestoreScreen

        DelayedLoadingIndicator {
            id: transitionCue
            active: parent.transitionCueActive
            delayMs: parent.startupRestoreCueActive ? 0 : root.loadingIndicatorDelayMs
            minimumVisibleMs: 0
            x: Sizing.center(parent.width, width)
            y: Sizing.center(parent.height, height)
            onShowingChanged: root.transitionCueVisible = showing
            Component.onCompleted: root.transitionCueVisible = showing
            text: {
                switch (parent.cueScreen) {
                case "systems":
                    return qsTr("Loading systems…");
                case "games":
                    return qsTr("Loading games…");
                case "resume":
                    return qsTr("Loading game…");
                case "favorites":
                    return qsTr("Loading favorites…");
                case "recents":
                    return qsTr("Loading recently played…");
                case "settings":
                    return qsTr("Loading settings…");
                default:
                    return qsTr("Loading…");
                }
            }
        }

        // Hidden Image pool driven by `_prefetchSystemCovers`. Each Image
        // renders the tinted SVG logo off-screen at the same sourceSize as
        // the visible Tile so they share one QPixmapCache entry. When every
        // Image signals Ready or Error, `_systemCoverPrefetchPending` reaches
        // zero and `_completePrefetchSystemCovers` fires the transition
        // callback. `_systemCoverPrefetchUrls` is reset to [] when the gate
        // resolves, which destroys the delegates immediately. No background
        // fill is added — this Item is already a transparent overlay.
        Repeater {
            model: root._systemCoverPrefetchUrls
            delegate: Image {
                required property url modelData
                source: modelData
                sourceSize.width: 256
                asynchronous: true
                visible: false
                width: 0
                height: 0
                onStatusChanged: {
                    if (status !== Image.Ready && status !== Image.Error)
                        return;
                    root._systemCoverPrefetchPending = Math.max(0, root._systemCoverPrefetchPending - 1);
                    if (root._systemCoverPrefetchPending <= 0)
                        root._completePrefetchSystemCovers();
                }
            }
        }
    }

    // System-cover prefetch gate. Holds the "Loading systems…" transition
    // overlay until the first visible page of tinted SVG logos has decoded
    // (or the cap timer fires), then calls cb(). This ensures the Systems
    // grid reveals fully painted instead of showing name-text placeholders
    // that pop into logos one-by-one. Fast/re-entry navigations complete
    // within the 300ms DelayedLoadingIndicator threshold so no cue appears.
    // The hidden prefetch Repeater lives in the transition-cue Item above;
    // it watches `_systemCoverPrefetchUrls` and reports back via
    // `_systemCoverPrefetchPending`.
    function _prefetchSystemCovers(cb): void {
        const pageSize = Sizing.visibleCovers * 4;
        const count = Math.min(Browse.SystemsModel.count, pageSize);
        if (count === 0) {
            cb();
            return;
        }
        const urls = [];
        for (let i = 0; i < count; ++i) {
            const key = Browse.SystemsModel.cover_key_at(i);
            // Warm both the unfocused and focused tint ramps up front so the
            // first d-pad move never triggers an async SVG re-render.
            const unfocusedUrl = Resources.coverUrl(key, Theme.logoPrimary, Theme.logoSecondary, Theme.logoShadow);
            urls.push(unfocusedUrl);
            const focusedUrl = Resources.coverUrl(key, Theme.logoFocusPrimary, Theme.logoFocusSecondary, Theme.logoFocusShadow);
            // custom-image/ keys ignore tint params (served as-is), so both
            // URLs are identical — skip the duplicate to avoid redundant fetches.
            if (focusedUrl !== unfocusedUrl) {
                urls.push(focusedUrl);
            }
        }
        root._systemCoverPrefetchCallback = cb;
        root._systemCoverPrefetchPending = urls.length;
        root._systemCoverPrefetchUrls = urls;
        systemCoverPrefetchTimer.restart();
    }

    function _completePrefetchSystemCovers(): void {
        systemCoverPrefetchTimer.stop();
        root._systemCoverPrefetchUrls = [];
        root._systemCoverPrefetchPending = 0;
        const cb = root._systemCoverPrefetchCallback;
        root._systemCoverPrefetchCallback = null;
        if (cb !== null)
            cb();
    }

    Timer {
        id: resumeLaunchTimer
        interval: 50
        repeat: false
        onTriggered: root._startResumeLaunch()
    }

    // Desktop safety-clear for the resume "Loading game…" cue. On MiSTer the
    // launch replaces this process before this fires, so it never triggers and
    // the cue covers the core swap. On desktop nothing replaces us, so clear
    // the cue (and ungate input) once the launch has had time to take.
    Timer {
        id: resumeLaunchCueTimer
        interval: 8000
        repeat: false
        onTriggered: {
            if (root.pendingTransition === "resume")
                root.pendingTransition = "";
        }
    }

    Timer {
        id: favoritesTransitionTimer
        interval: 50
        repeat: false
        onTriggered: root._startFavoritesTransitionLoad()
    }

    Timer {
        id: recentsTransitionTimer
        interval: 50
        repeat: false
        onTriggered: root._startRecentsTransitionLoad()
    }

    Timer {
        id: backTransitionTimer
        interval: root.loadingIndicatorDelayMs + 50
        repeat: false
        onTriggered: root._maybeCompleteBackTransition()
    }

    Timer {
        id: folderBackTransitionTimer
        interval: root.loadingIndicatorDelayMs + 50
        repeat: false
        onTriggered: root._completeFolderBackTransition()
    }

    // Safety cap for the system-cover prefetch gate. If some logos haven't
    // decoded by this deadline they paint in after the screen reveals,
    // identical to the Games cover-gate timeout behavior. Cap = 300ms
    // (loadingIndicatorDelayMs) — logos that land faster than the
    // DelayedLoadingIndicator threshold complete the transition silently;
    // logos that are slower get a brief "Loading systems…" cue then pop in.
    Timer {
        id: systemCoverPrefetchTimer
        interval: root.loadingIndicatorDelayMs
        repeat: false
        onTriggered: root._completePrefetchSystemCovers()
    }

    // Deferred set_category trigger. When the existing model has rows,
    // the caller stretches the interval to the delayed cue threshold plus
    // one frame so the transition indicator is visible before synchronous
    // delegate teardown can freeze the GUI thread. Qt.callLater / interval
    // 0 fire inside the same event loop iteration before the next render.
    Timer {
        id: deferredCategorySetTimer
        interval: 50
        repeat: false
        property string targetCategory: ""
        onTriggered: {
            const category = deferredCategorySetTimer.targetCategory;
            root._startupTrace("startup/qml deferred category trigger", "category=" + category);
            Browse.SystemsModel.set_category(category);
            // Cleared after set_category so the resulting loading=false
            // edge is the one our callback consumes. If Rust returns
            // early because the same category is already populated, no
            // edge will arrive; complete synchronously in that no-op case.
            root._deferredCategoryPending = false;
            root._completeDeferredCategoryIfReady(category);
        }
    }

    Timer {
        id: deferredSystemSetTimer
        interval: 1
        repeat: false
        property string targetSystemId: ""
        onTriggered: {
            const systemId = deferredSystemSetTimer.targetSystemId;
            root._startupTrace("startup/qml deferred system trigger", "systemId=" + systemId);
            Browse.GamesModel.set_system(systemId);
            root._deferredSystemPending = false;
            root._completeDeferredSystemIfReady(systemId);
        }
    }
}
