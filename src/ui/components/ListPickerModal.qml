// Zaparoo Frontend
// Copyright (c) 2026 Wizzo Pty Ltd and the Zaparoo Project contributors.
// SPDX-License-Identifier: LicenseRef-PolyForm-Noncommercial-1.0.0
pragma ComponentBehavior: Bound

import QtQuick
import Zaparoo.Theme

// `entries` is a `var` array of plain JS objects (`{ id, label }`). The
// AOT compiler can't infer the shape of `var`, so reads of
// `entries.length` and `modelData.label` fall back to the JS interpreter
// and trip the compiler category. Suppress file-wide.
// qmllint disable compiler

// Software-rendering safe centered list-picker modal. Wraps the shared
// `Modal` shell in `kind: "shell"` so it inherits the standard chrome
// (scrim, panel fill, corner radius, title) used by every other modal.
//
// Use this for "pick one of these" prompts that are not anchored to a
// tile. Anchored selectors should use `ContextMenu.qml` instead.
//
// Pure presentation. Routing - mounting and dispatching `handleAction` -
// belongs to whichever consumer plumbs the modal into `Main.qml`'s modal
// stack. The component renders, navigates `currentIndex` on up/down,
// emits `accepted(id)` on accept, and `closeRequested()` on cancel.
Item {
    id: modal

    property bool open: false
    property string title: ""
    // Each entry is `{ id: string, label: string }`. `id` is the dispatch
    // key emitted by `accepted`; `label` is the localized display text.
    // Position-keyed dispatch is a footgun - dynamic entry sets silently
    // re-shuffle the index/action map.
    property var entries: []
    // Optional. When `open` flips true, sets `currentIndex` to the entry
    // whose id matches. Empty string or no match falls back to 0.
    property string initialId: ""
    property int currentIndex: 0

    // Push-in scale for the activated row, mirroring the tile push-in.
    property real _pressScale: 1.0
    property string _pendingId: ""

    signal accepted(string id)
    signal closeRequested

    readonly property int _rowHeight: Sizing.pctH(7)
    readonly property int _rowSpacing: Sizing.pctH(1)
    // Cap the picker viewport at a portion of the screen height so it
    // never grows past what the modal shell can reasonably contain.
    // Visible row count falls out of this - `floor((max + spacing) /
    // (rowHeight + spacing))` gives the row count whose viewport fits
    // inside `_maxViewportHeight`, with at least 1 row.
    readonly property int _maxViewportHeight: Sizing.pctH(60)
    readonly property int _visibleRows: Math.max(1, Math.min(entries.length, Math.floor((_maxViewportHeight + _rowSpacing) / (_rowHeight + _rowSpacing))))
    readonly property int _viewportHeight: _visibleRows * _rowHeight + Math.max(0, _visibleRows - 1) * _rowSpacing
    readonly property int _contentHeight: Math.max(1, entries.length) * _rowHeight + Math.max(0, entries.length - 1) * _rowSpacing
    readonly property bool _scrollable: entries.length > _visibleRows
    readonly property int _scrollArrowSize: Sizing.pctH(3)
    readonly property int _scrollArrowGap: Sizing.pctH(0.5)
    readonly property int _scrollIndicatorBand: _scrollable ? _scrollArrowSize + _scrollArrowGap : 0
    readonly property int _viewportSlotHeight: _viewportHeight + 2 * _scrollIndicatorBand
    readonly property bool _hasContentAbove: viewport.contentY > 1
    readonly property bool _hasContentBelow: viewport.contentY + viewport.height < viewport.contentHeight - 1

    visible: modal.open
    anchors.fill: parent
    z: 300

    onOpenChanged: {
        if (!modal.open) {
            // Disarm a pending accept so a press-then-close inside the deferred
            // window cannot apply a selection after the modal is dismissed.
            acceptCommit.stop();
            return;
        }
        modal._applyInitialIndex();
        modal._pressScale = 1.0;
        pressAnim.stop();
        modal._pendingId = "";
    }

    // Callers swap `entries` in place on an already-open modal (the launcher
    // flow replaces a one-row "saving" list with a three-row error list).
    // Without this the stale currentIndex can sit past the new list's end, so
    // no row renders focused and Accept is a dead key until the user moves.
    onEntriesChanged: {
        if (modal.open)
            modal._applyInitialIndex();
    }

    function _applyInitialIndex(): void {
        let next = 0;
        if (modal.initialId !== "") {
            for (let i = 0; i < modal.entries.length; ++i) {
                if (modal.entries[i].id === modal.initialId) {
                    next = i;
                    break;
                }
            }
        }
        viewport.contentY = 0;
        modal.currentIndex = next;
        modal._scrollCurrentIntoView();
    }

    function _scrollCurrentIntoView(): void {
        const stride = modal._rowHeight + modal._rowSpacing;
        const top = modal.currentIndex * stride;
        const bottom = top + modal._rowHeight;
        if (top < viewport.contentY) {
            viewport.contentY = top;
        } else if (bottom > viewport.contentY + viewport.height) {
            viewport.contentY = bottom - viewport.height;
        }
    }

    function move(delta: int): void {
        if (modal.entries.length <= 0)
            return;
        const len = modal.entries.length;
        modal.currentIndex = ((modal.currentIndex + delta) % len + len) % len;
    }

    function handleAction(action: string): void {
        if (action === "up") {
            modal.move(-1);
        } else if (action === "down") {
            modal.move(1);
        } else if (action === "accept") {
            if (modal.currentIndex >= 0 && modal.currentIndex < modal.entries.length)
                modal._commitAccept(modal.entries[modal.currentIndex].id);
        } else if (action === "cancel") {
            modal.closeRequested();
        }
    }

    function _commitAccept(id: string): void {
        modal._pendingId = id;
        pressAnim.restart();
        acceptCommit.arm();
    }

    NumberAnimation {
        id: pressAnim
        target: modal
        property: "_pressScale"
        to: Motion.rowPressScale
        duration: Motion.dur(Motion.pressMs)
        easing.type: Easing.OutQuad
    }

    DeferredAction {
        id: acceptCommit
        onDeferred: {
            const id = modal._pendingId;
            modal._pendingId = "";
            if (id !== "")
                modal.accepted(id);
        }
    }

    Modal {
        id: shell

        open: modal.open
        kind: "shell"
        title: modal.title

        Item {
            id: viewportSlot

            width: parent.width
            height: modal._viewportSlotHeight

            Flickable {
                id: viewport

                anchors.fill: parent
                anchors.topMargin: modal._scrollIndicatorBand
                anchors.bottomMargin: modal._scrollIndicatorBand
                contentWidth: width
                contentHeight: modal._contentHeight
                clip: true
                // Key navigation drives contentY; we don't want kinetic
                // dragging fighting with the focus tracker.
                interactive: false
                boundsBehavior: Flickable.StopAtBounds

                Column {
                    id: rowColumn

                    width: viewport.width
                    spacing: modal._rowSpacing

                    Repeater {
                        model: modal.entries

                        Rectangle {
                            id: row

                            required property int index
                            required property var modelData

                            width: rowColumn.width
                            height: modal._rowHeight
                            color: Theme.surfaceCard
                            border.width: row.index === modal.currentIndex ? Sizing.stroke(2) : Sizing.stroke(1)
                            border.color: row.index === modal.currentIndex ? Theme.accent : Theme.borderMid
                            radius: Sizing.cornerRadius
                            transformOrigin: Item.Center
                            scale: row.index === modal.currentIndex ? modal._pressScale : 1.0

                            Text {
                                anchors.left: parent.left
                                anchors.right: parent.right
                                anchors.verticalCenter: parent.verticalCenter
                                anchors.leftMargin: Sizing.pctW(2)
                                anchors.rightMargin: Sizing.pctW(2)
                                text: row.modelData.label
                                color: Theme.textPrimary
                                font.family: Theme.fontUi
                                font.pixelSize: Sizing.fontSize(2.6)
                                horizontalAlignment: Text.AlignHCenter
                                elide: Text.ElideRight
                                renderType: Text.NativeRendering
                            }

                            MouseArea {
                                anchors.fill: parent
                                hoverEnabled: true
                                acceptedButtons: Qt.LeftButton
                                cursorShape: Qt.PointingHandCursor
                                onEntered: modal.currentIndex = row.index
                                onClicked: modal._commitAccept(row.modelData.id)
                            }
                        }
                    }
                }
            }

            Image {
                source: Resources.iconUrl("ScrollUp")
                width: modal._scrollArrowSize
                height: width
                anchors.bottom: viewport.top
                anchors.bottomMargin: modal._scrollArrowGap
                anchors.horizontalCenter: viewport.horizontalCenter
                fillMode: Image.PreserveAspectFit
                smooth: true
                visible: modal._hasContentAbove
            }

            Image {
                source: Resources.iconUrl("ScrollDown")
                width: modal._scrollArrowSize
                height: width
                anchors.top: viewport.bottom
                anchors.topMargin: modal._scrollArrowGap
                anchors.horizontalCenter: viewport.horizontalCenter
                fillMode: Image.PreserveAspectFit
                smooth: true
                visible: modal._hasContentBelow
            }
        }
    }

    // Keep the focused row in view. When the current index moves above
    // or below the visible band we slide contentY just enough to bring
    // it back into view, no animation - software renderer pays per-frame
    // for any motion under translucent content.
    Connections {
        target: modal
        function onCurrentIndexChanged(): void {
            modal._scrollCurrentIntoView();
        }
    }
}
