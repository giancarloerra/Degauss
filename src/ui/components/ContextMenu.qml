// Zaparoo Frontend
// Copyright (c) 2026 Wizzo Pty Ltd and the Zaparoo Project contributors.
// SPDX-License-Identifier: LicenseRef-PolyForm-Noncommercial-1.0.0
pragma ComponentBehavior: Bound

import QtQuick
import Zaparoo.Theme

// `entries` is a `var` array of plain JS objects (`{ id, label }`). The
// AOT compiler can't infer the shape of `var`, so every binding that
// reads `entries.length` or `modelData.label` falls back to the JS
// interpreter and trips the compiler category. Suppress file-wide.
// qmllint disable compiler

// Software-rendering safe contextual menu. It positions itself next to an
// anchor rectangle and clamps to the window bounds so edge tiles never push
// the menu off-screen. The scrim is drawn as four bands around `anchorRect`
// so the anchored tile stays bright while the rest of the screen dims —
// a full-screen scrim would defeat the "this menu is about *that* tile"
// affordance.
Item {
    id: menu

    property bool open: false
    property rect anchorRect: Qt.rect(0, 0, 0, 0)
    // Each entry is `{ id: string, label: string }`. `id` is the dispatch
    // key the router switches on (e.g. "launch_game", "qr_code"); `label`
    // is the localized text. Position-keyed dispatch was a footgun —
    // dynamic per-owner menus silently re-shuffled the index/action map.
    property var entries: []
    property int currentIndex: 0
    property int bottomUnsafeHeight: Sizing.pctH(6) + Sizing.pctH(2)

    // Push-in scale for the activated row, mirroring the tile push-in.
    property real _pressScale: 1.0
    property string _pendingId: ""

    signal accepted(string id)
    signal closeRequested

    readonly property int margin: Sizing.pctH(2)
    readonly property int gap: Sizing.pctW(1.2)
    readonly property int rowHeight: Sizing.pctH(6)
    readonly property int rowSpacing: Sizing.pctH(1)
    readonly property int horizontalPadding: Sizing.pctW(2)
    readonly property int panelSideMargin: Sizing.pctW(1)
    readonly property int _widestLabelWidth: _widestEntryLabelWidth(entries)
    readonly property int _usableBottom: Math.max(menu.margin, height - menu.bottomUnsafeHeight - menu.margin)
    readonly property int _minPanelWidth: Math.max(Sizing.pctW(24), Sizing.pctH(32))
    readonly property int _desiredPanelWidth: _widestLabelWidth + 2 * horizontalPadding + 2 * panelSideMargin + 2 * Sizing.stroke(2)
    readonly property int panelWidth: Math.min(Math.max(_minPanelWidth, _desiredPanelWidth), Math.max(0, width - 2 * margin))
    // Top/bottom margins inside the panel are sized to the panel
    // radius so a focused row's accent ring never intersects the
    // rounded corners — see the panel `Rectangle` below.
    readonly property int panelRadius: Sizing.half(Sizing.cornerRadius)
    readonly property int panelHeight: Math.min(entries.length * rowHeight + Math.max(0, entries.length - 1) * rowSpacing + 2 * panelRadius, Math.max(0, _usableBottom - menu.margin))
    readonly property bool _fitsRight: anchorRect.x + anchorRect.width + gap + panelWidth <= width - margin
    readonly property bool _fitsLeft: anchorRect.x - gap - panelWidth >= margin
    readonly property bool _placeBesideAnchor: _fitsRight || _fitsLeft
    readonly property bool _fitsBelow: anchorRect.y + anchorRect.height + gap + panelHeight <= _usableBottom
    readonly property int preferredX: _placeBesideAnchor ? (_fitsRight ? Sizing.px(anchorRect.x + anchorRect.width + gap) : Sizing.px(anchorRect.x - gap - panelWidth)) : Sizing.px(anchorRect.x + Sizing.center(anchorRect.width, panelWidth))
    readonly property int preferredY: _placeBesideAnchor ? Sizing.px(anchorRect.y + Sizing.center(anchorRect.height, panelHeight)) : (_fitsBelow ? Sizing.px(anchorRect.y + anchorRect.height + gap) : Sizing.px(anchorRect.y - gap - panelHeight))

    visible: open
    enabled: visible
    anchors.fill: parent
    z: 250

    onOpenChanged: {
        if (open) {
            currentIndex = 0;
            menu._pressScale = 1.0;
            pressAnim.stop();
            menu._pendingId = "";
        }
    }

    onEntriesChanged: {
        if (menu.entries.length <= 0) {
            currentIndex = 0;
            return;
        }
        if (menu.currentIndex >= menu.entries.length)
            currentIndex = menu.entries.length - 1;
    }

    function _widestEntryLabelWidth(source: var): int {
        let widest = 0;
        if (source === null || source === undefined)
            return widest;
        for (let i = 0; i < source.length; ++i) {
            const label = source[i] && source[i].label !== undefined ? String(source[i].label) : "";
            widest = Math.max(widest, Math.ceil(labelMetrics.advanceWidth(label)));
        }
        return widest;
    }

    function move(delta: int): void {
        if (menu.entries.length <= 0)
            return;
        menu.currentIndex = ((menu.currentIndex + delta) % menu.entries.length + menu.entries.length) % menu.entries.length;
    }

    function handleAction(action: string): void {
        if (action === "up")
            menu.move(-1);
        else if (action === "down")
            menu.move(1);
        else if (action === "accept") {
            if (menu.currentIndex >= 0 && menu.currentIndex < menu.entries.length)
                menu._commitAccept(menu.entries[menu.currentIndex].id);
        } else if (action === "cancel" || action === "context_menu")
            menu.closeRequested();
    }

    // Play the push-in cue on the focused row, then emit accepted(id)
    // deferred so the animation completes before the caller acts.
    function _commitAccept(id: string): void {
        menu._pendingId = id;
        pressAnim.restart();
        acceptCommit.arm();
    }

    NumberAnimation {
        id: pressAnim
        target: menu
        property: "_pressScale"
        to: Motion.rowPressScale
        duration: Motion.dur(Motion.pressMs)
        easing.type: Easing.OutQuad
    }

    DeferredAction {
        id: acceptCommit
        onDeferred: {
            const id = menu._pendingId;
            menu._pendingId = "";
            if (id !== "")
                menu.accepted(id);
        }
    }

    FontMetrics {
        id: labelMetrics
        font.family: Theme.fontUi
        font.pixelSize: Sizing.fontSize(2.4)
    }

    // Catches dismiss-clicks on the dimmed area around the anchor.
    // Sits beneath the four scrim bands and the panel; per-row
    // MouseAreas inside the panel win for clicks on rows because the
    // panel is declared after this MouseArea, so the panel subtree
    // sits on top in z-order. Clicks on the anchor area also hit this
    // MouseArea (the bands don't cover the anchor) and close the menu.
    // Clicks inside the panel chrome (top/bottom radius padding, side
    // margins, row spacing) are filtered out by the bounding-rect
    // check so a stray press on padding doesn't dismiss.
    //
    // Swallows hover and every mouse button so neither hover events
    // nor right-clicks bleed through to the underlying grid while the
    // menu is open. Without `hoverEnabled` and `Qt.AllButtons` the
    // grid below would highlight tiles under the scrim and a right
    // click on the dim area would land on the grid's context handler.
    MouseArea {
        anchors.fill: parent
        hoverEnabled: true
        acceptedButtons: Qt.AllButtons
        onClicked: mouse => {
            if (mouse.x < panel.x || mouse.y < panel.y || mouse.x > panel.x + panel.width || mouse.y > panel.y + panel.height)
                menu.closeRequested();
        }
    }

    // Four scrim bands framing `anchorRect`. The anchor area itself is
    // intentionally not painted so the tile the menu is *about* stays
    // bright. `Math.max(0, ...)` clamps every dimension so an anchor
    // flush against an edge collapses the matching band rather than
    // overflowing into negative territory.
    Rectangle {
        x: 0
        y: 0
        width: menu.width
        height: Sizing.px(Math.max(0, menu.anchorRect.y))
        color: Theme.scrim
    }
    Rectangle {
        x: 0
        y: Sizing.px(menu.anchorRect.y + menu.anchorRect.height)
        width: menu.width
        height: Sizing.px(Math.max(0, menu.height - (menu.anchorRect.y + menu.anchorRect.height)))
        color: Theme.scrim
    }
    Rectangle {
        x: 0
        y: Sizing.px(menu.anchorRect.y)
        width: Sizing.px(Math.max(0, menu.anchorRect.x))
        height: Sizing.px(Math.max(0, menu.anchorRect.height))
        color: Theme.scrim
    }
    Rectangle {
        x: Sizing.px(menu.anchorRect.x + menu.anchorRect.width)
        y: Sizing.px(menu.anchorRect.y)
        width: Sizing.px(Math.max(0, menu.width - (menu.anchorRect.x + menu.anchorRect.width)))
        height: Sizing.px(Math.max(0, menu.anchorRect.height))
        color: Theme.scrim
    }

    Rectangle {
        id: panel

        x: Sizing.px(Math.max(menu.margin, Math.min(menu.preferredX, menu.width - menu.margin - menu.panelWidth)))
        y: Sizing.px(Math.max(menu.margin, Math.min(menu.preferredY, menu._usableBottom - menu.panelHeight)))
        width: menu.panelWidth
        height: menu.panelHeight
        color: Theme.bgPanel
        radius: menu.panelRadius

        Column {
            anchors.fill: parent
            anchors.topMargin: menu.panelRadius
            anchors.bottomMargin: menu.panelRadius
            anchors.leftMargin: menu.panelSideMargin
            anchors.rightMargin: menu.panelSideMargin
            spacing: menu.rowSpacing

            Repeater {
                model: menu.entries

                Rectangle {
                    id: row

                    required property int index
                    required property var modelData

                    width: parent.width
                    height: menu.rowHeight
                    color: Theme.surfaceCard
                    border.width: index === menu.currentIndex ? Sizing.stroke(2) : Sizing.stroke(1)
                    border.color: index === menu.currentIndex ? Theme.focusItem : Theme.borderMid
                    radius: Sizing.cornerRadius
                    transformOrigin: Item.Center
                    scale: row.index === menu.currentIndex ? menu._pressScale : 1.0

                    Text {
                        anchors.left: parent.left
                        anchors.right: parent.right
                        anchors.verticalCenter: parent.verticalCenter
                        anchors.leftMargin: menu.horizontalPadding
                        anchors.rightMargin: menu.horizontalPadding
                        text: row.modelData.label
                        color: Theme.textPrimary
                        font.family: Theme.fontUi
                        font.pixelSize: Sizing.fontSize(2.4)
                        elide: Text.ElideRight
                        renderType: Text.NativeRendering
                    }

                    MouseArea {
                        anchors.fill: parent
                        hoverEnabled: true
                        acceptedButtons: Qt.LeftButton
                        cursorShape: Qt.PointingHandCursor
                        onEntered: menu.currentIndex = row.index
                        onClicked: menu._commitAccept(row.modelData.id)
                    }
                }
            }
        }
    }
}
