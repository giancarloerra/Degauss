// Zaparoo Frontend
// Copyright (c) 2026 Wizzo Pty Ltd and the Zaparoo Project contributors.
// SPDX-License-Identifier: LicenseRef-PolyForm-Noncommercial-1.0.0

import QtQuick
import Zaparoo.Theme

// Single row in a `SettingsScreen.qml` form. Label on the left, an
// accessory cluster on the right whose shape is selected by `control`:
//   "picker"   — current value text. Accept opens a list-picker modal.
//   "toggle"   — pill-shaped on/off control.
//   "action"   — trigger row. Status caption when an operation is in
//                flight; nothing while idle.
//   "navigate" — `›` chevron. Reserved for rows that open another
//                screen (subpages, About / License).
//
// Surface is always `surfaceCard`; focus swaps the border to
// `Theme.focusItem` (2px) and unfocused rows show `Theme.borderMid` (1px).
// Same recipe as tile cards, modal buttons, and list-picker rows —
// settings rows are no longer the visual outlier.
//
// The component is purely presentational. The screen owns layout (Column
// stacking + selection index) and value mutation.
Item {
    id: root

    required property string label
    required property string value
    property string control: "picker"
    property bool checked: false
    property bool isFocused: false
    // Set false by SettingsScreen during a page switch so reused delegates
    // do not animate the focus-border or toggle position when the new
    // page's model data lands. Restored to true on the next event-loop
    // tick so ordinary user navigation still animates.
    property bool animateChanges: true
    // For `control: "action"` — short live-state string painted on the
    // right ("In progress", "Paused", or "" when idle). The screen
    // owns the binding; the field treats it as a plain caption.
    property string actionStatus: ""
    // Push-in cue, matching the tile vocabulary. The host increments this on
    // accept; the focused row plays a one-shot tap (scale down then back). A
    // tap rather than a held press because settings rows stay on screen (a
    // picker opens a modal over the row, an action runs in place), so a held
    // scale would look stuck. Toggle rows are exempt — the knob slide is their
    // feedback — so they ignore the pulse below.
    property int activatePulse: 0
    property real _pressScale: 1.0

    signal hovered
    signal clicked
    signal rightClicked
    // Emitted when the action-control row receives an accept press.
    // The screen wires this to the matching invokable (start/cancel
    // index, start/cancel scrape) and gates by `actionStatus`.
    signal accepted

    // Item.enabled (built-in) gates the MouseArea below; the dimmed
    // opacity here gives a matching visual cue. Setting `enabled: false`
    // on the row makes Accept a no-op (the index/scrape pair use this
    // when one of the two is in flight — Core serialises them).
    opacity: enabled ? 1 : 0.4
    implicitHeight: Sizing.pctH(8)
    scale: root._pressScale
    transformOrigin: Item.Center

    // One-shot push-in tap. Non-toggle rows only; the focused row plays it.
    // `animateChanges` is false during a page switch (same gate the field's
    // other animations use), so a reused delegate cannot tap as a new page
    // mounts.
    onActivatePulseChanged: {
        if (root.isFocused && root.control !== "toggle" && root.animateChanges)
            pressAnim.restart();
    }

    SequentialAnimation {
        id: pressAnim
        NumberAnimation {
            target: root
            property: "_pressScale"
            to: Motion.rowPressScale
            duration: Motion.dur(Motion.pressMs)
            easing.type: Easing.OutQuad
        }
        NumberAnimation {
            target: root
            property: "_pressScale"
            to: 1.0
            duration: Motion.dur(Motion.settleMs)
            easing.type: Easing.OutQuad
        }
    }

    Rectangle {
        id: surface

        anchors.fill: parent
        radius: Sizing.cornerRadius
        color: Theme.surfaceCard
        border.color: root.isFocused ? Theme.focusItem : Theme.borderMid
        border.width: root.isFocused ? Sizing.stroke(2) : Sizing.stroke(1)
    }

    Text {
        id: labelText

        anchors.left: parent.left
        anchors.leftMargin: Sizing.pctW(2)
        anchors.verticalCenter: parent.verticalCenter
        text: root.label
        color: Theme.textPrimary
        font.family: Theme.fontUi
        font.pixelSize: Sizing.fontSize(2.6)
        renderType: Text.NativeRendering
    }

    // Right-side current-value text for `control: "picker"`. Accept on
    // a picker row opens the list-picker modal owned by `Main.qml`;
    // left/right are no-ops (no inline cycling — see `SettingsScreen`).
    //
    // Anchors clamp between the label's right edge and the row's
    // right padding so a long localized value (e.g. a translated
    // language name) elides instead of overlapping `labelText`.
    Text {
        visible: root.control === "picker"
        anchors.left: labelText.right
        anchors.leftMargin: Sizing.pctW(2)
        anchors.right: parent.right
        anchors.rightMargin: Sizing.pctW(2)
        anchors.verticalCenter: parent.verticalCenter
        text: root.value
        color: Theme.textPrimary
        font.family: Theme.fontUi
        font.pixelSize: Sizing.fontSize(2.6)
        elide: Text.ElideRight
        horizontalAlignment: Text.AlignRight
        renderType: Text.NativeRendering
    }

    Item {
        id: toggle

        visible: root.control === "toggle"
        anchors.right: parent.right
        anchors.rightMargin: Sizing.pctW(2)
        anchors.verticalCenter: parent.verticalCenter
        // Standard pill-toggle proportion: width ≈ 1.85 × height keeps
        // the handle's travel close to one diameter on either side
        // without leaving the long rail of empty pill the previous
        // pctW(8) (~3.7× height on a 16:9 panel) painted.
        height: Sizing.pctH(3.8)
        width: Sizing.px(height * 1.85)

        Rectangle {
            anchors.fill: parent
            radius: Sizing.half(height)
            // Fill alone carries on/off state — no border. The row's
            // outer surface carries the focus indicator, and against
            // the always-on card behind the toggle a static pill
            // border read as chrome-on-chrome.
            color: root.checked ? Theme.focusItem : Theme.borderMid
        }

        Rectangle {
            width: toggle.height - Sizing.pctH(0.9)
            height: width
            radius: Sizing.half(width)
            x: root.checked ? Sizing.px(toggle.width - width - Sizing.pctH(0.45)) : Sizing.pctH(0.45)
            anchors.verticalCenter: parent.verticalCenter
            color: Theme.textPrimary

            Behavior on x {
                enabled: Motion.enabled && root.animateChanges
                NumberAnimation {
                    duration: Motion.dur(Motion.settleMs)
                    easing.type: Easing.OutQuad
                }
            }
        }
    }

    // Right-side value for `control: "action"`. Carries either a
    // transient run state ("In progress" / "Paused" / "Optimizing")
    // or a persistent idle count ("100,000 indexed"). Styled to match
    // the picker right-text recipe so idle counts read as values, not
    // dimmed chrome. No chevron — chevron is reserved for navigation.
    Text {
        visible: root.control === "action" && root.actionStatus !== ""
        anchors.left: labelText.right
        anchors.leftMargin: Sizing.pctW(2)
        anchors.right: parent.right
        anchors.rightMargin: Sizing.pctW(2)
        anchors.verticalCenter: parent.verticalCenter
        text: root.actionStatus
        color: Theme.textPrimary
        font.family: Theme.fontUi
        font.pixelSize: Sizing.fontSize(2.6)
        elide: Text.ElideRight
        horizontalAlignment: Text.AlignRight
        renderType: Text.NativeRendering
    }

    // Right-side chevron for `control: "navigate"`. Means "this row
    // opens another page" — used for About / License today and any
    // future subpage entries.
    Image {
        visible: root.control === "navigate"
        anchors.right: parent.right
        anchors.rightMargin: Sizing.pctW(2)
        anchors.verticalCenter: parent.verticalCenter
        source: Resources.iconUrl("NavRight")
        width: Sizing.pctH(3.5)
        height: width
        fillMode: Image.PreserveAspectFit
        smooth: true
    }

    MouseArea {
        anchors.fill: parent
        hoverEnabled: true
        acceptedButtons: Qt.LeftButton | Qt.RightButton
        cursorShape: Qt.PointingHandCursor
        onEntered: root.hovered()
        // Action rows fire `accepted()` (the screen runs start/cancel
        // there); every other control fires `clicked()` (the screen
        // moves focus and toggles a value). Emitting both for action
        // rows used to make `onClicked` and `onAccepted` race over
        // the same press.
        onClicked: mouse => {
            if (mouse.button === Qt.RightButton)
                root.rightClicked();
            else if (root.control === "action" || root.control === "navigate" || root.control === "picker")
                root.accepted();
            else
                root.clicked();
        }
    }
}
