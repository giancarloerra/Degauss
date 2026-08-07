// Zaparoo Frontend
// Copyright (c) 2026 Wizzo Pty Ltd and the Zaparoo Project contributors.
// SPDX-License-Identifier: LicenseRef-PolyForm-Noncommercial-1.0.0

import QtQuick
import Zaparoo.Theme

// Reusable modal panel. Four flavors selected by `kind`:
//   "action_error" — title (+ optional body) + one OK button. Caller
//                    wires `accepted` to its dismiss handler.
//   "transient"    — title (+ optional body) + optional Cancel pill, no
//                    accept button. Auto-dismisses via the caller's
//                    failure timer or external signal. The Cancel pill
//                    hides once `failed` flips so the failure flash is
//                    non-interactive.
//   "confirm"      — title + body + two pills (No / Yes). Default focus
//                    is "No", so a stray accept can't trigger the
//                    destructive path. The router calls `handleAction`
//                    to toggle focus and dispatch confirm/cancel.
//   "shell"        — title + caller-provided content slot, no built-in
//                    body or buttons. Used by the first-run, commercial-
//                    notice, and log-upload modals so they share this
//                    chrome instead of hand-rolling their own scrim,
//                    panel, and Column. The consumer places its content
//                    (and any phase-specific buttons) in the default
//                    property slot and owns its own `handleAction` and
//                    dismissal.
//
// All four kinds share the same chrome — rounded corners
// (`Sizing.cornerRadius`), `Theme.bgPanel` fill, dark scrim — so every
// modal in the app reads as the same surface. See `docs/style.md` →
// "Modal chrome".
//
// Pure presentation: input routing for the prebaked kinds lives in
// Main.qml, persistence in Browse.AppState. The component renders,
// swallows clicks on its scrim, and emits `cancelRequested` (transient
// Cancel pill, confirm No / Back), `accepted` (action_error button), or
// `confirmed` (confirm Yes).
//
// Software-rendering safe — only Item, Rectangle, Text, Column, Row,
// MouseArea, and scale transforms (buttons push in on activation).
Item {
    id: modal

    property bool open: false
    property string kind: "action_error"
    property string title: ""
    property string body: ""                 // optional secondary line
    property string buttonLabel: qsTr("OK")  // action_error only
    property string confirmYesLabel: qsTr("Yes")  // confirm only
    property string confirmNoLabel: qsTr("No")    // confirm only
    property bool failed: false              // transient only
    // Override the panel's max width on a per-callsite basis. The
    // content-heavier shell modals (legal notice, log upload with QR)
    // bump this up.
    property int panelMaxWidth: Sizing.pctH(90)

    // confirm-only focus. False = No focused (safe default), true = Yes
    // focused. Reset on every open so a previous Yes-focus doesn't leak
    // into the next prompt.
    property bool _focusYes: false

    // Push-in scale for button activation, mirroring the tile push-in.
    // _pressTarget identifies which button is currently scaled; the others
    // stay at 1.0 so only the pressed button cues the user's intention.
    property real _pressScale: 1.0
    property string _pressTarget: ""
    property string _pendingSignal: ""

    // Shell-mode content slot. Children declared inside a Modal are
    // routed here; only rendered when kind === "shell" so a stray child
    // on a prebaked-kind modal can't leak into the panel.
    default property alias contentData: contentSlot.data

    signal accepted         // action_error: button click
    signal cancelRequested  // transient Cancel; confirm No / Back
    signal confirmed        // confirm: Yes selected

    visible: modal.open
    anchors.fill: parent
    z: 300

    onOpenChanged: {
        if (!modal.open) {
            // Disarm a pending deferred signal so a press-then-close inside the
            // deferred window cannot emit confirmed/accepted after dismissal.
            actionCommit.stop();
            return;
        }
        if (modal.kind === "confirm")
            modal._focusYes = false;
        modal._pressScale = 1.0;
        modal._pressTarget = "";
        modal._pendingSignal = "";
        pressAnim.stop();
    }

    // Input dispatch. Main.qml routes key/controller actions here while
    // this modal is on top of the stack.
    function handleAction(action: string): void {
        // action_error has a single OK button — accept plays the push cue
        // then emits `accepted`, matching the mouse path so key/controller
        // dismissal animates identically.
        if (modal.kind === "action_error") {
            if (action === "accept")
                modal._commit("ok", "accepted");
            return;
        }
        if (modal.kind !== "confirm")
            return;
        if (action === "left") {
            modal._focusYes = false;
        } else if (action === "right") {
            modal._focusYes = true;
        } else if (action === "accept") {
            if (modal._focusYes)
                modal._commit("yes", "confirmed");
            else
                modal._commit("no", "cancelRequested");
        } else if (action === "cancel") {
            // Back-key dismissal — not an on-screen button, no push-in.
            modal.cancelRequested();
        }
    }

    // Play the push-in cue on the named button, then emit the pending
    // signal deferred so the animation completes before the caller acts.
    function _commit(target: string, sig: string): void {
        modal._pressTarget = target;
        modal._pendingSignal = sig;
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
            const sig = modal._pendingSignal;
            modal._pendingSignal = "";
            if (sig === "accepted")
                modal.accepted();
            else if (sig === "confirmed")
                modal.confirmed();
            else if (sig === "cancelRequested")
                modal.cancelRequested();
        }
    }

    Rectangle {
        anchors.fill: parent
        color: Theme.scrim

        // Eat clicks AND hover on the scrim so they don't reach the
        // screens underneath. Without `hoverEnabled`, mouse-mode hover
        // events fall through to the screen, and the screen's
        // `onHovered` handlers keep moving its `currentIndex` while
        // a modal is on top — focus tracks the cursor under the scrim.
        MouseArea {
            anchors.fill: parent
            hoverEnabled: true
            acceptedButtons: Qt.AllButtons
        }

        Rectangle {
            x: Sizing.center(parent.width, width)
            y: Sizing.center(parent.height, height)
            width: Sizing.px(Math.min(parent.width * 0.78, modal.panelMaxWidth))
            height: contentColumn.height + Sizing.pctH(8)
            color: Theme.bgPanel
            radius: Sizing.cornerRadius

            Column {
                id: contentColumn

                anchors.left: parent.left
                anchors.right: parent.right
                anchors.top: parent.top
                anchors.topMargin: Sizing.pctH(4)
                anchors.leftMargin: Sizing.pctW(4)
                anchors.rightMargin: Sizing.pctW(4)
                spacing: Sizing.pctH(3)

                Text {
                    width: parent.width
                    visible: modal.title !== ""
                    text: modal.title
                    font.family: Theme.fontUi
                    font.pixelSize: Sizing.fontSize(3.2)
                    color: Theme.textPrimary
                    wrapMode: Text.WordWrap
                    horizontalAlignment: Text.AlignHCenter
                    renderType: Text.NativeRendering
                }

                Text {
                    width: parent.width
                    visible: modal.body !== "" && modal.kind !== "shell"
                    text: modal.body
                    font.family: Theme.fontUi
                    font.pixelSize: Sizing.fontSize(2.6)
                    color: Theme.textPrimary
                    wrapMode: Text.WordWrap
                    horizontalAlignment: Text.AlignHCenter
                    renderType: Text.NativeRendering
                }

                // Caller content — only rendered in shell mode. Column
                // skips invisible children, so the slot consumes no
                // vertical space outside shell mode.
                Item {
                    id: contentSlot

                    width: parent.width
                    height: childrenRect.height
                    visible: modal.kind === "shell"
                }

                // Cancel pill — transient flavor, hidden once `failed`
                // flips. Failure is a terminal display that auto-dismisses,
                // not interactive.
                Item {
                    id: cancelSlot
                    width: parent.width
                    height: Sizing.pctH(7)
                    visible: modal.kind === "transient" && !modal.failed

                    Rectangle {
                        x: Sizing.center(parent.width, width)
                        y: Sizing.center(parent.height, height)
                        // Cap at pctW(28) for the typical case but never
                        // exceed the slot width — the modal panel is
                        // height-bound on widescreen, so a screen-width
                        // pill can otherwise overflow the panel.
                        width: Math.min(Sizing.pctW(28), cancelSlot.width)
                        height: parent.height
                        color: Theme.surfaceCard
                        // Single button — always the default action, so
                        // render with the focused recipe (accent border,
                        // 2px) instead of the unfocused borderMid edge.
                        border.width: Sizing.stroke(2)
                        border.color: Theme.focusItem
                        radius: Sizing.cornerRadius
                        transformOrigin: Item.Center
                        scale: modal._pressTarget === "cancel" ? modal._pressScale : 1.0

                        Text {
                            x: Sizing.center(parent.width, width)
                            y: Sizing.center(parent.height, height)
                            text: qsTr("Cancel")
                            font.family: Theme.fontUi
                            font.pixelSize: Sizing.fontSize(2.6)
                            color: Theme.textPrimary
                            renderType: Text.NativeRendering
                        }

                        MouseArea {
                            anchors.fill: parent
                            cursorShape: Qt.PointingHandCursor
                            onClicked: modal._commit("cancel", "cancelRequested")
                        }
                    }
                }

                // Accept button — action_error flavor.
                Item {
                    id: acceptSlot
                    width: parent.width
                    height: Sizing.pctH(7)
                    visible: modal.kind === "action_error"

                    Rectangle {
                        x: Sizing.center(parent.width, width)
                        y: Sizing.center(parent.height, height)
                        width: Math.min(Sizing.pctW(28), acceptSlot.width)
                        height: parent.height
                        color: Theme.surfaceCard
                        // Single button — always the default action, so
                        // render with the focused recipe (accent border,
                        // 2px) instead of the unfocused borderMid edge.
                        border.width: Sizing.stroke(2)
                        border.color: Theme.focusItem
                        radius: Sizing.cornerRadius
                        transformOrigin: Item.Center
                        scale: modal._pressTarget === "ok" ? modal._pressScale : 1.0

                        Text {
                            x: Sizing.center(parent.width, width)
                            y: Sizing.center(parent.height, height)
                            text: modal.buttonLabel
                            font.family: Theme.fontUi
                            font.pixelSize: Sizing.fontSize(2.6)
                            color: Theme.textPrimary
                            renderType: Text.NativeRendering
                        }

                        MouseArea {
                            anchors.fill: parent
                            cursorShape: Qt.PointingHandCursor
                            onClicked: modal._commit("ok", "accepted")
                        }
                    }
                }

                // No / Yes pair — confirm flavor. Focused pill draws an
                // accent border; mouse clicks bypass focus and dispatch
                // straight to the matching signal.
                Item {
                    id: confirmSlot

                    width: parent.width
                    height: Sizing.pctH(7)
                    visible: modal.kind === "confirm"

                    // Pill width caps at pctW(28) but shrinks to half
                    // the slot (minus the gap) when the panel is too
                    // narrow for two preferred pills. Computed off the
                    // slot, not the Row, so the Row can stay implicitly
                    // sized by its children and centered.
                    readonly property int _gap: Sizing.pctW(2)
                    readonly property int _pillWidth: Math.min(Sizing.pctW(28), Math.max(0, Sizing.px((width - _gap) / 2)))

                    Row {
                        x: Sizing.center(parent.width, width)
                        y: Sizing.center(parent.height, height)
                        spacing: confirmSlot._gap

                        Rectangle {
                            width: confirmSlot._pillWidth
                            height: Sizing.pctH(7)
                            color: Theme.surfaceCard
                            border.width: modal._focusYes ? Sizing.stroke(1) : Sizing.stroke(2)
                            border.color: modal._focusYes ? Theme.borderMid : Theme.focusItem
                            radius: Sizing.cornerRadius
                            transformOrigin: Item.Center
                            scale: modal._pressTarget === "no" ? modal._pressScale : 1.0

                            Text {
                                x: Sizing.center(parent.width, width)
                                y: Sizing.center(parent.height, height)
                                text: modal.confirmNoLabel
                                font.family: Theme.fontUi
                                font.pixelSize: Sizing.fontSize(2.6)
                                color: Theme.textPrimary
                                renderType: Text.NativeRendering
                            }

                            MouseArea {
                                anchors.fill: parent
                                cursorShape: Qt.PointingHandCursor
                                onClicked: {
                                    modal._focusYes = false;
                                    modal._commit("no", "cancelRequested");
                                }
                            }
                        }

                        Rectangle {
                            width: confirmSlot._pillWidth
                            height: Sizing.pctH(7)
                            color: Theme.surfaceCard
                            border.width: modal._focusYes ? Sizing.stroke(2) : Sizing.stroke(1)
                            border.color: modal._focusYes ? Theme.focusItem : Theme.borderMid
                            radius: Sizing.cornerRadius
                            transformOrigin: Item.Center
                            scale: modal._pressTarget === "yes" ? modal._pressScale : 1.0

                            Text {
                                x: Sizing.center(parent.width, width)
                                y: Sizing.center(parent.height, height)
                                text: modal.confirmYesLabel
                                font.family: Theme.fontUi
                                font.pixelSize: Sizing.fontSize(2.6)
                                color: Theme.textPrimary
                                renderType: Text.NativeRendering
                            }

                            MouseArea {
                                anchors.fill: parent
                                cursorShape: Qt.PointingHandCursor
                                onClicked: {
                                    modal._focusYes = true;
                                    modal._commit("yes", "confirmed");
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
