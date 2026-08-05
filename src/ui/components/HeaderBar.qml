// Zaparoo Frontend
// Copyright (c) 2026 Wizzo Pty Ltd and the Zaparoo Project contributors.
// SPDX-License-Identifier: LicenseRef-PolyForm-Noncommercial-1.0.0
// cxx-qt 0.8 patches `isFinal: true` on singleton properties but the
// qmltypes schema has no `isFinal` slot, so every read of a Browse
// singleton trips qmllint's "Member can be shadowed" check. Suppress
// the compiler category file-wide until the schema grows the slot.
// qmllint disable compiler
// Top header bar — Degauss logo on the left, host status row + Core
// status pill stacked on the right. Height is fixed at
// `Sizing.headerHeight` so the pill's slot is reserved even when the
// pill is idle and the logo can match the two stacked rows exactly.

import QtQuick
import Zaparoo.Browse as Browse
import Zaparoo.Theme

// Software-renderer safe: only Image, Row, Item, Text, and the
// existing CoreStatusPill subtree. No transforms, no shaders.
Item {
    id: header

    Component.onCompleted: console.debug("startup/qml component HeaderBar completed")

    // Exposed for the screensaver overlay so it can read the logo's
    // on-screen geometry (mapToItem + paintedWidth/Height) and start
    // the bouncing copy at exactly the same position.
    property alias logoItem: logo
    property var layoutProfile: null
    readonly property var _headerProfile: header.layoutProfile && header.layoutProfile.header ? header.layoutProfile.header : null
    property string browseTitle: ""
    property string browseProgressText: ""
    property bool statusIconsEnabled: false
    property bool mediaActivityEnabled: false

    height: Sizing.headerHeight

    Image {
        id: logo

        anchors.left: parent.left
        anchors.leftMargin: Sizing.headerSideMargin
        anchors.top: parent.top
        anchors.bottom: parent.bottom
        // PreserveAspectFit caps width by the logo's intrinsic aspect,
        // so the brand mark never stretches even though the Image
        // element fills the full header height.
        fillMode: Image.PreserveAspectFit
        horizontalAlignment: Image.AlignLeft
        source: "qrc:/qt/qml/Zaparoo/App/resources/images/logo.png"
    }

    Text {
        id: browseProgressLabel

        visible: header.browseProgressText !== ""
        anchors.left: logo.right
        anchors.leftMargin: Sizing.pctW(1)
        anchors.verticalCenter: logo.verticalCenter
        width: Math.max(0, Math.floor(parent.width / 4))
        height: Sizing.headerRowHeight
        elide: Text.ElideRight
        horizontalAlignment: Text.AlignLeft
        verticalAlignment: Text.AlignVCenter
        text: header.browseProgressText
        font.family: Theme.fontUi
        font.pixelSize: Sizing.fontSize(2.9)
        color: Theme.textPrimary
        renderType: Text.NativeRendering
    }

    function _clockLocale(): var {
        const language = Browse.Settings.current_language;
        if (language === "" || language === "auto")
            return Qt.locale();
        return Qt.locale(language);
    }

    function _clockUses12Hour(): bool {
        const format = Browse.Settings.current_clock_format;
        if (format === "12h")
            return true;
        if (format === "24h")
            return false;
        const localeFormat = header._clockLocale().timeFormat(Locale.ShortFormat);
        if (localeFormat.indexOf("H") >= 0)
            return false;
        return localeFormat.indexOf("h") >= 0;
    }

    function _clockFormatString(): string {
        return header._clockUses12Hour() ? "h:mm AP" : "HH:mm";
    }

    function _clockText(date: date): string {
        return date.toLocaleTimeString(header._clockLocale(), header._clockFormatString());
    }

    function _clockMetricSample(): string {
        const sample = header._clockUses12Hour() ? new Date(2000, 0, 1, 12, 59) : new Date(2000, 0, 1, 23, 59);
        return header._clockText(sample);
    }

    TextMetrics {
        id: clockMetrics

        text: header._clockMetricSample()
        font.family: Theme.fontUi
        font.pixelSize: Sizing.headerRowHeight
    }

    // Host status row — NFC / Wi-Fi / LAN / Bluetooth icons plus the
    // wall clock, right-anchored so badges can appear and disappear
    // without nudging the clock away from the edge. Clock width is
    // measured from the widest sample for the selected 12h/24h format.
    Row {
        id: topHud

        anchors.top: header._headerProfile && header._headerProfile.hudBottomAligned ? undefined : parent.top
        anchors.bottom: header._headerProfile && header._headerProfile.hudBottomAligned ? parent.bottom : undefined
        anchors.right: parent.right
        anchors.rightMargin: Sizing.headerSideMargin
        spacing: Sizing.pctW(1)
        // Explicit row height keeps every child on a single line. Without
        // this the Row would track the tallest child (clock Text — font
        // ascender + descender), which pushes icons up and out of
        // alignment with the clock baseline.
        height: Sizing.headerRowHeight

        StatusIcon {
            anchors.verticalCenter: parent.verticalCenter
            visible: header.statusIconsEnabled && Browse.SystemStatus.has_nfc
            source: Resources.statusIconUrl("NFC")
            name: "NFC"
        }

        StatusIcon {
            anchors.verticalCenter: parent.verticalCenter
            visible: header.statusIconsEnabled && Browse.SystemStatus.has_wifi_internet
            source: Resources.statusIconUrl("WiFi")
            name: "Wi-Fi"
        }

        StatusIcon {
            anchors.verticalCenter: parent.verticalCenter
            visible: header.statusIconsEnabled && Browse.SystemStatus.has_lan_internet
            source: Resources.statusIconUrl("WiredNetwork")
            name: "LAN"
        }

        StatusIcon {
            anchors.verticalCenter: parent.verticalCenter
            visible: header.statusIconsEnabled && Browse.SystemStatus.has_bluetooth
            source: Resources.statusIconUrl("Bluetooth")
            name: "Bluetooth"
        }

        Text {
            id: clockLabel

            // 30s tick keeps the displayed minute fresh without per-second
            // wakeups; minutes-only display means we never need finer.
            // Fixed width avoids reflow on the minute boundary because
            // proportional digits make "11:11" narrower than "10:00".
            property date currentDate: new Date()
            readonly property string currentTime: header._clockText(clockLabel.currentDate)

            anchors.verticalCenter: parent.verticalCenter
            height: parent.height
            width: Sizing.px(clockMetrics.advanceWidth)
            verticalAlignment: Text.AlignVCenter
            horizontalAlignment: Text.AlignRight
            text: clockLabel.currentTime
            font.family: Theme.fontUi
            font.pixelSize: Sizing.headerRowHeight
            color: Theme.textPrimary
            renderType: Text.NativeRendering

            Timer {
                interval: 30000
                running: true
                repeat: true
                triggeredOnStart: true
                onTriggered: clockLabel.currentDate = new Date()
            }
        }
    }

    TextMetrics {
        id: crtTitleMetrics

        text: header.browseTitle
        font.family: Theme.fontUi
        font.pixelSize: Sizing.headerRowHeight
        font.weight: Font.Medium
    }

    Text {
        id: crtTitleLabel

        visible: header._headerProfile && header._headerProfile.titleInHeader && header.browseTitle !== ""
        x: Sizing.center(parent.width, width)
        y: parent.height - height
        width: Math.min(Math.floor(parent.width / 3), Math.ceil(Math.max(crtTitleMetrics.advanceWidth, crtTitleMetrics.boundingRect.width)))
        height: Sizing.headerRowHeight
        elide: Text.ElideRight
        horizontalAlignment: Text.AlignLeft
        verticalAlignment: Text.AlignVCenter
        text: header.browseTitle
        font.family: Theme.fontUi
        font.pixelSize: Sizing.headerRowHeight
        font.weight: Font.Medium
        color: Theme.textPrimary
        renderType: Text.NativeRendering
    }

    // Mutually-exclusive Core / indexing / scraper status surface. Sits
    // on its own line directly under `topHud`, right-aligned to the
    // same edge as the clock. The pill collapses to zero size when
    // idle, but its slot stays reserved by the header's fixed height
    // so the logo and the surrounding layout don't shift.
    CoreStatusPill {
        anchors.top: header._headerProfile && header._headerProfile.statusPillPinnedTop ? parent.top : topHud.bottom
        anchors.right: topHud.right
        anchors.topMargin: header._headerProfile && header._headerProfile.statusPillPinnedTop ? 0 : Sizing.headerStackGap
        mediaActivityEnabled: header.mediaActivityEnabled
    }
}
