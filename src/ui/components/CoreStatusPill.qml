// Zaparoo Frontend
// Copyright (c) 2026 Wizzo Pty Ltd and the Zaparoo Project contributors.
// SPDX-License-Identifier: LicenseRef-PolyForm-Noncommercial-1.0.0
// cxx-qt 0.8 patches `isFinal: true` on singleton properties but the
// qmltypes schema has no `isFinal` slot, so every read trips qmllint's
// "Member can be shadowed" check. Until the schema grows the slot,
// suppress the compiler category file-wide.
// qmllint disable compiler
pragma ComponentBehavior: Bound
// Single mutually-exclusive status pill anchored in the top-right HUD.
// Surfaces — in priority order, only one ever paints — Core link state
// (disconnected / connecting / error), media-database build progress
// (indexing / optimizing / paused), and scraper progress.
// When no condition is interesting the pill is `visible: false` and
// `width: 0`, so the surrounding Row collapses around it and the rest
// of the HUD slides flush to the right edge.

import QtQuick
import Zaparoo.Browse as Browse
import Zaparoo.Theme

// Software-rendering safe status pill. Connection states keep the compact
// text-only treatment; active media work switches to a fixed-width progress
// pill with a tiny local spinner and clipped inverted foreground over the
// fill. Only this small item repaints while the spinner advances.
Item {
    id: pill
    property bool mediaActivityEnabled: false

    Component.onCompleted: console.debug("startup/qml component CoreStatusPill completed")

    // Link-state constants mirror rust/frontend/src/models/app_status.rs:
    //   0 DISCONNECTED · 1 CONNECTING · 2 CONNECTED · 3 RECONNECTING · 4 UNREACHABLE.
    // Catalog connection_state constants:
    //   0 DISCONNECTED · 1 CONNECTING · 2 READY · 3 ERROR.
    readonly property int _linkDisconnected: 0
    readonly property int _linkConnecting: 1
    readonly property int _linkConnected: 2
    readonly property int _linkReconnecting: 3
    readonly property int _linkUnreachable: 4
    readonly property int _connError: 3
    readonly property string _connectionLabel: {
        const link = Browse.AppStatus.link_state ?? pill._linkDisconnected;
        const conn = Browse.AppStatus.connection_state ?? pill._linkDisconnected;

        if (link === pill._linkUnreachable)
            return qsTr("Disconnected");

        if (link === pill._linkReconnecting)
            return qsTr("Reconnecting…");

        if (link === pill._linkDisconnected)
            return qsTr("Disconnected");

        if (link === pill._linkConnecting)
            return qsTr("Connecting…");

        if (conn === pill._connError)
            return qsTr("Core error");

        return "";
    }
    readonly property bool _isMediaActivity: pill.mediaActivityEnabled && pill._connectionLabel === "" && (Browse.MediaStatus.optimizing || Browse.MediaStatus.indexing || Browse.MediaStatus.scraping)
    readonly property bool _mediaPaused: Browse.MediaStatus.indexing ? Browse.MediaStatus.paused : (Browse.MediaStatus.scraping ? Browse.MediaStatus.scrape_paused : false)
    readonly property int _progressCurrent: Browse.MediaStatus.indexing ? Browse.MediaStatus.current_step : (Browse.MediaStatus.scraping ? Browse.MediaStatus.scrape_current_step : 0)
    readonly property int _progressTotal: Browse.MediaStatus.indexing ? Browse.MediaStatus.total_steps : (Browse.MediaStatus.scraping ? Browse.MediaStatus.scrape_total_steps : 0)
    readonly property real _progressFraction: pill._progressTotal > 0 ? Math.max(0, Math.min(1, pill._progressCurrent / pill._progressTotal)) : 0
    readonly property bool _spinnerActive: pill._isMediaActivity && !pill._mediaPaused
    readonly property string _mediaLabel: {
        const cur = pill._progressCurrent;
        const tot = pill._progressTotal;
        const known = tot > 0;
        const compact = Theme.crtNativePath;

        if (pill._mediaPaused)
            return known ? qsTr("Paused %1/%2").arg(cur).arg(tot) : qsTr("Paused");

        if (Browse.MediaStatus.optimizing)
            return compact ? qsTr("Opt…") : qsTr("Optimizing");

        if (Browse.MediaStatus.indexing) {
            if (known)
                return compact ? qsTr("Idx %1/%2").arg(cur).arg(tot) : qsTr("Indexing %1/%2").arg(cur).arg(tot);
            return compact ? qsTr("Idx…") : qsTr("Indexing…");
        }

        if (Browse.MediaStatus.scraping) {
            if (known)
                return compact ? qsTr("Scr %1/%2").arg(cur).arg(tot) : qsTr("Scraping %1/%2").arg(cur).arg(tot);
            return compact ? qsTr("Scr…") : qsTr("Scraping…");
        }

        return "";
    }
    readonly property string _label: pill._connectionLabel !== "" ? pill._connectionLabel : pill._mediaLabel
    // Border colour leans on the same convention as the old connection
    // strip: warmer accent for error-class link states, muted otherwise.
    readonly property bool _isError: Browse.AppStatus.link_state === pill._linkUnreachable || Browse.AppStatus.connection_state === pill._connError
    readonly property int _mediaWidth: Theme.crtNativePath ? Sizing.pctH(42) : Math.min(Math.max(Sizing.pctH(28), Sizing.pctW(18)), Sizing.pctH(30))
    readonly property int _textMargin: Sizing.pctW(1.2)
    readonly property int _spinnerSize: Math.max(Sizing.pctH(1.8), Sizing.fontSize(2.2))
    readonly property int _spinnerDotSize: Math.max(Sizing.stroke(2), Sizing.px(pill._spinnerSize / 3))
    readonly property int _spinnerGap: Sizing.pctW(0.8)
    property int _spinnerFrame: 0

    visible: pill._label !== ""
    height: pill.visible ? Sizing.fontSize(3.4) : 0
    width: pill.visible ? (pill._isMediaActivity ? pill._mediaWidth : Sizing.px(labelMetrics.implicitWidth + Sizing.pctW(2.4))) : 0

    function _spinnerDotX(index: int, size: int): int {
        if (index === 1)
            return size - pill._spinnerDotSize;
        if (index === 3)
            return 0;
        return Sizing.center(size, pill._spinnerDotSize);
    }

    function _spinnerDotY(index: int, size: int): int {
        if (index === 0)
            return 0;
        if (index === 2)
            return size - pill._spinnerDotSize;
        return Sizing.center(size, pill._spinnerDotSize);
    }

    // Activity wears the brand teal: the active dot over the neutral
    // pill, and the progress fill below. Inverted inactive dots match
    // the fill colour so they disappear against it.
    function _spinnerDotColor(index: int, inverted: bool): color {
        if (index === pill._spinnerFrame)
            return inverted ? Theme.bgBar : Theme.stateMarker;
        return inverted ? Theme.stateMarker : Theme.borderMid;
    }

    Timer {
        interval: 140
        running: pill._spinnerActive && pill.visible
        repeat: true
        onTriggered: pill._spinnerFrame = (pill._spinnerFrame + 1) % 4
    }

    Text {
        id: labelMetrics
        visible: false
        text: pill._label
        font.family: Theme.fontUi
        font.pixelSize: Sizing.fontSize(2.2)
    }

    Rectangle {
        anchors.fill: parent
        color: Theme.surfaceCard
        radius: Sizing.half(pill.height)
    }

    Item {
        id: fillClip

        anchors.left: parent.left
        anchors.top: parent.top
        anchors.bottom: parent.bottom
        width: pill._isMediaActivity ? Sizing.px(pill.width * pill._progressFraction) : 0
        clip: true
        visible: width > 0

        Rectangle {
            width: pill.width
            height: pill.height
            // Progress is activity, so it fills in the brand teal;
            // yellow stays reserved for selection.
            color: Theme.stateMarker
            radius: Sizing.half(pill.height)
        }
    }

    Item {
        id: normalForeground

        anchors.fill: parent

        Item {
            id: normalSpinner

            visible: pill._spinnerActive
            anchors.left: parent.left
            anchors.leftMargin: pill._textMargin
            anchors.verticalCenter: parent.verticalCenter
            width: pill._spinnerSize
            height: pill._spinnerSize

            Repeater {
                model: 4
                delegate: Rectangle {
                    required property int modelData
                    width: pill._spinnerDotSize
                    height: width
                    radius: Sizing.half(width)
                    x: pill._spinnerDotX(modelData, normalSpinner.width)
                    y: pill._spinnerDotY(modelData, normalSpinner.height)
                    color: pill._spinnerDotColor(modelData, false)
                }
            }
        }

        Text {
            anchors.verticalCenter: parent.verticalCenter
            anchors.left: pill._spinnerActive ? normalSpinner.right : parent.left
            anchors.leftMargin: pill._spinnerActive ? pill._spinnerGap : pill._textMargin
            anchors.right: parent.right
            anchors.rightMargin: pill._textMargin
            elide: Text.ElideRight
            horizontalAlignment: pill._spinnerActive ? Text.AlignLeft : Text.AlignHCenter
            text: pill._label
            font.family: Theme.fontUi
            font.pixelSize: Sizing.fontSize(2.2)
            color: Theme.textPrimary
            renderType: Text.NativeRendering
        }
    }

    Item {
        id: invertedClip

        anchors.left: parent.left
        anchors.top: parent.top
        anchors.bottom: parent.bottom
        width: fillClip.width
        clip: true
        visible: fillClip.visible

        Item {
            width: pill.width
            height: pill.height

            Item {
                id: invertedSpinner

                visible: pill._spinnerActive
                anchors.left: parent.left
                anchors.leftMargin: pill._textMargin
                anchors.verticalCenter: parent.verticalCenter
                width: pill._spinnerSize
                height: pill._spinnerSize

                Repeater {
                    model: 4
                    delegate: Rectangle {
                        required property int modelData
                        width: pill._spinnerDotSize
                        height: width
                        radius: Sizing.half(width)
                        x: pill._spinnerDotX(modelData, invertedSpinner.width)
                        y: pill._spinnerDotY(modelData, invertedSpinner.height)
                        color: pill._spinnerDotColor(modelData, true)
                    }
                }
            }

            Text {
                anchors.verticalCenter: parent.verticalCenter
                anchors.left: pill._spinnerActive ? invertedSpinner.right : parent.left
                anchors.leftMargin: pill._spinnerActive ? pill._spinnerGap : pill._textMargin
                anchors.right: parent.right
                anchors.rightMargin: pill._textMargin
                elide: Text.ElideRight
                horizontalAlignment: pill._spinnerActive ? Text.AlignLeft : Text.AlignHCenter
                text: pill._label
                font.family: Theme.fontUi
                font.pixelSize: Sizing.fontSize(2.2)
                color: Theme.bgBar
                renderType: Text.NativeRendering
            }
        }
    }

    Rectangle {
        anchors.fill: parent
        color: "transparent"
        radius: Sizing.half(pill.height)
        border.width: Sizing.stroke(1)
        border.color: pill._isError ? Theme.accent : Theme.borderMid
    }
}
