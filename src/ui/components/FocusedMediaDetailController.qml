// Zaparoo Frontend
// Copyright (c) 2026 Wizzo Pty Ltd and the Zaparoo Project contributors.
// SPDX-License-Identifier: LicenseRef-PolyForm-Noncommercial-1.0.0
// var-typed function properties (identityForIndex, loadForIndex, clearDetail)
// and var-typed mediaModel cannot be statically typed. Structural; suppress compiler.
// qmllint disable compiler

import QtQuick

Item {
    id: root

    property int itemCount: 0
    property int currentIndex: 0
    // Sized for a Core round trip; when metadata is a local database
    // read the wait serves no purpose beyond input coalescing, so the
    // host may drop it to a frame-scale value (see MediaListScreen).
    property int debounceMs: 220
    property var mediaModel: null
    property var identityForIndex: null
    property var loadForIndex: null
    // Optional immediate (non-debounced) hook fired the moment the focused row
    // changes, so the detail pane reflects THIS row at once (cached/local
    // metadata or a clean blank) instead of holding the previous row's values
    // through the load debounce. Defaults to the model's `peek_detail_at`.
    property var peekForIndex: null
    property var clearDetail: null
    property bool clearOnDisable: true
    property bool rapidScrollActive: false

    property string _requestedKey: ""
    property string _pendingKey: ""
    property int _pendingIndex: -1
    // Identity the immediate peek last ran for, so a repeated schedule for the
    // same row does not re-peek.
    property string _peekedKey: ""

    visible: false

    function requestNow(): void {
        root._schedule(true);
    }

    function clearTransient(): void {
        root._resetTransientState(true);
        if (!root.rapidScrollActive)
            root._schedule(false);
    }

    function _identityAt(index: int): string {
        if (!root.enabled || root.itemCount <= 0 || index < 0 || index >= root.itemCount)
            return "";
        if (typeof root.identityForIndex === "function") {
            const custom = root.identityForIndex(index);
            return custom ?? "";
        }
        if (root.mediaModel === null || typeof root.mediaModel.system_id_at !== "function" || typeof root.mediaModel.path_at !== "function")
            return "";
        const systemId = root.mediaModel.system_id_at(index);
        const path = root.mediaModel.path_at(index);
        return systemId !== "" && path !== "" ? systemId + "\n" + path : "";
    }

    function _clearDetail(): void {
        if (typeof root.clearDetail === "function")
            root.clearDetail();
        else if (root.mediaModel !== null && typeof root.mediaModel.clear_current_detail === "function")
            root.mediaModel.clear_current_detail();
    }

    function _resetTransientState(clearDetail: bool): void {
        detailLoadDebounce.stop();
        root._requestedKey = "";
        root._pendingKey = "";
        root._pendingIndex = -1;
        root._peekedKey = "";
        if (clearDetail)
            root._clearDetail();
    }

    function _peek(index: int): void {
        if (typeof root.peekForIndex === "function")
            root.peekForIndex(index);
        else if (root.mediaModel !== null && typeof root.mediaModel.peek_detail_at === "function")
            root.mediaModel.peek_detail_at(index);
    }

    function _schedule(force: bool): void {
        if (!root.enabled)
            return;
        if (root.rapidScrollActive) {
            root._resetTransientState(true);
            return;
        }
        const key = root._identityAt(root.currentIndex);
        if (key === "") {
            root._resetTransientState(true);
            return;
        }
        // Reflect the focused row immediately (cached/local metadata or a clean
        // blank), before the debounce, so the table never shows the previous
        // row's values during the load window.
        if (root._peekedKey !== key) {
            root._peekedKey = key;
            root._peek(root.currentIndex);
        }
        if (!force && key === root._requestedKey)
            return;
        root._pendingKey = key;
        root._pendingIndex = root.currentIndex;
        detailLoadDebounce.restart();
    }

    function _loadPending(): void {
        if (!root.enabled || root._pendingKey === "" || root._pendingIndex < 0)
            return;
        const key = root._identityAt(root._pendingIndex);
        if (key === "" || key !== root._pendingKey)
            return;
        root._requestedKey = key;
        if (typeof root.loadForIndex === "function")
            root.loadForIndex(root._pendingIndex);
        else if (root.mediaModel !== null && typeof root.mediaModel.load_detail_at === "function")
            root.mediaModel.load_detail_at(root._pendingIndex);
    }

    onEnabledChanged: {
        if (enabled) {
            root._schedule(false);
        } else {
            root._resetTransientState(root.clearOnDisable);
        }
    }
    onRapidScrollActiveChanged: {
        if (rapidScrollActive)
            root._resetTransientState(true);
        else
            root._schedule(false);
    }
    onCurrentIndexChanged: root._schedule(false)
    onItemCountChanged: root._schedule(false)
    Component.onCompleted: root._schedule(false)

    Timer {
        id: detailLoadDebounce
        interval: root.debounceMs
        repeat: false
        onTriggered: root._loadPending()
    }
}
