// Zaparoo Frontend
// Copyright (c) 2026 Wizzo Pty Ltd and the Zaparoo Project contributors.
// SPDX-License-Identifier: LicenseRef-PolyForm-Noncommercial-1.0.0
pragma ComponentBehavior: Bound

import QtQuick
import Zaparoo.Theme
import Zaparoo.Ui
import Zaparoo.Browse as Browse

// cxx-qt 0.8 patches `isFinal: true` on singleton properties but the
// qmltypes schema has no `isFinal` slot for Method, so every qinvokable
// call on a Zaparoo.Browse singleton (set_resolution) still trips
// qmllint's "Member can be shadowed" check. Until the schema grows
// method-level finality, suppress the compiler category file-wide.
// qmllint disable compiler

// Settings screen — gamepad-driven vertical form. Resolution is MiSTer-only
// because it changes frontend startup video config and applies on restart.
// Button style is cross-platform and selects the resource directory for
// help-bar button glyphs (Style A/B/C/D → resources/images/buttons/{a,b,c,d}/).
// Mouse support is cross-platform and controls cursor visibility plus mouse
// hit targets.
//
// Pure input dispatcher: root rows open category subpages; subpage
// rows open pickers, toggle values, or emit router actions. Escape
// returns from subpage to root, then from root to Hub.
Item {
    id: settings

    Component.onCompleted: console.debug("startup/qml component SettingsScreen completed")

    // Bound by MainLayout to `root.pendingTransition !== ""`. Settings
    // is a destination, never a source, so this is currently always
    // false when the screen is visible — kept for parity with the
    // other screens so the convention holds when a future routing
    // change adds a Settings-as-source path.
    property bool transitioning: false
    property bool optimisticLoading: false

    signal requestHubScreen
    // Forward signal carrying the focused action row's id. The router
    // decides what the payload means — `"uploadLog"` opens the log-
    // upload modal, `"aboutLicense"` navigates to the About screen.
    signal requestAccept(actionId: string)
    // Picker request. The router mounts `ListPickerModal` with these
    // properties and dispatches the user's selection back through the
    // matching `Browse.Settings` setter (keyed off `fieldId`).
    signal requestListPicker(title: string, entries: var, initialId: string, fieldId: string)

    readonly property string pageRoot: "root"
    readonly property string pageDisplayInterface: "displayInterface"
    readonly property string pageBrowsing: "browsing"
    readonly property string pageLanguage: "language"
    readonly property string pageControlsInput: "controlsInput"
    readonly property string pageLibraryData: "libraryData"
    readonly property string pageSupportAbout: "supportAbout"
    property string currentPage: settings.pageRoot
    readonly property bool showingRootGrid: settings.currentPage === settings.pageRoot
    property var _pageIndexes: ({})
    // Incremented when the user accepts a category tile so it plays the
    // push-in animation. Forwarded to all category TileLoaders.
    property int activatePulse: 0
    // Sibling of `activatePulse` for the in-page SettingsField rows: bumped on
    // a field accept so the focused non-toggle row plays its push-in tap.
    // Toggle rows ignore it (their knob slide is the feedback).
    property int fieldActivatePulse: 0
    // True for one event-loop tick during a page switch. Passed as
    // `animateChanges: false` to SettingsField delegates so a reused delegate
    // does not animate its toggle-knob slide when the new page's field model
    // lands. Normal user navigation animates because `_pageSwitching` is false
    // at that point.
    property bool _pageSwitching: false

    // Page-aware field registries. The root mirrors console settings
    // menus: stable domain categories first, short subpages second.
    // Future Core features should land in these domains rather than a
    // vague Advanced bucket.
    readonly property var categoryFields: [
        {
            kind: "field",
            id: "pageDisplayInterface",
            label: qsTr("Display"),
            coverKey: "icons/Display"
        },
        {
            kind: "field",
            id: "pageBrowsing",
            label: qsTr("Browsing"),
            coverKey: "icons/Browsing"
        },
        {
            kind: "field",
            id: "pageLanguage",
            label: qsTr("Language"),
            coverKey: "icons/Language"
        },
        {
            kind: "field",
            id: "pageControlsInput",
            label: qsTr("Controls"),
            coverKey: "icons/Controls"
        },
        {
            kind: "field",
            id: "pageLibraryData",
            label: qsTr("Library"),
            coverKey: "icons/Library"
        },
        {
            kind: "field",
            id: "pageSupportAbout",
            label: qsTr("Support"),
            coverKey: "icons/Support"
        }
    ]
    // Display = video output only. Resolution is MiSTer-only (changes startup
    // video config, applies on restart).
    readonly property var displayInterfaceFields: {
        const out = [];
        if (Browse.Settings.is_mister) {
            out.push({
                kind: "field",
                id: "resolution",
                label: qsTr("Resolution")
            });
        }
        out.push({
            kind: "field",
            id: "orientation",
            label: qsTr("Orientation")
        });
        out.push({
            kind: "field",
            id: "screensaverTimeout",
            label: qsTr("Screensaver")
        });
        if (Browse.Settings.is_mister) {
            // Native CRT video path (Menu fork DDR writer). The toggle is
            // always offered on MiSTer so HDMI users can switch in; the
            // standard picker and calibration screen only matter once the
            // frontend is running with --crt.
            out.push({
                kind: "header",
                label: qsTr("Analog video")
            });
            out.push({
                kind: "field",
                id: "crtEnabled",
                label: qsTr("CRT mode")
            });
            if (Browse.CrtVideo.crt_enabled) {
                out.push({
                    kind: "field",
                    id: "crtVideoStandard",
                    label: qsTr("Video standard")
                });
                out.push({
                    kind: "field",
                    id: "crtCalibration",
                    label: qsTr("Screen position")
                });
            }
        }
        return out;
    }
    // Browsing = how the library is presented and which items show.
    readonly property var browsingFields: [
        {
            kind: "field",
            id: "browseLayout",
            label: qsTr("Browsing layout")
        },
        {
            kind: "field",
            id: "systemLogoStyle",
            label: qsTr("System logos")
        },
        {
            kind: "field",
            id: "theme",
            label: qsTr("Theme")
        },
        {
            kind: "field",
            id: "mediaImageType",
            label: qsTr("Preferred artwork")
        },
        {
            kind: "field",
            id: "showHidden",
            label: qsTr("Show hidden items")
        },
        {
            kind: "field",
            id: "showOriginalFilenames",
            label: qsTr("Show original filenames")
        }
    ]
    // Language = locale/regional preferences.
    readonly property var languageFields: [
        {
            kind: "field",
            id: "language",
            label: qsTr("Language")
        },
        {
            kind: "field",
            id: "region",
            label: qsTr("System names")
        },
        {
            kind: "field",
            id: "clockFormat",
            label: qsTr("Clock format")
        }
    ]
    readonly property var controlsInputFields: [
        {
            kind: "field",
            id: "buttonLayout",
            label: qsTr("Button style")
        },
        {
            kind: "field",
            id: "mouseEnabled",
            label: qsTr("Mouse support")
        },
        {
            kind: "field",
            id: "reduceMotion",
            label: qsTr("Reduce motion")
        }
    ]
    readonly property var libraryDataFields: [
        {
            kind: "field",
            id: "updateMediaDb",
            label: qsTr("Update media database")
        },
        {
            kind: "field",
            id: "discoverArcadeAlternateVersions",
            label: qsTr("Discover arcade alternate versions")
        },
        {
            kind: "field",
            id: "runScraper",
            label: qsTr("Scrape metadata")
        },
        {
            kind: "field",
            id: "rescrapeExisting",
            label: qsTr("Re-scrape existing")
        }
    ]
    readonly property var supportAboutFields: [
        {
            kind: "field",
            id: "aboutLicense",
            label: qsTr("About / License")
        },
        {
            kind: "field",
            id: "debugLogging",
            label: qsTr("Debug logging")
        },
        {
            kind: "field",
            id: "uploadLog",
            label: qsTr("Upload log file")
        }
    ]
    readonly property var fields: {
        if (settings.currentPage === settings.pageDisplayInterface)
            return settings.displayInterfaceFields;
        if (settings.currentPage === settings.pageBrowsing)
            return settings.browsingFields;
        if (settings.currentPage === settings.pageLanguage)
            return settings.languageFields;
        if (settings.currentPage === settings.pageControlsInput)
            return settings.controlsInputFields;
        if (settings.currentPage === settings.pageLibraryData)
            return settings.libraryDataFields;
        if (settings.currentPage === settings.pageSupportAbout)
            return settings.supportAboutFields;
        return settings.categoryFields;
    }
    readonly property string pageTitle: {
        if (settings.currentPage === settings.pageDisplayInterface)
            return qsTr("Display");
        if (settings.currentPage === settings.pageBrowsing)
            return qsTr("Browsing");
        if (settings.currentPage === settings.pageLanguage)
            return qsTr("Language");
        if (settings.currentPage === settings.pageControlsInput)
            return qsTr("Controls");
        if (settings.currentPage === settings.pageLibraryData)
            return qsTr("Library");
        if (settings.currentPage === settings.pageSupportAbout)
            return qsTr("Support");
        return qsTr("Settings");
    }

    // Live-state caption helpers for the action rows. While the matching
    // operation is in flight we paint the same vocabulary as the Core TUI
    // (Optimizing / In progress / Paused). When idle, fall back to a
    // count summary so the user can see at a glance how much is indexed
    // / scraped without having to start a job. The fields used here
    // mirror the TUI's `formatDBMenuLabel` and `formatScrapeMenuLabel`:
    // `total_media` is the populated-when-idle indexed count;
    // `scrape_total_scraped` is the cumulative scraped count, seeded
    // via `media.scrape.status` on connect.
    function _indexActionStatus(): string {
        if (Browse.MediaStatus.optimizing)
            return qsTr("Optimizing");
        if (Browse.MediaStatus.indexing)
            return Browse.MediaStatus.paused ? qsTr("Paused") : qsTr("In progress");
        const total = Browse.MediaStatus.total_media;
        if (total > 0)
            return qsTr("%1 indexed").arg(total);
        return "";
    }

    function _scrapeActionStatus(): string {
        if (Browse.MediaStatus.scraping)
            return Browse.MediaStatus.scrape_paused ? qsTr("Paused") : qsTr("In progress");
        const total = Browse.MediaStatus.scrape_total_scraped;
        if (total > 0)
            return qsTr("%1 scraped").arg(total);
        return "";
    }

    // Index and scrape can't run concurrently — Core serialises them.
    // While one is in flight the *other* row is non-actionable so we
    // don't queue a request that Core will reject.
    readonly property bool _indexBusy: Browse.MediaStatus.indexing || Browse.MediaStatus.optimizing
    readonly property bool _scrapeBusy: Browse.MediaStatus.scraping
    property bool rescrapeExisting: false
    // Keep the one-shot force flag visible while the scrape it started
    // is active; clear it only after Core reports the scrape stopped.
    property bool _activeScrapeUsedRescrape: false
    readonly property bool _visibleRescrapeExisting: settings._scrapeBusy && Browse.MediaStatus.scrape_force_known ? Browse.MediaStatus.scrape_force : settings.rescrapeExisting

    // Drive the top/bottom scroll chevrons. Ignore the spacer-only
    // overflow at the form edges: the arrows should mean another row
    // is hidden, not that there is padding past the last visible row.
    // The 1-px epsilon swallows sub-pixel rounding so the chevrons
    // don't flicker on exact-fit content. Use the Column geometry
    // rather than Repeater.itemAt() so the binding re-evaluates after
    // layout settles; itemAt() returning null during construction made
    // the bottom chevron miss overflowing pages.
    readonly property bool _hasContentAbove: settings._firstFieldTop() >= 0 && flickable.contentY > settings._firstFieldTop() + 1
    readonly property bool _hasContentBelow: settings._lastFieldBottom() >= 0 && flickable.contentY + flickable.height < settings._lastFieldBottom() - 1

    function _firstFieldTop(): real {
        if (settings.fieldCount <= 0)
            return -1;
        return leadingSpacer.height + form.spacing;
    }

    function _lastFieldBottom(): real {
        if (settings.fieldCount <= 0)
            return -1;
        return Math.max(0, form.implicitHeight - trailingSpacer.height - form.spacing);
    }

    function _triggerIndex(): void {
        if (settings._scrapeBusy)
            return;
        if (settings._indexBusy)
            Browse.MediaStatus.cancel_index();
        else
            Browse.MediaStatus.start_index();
    }

    function _triggerScrape(): void {
        if (settings._indexBusy)
            return;
        if (settings._scrapeBusy)
            Browse.MediaStatus.cancel_scrape();
        else {
            settings._activeScrapeUsedRescrape = settings.rescrapeExisting;
            Browse.MediaStatus.start_scrape(settings.rescrapeExisting);
        }
    }

    on_ScrapeBusyChanged: {
        if (!settings._scrapeBusy && settings._activeScrapeUsedRescrape) {
            settings.rescrapeExisting = false;
            settings._activeScrapeUsedRescrape = false;
        }
    }

    function _fieldEnabled(id: string): bool {
        if (id === "updateMediaDb")
            return !settings._scrapeBusy;
        if (id === "runScraper")
            return !settings._indexBusy;
        if (id === "rescrapeExisting")
            return !settings._indexBusy && !settings._scrapeBusy;
        return true;
    }

    function _fieldValue(id: string): string {
        if (id === "resolution")
            return settings._resolutionDisplay(Browse.Settings.current_resolution);
        if (id === "language")
            return settings._languageDisplay(Browse.Settings.current_language);
        if (id === "orientation")
            return settings._orientationDisplay(Browse.Settings.current_orientation);
        if (id === "browseLayout")
            return settings._browseLayoutDisplay(Browse.Settings.current_browse_layout);
        if (id === "systemLogoStyle")
            return settings._systemLogoStyleDisplay(Browse.Settings.current_system_logo_style);
        if (id === "theme")
            return settings._themeDisplay(Browse.Settings.current_theme);
        if (id === "buttonLayout")
            return settings._buttonLayoutDisplay(Browse.Settings.current_button_layout);
        if (id === "screensaverTimeout")
            return settings._screensaverTimeoutDisplay(Browse.Settings.current_screensaver_timeout);
        if (id === "clockFormat")
            return settings._clockFormatDisplay(Browse.Settings.current_clock_format);
        if (id === "region")
            return settings._regionDisplay(Browse.Settings.current_region);
        if (id === "mediaImageType")
            return settings._mediaImageTypeDisplay(Browse.Settings.current_media_image_type);
        if (id === "crtVideoStandard")
            return settings._videoStandardDisplay(Browse.CrtVideo.current_video_standard);
        return "";
    }

    function _fieldControl(id: string): string {
        if (id === "mouseEnabled" || id === "showHidden" || id === "showOriginalFilenames" || id === "discoverArcadeAlternateVersions" || id === "debugLogging" || id === "rescrapeExisting" || id === "reduceMotion" || id === "crtEnabled")
            return "toggle";
        if (id === "aboutLicense" || id === "pageDisplayInterface" || id === "pageBrowsing" || id === "pageLanguage" || id === "pageControlsInput" || id === "pageLibraryData" || id === "pageSupportAbout" || id === "crtCalibration")
            if (id === "updateMediaDb" || id === "runScraper" || id === "uploadLog")
                return "action";
        return "picker";
    }

    function _fieldChecked(id: string): bool {
        if (id === "debugLogging")
            return Browse.Settings.current_debug_logging;
        if (id === "discoverArcadeAlternateVersions")
            return Browse.Settings.current_discover_arcade_alternate_versions;
        if (id === "rescrapeExisting")
            return settings._visibleRescrapeExisting;
        if (id === "showHidden")
            return Browse.Settings.current_show_hidden;
        if (id === "showOriginalFilenames")
            return Browse.Settings.current_show_original_filenames;
        if (id === "reduceMotion")
            return Browse.Settings.current_reduce_motion;
        if (id === "crtEnabled")
            return Browse.CrtVideo.crt_enabled;
        return Browse.Settings.current_mouse_enabled;
    }

    readonly property int fieldCount: settings.fields.length

    // True iff `idx` points at a focusable field row (not a header,
    // not out of bounds). All `focused*` derivations early-return on
    // header indices so a defensive out-of-band write to currentIndex
    // can't mis-light the help bar.
    function _isField(idx: int): bool {
        if (idx < 0 || idx >= settings.fieldCount)
            return false;
        return settings.fields[idx].kind === "field";
    }

    // First focusable row in the registry. Used to seed `currentIndex`
    // at construction; returns -1 only if every entry is a header
    // (registry mistake — shouldn't happen).
    function _firstNavigableIndex(): int {
        for (let i = 0; i < settings.fieldCount; i++)
            if (settings.fields[i].kind === "field")
                return i;
        return -1;
    }

    // Walk from `from` in `direction` (±1) until we hit a focusable
    // row. Headers are transparent, and edges wrap so Up on the first
    // field lands on the last field (and vice versa).
    function _seekNavigable(from: int, direction: int): int {
        if (settings.fieldCount <= 0)
            return from;
        let i = from;
        for (let steps = 0; steps < settings.fieldCount; steps++) {
            i += direction;
            if (i < 0)
                i = settings.fieldCount - 1;
            else if (i >= settings.fieldCount)
                i = 0;
            if (settings.fields[i].kind === "field")
                return i;
        }
        return from;
    }

    readonly property int rootGridRows: 2
    // Columns track the (fixed) category count so the menu auto-balances into
    // `rootGridRows` rows (six categories -> 3x2) instead of a hardcoded count.
    // This is chrome with a known small item set, not content that should
    // reflow with screen width; the cell geometry below is already
    // sizing-driven (pctW/pctH with a maxCellSize cap), so the cells shrink to
    // fit any screen while the layout stays a deliberate balanced grid.
    readonly property int rootGridColumns: Math.ceil(settings.categoryFields.length / settings.rootGridRows)

    function _moveRootGrid(dx: int, dy: int): void {
        if (settings.fieldCount <= 0)
            return;
        const columns = settings.rootGridColumns;
        const row = Math.floor(settings.currentIndex / columns);
        const col = settings.currentIndex % columns;
        if (dx !== 0) {
            const rowStart = row * columns;
            const rowEnd = Math.min(settings.fieldCount - 1, rowStart + columns - 1);
            let next = settings.currentIndex + dx;
            if (next < rowStart)
                next = rowEnd;
            else if (next > rowEnd)
                next = rowStart;
            settings.currentIndex = next;
            return;
        }
        if (dy !== 0) {
            let next = settings.currentIndex + dy * columns;
            if (next < 0) {
                const lastRow = Math.floor((settings.fieldCount - 1) / columns);
                next = Math.min(lastRow * columns + col, settings.fieldCount - 1);
            } else if (next >= settings.fieldCount) {
                next = Math.min(col, settings.fieldCount - 1);
            }
            settings.currentIndex = next;
        }
    }

    function _focusRootIndex(index: int): void {
        if (index < 0 || index >= settings.fieldCount)
            return;
        settings.currentIndex = index;
    }

    readonly property bool focusedFieldIsToggle: {
        if (!settings._isField(settings.currentIndex))
            return false;
        const id = settings.fields[settings.currentIndex].id;
        return id === "mouseEnabled" || id === "showHidden" || id === "showOriginalFilenames" || id === "discoverArcadeAlternateVersions" || id === "debugLogging" || id === "rescrapeExisting" || id === "reduceMotion" || id === "crtEnabled";
    }
    // True when the focused field is a list-picker row (Accept opens a
    // modal; left/right is a no-op — pickers don't cycle inline). Drives
    // the help-bar A: Open hint.
    readonly property bool focusedFieldIsPicker: {
        if (!settings._isField(settings.currentIndex))
            return false;
        const id = settings.fields[settings.currentIndex].id;
        return id === "language" || id === "clockFormat" || id === "region" || id === "orientation" || id === "browseLayout" || id === "systemLogoStyle" || id === "theme" || id === "buttonLayout" || id === "resolution" || id === "screensaverTimeout" || id === "mediaImageType" || id === "crtVideoStandard";
    }
    // True when focused row accepts A without left/right cycling:
    // pickers, jobs, modal/navigation rows, and root category rows.
    // Drives help-bar Accept hint and suppresses left/right Change cue.
    readonly property bool focusedFieldIsAction: {
        if (!settings._isField(settings.currentIndex))
            return false;
        const id = settings.fields[settings.currentIndex].id;
        return settings.focusedFieldIsPicker || id === "updateMediaDb" || id === "runScraper" || id === "uploadLog" || id === "aboutLicense" || id === "pageDisplayInterface" || id === "pageBrowsing" || id === "pageLanguage" || id === "pageControlsInput" || id === "pageLibraryData" || id === "pageSupportAbout" || id === "crtCalibration";
    }
    // Verb shown on the help-bar Accept hint for the focused action
    // row. Index/scrape flip between Start and Cancel because the press
    // toggles the in-flight operation; uploadLog reads "Upload" because
    // the press opens the upload-flow modal rather than kicking off an
    // in-row job; aboutLicense reads "Open" because the press navigates.
    readonly property string focusedActionLabel: {
        if (!settings._isField(settings.currentIndex))
            return "";
        const id = settings.fields[settings.currentIndex].id;
        if (id === "updateMediaDb" || id === "runScraper")
            return settings.focusedActionBusy ? qsTr("Cancel") : qsTr("Start");
        if (id === "uploadLog")
            return qsTr("Upload");
        return qsTr("Open");
    }
    // True when the focused action's matching operation is currently
    // running, so the help bar can label Accept as "Cancel" rather
    // than "Start".
    readonly property bool focusedActionBusy: {
        if (!settings._isField(settings.currentIndex))
            return false;
        const id = settings.fields[settings.currentIndex].id;
        if (id === "updateMediaDb")
            return settings._indexBusy;
        if (id === "runScraper")
            return settings._scrapeBusy;
        return false;
    }
    // True when the focused action can't run right now because the
    // *other* media operation has the bus. Drives the dimmed-row
    // visual and lets the help bar drop the Accept hint instead of
    // promising a press that will silently no-op.
    readonly property bool focusedActionDisabled: {
        if (!settings._isField(settings.currentIndex))
            return false;
        const id = settings.fields[settings.currentIndex].id;
        if (id === "updateMediaDb")
            return settings._scrapeBusy;
        if (id === "runScraper")
            return settings._indexBusy;
        return false;
    }

    // Initial focus: first navigable row. The binding evaluates once
    // (no reactive dependencies inside `_firstNavigableIndex`) and is
    // broken the first time the user moves focus — handleAction's
    // up/down branches assign to `currentIndex` directly. Falling back
    // to 0 covers the all-headers degenerate case; helpers below
    // early-return on `_isField(0) === false` if it ever lands there.
    property int currentIndex: {
        const idx = settings._firstNavigableIndex();
        return idx >= 0 ? idx : 0;
    }

    function _resolutionList(): list<string> {
        const raw = Browse.Settings.available_resolutions;
        return raw === undefined || raw === null ? [] : raw;
    }

    function _resolutionDisplay(value: string): string {
        // Empty resolution means "fall back to frontend.toml defaults",
        // which the Settings model treats as the platform default. Render
        // it as a translated label rather than an empty cell so the user
        // sees something selectable.
        return value === "" ? qsTr("Default") : value;
    }

    function _buttonLayoutList(): list<string> {
        const raw = Browse.Settings.available_button_layouts;
        return raw === undefined || raw === null ? [] : raw;
    }

    function _browseLayoutList(): list<string> {
        const raw = Browse.Settings.available_browse_layouts;
        return raw === undefined || raw === null ? [] : raw;
    }

    function _systemLogoStyleList(): list<string> {
        const raw = Browse.Settings.available_system_logo_styles;
        return raw === undefined || raw === null ? [] : raw;
    }

    function _themeList(): list<string> {
        const raw = Browse.Settings.available_themes;
        return raw === undefined || raw === null ? [] : raw;
    }

    function _languageList(): list<string> {
        const raw = Browse.Settings.available_languages;
        return raw === undefined || raw === null ? [] : raw;
    }

    function _clockFormatList(): list<string> {
        const raw = Browse.Settings.available_clock_formats;
        return raw === undefined || raw === null ? [] : raw;
    }

    function _orientationList(): list<string> {
        const raw = Browse.Settings.available_orientations;
        return raw === undefined || raw === null ? [] : raw;
    }

    function _languageDisplay(value: string): string {
        if (value === "en" || value === "en_US" || value === "en_GB")
            return qsTr("English");
        if (value === "it" || value === "it_IT")
            return qsTr("Italian");
        if (value === "es" || value === "es_ES")
            return qsTr("Spanish");
        if (value === "eu" || value === "eu_ES")
            return qsTr("Basque");
        if (value === "de" || value === "de_DE")
            return qsTr("German");
        if (value === "el" || value === "el_GR")
            return qsTr("Greek");
        if (value === "ja" || value === "ja_JP")
            return qsTr("Japanese");
        if (value === "ko" || value === "ko_KR")
            return qsTr("Korean");
        if (value === "nl" || value === "nl_NL")
            return qsTr("Dutch");
        if (value === "ro" || value === "ro_RO")
            return qsTr("Romanian");
        if (value === "sk" || value === "sk_SK")
            return qsTr("Slovak");
        if (value === "uk" || value === "uk_UA")
            return qsTr("Ukrainian");
        if (value === "zh_CN")
            return qsTr("Chinese (Simplified)");
        if (value === "zh_TW" || value === "zh_HK")
            return qsTr("Chinese (Traditional)");
        if (value === "he" || value === "he_IL")
            return qsTr("Hebrew");
        if (value === "ar" || value === "ar_SA")
            return qsTr("Arabic");
        if (value === "hi" || value === "hi_IN")
            return qsTr("Hindi");
        return qsTr("Auto");
    }

    function _clockFormatDisplay(value: string): string {
        if (value === "12h")
            return qsTr("12-hour");
        if (value === "24h")
            return qsTr("24-hour");
        return qsTr("Auto");
    }

    function _regionList(): list<string> {
        const raw = Browse.Settings.available_regions;
        return raw === undefined || raw === null ? [] : raw;
    }

    function _regionDisplay(value: string): string {
        if (value === "us")
            return qsTr("Americas");
        if (value === "eu")
            return qsTr("Europe");
        if (value === "jp")
            return qsTr("Japan");
        return qsTr("Automatic");
    }

    function _orientationDisplay(value: string): string {
        if (value === "cw")
            return qsTr("Rotated CW");
        if (value === "ccw")
            return qsTr("Rotated CCW");
        return qsTr("Horizontal");
    }

    function _browseLayoutDisplay(value: string): string {
        if (value === "list")
            return qsTr("Detailed list view");
        return qsTr("Grid view");
    }

    function _systemLogoStyleDisplay(value: string): string {
        if (value === "color")
            return qsTr("Full color");
        return qsTr("Tinted");
    }

    function _themeDisplay(value: string): string {
        if (value === "crt-light")
            return qsTr("CRT light");
        return qsTr("Default");
    }

    function _buttonLayoutDisplay(value: string): string {
        if (value === "b")
            return qsTr("Style B");
        if (value === "c")
            return qsTr("Style C");
        if (value === "d")
            return qsTr("Style D");
        return qsTr("Style A");
    }

    function _screensaverTimeoutList(): list<string> {
        const raw = Browse.Settings.available_screensaver_timeouts;
        return raw === undefined || raw === null ? [] : raw;
    }

    function _mediaImageTypeList(): list<string> {
        const raw = Browse.Settings.available_media_image_types;
        return raw === undefined || raw === null ? [] : raw;
    }

    function _screensaverTimeoutDisplay(value: string): string {
        if (value === "off")
            return qsTr("Off");
        if (value === "1")
            return qsTr("1 second (testing)");
        if (value === "60")
            return qsTr("1 minute");
        if (value === "120")
            return qsTr("2 minutes");
        if (value === "300")
            return qsTr("5 minutes");
        if (value === "600")
            return qsTr("10 minutes");
        if (value === "900")
            return qsTr("15 minutes");
        if (value === "1800")
            return qsTr("30 minutes");
        return qsTr("%1 seconds").arg(value);
    }

    function _videoStandardList(): list<string> {
        const raw = Browse.CrtVideo.available_video_standards;
        return raw === undefined || raw === null ? [] : raw;
    }

    function _videoStandardDisplay(value: string): string {
        if (value === "pal")
            return qsTr("PAL (50 Hz)");
        if (value === "480i")
            return qsTr("480i (60 Hz)");
        return qsTr("NTSC (60 Hz)");
    }

    function _mediaImageTypeDisplay(value: string): string {
        if (value === "auto")
            return qsTr("Auto");
        if (value === "image")
            return qsTr("Image");
        if (value === "thumbnail")
            return qsTr("Thumbnail");
        if (value === "boxart")
            return qsTr("Box art");
        if (value === "boxart3d")
            return qsTr("3D box art");
        if (value === "screenshot")
            return qsTr("Screenshot");
        if (value === "wheel")
            return qsTr("Wheel");
        if (value === "titleshot")
            return qsTr("Title screen");
        if (value === "map")
            return qsTr("Map");
        if (value === "marquee")
            return qsTr("Marquee");
        if (value === "fanart")
            return qsTr("Fan art");
        if (value === "boxartside")
            return qsTr("Box side");
        if (value === "boxartback")
            return qsTr("Box back");
        return value;
    }

    // Build the picker entry list for a field. Each entry is
    // `{ id: string, label: string }` — `id` is the canonical value
    // the model stores, `label` is the localised display string.
    // The router emits `requestListPicker` and `Main.qml` mounts the
    // shared `ListPickerModal` with these.
    function _openPickerForField(id: string): void {
        let title = "";
        let entries = [];
        let initialId = "";
        if (id === "resolution") {
            title = qsTr("Resolution");
            const list = settings._resolutionList();
            for (let i = 0; i < list.length; i++)
                entries.push({
                    id: list[i],
                    label: settings._resolutionDisplay(list[i])
                });
            initialId = Browse.Settings.current_resolution;
        } else if (id === "language") {
            title = qsTr("Language");
            const list = settings._languageList();
            for (let i = 0; i < list.length; i++)
                entries.push({
                    id: list[i],
                    label: settings._languageDisplay(list[i])
                });
            initialId = Browse.Settings.current_language;
        } else if (id === "clockFormat") {
            title = qsTr("Clock format");
            const list = settings._clockFormatList();
            for (let i = 0; i < list.length; i++)
                entries.push({
                    id: list[i],
                    label: settings._clockFormatDisplay(list[i])
                });
            initialId = Browse.Settings.current_clock_format;
        } else if (id === "region") {
            title = qsTr("System names");
            const list = settings._regionList();
            for (let i = 0; i < list.length; i++)
                entries.push({
                    id: list[i],
                    label: settings._regionDisplay(list[i])
                });
            initialId = Browse.Settings.current_region;
        } else if (id === "orientation") {
            title = qsTr("Orientation");
            const list = settings._orientationList();
            for (let i = 0; i < list.length; i++)
                entries.push({
                    id: list[i],
                    label: settings._orientationDisplay(list[i])
                });
            initialId = Browse.Settings.current_orientation;
        } else if (id === "browseLayout") {
            title = qsTr("Browsing layout");
            const list = settings._browseLayoutList();
            for (let i = 0; i < list.length; i++)
                entries.push({
                    id: list[i],
                    label: settings._browseLayoutDisplay(list[i])
                });
            initialId = Browse.Settings.current_browse_layout;
        } else if (id === "systemLogoStyle") {
            title = qsTr("System logos");
            const list = settings._systemLogoStyleList();
            for (let i = 0; i < list.length; i++)
                entries.push({
                    id: list[i],
                    label: settings._systemLogoStyleDisplay(list[i])
                });
            initialId = Browse.Settings.current_system_logo_style;
        } else if (id === "theme") {
            title = qsTr("Theme");
            const list = settings._themeList();
            for (let i = 0; i < list.length; i++)
                entries.push({
                    id: list[i],
                    label: settings._themeDisplay(list[i])
                });
            initialId = Browse.Settings.current_theme;
        } else if (id === "buttonLayout") {
            title = qsTr("Button style");
            const list = settings._buttonLayoutList();
            for (let i = 0; i < list.length; i++)
                entries.push({
                    id: list[i],
                    label: settings._buttonLayoutDisplay(list[i])
                });
            initialId = Browse.Settings.current_button_layout;
        } else if (id === "screensaverTimeout") {
            title = qsTr("Screensaver");
            const list = settings._screensaverTimeoutList();
            for (let i = 0; i < list.length; i++)
                entries.push({
                    id: list[i],
                    label: settings._screensaverTimeoutDisplay(list[i])
                });
            initialId = Browse.Settings.current_screensaver_timeout;
        } else if (id === "mediaImageType") {
            title = qsTr("Preferred artwork");
            const list = settings._mediaImageTypeList();
            for (let i = 0; i < list.length; i++)
                entries.push({
                    id: list[i],
                    label: settings._mediaImageTypeDisplay(list[i])
                });
            initialId = Browse.Settings.current_media_image_type;
        } else if (id === "crtVideoStandard") {
            title = qsTr("Video standard");
            const list = settings._videoStandardList();
            for (let i = 0; i < list.length; i++)
                entries.push({
                    id: list[i],
                    label: settings._videoStandardDisplay(list[i])
                });
            initialId = Browse.CrtVideo.current_video_standard;
        } else {
            return;
        }
        if (entries.length === 0)
            return;
        settings.requestListPicker(title, entries, initialId, id);
    }

    function _reprojectBrowseModels(): void {
        Browse.SystemsModel.reproject();
        Browse.CategoriesModel.reproject();
    }

    function _setShowHidden(direction: int): void {
        const showHidden = direction > 0;
        if (Browse.Settings.current_show_hidden === showHidden)
            return;
        Browse.Settings.set_show_hidden(showHidden);
        settings._reprojectBrowseModels();
    }

    function _toggleShowHidden(): void {
        Browse.Settings.set_show_hidden(!Browse.Settings.current_show_hidden);
        settings._reprojectBrowseModels();
    }

    function _setShowOriginalFilenames(direction: int): void {
        Browse.Settings.set_show_original_filenames(direction > 0);
    }

    function _toggleShowOriginalFilenames(): void {
        Browse.Settings.set_show_original_filenames(!Browse.Settings.current_show_original_filenames);
    }

    function _setMouseEnabled(direction: int): void {
        Browse.Settings.set_mouse_enabled(direction > 0);
    }

    function _toggleMouseEnabled(): void {
        Browse.Settings.set_mouse_enabled(!Browse.Settings.current_mouse_enabled);
    }

    function _setDebugLogging(direction: int): void {
        Browse.Settings.set_debug_logging(direction > 0);
    }

    function _toggleDebugLogging(): void {
        Browse.Settings.set_debug_logging(!Browse.Settings.current_debug_logging);
    }

    function _setReduceMotion(direction: int): void {
        Browse.Settings.set_reduce_motion(direction > 0);
    }

    function _toggleReduceMotion(): void {
        Browse.Settings.set_reduce_motion(!Browse.Settings.current_reduce_motion);
    }

    function _setDiscoverArcadeAlternateVersions(direction: int): void {
        Browse.Settings.set_discover_arcade_alternate_versions(direction > 0);
    }

    function _toggleDiscoverArcadeAlternateVersions(): void {
        Browse.Settings.set_discover_arcade_alternate_versions(!Browse.Settings.current_discover_arcade_alternate_versions);
    }

    function _setRescrapeExisting(direction: int): void {
        if (settings._indexBusy || settings._scrapeBusy)
            return;
        settings.rescrapeExisting = direction > 0;
    }

    function _toggleRescrapeExisting(): void {
        if (settings._indexBusy || settings._scrapeBusy)
            return;
        settings.rescrapeExisting = !settings.rescrapeExisting;
    }

    // CRT mode is confirm-gated: the pill never flips locally. The
    // request routes to Main.qml, which stages the restart-confirm
    // modal; the actual flip happens on the post-exit-42 respawn.
    function _requestCrtEnabled(enable: bool): void {
        if (enable === Browse.CrtVideo.crt_enabled)
            return;
        settings.requestAccept(enable ? "crtEnable" : "crtDisable");
    }

    function _cycleFocused(direction: int): void {
        if (!settings._isField(settings.currentIndex))
            return;
        const id = settings.fields[settings.currentIndex].id;
        // Picker fields ignore left/right - accept opens the
        // list-picker modal instead. Only toggles still respond to
        // direction presses (left = off, right = on).
        if (id === "mouseEnabled")
            settings._setMouseEnabled(direction);
        else if (id === "showHidden")
            settings._setShowHidden(direction);
        else if (id === "showOriginalFilenames")
            settings._setShowOriginalFilenames(direction);
        else if (id === "discoverArcadeAlternateVersions")
            settings._setDiscoverArcadeAlternateVersions(direction);
        else if (id === "debugLogging")
            settings._setDebugLogging(direction);
        else if (id === "rescrapeExisting")
            settings._setRescrapeExisting(direction);
        else if (id === "reduceMotion")
            settings._setReduceMotion(direction);
        else if (id === "crtEnabled")
            settings._requestCrtEnabled(direction > 0);
    }

    function _rememberPageFocus(): void {
        settings._pageIndexes[settings.currentPage] = settings.currentIndex;
    }

    function _restorePageFocus(): void {
        const first = settings._firstNavigableIndex();
        const fallback = first >= 0 ? first : 0;
        const remembered = settings._pageIndexes[settings.currentPage];
        const idx = remembered === undefined ? fallback : Math.max(0, Math.min(settings.fieldCount - 1, remembered));
        settings.currentIndex = settings._isField(idx) ? idx : fallback;
        flickable.contentY = 0;
    }

    function _switchPage(page: string): void {
        // Disable SettingsField Behaviors for this synchronous block so
        // reused delegates don't animate focus-border or toggle-position
        // changes when the new page's field model lands. The flag is
        // cleared on the next event-loop tick so subsequent user moves
        // still animate normally.
        settings._pageSwitching = true;
        settings._rememberPageFocus();
        settings.currentPage = page;
        settings._restorePageFocus();
        Qt.callLater(() => {
            settings._pageSwitching = false;
        });
    }

    function _openPage(id: string): bool {
        // Resolve the target page first so we can return false quickly for
        // non-page IDs. Then fire the pulse (cue plays on the still-visible
        // tile) and defer _switchPage so the push-in's downward leg is
        // fully visible before the page swaps out.
        let page = "";
        if (id === "pageDisplayInterface")
            page = settings.pageDisplayInterface;
        else if (id === "pageBrowsing")
            page = settings.pageBrowsing;
        else if (id === "pageLanguage")
            page = settings.pageLanguage;
        else if (id === "pageControlsInput")
            page = settings.pageControlsInput;
        else if (id === "pageLibraryData")
            page = settings.pageLibraryData;
        else if (id === "pageSupportAbout")
            page = settings.pageSupportAbout;
        else
            return false;
        settings.activatePulse++;
        pressCommit._page = page;
        pressCommit.arm();
        return true;
    }

    function _goBack(): void {
        // Disarm pending accepts so a press-then-back inside the deferred
        // window cannot drill into a subpage / open a picker after backing out.
        pressCommit.stop();
        fieldCommit.stop();
        if (settings.currentPage !== settings.pageRoot) {
            settings._switchPage(settings.pageRoot);
            return;
        }
        settings.requestHubScreen();
    }

    function handleAction(action: string): void {
        if (settings.optimisticLoading) {
            if (action === "cancel")
                settings._goBack();
            return;
        }
        if (action === "up") {
            if (settings.showingRootGrid)
                settings._moveRootGrid(0, -1);
            else
                settings.currentIndex = settings._seekNavigable(settings.currentIndex, -1);
        } else if (action === "down") {
            if (settings.showingRootGrid)
                settings._moveRootGrid(0, 1);
            else
                settings.currentIndex = settings._seekNavigable(settings.currentIndex, 1);
        } else if (action === "left") {
            if (settings.showingRootGrid)
                settings._moveRootGrid(-1, 0);
            else
                settings._cycleFocused(-1);
        } else if (action === "right") {
            if (settings.showingRootGrid)
                settings._moveRootGrid(1, 0);
            else
                settings._cycleFocused(1);
        } else if (action === "accept") {
            if (!settings._isField(settings.currentIndex))
                return;
            const id = settings.fields[settings.currentIndex].id;
            if (settings._openPage(id))
                return;
            // Toggles flip in place — the knob slide is their cue, so act now
            // and skip the push-in.
            if (settings._fieldControl(id) === "toggle") {
                if (id === "mouseEnabled")
                    settings._toggleMouseEnabled();
                else if (id === "showHidden")
                    settings._toggleShowHidden();
                else if (id === "showOriginalFilenames")
                    settings._toggleShowOriginalFilenames();
                else if (id === "discoverArcadeAlternateVersions")
                    settings._toggleDiscoverArcadeAlternateVersions();
                else if (id === "debugLogging")
                    settings._toggleDebugLogging();
                else if (id === "rescrapeExisting")
                    settings._toggleRescrapeExisting();
                else if (id === "reduceMotion")
                    settings._toggleReduceMotion();
                else if (id === "crtEnabled")
                    settings._requestCrtEnabled(!Browse.CrtVideo.crt_enabled);
                return;
            }
            // Picker / action / about either open a modal or navigate away,
            // which would cover or replace the row before its push-in could
            // show. Play the cue, then run the action deferred (the same
            // deferred-flip the tiles use) so the press is visible on the
            // still-static settings screen first.
            settings.fieldActivatePulse++;
            fieldCommit._id = id;
            fieldCommit.arm();
        } else if (action === "cancel") {
            settings._goBack();
        }
    }

    // ── Visual tree ───────────────────────────────────────────────────────────

    DeferredAction {
        id: pressCommit
        property string _page: ""
        onDeferred: {
            const p = _page;
            _page = "";
            settings._switchPage(p);
        }
    }

    // Deferred-flip for non-toggle field activations: the focused row's push-in
    // plays on the still-visible settings screen, then this fires `pressMs`
    // later to open the modal / navigate. Without the defer the modal scrim or
    // screen change covers the row before the cue can render.
    DeferredAction {
        id: fieldCommit
        property string _id: ""
        onDeferred: {
            const id = fieldCommit._id;
            fieldCommit._id = "";
            if (id === "updateMediaDb")
                settings._triggerIndex();
            else if (id === "runScraper")
                settings._triggerScrape();
            else if (id === "uploadLog")
                settings.requestAccept("uploadLog");
            else if (id === "aboutLicense")
                settings.requestAccept("aboutLicense");
            else if (id === "crtCalibration")
                settings.requestAccept("crtCalibration");
            else
                settings._openPickerForField(id);
        }
    }

    MouseArea {
        anchors.fill: parent
        acceptedButtons: Qt.RightButton
        onClicked: settings._goBack()
    }

    TopStatusStrip {
        id: topStrip
        visible: !settings.optimisticLoading
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: parent.top
        anchors.topMargin: Sizing.headerBottom
        height: Sizing.pctH(7)
        title: settings.pageTitle
        currentPage: 0
        totalPages: 0
        totalText: ""
    }

    // Scroll focused row into view if it sits outside the Flickable's
    // current viewport. No-op for header indices (they aren't focusable
    // and currentIndex never lands on one in normal flow). Bind via
    // onCurrentIndexChanged below; no animation — software-renderer
    // budget can't pay for a moving column behind a focus border.
    function _scrollFocusedIntoView(): void {
        if (settings.showingRootGrid || !settings._isField(settings.currentIndex))
            return;
        const row = rowRepeater.itemAt(settings.currentIndex);
        if (row === null)
            return;
        const top = row.y;
        const bottom = top + row.height;
        if (top < flickable.contentY)
            flickable.contentY = top;
        else if (bottom > flickable.contentY + flickable.height)
            flickable.contentY = bottom - flickable.height;
    }

    onCurrentIndexChanged: settings._scrollFocusedIntoView()

    Item {
        id: categoryGrid

        visible: !settings.optimisticLoading && settings.showingRootGrid && settings.fieldCount > 0
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: topStrip.bottom
        anchors.topMargin: Sizing.pctH(2)
        anchors.bottom: parent.bottom
        anchors.bottomMargin: Sizing.pctH(15)

        readonly property int columns: settings.rootGridColumns
        readonly property int rows: settings.rootGridRows
        readonly property int leftInset: Sizing.pctW(5)
        readonly property int rightInset: Sizing.pctW(5)
        readonly property int topInset: Sizing.pctH(2)
        readonly property int bottomInset: Sizing.pctH(2)
        readonly property int cellSpacingX: Sizing.pctW(3)
        readonly property int cellSpacingY: Sizing.pctH(4)
        readonly property int maxCellSize: Sizing.pctH(22)
        readonly property int _availableWidth: Math.max(0, width - leftInset - rightInset)
        readonly property int _availableHeight: Math.max(0, height - topInset - bottomInset)
        readonly property int cellSize: Math.max(0, Math.min(maxCellSize, Math.floor((_availableWidth - (columns - 1) * cellSpacingX) / columns), Math.floor((_availableHeight - (rows - 1) * cellSpacingY) / rows)))
        readonly property int visibleColumns: Math.max(1, Math.min(columns, settings.fieldCount))
        readonly property int visibleRows: Math.min(rows, Math.max(1, Math.ceil(settings.fieldCount / columns)))
        readonly property int contentWidth: visibleColumns * cellSize + (visibleColumns - 1) * cellSpacingX
        readonly property int contentHeight: visibleRows * cellSize + (visibleRows - 1) * cellSpacingY
        readonly property int originX: Sizing.center(width, contentWidth)
        readonly property int originY: Sizing.center(height, contentHeight)

        Component {
            id: categoryTileDelegate
            Tile {}
        }

        Repeater {
            model: settings.fields

            Item {
                id: categoryCell

                required property int index
                required property var modelData

                readonly property int cellRow: Math.floor(index / categoryGrid.columns)
                readonly property int cellCol: index % categoryGrid.columns
                readonly property bool isSelected: index === settings.currentIndex

                x: categoryGrid.originX + cellCol * (categoryGrid.cellSize + categoryGrid.cellSpacingX)
                y: categoryGrid.originY + cellRow * (categoryGrid.cellSize + categoryGrid.cellSpacingY)
                width: categoryGrid.cellSize
                height: categoryGrid.cellSize
                z: isSelected ? 1 : 0

                TileLoader {
                    anchors.fill: parent
                    sourceComponent: categoryTileDelegate
                    isSelected: categoryCell.isSelected
                    isFocused: true
                    name: categoryCell.modelData.label
                    coverKey: categoryCell.modelData.coverKey
                    activatePulse: settings.activatePulse
                }

                MouseArea {
                    anchors.fill: parent
                    hoverEnabled: true
                    acceptedButtons: Qt.LeftButton | Qt.RightButton
                    cursorShape: Qt.PointingHandCursor

                    onEntered: settings._focusRootIndex(categoryCell.index)
                    onClicked: mouse => {
                        if (mouse.button === Qt.RightButton) {
                            settings._goBack();
                            return;
                        }
                        settings._focusRootIndex(categoryCell.index);
                        settings.handleAction("accept");
                    }
                }
            }
        }
    }

    ActiveLabel {
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.bottom: parent.bottom
        anchors.bottomMargin: Sizing.pctH(8)
        height: Sizing.pctH(7)
        text: settings.showingRootGrid && settings._isField(settings.currentIndex) ? settings.fields[settings.currentIndex].label : ""
        visible: !settings.optimisticLoading && settings.showingRootGrid && settings.fieldCount > 0
    }

    // Form lives in a Flickable so the section bands can grow past
    // a single screen without dropping off-frame. Width capped so
    // the rows don't stretch edge-to-edge on widescreen; bottom
    // margin clears the help bar (pctH(6)) plus a small gap.
    Flickable {
        id: flickable
        visible: !settings.optimisticLoading && !settings.showingRootGrid

        // topMargin and bottomMargin are sized to leave a clear band
        // for the scroll chevrons to sit outside the scrollable area
        // (chevron pctH(3) + breathing room). bottomMargin also has to
        // clear the help bar (pctH(6)) plus a small gap.
        anchors.top: topStrip.bottom
        anchors.topMargin: Sizing.pctH(4)
        anchors.bottom: parent.bottom
        anchors.bottomMargin: Sizing.pctH(10)
        anchors.horizontalCenter: parent.horizontalCenter
        width: Math.min(parent.width - Sizing.pctW(10), Sizing.pctW(70))
        contentWidth: width
        contentHeight: form.implicitHeight
        clip: true
        boundsBehavior: Flickable.StopAtBounds

        Column {
            id: form

            width: parent.width
            spacing: Sizing.pctH(1.5)
            visible: settings.fieldCount > 0

            // Leading spacer — keeps the first field clear of the top
            // scroll chevron and gives the cut-off edge a breath of
            // whitespace instead of clipping mid-row.
            Item {
                id: leadingSpacer

                width: form.width
                height: Sizing.pctH(2)
            }

            Repeater {
                id: rowRepeater
                model: settings.fields

                // Wrapper row — both potential children exist but only
                // the kind-matching one paints. A Loader would also
                // work, but binding-through-`parent.modelData` adds
                // static-analysis friction under
                // ComponentBehavior:Bound; the wrapper Item is cheap
                // (≤ 3 headers + ≤ 7 fields) and keeps every
                // field-row binding readable in place.
                Item {
                    id: row

                    required property int index
                    required property var modelData

                    readonly property bool isHeader: modelData.kind === "header"

                    width: form.width
                    implicitHeight: row.isHeader ? header.implicitHeight : field.implicitHeight

                    SettingsSectionHeader {
                        id: header
                        visible: row.isHeader
                        anchors.left: parent.left
                        anchors.right: parent.right
                        label: row.modelData.label
                    }

                    SettingsField {
                        id: field
                        visible: !row.isHeader
                        anchors.left: parent.left
                        anchors.right: parent.right
                        isFocused: row.index === settings.currentIndex
                        animateChanges: !settings._pageSwitching
                        activatePulse: settings.fieldActivatePulse
                        // Index and scrape can't run together; while
                        // one operation is in flight the other row
                        // dims and its MouseArea stops responding.
                        // Keyboard Accept is separately gated in
                        // `_triggerIndex`/`_triggerScrape`.
                        enabled: settings._fieldEnabled(row.modelData.id)
                        label: row.modelData.label
                        value: settings._fieldValue(row.modelData.id)
                        control: settings._fieldControl(row.modelData.id)
                        checked: settings._fieldChecked(row.modelData.id)
                        actionStatus: row.modelData.id === "updateMediaDb" ? settings._indexActionStatus() : row.modelData.id === "runScraper" ? settings._scrapeActionStatus() : ""
                        onHovered: settings.currentIndex = row.index
                        onClicked: {
                            settings.currentIndex = row.index;
                            if (row.modelData.id === "mouseEnabled")
                                settings._toggleMouseEnabled();
                            else if (row.modelData.id === "showHidden")
                                settings._toggleShowHidden();
                            else if (row.modelData.id === "showOriginalFilenames")
                                settings._toggleShowOriginalFilenames();
                            else if (row.modelData.id === "discoverArcadeAlternateVersions")
                                settings._toggleDiscoverArcadeAlternateVersions();
                            else if (row.modelData.id === "debugLogging")
                                settings._toggleDebugLogging();
                            else if (row.modelData.id === "rescrapeExisting")
                                settings._toggleRescrapeExisting();
                            else if (row.modelData.id === "reduceMotion")
                                settings._toggleReduceMotion();
                            else if (row.modelData.id === "crtEnabled")
                                settings._requestCrtEnabled(!Browse.CrtVideo.crt_enabled);
                        }
                        onRightClicked: settings._goBack()
                        // Picker, action, and navigate rows route
                        // through `onAccepted` (see SettingsField's
                        // MouseArea), so the focus commit lives here
                        // too — clicking commits focus before firing
                        // the action.
                        onAccepted: {
                            settings.currentIndex = row.index;
                            if (settings._openPage(row.modelData.id))
                                return;
                            // Non-toggle rows route here (toggles use onClicked).
                            // Defer like the keyboard path so the push-in shows
                            // before the modal opens / the screen navigates.
                            settings.fieldActivatePulse++;
                            fieldCommit._id = row.modelData.id;
                            fieldCommit.arm();
                        }
                    }
                }
            }

            // Trailing spacer — symmetric with the leading spacer, so
            // the last field clears the bottom chevron and the cut-off
            // edge sits in whitespace.
            Item {
                id: trailingSpacer

                width: form.width
                height: Sizing.pctH(2)
            }
        }
    }

    // Top/bottom scroll chevrons — mirror the PagedGrid/BrowseList
    // recipe (same SVG icons, `PreserveAspectFit` + `smooth: true`)
    // but centered on the viewport in the chrome gap *above* and
    // *below* the Flickable, not inside its visible band. Sitting
    // outside the scrolled area means the chevrons never overlap
    // moving content as the user scrolls. Visible only when content
    // extends past the matching edge.
    Image {
        source: Resources.iconUrl("ScrollUp")
        width: Sizing.pctH(3)
        height: width
        anchors.bottom: flickable.top
        anchors.bottomMargin: Sizing.pctH(0.5)
        anchors.horizontalCenter: flickable.horizontalCenter
        fillMode: Image.PreserveAspectFit
        smooth: true
        visible: !settings.optimisticLoading && !settings.showingRootGrid && settings._hasContentAbove
    }

    Image {
        source: Resources.iconUrl("ScrollDown")
        width: Sizing.pctH(3)
        height: width
        anchors.top: flickable.bottom
        anchors.topMargin: Sizing.pctH(0.5)
        anchors.horizontalCenter: flickable.horizontalCenter
        fillMode: Image.PreserveAspectFit
        smooth: true
        visible: !settings.optimisticLoading && !settings.showingRootGrid && settings._hasContentBelow
    }

    // Empty-state placeholder shown on runtimes with no settings to
    // expose. Centered in the body so it doesn't compete with the
    // top strip or help bar.
    Text {
        x: Sizing.center(parent.width, width)
        y: Sizing.center(parent.height, height)
        visible: !settings.optimisticLoading && settings.fieldCount === 0
        text: qsTr("No settings available on this platform")
        color: Theme.textLabel
        font.family: Theme.fontUi
        font.pixelSize: Sizing.fontSize(2.6)
        renderType: Text.NativeRendering
    }

    ScreenStateOverlay {
        anchors.fill: parent
        enabled: settings.optimisticLoading
        loading: settings.optimisticLoading
        count: 0
        loadingText: qsTr("Loading settings…")
    }
}
