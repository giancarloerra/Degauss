// Zaparoo Frontend
// Copyright (c) 2026 Wizzo Pty Ltd and the Zaparoo Project contributors.
// SPDX-License-Identifier: LicenseRef-PolyForm-Noncommercial-1.0.0
pragma ComponentBehavior: Bound

import QtQuick
import Zaparoo.Theme
import Zaparoo.Ui
import Zaparoo.Browse as Browse

// cxx-qt 0.8 patches `isFinal: true` on singleton properties but the
// qmltypes schema has no `isFinal` slot, so reads of `Browse.BuildInfo`
// fields trip qmllint's "Member can be shadowed" check. Suppress just
// the compiler category file-wide; matches the pattern used in
// CommercialNoticeModal.qml.
// qmllint disable compiler

// About / License screen — static, scrollable info page reachable from
// Settings → About / License. Pure input dispatcher: emits
// `requestSettingsScreen()` on cancel; Up/Down scroll the Flickable.
//
// Build provenance (git commit, build channel, official-build marker)
// will plug into the version line in a follow-up round; for now only
// the hardcoded `Qt.application.version` is shown.
Item {
    id: about

    Component.onCompleted: console.debug("startup/qml component AboutScreen completed")

    // Bound by MainLayout to `root.pendingTransition !== ""`. About is
    // a destination, never a source — kept for parity with the other
    // screens.
    property bool transitioning: false

    signal requestSettingsScreen

    // True when the body Column overflows the Flickable viewport, so
    // the help bar can show the Up/Down scroll cue only when it's
    // actually meaningful. Per the minimal-help-bar policy, hints
    // shouldn't promise a press that no-ops.
    readonly property bool contentOverflows: body.implicitHeight > flickable.height

    // Drive the top/bottom scroll chevrons. The 1-px epsilon swallows
    // sub-pixel rounding so the chevrons don't flicker on exact-fit
    // content.
    readonly property bool _hasContentAbove: flickable.contentY > 1
    readonly property bool _hasContentBelow: flickable.contentY + flickable.height < flickable.contentHeight - 1

    function _scrollBy(delta: int): void {
        const maxY = Math.max(0, flickable.contentHeight - flickable.height);
        flickable.contentY = Math.max(0, Math.min(maxY, flickable.contentY + delta));
    }

    function handleAction(action: string): void {
        if (action === "up")
            about._scrollBy(-Sizing.pctH(8));
        else if (action === "down")
            about._scrollBy(Sizing.pctH(8));
        else if (action === "cancel")
            about.requestSettingsScreen();
    // accept and left/right are no-ops on a static page.
    }

    // ── Visual tree ───────────────────────────────────────────────────────────

    TopStatusStrip {
        id: topStrip
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: parent.top
        anchors.topMargin: Sizing.headerBottom
        height: Sizing.pctH(7)
        title: qsTr("About / License")
        currentPage: 0
        totalPages: 0
        totalText: ""
    }

    // Body lives in a Flickable so the static content can grow past a
    // single screen on MiSTer 240p without dropping off-frame. Width
    // is capped tighter than Settings's pctW(70) — prose reads better
    // at narrow line lengths, and the cap also keeps the logo from
    // having to scale up past its 600px native width on widescreen.
    // Bottom margin clears the help bar (pctH(6)) plus a small gap.
    //
    // Card stays put; the Flickable sits inside the card and content
    // scrolls within it. Putting the Flickable outside the card would
    // scroll the card itself, which reads wrong. Internal padding
    // matches the Settings row recipe (pctW(3) / pctH(3)) so the body
    // text doesn't kiss the card edge.
    Rectangle {
        id: card

        anchors.top: topStrip.bottom
        anchors.topMargin: Sizing.pctH(2)
        anchors.bottom: parent.bottom
        anchors.bottomMargin: Sizing.pctH(8)
        anchors.horizontalCenter: parent.horizontalCenter
        width: Math.min(parent.width - Sizing.pctW(10), Sizing.pctW(50))
        color: Theme.surfaceCard
        radius: Sizing.cornerRadius
        border.color: Theme.borderMid
        border.width: Sizing.stroke(1)

        Flickable {
            id: flickable

            // top/bottomMargin is sized to leave a clear band inside
            // the card for the scroll chevrons to sit outside the
            // scrollable area (chevron pctH(3) + breathing room).
            anchors.fill: parent
            anchors.leftMargin: Sizing.pctW(3)
            anchors.rightMargin: Sizing.pctW(3)
            anchors.topMargin: Sizing.pctH(4)
            anchors.bottomMargin: Sizing.pctH(4)
            contentWidth: width
            contentHeight: body.implicitHeight
            clip: true
            boundsBehavior: Flickable.StopAtBounds

            Column {
                id: body

                width: parent.width
                spacing: Sizing.pctH(2)

                // Leading spacer — keeps the logo clear of the top
                // scroll chevron when the page overflows, and gives
                // the cut-off edge a breath of whitespace instead of
                // clipping the logo mid-stroke.
                Item {
                    width: body.width
                    height: Sizing.pctH(2)
                }

                // Logo width is capped at a screen-height-relative size so
                // the brand mark stays a header element across 240p →
                // 1080p without ballooning. sourceSize is pinned to the
                // native pixel dimensions to stop Qt upscaling then
                // downsampling and to keep the lines crisp; height is
                // derived from width via the image's intrinsic aspect.
                Image {
                    anchors.horizontalCenter: parent.horizontalCenter
                    source: "qrc:/qt/qml/Zaparoo/App/resources/images/logo.png"
                    fillMode: Image.PreserveAspectFit
                    sourceSize.width: 600
                    sourceSize.height: 150
                    width: Math.min(parent.width, Sizing.pctH(35))
                    height: Sizing.px(width * 150 / 600)
                }

                Text {
                    anchors.horizontalCenter: parent.horizontalCenter
                    horizontalAlignment: Text.AlignHCenter
                    text: qsTr("Degauss")
                    color: Theme.textPrimary
                    font.family: Theme.fontUi
                    font.pixelSize: Sizing.fontSize(4)
                    font.weight: Font.Medium
                    renderType: Text.NativeRendering
                }

                Text {
                    anchors.horizontalCenter: parent.horizontalCenter
                    horizontalAlignment: Text.AlignHCenter
                    text: qsTr("An independent fork of Zaparoo Frontend, for Zaparoo Core.")
                    color: Theme.textLabel
                    font.family: Theme.fontUi
                    font.pixelSize: Sizing.fontSize(2.4)
                    renderType: Text.NativeRendering
                }

                Text {
                    anchors.horizontalCenter: parent.horizontalCenter
                    horizontalAlignment: Text.AlignHCenter
                    text: qsTr("Version %1 · %2 · %3").arg(Qt.application.version).arg(Browse.BuildInfo.commit).arg(Browse.BuildInfo.channel)
                    color: Theme.textLabel
                    font.family: Theme.fontUi
                    font.pixelSize: Sizing.fontSize(2.4)
                    renderType: Text.NativeRendering
                }

                Text {
                    anchors.horizontalCenter: parent.horizontalCenter
                    horizontalAlignment: Text.AlignHCenter
                    text: qsTr("Built %1").arg(Browse.BuildInfo.build_date)
                    color: Theme.textLabel
                    font.family: Theme.fontUi
                    font.pixelSize: Sizing.fontSize(2.2)
                    renderType: Text.NativeRendering
                }

                Text {
                    width: parent.width
                    horizontalAlignment: Text.AlignHCenter
                    wrapMode: Text.Wrap
                    text: qsTr("Copyright 2026 Wizzo Pty Ltd and the Zaparoo Project contributors.")
                    color: Theme.textPrimary
                    font.family: Theme.fontUi
                    font.pixelSize: Sizing.fontSize(2.6)
                    renderType: Text.NativeRendering
                }

                Text {
                    width: parent.width
                    horizontalAlignment: Text.AlignHCenter
                    wrapMode: Text.Wrap
                    text: qsTr("Source available under the PolyForm Noncommercial License 1.0.0. Free for personal, non-commercial use. Commercial use requires a separate license.")
                    color: Theme.textPrimary
                    font.family: Theme.fontUi
                    font.pixelSize: Sizing.fontSize(2.6)
                    renderType: Text.NativeRendering
                }

                Text {
                    width: parent.width
                    horizontalAlignment: Text.AlignHCenter
                    wrapMode: Text.Wrap
                    text: qsTr("Commercial licensing: legal@zaparoo.org")
                    color: Theme.textPrimary
                    font.family: Theme.fontUi
                    font.pixelSize: Sizing.fontSize(2.6)
                    renderType: Text.NativeRendering
                }

                Text {
                    width: parent.width
                    horizontalAlignment: Text.AlignHCenter
                    wrapMode: Text.Wrap
                    text: qsTr("Project: https://zaparoo.org")
                    color: Theme.textPrimary
                    font.family: Theme.fontUi
                    font.pixelSize: Sizing.fontSize(2.6)
                    renderType: Text.NativeRendering
                }

                Text {
                    anchors.horizontalCenter: parent.horizontalCenter
                    horizontalAlignment: Text.AlignHCenter
                    text: qsTr("Created by")
                    color: Theme.textLabel
                    font.family: Theme.fontUi
                    font.pixelSize: Sizing.fontSize(2.4)
                    renderType: Text.NativeRendering
                }

                // Contributor names are not translated — they're proper
                // names. Joined with newlines (not separate Text items)
                // so the block reads as one credits paragraph and the
                // Column spacing doesn't push them apart.
                Text {
                    anchors.horizontalCenter: parent.horizontalCenter
                    horizontalAlignment: Text.AlignHCenter
                    text: "Andrea Bogazzi\nBossRighteous\nTim Wilsie\nWizzo"
                    color: Theme.textPrimary
                    font.family: Theme.fontUi
                    font.pixelSize: Sizing.fontSize(2.6)
                    renderType: Text.NativeRendering
                }

                Text {
                    anchors.horizontalCenter: parent.horizontalCenter
                    horizontalAlignment: Text.AlignHCenter
                    text: qsTr("Translations")
                    color: Theme.textLabel
                    font.family: Theme.fontUi
                    font.pixelSize: Sizing.fontSize(2.4)
                    renderType: Text.NativeRendering
                }

                // Translator names are proper names; the full block stays
                // one text item so translators can localize language labels
                // without changing the credits layout.
                Text {
                    anchors.horizontalCenter: parent.horizontalCenter
                    horizontalAlignment: Text.AlignHCenter
                    text: qsTr("Italiano - Andrea Bogazzi\nEspañol - Carlos R.\nEuskara - devilschile2")
                    color: Theme.textPrimary
                    font.family: Theme.fontUi
                    font.pixelSize: Sizing.fontSize(2.6)
                    renderType: Text.NativeRendering
                }

                Text {
                    width: parent.width
                    horizontalAlignment: Text.AlignHCenter
                    wrapMode: Text.Wrap
                    text: qsTr("Full license text in COPYING.")
                    color: Theme.textLabel
                    font.family: Theme.fontUi
                    font.pixelSize: Sizing.fontSize(2.4)
                    renderType: Text.NativeRendering
                }

                // Trailing spacer — symmetric with the leading spacer.
                Item {
                    width: body.width
                    height: Sizing.pctH(2)
                }
            }
        }

        // Top/bottom scroll chevrons — mirror the PagedGrid/BrowseList
        // recipe (same SVG icons, `PreserveAspectFit` + `smooth: true`)
        // but centered on the viewport in the card's chrome gap *above*
        // and *below* the Flickable, not inside its visible band.
        // Sitting outside the scrolled area means the chevrons never
        // overlap moving content as the user scrolls.
        Image {
            source: Resources.iconUrl("ScrollUp")
            width: Sizing.pctH(3)
            height: width
            anchors.bottom: flickable.top
            anchors.bottomMargin: Sizing.pctH(0.5)
            anchors.horizontalCenter: flickable.horizontalCenter
            fillMode: Image.PreserveAspectFit
            smooth: true
            visible: about._hasContentAbove
        }

        Image {
            source: Resources.iconUrl("ScrollDown")
            width: Sizing.pctH(3)
            height: width
            anchors.top: flickable.bottom
            anchors.topMargin: Sizing.pctH(0.5)
            anchors.horizontalCenter: flickable.horizontalCenter
            fillMode: Image.PreserveAspectFit
            smooth: true
            visible: about._hasContentBelow
        }
    }
}
