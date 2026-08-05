// Zaparoo Frontend
// Copyright (c) 2026 Wizzo Pty Ltd and the Zaparoo Project contributors.
// SPDX-License-Identifier: LicenseRef-PolyForm-Noncommercial-1.0.0
pragma ComponentBehavior: Bound

import QtQuick
import Zaparoo.Theme

Item {
    id: root

    property string title: ""
    property string coverKey: ""
    property string description: ""
    property bool showDescription: true
    property bool showTitle: true
    property string detailTags: ""
    property bool canPreviousImage: false
    property bool canNextImage: false
    // Reserve the carousel gutter even before can_next/can_prev are known,
    // so the cover footprint is stable from the first frame on screens that
    // support image cycling. Set to true by GamesScreen; Recents/Favorites
    // leave it false (they have no carousel wiring).
    property bool reserveImageNav: false
    property bool loading: false
    property bool detailSuppressed: false
    property bool showChrome: true
    property string loadingText: qsTr("Loading…")
    property int loadingDelayMs: 150
    property var layoutProfile: null

    readonly property var _detail: root.layoutProfile && root.layoutProfile.detail ? root.layoutProfile.detail : null
    readonly property var _surface: root.layoutProfile && root.layoutProfile.surface ? root.layoutProfile.surface : null
    readonly property int _panePaddingLeft: root._detail ? root._detail.panePaddingLeft : Sizing.pctW(2)
    readonly property int _panePaddingRight: root._detail ? root._detail.panePaddingRight : Sizing.pctW(2)
    readonly property int _panePaddingTop: root._detail ? root._detail.panePaddingTop : Sizing.pctH(2)
    readonly property int _panePaddingBottom: root._detail ? root._detail.panePaddingBottom : Sizing.pctH(2)
    readonly property int _imagePaddingLeft: root._detail ? root._detail.imagePaddingLeft : 0
    readonly property int _imagePaddingRight: root._detail ? root._detail.imagePaddingRight : 0
    readonly property int _imagePaddingTop: root._detail ? root._detail.imagePaddingTop : 0
    readonly property int _imagePaddingBottom: root._detail ? root._detail.imagePaddingBottom : 0
    readonly property int _metadataPaddingLeft: root._detail ? root._detail.metadataPaddingLeft : 0
    readonly property int _metadataPaddingRight: root._detail ? root._detail.metadataPaddingRight : 0
    readonly property int _metadataPaddingTop: root._detail ? root._detail.metadataPaddingTop : 0
    readonly property int _metadataPaddingBottom: root._detail ? root._detail.metadataPaddingBottom : 0
    readonly property int _metadataTopMargin: root._detail && root._detail.metadataTopMargin !== undefined ? root._detail.metadataTopMargin : 0
    readonly property int _metadataLeftMargin: root._detail && root._detail.metadataLeftMargin !== undefined ? root._detail.metadataLeftMargin : 0
    readonly property int _metadataRightMargin: root._detail && root._detail.metadataRightMargin !== undefined ? root._detail.metadataRightMargin : 0
    readonly property int _metadataHeightAdjustment: root._detail && root._detail.metadataHeightAdjustment !== undefined ? root._detail.metadataHeightAdjustment : 0
    readonly property int _sectionGap: root._detail ? root._detail.sectionGap : Sizing.pctH(2)
    readonly property bool _horizontalSections: root._detail && root._detail.contentAxis === "horizontal"
    readonly property real _imageShare: root._detail && root._detail.imageShare !== undefined ? root._detail.imageShare : 1
    readonly property real _metadataShare: root._detail && root._detail.metadataShare !== undefined ? root._detail.metadataShare : 1
    readonly property real _shareTotal: Math.max(1, root._imageShare + root._metadataShare)
    readonly property int _tagRowHeight: root._detail ? root._detail.tagRowHeight : Sizing.pctH(3)
    readonly property int _tagRowSpacing: root._detail ? root._detail.tagRowSpacing : Sizing.pctH(0.55)
    readonly property bool _metadataBottomAligned: root._detail && root._detail.metadataBottomAligned === true
    readonly property int _titleBottomMargin: root._detail ? root._detail.titleBottomMargin : Sizing.pctH(2)
    readonly property real _imageHeightRatioWithTitle: root._detail && root._detail.imageHeightRatioWithTitle !== undefined ? root._detail.imageHeightRatioWithTitle : 48
    readonly property int _imageReservedWidth: root._detail && root._detail.imageReservedWidth !== undefined ? root._detail.imageReservedWidth : 0
    readonly property int _imageReservedHeight: root._detail && root._detail.imageReservedHeight !== undefined ? root._detail.imageReservedHeight : 0
    readonly property int _imageBottomMargin: root._detail && root._detail.imageBottomMargin !== undefined ? root._detail.imageBottomMargin : 0
    readonly property int _cardRadius: root._surface ? root._surface.cornerRadius : Sizing.cornerRadius
    // Reserve the side gutter whenever this screen supports image cycling
    // (reserveImageNav) OR when can_prev/can_next are already known, so the
    // cover footprint never changes when can_next flips async after meta loads.
    readonly property int _carouselGutter: (root.reserveImageNav || canPreviousImage || canNextImage) ? Sizing.pctW(4) : 0
    readonly property bool _coverPending: coverKey === "icons/Loading"
    // During the loading grace window, hold the last good cover URL so the
    // area does not blank while the new bytes arrive. Once the grace elapses
    // without resolution the source becomes "" and the busy indicator shows.
    readonly property url _coverSource: _coverPending ? (_coverLoadingDelayElapsed ? "" : _lastGoodCoverSource) : Resources.coverUrl(coverKey, Theme.logoFocusPrimary, Theme.logoFocusSecondary, Theme.logoFocusShadow)
    // True whenever the cover Image is in flight (model pending, Qt async
    // decode, or any non-media-image provider still loading).
    readonly property bool _coverMediaImagePending: coverKey.startsWith("media-image/") && cover.status !== Image.Ready && cover.status !== Image.Error
    readonly property bool _coverBusy: root._coverPending || root._coverMediaImagePending || cover.status === Image.Loading
    readonly property bool _paneLoading: root.loading
    readonly property bool _delayedPaneLoading: root._paneLoading && root._paneLoadingDelayElapsed
    // Gate every busy-cover signal behind the same grace delay. A cover that
    // resolves within `loadingDelayMs` (150 ms, the common warm case) never
    // shows the hourglass. A genuinely cold cover still pending after the
    // grace becomes visible because `_coverLoadingDelayElapsed` flips true.
    readonly property bool _coverBusyIndicatorVisible: root._coverBusy && root._coverLoadingDelayElapsed
    readonly property bool _detailVisible: !root.detailSuppressed
    readonly property bool _emptyPaneLoading: root._delayedPaneLoading && !root._coverBusyIndicatorVisible && root._coverSource === "" && root._displayRows.length === 0 && root.title === ""
    readonly property var _detailRows: _parseDetailTags(detailTags)
    readonly property int _tagRowCount: _displayRows.length
    readonly property int _tagTextSize: Sizing.fontSize(2.2)
    readonly property int _tagLabelGap: Sizing.pctW(1.4)
    readonly property int _metadataLabelMaxWidth: root._detail && root._detail.metadataLabelMaxWidth !== undefined ? root._detail.metadataLabelMaxWidth : 0
    readonly property int _labelColumnWidth: root._metadataLabelMaxWidth > 0 ? Math.min(root._labelColumnNaturalWidth, root._metadataLabelMaxWidth) : root._labelColumnNaturalWidth
    readonly property int _metadataNaturalHeight: _tagRowCount <= 0 ? 0 : (_tagRowCount * _tagRowHeight) + ((_tagRowCount - 1) * _tagRowSpacing)
    readonly property int _compactMetadataHeight: Math.min(Sizing.px(content.height * 0.38), _metadataNaturalHeight)
    // True for system-logo cover keys; used to select the wordmark fallback
    // instead of the generic File chip when no logo SVG exists.
    readonly property bool _isSystemCover: root.coverKey.startsWith("systems/")

    property int _labelColumnNaturalWidth: 0
    property bool _paneLoadingDelayElapsed: false
    property bool _coverLoadingDelayElapsed: false
    // Holds the last resolved cover URL so we can display it during the
    // loading grace window instead of blanking the cover area.
    property url _lastGoodCoverSource: ""
    // The detail table tracks the focused row's metadata directly. The model
    // keeps `current_detail_tags` identity-correct on every move — an immediate
    // peek shows cached/local rows or a clean blank, never the previous row's
    // values — so no client-side hold is needed. (The cover keeps its own hold;
    // see `coverHold`/`_lastGoodCoverSource` below.)
    readonly property var _displayRows: root._detailRows

    onLoadingChanged: root._updatePaneLoadingDelay()
    onLoadingDelayMsChanged: {
        root._updatePaneLoadingDelay();
        root._updateCoverLoadingDelay();
    }
    on_CoverBusyChanged: root._updateCoverLoadingDelay()

    Timer {
        id: paneLoadingDelayTimer

        interval: Math.max(0, root.loadingDelayMs)
        repeat: false
        onTriggered: root._paneLoadingDelayElapsed = root._paneLoading
    }

    Timer {
        id: coverLoadingDelayTimer

        interval: Math.max(0, root.loadingDelayMs)
        repeat: false
        onTriggered: root._coverLoadingDelayElapsed = root._coverBusy
    }

    function _updatePaneLoadingDelay(): void {
        paneLoadingDelayTimer.stop();
        root._paneLoadingDelayElapsed = false;
        if (!root._paneLoading)
            return;
        if (root.loadingDelayMs <= 0) {
            root._paneLoadingDelayElapsed = true;
            return;
        }
        paneLoadingDelayTimer.restart();
    }

    function _updateCoverLoadingDelay(): void {
        coverLoadingDelayTimer.stop();
        root._coverLoadingDelayElapsed = false;
        if (!root._coverBusy)
            return;
        if (root.loadingDelayMs <= 0) {
            root._coverLoadingDelayElapsed = true;
            return;
        }
        coverLoadingDelayTimer.restart();
    }

    function _tagLabel(fullLabel: string, shortLabel: string): var {
        return {
            "label": fullLabel + "\u009C" + shortLabel,
            "measureLabel": fullLabel
        };
    }

    function _localizedTagLabel(label: string): var {
        if (label === "Year")
            return root._tagLabel(qsTr("Year"), qsTr("Yr", "Short metadata label for Year; keep 2-4 characters if possible"));
        if (label === "Genre")
            return root._tagLabel(qsTr("Genre"), qsTr("Gen", "Short metadata label for Genre; keep 2-4 characters if possible"));
        if (label === "Players")
            return root._tagLabel(qsTr("Players"), qsTr("Plyr", "Short metadata label for Players; keep 2-4 characters if possible"));
        if (label === "Developer")
            return root._tagLabel(qsTr("Developer"), qsTr("Dev", "Short metadata label for Developer; keep 2-4 characters if possible"));
        if (label === "Publisher")
            return root._tagLabel(qsTr("Publisher"), qsTr("Pub", "Short metadata label for Publisher; keep 2-4 characters if possible"));
        if (label === "Rating")
            return root._tagLabel(qsTr("Rating"), qsTr("Rtg", "Short metadata label for Rating; keep 2-4 characters if possible"));
        if (label === "Category")
            return root._tagLabel(qsTr("Category"), qsTr("Cat", "Short metadata label for Category; keep 2-4 characters if possible"));
        if (label === "Release date")
            return root._tagLabel(qsTr("Release date"), qsTr("Date", "Short metadata label for Release date; keep 2-4 characters if possible"));
        if (label === "Manufacturer")
            return root._tagLabel(qsTr("Manufacturer"), qsTr("Mfr", "Short metadata label for Manufacturer; keep 2-4 characters if possible"));
        return {
            "label": label,
            "measureLabel": label
        };
    }

    function _parseDetailTags(tags: string): var {
        if (tags === "")
            return [];
        return tags.split("\n").map(row => {
            const parts = row.split("\t");
            const rawLabel = parts.length > 0 ? parts[0] : "";
            const label = root._localizedTagLabel(rawLabel);
            return {
                "rawLabel": rawLabel,
                "label": label.label,
                "measureLabel": label.measureLabel,
                "value": parts.length > 1 ? parts[1] : ""
            };
        });
    }

    Rectangle {
        anchors.fill: parent
        color: Theme.surfaceCard
        border.width: Sizing.stroke(1)
        border.color: Theme.borderMid
        radius: root._cardRadius
        visible: root.showChrome
    }

    Item {
        id: content

        anchors.fill: parent
        anchors.leftMargin: root._panePaddingLeft
        anchors.rightMargin: root._panePaddingRight
        anchors.topMargin: root._panePaddingTop
        anchors.bottomMargin: root._panePaddingBottom
        clip: true

        readonly property int primarySpan: root._horizontalSections ? Math.floor((width - root._sectionGap) * root._imageShare / root._shareTotal) : Math.floor((height - root._sectionGap) * root._imageShare / root._shareTotal)
        readonly property int secondarySpan: root._horizontalSections ? Math.max(0, width - primarySpan - root._sectionGap) : Math.max(0, height - imageSlotHeight - root._sectionGap)
        readonly property int imageSlotX: root._horizontalSections ? root._carouselGutter : root._imagePaddingLeft + root._carouselGutter
        readonly property int imageSlotY: 0
        readonly property int imageSlotWidth: {
            if (root._horizontalSections)
                return Math.max(0, primarySpan - (2 * root._carouselGutter));
            const availableWidth = Math.max(0, width - (2 * root._carouselGutter) - root._imagePaddingLeft - root._imagePaddingRight);
            const maxWidth = Math.max(0, width - root._imagePaddingLeft - root._imagePaddingRight);
            return Math.max(0, Math.min(maxWidth, availableWidth + root._imageReservedWidth));
        }
        readonly property int imageSlotHeight: {
            if (root._horizontalSections)
                return height;
            const availableWidth = Math.max(0, width - (2 * root._carouselGutter) - root._imagePaddingLeft - root._imagePaddingRight);
            // Title visible: use the fixed ratio from the profile.
            // Title not visible (media screens): use the share-based primarySpan
            // so the cover footprint is stable from the first frame regardless of
            // whether metadata tags have loaded yet. _compactMetadataHeight
            // is metadata-driven and would cause a reflow on every move.
            const imageLimit = titleText.visible ? Math.floor((height * root._imageHeightRatioWithTitle) / 100) : Math.max(0, primarySpan - root._imageBottomMargin);
            return Math.max(0, Math.min(height, Math.min(availableWidth, imageLimit) + root._imageReservedHeight));
        }
        readonly property int metadataX: root._horizontalSections ? primarySpan + root._sectionGap : 0
        readonly property int metadataY: root._horizontalSections ? 0 : imageSlotHeight + root._sectionGap
        readonly property int metadataWidth: root._horizontalSections ? secondarySpan : width
        readonly property int metadataHeight: root._horizontalSections ? height : secondarySpan

        Item {
            id: imageSlot

            x: content.imageSlotX
            y: content.imageSlotY
            width: content.imageSlotWidth
            height: content.imageSlotHeight
            clip: true

            Item {
                anchors.fill: parent
                anchors.leftMargin: root._horizontalSections ? root._imagePaddingLeft : 0
                anchors.rightMargin: root._horizontalSections ? root._imagePaddingRight : 0
                anchors.topMargin: root._imagePaddingTop
                anchors.bottomMargin: root._imagePaddingBottom

                // Holds the previously decoded cover while the new one async-decodes.
                // Prevents the slot from blanking during the brief Qt pixmap-decode
                // window (typically < 150 ms for a cached JPEG). Dropped when the
                // grace elapses without resolution so a genuinely cold cover shows a
                // clean hourglass instead of a stale image persisting forever.
                Image {
                    id: coverHold

                    objectName: "detailCoverHold"
                    anchors.fill: parent
                    source: root._lastGoodCoverSource
                    fillMode: Image.PreserveAspectFit
                    sourceSize.width: Sizing.detailCoverSourceWidth
                    smooth: true
                    asynchronous: false
                    cache: true
                    visible: root._lastGoodCoverSource !== "" && root._lastGoodCoverSource !== root._coverSource && cover.status !== Image.Ready && !root.detailSuppressed && !root._isSystemCover && !root._coverBusyIndicatorVisible
                }

                Image {
                    id: cover

                    objectName: "detailCoverImage"
                    anchors.fill: parent
                    source: root._coverSource
                    fillMode: Image.PreserveAspectFit
                    sourceSize.width: Sizing.detailCoverSourceWidth
                    smooth: true
                    asynchronous: true
                    visible: root._coverSource !== "" && status === Image.Ready && !root.detailSuppressed
                    // Record the decoded cover URL so coverHold can display it
                    // while the next cover async-decodes after a d-pad move.
                    onStatusChanged: {
                        if (status === Image.Ready)
                            root._lastGoodCoverSource = source;
                    }
                }

                Image {
                    id: placeholderIcon

                    objectName: "detailPlaceholderIcon"
                    x: Sizing.center(parent.width, width)
                    y: Sizing.center(parent.height, height)
                    // Size the chip to ~50% of the cover-slot width so it reads
                    // as a modest accent rather than a large placeholder icon.
                    width: Math.round(parent.width * 0.5)
                    height: width
                    source: root._coverBusy ? Resources.iconUrl("Loading") : Resources.coverUrl("icons/File", Theme.logoFocusPrimary, Theme.logoFocusSecondary, Theme.logoFocusShadow)
                    sourceSize.width: Sizing.px(width)
                    sourceSize.height: Sizing.px(height)
                    fillMode: Image.PreserveAspectFit
                    smooth: true
                    asynchronous: false
                    visible: !root.detailSuppressed && !root._isSystemCover && (root._coverBusyIndicatorVisible || (!root._coverBusy && (root._coverSource === "" || cover.status === Image.Error)))
                }

                // Wordmark fallback for system entries with no curated logo SVG.
                // Mirrors the grid Tile's fitted-text treatment: DemiBold, logo-focus
                // tint, shrinks to fill. Hidden while a logo is loading so the busy
                // window is brief. The File chip above is suppressed for system keys
                // (via !_isSystemCover) so exactly one of the two placeholders shows.
                Text {
                    objectName: "detailLogoWordmark"

                    anchors.fill: parent
                    anchors.margins: Sizing.pctH(1)
                    text: root.title
                    font.family: Theme.fontUi
                    font.pixelSize: Sizing.fontSize(5.8)
                    fontSizeMode: Text.Fit
                    minimumPixelSize: Sizing.fontSize(2.8)
                    font.weight: Font.DemiBold
                    color: Theme.logoFocusPrimary
                    wrapMode: Text.Wrap
                    horizontalAlignment: Text.AlignHCenter
                    verticalAlignment: Text.AlignVCenter
                    renderType: Text.NativeRendering
                    visible: root._isSystemCover && !root._coverBusy && cover.status !== Image.Ready && root.title !== "" && !root.detailSuppressed
                    clip: true
                }
            }
        }

        Image {
            source: Resources.iconUrl("NavLeft")
            width: Sizing.pctH(4)
            height: width
            anchors.left: parent.left
            anchors.verticalCenter: imageSlot.verticalCenter
            fillMode: Image.PreserveAspectFit
            smooth: true
            visible: root._detailVisible && root.canPreviousImage
        }

        Image {
            source: Resources.iconUrl("NavRight")
            width: Sizing.pctH(4)
            height: width
            anchors.right: parent.right
            anchors.verticalCenter: imageSlot.verticalCenter
            fillMode: Image.PreserveAspectFit
            smooth: true
            visible: root._detailVisible && root.canNextImage
        }

        Item {
            id: metadataSlot

            x: content.metadataX
            y: content.metadataY
            width: content.metadataWidth
            height: content.metadataHeight
            clip: true

            Item {
                id: metadataInner

                anchors.fill: parent
                anchors.leftMargin: root._horizontalSections ? root._metadataPaddingLeft : root._metadataLeftMargin
                anchors.rightMargin: root._horizontalSections ? root._metadataPaddingRight : root._metadataRightMargin
                anchors.topMargin: root._horizontalSections ? root._metadataPaddingTop : 0
                anchors.bottomMargin: root._horizontalSections ? root._metadataPaddingBottom : 0
                clip: true

                Text {
                    id: titleText

                    objectName: "detailTitleText"
                    anchors.left: parent.left
                    anchors.right: parent.right
                    anchors.top: parent.top
                    text: root.title
                    color: Theme.textPrimary
                    font.family: Theme.fontUi
                    font.pixelSize: Sizing.fontSize(3.2)
                    wrapMode: Text.Wrap
                    maximumLineCount: 3
                    elide: Text.ElideRight
                    horizontalAlignment: Text.AlignLeft
                    renderType: Text.NativeRendering
                    visible: root._detailVisible && root.showTitle && root.title !== ""
                }

                Item {
                    id: detailBody

                    readonly property int _bodyTopOffset: titleText.visible ? titleText.y + titleText.height + root._titleBottomMargin : 0
                    readonly property bool _bottomAlignedCompactMetadata: !titleText.visible && !root._horizontalSections && root._metadataBottomAligned

                    x: 0
                    y: _bodyTopOffset + (_bottomAlignedCompactMetadata ? 0 : root._metadataTopMargin)
                    width: parent.width
                    height: {
                        if (root._horizontalSections)
                            return Math.max(0, parent.height - y);
                        if (titleText.visible)
                            return Math.max(0, parent.height - y + root._metadataHeightAdjustment);
                        if (_bottomAlignedCompactMetadata)
                            return Math.max(0, Math.min(root._compactMetadataHeight + root._metadataTopMargin, parent.height));
                        return Math.max(0, Math.min(root._compactMetadataHeight, parent.height - y));
                    }
                    clip: true

                    Column {
                        id: tagTable

                        objectName: "detailTagTable"
                        anchors.left: parent.left
                        anchors.right: parent.right
                        anchors.top: root._metadataBottomAligned && !titleText.visible ? undefined : parent.top
                        anchors.bottom: root._metadataBottomAligned && !titleText.visible ? parent.bottom : undefined
                        spacing: root._tagRowSpacing
                        clip: true
                        visible: root._detailVisible && root._displayRows.length > 0

                        Repeater {
                            model: root._displayRows

                            delegate: Item {
                                id: tagRow

                                required property var modelData

                                width: tagTable.width
                                height: root._tagRowHeight

                                readonly property string label: modelData.label ?? ""
                                readonly property string measureLabel: modelData.measureLabel ?? tagRow.label
                                readonly property string value: modelData.value ?? ""

                                TextMetrics {
                                    id: labelMetrics

                                    text: tagRow.measureLabel
                                    font.family: Theme.fontUi
                                    font.pixelSize: root._tagTextSize
                                    onAdvanceWidthChanged: root._labelColumnNaturalWidth = Math.max(root._labelColumnNaturalWidth, Math.ceil(advanceWidth))
                                }

                                Component.onCompleted: root._labelColumnNaturalWidth = Math.max(root._labelColumnNaturalWidth, Math.ceil(labelMetrics.advanceWidth))

                                Text {
                                    anchors.left: parent.left
                                    anchors.top: parent.top
                                    width: root._labelColumnWidth
                                    text: tagRow.label
                                    color: Theme.textLabel
                                    font.family: Theme.fontUi
                                    font.pixelSize: root._tagTextSize
                                    textFormat: Text.PlainText
                                    elide: Text.ElideRight
                                    horizontalAlignment: Text.AlignRight
                                    renderType: Text.NativeRendering
                                }

                                Text {
                                    anchors.left: parent.left
                                    anchors.leftMargin: root._labelColumnWidth + root._tagLabelGap
                                    anchors.right: parent.right
                                    anchors.top: parent.top
                                    text: tagRow.value
                                    color: Theme.textPrimary
                                    font.family: Theme.fontUi
                                    font.pixelSize: root._tagTextSize
                                    wrapMode: Text.NoWrap
                                    maximumLineCount: 1
                                    elide: Text.ElideRight
                                    horizontalAlignment: Text.AlignLeft
                                    renderType: Text.NativeRendering
                                }
                            }
                        }
                    }
                }
            }
        }

        LoadingIndicator {
            objectName: "detailLoadingIndicator"
            visible: root._emptyPaneLoading && !root.detailSuppressed
            x: Sizing.center(parent.width, width)
            y: Sizing.center(parent.height, height)
            text: root.loadingText
        }
    }
}
