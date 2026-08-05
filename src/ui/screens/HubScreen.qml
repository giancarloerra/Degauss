// Zaparoo Frontend
// Copyright (c) 2026 Wizzo Pty Ltd and the Zaparoo Project contributors.
// SPDX-License-Identifier: LicenseRef-PolyForm-Noncommercial-1.0.0
pragma ComponentBehavior: Bound

import QtQuick
import Zaparoo.Theme
import Zaparoo.Ui
import Zaparoo.Browse as Browse

// cxx-qt 0.8 patches `isFinal: true` on singleton properties but the
// qmltypes schema has no `isFinal` slot for Method, so every qinvokable
// call on a Zaparoo.Browse singleton (set_category, index_for_category,
// etc.) still trips qmllint's "Member can be shadowed" check. Until
// the schema grows method-level finality, suppress the compiler
// category file-wide.
// qmllint disable compiler

// Hub screen — two centered rows the user navigates as one grid:
//
//   * Top row: dynamic categories from Browse.CategoriesModel (Arcade,
//     Computer, Console, Handheld).
//   * Bottom row: actions — optional Resume Game, Favorites,
//     Recently Played, optional Update and Settings.
//
// Both rows wrap left/right modulo their own count, and Up/Down flip
// between rows in a closed loop (Up from top wraps to bottom, Down
// from bottom wraps to top). Both rows share cell width and spacing
// and the bottom row is horizontally centered under the top, so the
// cross-row jump is "round to the visually-nearest cell". Measuring
// positions in cell-widths, every cell's center sits at i + 0.5 and
// the bottom row is offset left by (topCount − bottomCount) / 2 cells.
// So for either direction:
//
//   destIdx = round(sourceIdx − (sourceCount − destCount) / 2)
//
// clamped into the destination row. The formula is symmetric and
// generalizes for any (topCount, bottomCount); see `_mapCrossRow`.
//
// Pure input dispatcher: emits one of `requestAccept(payload)`,
// `requestFavoritesScreen`, `requestRecentsScreen`,
// `requestUpdateScreen`, `requestSettingsScreen`, or `requestQuit`.
//
// All cross-screen orchestration (model fills, deferred set_category,
// cover prefetch, transition overlay, screen flip) lives in Main.qml.
// `transitioning` is written by the router so both rows hide during
// the loading wait.
Item {
    id: hub

    Component.onCompleted: console.debug("startup/qml component HubScreen completed")

    // Prefer a user override cover key over the bundled default. Pure: takes
    // the override-lookup result (empty string when none) and the fallback,
    // so it is unit-testable without the Browse.ImageOverrides singleton.
    // Hub overrides live under the `hub/` customization subfolder, keyed by
    // category id (Arcade/Computer/Console/Handheld) or action id
    // (resume/favorites/recents/update/settings); see docs/customization.md.
    function _preferOverride(overrideKey: string, fallbackKey: string): string {
        return (overrideKey && overrideKey.length > 0) ? overrideKey : fallbackKey;
    }

    // Resolve the cover key for a Hub item: a user override from the `hub/`
    // namespace if present, else the bundled key. Before the deferred scan
    // completes, return empty so first paint does no icon image work and avoids
    // a bundled-to-custom flash.
    function _hubCoverKey(id: string, fallbackKey: string): string {
        if (!Browse.ImageOverrides.hub_loaded)
            return "";
        return hub._preferOverride(Browse.ImageOverrides.override_cover_key("hub", id), fallbackKey);
    }

    readonly property var _placeholderCategories: [
        {
            id: CategoryIds.arcadeId,
            name: qsTr("Arcade"),
            coverKey: hub._hubCoverKey(CategoryIds.arcadeId, CategoryIds.coverKey(CategoryIds.arcadeId))
        },
        {
            id: CategoryIds.computerId,
            name: qsTr("Computers"),
            coverKey: hub._hubCoverKey(CategoryIds.computerId, CategoryIds.coverKey(CategoryIds.computerId))
        },
        {
            id: CategoryIds.consoleId,
            name: qsTr("Consoles"),
            coverKey: hub._hubCoverKey(CategoryIds.consoleId, CategoryIds.coverKey(CategoryIds.consoleId))
        },
        {
            id: CategoryIds.handheldId,
            name: qsTr("Handhelds"),
            coverKey: hub._hubCoverKey(CategoryIds.handheldId, CategoryIds.coverKey(CategoryIds.handheldId))
        },
        {
            id: CategoryIds.otherId,
            name: qsTr("Other"),
            coverKey: hub._hubCoverKey(CategoryIds.otherId, CategoryIds.coverKey(CategoryIds.otherId))
        }
    ]
    readonly property var visibleCategoryEntries: {
        if (!Browse.CategoriesModel.loaded || Browse.CategoriesModel.raw_count <= 0) {
            const placeholders = [];
            for (let i = 0; i < hub._placeholderCategories.length; i++) {
                const entry = hub._placeholderCategories[i];
                const hidden = Browse.HubState.is_category_hidden(entry.id);
                if (hidden && !Browse.Settings.current_show_hidden)
                    continue;
                placeholders.push({
                    id: entry.id,
                    name: entry.name,
                    coverKey: entry.coverKey,
                    hidden: hidden
                });
            }
            return placeholders;
        }
        const entries = [];
        for (let i = 0; i < Browse.CategoriesModel.count; i++) {
            const name = Browse.CategoriesModel.category_at(i);
            entries.push({
                id: name,
                name: name,
                coverKey: hub._hubCoverKey(name, CategoryIds.coverKey(name)),
                hidden: Browse.CategoriesModel.is_hidden_at(i)
            });
        }
        return entries;
    }
    property bool transitioning: false
    // 0 = categories row, 1 = actions row.
    property int currentRow: 1
    // Index within the active row. Resume is first while optimistic/history is unknown.
    property int currentIndex: 0
    // False on the first-paint path so Hub can draw a static Resume tile
    // without touching RecentsModel. MainLayout flips this after the
    // first frame, then Resume can hide/update from Core history.
    property bool resumeModelEnabled: false
    // Incremented on each Accept so the focused tile plays its push-in
    // animation. Forwarded to every TileLoader; only the focused+selected
    // Tile fires its animation.
    property int activatePulse: 0
    // False until the user takes control of focus (first input). Combined
    // with `_restoreDone` into `_focusReady`, which gates whether the tiles
    // render focus at all.
    property bool _focusArmed: false
    // Set true once the load-time category restore has run. Combined with
    // `_focusArmed` into `_focusReady`, which gates whether the tiles render
    // focus at all — so the action row's default Resume selection never paints
    // a ring during the window before `restoreFromCategoriesReset` corrects
    // focus to the saved tile on a cold start.
    property bool _restoreDone: false
    readonly property bool _focusReady: hub._focusArmed || hub._restoreDone
    // Source-row index from the most recent cross. Used to make a
    // Down → Up (or Up → Down) round-trip return to the originating
    // tile, which the centered visual-nearest mapping in `_mapCrossRow`
    // can't deliver when `sourceCount !== destCount`. -1 means no
    // round-trip is armed: either no cross has happened yet, or the
    // user moved horizontally on the destination row since the last
    // cross — at which point the saved index represents stale state
    // and the next cross falls back to `_mapCrossRow`.
    property int _crossSavedIndex: -1

    signal requestAccept(category: string)
    signal requestQuit
    signal requestFavoritesScreen
    signal requestRecentsScreen
    signal requestUpdateScreen
    signal requestSettingsScreen
    // Emitted when the user opens the options menu on a category tile.
    // `anchorRect` is the tile's bounding rect mapped to hub coordinates,
    // used by the context menu to position itself.
    signal requestContextMenu(index: int, anchorRect: rect)

    // Vertically center the (categories row + actions row + activeLabel)
    // block in the band between the HeaderBar bottom (Sizing.headerBottom)
    // and the help bar top (hub.height - pctH(6)). `_blockHeight`
    // mirrors the anchor chain below: each row is
    // `cellHeight + 2*verticalPadding`, the gap between them collapses
    // the focus-bleed padding (see `actionsRow.anchors.topMargin`),
    // and the label sits pctH(3) below the actions row at pctH(7) tall.
    readonly property int _blockHeight: 2 * (categoriesRow.cellHeight + 2 * categoriesRow.verticalPadding) + (categoriesRow.spacing - categoriesRow.verticalPadding - actionsRow.verticalPadding) + Sizing.pctH(3) + Sizing.pctH(7)
    readonly property int _blockY: Math.round((Sizing.headerBottom + hub.height - Sizing.pctH(6) - hub._blockHeight) / 2)

    readonly property bool resumeKnownUnavailable: hub.resumeModelEnabled && !Browse.RecentsModel.resume_loading && !Browse.RecentsModel.resume_available && Browse.AppStatus.connection_state === 2
    readonly property bool resumeActionVisible: !hub.resumeKnownUnavailable
    readonly property string _emptyCatalogFallbackAction: Browse.BuildInfo.update_enabled ? "update" : "settings"

    // Action-row data. Resume is visible by default while Core history
    // is unknown; hide it only after Recents proves there is nothing
    // resumable. The tile always uses the play icon so startup never
    // waits on a game cover for the Hub's primary action.
    readonly property var actionEntries: {
        const entries = [];
        if (hub.resumeActionVisible) {
            const resumeName = hub.resumeModelEnabled ? Browse.RecentsModel.resume_name : "";
            entries.push({
                id: "resume",
                coverKey: hub._hubCoverKey("resume", "icons/PlayOutline"),
                enabled: true,
                text: resumeName.length > 0 ? resumeName : qsTr("Resume")
            });
        }
        entries.push({
            id: "favorites",
            coverKey: hub._hubCoverKey("favorites", "icons/HeartOutline"),
            enabled: true,
            text: qsTr("Favorites")
        });
        entries.push({
            id: "recents",
            coverKey: hub._hubCoverKey("recents", "icons/History"),
            enabled: true,
            text: qsTr("Recently Played")
        });
        if (Browse.BuildInfo.update_enabled) {
            entries.push({
                id: "update",
                coverKey: hub._hubCoverKey("update", "icons/RefreshCw"),
                enabled: true,
                text: qsTr("Update")
            });
        }
        entries.push({
            id: "settings",
            coverKey: hub._hubCoverKey("settings", "icons/Tools"),
            enabled: true,
            text: qsTr("Settings & Utilities")
        });
        return entries;
    }

    function _actionIndexForId(id: string): int {
        for (let i = 0; i < hub.actionEntries.length; i++)
            if (hub.actionEntries[i].id === id)
                return i;
        return 0;
    }

    function _remapActionFocus(): void {
        if (hub.currentRow !== 1)
            return;
        hub.currentIndex = hub._actionIndexForId(Browse.HubState.selected_action);
    }

    function focusResumeIfVisible(): void {
        const resumeIndex = hub._actionIndexForId("resume");
        if (!hub.resumeActionVisible || hub.actionEntries[resumeIndex].id !== "resume")
            return;
        hub.currentRow = 1;
        hub.currentIndex = resumeIndex;
        hub._crossSavedIndex = -1;
        hub._commitActionSelection();
    }

    function _focusFallbackAfterResumeRemoved(): void {
        if (Browse.CategoriesModel.count > 0) {
            hub.currentRow = 0;
            hub.currentIndex = 0;
            hub._crossSavedIndex = -1;
            hub._commitCategorySelection();
            return;
        }
        hub.currentRow = 1;
        hub.currentIndex = hub._actionIndexForId("settings");
        hub._crossSavedIndex = -1;
        hub._commitActionSelection();
    }

    onActionEntriesChanged: {
        // Only treat a vanishing Resume tile as a real removal once the user
        // is driving focus (_focusArmed). During the cold-boot settle the
        // resume fetch can briefly read unavailable before it resolves to the
        // just-played game; reacting then would jump focus to Arcade and
        // persist it, stranding the user off the (about-to-reappear) Resume
        // tile. While !_focusArmed, fall through to _remapActionFocus, which
        // keeps the actions row aligned to the saved "resume" intent without
        // overwriting persisted state.
        if (hub._focusArmed && hub.currentRow === 1 && Browse.HubState.selected_action === "resume" && !hub.resumeActionVisible) {
            hub._focusFallbackAfterResumeRemoved();
            return;
        }
        hub._remapActionFocus();
    }

    // Test-harness hook so `tst_navigation.qml` can reset both focus
    // axes between cases without poking individual properties through
    // MainLayout's alias.
    function resetFocus(): void {
        hub.currentRow = 1;
        hub.currentIndex = hub._actionIndexForId("resume");
        hub._crossSavedIndex = -1;
    }

    // Restore the hub from the persisted `Browse.HubState`. The router
    // decides whether this pass should cascade into
    // `SystemsModel.set_category`; first Hub paint restores focus only,
    // then post-frame restore/transition paths can pay for Systems.
    //
    // Called from two sites in Main.qml — the Component.onCompleted
    // early-arrival path (catalog already seeded synchronously) and the
    // CategoriesModel.onModelReset listener (later refreshes). On a
    // refresh the category list can reorder, so the row index MUST be
    // re-seeded even when SystemsModel is already on the chosen
    // category — otherwise the visible focus drifts off whichever
    // screen the user is on.
    function restoreFromCategoriesReset(cascadeSystems: bool): void {
        // Focus is now being finalized from persisted state; let the tiles
        // render focus from here on (snapped, since `_focusArmed` is still
        // false until the first user input).
        hub._restoreDone = true;
        const savedCategory = CategoryIds.canonicalize(Browse.HubState.category);
        const idx = savedCategory === "" ? -1 : Browse.CategoriesModel.index_for_category(savedCategory);
        const chosenCategoryIndex = idx >= 0 ? idx : 0;
        const chosenCategory = idx >= 0 ? savedCategory : Browse.CategoriesModel.category_at(chosenCategoryIndex);

        // Restore which row the user was on, then point currentIndex
        // at the right slot for that row. Saved row outside [0, 1] is
        // treated as 0 — same belt-and-braces stance as the category
        // fallback above. When the catalog reports 0 categories the
        // top row has no tiles to focus, so we drop focus onto
        // Update when it exists, otherwise Settings so the user lands
        // on an actionable tile.
        const savedRow = Browse.HubState.selected_row;
        const savedAction = Browse.HubState.selected_action;
        if (savedRow === 1 && savedAction !== "") {
            hub.currentRow = 1;
            hub.currentIndex = hub._actionIndexForId(savedAction);
        } else if (idx >= 0) {
            hub.currentRow = 0;
            hub.currentIndex = chosenCategoryIndex;
        } else if (hub.resumeActionVisible) {
            // Non-persisting seat: this is a programmatic restore, so it must
            // not commit like focusResumeIfVisible() would. Committing here
            // clobbers a saved category-row intent whenever the catalog is
            // momentarily empty (cold-boot restore, in-session catalog
            // refresh), stranding the user on Resume when they back out.
            hub.currentRow = 1;
            hub.currentIndex = hub._actionIndexForId("resume");
        } else if (Browse.CategoriesModel.count === 0) {
            hub.currentRow = 1;
            hub.currentIndex = hub._actionIndexForId(hub._emptyCatalogFallbackAction);
        } else {
            hub.currentRow = 0;
            hub.currentIndex = chosenCategoryIndex;
        }
        // A reseat from disk or from a category-list refresh makes any
        // armed round-trip context meaningless (the user might be on a
        // different row entirely now, and the saved source-index could
        // point past the new category list).
        hub._crossSavedIndex = -1;

        if (!cascadeSystems)
            return;
        // Cold boot before Core delivers the catalog: focus was seated above
        // (and `_restoreDone` set, so the focus ring paints immediately
        // instead of waiting for the connection), but the set_category
        // cascade needs the real catalog. Defer it — this function re-runs
        // on CategoriesModel.onModelReset once the catalog lands, and the
        // cascade fires then.
        if (Browse.CategoriesModel.count <= 0)
            return;
        if (Browse.SystemsModel.current_category === chosenCategory && Browse.SystemsModel.count > 0)
            return;
        Browse.SystemsModel.set_category(chosenCategory);
    }

    // Returns true if the focus actually moved. Empty rows leave disk
    // state alone — see tst_persistence.qml for the regression guarded
    // against. Both rows wrap modulo their count so a single Left/Right
    // press at either end whips around to the far side.
    function _navigate(delta: int): bool {
        const count = hub.currentRow === 0 ? hub.visibleCategoryEntries.length : hub.actionEntries.length;
        if (count <= 0)
            return false;
        const next = ((hub.currentIndex + delta) % count + count) % count;
        if (next === hub.currentIndex)
            return false;
        hub.currentIndex = next;
        // Horizontal motion on the destination row invalidates the
        // round-trip context — the user's intent is now to navigate
        // within this row, not bounce back to where they came from.
        hub._crossSavedIndex = -1;
        return true;
    }

    // Pure arithmetic — no model access. Maps an index in a row of
    // `sourceCount` cells to the visually-nearest index in a centered
    // row of `destCount` cells (both rows assumed to share cell width
    // and spacing). Returned index is clamped to [0, destCount-1]; a
    // degenerate `destCount <= 0` returns 0 — callers must guard
    // empty destination rows separately, this exists so the mapping
    // can be unit-tested without populating CategoriesModel.
    function _mapCrossRow(sourceIdx: int, sourceCount: int, destCount: int): int {
        if (destCount <= 0)
            return 0;
        const offset = (sourceCount - destCount) / 2;
        const target = Math.round(sourceIdx - offset);
        return Math.max(0, Math.min(destCount - 1, target));
    }

    // Cross-row jump. Up and Down both flip to the *other* row — the
    // two rows form a closed two-row loop, there is no "off the top"
    // or "off the bottom".
    //
    // The destination index has two sources:
    //
    //   1. If `_crossSavedIndex` is armed (>= 0) and within the
    //      destination row's bounds, restore it. This is the round-trip
    //      path: the user pressed Down, then Up without horizontal
    //      input in between, so the originating tile is the natural
    //      target. With unequal row counts the centered visual mapping
    //      can't return there on its own.
    //
    //   2. Otherwise fall back to `_mapCrossRow` (visually-nearest
    //      cell in the centered row). This fires on the very first
    //      cross of a session and after any horizontal input on the
    //      destination row clears the armed index.
    //
    // Returns false only when the destination row is empty (no
    // categories loaded yet, etc.).
    function _crossRow(): bool {
        const topCount = hub.visibleCategoryEntries.length;
        const bottomCount = hub.actionEntries.length;
        const sourceCount = hub.currentRow === 0 ? topCount : bottomCount;
        const destCount = hub.currentRow === 0 ? bottomCount : topCount;
        if (destCount <= 0)
            return false;

        const sourceIdx = hub.currentIndex;
        const restored = hub._crossSavedIndex >= 0 && hub._crossSavedIndex < destCount;
        const destIdx = restored ? hub._crossSavedIndex : hub._mapCrossRow(sourceIdx, sourceCount, destCount);

        // Save the source-row index BEFORE flipping so the next cross
        // can return here. Reading `currentIndex` after the flip would
        // capture the destination index instead.
        hub._crossSavedIndex = sourceIdx;
        hub.currentRow = 1 - hub.currentRow;
        hub.currentIndex = destIdx;
        return true;
    }

    // Side-effect of every focus move: persist HubState. We do NOT call
    // SystemsModel.set_category here — that one's reserved for Accept
    // (and the router orchestrates it). Calling it on every left/right
    // press fires two model resets per press, each destroying-and-
    // recreating SystemsScreen's bound delegates on the UI thread —
    // choppy on MiSTer even though SystemsScreen is `visible: false`.
    function _currentCategoryId(): string {
        if (Browse.CategoriesModel.count > 0 && hub.currentIndex < Browse.CategoriesModel.count)
            return Browse.CategoriesModel.category_at(hub.currentIndex);
        const entry = hub.visibleCategoryEntries[hub.currentIndex];
        return entry ? CategoryIds.canonicalize(entry.id) : "";
    }

    function _commitCategorySelection(): void {
        Browse.HubState.selected_row = 0;
        const category = hub._currentCategoryId();
        if (category !== "")
            Browse.HubState.category = category;
    }

    function _commitActionSelection(): void {
        Browse.HubState.selected_row = 1;
        Browse.HubState.selected_action = hub.actionEntries[hub.currentIndex].id;
    }

    function _commitCurrent(): void {
        if (hub.currentRow === 0)
            hub._commitCategorySelection();
        else
            hub._commitActionSelection();
    }

    function _focusCategory(index: int): void {
        if (index < 0 || index >= hub.visibleCategoryEntries.length)
            return;
        hub._focusArmed = true;
        hub.currentRow = 0;
        hub.currentIndex = index;
        // Mouse focus is a deliberate landing on a specific tile — any
        // armed cross-row round-trip is no longer what the user wants.
        hub._crossSavedIndex = -1;
        hub._commitCategorySelection();
    }

    function _focusAction(index: int): void {
        if (index < 0 || index >= hub.actionEntries.length)
            return;
        hub._focusArmed = true;
        hub.currentRow = 1;
        hub.currentIndex = index;
        hub._crossSavedIndex = -1;
        hub._commitActionSelection();
    }

    // Emit the navigation signal for the currently selected entry.
    // Separated from _activateCurrent so DeferredAction can call it
    // after the push-in cue has had time to play.
    function _emitActivate(): void {
        if (hub.currentRow === 0) {
            // During optimistic boot the visible category row is backed
            // by localized placeholder labels. Accept the stable category
            // id, not the display name, so persisted HubState and router
            // comparisons remain locale-independent.
            hub.requestAccept(hub._currentCategoryId());
            return;
        }

        const entry = hub.actionEntries[hub.currentIndex];
        if (!entry || entry.enabled === false)
            return;

        const id = entry.id;
        if (id === "resume")
            hub.requestAccept("resume");
        else if (id === "favorites")
            hub.requestFavoritesScreen();
        else if (id === "recents")
            hub.requestRecentsScreen();
        else if (id === "update")
            hub.requestUpdateScreen();
        else if (id === "settings")
            hub.requestSettingsScreen();
    }

    function _activateCurrent(): void {
        hub.activatePulse++;
        hub._commitCurrent();
        pressCommit.arm();
    }

    // Returns the bounding rect of the currently focused category cell,
    // mapped to hub coordinates. Used to anchor the context menu.
    function _currentCategoryCellRect(): rect {
        const item = itemRepeater.itemAt(hub.currentIndex);
        if (!item)
            return Qt.rect(0, 0, 0, 0);
        return item.mapToItem(hub, 0, 0, item.width, item.height);
    }

    function handleAction(action: string): void {
        hub._focusArmed = true;
        if (action === "left") {
            if (hub._navigate(-1))
                hub._commitCurrent();
        } else if (action === "right") {
            if (hub._navigate(1))
                hub._commitCurrent();
        } else if (action === "down" || action === "up") {
            if (hub._crossRow())
                hub._commitCurrent();
        } else if (action === "accept") {
            hub._activateCurrent();
        } else if (action === "cancel") {
            hub.requestQuit();
        } else if (action === "context_menu") {
            // Only open the context menu for real (non-placeholder) category
            // tiles — placeholders have no category to hide or scrape.
            if (hub.currentRow === 0 && hub.currentIndex < Browse.CategoriesModel.count)
                hub.requestContextMenu(hub.currentIndex, hub._currentCategoryCellRect());
        }
    }

    // ── Visual tree ───────────────────────────────────────────────────────────

    DeferredAction {
        id: pressCommit
        onDeferred: hub._emitActivate()
    }

    Item {
        id: categoriesRow

        // Cell layout. Tiles are icon-only (no label inside), so the
        // cell is a roughly-square image area. The category name for
        // the focused tile renders below the grid in `activeLabel`,
        // not inside the tile.
        readonly property int spacing: Sizing.pctW(3)
        readonly property int sideInset: Sizing.pctW(5)
        readonly property int maxCellWidth: Sizing.pctH(22)
        readonly property int n: hub.visibleCategoryEntries.length
        // n=0 falls back to maxCellWidth so the actions row (which
        // mirrors `categoriesRow.cellWidth`) still renders at proper
        // size when the catalog reports 0 systems. Without the
        // fallback the Settings tile collapses to width=0 and the
        // user has nothing to navigate to.
        readonly property int rawCellWidth: n > 0 ? Math.floor((width - 2 * sideInset - (n - 1) * spacing) / n) : maxCellWidth
        readonly property int cellWidth: Math.min(maxCellWidth, rawCellWidth)
        // Square cells (1:1) for the main menu. The focused tile's
        // 1.06× scale bleed is absorbed by `verticalPadding` on the
        // row Item, not by inflating the cell.
        readonly property int cellHeight: cellWidth
        readonly property int totalRowWidth: n > 0 ? n * cellWidth + (n - 1) * spacing : 0
        readonly property int rowOriginX: Sizing.center(width, totalRowWidth)

        // Symmetric padding contains the focused tile's 1.06× scale
        // bleed inside the row's own bounds.
        readonly property int verticalPadding: Sizing.pctH(2)

        anchors.horizontalCenter: parent.horizontalCenter
        width: parent.width
        height: cellHeight + 2 * verticalPadding
        // Vertically centered with actionsRow + activeLabel as one
        // block in the band between the logo and the help bar. See
        // `_blockHeight` / `_blockY` on the hub root for the math.
        y: hub._blockY

        // Hide the tiles while the router holds us here on a forward
        // transition so the centred "Loading…" cue (painted from
        // Main.qml) reads alone.
        visible: !hub.transitioning

        Component {
            id: tileDelegate
            Tile {
                focusRingColor: Theme.stateMarker
            }
        }

        Repeater {
            id: itemRepeater

            model: hub.visibleCategoryEntries

            Item {
                id: cellItem

                required property int index
                required property var modelData

                x: categoriesRow.rowOriginX + index * (categoriesRow.cellWidth + categoriesRow.spacing)
                y: categoriesRow.verticalPadding
                width: categoriesRow.cellWidth
                height: categoriesRow.cellHeight

                readonly property bool isSelected: hub.currentRow === 0 && index === hub.currentIndex
                // Focused tile draws on top so its 1.06× scale-up isn't
                // clipped by neighbours to the right.
                z: isSelected ? 1 : 0

                TileLoader {
                    anchors.fill: parent
                    sourceComponent: tileDelegate
                    isSelected: cellItem.isSelected
                    isFocused: hub.currentRow === 0
                    name: cellItem.modelData.name
                    coverKey: cellItem.modelData.coverKey
                    hidden: cellItem.modelData.hidden ?? false
                    activatePulse: hub.activatePulse
                    settling: !hub.visible
                    focusReady: hub._focusReady
                }

                MouseArea {
                    anchors.fill: parent
                    hoverEnabled: true
                    acceptedButtons: Qt.LeftButton | Qt.RightButton
                    cursorShape: Qt.PointingHandCursor

                    onEntered: hub._focusCategory(cellItem.index)
                    onClicked: mouse => {
                        hub._focusCategory(cellItem.index);
                        if (mouse.button === Qt.RightButton) {
                            if (cellItem.index < Browse.CategoriesModel.count)
                                hub.requestContextMenu(cellItem.index, cellItem.mapToItem(hub, 0, 0, cellItem.width, cellItem.height));
                        } else {
                            hub._activateCurrent();
                        }
                    }
                }
            }
        }
    }

    // Action row. Same cell geometry and centring formula as
    // categoriesRow so the two rows visually read as one grid; the
    // only difference is a small array model with optional Resume. Positioned
    // directly below categoriesRow with a vertical gap equal to
    // categoriesRow.spacing so the visual gutter between rows matches
    // the gutter between tiles within a row.
    Item {
        id: actionsRow

        // Mirror categoriesRow's cell metrics so both rows line up
        // pixel-for-pixel.
        readonly property int spacing: categoriesRow.spacing
        readonly property int cellWidth: categoriesRow.cellWidth
        readonly property int cellHeight: categoriesRow.cellHeight
        readonly property int verticalPadding: categoriesRow.verticalPadding
        readonly property int n: hub.actionEntries.length
        readonly property int totalRowWidth: n > 0 ? n * cellWidth + (n - 1) * spacing : 0
        readonly property int rowOriginX: Sizing.center(width, totalRowWidth)

        anchors.horizontalCenter: parent.horizontalCenter
        anchors.top: categoriesRow.bottom
        // Visual gap between the bottom edge of a category cell and the
        // top edge of an action cell must equal the horizontal `spacing`
        // between tiles within a row. Both rows reserve `verticalPadding`
        // above and below their cells (to contain the focused tile's
        // 1.06× scale bleed); without compensating here the visible gap
        // would be `spacing + 2 × verticalPadding`.
        anchors.topMargin: categoriesRow.spacing - categoriesRow.verticalPadding - actionsRow.verticalPadding
        width: parent.width
        height: cellHeight + 2 * verticalPadding
        visible: !hub.transitioning

        Component {
            id: actionTileDelegate
            Tile {
                focusRingColor: Theme.stateMarker
            }
        }

        Repeater {
            model: hub.actionEntries

            Item {
                id: actionCellItem

                required property int index
                required property var modelData

                x: actionsRow.rowOriginX + index * (actionsRow.cellWidth + actionsRow.spacing)
                y: actionsRow.verticalPadding
                width: actionsRow.cellWidth
                height: actionsRow.cellHeight

                readonly property bool isSelected: hub.currentRow === 1 && index === hub.currentIndex
                z: isSelected ? 1 : 0

                TileLoader {
                    anchors.fill: parent
                    sourceComponent: actionTileDelegate
                    isSelected: actionCellItem.isSelected
                    isFocused: hub.currentRow === 1
                    name: actionCellItem.modelData.text
                    coverKey: actionCellItem.modelData.coverKey
                    activatePulse: hub.activatePulse
                    settling: !hub.visible
                    focusReady: hub._focusReady
                }

                MouseArea {
                    anchors.fill: parent
                    hoverEnabled: true
                    acceptedButtons: actionCellItem.modelData.enabled === false ? Qt.NoButton : Qt.LeftButton
                    cursorShape: actionCellItem.modelData.enabled === false ? Qt.ArrowCursor : Qt.PointingHandCursor

                    onEntered: hub._focusAction(actionCellItem.index)
                    onClicked: {
                        hub._focusAction(actionCellItem.index);
                        hub._activateCurrent();
                    }
                }
            }
        }
    }

    // Active label — single big line under the bottom row, swaps text
    // on every move. Reads from whichever row owns focus. Hidden during
    // a forward transition, mirroring the rows.
    ActiveLabel {
        id: activeLabel

        anchors.top: actionsRow.bottom
        anchors.topMargin: Sizing.pctH(3)
        anchors.left: parent.left
        anchors.right: parent.right
        height: Sizing.pctH(7)
        text: {
            if (hub.currentRow === 1) {
                // currentIndex can briefly outrun actionEntries.length
                // during cold launch, before HubState is clamped to the
                // row. Guard the lookup so an undefined access doesn't
                // surface as a TypeError in the log.
                const entry = hub.actionEntries[hub.currentIndex];
                return entry ? entry.text : "";
            }
            const entry = hub.visibleCategoryEntries[hub.currentIndex];
            if (entry)
                return entry.name;
            return "";
        }
        visible: !hub.transitioning
    }

    // CategoriesModel has no `loading` qproperty — the catalog is
    // fetched eagerly via bind_to_endpoint!. The brief cold-launch
    // window where count===0 surfaces as "No categories" is acceptable
    // per the "Loading is brief" locked decision in MVP_PLAN.md.
    ScreenStateOverlay {
        x: categoriesRow.x + Sizing.center(categoriesRow.width, width)
        y: categoriesRow.y + Sizing.center(categoriesRow.height, height)
        width: categoriesRow.width
        height: categoriesRow.height
        enabled: Browse.CategoriesModel.loaded || (Browse.CategoriesModel.error_message ?? "") !== ""
        loading: false
        errorMessage: Browse.CategoriesModel.error_message ?? ""
        count: Browse.CategoriesModel.loaded ? Browse.CategoriesModel.count : 1
        emptyText: qsTr("No systems available. Run Update media database from Settings.")
    }
}
