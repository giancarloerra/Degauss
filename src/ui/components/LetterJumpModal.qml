// Zaparoo Frontend
// Copyright (c) 2026 Wizzo Pty Ltd and the Zaparoo Project contributors.
// SPDX-License-Identifier: LicenseRef-PolyForm-Noncommercial-1.0.0
pragma ComponentBehavior: Bound

import QtQuick
import Zaparoo.Theme

// `entries` is a `var` array of plain JS objects
// (`{ key, label, count, cursor }`). The AOT compiler can't infer the shape of
// `var`, so reads of `entries.length`/`modelData.label` fall back to the JS
// interpreter and trip the compiler category. Suppress file-wide.
// qmllint disable compiler

// Software-rendering safe "jump to letter" grid picker. Wraps the shared
// `Modal` shell (`kind: "shell"`) so it inherits the standard chrome (scrim,
// panel fill, corner radius, title) used by every other modal.
//
// The buckets come from `media.browse.index` (only non-empty sections, in sort
// order), so the grid is data-driven: no dead letters to skip over. Each cell
// shows the bucket label and its item count; accepting one emits the bucket's
// opaque `media.browse` cursor for the router to seek to.
//
// Pure presentation. Mounting and dispatching `handleAction` belong to the
// consumer that plumbs the modal into `Main.qml`'s modal stack. The component
// renders, navigates `currentIndex` in 2D, emits `accepted(cursor)` on accept,
// and `closeRequested()` on cancel.
Item {
    id: modal

    property bool open: false
    property string title: qsTr("Go to...")
    // Each entry is `{ key, label, count, cursor, offset }`. `label` is the
    // display text; `count` is the bucket size (dim subscript); `offset` is the
    // bucket's authoritative position (from Core), emitted on accept so the
    // router can jump there. The `cursor` field is unused by this UX (the jump
    // is a position jump, not a forward-only cursor seek).
    property var entries: []
    property int currentIndex: 0
    // True while the facet is still being fetched (no scheme resolved yet). When
    // false and `entries` is empty, no rail applies for this scope.
    property bool loading: false

    // Push-in scale for the activated cell, mirroring the tile push-in.
    property real _pressScale: 1.0
    property int _pendingOffset: 0
    property bool _hasPendingAccept: false

    // Emits the index of the selected bucket's first item within the grouped
    // sequence (cumulative count of all earlier buckets). The router adds the
    // leading directory count and jumps the grid to that absolute position.
    signal accepted(int itemOffset)
    signal closeRequested

    // Wider than the default modal so the grid has room to breathe. The panel
    // width and content margins below mirror `Modal`'s own math so the column
    // count we navigate by matches the grid the user sees.
    readonly property int _panelMax: Sizing.pctW(92)
    readonly property int _panelWidth: Math.min(Sizing.pctW(78), _panelMax)
    readonly property int _contentWidth: Math.max(Sizing.pctW(10), _panelWidth - 2 * Sizing.pctW(4))
    readonly property int _gap: Sizing.pctW(1)
    // Cell-size bounds: a legibility floor and a cap so a few-bucket scope
    // doesn't blow the cells up to fill the panel.
    readonly property int _minCell: Sizing.pctW(7)
    readonly property int _maxCell: Sizing.pctW(13)
    // Vertical budget for the grid block. Bounds the row count so a full
    // alphabet (`#`, `0-9`, A-Z) can't grow the panel past the screen; the
    // remaining height holds the panel title and margins.
    readonly property int _availHeight: Sizing.pctH(52)
    readonly property int _count: entries.length
    readonly property int _columns: Math.max(1, modal._fitColumns(_count, _contentWidth, _availHeight, _gap))
    readonly property int _rows: Math.ceil(Math.max(1, _count) / _columns)
    readonly property int _cellRaw: Math.min((_contentWidth - (_columns - 1) * _gap) / _columns, (_availHeight - (_rows - 1) * _gap) / _rows)
    readonly property int _cell: Sizing.px(Math.max(_minCell, Math.min(_maxCell, _cellRaw)))
    readonly property int _gridHeight: _rows * _cell + Math.max(0, _rows - 1) * _gap

    visible: modal.open
    anchors.fill: parent
    z: 300

    // Clamp the cursor if the bucket list shrank under it (e.g. a facet
    // refetch landed while the picker was open) so navigation and accept keep
    // targeting a live cell instead of an empty index.
    onEntriesChanged: {
        const last = modal.entries.length - 1;
        if (modal.currentIndex > last)
            modal.currentIndex = last < 0 ? 0 : last;
    }

    onOpenChanged: {
        if (!modal.open) {
            // Disarm a pending accept so a press-then-close inside the deferred
            // window cannot jump after the modal is dismissed.
            acceptCommit.stop();
            return;
        }
        modal.currentIndex = 0;
        modal._pressScale = 1.0;
        pressAnim.stop();
        modal._pendingOffset = 0;
        modal._hasPendingAccept = false;
    }

    // The bucket's authoritative offset (its first item's position among the
    // scope's files), supplied by Core. 0 when absent.
    function _offsetForIndex(index: int): int {
        if (index < 0 || index >= modal.entries.length)
            return 0;
        return modal.entries[index].offset ?? 0;
    }

    // Pure layout helper. Picks the column count whose square cell - bounded
    // by both the available width per column and the available height per row -
    // is largest, so the grid uses the area well and always fits within
    // `availH`. Kept side-effect free so it is unit-testable in isolation.
    function _fitColumns(count: int, availW: int, availH: int, gap: int): int {
        if (count <= 0)
            return 1;
        let best = 1;
        let bestCell = 0;
        for (let c = 1; c <= count; ++c) {
            const rows = Math.ceil(count / c);
            const cw = (availW - (c - 1) * gap) / c;
            const ch = (availH - (rows - 1) * gap) / rows;
            const cell = Math.min(cw, ch);
            if (cell > bestCell) {
                bestCell = cell;
                best = c;
            }
        }
        return best;
    }

    // Pure 2D grid move. Clamps within bounds; `down` past the last full row
    // lands on the final cell so scanning down the alphabet always reaches the
    // tail bucket. Kept side-effect free so it is unit-testable in isolation.
    function nextIndex(action: string, index: int, count: int, columns: int): int {
        if (count <= 0)
            return 0;
        const cols = Math.max(1, columns);
        let next = index;
        if (action === "left")
            next = index - 1;
        else if (action === "right")
            next = index + 1;
        else if (action === "up")
            next = index - cols;
        else if (action === "down") {
            next = index + cols;
            if (next >= count) {
                const lastRowStart = (Math.ceil(count / cols) - 1) * cols;
                next = index < lastRowStart ? count - 1 : index;
            }
        }
        if (next < 0 || next >= count)
            return index;
        return next;
    }

    function handleAction(action: string): void {
        if (action === "up" || action === "down" || action === "left" || action === "right") {
            modal.currentIndex = modal.nextIndex(action, modal.currentIndex, modal.entries.length, modal._columns);
        } else if (action === "accept") {
            if (modal.currentIndex >= 0 && modal.currentIndex < modal.entries.length) {
                modal._commitAccept(modal._offsetForIndex(modal.currentIndex));
            }
        } else if (action === "cancel" || action === "page_menu") {
            modal.closeRequested();
        }
    }

    function _commitAccept(itemOffset: int): void {
        modal._pendingOffset = itemOffset;
        modal._hasPendingAccept = true;
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
            const offset = modal._pendingOffset;
            const had = modal._hasPendingAccept;
            modal._pendingOffset = 0;
            modal._hasPendingAccept = false;
            if (had)
                modal.accepted(offset);
        }
    }

    Modal {
        id: shell

        open: modal.open
        kind: "shell"
        title: modal.title
        panelMaxWidth: modal._panelMax

        Item {
            id: gridSlot

            width: parent.width
            height: modal._count > 0 ? modal._gridHeight : Sizing.pctH(10)

            // Loading / no-sections cue. Shown only when there are no buckets:
            // "loading" while the facet is still resolving, otherwise this scope
            // has no first-character rail (e.g. a non-alphabetical sort).
            Text {
                anchors.centerIn: parent
                visible: modal._count <= 0
                text: modal.loading ? qsTr("Loading…") : qsTr("No sections")
                color: Theme.textLabel
                font.family: Theme.fontUi
                font.pixelSize: Sizing.fontSize(2.6)
                renderType: Text.NativeRendering
            }

            Grid {
                id: grid

                visible: modal._count > 0
                x: Sizing.center(gridSlot.width, width)
                columns: modal._columns
                spacing: modal._gap

                Repeater {
                    model: modal.entries

                    Rectangle {
                        id: cell

                        required property int index
                        required property var modelData

                        width: modal._cell
                        height: modal._cell
                        color: Theme.surfaceCard
                        border.width: cell.index === modal.currentIndex ? Sizing.stroke(2) : Sizing.stroke(1)
                        border.color: cell.index === modal.currentIndex ? Theme.focusItem : Theme.borderMid
                        radius: Sizing.cornerRadius
                        transformOrigin: Item.Center
                        scale: cell.index === modal.currentIndex ? modal._pressScale : 1.0

                        Column {
                            // Centered as one block via Sizing.center on the
                            // Column itself; glyphs render left-aligned so no
                            // run straddles a half-pixel (see Integer-pixel
                            // rules).
                            x: Sizing.center(cell.width, width)
                            y: Sizing.center(cell.height, height)
                            spacing: Sizing.pctH(0.4)

                            Text {
                                x: Sizing.center(parent.width, width)
                                text: cell.modelData.label
                                color: Theme.textPrimary
                                font.family: Theme.fontUi
                                font.pixelSize: Sizing.fontSize(3.4)
                                renderType: Text.NativeRendering
                            }

                            Text {
                                x: Sizing.center(parent.width, width)
                                text: cell.modelData.count
                                color: Theme.textLabel
                                font.family: Theme.fontUi
                                font.pixelSize: Sizing.fontSize(1.8)
                                renderType: Text.NativeRendering
                            }
                        }

                        MouseArea {
                            anchors.fill: parent
                            hoverEnabled: true
                            acceptedButtons: Qt.LeftButton
                            cursorShape: Qt.PointingHandCursor
                            onEntered: modal.currentIndex = cell.index
                            onClicked: modal._commitAccept(modal._offsetForIndex(cell.index))
                        }
                    }
                }
            }
        }
    }
}
