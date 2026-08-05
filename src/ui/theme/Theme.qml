// Zaparoo Frontend
// Copyright (c) 2026 Wizzo Pty Ltd and the Zaparoo Project contributors.
// SPDX-License-Identifier: LicenseRef-PolyForm-Noncommercial-1.0.0
pragma Singleton
import QtQuick

// Project-wide color and font constants.
// Never hardcode colors or font families inline — use these instead.
//
// Degauss runs a monochrome palette: every tone is a grey whose value
// was derived from the upstream colour it replaced by preserving
// relative luminance (sRGB -> linear -> Y -> sRGB), so contrast
// relationships and perceived brightness match the original design
// exactly; only hue and saturation were removed. Content (covers,
// artwork, the wordmark) stays full colour.
QtObject {
    property bool crtNativePath: false

    // UI colour theme, driven by the persisted `theme` setting via
    // MainLayout's Binding. "crt-light" lifts the backgrounds and
    // surfaces for analog CRT output, where the stock near-black
    // palette reads too dark on a tube.
    property string theme: "default"
    readonly property bool _crtLight: theme === "crt-light"

    // Backgrounds
    readonly property color bgDeep: _crtLight ? "#6C6C6C" : "#111111"
    readonly property color bgPanel: _crtLight ? "#3A3A3A" : "#1D1D1D"
    readonly property color bgBar: _crtLight ? "#494949" : "#0B0B0B"
    // Card surface used for tile bodies in rows/grids. Sits a step
    // above bgPanel so a solid white icon+label silhouette has clear
    // contrast — the page bg pattern stays visible in the gaps between
    // tiles, and each tile reads as a self-contained chip.
    readonly property color surfaceCard: _crtLight ? "#494949" : "#242424"
    // Selected row fill. Darker than the accent tone so text stays
    // high-contrast while the accent bar remains the focus cue layered
    // on top.
    readonly property color selectionSurface: "#3E3E3E"
    // Modal scrim — translucent black so the screen behind a modal
    // dims uniformly without a blur or shader pass.
    readonly property color scrim: "#cc000000"
    // Borders
    readonly property color borderSubtle: "#1C1C1C"
    readonly property color borderMid: "#434343"

    // Text
    readonly property color textPrimary: "#ffffff"
    readonly property color textLabel: "#888888"
    // Unselected list-row titles: on the lighter crt-light surfaces the
    // stock mid-grey sinks into the background, so lift it while keeping
    // clear headroom below the selected row's full white.
    readonly property color textListTitleDim: _crtLight ? "#C4C4C4" : "#888888"
    // Variant/disambiguation suffix tone — a muted grey that reads as
    // secondary metadata next to the title without competing with it, and
    // stays legible on `surfaceCard` and on the CRT path. Drawn after the name
    // in the inline caption (see `ScrollingCaption.qml`).
    readonly property color textVariant: "#8D8D8D"
    // Accent — bright tone used for selection highlights.
    readonly property color accent: "#C2C2C2"
    // Persistent-state marker tint (favorite heart, hidden badge). A step
    // darker than the accent so these markers stay distinct from the focus
    // ring/logo tint instead of melting into them — the accent brightness
    // means "selected" exclusively. Paired with a dark `bgBar`
    // outline/border for visibility on light cover art. The hidden badge
    // uses it directly (TileBadge); the favorite heart is tinted to it on
    // the fly via the tinted-svg provider (Heart.svg is a neutral
    // grayscale source), so the color lives only here.
    readonly property color stateMarker: "#9D9D9D"
    // System logo tint tokens — two ramps, selected by Tile based on focus state.
    // Inactive ramp: mid grey so unfocused tiles read as secondary
    // against the brighter focused ramp.
    readonly property color logoPrimary: "#9D9D9D"
    readonly property color logoSecondary: "#676767"
    readonly property color logoShadow: "#444444"
    // Focused ramp: the bright accent tone marks the selected tile's logo.
    readonly property color logoFocusPrimary: "#E7E7E7"
    readonly property color logoFocusSecondary: accent
    readonly property color logoFocusShadow: "#6D6D6D"
    // Error emphasis, kept distinct in brightness from the selection accent.
    readonly property string errorHex: "#ABABAB"
    readonly property color error: errorHex
    // Fonts
    readonly property string fontUi: crtNativePath ? "MxPlus HP 100LX 6x8" : "Noto Sans"
    readonly property string fontMono: crtNativePath ? "MxPlus HP 100LX 6x8" : "monospace"
}
