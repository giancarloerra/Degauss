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

// Blocking first-run modal. Shown once per session when the frontend
// first connects to a Core whose media database doesn't exist yet.
// The frontend refuses to drill into Systems / Games until an initial
// scan has run, so this modal owns input and only releases when the
// scan completes (auto-dismiss after a short "Done" beat) or — after
// a cancel — when the user presses Start again and lets it finish.
//
// Three visual phases:
//   "idle"      — explanatory body + a single "Start scan" button.
//   "running"   — progress bar driven by `current_step / total_steps`,
//                 the current step display below, and a Cancel button.
//                 During the vacuum phase the bar is replaced with an
//                 "Optimizing database — almost done" line (no animation
//                 — software-renderer constraint).
//   "completed" — "Done. N files indexed." for ~1.5 s, then auto-pop.
//
// The component is purely presentational; routing and the modal stack
// are owned by `Main.qml`. We emit `closeRequested` and trust the
// router to actually pop. `handleAction(accept|cancel)` is the input
// hook called from `Main.qml`'s modal-dispatch branch. Chrome (scrim,
// panel, border, radius, title) comes from the shared `Modal` shell so
// every dialog in the app reads as the same surface.
Item {
    id: modal

    property bool open: false

    // "idle" → user must press Start.
    // "running" → indexing in flight (or optimizing).
    // "completed" → finished, completionTimer ticking down to dismiss.
    property string phase: "idle"

    // Push-in scale for the single phase button, mirroring the shared
    // Modal shell's prebaked buttons. Both mouse and key/controller accept
    // play the same press cue before the action fires.
    property real _pressScale: 1.0
    property string _pendingAction: ""

    // True between a Cancel press and Core's `indexing` flag dropping, so the
    // completion check can tell a user cancel apart from a finished scan
    // (both end with indexing=false).
    property bool _cancelling: false

    signal closeRequested

    visible: modal.open
    anchors.fill: parent
    z: 300

    onOpenChanged: {
        if (modal.open) {
            modal.phase = Browse.MediaStatus.indexing ? "running" : "idle";
            modal._pressScale = 1.0;
            modal._cancelling = false;
            pressAnim.stop();
        } else {
            modal.phase = "idle";
            completionTimer.stop();
            // Disarm a pending deferred action so a press-then-close inside
            // the cue window can't fire after dismissal.
            actionCommit.stop();
            modal._pendingAction = "";
        }
    }

    // Track Core's indexing/optimizing flags. Pressing Start optimistically
    // flips phase to "running" before the first notification arrives (so the
    // "Preparing…" body paints immediately); the Connections below confirm
    // or unwind when the notification stream catches up.
    Connections {
        target: Browse.MediaStatus
        function onIndexingChanged(): void {
            if (!modal.open)
                return;
            if (Browse.MediaStatus.indexing) {
                modal.phase = "running";
                completionTimer.stop();
                return;
            }
            modal._maybeFinishRun();
        }
        // The vacuum/optimize pass runs with indexing=false; only once it
        // also clears is the run truly done.
        function onOptimizingChanged(): void {
            if (modal.open)
                modal._maybeFinishRun();
        }
    }

    // If the catalog acquires indexed systems out of band while the modal
    // sits idle — a restart, a manual TUI-driven index in another session, a
    // reconnect to a different Core — the first-run gate has nothing left to
    // defend, so we close. We only act in the "idle" phase: an in-flight scan
    // owns its own completion via `_maybeFinishRun` (which keys off Core's
    // flags, not the catalog, because the catalog refetch lags the
    // indexing=false edge). indexed_count, not count: launchables make count
    // non-zero the instant the modal opens.
    Connections {
        target: Browse.CategoriesModel
        function onIndexed_countChanged(): void {
            if (modal.open && Browse.CategoriesModel.indexed_count > 0 && modal.phase === "idle") {
                modal.closeRequested();
            }
        }
    }

    // Input hook called from the button and from Main.qml's modal-dispatch
    // branch. Only the actionable cases play the cue: Start in idle, Cancel
    // in running. A stray accept while running (or vice versa) is a no-op,
    // so it neither pulses the button nor arms the deferred action.
    function handleAction(action: string): void {
        if (action === "accept" && modal.phase === "idle")
            modal._commit("accept");
        else if (action === "cancel" && modal.phase === "running")
            modal._commit("cancel");
    }

    // Play the push-in cue on the button, then run the action deferred so
    // the animation completes before the phase flips under it.
    function _commit(action: string): void {
        modal._pendingAction = action;
        pressAnim.restart();
        actionCommit.arm();
    }

    function _apply(action: string): void {
        if (action === "accept") {
            // Optimistically flip to "running" so the "Preparing…" body
            // paints before Core's first indexing notification lands; the
            // Connections below confirms or unwinds.
            Browse.MediaStatus.start_index();
            modal.phase = "running";
        } else if (action === "cancel") {
            // Mark intent so the completion check routes the resulting
            // indexing=false edge to "idle" instead of "completed".
            modal._cancelling = true;
            Browse.MediaStatus.cancel_index();
        }
        // Release the press cue so the (reused) button doesn't stay shrunk
        // after the phase changes under it.
        modal._pressScale = 1.0;
    }

    // Decide a running scan's outcome from Core's own flags plus the cancel
    // intent, not the catalog count: the catalog refetch lags the
    // indexing=false edge, so keying off indexed_count here would miss the
    // "Done" beat. Completes only once both indexing and optimizing clear.
    function _maybeFinishRun(): void {
        if (modal.phase !== "running")
            return;
        if (Browse.MediaStatus.indexing || Browse.MediaStatus.optimizing)
            return;
        if (modal._cancelling) {
            modal._cancelling = false;
            modal.phase = "idle";
            return;
        }
        modal.phase = "completed";
        completionTimer.restart();
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
            const action = modal._pendingAction;
            modal._pendingAction = "";
            modal._apply(action);
        }
    }

    Modal {
        id: shell

        open: modal.open
        kind: "shell"
        title: qsTr("First-time setup")

        Column {
            width: parent.width
            spacing: Sizing.pctH(3)

            Text {
                width: parent.width
                visible: modal.phase === "idle"
                text: qsTr("Zaparoo Core needs to scan your games before you can use the frontend. This usually takes a few minutes.")
                font.family: Theme.fontUi
                font.pixelSize: Sizing.fontSize(2.6)
                color: Theme.textPrimary
                wrapMode: Text.WordWrap
                horizontalAlignment: Text.AlignHCenter
                renderType: Text.NativeRendering
            }

            Item {
                id: runningView

                width: parent.width
                height: Sizing.pctH(14)
                visible: modal.phase === "running"

                // Vacuum phase fallback. Mirrors the mobile app's "Optimizing
                // database" copy without the pulsing animation (software
                // renderer pays per-frame for translucent overlays).
                Text {
                    anchors.fill: parent
                    visible: Browse.MediaStatus.optimizing
                    text: qsTr("Optimizing database - almost done")
                    font.family: Theme.fontUi
                    font.pixelSize: Sizing.fontSize(2.6)
                    color: Theme.textPrimary
                    horizontalAlignment: Text.AlignHCenter
                    verticalAlignment: Text.AlignVCenter
                    wrapMode: Text.WordWrap
                    renderType: Text.NativeRendering
                }

                Item {
                    anchors.fill: parent
                    visible: !Browse.MediaStatus.optimizing

                    Rectangle {
                        id: progressTrack

                        anchors.left: parent.left
                        anchors.right: parent.right
                        anchors.top: parent.top
                        anchors.topMargin: Sizing.pctH(1)
                        height: Sizing.pctH(1.4)
                        color: Theme.borderSubtle
                        radius: Sizing.half(height)

                        Rectangle {
                            readonly property real _ratio: {
                                const tot = Browse.MediaStatus.total_steps;
                                if (tot <= 0)
                                    return 0;
                                const cur = Browse.MediaStatus.current_step;
                                return Math.max(0, Math.min(1, cur / tot));
                            }

                            anchors.left: parent.left
                            anchors.top: parent.top
                            anchors.bottom: parent.bottom
                            width: Sizing.px(progressTrack.width * _ratio)
                            color: Theme.stateMarker
                            radius: parent.radius
                        }
                    }

                    Text {
                        anchors.left: parent.left
                        anchors.right: parent.right
                        anchors.top: progressTrack.bottom
                        anchors.topMargin: Sizing.pctH(2.4)
                        text: {
                            const display = Browse.MediaStatus.current_step_display;
                            const cur = Browse.MediaStatus.current_step;
                            const tot = Browse.MediaStatus.total_steps;
                            if (Browse.MediaStatus.paused)
                                return qsTr("Indexing paused");
                            if (tot > 0 && display !== "")
                                return qsTr("Step %1 of %2 - %3").arg(cur).arg(tot).arg(display);
                            if (tot > 0)
                                return qsTr("Step %1 of %2").arg(cur).arg(tot);
                            return qsTr("Preparing…");
                        }
                        font.family: Theme.fontUi
                        font.pixelSize: Sizing.fontSize(2.4)
                        color: Theme.textLabel
                        wrapMode: Text.WordWrap
                        horizontalAlignment: Text.AlignHCenter
                        renderType: Text.NativeRendering
                        elide: Text.ElideRight
                        maximumLineCount: 2
                    }
                }
            }

            Text {
                width: parent.width
                visible: modal.phase === "completed"
                text: qsTr("Done. %1 files indexed.").arg(Browse.MediaStatus.total_files)
                font.family: Theme.fontUi
                font.pixelSize: Sizing.fontSize(2.6)
                color: Theme.textPrimary
                horizontalAlignment: Text.AlignHCenter
                wrapMode: Text.WordWrap
                renderType: Text.NativeRendering
            }

            Item {
                width: parent.width
                height: Sizing.pctH(7)
                visible: modal.phase !== "completed"

                Rectangle {
                    x: Sizing.center(parent.width, width)
                    y: Sizing.center(parent.height, height)
                    width: Sizing.pctW(28)
                    height: parent.height
                    color: Theme.surfaceCard
                    // Single button per phase — always the default action,
                    // so render with the focused recipe (accent border,
                    // 2px) instead of the unfocused borderMid edge.
                    border.width: Sizing.stroke(2)
                    border.color: Theme.focusItem
                    radius: Sizing.cornerRadius
                    transformOrigin: Item.Center
                    scale: modal._pressScale

                    Text {
                        x: Sizing.center(parent.width, width)
                        y: Sizing.center(parent.height, height)
                        text: modal.phase === "running" ? qsTr("Cancel") : qsTr("Start scan")
                        font.family: Theme.fontUi
                        font.pixelSize: Sizing.fontSize(2.6)
                        color: Theme.textPrimary
                        renderType: Text.NativeRendering
                    }

                    MouseArea {
                        anchors.fill: parent
                        acceptedButtons: Qt.LeftButton
                        cursorShape: Qt.PointingHandCursor
                        onClicked: modal.handleAction(modal.phase === "running" ? "cancel" : "accept")
                    }
                }
            }
        }
    }

    Timer {
        id: completionTimer

        interval: 1500
        repeat: false
        onTriggered: modal.closeRequested()
    }
}
