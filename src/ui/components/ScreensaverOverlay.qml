// Zaparoo Frontend
// Copyright (c) 2026 Wizzo Pty Ltd and the Zaparoo Project contributors.
// SPDX-License-Identifier: LicenseRef-PolyForm-Noncommercial-1.0.0

import QtQuick
import Zaparoo.Theme

// Screen-burn protection. After an idle timeout the frontend paints a
// solid-black backstop and bounces a single copy of the Degauss logo
// across it. Black is the only background that fully removes the
// burn-in load on OLED (pixels off = zero degradation) and gives CRT
// phosphor a true rest; a translucent scrim over a snapshot of the
// scene still leaves the static UI shapes contributing to wear,
// which defeats the feature's purpose.
//
// Software-renderer cost: the only per-frame dirty rect is the logo's
// own bounding box. The black backstop is static, so the only blits
// per frame are the small region the logo just left (repainted black)
// and the small region it just entered. State is purely in-memory;
// the timeout itself is persisted on
// `Browse.Settings.current_screensaver_timeout`.
Item {
    id: overlay

    // Public API ─────────────────────────────────────────────────────
    // True between activate() and deactivate(). Visible-bound below.
    readonly property bool armed: overlay._armed
    // Source for the bouncing logo on top of the black backstop.
    property url logoSource: ""

    // Emitted when the user clicks/taps anywhere on the overlay while
    // armed. Main.qml uses this to run the same dismiss path as a
    // keyboard/gamepad press (deactivate + reset idle + clear repeat).
    signal userDismissed

    // Internal ───────────────────────────────────────────────────────
    property bool _armed: false
    // Logo geometry copied from the live header logo, in the overlay's
    // own coordinate space.
    property real _logoStartX: 0
    property real _logoStartY: 0
    property real _logoStartW: 0
    property real _logoStartH: 0
    // Bounce direction. (+1, +1) starts down-right per spec; flipped on
    // each wall hit by `_scheduleNextBounce`.
    property int _dx: 1
    property int _dy: 1

    visible: overlay._armed

    // Activate: paint a black backstop, hold the still copy of the logo
    // at its original position for 1 s, then begin the 45° bounce.
    function activate(logoSrc: url, startRect: rect): void {
        if (overlay._armed)
            return;
        overlay.logoSource = logoSrc;
        overlay._logoStartX = Sizing.px(startRect.x);
        overlay._logoStartY = Sizing.px(startRect.y);
        overlay._logoStartW = Sizing.px(startRect.width);
        overlay._logoStartH = Sizing.px(startRect.height);
        ssLogo.x = overlay._logoStartX;
        ssLogo.y = overlay._logoStartY;
        ssLogo.width = overlay._logoStartW;
        ssLogo.height = overlay._logoStartH;
        overlay._dx = 1;
        overlay._dy = 1;
        overlay._armed = true;
        holdBeforeBounce.restart();
    }

    function deactivate(): void {
        if (!overlay._armed)
            return;
        bounceSegment.stop();
        holdBeforeBounce.stop();
        overlay._armed = false;
    }

    // ── Solid-black backstop ─────────────────────────────────────────
    // The only background. Opaque, painted the instant the screensaver
    // arms. Zero static luminance under the logo means zero burn-in
    // contribution on OLED and minimal phosphor wear on CRT.
    Rectangle {
        id: hardBackstop

        anchors.fill: parent
        color: "black"
        visible: overlay._armed
    }

    // ── Bouncing logo ────────────────────────────────────────────────
    // Single Image element whose `x`/`y` are driven by a chained
    // ParallelAnimation. PreserveAspectFit + smooth: false keeps the
    // raster crisp at any window size; the start geometry mirrors the
    // header logo so the activation looks like the logo dimming in
    // place before walking off.
    Image {
        id: ssLogo

        source: overlay.logoSource
        fillMode: Image.PreserveAspectFit
        smooth: false
        cache: true
        visible: overlay._armed
    }

    // Click/tap dismissal. Enabled only while armed so the overlay
    // does not eat input on the live screens. The top-level idle
    // MouseArea (Qt.NoButton) above this lets press events fall
    // through to here.
    MouseArea {
        id: dismissArea

        anchors.fill: parent
        enabled: overlay._armed
        visible: enabled
        hoverEnabled: false
        acceptedButtons: Qt.AllButtons
        onPressed: mouse => {
            overlay.userDismissed();
            mouse.accepted = true;
        }
    }

    Timer {
        id: holdBeforeBounce
        interval: 1000
        repeat: false
        onTriggered: overlay._scheduleNextBounce()
    }

    ParallelAnimation {
        id: bounceSegment

        NumberAnimation {
            id: animX
            target: ssLogo
            property: "x"
            easing.type: Easing.Linear
        }
        NumberAnimation {
            id: animY
            target: ssLogo
            property: "y"
            easing.type: Easing.Linear
        }
        onFinished: overlay._scheduleNextBounce()
    }

    function _scheduleNextBounce(): void {
        if (!overlay._armed)
            return;
        const minX = 0;
        const minY = 0;
        const maxX = overlay.width - ssLogo.width;
        const maxY = overlay.height - ssLogo.height;
        if (maxX <= minX || maxY <= minY)
            return;
        // Snap-to-edge correction. Floating-point drift can leave the
        // logo a sub-pixel shy of the wall when the previous segment
        // ended; treat anything within 0.5 px as flush so the new
        // direction flips deterministically.
        if (ssLogo.x <= minX + 0.5)
            overlay._dx = 1;
        else if (ssLogo.x >= maxX - 0.5)
            overlay._dx = -1;
        if (ssLogo.y <= minY + 0.5)
            overlay._dy = 1;
        else if (ssLogo.y >= maxY - 0.5)
            overlay._dy = -1;
        const distX = overlay._dx > 0 ? maxX - ssLogo.x : ssLogo.x - minX;
        const distY = overlay._dy > 0 ? maxY - ssLogo.y : ssLogo.y - minY;
        const dist = Math.min(distX, distY);
        if (dist < 1)
            return;
        const endX = Sizing.px(ssLogo.x + overlay._dx * dist);
        const endY = Sizing.px(ssLogo.y + overlay._dy * dist);
        // Speed = full window width per 10 s, scaled so a 45° vector
        // covers the same screen-width-per-second regardless of the
        // current resolution. Slow DVD-player drift, ~3x slower than a
        // typical bouncing-logo screensaver feels at this size.
        const speedPxPerS = overlay.width > 0 ? overlay.width / 10.0 : 1;
        const dur = Math.max(16, Math.round((dist / speedPxPerS) * 1000));
        bounceSegment.stop();
        animX.from = ssLogo.x;
        animX.to = endX;
        animX.duration = dur;
        animY.from = ssLogo.y;
        animY.to = endY;
        animY.duration = dur;
        bounceSegment.start();
    }
}
