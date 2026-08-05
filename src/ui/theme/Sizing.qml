// Zaparoo Frontend
// Copyright (c) 2026 Wizzo Pty Ltd and the Zaparoo Project contributors.
// SPDX-License-Identifier: LicenseRef-PolyForm-Noncommercial-1.0.0
pragma Singleton
import QtQuick

// Resolution-agnostic sizing helpers.
// All UI elements must use these functions rather than hardcoded pixel values.
// The UI must run correctly from 240p (CRT) through 1080p.
QtObject {
    id: root

    // Reference window dimensions — updated by Main.qml on start and resize.
    property real screenWidth: 640
    property real screenHeight: 480
    property bool crtNativePath: false
    property bool swapPercentageAxes: false

    // Visible tile-row covers: fewer at very low resolution to avoid crowding.
    readonly property int visibleCovers: screenHeight < 300 ? 3 : 5
    // Shared browse-grid bounds. Systems and games both solve the same
    // viewport-fit problem now, so the common limits live here and the
    // per-surface configs only override what is materially different.
    readonly property var _browseGridBaseConfig: _browseGridBaseConfigForTheme()
    // Systems grid uses the same viewport-driven shape selection as
    // games so both browse screens present a similar amount of content.
    // Systems tiles are squarer than box-art tiles, so they target a
    // slightly wider aspect while keeping the same preferred page size.
    readonly property var _systemsGridConfig: _gridConfig(_browseGridBaseConfig, {
        "minCellHeight": crtNativePath ? 72 : 140,
        "preferredPageSize": crtNativePath ? 9 : 12,
        "targetAspect": 1.25
    })
    readonly property var _systemsGridShape: systemsGridShape(screenWidth, screenHeight)
    // qmllint disable compiler
    readonly property int systemsGridColumns: _systemsGridShape.columns
    readonly property int systemsGridRows: _systemsGridShape.rows
    // qmllint enable compiler
    // Games grid shape comes from the logical viewport, not from
    // screen-height-only breakpoints. The selector preserves a stable
    // tile aspect while respecting a minimum readable tile size, so
    // rotating the scene changes how many tiles fit without stretching
    // the cards into a different shape.
    readonly property var _gamesGridConfig: _gridConfig(_browseGridBaseConfig, {
        "minCellHeight": crtNativePath ? 96 : 210,
        "targetAspect": crtNativePath ? 0.78 : 0.71
    })
    readonly property var _gamesGridShape: gamesGridShape(screenWidth, screenHeight)
    // qmllint disable compiler
    readonly property int gamesGridColumns: _gamesGridShape.columns
    readonly property int gamesGridRows: _gamesGridShape.rows
    // qmllint enable compiler
    // Standard corner radius for rounded surfaces — tile cards, focus
    // rings (computed as `cornerRadius - outlineGap`), settings rows.
    // Pill controls (toggle track/thumb) use `height/2` instead and
    // are intentionally a different shape. See docs/style.md.
    readonly property int cornerRadius: pctH(3.5)
    // ── Top header (logo + status row + status pill) ──────────────────
    // Single source of truth for the header bar that sits at the top of
    // every screen. The logo's height is locked to the stacked-row
    // total so the brand mark sits flush with the top of the status
    // row and the bottom of the pill row, even when the pill is idle
    // (its space is reserved). Screen content clears `headerBottom`.
    readonly property int headerRowHeight: fontSize(3.4)
    readonly property int headerStackGap: pctH(0.8)
    readonly property int headerTopMargin: pctH(2)
    readonly property int headerSideMargin: pctW(2)
    readonly property int headerHeight: 2 * headerRowHeight + headerStackGap
    readonly property int headerBottom: headerTopMargin + headerHeight

    function _browseGridBaseConfigForTheme(): var {
        return {
            "minCellWidth": crtNativePath ? 72 : 160,
            "preferredPageSize": crtNativePath ? 6 : 10,
            "minColumns": 2,
            "maxColumns": crtNativePath ? 3 : 5,
            "minRows": 2,
            "maxRows": crtNativePath ? 3 : 5
        };
    }

    function pctH(percent: real): int {
        return Math.round((swapPercentageAxes ? screenWidth : screenHeight) * percent / 100);
    }

    function pctW(percent: real): int {
        return Math.round((swapPercentageAxes ? screenHeight : screenWidth) * percent / 100);
    }

    function px(value: real): int {
        return Math.round(value);
    }

    function stroke(value: real): int {
        return Math.max(1, px(value));
    }

    function center(parentSize: real, childSize: real): int {
        return px((parentSize - childSize) / 2);
    }

    function half(value: real): int {
        return px(value / 2);
    }

    function gamesGridShape(viewportWidth: int, viewportHeight: int): var {
        return root._selectGridShape(viewportWidth, viewportHeight, root._gamesGridConfig);
    }

    // Cover decode tiers, mirroring Core's resize ladder. A cover's source
    // size must be one of these so the QML-decoded texture matches the WebP
    // Core delivers (no resample) and small resolution wobble does not move the
    // tier. Snaps up to the smallest tier that covers `px`.
    function snapCoverTier(px: real): int {
        if (px <= 128)
            return 128;
        if (px <= 256)
            return 256;
        if (px <= 512)
            return 512;
        return 768;
    }

    // Raw painted height (px) of a games-grid cover in caption mode: the cell
    // height minus the top padding and the bottom caption band. Box art is
    // portrait, so this height is the bounding side. Pure function of the
    // resolution — the same value for every grid tile — so callers use it as a
    // stable Image.sourceSize input rather than the live painted height, which
    // fluctuates per layout/recycle and would make Qt reload the Image on every
    // change. The percentage bands track Tile.qml's _padding / _captionHeight /
    // _captionGap; exact agreement is not required since the result is snapped.
    // qmllint disable compiler
    function _gamesGridCoverBox(viewportWidth: int, viewportHeight: int): int {
        const shape = gamesGridShape(viewportWidth, viewportHeight);
        const tileHeight = Math.ceil(Math.max(1, viewportHeight) / Math.max(1, shape.rows));
        const coverBox = tileHeight - pctH(2) - (pctH(5.5) + pctH(0.4));
        return Math.max(1, coverBox);
    }
    // qmllint enable compiler

    // Stable per-view cover decode size (px), snapped to a Core tier. One source
    // of truth for both the Core fetch request (Main.qml) and the grid tile's
    // Image.sourceSize (Tile.qml), so request size == decode size.
    function gamesGridCoverSourceSize(viewportWidth: int, viewportHeight: int): int {
        return snapCoverTier(_gamesGridCoverBox(viewportWidth, viewportHeight));
    }

    // Detail-pane cover decode size: ~2x the grid cover, snapped to its own
    // tier. The detail view paints one large cover, so it lands a tier above the
    // grid (e.g. 768 vs 512 at 1080p), keeping the two views distinct. A future
    // metadata modal adds its own ...CoverSourceSize here.
    //
    // Capped at the largest tier the viewport can actually express: on a
    // CRT-native scene (~316 px wide after safe-area insets) the doubled
    // tier used to request 512-wide decodes that the framebuffer can never
    // display. Each such decode costs a resample step and a ~1.1 MB
    // decoded-cache entry, so the 64 MB cache held only ~57 covers and
    // list scrolling re-decoded on every pass. The cap keys off the live
    // viewport, so HDMI and desktop scenes (>=768 px wide) are unchanged.
    function detailCoverSourceSize(viewportWidth: int, viewportHeight: int): int {
        const doubled = snapCoverTier(_gamesGridCoverBox(viewportWidth, viewportHeight) * 2);
        return Math.min(doubled, maxExpressibleCoverTier(viewportWidth));
    }

    // Viewport the detail-cover tier is computed from. Defaults to the
    // full scene; Main.qml binds these to the games-grid viewport (the
    // scene minus header strips and margins) so the Core fetch request
    // and the QML decode tier consume identical dimensions — if they
    // ever diverged, a fetched tier could mismatch the decode tier and
    // miss the pixmap cache.
    property real detailCoverViewportWidth: screenWidth
    property real detailCoverViewportHeight: screenHeight

    // Live detail-cover decode width for Image.sourceSize consumers
    // (detail panes and their neighbour-prefetch pools). One binding so
    // every consumer decodes at the same width — a prefetch at a
    // different sourceSize would populate a different pixmap-cache
    // entry and never be hit by the visible cover.
    readonly property int detailCoverSourceWidth: detailCoverSourceSize(detailCoverViewportWidth, detailCoverViewportHeight)

    // Largest cover tier that is not wider than the viewport itself: a
    // decode wider than the scene can only ever be shown downscaled, so
    // requesting it wastes resample time and decoded-cache bytes.
    function maxExpressibleCoverTier(viewportWidth: int): int {
        if (viewportWidth >= 768)
            return 768;
        if (viewportWidth >= 512)
            return 512;
        if (viewportWidth >= 256)
            return 256;
        return 128;
    }

    function systemsGridShape(viewportWidth: int, viewportHeight: int): var {
        return root._selectGridShape(viewportWidth, viewportHeight, root._systemsGridConfig);
    }

    function _gridConfig(base: var, overrides: var): var {
        const merged = {};
        for (const key in base)
            merged[key] = base[key];
        for (const key in overrides)
            merged[key] = overrides[key];
        return merged;
    }

    // qmllint disable compiler
    function _selectGridShape(viewportWidth: int, viewportHeight: int, options: var): var {
        const safeWidth = Math.max(1, viewportWidth);
        const safeHeight = Math.max(1, viewportHeight);
        let bestColumns = options.minColumns;
        let bestRows = options.minRows;
        let bestScore = Number.MAX_VALUE;

        for (let columns = options.minColumns; columns <= options.maxColumns; columns++) {
            const cellWidth = safeWidth / columns;
            if (cellWidth < options.minCellWidth)
                continue;
            for (let rows = options.minRows; rows <= options.maxRows; rows++) {
                const cellHeight = safeHeight / rows;
                if (cellHeight < options.minCellHeight)
                    continue;
                const aspect = cellWidth / cellHeight;
                const aspectError = Math.abs(Math.log(aspect / options.targetAspect));
                const pagePenalty = Math.abs((columns * rows) - options.preferredPageSize) * 0.04;
                const score = aspectError + pagePenalty;
                if (score < bestScore) {
                    bestScore = score;
                    bestColumns = columns;
                    bestRows = rows;
                }
            }
        }

        return {
            "columns": bestColumns,
            "rows": bestRows
        };
    }
    // qmllint enable compiler

    // Minimum 8px to remain legible on CRT 240p displays.
    function fontSize(percent: real): int {
        const size = Math.max(8, pctH(percent));
        if (!crtNativePath)
            return size;
        return size < 12 ? 8 : 16;
    }
}
