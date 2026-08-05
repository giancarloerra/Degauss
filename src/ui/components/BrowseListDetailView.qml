// Zaparoo Frontend
// Copyright (c) 2026 Wizzo Pty Ltd and the Zaparoo Project contributors.
// SPDX-License-Identifier: LicenseRef-PolyForm-Noncommercial-1.0.0

import QtQuick
import Zaparoo.Theme

Item {
    id: root

    property alias model: browseList.model
    property alias currentIndex: browseList.currentIndex
    property alias totalItemsOverride: browseList.totalItemsOverride
    property alias targetVisibleRowCount: browseList.targetVisibleRowCount
    property alias currentName: browseList.currentName
    property alias currentCoverKey: browseList.currentCoverKey
    property alias itemCount: browseList.itemCount
    property alias visibleRowCount: browseList.visibleRowCount
    property var layoutProfile: null

    property alias detailTitle: detailPane.title
    property alias detailCoverKey: detailPane.coverKey
    property alias detailDescription: detailPane.description
    property alias detailShowDescription: detailPane.showDescription
    property alias detailShowTitle: detailPane.showTitle
    property alias detailTags: detailPane.detailTags
    property alias detailLoading: detailPane.loading
    property alias detailLoadingText: detailPane.loadingText
    property alias detailSuppressed: detailPane.detailSuppressed
    property alias activatePulse: browseList.activatePulse
    property alias releasePulse: browseList.releasePulse
    property alias screenSettling: browseList.screenSettling
    property alias focusReady: browseList.focusReady
    property alias rapidRenderMode: browseList.rapidRenderMode
    property alias detailCanPreviousImage: detailPane.canPreviousImage
    property alias detailCanNextImage: detailPane.canNextImage
    property alias detailReserveImageNav: detailPane.reserveImageNav

    // layoutProfile and its sub-objects are JS-object vars; the QML compiler
    // cannot statically type their properties. Suppress for these bindings.
    // qmllint disable compiler
    property var _listProfile: root.layoutProfile && root.layoutProfile.list ? root.layoutProfile.list : null
    property var _surfaceProfile: root.layoutProfile && root.layoutProfile.surface ? root.layoutProfile.surface : null
    readonly property bool _verticalSplit: root._listProfile && root._listProfile.contentAxis === "vertical"
    readonly property int _dividerWidth: root._listProfile && root._listProfile.dividerWidth !== undefined ? root._listProfile.dividerWidth : Sizing.stroke(1)
    readonly property int _dividerMargin: root._listProfile && root._listProfile.dividerMargin !== undefined ? root._listProfile.dividerMargin : 0
    readonly property real _listShare: root._listProfile && root._listProfile.listShare !== undefined ? root._listProfile.listShare : 2
    readonly property real _detailShare: root._listProfile && root._listProfile.detailShare !== undefined ? root._listProfile.detailShare : 1
    readonly property real _shareTotal: Math.max(1, root._listShare + root._detailShare)
    readonly property int _listSpan: root._verticalSplit ? Math.max(0, Math.floor((height - root._dividerWidth) * root._listShare / root._shareTotal) + root._dividerMargin) : Math.max(0, Math.floor((width - root._dividerWidth) * root._listShare / root._shareTotal) + root._dividerMargin)
    readonly property int _detailSpan: root._verticalSplit ? Math.max(0, height - root._listSpan - root._dividerWidth) : Math.max(0, width - root._listSpan - root._dividerWidth)
    readonly property int _cardRadius: root._surfaceProfile ? root._surfaceProfile.cornerRadius : Sizing.cornerRadius
    // qmllint enable compiler

    signal itemHovered(int index)
    signal itemClicked(int index)
    signal itemRightClicked(int index)
    signal emptyRightClicked
    signal pageWheelRequested(int delta)

    function currentCellRectIn(target: Item): rect {
        return browseList.currentCellRectIn(target);
    }

    Rectangle {
        anchors.fill: parent
        color: Theme.surfaceCard
        border.width: Sizing.stroke(1)
        border.color: Theme.borderMid
        radius: root._cardRadius
    }

    BrowseList {
        id: browseList

        x: 0
        y: 0
        width: root._verticalSplit ? parent.width : root._listSpan
        height: root._verticalSplit ? root._listSpan : parent.height
        layoutProfile: root.layoutProfile
        showChrome: false
        onItemHovered: index => root.itemHovered(index)
        onItemClicked: index => root.itemClicked(index)
        onItemRightClicked: index => root.itemRightClicked(index)
        onEmptyRightClicked: root.emptyRightClicked()
        onPageWheelRequested: delta => root.pageWheelRequested(delta)
    }

    Rectangle {
        x: root._verticalSplit ? 0 : browseList.width
        y: root._verticalSplit ? browseList.height : 0
        width: root._verticalSplit ? parent.width : root._dividerWidth
        height: root._verticalSplit ? root._dividerWidth : parent.height
        color: Theme.borderMid
    }

    BrowseDetailPane {
        id: detailPane

        x: root._verticalSplit ? 0 : browseList.width + root._dividerWidth
        y: root._verticalSplit ? browseList.height + root._dividerWidth : 0
        width: root._verticalSplit ? parent.width : root._detailSpan
        height: root._verticalSplit ? root._detailSpan : parent.height
        layoutProfile: root.layoutProfile
        showChrome: false
    }
}
