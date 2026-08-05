// Zaparoo Frontend
// Copyright (c) 2026 Wizzo Pty Ltd and the Zaparoo Project contributors.
// SPDX-License-Identifier: LicenseRef-PolyForm-Noncommercial-1.0.0
// cxx-qt 0.8 singleton members aren't marked final, so every Browse.* read
// trips "can be shadowed". Structural; suppress compiler file-wide as the
// other screens do.
// qmllint disable compiler

import QtQuick
import Zaparoo.Browse as Browse

// Favorites screen — flat paged grid driven by
// `Browse.FavoritesModel`. Pure input dispatcher: emits
// `requestHubScreen()` on Escape and launches the highlighted entry on
// Accept by calling the model's `launch_at` (which fans out to Core's
// `run` endpoint).
//
// Favorites is a flat list — no folder navigation, no card-write flow —
// so it reuses the shared `MediaListScreen` shell with the
// favorites-specific model, persisted selection state, and copy. The View
// menu adds ordering and a system/category scope on top of that shell.
MediaListScreen {
    id: favorites

    property alias favoritesGrid: favorites.mediaGrid

    mediaModel: Browse.FavoritesModel
    mediaState: Browse.FavoritesState
    screenTitle: qsTr("Favorites")
    // An active scope can empty the list even when favorites exist, so say
    // which state the user is actually looking at.
    emptyText: Browse.FavoritesModel.filter !== "" ? qsTr("No favorites in this scope") : qsTr("No favorites yet")
    loadingText: qsTr("Loading favorites…")
    // The sort/filter full load runs under its own flag, not the model's
    // `loading`, so a cold entry with a persisted scope could flash the
    // empty text while the full set was still in flight. An empty list
    // with the full load running is a loading state, not an empty one.
    optimisticLoading: Browse.FavoritesModel.full_loading && Browse.FavoritesModel.count === 0
    detailShowTitle: false
    // The View menu owns the filter, so it has to stay reachable when that
    // filter matches nothing — otherwise the scope can never be cleared.
    pageMenuEnabledWhenEmpty: true
    // Narrowed lists read as "N of M" so the hidden remainder is visible.
    topStripTotalTextProvider: () => {
        const shown = Browse.FavoritesModel.count;
        const total = Browse.FavoritesModel.total_count;
        if (favorites._listLayout)
            return "";
        if (Browse.FavoritesModel.filter !== "" && total > shown)
            return qsTr("%1 of %2").arg(shown).arg(total);
        return shown > 0 ? qsTr("%1 entries").arg(shown) : "";
    }

    // Restore the persisted order/scope once, on first load. Applying them
    // triggers the full fetch the sort and filter both need.
    Component.onCompleted: {
        const sort = Browse.FavoritesState.sort ?? "";
        const filter = Browse.FavoritesState.filter ?? "";
        if (sort !== "")
            Browse.FavoritesModel.set_sort_mode(sort);
        if (filter !== "")
            Browse.FavoritesModel.set_filter(filter);
    }
}
