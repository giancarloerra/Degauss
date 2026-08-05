// Zaparoo Frontend
// Copyright (c) 2026 Wizzo Pty Ltd and the Zaparoo Project contributors.
// SPDX-License-Identifier: LicenseRef-PolyForm-Noncommercial-1.0.0
pragma ComponentBehavior: Bound

import QtQuick
import Zaparoo.Theme

Item {
    id: root

    required property var model
    property int currentIndex: 0
    // Gates whether the selected row paints its highlight (selection surface +
    // accent bar + bright text). The host leaves it false until the screen's
    // selection is finalized (restore or first input) so the default row 0
    // never lights up during the window before restore points currentIndex at
    // the saved item on a cold start. Default true so unwired hosts highlight
    // the selection normally.
    property bool focusReady: true
    property string currentName: ""
    property string currentCoverKey: ""
    property int totalItemsOverride: -1
    property int targetVisibleRowCount: 0
    property bool showChrome: true
    property var layoutProfile: null
    // layoutProfile and its sub-objects (_list, _grid, _surface) are JS-object
    // vars; the QML compiler cannot statically type their properties. Suppress
    // the compiler category for these bindings only.
    // qmllint disable compiler
    readonly property var _list: root.layoutProfile && root.layoutProfile.list ? root.layoutProfile.list : null
    readonly property var _grid: root.layoutProfile && root.layoutProfile.grid ? root.layoutProfile.grid : null
    readonly property var _surface: root.layoutProfile && root.layoutProfile.surface ? root.layoutProfile.surface : null
    readonly property int itemCount: listView.count
    readonly property int totalItems: totalItemsOverride >= 0 ? totalItemsOverride : itemCount
    readonly property bool _portraitNonCrt: !Theme.crtNativePath && Sizing.screenWidth < Sizing.screenHeight
    readonly property int _selectionRadius: root._surface ? root._surface.cornerRadius : Sizing.cornerRadius
    readonly property int cardPaddingLeft: root._list ? root._list.cardPaddingLeft : Sizing.pctW(2)
    readonly property int cardPaddingRight: root._list ? root._list.cardPaddingRight : Sizing.pctW(2)
    readonly property int cardPaddingTop: root._list ? root._list.cardPaddingTop : Sizing.pctH(2)
    readonly property int cardPaddingBottom: root._list ? root._list.cardPaddingBottom : Sizing.pctH(2)
    readonly property int rowSpacing: root._list ? root._list.rowSpacing : (root._portraitNonCrt ? Sizing.pctH(0.3) : Sizing.pctH(0.7))
    readonly property int contentHeight: Math.max(0, height - cardPaddingTop - cardPaddingBottom)
    readonly property int rowHeight: root._list && root._list.rowHeight > 0 ? root._list.rowHeight : (targetVisibleRowCount > 0 ? Math.max(Sizing.pctH(3), Math.floor((contentHeight - (rowSpacing * (targetVisibleRowCount - 1))) / targetVisibleRowCount)) : Sizing.pctH(6))
    readonly property int rowStride: rowHeight + rowSpacing
    readonly property int visibleRowCount: targetVisibleRowCount > 0 ? targetVisibleRowCount : Math.max(1, Math.floor((contentHeight + rowSpacing) / rowStride))
    readonly property int _centerSlot: root._list && root._list.centerSlot >= 0 ? Math.max(0, Math.min(visibleRowCount - 1, root._list.centerSlot)) : Math.max(0, Math.floor((visibleRowCount - 1) / 2))
    readonly property int _maxViewTopIndex: Math.max(0, itemCount - visibleRowCount)
    readonly property int _viewTopIndex: Math.max(0, Math.min(_maxViewTopIndex, currentIndex - _centerSlot))
    readonly property int _targetContentY: _viewTopIndex * rowStride
    readonly property int _maxScrollTopIndex: Math.max(0, totalItems - visibleRowCount)
    readonly property int _gutterWidth: root._grid ? root._grid.gutterWidth : Sizing.pctW(3)
    readonly property int _gutterGap: root._list && root._list.scrollbarGap !== undefined ? root._list.scrollbarGap : (root._grid ? root._grid.gutterGap : Sizing.pctW(1.5))
    readonly property int _scrollThumbWidth: root._grid ? root._grid.scrollThumbWidth : Sizing.pctW(1.2)
    readonly property int _scrollThumbRightInset: root._grid ? root._grid.scrollThumbRightInset : 0
    readonly property bool _scrollThumbRightAligned: root._grid && root._grid.scrollThumbRightAligned !== undefined ? root._grid.scrollThumbRightAligned : false
    readonly property int _scrollArrowSize: root._grid ? root._grid.scrollArrowSize : Math.min(root._gutterWidth, Sizing.pctH(4))
    readonly property int _selectionAccentWidth: root._list && root._list.selectionAccentWidth !== undefined ? root._list.selectionAccentWidth : Sizing.pctW(0.45)
    readonly property int _rowTextLeftPadding: root._list ? root._list.rowTextLeftPadding : Sizing.pctW(1.6)
    readonly property int _rowTextRightPadding: root._list ? root._list.rowTextRightPadding : Sizing.pctW(1.6)
    readonly property int _favoriteRightPadding: root._list ? root._list.favoriteRightPadding : Sizing.pctW(1.6)
    // qmllint enable compiler

    // Pulse counter for the one-shot row push-in. Callers increment via
    // activatePulse; only the selected row fires its animation, matching
    // the Tile activation-pulse vocabulary. Forward navigation and game
    // launch share this single cue.
    property int activatePulse: 0
    // Release counter for the row push-in. Incremented by the host to settle
    // the selected row's scale back to 1.0 after a launch that keeps the
    // frontend on the same screen. Forward navigation never increments it (the
    // screen transition resets the push-in off-screen via screenSettling).
    property int releasePulse: 0
    // When true, resets the row push-in scale back to 1.0 so a held press does
    // not persist when the screen is shown again. Set by the host to
    // !active while the screen is off-screen.
    property bool screenSettling: false
    // During rapid scrolling every landing rebinds all visible rows at
    // once, and on the software renderer the decoration (heart icon,
    // disambiguating tag layout) is a measurable share of that cost.
    // Rapid render mode strips rows to their plain title so fast paging
    // spends its frame budget on travel, mirroring the grid's
    // rapidRenderMode; decoration pops back in when the scroll settles.
    property bool rapidRenderMode: false

    signal itemHovered(int index)
    signal itemClicked(int index)
    signal itemRightClicked(int index)
    signal emptyRightClicked
    signal pageWheelRequested(int delta)

    function _handleWheel(wheel: WheelEvent): void {
        const amount = wheel.angleDelta.y !== 0 ? wheel.angleDelta.y : wheel.pixelDelta.y;
        if (amount === 0)
            return;
        root.pageWheelRequested(amount < 0 ? 1 : -1);
        wheel.accepted = true;
    }

    function currentCellRectIn(target: Item): rect {
        if (root.itemCount <= 0)
            return Qt.rect(0, 0, 0, 0);
        const item = listView.currentItem;
        if (item === null)
            return Qt.rect(0, 0, 0, 0);
        const p = listView.mapToItem(target, 0, item.y - listView.contentY);
        return Qt.rect(p.x, p.y, listView.width, root.rowHeight);
    }

    function _syncContentY(): void {
        const maxY = Math.max(0, listView.contentHeight - listView.height);
        const targetY = Math.min(root._targetContentY, maxY);
        if (listView.contentY !== targetY)
            listView.contentY = targetY;
    }

    clip: true

    Rectangle {
        anchors.fill: parent
        color: Theme.surfaceCard
        border.width: Sizing.stroke(1)
        border.color: Theme.borderMid
        radius: root._selectionRadius
        visible: root.showChrome
    }

    onItemCountChanged: {
        if (root.itemCount === 0) {
            root.currentName = "";
            root.currentCoverKey = "";
        }
        root._syncContentY();
    }
    on_TargetContentYChanged: root._syncContentY()

    MouseArea {
        anchors.fill: parent
        acceptedButtons: Qt.RightButton
        onClicked: root.emptyRightClicked()
        onWheel: wheel => root._handleWheel(wheel)
    }

    ListView {
        id: listView

        anchors.left: parent.left
        anchors.leftMargin: root.cardPaddingLeft
        anchors.top: parent.top
        anchors.topMargin: root.cardPaddingTop
        anchors.bottom: parent.bottom
        anchors.bottomMargin: root.cardPaddingBottom
        anchors.right: parent.right
        anchors.rightMargin: root.totalItems > root.visibleRowCount ? root._gutterWidth + root._gutterGap + root.cardPaddingRight : root.cardPaddingRight
        model: root.model
        currentIndex: root.currentIndex
        boundsBehavior: Flickable.StopAtBounds
        interactive: false
        spacing: root.rowSpacing
        highlightFollowsCurrentItem: false
        Component.onCompleted: root._syncContentY()
        onContentHeightChanged: root._syncContentY()
        onHeightChanged: root._syncContentY()

        delegate: Item {
            id: row

            required property int index
            required property string name
            required property string fileStem
            required property string coverKey
            required property int favorite
            // Newline-joined disambiguating-tag tokens (empty for models
            // without variants). Every Browse model exposes this role.
            required property string disambiguatingTags

            width: listView.width
            height: root.rowHeight
            // One-shot push-in cue, identical to the tile vocabulary: the
            // selected row scales to Motion.rowPressScale on accept/activate.
            scale: row._activateScale
            transformOrigin: Item.Center

            readonly property bool selected: row.index === root.currentIndex
            // Visual highlight is withheld until the host marks focus ready, so
            // the default row 0 never paints the accent before restore lands.
            // `selected` itself stays ungated so the detail-pane bindings below
            // still track content during the pre-restore window.
            readonly property bool _highlightVisible: row.selected && root.focusReady
            readonly property string _baseTitle: row.name !== "" ? row.name : row.fileStem
            // Horizontal space reserved on the right for the favorite heart.
            readonly property int _favoriteSlot: row.favorite !== 0 && !root.rapidRenderMode ? root._favoriteRightPadding + Sizing.pctH(3.2) : 0
            property real _activateScale: 1.0

            // Push in and hold — mirrors Tile.qml. The activate leg has no
            // return-to-rest because a forward navigation holds the row pressed
            // while the screen transitions; the release leg settles it back only
            // when the launch stays on this screen (e.g. an Audio track), and
            // `screenSettling` resets it off-screen so it is clean on return.
            NumberAnimation {
                id: activateAnim
                target: row
                property: "_activateScale"
                to: Motion.rowPressScale
                duration: Motion.dur(Motion.pressMs)
                easing.type: Easing.OutQuad
            }

            NumberAnimation {
                id: releaseAnim
                target: row
                property: "_activateScale"
                to: 1.0
                duration: Motion.dur(Motion.settleMs)
                easing.type: Easing.OutQuad
            }

            Connections {
                target: root
                function onActivatePulseChanged(): void {
                    if (row.selected)
                        activateAnim.restart();
                }
                function onReleasePulseChanged(): void {
                    if (row.selected) {
                        activateAnim.stop();
                        releaseAnim.restart();
                    }
                }
                function onScreenSettlingChanged(): void {
                    if (root.screenSettling) {
                        activateAnim.stop();
                        releaseAnim.stop();
                        row._activateScale = 1.0;
                    }
                }
            }

            Binding {
                target: root
                property: "currentName"
                when: row.selected
                value: row._baseTitle
                restoreMode: Binding.RestoreNone
            }

            Binding {
                target: root
                property: "currentCoverKey"
                when: row.selected
                value: row.coverKey
                restoreMode: Binding.RestoreNone
            }

            Item {
                width: parent.width
                height: parent.height
                visible: row._highlightVisible

                Rectangle {
                    anchors.fill: parent
                    color: Theme.selectionSurface
                    radius: root._selectionRadius
                }

                Rectangle {
                    anchors.left: parent.left
                    anchors.top: parent.top
                    anchors.bottom: parent.bottom
                    width: root._selectionRadius
                    color: Theme.selectionSurface
                }
            }

            Rectangle {
                anchors.top: parent.top
                anchors.bottom: parent.bottom
                width: root._selectionAccentWidth
                color: Theme.accent
                visible: row._highlightVisible
                radius: Math.max(0, Sizing.px(width / 3))
            }

            // Row title carrying the inline dim token suffix. ScrollingCaption
            // left-aligns and elides it, pins the top token after the name
            // elides, and marquees the full string while this row is the
            // selection (reduce-motion falls back to a static elide). The right
            // margin reserves the favorite-heart slot.
            ScrollingCaption {
                anchors.left: parent.left
                anchors.leftMargin: root._rowTextLeftPadding
                anchors.right: parent.right
                anchors.rightMargin: row._favoriteSlot + root._rowTextRightPadding
                anchors.verticalCenter: parent.verticalCenter
                height: parent.height
                name: row._baseTitle
                tags: root.rapidRenderMode ? "" : row.disambiguatingTags
                focused: row._highlightVisible
                centerContent: false
                fontPixelSize: Sizing.fontSize(2.9)
                nameColor: row._highlightVisible ? Theme.textPrimary : Theme.textListTitleDim
            }

            Image {
                anchors.right: parent.right
                anchors.rightMargin: root._favoriteRightPadding
                anchors.verticalCenter: parent.verticalCenter
                width: Sizing.pctH(3.2)
                height: width
                // Tinted like the tile heart: favourite red with the
                // dark keyline, via the tinted-svg provider.
                source: Resources.coverUrl("icons/Heart", Theme.favoriteMarker, Theme.favoriteMarker, Theme.bgBar)
                sourceSize.width: Sizing.px(width)
                sourceSize.height: Sizing.px(height)
                fillMode: Image.PreserveAspectFit
                smooth: true
                asynchronous: false
                visible: row.favorite !== 0 && !root.rapidRenderMode
            }

            MouseArea {
                anchors.fill: parent
                hoverEnabled: true
                acceptedButtons: Qt.LeftButton | Qt.RightButton
                cursorShape: Qt.PointingHandCursor

                onEntered: root.itemHovered(row.index)
                onClicked: mouse => {
                    if (mouse.button === Qt.RightButton)
                        root.itemRightClicked(row.index);
                    else
                        root.itemClicked(row.index);
                }
                onWheel: wheel => root._handleWheel(wheel)
            }
        }
    }

    // ── Left-half scroll indicator ────────────────────────────────────────
    Item {
        id: scrollGutter

        anchors.right: parent.right
        anchors.rightMargin: root.cardPaddingRight
        anchors.top: parent.top
        anchors.topMargin: root.cardPaddingTop
        anchors.bottom: parent.bottom
        anchors.bottomMargin: root.cardPaddingBottom
        width: root._gutterWidth
        visible: root.totalItems > root.visibleRowCount

        Image {
            id: upArrow
            source: Resources.iconUrl("ScrollUp")
            width: root._scrollArrowSize
            height: root._scrollArrowSize
            anchors.top: parent.top
            anchors.horizontalCenter: parent.horizontalCenter
            fillMode: Image.PreserveAspectFit
            smooth: true
            visible: root.currentIndex > 0

            MouseArea {
                anchors.fill: parent
                acceptedButtons: Qt.LeftButton
                cursorShape: Qt.PointingHandCursor
                enabled: upArrow.visible
                onClicked: root.pageWheelRequested(-1)
            }
        }

        Image {
            id: downArrow
            source: Resources.iconUrl("ScrollDown")
            width: root._scrollArrowSize
            height: root._scrollArrowSize
            anchors.bottom: parent.bottom
            anchors.horizontalCenter: parent.horizontalCenter
            fillMode: Image.PreserveAspectFit
            smooth: true
            visible: root.currentIndex < root.totalItems - 1

            MouseArea {
                anchors.fill: parent
                acceptedButtons: Qt.LeftButton
                cursorShape: Qt.PointingHandCursor
                enabled: downArrow.visible
                onClicked: root.pageWheelRequested(1)
            }
        }

        Item {
            id: scrollRegion
            anchors.top: parent.top
            anchors.topMargin: root._scrollArrowSize + Sizing.pctH(1)
            anchors.bottom: parent.bottom
            anchors.bottomMargin: root._scrollArrowSize + Sizing.pctH(1)
            anchors.right: root._scrollThumbRightAligned ? parent.right : undefined
            anchors.rightMargin: root._scrollThumbRightAligned ? root._scrollThumbRightInset : 0
            anchors.horizontalCenter: root._scrollThumbRightAligned ? undefined : parent.horizontalCenter
            width: root._scrollThumbWidth

            readonly property int _minThumbHeight: Sizing.pctH(4)
            readonly property int _thumbHeight: root.totalItems <= 0 ? 0 : Math.min(scrollRegion.height, Math.max(_minThumbHeight, Math.round(scrollRegion.height * root.visibleRowCount / root.totalItems)))
            readonly property int _thumbY: root._maxScrollTopIndex <= 0 ? 0 : Sizing.px((root._viewTopIndex / root._maxScrollTopIndex) * (scrollRegion.height - _thumbHeight))

            Rectangle {
                id: scrollThumb
                width: root._scrollThumbWidth
                height: scrollRegion._thumbHeight
                anchors.right: root._scrollThumbRightAligned ? parent.right : undefined
                anchors.horizontalCenter: root._scrollThumbRightAligned ? undefined : parent.horizontalCenter
                y: scrollRegion._thumbY
                color: Theme.textPrimary
                radius: Sizing.half(width)
            }
        }
    }
}
