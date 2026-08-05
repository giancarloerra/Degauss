// Zaparoo Frontend
// Copyright (c) 2026 Wizzo Pty Ltd and the Zaparoo Project contributors.
// SPDX-License-Identifier: LicenseRef-PolyForm-Noncommercial-1.0.0
pragma Singleton
import QtQuick

// Project-wide color and font constants.
// Never hardcode colors or font families inline — use these instead.
QtObject {
    property bool crtNativePath: false

    // UI colour theme, driven by the persisted `theme` setting via
    // MainLayout's Binding. "crt-light" lifts the backgrounds and
    // surfaces for analog CRT output, where the stock near-black
    // palette reads too dark on a tube.
    property string theme: "default"
    readonly property bool _crtLight: theme === "crt-light"

    // Backgrounds
    readonly property color bgDeep: _crtLight ? "#69698A" : "#0f0f23"
    readonly property color bgPanel: _crtLight ? "#36365E" : "#1a1a35"
    readonly property color bgBar: _crtLight ? "#464669" : "#0a0a15"
    // Card surface used for tile bodies in rows/grids. Sits a step
    // above bgPanel so a solid white icon+label silhouette has clear
    // contrast — the page bg pattern stays visible in the gaps between
    // tiles, and each tile reads as a self-contained chip.
    readonly property color surfaceCard: _crtLight ? "#464669" : "#22223a"
    // Selected row fill. Cooler and darker than the amber accent so
    // text stays high-contrast while the accent bar remains the focus
    // cue layered on top.
    readonly property color selectionSurface: "#3a3a66"
    // Modal scrim — translucent black so the screen behind a modal
    // dims uniformly without a blur or shader pass.
    readonly property color scrim: "#cc000000"
    // Borders
    readonly property color borderSubtle: "#1a1a2e"
    readonly property color borderMid: "#404060"

    // Text
    readonly property color textPrimary: "#ffffff"
    readonly property color textLabel: "#888888"
    // Unselected list-row titles: on the lighter crt-light surfaces the
    // stock mid-grey sinks into the background, so lift it while keeping
    // clear headroom below the selected row's full white.
    readonly property color textListTitleDim: _crtLight ? "#C2C2DC" : "#888888"
    // Variant/disambiguation suffix tone — a muted lavender-grey that reads as
    // secondary metadata next to the title without competing with it, and
    // stays legible on `surfaceCard` and on the CRT path. Drawn after the name
    // in the inline caption (see `ScrollingCaption.qml`).
    readonly property color textVariant: "#8a8ab2"
    // Accent — static warm amber used for selection highlights.
    readonly property color accent: "#FFB347"
    // Persistent-state marker tint (favorite heart, hidden badge). Lavender,
    // not the amber accent, so these markers stay distinct from the focus
    // ring/logo tint instead of melting into them — amber means "selected"
    // exclusively. Paired with a dark `bgBar` outline/border for visibility on
    // light cover art. The hidden badge uses it directly (TileBadge); the
    // favorite heart is tinted to it on the fly via the tinted-svg provider
    // (Heart.svg is a neutral grayscale source), so the color lives only here.
    readonly property color stateMarker: "#9898CC"
    // System logo tint tokens — two ramps, selected by Tile based on focus state.
    // Inactive ramp: medium purple so unfocused tiles read as secondary
    // against the amber focused ramp. Was near-white (#E4E4F6) which
    // made unfocused and focused tiles look too similar.
    readonly property color logoPrimary: "#9898CC"
    readonly property color logoSecondary: "#6060A8"
    readonly property color logoShadow: "#3C3C80"
    // Focused ramp: amber accent marks the selected tile's logo.
    readonly property color logoFocusPrimary: "#FFE3B8"
    readonly property color logoFocusSecondary: accent
    readonly property color logoFocusShadow: "#9E5E15"
    // Error emphasis, kept distinct from the amber selection accent.
    readonly property string errorHex: "#ff8a7a"
    readonly property color error: errorHex
    // Fonts
    readonly property string fontUi: crtNativePath ? "MxPlus HP 100LX 6x8" : "Noto Sans"
    readonly property string fontMono: crtNativePath ? "MxPlus HP 100LX 6x8" : "monospace"
}
