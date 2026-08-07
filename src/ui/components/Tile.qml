// Zaparoo Frontend
// Copyright (c) 2026 Wizzo Pty Ltd and the Zaparoo Project contributors.
// SPDX-License-Identifier: LicenseRef-PolyForm-Noncommercial-1.0.0
// Unified grid tile. Solid card with a centered icon area filling the
// card body, plus an accent-coloured outline ring around the card when
// this tile is the focused selection. Used by every tile surface in the frontend
// — hub categories row, systems grid, games grid, recents grid — so
// the vocabulary is identical across screens.
// Two layout modes, gated by `showCaption`:
//   - off (default): full-bleed icon, no in-tile label. Used by Hub
//     and Systems where a curated logo already carries identity.
//   - on: cover slot shrinks vertically to free a thin band along the
//     bottom edge for a one-line elided name caption. Used by Games
//     and Recents because a long shelf of similar boxart needs
//     per-tile labelling — the focused-tile caption below the grid
//     (ActiveLabel) only identifies one cell at a time.
// In caption mode the loading-state fallback is an hourglass glyph,
// not the wrapping-name text used in non-caption mode — the bottom
// caption already shows the name, so the centred-text fallback would
// just read it twice.
// Parent contract — Tile must be loaded inside a host that exposes:
//   - isSelected: bool   — true when this tile is the focused selection
//   - isFocused:  bool   — true when the section owning this tile has user focus
//   - name:       string — model display name (used by the procedural
//                          fallback while the cover PNG decodes)
//   - coverKey:   string — relative path under resources/images/ (no extension)
//   - favorite:   int    — optional 0/1; shows a small heart badge when 1

import QtQuick
import Zaparoo.Theme

// PagedGrid.qml and HubScreen's static category row both wrap their
// Tile delegate in a TileLoader that defines the required properties
// above; QML's late-binding model means a caller that forgets one
// fails silently at runtime rather than at build time, so the
// Component.onCompleted check below converts that footgun into a
// loud warning.
Item {
    // Do NOT add `layer.enabled` here. On Qt's software adaptation
    // it allocates a per-item QImage backing store, blits it into
    // the parent on every paint (extra memcpy, not the cached-blit
    // win the docs imply for hardware rendering), and its
    // compositing path with translucent siblings/parents differs
    // from the direct-paint path — visible as flicker on focus
    // moves and lost transparency around the focus ring.
    // `layer.enabled` is documented for scene graph (GPU) rendering;
    // on the MiSTer software target it is a regression, not an
    // optimization.
    // qmllint enable missing-property compiler
    // No persistent focus scale. The earlier 1.06 scale held on every
    // focused tile forced a bilinear resample of the cover pixmap on
    // every d-pad move and overflowed the cell by ~3% on each side,
    // dirtying strips of up to four neighbours per press. Under Qt's
    // software adaptation on MiSTer that read as choppy navigation on
    // covered grids. That scale was a persistent, per-focus-move cost
    // across the whole visible grid.
    //
    // The activate/launch animations below are a different cost class:
    // a one-shot transient triggered on a single tile at the moment of
    // activation. The dirty rect is bounded to one cell for ~90-360ms
    // total and neighbours are unaffected. See `docs/qml-gotchas.md` →
    // "Software-renderer animation costs" for the full distinction.
    // Bottom caption strip (caption mode only). Single line, ellipsised
    // when long. Tints to `textPrimary` on the focused tile so the
    // selection reads at a glance even when the focus outline ring is
    // outside the eye's centre — matches the procedural fallback's
    // focus tint above.

    id: root

    // Focus ring colour. Selection yellow by default; the Hub's home
    // tiles override it to the state teal so all three wordmark colours
    // live on screen with distinct roles.
    property color focusRingColor: Theme.focusContainer

    // qmllint disable missing-property compiler
    readonly property bool delegateIsSelected: parent.isSelected
    readonly property bool delegateIsFocused: parent.isFocused
    readonly property string delegateName: parent.name
    readonly property string delegateCoverKey: parent.coverKey
    readonly property bool delegateFavorite: parent.favorite !== 0
    // qmllint disable missing-property compiler
    readonly property bool delegateHidden: parent.hidden === true
    // qmllint disable missing-property compiler
    // Sibling-diffed disambiguating-tag display string (region, disc, rev, ...).
    // Empty for items with no variants. Rendered as a dim inline suffix after
    // the name in the bottom caption (see ScrollingCaption), identically on the
    // default and CRT paths.
    readonly property string delegateDisambiguatingTags: parent.disambiguatingTags ?? ""
    // Pulse counter forwarded by TileLoader — increment to trigger the
    // push-in cue on the focused tile. Every button-like action (folder
    // drill-in, system select, game launch) shares this single cue, so
    // there is no separate launch animation. Default to 0 so hosts that
    // do not wire it are silently no-ops.
    readonly property int delegateActivatePulse: parent.activatePulse ?? 0
    // Release counter forwarded by TileLoader — increment to settle the
    // push-in cue back to rest (scale 1.0). Fired only after a launch that
    // keeps the frontend on the same screen (e.g. an Audio track that does
    // not take the FPGA); forward navigation never fires it because the
    // screen transition + `settling` already reset the held scale off-screen.
    // Defaults to 0 so hosts that do not wire it are silently no-ops.
    readonly property int delegateReleasePulse: parent.releasePulse ?? 0
    // `settling` is set true by the host screen when the screen becomes
    // inactive (off-screen). Used to reset a held push-in scale so the
    // tile is back at 1.0 before the screen is shown again.
    readonly property bool delegateSettling: parent.settling ?? false
    // `focusReady` gates whether this tile renders its focused styling at all
    // (ring + focused cover ramp). The host leaves it false until the screen's
    // focus index is finalized
    // (programmatic restore or first input). Before that, a tile that happens
    // to sit at the default index must not paint a ring, or the wrong tile
    // flashes focused for the frames before restore corrects the index.
    // Defaults true for hosts that do not wire it.
    readonly property bool delegateFocusReady: parent.focusReady ?? true
    // qmllint enable missing-property
    property var layoutProfile: null
    readonly property var _surfaceProfile: root.layoutProfile && root.layoutProfile.surface ? root.layoutProfile.surface : null
    // Opt-in per-tile name caption. Off by default so Hub and Systems
    // keep their full-bleed logo layout. Cover-art screens (Games,
    // Recents) flip this on at the delegate template.
    property bool showCaption: false
    // Decode size (px) for real cover art, set by the owning screen to its
    // per-view, resolution-derived tier (see Sizing.…CoverSourceSize). Held
    // stable while browsing so Qt's pixmap cache short-circuits reloads instead
    // of re-decoding; default 256 matches the built-in tinted-asset raster.
    property int coverSourceSize: 256
    // Equal cover padding on top, left, and right — the bottom is
    // owned by the caption strip in caption mode and matches `_padding`
    // visually in non-caption mode. pctH(2) is enough to read as
    // deliberate breathing room without giving back much cover area.
    // Below the cover sits the caption flush against the card's
    // bottom edge, separated from the cover by `_captionGap`.
    readonly property int _padding: Sizing.pctH(2)
    readonly property int _outlineGap: Sizing.pctH(0.4)
    readonly property int _outlineWidth: Sizing.stroke(Sizing.pctH(0.6))
    readonly property int _captionHeight: Sizing.pctH(5.5)
    readonly property int _captionGap: Sizing.pctH(0.4)
    readonly property int _captionTextSize: Sizing.fontSize(2.2)
    readonly property int _tileCornerRadius: root._surfaceProfile ? root._surfaceProfile.cornerRadius : Sizing.cornerRadius
    // Width available to the bottom caption. A half-corner-radius inset on each
    // side keeps glyphs clear of the rounded corners while giving the title a
    // bit more room than the old full-radius inset. ScrollingCaption does its
    // own measuring/eliding/marquee inside this width.
    readonly property int _captionSideInset: Sizing.half(root._tileCornerRadius)
    readonly property int _captionTextMaxWidth: Math.max(0, root.width - 2 * root._captionSideInset)

    // Focused styling (ring + focused cover ramp) is withheld until the host
    // marks focus ready via `delegateFocusReady`. This keeps a default-index
    // tile from painting a ring during the window between first paint and the
    // programmatic restore that finalizes the real selection — the source of
    // the load-time "wrong tile flashes focused" bug. The focus ring snaps on
    // and off with selection; the only per-tile motion is the push-in cue.
    readonly property bool _focusedSelection: root.delegateIsSelected && root.delegateIsFocused && root.delegateFocusReady
    // `coverKey` is the relative path under `resources/images/` without
    // extension — `systems/snes`, `categories/Consoles`, etc. The model
    // chooses the subdirectory; Tile is agnostic. Resources.coverUrl is
    // the single source of truth for the qrc layout — see Resources.qml.
    //
    // The model's `icons/Loading` sentinel is a special case: it means
    // "cover fetch is in flight". Routing it through the full-bleed
    // cover slot would rasterise the SVG at the entire icon area; the
    // existing `loadingGlyph` overlay below already defines the
    // standard centred hourglass size, so swallow the source here and
    // let `loadingGlyph` own the painting.
    readonly property bool _coverPending: root.delegateCoverKey === "icons/Loading"
    readonly property bool _systemCover: root.delegateCoverKey.startsWith("systems/")
    // True for any built-in icon routed through the tinted-svg provider:
    // system logos, hub category icons, folder/file/action UI glyphs.
    // False for real art (media-image/, custom-image/) which is never recolored
    // — a user override is shown exactly as it is on disk.
    readonly property bool _isTinted: root.delegateCoverKey.startsWith("systems/") || root.delegateCoverKey.startsWith("categories/") || root.delegateCoverKey.startsWith("icons/")
    // True for real raster cover art (a fetched media/custom image) as opposed
    // to the built-in tinted vector assets (system logos, category/UI glyphs).
    // Drives the cover decode size below; defined on the art's own identity
    // (its key prefix), not on `_isTinted`, so the decode policy stays correct
    // independently of theme-tint behavior.
    readonly property bool _coverIsRealArt: root.delegateCoverKey.startsWith("media-image/") || root.delegateCoverKey.startsWith("custom-image/")
    // Real raster art still in flight: the key is media/custom art but the
    // Image has not reached a terminal state. Paired with _coverPending below
    // so the busy state stays continuously true across the model's
    // pending-sentinel -> real-key handoff (both derive from delegateCoverKey
    // and flip together), which is what keeps the loading cue from blinking.
    readonly property bool _coverMediaImagePending: root._coverIsRealArt && coverBase.status !== Image.Ready && coverBase.status !== Image.Error
    // Single combined "waiting on a cover" predicate. Mirrors
    // BrowseDetailPane._coverBusy — the loading cue is gated on this one value
    // so an internal state change mid-wait never resets the debounce.
    readonly property bool _coverBusy: root._coverPending || root._coverMediaImagePending || coverBase.status === Image.Loading
    // Unfocused ramp — always loaded for tinted keys; also the sole source for
    // real art (media-image/, custom-image/) which is focus-independent.
    readonly property url _coverBaseSrc: root._coverPending ? "" : Resources.coverUrl(root.delegateCoverKey, Theme.logoPrimary, Theme.logoSecondary, Theme.logoShadow)
    // Focused ramp — only loaded for tinted icons; empty string for real art so
    // the Image item never initiates a second fetch for cover/boxart tiles.
    readonly property url _coverFocusSrc: (root._isTinted && !root._coverPending) ? Resources.coverUrl(root.delegateCoverKey, Theme.logoFocusPrimary, Theme.logoFocusSecondary, Theme.logoFocusShadow) : ""
    // True once the focused ramp is decoded and this tile is the focused
    // selection — used to suppress coverBase so the two renders don't stack
    // (which would double the effective opacity on hidden tiles).
    readonly property bool _focusCoverActive: root._focusedSelection && root._isTinted && coverFocus.status === Image.Ready
    // Show the procedural name fallback only when the icon genuinely failed
    // to load (no such logo), never while it is merely decoding. During the
    // Loading/Null window the slot stays blank so the name does not flash in
    // before the icon pops in. coverBase always has a real source when not
    // _coverPending, so it reliably reaches Ready or Error.
    readonly property bool _fallbackVisible: !root.showCaption && !root._coverPending && coverBase.status === Image.Error
    readonly property int _fallbackTextSize: root._systemCover ? Sizing.fontSize(5.8) : Sizing.fontSize(2.4)
    readonly property int _fallbackMinimumTextSize: root._systemCover ? Sizing.fontSize(2.8) : Sizing.fontSize(2.4)
    readonly property bool _startupTraceResource: root.delegateCoverKey.startsWith("categories/") || root.delegateCoverKey === "icons/PlayOutline" || root.delegateCoverKey === "icons/HeartOutline" || root.delegateCoverKey === "icons/History" || root.delegateCoverKey === "icons/Settings"
    property double _startupTraceLoadStartedAt: 0
    // Loading-cue debounce. A QML Image always passes through Image.Loading for
    // ~1 frame before Ready, even when the media-image provider returns a cached
    // cover in ~0.1ms, which would flash the hourglass on every cached tile.
    // Gated on _coverBusy (not raw Image status) so the cue stays solid across
    // the pending->loading handoff; only flips true once a wait outlasts the
    // delay, so instant cached covers never show it. Mirrors BrowseDetailPane.
    property bool _coverLoadingDelayElapsed: false

    on_CoverBusyChanged: root._updateCoverLoadingDelay()

    Timer {
        id: coverLoadingDelayTimer

        interval: 150
        repeat: false
        onTriggered: root._coverLoadingDelayElapsed = root._coverBusy
    }

    function _updateCoverLoadingDelay(): void {
        coverLoadingDelayTimer.stop();
        root._coverLoadingDelayElapsed = false;
        if (!root._coverBusy)
            return;
        coverLoadingDelayTimer.restart();
    }

    anchors.fill: parent
    // One-shot push-in scale, shared by every button-like action. The
    // persistent 1.06 focus scale was removed; this is a bounded transient
    // on a single tile at activation time. See the comment above for the
    // cost-profile distinction.
    property real _activateScale: 1.0
    // Public read for siblings that must track this scale (e.g.
    // PagedGrid's placeholderCard, which sits behind TileLoader).
    readonly property real cardScale: root._activateScale
    scale: root._activateScale
    transformOrigin: Item.Center

    // Fire the push-in cue only for a genuine activation, never as a
    // side effect of delegate creation. A freshly built delegate sees its
    // `delegateActivatePulse` resolve from the `?? 0` fallback up to the
    // current pulse, which fires this handler once during construction. When
    // such a delegate is momentarily the focused selection (e.g. the Settings
    // category grid rebuilt with a stale currentIndex on a page switch), that
    // spurious change would restart the animation and leave the wrong tile
    // pushed in. `_mounted` flips true one event-loop pass after completion,
    // after every construction-time transient has settled, so only real pulse
    // increments (always delivered well after mount) play the cue.
    property bool _mounted: false
    onDelegateActivatePulseChanged: {
        if (root._mounted && root._focusedSelection)
            activateAnim.restart();
    }

    // Settle the held push-in back to rest. Used when the activation kept us
    // on the same screen (a launcher that did not take the FPGA), so the cue
    // does not stay stuck pushed in. The `_mounted` guard ignores any
    // construction-time pulse transient, matching the activate handler above.
    onDelegateReleasePulseChanged: {
        // Only the focused selection can be holding a push-in, so only it needs
        // to settle back — matches the activate handler's gate and avoids
        // starting a redundant animation on every tile in the grid.
        if (root._mounted && root._focusedSelection) {
            activateAnim.stop();
            releaseAnim.restart();
        }
    }

    // Bleed guard — stop and reset if the delegate is rebound to a
    // different entry while an animation is in flight (rapid-scroll +
    // accept on the same frame). delegateName changes only on a genuine
    // content rebind, not on cover-load completion, so this never cuts
    // a legitimate same-tile cue short. DeferredAction's lead means the
    // cue usually completes before teardown anyway; this is belt-and-suspenders.
    onDelegateNameChanged: {
        activateAnim.stop();
        root._activateScale = 1.0;
    }

    // Reset scale when the host screen goes inactive so the tile is at
    // 1.0 before it is shown again. The single-leg push-in holds at
    // pressScale; without this reset, returning to a screen with a
    // persistent delegate would show a permanently shrunken tile.
    onDelegateSettlingChanged: {
        if (root.delegateSettling) {
            activateAnim.stop();
            root._activateScale = 1.0;
        }
    }

    // Push in and hold — the single cue for every button-like action,
    // whether the press navigates forward or launches a game. The screen
    // changes while the tile is held at pressScale; the host screen's
    // `settling` flag resets scale to 1.0 off-screen so the tile is clean
    // when the user returns. No return-to-normal leg — the user is
    // navigating away; a bounce-back was visible and unwanted because the
    // source screen is held visible for the full 300 ms deferred-flip grace.
    NumberAnimation {
        id: activateAnim
        target: root
        property: "_activateScale"
        to: Motion.pressScale
        duration: Motion.dur(Motion.pressMs)
        easing.type: Easing.OutQuad
    }

    // Release leg — settles the held push-in back to rest after a launch that
    // stays on the page. Stops `activateAnim` first so a release that lands
    // mid-push does not fight it. See `onDelegateReleasePulseChanged`.
    NumberAnimation {
        id: releaseAnim
        target: root
        property: "_activateScale"
        to: 1.0
        duration: Motion.dur(Motion.settleMs)
        easing.type: Easing.OutQuad
    }

    Component.onCompleted: {
        // Self-check the parent contract. Logs once at construction so
        // a future caller that drops Tile into a non-conforming wrapper
        // sees the failure mode immediately instead of debugging
        // mysteriously empty tiles.
        // qmllint disable missing-property compiler
        if (typeof parent.isSelected !== "boolean" || typeof parent.isFocused !== "boolean" || typeof parent.name !== "string" || typeof parent.coverKey !== "string")
            console.warn("Tile: parent does not satisfy the delegate contract " + "(expected isSelected:bool, isFocused:bool, " + "name:string, coverKey:string)");
        // Defer one event-loop pass so construction-time activate-pulse
        // transients (see onDelegateActivatePulseChanged) do not fire the cue.
        Qt.callLater(() => {
            root._mounted = true;
        });
    }

    function _startupTrace(stage: string, details: string): void {
        if (!root._startupTraceResource)
            return;
        let node = root.parent;
        while (node) {
            if (typeof node._startupTrace === "function") {
                node._startupTrace(stage, "coverKey=" + root.delegateCoverKey, details);
                return;
            }
            node = node.parent;
        }
        console.debug(stage + " coverKey=" + root.delegateCoverKey + " " + details);
    }

    // Tile body. Solid card so the white icon has a high-contrast
    // surface. Always visible — no opacity gating — which is the
    // unified-Tile contract: every grid renders the same shape.
    // Static 1px borderMid edge gives every tile a card edge whether
    // focused or not — same depth cue settings rows carry. The accent
    // focus ring still paints on top when this tile is the focused
    // selection.
    Rectangle {
        anchors.fill: parent
        radius: root._tileCornerRadius
        color: Theme.surfaceCard
        border.color: Theme.borderMid
        border.width: Sizing.stroke(1)
    }

    // Focus outline ring. Drawn *inside* the card edge so the ring
    // never bleeds past the cell bounds — that's the project standard:
    // borders/outlines stay within their parent rather than overflowing
    // it. Keeps the ring out of PagedGrid's clip rect at the row edges
    // and means callers don't have to reserve bleed room for it. Gated
    // on `_focusedSelection` so only the focused tile in the focused
    // section lights up — keeps multiple tile sections on screen from
    // competing for the eye. Drawn after the card so the border sits on
    // top; the icon padding (`_padding = pctH(3)`) is far larger than
    // the inset (`_outlineGap = pctH(0.4)`), so the ring never overlaps
    // content.
    // Focus ring drawn as two stacked *filled* rounded rectangles — an
    // outer accent pill and an inner surfaceCard mask that punches the
    // centre back, leaving a uniform outline. Equivalent to the older
    // single-Rectangle `border.color` + `border.width` approach but
    // significantly smoother on the corners under Qt's software
    // adaptation: filled rounded rects honour the AA path, while thin
    // rounded *borders* are tessellated without subpixel coverage and
    // step visibly at the corners (see QTBUG-123210). Both rectangles
    // are still inside the card edge by `_outlineGap`, so the ring
    // never bleeds past the cell bounds. The ring is an accent rect with a
    // surface-colored inner mask punched out of its center; both snap on and
    // off with `_focusedSelection` (no fade).
    Rectangle {
        id: focusRingOuter

        anchors.fill: parent
        anchors.margins: root._outlineGap
        color: root.focusRingColor
        radius: Math.max(0, root._tileCornerRadius - root._outlineGap)
        antialiasing: true
        visible: root._focusedSelection
    }

    Rectangle {
        anchors.fill: focusRingOuter
        anchors.margins: root._outlineWidth
        color: Theme.surfaceCard
        // Inner radius shrinks to keep the visible ring's outer edge
        // and inner edge concentric with the card corners. Floor at 0
        // so very small tiles (where _outlineWidth approaches the
        // outer radius) collapse to a sharp inner mask rather than
        // negative-radius garbage.
        radius: Math.max(0, focusRingOuter.radius - root._outlineWidth)
        antialiasing: true
        visible: root._focusedSelection
    }

    // Icon area — two stacked Images for the unfocused and focused tint ramps.
    // Both share identical geometry; `coverFocus` sits above `coverBase` (z: 1)
    // and is only loaded for tinted keys (system logos, category icons, UI
    // glyphs). Real art (media-image/, custom-image/) uses only `coverBase`.
    //
    // Focus transitions are an instant visibility swap with zero async work:
    // both ramps are decoded while the tile is idle (coverBase during the
    // prefetch gate; coverFocus as soon as it enters the visible delegate pool),
    // so moving the cursor never re-requests the SVG render or drops to the
    // procedural Text fallback.
    //
    // `_focusCoverActive` suppresses coverBase when the focused ramp is on top,
    // preventing the two opaque layers from stacking their alpha on hidden tiles.
    Image {
        id: coverBase

        width: parent.width - 2 * root._padding
        source: root._coverBaseSrc
        // Cover decode size, split by what the image IS (not whether it is
        // tinted). Real raster art decodes at the per-view `coverSourceSize` the
        // owning screen supplies — box art is portrait, so height is the bounding
        // side. That value is a stable, resolution-derived tier, NOT the live
        // painted height: Qt reloads an Image whenever sourceSize changes ("Avoid
        // changing this property dynamically"), so binding it to the fluctuating
        // box height (per layout pass, recycle, and grid retention revisit) would
        // re-decode the same cover many times over. A constant requestedSize lets
        // every reload short-circuit to the pixmap cache. Built-in tinted vector
        // assets (system logos, category/UI glyphs) instead pin a fixed 256 px
        // raster: the same logo appears at different sizes across screens, and
        // the tinted-svg image provider's cache (16 MB cap, keyed id+size) would
        // churn if each screen requested its own size — the fixed raster
        // consolidates every tile to one cache entry per logo.
        sourceSize.width: root._coverIsRealArt ? 0 : 256
        sourceSize.height: root._coverIsRealArt ? root.coverSourceSize : 0
        fillMode: Image.PreserveAspectFit
        smooth: true
        asynchronous: true
        // Hide when the focused ramp is fully decoded and showing on top; the
        // normal hidden-item dim (0.4) is still applied so the two renders are
        // never stacked at the same opacity simultaneously.
        opacity: (coverBase.status === Image.Ready && !root._focusCoverActive) ? (root.delegateHidden ? 0.4 : 1.0) : 0

        anchors {
            top: parent.top
            topMargin: root._padding
            bottom: parent.bottom
            // In caption mode the cover sits above the bottom caption strip with
            // only `_captionGap` of breathing room. The caption is flush against
            // the card's bottom edge, so the cover's lower bound is just
            // (caption height + gap) — no second layer of card padding below.
            bottomMargin: root.showCaption ? root._captionHeight + root._captionGap : root._padding
            horizontalCenter: parent.horizontalCenter
        }

        onStatusChanged: {
            if (!root._startupTraceResource)
                return;
            if (status === Image.Loading) {
                root._startupTraceLoadStartedAt = Date.now();
                root._startupTrace("startup/qml resource load start", "source=" + source);
            } else if (status === Image.Ready) {
                const durMs = root._startupTraceLoadStartedAt > 0 ? Math.max(0, Date.now() - root._startupTraceLoadStartedAt) : 0;
                root._startupTrace("startup/qml resource load ready", "source=" + source, "dur_ms=" + durMs, "paintedWidth=" + width, "paintedHeight=" + height);
                root._startupTraceLoadStartedAt = 0;
            } else if (status === Image.Error) {
                const durMs = root._startupTraceLoadStartedAt > 0 ? Math.max(0, Date.now() - root._startupTraceLoadStartedAt) : 0;
                root._startupTrace("startup/qml resource load error", "source=" + source, "dur_ms=" + durMs);
                root._startupTraceLoadStartedAt = 0;
            }
        }
    }

    // Focused-ramp variant. Only loaded for tinted keys (_isTinted); source is
    // "" for real art so this Image never initiates a fetch for boxart tiles.
    // Painted on top of coverBase (z: 1); visible only on the focused+selected
    // tile. When not yet decoded (status != Ready) opacity is 0, so coverBase
    // shows through as a fallback unfocused-ramp — no flash to text.
    Image {
        id: coverFocus

        z: 1
        width: coverBase.width
        source: root._coverFocusSrc
        sourceSize.width: 256
        fillMode: Image.PreserveAspectFit
        smooth: true
        asynchronous: true
        visible: root._focusedSelection && root._isTinted
        opacity: coverFocus.status === Image.Ready ? (root.delegateHidden ? 0.4 : 1.0) : 0

        anchors {
            top: parent.top
            topMargin: root._padding
            bottom: parent.bottom
            bottomMargin: root.showCaption ? root._captionHeight + root._captionGap : root._padding
            horizontalCenter: parent.horizontalCenter
        }
    }

    // Caption-mode loading cue. Centred hourglass glyph that paints
    // only during the Image.Loading window — once the cover lands the
    // glyph hides and the cover paints in. Error/Null cover state
    // also hides the glyph (a stuck hourglass on a permanently failed
    // cover would mislead) and the bottom caption still identifies
    // the tile. Bundled qrc asset, decode is cheap, no animation.
    Image {
        id: loadingGlyph

        x: coverBase.x + Sizing.center(coverBase.width, width)
        y: coverBase.y + Sizing.center(coverBase.height, height)
        width: Sizing.pctH(10)
        height: Sizing.pctH(10)
        source: Resources.iconUrl("Loading")
        // Loading.svg has a 24×24 native viewBox; without sourceSize
        // Qt rasterises at that intrinsic size and bilinear-upscales
        // to the rendered box, which reads as soft on every screen
        // taller than ~240 px. Pinning sourceSize to the rendered
        // dimensions makes the SVG renderer rasterise at target size
        // — same pattern StatusIcon.qml and LoadingIndicator.qml use.
        sourceSize.width: Sizing.px(width)
        sourceSize.height: Sizing.px(height)
        fillMode: Image.PreserveAspectFit
        smooth: true
        asynchronous: false
        visible: root.showCaption && root._coverBusy && root._coverLoadingDelayElapsed
    }

    Image {
        id: favoriteGlyph

        anchors.left: parent.left
        anchors.top: parent.top
        anchors.leftMargin: Sizing.px(parent.width / 12)
        anchors.topMargin: Sizing.px(parent.width / 12)
        width: Sizing.px(parent.width / 6)
        height: width
        // Tinted on the fly from theme tokens (fill -> favoriteMarker red,
        // keyline -> bgBar dark outline) via the tinted-svg provider, like every
        // other icon. The source SVG is neutral grayscale; colors live in Theme.
        source: Resources.coverUrl("icons/Heart", Theme.favoriteMarker, Theme.favoriteMarker, Theme.bgBar)
        sourceSize.width: Sizing.px(width)
        sourceSize.height: Sizing.px(height)
        fillMode: Image.PreserveAspectFit
        smooth: true
        asynchronous: false
        visible: root.delegateFavorite
    }

    // User-hidden state badge. It stays fully opaque over dimmed art
    // so hidden tiles remain visually distinct when Show hidden items
    // is enabled.
    TileBadge {
        anchors.right: parent.right
        anchors.top: parent.top
        anchors.rightMargin: Sizing.px(parent.width / 12)
        anchors.topMargin: Sizing.px(parent.width / 12)
        label: qsTr("Hidden")
        visible: root.delegateHidden
    }

    // Non-caption procedural fallback. Sits at the same geometry as the
    // cover and appears only when the icon fails to load (Image.Error), not
    // while it is decoding — the slot stays blank until the icon pops in so
    // the name never flashes in first. Missing system logos use a larger
    // fitted wordmark-style treatment so the tile reads as intentional text
    // artwork, not a broken-image placeholder. In caption mode this is
    // suppressed — the bottom caption already shows the name and the
    // hourglass above signals load progress, so a wrapping copy of the name
    // in this slot is redundant.
    Text {
        anchors.fill: coverBase
        anchors.margins: root._systemCover ? Sizing.pctH(1) : 0
        text: root.delegateName
        font.family: Theme.fontUi
        font.pixelSize: root._fallbackTextSize
        fontSizeMode: root._systemCover ? Text.Fit : Text.FixedSize
        minimumPixelSize: root._fallbackMinimumTextSize
        font.weight: root._systemCover ? Font.DemiBold : Font.Normal
        color: root._isTinted ? (root._focusedSelection ? Theme.logoFocusPrimary : Theme.logoPrimary) : (root._focusedSelection ? Theme.textPrimary : Theme.textLabel)
        // Wrap (not WordWrap): an unbreakable identifier like
        // `_LongCollectionName_Definitive_Cut.smc` would otherwise
        // render past `width` and bleed out of the tile.
        wrapMode: Text.Wrap
        horizontalAlignment: Text.AlignHCenter
        verticalAlignment: Text.AlignVCenter
        renderType: Text.NativeRendering
        opacity: root._fallbackVisible ? (root.delegateHidden ? 0.4 : 1.0) : 0
        clip: true
    }

    // Bottom caption (caption mode only). Single line carrying the name plus an
    // inline dim suffix of disambiguating tokens; ScrollingCaption centers and
    // elides it, pins the top token after the name elides, and marquees the
    // full string while this tile is the focused selection (reduce-motion falls
    // back to a static elide).
    //
    // The strip sits flush at the card's bottom edge so the title visually owns
    // the bottom of the tile. The width clears `cornerRadius` on both sides so
    // glyphs never enter the rounded-corner region. The text lands well inside
    // the focus ring's inner mask zone, so its background stays surfaceCard even
    // on a focused tile. Tints to `textPrimary` on the focused tile so the
    // selection reads at a glance.
    ScrollingCaption {
        id: caption

        x: root._captionSideInset
        y: parent.height - root._captionHeight
        width: root._captionTextMaxWidth
        height: root._captionHeight
        visible: root.showCaption
        centerContent: true
        focused: root._focusedSelection
        name: root.delegateName
        tags: root.delegateDisambiguatingTags
        fontPixelSize: root._captionTextSize
        nameColor: root._focusedSelection ? Theme.textPrimary : Theme.textLabel
    }
}
