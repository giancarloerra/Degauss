// Zaparoo Frontend
// Copyright (c) 2026 Wizzo Pty Ltd and the Zaparoo Project contributors.
// SPDX-License-Identifier: LicenseRef-PolyForm-Noncommercial-1.0.0

import QtQuick
import QtTest
import Zaparoo.Theme
import Zaparoo.App

// Resolution-agnostic sizing contract: pctH/pctW/fontSize must scale with the
// Main window's screenWidth/screenHeight, and visibleCovers must honour the
// 240p special-case (3 covers instead of 5).
TestCase {
    name: "UiSizing"
    when: windowShown

    Main {
        id: main
        fullScreen: false
        width: 1280
        height: 720
    }

    function cleanup(): void {
        main.debugCrtSafeAreaOverlay = false;
        main.crtNativePath = false;
    }

    function setResolution(w: int, h: int): void {
        setResolutionExpect(w, h, w, h);
    }

    function setResolutionExpect(w: int, h: int, expectedW: int, expectedH: int): void {
        main.width = w;
        main.height = h;
        // Main.qml's onWidthChanged/onHeightChanged propagate to Sizing.
        tryCompare(Sizing, "screenWidth", expectedW);
        tryCompare(Sizing, "screenHeight", expectedH);
    }

    function crtSafeWidth(w: int): int {
        return w - 2 * Math.round(w * 0.05);
    }

    function crtSafeHeight(h: int): int {
        return h - 2 * Math.round(h * 0.05);
    }

    function test_pct_helpers_scale_with_window_size(): void {
        setResolution(1920, 1080);
        compare(Sizing.pctH(10), 108);
        compare(Sizing.pctW(50), 960);
        compare(Sizing.pctH(100), 1080);

        setResolution(1280, 720);
        compare(Sizing.pctH(10), 72);
        compare(Sizing.pctW(50), 640);

        setResolution(320, 240);
        compare(Sizing.pctH(10), 24);
        compare(Sizing.pctW(50), 160);
    }

    function test_font_size_respects_minimum_for_240p(): void {
        setResolution(320, 240);
        // pctH(2) would be 5 at 240p, but fontSize clamps to 8.
        verify(Sizing.fontSize(2) >= 8, "fontSize must never fall below 8px for CRT legibility");
        // A larger percent still scales above the floor.
        compare(Sizing.fontSize(10), 24);
    }

    function test_visible_covers_drops_to_three_on_240p(): void {
        setResolution(320, 240);
        compare(Sizing.visibleCovers, 3);

        setResolution(1280, 720);
        compare(Sizing.visibleCovers, 5);

        setResolution(1920, 1080);
        compare(Sizing.visibleCovers, 5);
    }

    function test_debug_crt_safe_area_guide_visibility(): void {
        main.debugCrtSafeAreaOverlay = false;
        main.crtNativePath = true;
        setResolutionExpect(320, 240, crtSafeWidth(320), crtSafeHeight(240));
        compare(main._debugCrtSafeAreaGuideVisible, false);

        main.debugCrtSafeAreaOverlay = true;
        compare(main._debugCrtSafeAreaGuideVisible, true);

        main.crtNativePath = false;
        compare(main._debugCrtSafeAreaGuideVisible, false);

        main.crtNativePath = true;
        setResolutionExpect(640, 480, crtSafeWidth(640), crtSafeHeight(480));
        compare(main._debugCrtSafeAreaGuideVisible, false);

        setResolutionExpect(640, 288, crtSafeWidth(640), crtSafeHeight(288));
        compare(main._debugCrtSafeAreaGuideVisible, true);

        main.debugCrtSafeAreaOverlay = false;
        main.crtNativePath = false;
    }

    function test_sizing_updates_propagate_proportionally(): void {
        setResolution(1280, 720);
        var baseline = Sizing.pctH(10);
        compare(baseline, 72);

        setResolution(1920, 1080);
        var scaled = Sizing.pctH(10);
        // 1080/720 = 1.5 → 72 * 1.5 = 108. Allow ±1px for rounding.
        verify(Math.abs(scaled - baseline * 1.5) <= 1, "pctH scaling should track screen height proportionally");
    }

    function test_detail_cover_tier_capped_by_viewport_width(): void {
        // Pin the resolution through the Main harness first: the cover-box
        // math reads the singleton's live screen size for its paddings, so
        // asserting against detached argument pairs would depend on test
        // order.
        //
        // CRT-native scene: 352x240 minus the 5% safe-area insets. The
        // doubled detail tier must not exceed what the framebuffer can
        // express -- a 512-wide decode can never be displayed at 512 on a
        // 352-wide mode and only wastes resample time and decoded-cache
        // bytes.
        main.crtNativePath = true;
        setResolutionExpect(352, 240, crtSafeWidth(352), crtSafeHeight(240));
        verify(Sizing.detailCoverSourceSize(Sizing.screenWidth, Sizing.screenHeight) <= 256,
               "CRT detail tier must not exceed the viewport-expressible tier");
        compare(Sizing.detailCoverSourceWidth,
                Sizing.detailCoverSourceSize(Sizing.detailCoverViewportWidth,
                                             Sizing.detailCoverViewportHeight),
                "decode width must equal the tier for its bound viewport");

        // Wider scenes keep the historical behavior: at 1080p the doubled
        // grid tier lands on the top tier, uncapped.
        main.crtNativePath = false;
        setResolution(1920, 1080);
        compare(Sizing.detailCoverSourceSize(1920, 1080), 768,
                "1080p detail tier must stay the top tier");

        // Restore the harness default so later tests stay order-independent.
        setResolution(1280, 720);
    }

    function test_crt_systems_grid_is_three_by_three(): void {
        Sizing.crtNativePath = true;
        setResolution(352, 240);
        compare(Sizing.systemsGridColumns, 3);
        compare(Sizing.systemsGridRows, 3);

        setResolution(352, 288);
        compare(Sizing.systemsGridColumns, 3);
        compare(Sizing.systemsGridRows, 3);
        Sizing.crtNativePath = false;
    }
}
