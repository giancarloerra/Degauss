// Zaparoo Frontend
// Copyright (c) 2026 Wizzo Pty Ltd and the Zaparoo Project contributors.
// SPDX-License-Identifier: LicenseRef-PolyForm-Noncommercial-1.0.0

import QtQuick
import Zaparoo.Theme
import Zaparoo.Browse as Browse

// cxx-qt 0.8 patches `isFinal: true` on singleton properties but the
// qmltypes schema has no `isFinal` slot, so every read trips qmllint's
// "Member can be shadowed" check. Until the schema grows the slot,
// suppress the compiler category file-wide.
// qmllint disable compiler

// Blocking first-run notice. Shown once per install before the frontend
// becomes usable, then never again — `Browse.Notice.commercial_ack`
// persists in `frontend.toml` (not `state.toml`, which is tmpfs on
// MiSTer). Routed in front of the media-DB first-run modal so the user
// sees this prose first, then the indexing prompt.
//
// Pure presentation: input is dispatched by `Main.qml`, the only
// interactive surface is the "I understand" action, and dismiss runs
// through the `closeRequested` signal so the router owns the modal
// stack. Chrome (scrim, panel, border, radius, title) comes from the
// shared `Modal` shell so every dialog in the app reads as the same
// surface.
Item {
    id: modal

    property bool open: false

    // Push-in scale for the "I understand" button, mirroring the shared
    // Modal shell's prebaked buttons. The button renders at this scale so
    // both mouse and key/controller accept play the same press cue before
    // the modal dismisses.
    property real _pressScale: 1.0

    signal closeRequested

    visible: modal.open
    anchors.fill: parent
    z: 310

    onOpenChanged: {
        if (!modal.open) {
            // Disarm a pending deferred accept so a press-then-close inside
            // the cue window can't ack twice after dismissal.
            actionCommit.stop();
            return;
        }
        modal._pressScale = 1.0;
        pressAnim.stop();
    }

    function handleAction(action: string): void {
        if (action === "accept")
            modal._commit();
    // No cancel path. The notice is informational, not a license
    // condition — but it must be acknowledged once so the user has
    // demonstrably seen the non-commercial-use message before the
    // frontend becomes interactive.
    }

    // Play the push-in cue on the button, then ack + close deferred so the
    // animation completes before the router pops the modal.
    function _commit(): void {
        pressAnim.restart();
        actionCommit.arm();
    }

    NumberAnimation {
        id: pressAnim
        target: modal
        property: "_pressScale"
        to: Motion.pressScale
        duration: Motion.dur(Motion.pressMs)
        easing.type: Easing.OutQuad
    }

    DeferredAction {
        id: actionCommit
        onDeferred: {
            // Persist before signalling close so the next Component
            // construction (warm-restart on MiSTer) sees the ack flag
            // already written. The Rust slot is synchronous on the Qt
            // thread; if the disk write fails it logs and still flips
            // the in-memory flag for this session.
            Browse.Notice.acknowledge_commercial();
            modal.closeRequested();
        }
    }

    Modal {
        id: shell

        open: modal.open
        kind: "shell"
        title: qsTr("Welcome to Degauss")
        panelMaxWidth: Sizing.pctH(110)

        Column {
            width: parent.width
            spacing: Sizing.pctH(3)

            Text {
                width: parent.width
                text: qsTr("Copyright 2026 Wizzo Pty Ltd and the Zaparoo Project contributors.")
                font.family: Theme.fontUi
                font.pixelSize: Sizing.fontSize(2.6)
                color: Theme.textPrimary
                wrapMode: Text.WordWrap
                horizontalAlignment: Text.AlignHCenter
                renderType: Text.NativeRendering
            }

            Text {
                width: parent.width
                text: qsTr("This free source-available build is for personal and non-commercial use only. Commercial use requires a separate license.")
                font.family: Theme.fontUi
                font.pixelSize: Sizing.fontSize(2.6)
                color: Theme.textPrimary
                wrapMode: Text.WordWrap
                horizontalAlignment: Text.AlignHCenter
                renderType: Text.NativeRendering
            }

            Text {
                width: parent.width
                text: qsTr("Contact: legal@zaparoo.org")
                font.family: Theme.fontUi
                font.pixelSize: Sizing.fontSize(2.6)
                color: Theme.textLabel
                wrapMode: Text.WordWrap
                horizontalAlignment: Text.AlignHCenter
                renderType: Text.NativeRendering
            }

            Text {
                width: parent.width
                text: qsTr("Full details available any time under Settings > About / License.")
                font.family: Theme.fontUi
                font.pixelSize: Sizing.fontSize(2.4)
                color: Theme.textLabel
                wrapMode: Text.WordWrap
                horizontalAlignment: Text.AlignHCenter
                renderType: Text.NativeRendering
            }

            // Tight inner Column so the "Created by" caption sits
            // directly above the names without inheriting the outer
            // Column's paragraph spacing.
            Column {
                width: parent.width
                spacing: Sizing.pctH(0.4)

                Text {
                    width: parent.width
                    text: qsTr("Created by")
                    font.family: Theme.fontUi
                    font.pixelSize: Sizing.fontSize(2.4)
                    color: Theme.textLabel
                    horizontalAlignment: Text.AlignHCenter
                    renderType: Text.NativeRendering
                }

                // Contributor names are not translated — they're
                // proper names.
                Text {
                    width: parent.width
                    text: "Andrea Bogazzi, BossRighteous, Tim Wilsie, Wizzo"
                    font.family: Theme.fontUi
                    font.pixelSize: Sizing.fontSize(2.6)
                    color: Theme.textPrimary
                    wrapMode: Text.WordWrap
                    horizontalAlignment: Text.AlignHCenter
                    renderType: Text.NativeRendering
                }
            }

            Item {
                id: understandSlot
                width: parent.width
                height: Sizing.pctH(7)

                Rectangle {
                    x: Sizing.center(parent.width, width)
                    y: Sizing.center(parent.height, height)
                    width: Math.min(Sizing.pctW(28), understandSlot.width)
                    height: parent.height
                    color: Theme.surfaceCard
                    // Single button — always the default action, so
                    // render with the focused recipe (accent border, 2px)
                    // instead of the unfocused borderMid edge.
                    border.width: Sizing.stroke(2)
                    border.color: Theme.focusItem
                    radius: Sizing.cornerRadius
                    transformOrigin: Item.Center
                    scale: modal._pressScale

                    Text {
                        x: Sizing.center(parent.width, width)
                        y: Sizing.center(parent.height, height)
                        text: qsTr("I understand")
                        font.family: Theme.fontUi
                        font.pixelSize: Sizing.fontSize(2.6)
                        color: Theme.textPrimary
                        renderType: Text.NativeRendering
                    }

                    MouseArea {
                        anchors.fill: parent
                        acceptedButtons: Qt.LeftButton
                        cursorShape: Qt.PointingHandCursor
                        onClicked: modal._commit()
                    }
                }
            }
        }
    }
}
