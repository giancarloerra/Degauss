// Zaparoo Frontend
// Copyright (c) 2026 Wizzo Pty Ltd and the Zaparoo Project contributors.
// SPDX-License-Identifier: LicenseRef-PolyForm-Noncommercial-1.0.0
//
// `Browse.Settings` — gamepad-accessible settings form. The model is the
// seam between the QML form and the persistence/runtime side: it owns
// curated picker lists, remembers what the user picked, and writes
// restart-applied settings back to config/state.
//
// Field design:
//   * `is_mister` — CONSTANT. Drives whether MiSTer-only fields render
//     in the form.
//   * `available_resolutions` — CONSTANT. Empty off MiSTer; on MiSTer,
//     the curated picker list. Order matters: it's the cycle order in
//     the UI's left/right cycler.
//   * `current_resolution` — READ + NOTIFY, persisted. Empty means "use
//     `[mister.video_*]` defaults from frontend.toml". The Settings
//     screen renders that empty value as `qsTr("Default")`.
//   * `available_languages` — CONSTANT. Curated language tags plus the
//     `auto` sentinel. The runtime translator is still startup-only, so
//     this setting applies on the next launch.
//   * `current_language` — READ + NOTIFY. Mirrors `[general].language`
//     from frontend.toml and is also recorded in persisted state so the
//     settings snapshot stays coherent.
//   * `available_clock_formats` — CONSTANT. Tri-state wall-clock format:
//     `auto` follows the effective UI locale, `12h` and `24h` force an
//     override.
//   * `current_clock_format` — READ + NOTIFY, persisted. Defaults to
//     `auto` so existing installs keep locale-driven behavior.
//   * `available_orientations` — CONSTANT. Three display transforms:
//     horizontal (default), rotated clockwise, rotated counter-clockwise.
//   * `current_orientation` — READ + NOTIFY, persisted. Applied live by
//     the QML scene wrapper while also mirrored into frontend.toml so
//     MiSTer survives `/tmp` resets.
//   * `available_browse_layouts` — CONSTANT. The browsing layout picker
//     choices. "grid" is the existing layout; "list" is the detailed list
//     placeholder until the new browsing screen is built.
//   * `current_browse_layout` — READ + NOTIFY, persisted. Defaults to
//     "grid" so existing installs keep current behavior.
//   * `available_system_logo_styles` — CONSTANT. "tinted" keeps the default
//     theme-colored SVGs; "color" opts into restored full-color logos.
//   * `current_system_logo_style` — READ + NOTIFY, persisted. Defaults to
//     "tinted" so existing installs keep current behavior.
//   * `available_button_layouts` — CONSTANT. Single-letter ids used to
//     compose resources/images/buttons/<layout>/Button*.png. User-facing
//     labels are "Style A/B/C/D" (see
//     `SettingsScreen.qml::_buttonLayoutDisplay`) so the picker stays a
//     neutral aesthetic choice and avoids implying platform affiliation.
//   * `current_button_layout` — READ + NOTIFY, persisted. Defaults to
//     "a" — the new id for the previous "nintendo" asset directory.
//     `normalize_button_layout` migrates legacy persisted values
//     (`nintendo`/`xbox`/`sony`) to the new ids so users keep their
//     selection across the rename.
//   * `current_mouse_enabled` — READ + NOTIFY, persisted. Defaults to true
//     so existing installs keep the visible cursor and mouse hit targets.
//   * `current_reduce_motion` — READ + NOTIFY, persisted. Defaults to false
//     (motion on). When true, all Behavior durations in the UI collapse to
//     0 via Motion.dur() so animations complete in one frame.
//   * `current_debug_logging` — READ + NOTIFY, persisted. Defaults to false.
//     Toggling it writes `[logging] debug = …` into frontend.toml; the
//     tracing subscriber is built once at startup so the change only takes
//     effect on the next launch (mirrors how `language` works).
// Frontend-owned durable settings are mirrored into both `state.toml`
// and `frontend.toml`. `state.toml` keeps the in-process snapshot
// coherent; `frontend.toml` is the durable copy that survives MiSTer's
// `/tmp` lifecycle and is what startup `vmode` / translator install
// read on the next process launch. Button layout only changes the QML
// resource path used by help-bar icons, browse layout selects the game
// browsing presentation, mouse support drives the QML cursor/input blocker,
// discover-arcade-alternate-versions gates placeholder menu affordances for
// MiSTer arcade alternates, and language still takes effect on the next launch
// because Qt installs translators only at startup.

use crate::models::{with_persist_mut, with_persist_read};
use cxx_qt::{CxxQtType, Initialize};
use cxx_qt_lib::{QString, QStringList};
use std::pin::Pin;
use tracing::warn;
use zaparoo_core::config::{load_config, save_settings_mirror, Config, SettingsMirror};
use zaparoo_core::persist::{self, SettingsState};
use zaparoo_core::platform_paths::config_file_path;
use zaparoo_core::runtime;

/// Curated `MiSTer` resolution choices. Order is the left/right cycle
/// order in the form. Keep the list short — every entry is a literal
/// the user can crash a CRT scaler with if it doesn't suit their
/// monitor — and ASCII-only so the QML side never needs to translate
/// the strings (they're not user-facing labels, they're keys). The
/// empty leading entry is the "use `frontend.toml` defaults" sentinel;
/// the form renders it as `qsTr("Default")` so users can cycle back
/// to no-override after picking a custom value.
const MISTER_RESOLUTIONS: &[&str] = &[
    "",
    "1280x720",
    "1920x1080",
    "2560x1440",
    "1920x1200",
    "1920x1440",
    "640x480",
    "2048x1536",
];
// One picker row per user-visible language. Region-specific tags are
// accepted below as aliases so old configs keep working without creating
// duplicate labels like English/English/English in the settings modal.
const LANGUAGES: &[&str] = &[
    "auto", "en", "it_IT", "es", "eu", "de", "el", "ja", "ko", "nl", "ro", "sk", "uk", "zh_CN",
    "zh_TW", "he", "ar", "hi",
];
const LANGUAGE_ALIASES: &[(&str, &str)] = &[
    ("en_US", "en"),
    ("en_GB", "en"),
    ("it", "it_IT"),
    ("es_ES", "es"),
    ("eu_ES", "eu"),
    ("de_DE", "de"),
    ("el_GR", "el"),
    ("ja_JP", "ja"),
    ("ko_KR", "ko"),
    ("nl_NL", "nl"),
    ("ro_RO", "ro"),
    ("sk_SK", "sk"),
    ("uk_UA", "uk"),
    ("zh_Hans", "zh_CN"),
    ("zh_Hans_CN", "zh_CN"),
    ("zh_Hant", "zh_TW"),
    ("zh_Hant_TW", "zh_TW"),
    ("zh_HK", "zh_TW"),
    ("zh_Hant_HK", "zh_TW"),
    ("he_IL", "he"),
    ("ar_SA", "ar"),
    ("hi_IN", "hi"),
];
const DEFAULT_LANGUAGE: &str = "auto";
const CLOCK_FORMATS: &[&str] = &["auto", "12h", "24h"];
const DEFAULT_CLOCK_FORMAT: &str = "auto";
const REGIONS: &[&str] = &["auto", "us", "eu", "jp"];
const DEFAULT_REGION: &str = "auto";
const ORIENTATIONS: &[&str] = &["horizontal", "cw", "ccw"];
const DEFAULT_ORIENTATION: &str = "horizontal";
const BROWSE_LAYOUTS: &[&str] = &["grid", "list"];
const DEFAULT_BROWSE_LAYOUT: &str = "grid";
const SYSTEM_LOGO_STYLES: &[&str] = &["tinted", "color"];
const DEFAULT_SYSTEM_LOGO_STYLE: &str = "tinted";
const THEMES: &[&str] = &["default", "crt-light"];
const DEFAULT_THEME: &str = "default";
const BUTTON_LAYOUTS: &[&str] = &["a", "b", "c", "d"];
const DEFAULT_BUTTON_LAYOUT: &str = "a";
// Screensaver idle-timeout choices. Values are seconds as ASCII
// strings, with the `"off"` sentinel meaning "never activate".
// Default of 5 minutes matches typical TV/console screensavers and
// is long enough that idle browsing does not trip it.
const SCREENSAVER_TIMEOUTS: &[&str] = &["off", "60", "120", "300", "600", "900", "1800"];
const DEFAULT_SCREENSAVER_TIMEOUT: &str = "300";
const MEDIA_IMAGE_TYPES: &[&str] = &[
    "auto",
    "image",
    "thumbnail",
    "boxart",
    "boxart3d",
    "screenshot",
    "wheel",
    "titleshot",
    "map",
    "marquee",
    "fanart",
    "boxartside",
    "boxartback",
];
const DEFAULT_MEDIA_IMAGE_TYPE: &str = "auto";

// Debug-only QA shortcut so the activation path can be exercised
// without waiting for the production timer. Only appears in debug
// builds; release builds drop both the picker entry and the
// normalization branch so a stray persisted "1" rounds back to the
// safe default.
#[cfg(debug_assertions)]
const SCREENSAVER_TIMEOUTS_DEBUG: &[&str] = &["1"];

#[allow(
    clippy::struct_excessive_bools,
    reason = "settings qobject is a persisted toggle bag exposed to QML"
)]
#[derive(Default)]
pub struct SettingsRust {
    is_mister: bool,
    available_resolutions: QStringList,
    current_resolution: QString,
    available_languages: QStringList,
    current_language: QString,
    available_clock_formats: QStringList,
    current_clock_format: QString,
    available_orientations: QStringList,
    current_orientation: QString,
    available_browse_layouts: QStringList,
    current_browse_layout: QString,
    available_system_logo_styles: QStringList,
    current_system_logo_style: QString,
    available_themes: QStringList,
    current_theme: QString,
    available_button_layouts: QStringList,
    current_button_layout: QString,
    current_mouse_enabled: bool,
    current_reduce_motion: bool,
    current_discover_arcade_alternate_versions: bool,
    current_debug_logging: bool,
    current_show_hidden: bool,
    current_show_original_filenames: bool,
    available_screensaver_timeouts: QStringList,
    current_screensaver_timeout: QString,
    available_media_image_types: QStringList,
    current_media_image_type: QString,
    available_regions: QStringList,
    current_region: QString,
}

#[cxx_qt::bridge]
pub mod ffi {
    unsafe extern "C++" {
        include!("model_includes.h");

        type QString = cxx_qt_lib::QString;
        type QStringList = cxx_qt_lib::QStringList;
    }

    unsafe extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        #[qproperty(bool, is_mister, READ, CONSTANT)]
        #[qproperty(QStringList, available_resolutions, READ, CONSTANT)]
        #[qproperty(QString, current_resolution, READ, WRITE = set_resolution, NOTIFY)]
        #[qproperty(QStringList, available_languages, READ, CONSTANT)]
        #[qproperty(QString, current_language, READ, WRITE = set_language, NOTIFY)]
        #[qproperty(QStringList, available_clock_formats, READ, CONSTANT)]
        #[qproperty(QString, current_clock_format, READ, WRITE = set_clock_format, NOTIFY)]
        #[qproperty(QStringList, available_orientations, READ, CONSTANT)]
        #[qproperty(QString, current_orientation, READ, WRITE = set_orientation, NOTIFY)]
        #[qproperty(QStringList, available_browse_layouts, READ, CONSTANT)]
        #[qproperty(QString, current_browse_layout, READ, WRITE = set_browse_layout, NOTIFY)]
        #[qproperty(QStringList, available_system_logo_styles, READ, CONSTANT)]
        #[qproperty(QString, current_system_logo_style, READ, WRITE = set_system_logo_style, NOTIFY)]
        #[qproperty(QStringList, available_themes, READ, CONSTANT)]
        #[qproperty(QString, current_theme, READ, WRITE = set_theme, NOTIFY)]
        #[qproperty(QStringList, available_button_layouts, READ, CONSTANT)]
        #[qproperty(QString, current_button_layout, READ, WRITE = set_button_layout, NOTIFY)]
        #[qproperty(bool, current_mouse_enabled, READ, WRITE = set_mouse_enabled, NOTIFY)]
        #[qproperty(bool, current_reduce_motion, READ, WRITE = set_reduce_motion, NOTIFY)]
        #[qproperty(bool, current_discover_arcade_alternate_versions, READ, WRITE = set_discover_arcade_alternate_versions, NOTIFY)]
        #[qproperty(bool, current_debug_logging, READ, WRITE = set_debug_logging, NOTIFY)]
        #[qproperty(bool, current_show_hidden, READ, WRITE = set_show_hidden, NOTIFY)]
        #[qproperty(bool, current_show_original_filenames, READ, WRITE = set_show_original_filenames, NOTIFY)]
        #[qproperty(QStringList, available_screensaver_timeouts, READ, CONSTANT)]
        #[qproperty(QString, current_screensaver_timeout, READ, WRITE = set_screensaver_timeout, NOTIFY)]
        #[qproperty(QStringList, available_media_image_types, READ, CONSTANT)]
        #[qproperty(QString, current_media_image_type, READ, WRITE = set_media_image_type, NOTIFY)]
        #[qproperty(QStringList, available_regions, READ, CONSTANT)]
        #[qproperty(QString, current_region, READ, WRITE = set_region, NOTIFY)]
        type Settings = super::SettingsRust;

        #[qinvokable]
        fn set_resolution(self: Pin<&mut Settings>, value: QString);

        #[qinvokable]
        fn set_language(self: Pin<&mut Settings>, value: QString);

        #[qinvokable]
        fn set_clock_format(self: Pin<&mut Settings>, value: QString);

        #[qinvokable]
        fn set_orientation(self: Pin<&mut Settings>, value: QString);

        #[qinvokable]
        fn set_browse_layout(self: Pin<&mut Settings>, value: QString);

        #[qinvokable]
        fn set_system_logo_style(self: Pin<&mut Settings>, value: QString);
        #[qinvokable]
        fn set_theme(self: Pin<&mut Settings>, value: QString);

        #[qinvokable]
        fn set_button_layout(self: Pin<&mut Settings>, value: QString);

        #[qinvokable]
        fn set_mouse_enabled(self: Pin<&mut Settings>, value: bool);

        #[qinvokable]
        fn set_reduce_motion(self: Pin<&mut Settings>, value: bool);

        #[qinvokable]
        fn set_discover_arcade_alternate_versions(self: Pin<&mut Settings>, value: bool);

        #[qinvokable]
        fn set_debug_logging(self: Pin<&mut Settings>, value: bool);

        #[qinvokable]
        fn set_screensaver_timeout(self: Pin<&mut Settings>, value: QString);

        #[qinvokable]
        fn set_media_image_type(self: Pin<&mut Settings>, value: QString);

        #[qinvokable]
        fn set_show_hidden(self: Pin<&mut Settings>, value: bool);

        #[qinvokable]
        fn set_show_original_filenames(self: Pin<&mut Settings>, value: bool);

        #[qinvokable]
        fn set_region(self: Pin<&mut Settings>, value: QString);
    }

    impl cxx_qt::Initialize for Settings {}
}

impl Initialize for ffi::Settings {
    fn initialize(mut self: Pin<&mut Self>) {
        let started = std::time::Instant::now();
        crate::startup_trace("rust:model Settings init start");
        let snapshot: SettingsState = with_persist_read(|s| s.settings.clone());
        let config_path = config_file_path();
        let is_mister = runtime::current().is_mister();
        let config = load_config(&config_path);
        let merged = merge_settings(&snapshot, &config);
        persist_if_changed(&snapshot, &merged);
        mirror_settings_to_config(&config_path, &merged);
        self.as_mut().rust_mut().is_mister = is_mister;
        self.as_mut().rust_mut().available_resolutions = if is_mister {
            curated_resolutions()
        } else {
            QStringList::default()
        };
        self.as_mut().rust_mut().current_resolution = QString::from(merged.resolution.as_str());
        self.as_mut().rust_mut().available_languages = languages();
        self.as_mut().rust_mut().current_language = QString::from(merged.language.as_str());
        self.as_mut().rust_mut().available_clock_formats = clock_formats();
        self.as_mut().rust_mut().current_clock_format = QString::from(merged.clock_format.as_str());
        self.as_mut().rust_mut().available_orientations = orientations();
        self.as_mut().rust_mut().current_orientation = QString::from(merged.orientation.as_str());
        self.as_mut().rust_mut().available_browse_layouts = browse_layouts();
        self.as_mut().rust_mut().current_browse_layout =
            QString::from(merged.browse_layout.as_str());
        self.as_mut().rust_mut().available_system_logo_styles = system_logo_styles();
        self.as_mut().rust_mut().current_system_logo_style =
            QString::from(merged.system_logo_style.as_str());
        self.as_mut().rust_mut().available_themes = themes();
        self.as_mut().rust_mut().current_theme = QString::from(merged.theme.as_str());
        self.as_mut().rust_mut().available_button_layouts = button_layouts();
        self.as_mut().rust_mut().current_button_layout =
            QString::from(merged.button_layout.as_str());
        self.as_mut().rust_mut().current_mouse_enabled = merged.mouse_enabled;
        self.as_mut().rust_mut().current_reduce_motion = merged.reduce_motion;
        self.as_mut()
            .rust_mut()
            .current_discover_arcade_alternate_versions = merged.discover_arcade_alternate_versions;
        self.as_mut().rust_mut().current_debug_logging = merged.debug_logging;
        self.as_mut().rust_mut().current_show_hidden = merged.show_hidden;
        self.as_mut().rust_mut().current_show_original_filenames = merged.show_original_filenames;
        self.as_mut().rust_mut().available_screensaver_timeouts = screensaver_timeouts();
        self.as_mut().rust_mut().current_screensaver_timeout =
            QString::from(merged.screensaver_timeout.as_str());
        self.as_mut().rust_mut().available_media_image_types = media_image_types();
        self.as_mut().rust_mut().current_media_image_type =
            QString::from(merged.media_image_type.as_str());
        self.as_mut().rust_mut().available_regions = regions();
        self.as_mut().rust_mut().current_region = QString::from(merged.region.as_str());
        crate::startup_trace(format!(
            "rust:model Settings init end dur_ms={}",
            started.elapsed().as_millis()
        ));
    }
}

impl ffi::Settings {
    fn set_resolution(mut self: Pin<&mut Self>, value: QString) {
        if self.current_resolution == value {
            return;
        }
        let value_str = value.to_string();
        // Resolution is restart-applied, so this setter only updates the
        // durable state/config read by the next frontend process.
        let snapshot = persist_settings(|s| s.resolution.clone_from(&value_str));
        mirror_settings_to_config(&config_file_path(), &snapshot.settings);
        self.as_mut().rust_mut().current_resolution = value;
        self.as_mut().current_resolution_changed();
    }

    #[allow(
        clippy::needless_pass_by_value,
        reason = "cxx-qt qinvokable signature requires QString by value"
    )]
    fn set_language(mut self: Pin<&mut Self>, value: QString) {
        let value_str = normalize_language(&value.to_string()).to_string();
        if self.current_language.to_string() == value_str {
            return;
        }
        let snapshot = persist_settings(|s| s.language.clone_from(&value_str));
        mirror_settings_to_config(&config_file_path(), &snapshot.settings);
        self.as_mut().rust_mut().current_language = QString::from(value_str.as_str());
        self.as_mut().current_language_changed();
    }

    #[allow(
        clippy::needless_pass_by_value,
        reason = "cxx-qt qinvokable signature requires QString by value"
    )]
    fn set_clock_format(mut self: Pin<&mut Self>, value: QString) {
        let value_str = normalize_clock_format(&value.to_string()).to_string();
        if self.current_clock_format.to_string() == value_str {
            return;
        }
        let snapshot = persist_settings(|s| s.clock_format.clone_from(&value_str));
        mirror_settings_to_config(&config_file_path(), &snapshot.settings);
        self.as_mut().rust_mut().current_clock_format = QString::from(value_str.as_str());
        self.as_mut().current_clock_format_changed();
    }

    #[allow(
        clippy::needless_pass_by_value,
        reason = "cxx-qt qinvokable signature requires QString by value"
    )]
    fn set_orientation(mut self: Pin<&mut Self>, value: QString) {
        let value_str = normalize_orientation(&value.to_string()).to_string();
        if self.current_orientation.to_string() == value_str {
            return;
        }
        let snapshot = persist_settings(|s| s.orientation.clone_from(&value_str));
        mirror_settings_to_config(&config_file_path(), &snapshot.settings);
        self.as_mut().rust_mut().current_orientation = QString::from(value_str.as_str());
        self.as_mut().current_orientation_changed();
    }

    #[allow(
        clippy::needless_pass_by_value,
        reason = "cxx-qt qinvokable signature requires QString by value"
    )]
    fn set_browse_layout(mut self: Pin<&mut Self>, value: QString) {
        let value_str = normalize_browse_layout(&value.to_string()).to_string();
        if self.current_browse_layout.to_string() == value_str {
            return;
        }
        let snapshot = persist_settings(|s| s.browse_layout.clone_from(&value_str));
        mirror_settings_to_config(&config_file_path(), &snapshot.settings);
        self.as_mut().rust_mut().current_browse_layout = QString::from(value_str.as_str());
        self.as_mut().current_browse_layout_changed();
    }

    #[allow(
        clippy::needless_pass_by_value,
        reason = "cxx-qt qinvokable signature requires QString by value"
    )]
    fn set_system_logo_style(mut self: Pin<&mut Self>, value: QString) {
        let value_str = normalize_system_logo_style(&value.to_string()).to_string();
        if self.current_system_logo_style.to_string() == value_str {
            return;
        }
        let snapshot = persist_settings(|s| s.system_logo_style.clone_from(&value_str));
        mirror_settings_to_config(&config_file_path(), &snapshot.settings);
        self.as_mut().rust_mut().current_system_logo_style = QString::from(value_str.as_str());
        self.as_mut().current_system_logo_style_changed();
    }

    #[allow(
        clippy::needless_pass_by_value,
        reason = "cxx-qt qinvokable signature requires QString by value"
    )]
    fn set_theme(mut self: Pin<&mut Self>, value: QString) {
        let value_str = normalize_theme(&value.to_string()).to_string();
        if self.current_theme.to_string() == value_str {
            return;
        }
        let snapshot = persist_settings(|s| s.theme.clone_from(&value_str));
        mirror_settings_to_config(&config_file_path(), &snapshot.settings);
        self.as_mut().rust_mut().current_theme = QString::from(value_str.as_str());
        self.as_mut().current_theme_changed();
    }

    #[allow(
        clippy::needless_pass_by_value,
        reason = "cxx-qt qinvokable signature requires QString by value"
    )]
    fn set_button_layout(mut self: Pin<&mut Self>, value: QString) {
        let value_str = normalize_button_layout(&value.to_string()).to_string();
        if self.current_button_layout.to_string() == value_str {
            return;
        }
        let snapshot = persist_settings(|s| s.button_layout.clone_from(&value_str));
        mirror_settings_to_config(&config_file_path(), &snapshot.settings);
        self.as_mut().rust_mut().current_button_layout = QString::from(value_str.as_str());
        self.as_mut().current_button_layout_changed();
    }

    fn set_mouse_enabled(mut self: Pin<&mut Self>, value: bool) {
        if self.current_mouse_enabled == value {
            return;
        }
        let snapshot = persist_settings(|s| s.mouse_enabled = value);
        mirror_settings_to_config(&config_file_path(), &snapshot.settings);
        self.as_mut().rust_mut().current_mouse_enabled = value;
        self.as_mut().current_mouse_enabled_changed();
    }

    fn set_reduce_motion(mut self: Pin<&mut Self>, value: bool) {
        if self.current_reduce_motion == value {
            return;
        }
        let snapshot = persist_settings(|s| s.reduce_motion = value);
        mirror_settings_to_config(&config_file_path(), &snapshot.settings);
        self.as_mut().rust_mut().current_reduce_motion = value;
        self.as_mut().current_reduce_motion_changed();
    }

    fn set_discover_arcade_alternate_versions(mut self: Pin<&mut Self>, value: bool) {
        if self.current_discover_arcade_alternate_versions == value {
            return;
        }
        let snapshot = persist_settings(|s| s.discover_arcade_alternate_versions = value);
        mirror_settings_to_config(&config_file_path(), &snapshot.settings);
        self.as_mut()
            .rust_mut()
            .current_discover_arcade_alternate_versions = value;
        self.as_mut()
            .current_discover_arcade_alternate_versions_changed();
    }

    fn set_debug_logging(mut self: Pin<&mut Self>, value: bool) {
        if self.current_debug_logging == value {
            return;
        }
        let snapshot = persist_settings(|s| s.debug_logging = value);
        mirror_settings_to_config(&config_file_path(), &snapshot.settings);
        self.as_mut().rust_mut().current_debug_logging = value;
        self.as_mut().current_debug_logging_changed();
    }

    #[allow(
        clippy::needless_pass_by_value,
        reason = "cxx-qt qinvokable signature requires QString by value"
    )]
    fn set_screensaver_timeout(mut self: Pin<&mut Self>, value: QString) {
        let value_str = normalize_screensaver_timeout(&value.to_string()).to_string();
        if self.current_screensaver_timeout.to_string() == value_str {
            return;
        }
        let snapshot = persist_settings(|s| s.screensaver_timeout.clone_from(&value_str));
        mirror_settings_to_config(&config_file_path(), &snapshot.settings);
        self.as_mut().rust_mut().current_screensaver_timeout = QString::from(value_str.as_str());
        self.as_mut().current_screensaver_timeout_changed();
    }

    #[allow(
        clippy::needless_pass_by_value,
        reason = "cxx-qt qinvokable signature requires QString by value"
    )]
    fn set_media_image_type(mut self: Pin<&mut Self>, value: QString) {
        let value_str = normalize_media_image_type(&value.to_string()).to_string();
        if self.current_media_image_type.to_string() == value_str {
            return;
        }
        let snapshot = persist_settings(|s| s.media_image_type.clone_from(&value_str));
        mirror_settings_to_config(&config_file_path(), &snapshot.settings);
        self.as_mut().rust_mut().current_media_image_type = QString::from(value_str.as_str());
        self.as_mut().current_media_image_type_changed();
    }

    fn set_show_hidden(mut self: Pin<&mut Self>, value: bool) {
        if self.current_show_hidden == value {
            return;
        }
        let snapshot = persist_settings(|s| s.show_hidden = value);
        mirror_settings_to_config(&config_file_path(), &snapshot.settings);
        self.as_mut().rust_mut().current_show_hidden = value;
        self.as_mut().current_show_hidden_changed();
    }

    fn set_show_original_filenames(mut self: Pin<&mut Self>, value: bool) {
        if self.current_show_original_filenames == value {
            return;
        }
        let snapshot = persist_settings(|s| s.show_original_filenames = value);
        mirror_settings_to_config(&config_file_path(), &snapshot.settings);
        self.as_mut().rust_mut().current_show_original_filenames = value;
        self.as_mut().current_show_original_filenames_changed();
    }

    #[allow(
        clippy::needless_pass_by_value,
        reason = "cxx-qt qinvokable signature requires QString by value"
    )]
    fn set_region(mut self: Pin<&mut Self>, value: QString) {
        let value_str = normalize_region(&value.to_string()).to_string();
        if self.current_region.to_string() == value_str {
            return;
        }
        let snapshot = persist_settings(|s| s.region.clone_from(&value_str));
        mirror_settings_to_config(&config_file_path(), &snapshot.settings);
        self.as_mut().rust_mut().current_region = QString::from(value_str.as_str());
        self.as_mut().current_region_changed();
    }
}

pub(super) fn persist_settings<F: FnOnce(&mut SettingsState)>(
    mutator: F,
) -> persist::PersistedState {
    let snapshot = with_persist_mut(|s| {
        mutator(&mut s.settings);
        s.clone()
    });
    persist::save(&snapshot);
    snapshot
}

fn persist_if_changed(current: &SettingsState, merged: &SettingsState) {
    if current == merged {
        return;
    }
    let snapshot = with_persist_mut(|s| {
        s.settings = merged.clone();
        s.clone()
    });
    persist::save(&snapshot);
}

pub(super) fn mirror_settings_to_config(config_path: &std::path::Path, settings: &SettingsState) {
    if let Err(e) = save_settings_mirror(
        config_path,
        SettingsMirror {
            resolution: settings.resolution.as_str(),
            language: settings.language.as_str(),
            orientation: settings.orientation.as_str(),
            clock_format: settings.clock_format.as_str(),
            browse_layout: settings.browse_layout.as_str(),
            system_logo_style: settings.system_logo_style.as_str(),
            theme: settings.theme.as_str(),
            button_layout: settings.button_layout.as_str(),
            mouse_enabled: settings.mouse_enabled,
            reduce_motion: settings.reduce_motion,
            discover_arcade_alternate_versions: settings.discover_arcade_alternate_versions,
            debug_logging: settings.debug_logging,
            screensaver_timeout: settings.screensaver_timeout.as_str(),
            media_image_type: settings.media_image_type.as_str(),
            show_hidden: settings.show_hidden,
            show_original_filenames: settings.show_original_filenames,
            region: settings.region.as_str(),
            crt_video_standard: settings.crt_video_standard.as_str(),
            crt_h_offset: settings.crt_h_offset,
            crt_v_offset: settings.crt_v_offset,
        },
    ) {
        warn!(
            "could not save settings mirror to {}: {e}",
            config_path.display()
        );
    }
}

// Resolve the native-CRT settings (video standard plus clamped centering
// trims) from config-over-snapshot. Split out of `merge_settings` so that
// function stays within the clippy line budget.
fn merge_crt_settings(snapshot: &SettingsState, config: &Config) -> (String, i32, i32) {
    let (h_offset, v_offset) = zaparoo_core::config::clamp_crt_offsets(
        config
            .settings
            .crt_h_offset
            .unwrap_or(snapshot.crt_h_offset),
        config
            .settings
            .crt_v_offset
            .unwrap_or(snapshot.crt_v_offset),
    );
    let standard = zaparoo_core::config::normalize_crt_video_standard(
        config
            .settings
            .crt_video_standard
            .as_deref()
            .unwrap_or(snapshot.crt_video_standard.as_str()),
    )
    .to_string();
    (standard, h_offset, v_offset)
}

/// Merge one string setting: the config value wins when present,
/// falling back to the persisted snapshot, normalized either way.
fn merge_string_setting(
    config_value: Option<&str>,
    snapshot_value: &str,
    normalize: impl Fn(&str) -> &'static str,
) -> String {
    normalize(config_value.unwrap_or(snapshot_value)).to_string()
}

fn merge_settings(snapshot: &SettingsState, config: &Config) -> SettingsState {
    let (crt_video_standard, crt_h_offset, crt_v_offset) = merge_crt_settings(snapshot, config);
    SettingsState {
        resolution: if config.video_explicit {
            format!("{}x{}", config.video_width, config.video_height)
        } else {
            String::new()
        },
        language: normalize_language(&config.language).to_string(),
        orientation: normalize_orientation(
            config
                .settings
                .orientation
                .as_deref()
                .unwrap_or(snapshot.orientation.as_str()),
        )
        .to_string(),
        clock_format: normalize_clock_format(
            config
                .settings
                .clock_format
                .as_deref()
                .unwrap_or(snapshot.clock_format.as_str()),
        )
        .to_string(),
        browse_layout: normalize_browse_layout(
            config
                .settings
                .browse_layout
                .as_deref()
                .unwrap_or(snapshot.browse_layout.as_str()),
        )
        .to_string(),
        system_logo_style: merge_string_setting(
            config.settings.system_logo_style.as_deref(),
            snapshot.system_logo_style.as_str(),
            normalize_system_logo_style,
        ),
        theme: merge_string_setting(
            config.settings.theme.as_deref(),
            snapshot.theme.as_str(),
            normalize_theme,
        ),
        button_layout: normalize_button_layout(
            config
                .settings
                .button_layout
                .as_deref()
                .unwrap_or(snapshot.button_layout.as_str()),
        )
        .to_string(),
        mouse_enabled: config
            .settings
            .mouse_enabled
            .unwrap_or(snapshot.mouse_enabled),
        reduce_motion: config
            .settings
            .reduce_motion
            .unwrap_or(snapshot.reduce_motion),
        discover_arcade_alternate_versions: config
            .settings
            .discover_arcade_alternate_versions
            .unwrap_or(snapshot.discover_arcade_alternate_versions),
        // Config wins so frontend.toml is the durable source of truth on
        // MiSTer (state.toml lives on tmpfs).
        debug_logging: config.debug_logging,
        screensaver_timeout: normalize_screensaver_timeout(
            config
                .settings
                .screensaver_timeout
                .as_deref()
                .unwrap_or(snapshot.screensaver_timeout.as_str()),
        )
        .to_string(),
        media_image_type: normalize_media_image_type(
            config
                .settings
                .media_image_type
                .as_deref()
                .unwrap_or(snapshot.media_image_type.as_str()),
        )
        .to_string(),
        show_hidden: config.settings.show_hidden.unwrap_or(snapshot.show_hidden),
        show_original_filenames: config
            .settings
            .show_original_filenames
            .unwrap_or(snapshot.show_original_filenames),
        region: normalize_region(
            config
                .settings
                .region
                .as_deref()
                .unwrap_or(snapshot.region.as_str()),
        )
        .to_string(),
        crt_video_standard,
        crt_h_offset,
        crt_v_offset,
    }
}

fn curated_resolutions() -> QStringList {
    let mut list = QStringList::default();
    for r in MISTER_RESOLUTIONS {
        list.append(QString::from(*r));
    }
    list
}

fn button_layouts() -> QStringList {
    let mut list = QStringList::default();
    for layout in BUTTON_LAYOUTS {
        list.append(QString::from(*layout));
    }
    list
}

fn system_logo_styles() -> QStringList {
    let mut list = QStringList::default();
    for style in SYSTEM_LOGO_STYLES {
        list.append(QString::from(*style));
    }
    list
}

fn themes() -> QStringList {
    let mut list = QStringList::default();
    for theme in THEMES {
        list.append(QString::from(*theme));
    }
    list
}

fn browse_layouts() -> QStringList {
    let mut list = QStringList::default();
    for layout in BROWSE_LAYOUTS {
        list.append(QString::from(*layout));
    }
    list
}

fn orientations() -> QStringList {
    let mut list = QStringList::default();
    for orientation in ORIENTATIONS {
        list.append(QString::from(*orientation));
    }
    list
}

fn languages() -> QStringList {
    let mut list = QStringList::default();
    for language in LANGUAGES {
        list.append(QString::from(*language));
    }
    list
}

fn clock_formats() -> QStringList {
    let mut list = QStringList::default();
    for format in CLOCK_FORMATS {
        list.append(QString::from(*format));
    }
    list
}

fn screensaver_timeouts() -> QStringList {
    let mut list = QStringList::default();
    #[cfg(debug_assertions)]
    for value in SCREENSAVER_TIMEOUTS_DEBUG {
        list.append(QString::from(*value));
    }
    for value in SCREENSAVER_TIMEOUTS {
        list.append(QString::from(*value));
    }
    list
}

fn media_image_types() -> QStringList {
    let mut list = QStringList::default();
    for value in MEDIA_IMAGE_TYPES {
        list.append(QString::from(*value));
    }
    list
}

fn regions() -> QStringList {
    let mut list = QStringList::default();
    for region in REGIONS {
        list.append(QString::from(*region));
    }
    list
}

fn normalize_language(value: &str) -> &str {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("auto") {
        return DEFAULT_LANGUAGE;
    }
    if let Some(language) = LANGUAGES
        .iter()
        .copied()
        .find(|language| *language == trimmed)
    {
        return language;
    }
    LANGUAGE_ALIASES
        .iter()
        .find_map(|(alias, language)| (*alias == trimmed).then_some(*language))
        .unwrap_or(DEFAULT_LANGUAGE)
}

fn normalize_clock_format(value: &str) -> &'static str {
    let trimmed = value.trim();
    CLOCK_FORMATS
        .iter()
        .copied()
        .find(|format| *format == trimmed)
        .unwrap_or(DEFAULT_CLOCK_FORMAT)
}

fn normalize_orientation(value: &str) -> &'static str {
    let trimmed = value.trim();
    ORIENTATIONS
        .iter()
        .copied()
        .find(|orientation| *orientation == trimmed)
        .unwrap_or(DEFAULT_ORIENTATION)
}

fn normalize_browse_layout(value: &str) -> &'static str {
    let trimmed = value.trim();
    BROWSE_LAYOUTS
        .iter()
        .copied()
        .find(|layout| *layout == trimmed)
        .unwrap_or(DEFAULT_BROWSE_LAYOUT)
}

fn normalize_system_logo_style(value: &str) -> &'static str {
    let trimmed = value.trim();
    SYSTEM_LOGO_STYLES
        .iter()
        .copied()
        .find(|style| *style == trimmed)
        .unwrap_or(DEFAULT_SYSTEM_LOGO_STYLE)
}

fn normalize_theme(value: &str) -> &'static str {
    let trimmed = value.trim();
    THEMES
        .iter()
        .copied()
        .find(|theme| *theme == trimmed)
        .unwrap_or(DEFAULT_THEME)
}

fn normalize_screensaver_timeout(value: &str) -> &'static str {
    let trimmed = value.trim();
    #[cfg(debug_assertions)]
    if let Some(found) = SCREENSAVER_TIMEOUTS_DEBUG
        .iter()
        .copied()
        .find(|v| *v == trimmed)
    {
        return found;
    }
    SCREENSAVER_TIMEOUTS
        .iter()
        .copied()
        .find(|v| *v == trimmed)
        .unwrap_or(DEFAULT_SCREENSAVER_TIMEOUT)
}

fn normalize_media_image_type(value: &str) -> &'static str {
    let trimmed = value.trim();
    MEDIA_IMAGE_TYPES
        .iter()
        .copied()
        .find(|v| *v == trimmed)
        .unwrap_or(DEFAULT_MEDIA_IMAGE_TYPE)
}

fn normalize_region(value: &str) -> &'static str {
    let trimmed = value.trim();
    REGIONS
        .iter()
        .copied()
        .find(|r| *r == trimmed)
        .unwrap_or(DEFAULT_REGION)
}

fn normalize_button_layout(value: &str) -> &'static str {
    let trimmed = value.trim();
    // Legacy alias map: state files written by builds before the
    // a/b/c rename hold "nintendo"/"xbox"/"sony"; preserve the user's
    // pick instead of silently snapping back to the default.
    let migrated = match trimmed {
        "nintendo" => "a",
        "xbox" => "b",
        "sony" => "c",
        other => other,
    };
    BUTTON_LAYOUTS
        .iter()
        .copied()
        .find(|layout| *layout == migrated)
        .unwrap_or(DEFAULT_BUTTON_LAYOUT)
}

#[cfg(test)]
mod tests {
    use super::{
        browse_layouts, button_layouts, clock_formats, curated_resolutions, languages,
        normalize_browse_layout, normalize_button_layout, normalize_clock_format,
        normalize_language, normalize_orientation, normalize_region, normalize_system_logo_style,
        orientations, regions, system_logo_styles, BROWSE_LAYOUTS, BUTTON_LAYOUTS, CLOCK_FORMATS,
        DEFAULT_BROWSE_LAYOUT, DEFAULT_BUTTON_LAYOUT, DEFAULT_CLOCK_FORMAT, DEFAULT_LANGUAGE,
        DEFAULT_ORIENTATION, DEFAULT_REGION, DEFAULT_SYSTEM_LOGO_STYLE, LANGUAGES,
        MISTER_RESOLUTIONS, ORIENTATIONS, REGIONS, SYSTEM_LOGO_STYLES,
    };

    #[test]
    fn curated_resolutions_preserves_order() {
        let list = curated_resolutions();
        let collected: Vec<String> = list.iter().map(String::from).collect();
        let expected: Vec<String> = MISTER_RESOLUTIONS
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        assert_eq!(collected, expected);
    }

    #[test]
    fn curated_list_contains_common_16_9_resolutions() {
        // Mostly a sanity guard — if a future edit silently drops the
        // most-likely-to-work 16:9 resolutions, this test catches it.
        let collected: Vec<&str> = MISTER_RESOLUTIONS.to_vec();
        assert!(collected.contains(&"1280x720"));
        assert!(collected.contains(&"1920x1080"));
        assert!(collected.contains(&"2560x1440"));
    }

    #[test]
    fn button_layouts_preserve_order() {
        let list = button_layouts();
        let collected: Vec<String> = list.iter().map(String::from).collect();
        let expected: Vec<String> = BUTTON_LAYOUTS.iter().map(|s| (*s).to_string()).collect();
        assert_eq!(collected, expected);
    }

    #[test]
    fn languages_preserve_order() {
        let list = languages();
        let collected: Vec<String> = list.iter().map(String::from).collect();
        let expected: Vec<String> = LANGUAGES.iter().map(|s| (*s).to_string()).collect();
        assert_eq!(collected, expected);
    }

    #[test]
    fn clock_formats_preserve_order() {
        let list = clock_formats();
        let collected: Vec<String> = list.iter().map(String::from).collect();
        let expected: Vec<String> = CLOCK_FORMATS.iter().map(|s| (*s).to_string()).collect();
        assert_eq!(collected, expected);
    }

    #[test]
    fn orientations_preserve_order() {
        let list = orientations();
        let collected: Vec<String> = list.iter().map(String::from).collect();
        let expected: Vec<String> = ORIENTATIONS.iter().map(|s| (*s).to_string()).collect();
        assert_eq!(collected, expected);
    }

    #[test]
    fn browse_layouts_preserve_order() {
        let list = browse_layouts();
        let collected: Vec<String> = list.iter().map(String::from).collect();
        let expected: Vec<String> = BROWSE_LAYOUTS.iter().map(|s| (*s).to_string()).collect();
        assert_eq!(collected, expected);
    }

    #[test]
    fn system_logo_styles_preserve_order() {
        let list = system_logo_styles();
        let collected: Vec<String> = list.iter().map(String::from).collect();
        let expected: Vec<String> = SYSTEM_LOGO_STYLES
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        assert_eq!(collected, expected);
    }

    #[test]
    fn browse_layout_normalization_defaults_to_grid() {
        assert_eq!(normalize_browse_layout(""), DEFAULT_BROWSE_LAYOUT);
        assert_eq!(normalize_browse_layout("detail"), DEFAULT_BROWSE_LAYOUT);
        assert_eq!(normalize_browse_layout("grid"), "grid");
        assert_eq!(normalize_browse_layout("list"), "list");
    }

    #[test]
    fn system_logo_style_normalization_defaults_to_tinted() {
        assert_eq!(normalize_system_logo_style(""), DEFAULT_SYSTEM_LOGO_STYLE);
        assert_eq!(
            normalize_system_logo_style("sepia"),
            DEFAULT_SYSTEM_LOGO_STYLE
        );
        assert_eq!(normalize_system_logo_style("tinted"), "tinted");
        assert_eq!(normalize_system_logo_style("color"), "color");
    }

    #[test]
    fn clock_format_normalization_defaults_to_auto() {
        assert_eq!(normalize_clock_format(""), DEFAULT_CLOCK_FORMAT);
        assert_eq!(normalize_clock_format("system"), DEFAULT_CLOCK_FORMAT);
        assert_eq!(normalize_clock_format("auto"), "auto");
        assert_eq!(normalize_clock_format("12h"), "12h");
        assert_eq!(normalize_clock_format("24h"), "24h");
    }

    #[test]
    fn orientation_normalization_defaults_to_horizontal() {
        assert_eq!(normalize_orientation(""), DEFAULT_ORIENTATION);
        assert_eq!(normalize_orientation("sideways"), DEFAULT_ORIENTATION);
        assert_eq!(normalize_orientation("horizontal"), "horizontal");
        assert_eq!(normalize_orientation("cw"), "cw");
        assert_eq!(normalize_orientation("ccw"), "ccw");
    }

    #[test]
    fn language_normalization_defaults_to_auto() {
        assert_eq!(normalize_language(""), DEFAULT_LANGUAGE);
        assert_eq!(normalize_language("auto"), DEFAULT_LANGUAGE);
        assert_eq!(normalize_language("AUTO"), DEFAULT_LANGUAGE);
        assert_eq!(normalize_language("fr"), DEFAULT_LANGUAGE);
        assert_eq!(normalize_language("it_IT"), "it_IT");
        assert_eq!(normalize_language("es"), "es");
        assert_eq!(normalize_language("eu"), "eu");
        assert_eq!(normalize_language("zh_TW"), "zh_TW");
    }

    #[test]
    fn language_normalization_migrates_region_aliases() {
        assert_eq!(normalize_language("en_US"), "en");
        assert_eq!(normalize_language("en_GB"), "en");
        assert_eq!(normalize_language("it"), "it_IT");
        assert_eq!(normalize_language("es_ES"), "es");
        assert_eq!(normalize_language("eu_ES"), "eu");
        assert_eq!(normalize_language("de_DE"), "de");
        assert_eq!(normalize_language("el_GR"), "el");
        assert_eq!(normalize_language("ja_JP"), "ja");
        assert_eq!(normalize_language("ko_KR"), "ko");
        assert_eq!(normalize_language("nl_NL"), "nl");
        assert_eq!(normalize_language("ro_RO"), "ro");
        assert_eq!(normalize_language("sk_SK"), "sk");
        assert_eq!(normalize_language("uk_UA"), "uk");
        assert_eq!(normalize_language("zh_Hans_CN"), "zh_CN");
        assert_eq!(normalize_language("zh_HK"), "zh_TW");
        assert_eq!(normalize_language("he_IL"), "he");
        assert_eq!(normalize_language("ar_SA"), "ar");
        assert_eq!(normalize_language("hi_IN"), "hi");
    }

    #[test]
    fn button_layout_values_are_lowercase() {
        for layout in BUTTON_LAYOUTS {
            assert_eq!(*layout, layout.to_ascii_lowercase());
        }
    }

    #[test]
    fn button_layout_normalization_defaults_to_a() {
        assert_eq!(normalize_button_layout(""), DEFAULT_BUTTON_LAYOUT);
        assert_eq!(
            normalize_button_layout("playstation"),
            DEFAULT_BUTTON_LAYOUT
        );
        assert_eq!(normalize_button_layout("b"), "b");
        assert_eq!(normalize_button_layout("d"), "d");
    }

    #[test]
    fn button_layout_migrates_legacy_vendor_ids() {
        assert_eq!(normalize_button_layout("nintendo"), "a");
        assert_eq!(normalize_button_layout("xbox"), "b");
        assert_eq!(normalize_button_layout("sony"), "c");
    }

    #[test]
    fn regions_preserve_order() {
        let list = regions();
        let collected: Vec<String> = list.iter().map(String::from).collect();
        let expected: Vec<String> = REGIONS.iter().map(|s| (*s).to_string()).collect();
        assert_eq!(collected, expected);
    }

    #[test]
    fn region_normalization_defaults_to_auto() {
        assert_eq!(normalize_region(""), DEFAULT_REGION);
        assert_eq!(normalize_region("unknown"), DEFAULT_REGION);
        assert_eq!(normalize_region("  "), DEFAULT_REGION);
        assert_eq!(normalize_region("auto"), "auto");
        assert_eq!(normalize_region("us"), "us");
        assert_eq!(normalize_region("eu"), "eu");
        assert_eq!(normalize_region("jp"), "jp");
    }

    #[test]
    fn region_values_are_lowercase() {
        for region in REGIONS {
            assert_eq!(*region, region.to_ascii_lowercase());
        }
    }
}
