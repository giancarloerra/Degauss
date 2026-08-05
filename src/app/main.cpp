// Zaparoo Frontend
// Copyright (c) 2026 Wizzo Pty Ltd and the Zaparoo Project contributors.
// SPDX-License-Identifier: LicenseRef-PolyForm-Noncommercial-1.0.0
//
// Thin C++ entry point for the Rust frontend. Domain logic lives in the
// zaparoo_frontend_rs staticlib; Qt plugin wiring is handled here so that
// Qt's CMake (qt_import_qml_plugins) can emit the correct link flags.

#include "custom_image_provider.h"
#include "media_image_provider.h"
#include "native_video_writer.h"
#include "tinted_svg_image_provider.h"

#include <QByteArray>
#include <QChar>
#include <QFont>
#include <QFontDatabase>
#include <QGuiApplication>
#include <QList>
#include <QLocale>
#include <QPixmapCache>
#include <QQmlApplicationEngine>
#include <QQuickStyle>
#include <QQuickWindow>
#include <QString>
#include <QStringList>
#include <QTranslator>
#include <QUrl>
#include <QVariantMap>
#include <QtQml/qqmlextensionplugin.h>
#include <algorithm>
#include <cerrno>
#include <cstddef>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <unistd.h>
#include <vector>

// Default QPixmapCache cap is 10 MiB. With ~100 system SVGs rasterized at
// 256 px sourceSize the working set straddles that limit, so navigating
// through every category evicts earlier system covers and re-renders
// them on the next visit. Bumping to 50 MiB keeps the entire system-
// cover set resident across category swaps for the cost of a one-time
// allocation — a worthwhile trade on MiSTer's 1 GiB DDR3 since
// pixmap decode on the UI thread is the visible "pop in" the user
// flagged.
constexpr int kPixmapCacheLimitKiB = 50 * 1024;

extern "C" int zaparoo_rust_init(bool crtNativePathForced);
extern "C" void zaparoo_rust_post_qt_start();
extern "C" void zaparoo_rust_shutdown();
extern "C" void zaparoo_log_qt(uint8_t level, const char* msg, size_t len);
extern "C" const char* zaparoo_rust_language_code();
extern "C" bool zaparoo_rust_crt_native_path_enabled();
extern "C" uint32_t zaparoo_rust_video_width();
extern "C" uint32_t zaparoo_rust_video_height();
extern "C" bool zaparoo_rust_debug_logging_enabled();
extern "C" void zaparoo_rust_trace_startup(const uint8_t* stage, size_t len);
// Push the effective UI locale into Rust so `system_region::current_region()`
// can resolve `auto` without calling back into Qt. Called once after
// `zaparoo_rust_init()` and the QLocale resolution below.
extern "C" void zaparoo_rust_set_effective_locale(const uint8_t* locale, size_t len);

// Pull Zaparoo QML plugin symbols into the final binary so the linker does
// not strip their static-initializer registration functions.
Q_IMPORT_QML_PLUGIN(Zaparoo_AppPlugin)
Q_IMPORT_QML_PLUGIN(Zaparoo_UiPlugin)
Q_IMPORT_QML_PLUGIN(Zaparoo_ThemePlugin)
Q_IMPORT_QML_PLUGIN(Zaparoo_ScreensPlugin)
#ifdef ZAPAROO_UPDATE_STATIC_QML_PLUGIN
Q_IMPORT_QML_PLUGIN(Zaparoo_UpdatePlugin)
#endif
Q_IMPORT_QML_PLUGIN(Zaparoo_Browse_plugin)
#ifdef ZAPAROO_UPDATE_STATIC_NATIVE_PLUGIN
Q_IMPORT_QML_PLUGIN(Zaparoo_Update_Native_plugin)
#endif

// For static Qt builds (MiSTer ARM32): the QtQuick.Controls plugin chain and
// platform plugin are embedded in the binary, not found on disk, so they
// must be explicitly imported. On dynamic (desktop) Qt these are loaded
// automatically and the symbols don't exist as static functions.
#ifdef QT_STATIC
#include <QtPlugin>
Q_IMPORT_QML_PLUGIN(QtQuickControls2Plugin)
Q_IMPORT_QML_PLUGIN(QtQuickControls2BasicStylePlugin)
Q_IMPORT_QML_PLUGIN(QtQuickControls2ImplPlugin)
Q_IMPORT_QML_PLUGIN(QtQuickTemplates2Plugin)
Q_IMPORT_QML_PLUGIN(QtQuick_WindowPlugin)
Q_IMPORT_PLUGIN(QLinuxFbIntegrationPlugin)
#ifdef ZAPAROO_STATIC_EGLFS_PLUGIN
Q_IMPORT_PLUGIN(QEglFSIntegrationPlugin)
#endif
#ifdef ZAPAROO_STATIC_EGLFS_KMS_GBM_PLUGIN
Q_IMPORT_PLUGIN(QEglFSKmsGbmIntegrationPlugin)
#endif
Q_IMPORT_PLUGIN(QSvgPlugin)
#endif

// Forward all Qt log messages to the Rust tracing registry (same sinks as
// Rust-side log output: stderr + frontend.log). Installed after
// zaparoo_rust_init() so the tracing subscriber is already alive.
static void qtMessageHandler(QtMsgType type, const QMessageLogContext& /*ctx*/, const QString& msg)
{
    const QByteArray utf8 = msg.toUtf8();
    zaparoo_log_qt(static_cast<uint8_t>(type), utf8.constData(), static_cast<size_t>(utf8.size()));
}

struct ParsedArguments
{
    bool crtNativePathForced = false;
    std::vector<char*> argv;
    // Unfiltered process arguments (nullptr-terminated). The restart
    // execvp must use these, not `argv`: `argv` has `--crt` stripped
    // for Qt, and restarting with the filtered vector would silently
    // drop the native CRT path on any restart-applied setting change.
    std::vector<char*> originalArgv;
};

static ParsedArguments extractCrtArgument(int argc, char* argv[])
{
    ParsedArguments parsed;
    parsed.argv.reserve(static_cast<size_t>(argc));
    std::copy_n(argv, argc, std::back_inserter(parsed.argv));
    parsed.originalArgv = parsed.argv;
    parsed.originalArgv.push_back(nullptr);

    std::vector<char*> filtered;
    filtered.reserve(parsed.argv.size());
    if (!parsed.argv.empty())
    {
        filtered.push_back(parsed.argv.front());
    }

    for (size_t i = 1; i < parsed.argv.size(); ++i)
    {
        char* arg = parsed.argv.at(i);
        if (std::strcmp(arg, "--crt") == 0)
        {
            parsed.crtNativePathForced = true;
            continue;
        }
        filtered.push_back(arg);
    }

    parsed.argv = std::move(filtered);
    parsed.argv.push_back(nullptr);
    return parsed;
}

static bool envFlagEnabled(const char* name)
{
    const QByteArray value = qgetenv(name).trimmed().toLower();
    return value == "1" || value == "true" || value == "yes" || value == "on";
}

constexpr int kRestartExitCode = 1000;

static void startupTrace(const char* stage)
{
    if (!zaparoo_rust_debug_logging_enabled())
    {
        return;
    }
    const size_t len = std::strlen(stage);
    // Rust FFI accepts raw UTF-8 bytes; char storage is the source buffer.
    // NOLINTNEXTLINE(cppcoreguidelines-pro-type-reinterpret-cast)
    zaparoo_rust_trace_startup(reinterpret_cast<const uint8_t*>(stage), len);
}

int main(int argc, char* argv[]) // NOLINT
{
    ParsedArguments parsedArgs = extractCrtArgument(argc, argv);
    const bool crtPreviewResolutionForced =
        !qEnvironmentVariableIsEmpty("ZAPAROO_CRT_PREVIEW_RESOLUTION");
    const bool crtNativePathForced = parsedArgs.crtNativePathForced || crtPreviewResolutionForced;
    const bool debugCrtSafeAreaOverlay = envFlagEnabled("ZAPAROO_DEBUG");
    int qtArgc = static_cast<int>(parsedArgs.argv.size()) - 1;

    char** qtArgv = parsedArgs.argv.data();

    // Desktop CRT preview: pin Qt's high-DPI handling so logical pixels
    // map 1:1 to physical pixels. Without this, on a screen with
    // devicePixelRatio != 1 the GL backend bilinear-filters the final
    // logical-to-physical present step, smearing the integer-upscaled
    // CRT output even when `layer.smooth: false` is set on the
    // upscale wrapper. Forcing scale factor to 1 keeps the window at
    // physical pixel size and the wrapper's nearest-neighbour scale
    // produces the literal pixel grid the preview is meant to expose.
    if (crtNativePathForced)
    {
        qputenv("QT_ENABLE_HIGHDPI_SCALING", "0");
        qputenv("QT_SCALE_FACTOR", "1");
        QGuiApplication::setHighDpiScaleFactorRoundingPolicy(
            Qt::HighDpiScaleFactorRoundingPolicy::Floor);
    }

    QGuiApplication::setApplicationName("Degauss");
    QGuiApplication::setApplicationVersion(ZAPAROO_VERSION);
    QGuiApplication::setOrganizationName("Degauss");

    if (zaparoo_rust_init(crtNativePathForced) != 0)
    {
        return EXIT_FAILURE;
    }
    startupTrace("cpp:rust init complete");

    // Start Core before Qt/font/QML setup so service boot overlaps the
    // frontend's own construction work. On desktop this is a no-op.
    zaparoo_rust_post_qt_start();
    startupTrace("cpp:post-qt-start hook complete");

    // Install after zaparoo_rust_init() so tracing is live before any Qt
    // messages are emitted.
    qInstallMessageHandler(qtMessageHandler);
    startupTrace("cpp:qt message handler installed");

    // Resolve language before font registration so startup only pays for
    // script fallback fonts the selected locale can actually use. The base
    // NotoSans.ttf covers Latin/Greek/Cyrillic UI locales; the large CJK
    // faces are only needed for Japanese/Korean/Chinese.
    const QString langCode = QString::fromUtf8(zaparoo_rust_language_code());
    const QLocale locale = langCode.isEmpty() ? QLocale::system() : QLocale(langCode);
    const QLocale::Language uiLanguage = locale.language();
    const bool crtNativePathEnabled = zaparoo_rust_crt_native_path_enabled();

    // Push the effective locale into Rust so `system_region::current_region()`
    // can resolve the `auto` region setting without calling back into Qt.
    // Called here — after the QLocale is fully resolved — before any QML
    // model Initialize callback runs. The Rust side stores it in an OnceLock
    // so later calls are silent no-ops.
    {
        const QByteArray localeName = locale.name().toUtf8();
        // NOLINTNEXTLINE(cppcoreguidelines-pro-type-reinterpret-cast)
        zaparoo_rust_set_effective_locale(reinterpret_cast<const uint8_t*>(localeName.constData()),
                                          static_cast<size_t>(localeName.size()));
    }
    startupTrace("cpp:effective locale pushed to Rust");

#ifdef ZAPAROO_EMBEDDED_BUILD
    if (qEnvironmentVariableIsEmpty("QT_QPA_FONTDIR"))
    {
        qputenv("QT_QPA_FONTDIR", QByteArrayLiteral("/tmp/zaparoo"));
    }
#endif

    QGuiApplication app(qtArgc, qtArgv);
    startupTrace("cpp:QGuiApplication constructed");
    QPixmapCache::setCacheLimit(kPixmapCacheLimitKiB);
    startupTrace("cpp:QPixmapCache limit set");

    // addApplicationFont returns -1 on failure (broken qrc path,
    // unreadable file). Logging the failure mode keeps a refactor that
    // breaks the resource alias from silently degrading to the default
    // font with no clue in the logs.
    const auto registerFont = [](const QString& path)
    {
        const int fontId = QFontDatabase::addApplicationFont(path);
        if (fontId == -1)
        {
            qWarning("Failed to register font: %s", qUtf8Printable(path));
            return;
        }
        qInfo("Registered font %s: %s", qUtf8Printable(path),
              qUtf8Printable(QFontDatabase::applicationFontFamilies(fontId).join(", ")));
    };
    struct FallbackFont
    {
        QChar::Script script;
        QString path;
        QString family;
    };
    const auto registerFallbackFont = [&registerFont](const FallbackFont& font)
    {
        registerFont(font.path);
        QFontDatabase::addApplicationFallbackFontFamily(font.script, font.family);
    };

    if (crtNativePathEnabled)
    {
        registerFont(
            QStringLiteral(":/qt/qml/Zaparoo/App/resources/fonts/MxPlus_HP_100LX_6x8.ttf"));
    }
    else
    {
        registerFont(QStringLiteral(":/qt/qml/Zaparoo/App/resources/fonts/NotoSans.ttf"));
    }

    bool registeredScriptFallback = false;
    if (uiLanguage == QLocale::Arabic)
    {
        registerFallbackFont({QChar::Script_Arabic,
                              QStringLiteral(":/qt/qml/Zaparoo/App/resources/fonts/"
                                             "NotoSansArabic.ttf"),
                              QStringLiteral("Noto Sans Arabic")});
        registeredScriptFallback = true;
    }
    if (!crtNativePathEnabled && uiLanguage == QLocale::Hebrew)
    {
        registerFallbackFont({QChar::Script_Hebrew,
                              QStringLiteral(":/qt/qml/Zaparoo/App/resources/fonts/"
                                             "NotoSansHebrew.ttf"),
                              QStringLiteral("Noto Sans Hebrew")});
        registeredScriptFallback = true;
    }
    if (uiLanguage == QLocale::Hindi)
    {
        registerFallbackFont({QChar::Script_Devanagari,
                              QStringLiteral(":/qt/qml/Zaparoo/App/resources/fonts/"
                                             "NotoSansDevanagari.ttf"),
                              QStringLiteral("Noto Sans Devanagari")});
        registeredScriptFallback = true;
    }
    if (uiLanguage == QLocale::Japanese)
    {
        registerFallbackFont({QChar::Script_Hiragana,
                              QStringLiteral(":/qt/qml/Zaparoo/App/resources/fonts/NotoSansJP.ttf"),
                              QStringLiteral("Noto Sans JP")});
        QFontDatabase::addApplicationFallbackFontFamily(QChar::Script_Katakana,
                                                        QStringLiteral("Noto Sans JP"));
        QFontDatabase::addApplicationFallbackFontFamily(QChar::Script_Han,
                                                        QStringLiteral("Noto Sans JP"));
        registeredScriptFallback = true;
    }
    if (uiLanguage == QLocale::Korean)
    {
        registerFallbackFont({QChar::Script_Hangul,
                              QStringLiteral(":/qt/qml/Zaparoo/App/resources/fonts/NotoSansKR.ttf"),
                              QStringLiteral("Noto Sans KR")});
        registeredScriptFallback = true;
    }
    if (uiLanguage == QLocale::Chinese)
    {
        registerFallbackFont({QChar::Script_Han,
                              QStringLiteral(":/qt/qml/Zaparoo/App/resources/fonts/NotoSansTC.ttf"),
                              QStringLiteral("Noto Sans TC")});
        registeredScriptFallback = true;
    }
    if (registeredScriptFallback)
    {
        qInfo("Registered locale-specific font fallbacks for %s", qUtf8Printable(locale.name()));
    }
    {
        QFont defaultFont = QGuiApplication::font();
        defaultFont.setFamily(crtNativePathEnabled ? QStringLiteral("MxPlus HP 100LX 6x8")
                                                   : QStringLiteral("Noto Sans"));
        QGuiApplication::setFont(defaultFont);
    }
    startupTrace("cpp:font registration complete");
    if (crtNativePathEnabled)
    {
        QQuickWindow::setTextRenderType(QQuickWindow::NativeTextRendering);
        qInfo("CRT native path: using native text rendering");
        // Desktop CRT preview: FreeType on X11/Wayland defaults to subpixel
        // RGB antialiasing ("ClearType"), which paints faint coloured
        // fringes either side of every glyph. MiSTer's linuxfb FreeType
        // does not enable subpixel AA, so the same scene reads pixel-
        // perfect there but blurry in the desktop preview. The bitmap
        // pixel font (MxPlus HP 100LX 6x8) is also designed to never be
        // smoothed. Set NoAntialias on the application default font so
        // every Text item that doesn't override styleStrategy inherits it.
        QFont defaultFont = QGuiApplication::font();
        defaultFont.setStyleStrategy(QFont::NoAntialias);
        defaultFont.setHintingPreference(QFont::PreferFullHinting);
        QGuiApplication::setFont(defaultFont);
    }
    QQuickStyle::setStyle("Basic");

    // Install the locale .qm translator before constructing the QML engine
    // so qsTr() lookups in Main.qml's initial bindings see translated text.
    // The Rust side resolves `[general] language` from frontend.toml into a
    // BCP-47 tag ("ja", "de_DE") or an empty string (follow system locale).
    // Stack lifetime is fine — `translator` outlives app.exec() and all QML.
    QTranslator translator;
    if (translator.load(locale, "frontend", "_", ":/i18n"))
    {
        QCoreApplication::installTranslator(&translator);
    }
    else
    {
        // Not an error on first run (English-only build ships a passthrough
        // frontend_en.qm). Log at info so the sink records the resolved
        // locale for bug reports without spamming at warn level.
        qInfo("No translation catalog for %s in :/i18n; using source strings",
              qUtf8Printable(locale.name()));
    }
    startupTrace("cpp:translator setup complete");

    QQmlApplicationEngine engine;
    // Engine takes ownership of the provider — it deletes it when the
    // engine is destroyed at process shutdown. The provider is the
    // bridge from `image://media-image/<encoded>` URLs to the
    // Rust-side in-memory media image cache, so it must be installed
    // before any QML type binds to a `coverKey` (every Tile inside
    // MainLayout does).
    // NOLINTNEXTLINE(cppcoreguidelines-owning-memory)
    engine.addImageProvider(QStringLiteral("media-image"), new MediaImageProvider());
    // NOLINTNEXTLINE(cppcoreguidelines-owning-memory)
    engine.addImageProvider(QStringLiteral("tinted-svg"), new TintedSvgImageProvider());
    // User-supplied customization images (system artwork and Hub icons).
    // Files under the `[custom] dir` root in `frontend.toml` are served as-is
    // -- no tint pipeline. The provider validates that decoded paths stay
    // inside the customization root to prevent arbitrary filesystem reads.
    // NOLINTNEXTLINE(cppcoreguidelines-owning-memory)
    engine.addImageProvider(QStringLiteral("custom-image"), new CustomImageProvider());
#ifdef ZAPAROO_UPDATE_RUNTIME_QML_IMPORT_PATH
    engine.addImportPath(QStringLiteral(ZAPAROO_UPDATE_RUNTIME_QML_IMPORT_PATH));
#endif
    startupTrace("cpp:QQmlApplicationEngine + image providers ready");

    QVariantMap initialProperties = {
        {"crtNativePath", crtNativePathEnabled},
        {"debugCrtSafeAreaOverlay", debugCrtSafeAreaOverlay},
    };
#ifdef ZAPAROO_EMBEDDED_BUILD
    // MainLayout's `fullScreen` defaults true so the binding pass during
    // QML construction evaluates width/height/visibility against the
    // embedded branch — no transient 1280x720 frame for the writer
    // thread to copy into the CRT scan-out region.
    initialProperties.insert(QStringLiteral("fullScreen"), true);
#else
    // Desktop preview: override the QML default so the dev workflow
    // gets a windowed 1280x720 design canvas. The brief
    // FullScreen→Windowed transition during construction isn't visible
    // — the desktop compositor buffers until the first paint.
    initialProperties.insert(QStringLiteral("fullScreen"), false);
    // Desktop CRT preview: when --crt is passed off-MiSTer, or
    // ZAPAROO_CRT_PREVIEW_RESOLUTION is set, render the QML scene at
    // the configured logical video size and integer-upscale via a
    // layered wrapper Item in MainLayout. Scale defaults to 0
    // (sentinel for "auto-pick the largest integer that fits the
    // primary screen with a 5% margin"); ZAPAROO_CRT_PREVIEW_RESOLUTION
    // also selects the logical CRT canvas on desktop, and
    // ZAPAROO_CRT_PREVIEW_SCALE overrides the integer window scale for
    // ad-hoc testing without rebuilding.
    if (crtNativePathEnabled)
    {
        int previewScale = 0;
        const QByteArray envScale = qgetenv("ZAPAROO_CRT_PREVIEW_SCALE");
        if (!envScale.isEmpty())
        {
            bool ok = false;
            const int parsed = envScale.toInt(&ok);
            if (ok && parsed > 0)
            {
                previewScale = parsed;
            }
        }
        initialProperties.insert(QStringLiteral("crtPreview"), true);
        initialProperties.insert(QStringLiteral("crtPreviewScale"), previewScale);
        initialProperties.insert(QStringLiteral("videoWidth"),
                                 static_cast<int>(zaparoo_rust_video_width()));
        initialProperties.insert(QStringLiteral("videoHeight"),
                                 static_cast<int>(zaparoo_rust_video_height()));
    }
#endif
    initialProperties.insert(QStringLiteral("videoWidth"),
                             static_cast<int>(zaparoo_rust_video_width()));
    initialProperties.insert(QStringLiteral("videoHeight"),
                             static_cast<int>(zaparoo_rust_video_height()));
    engine.setInitialProperties(initialProperties);
    startupTrace("cpp:QML initial properties set");

    // objectCreationFailed fires before loadFromModule returns when a QML
    // type fails to resolve or compile. Individual QML errors are already
    // routed through qtMessageHandler → tracing; this handler adds the
    // tying narrative ("the root object for Zaparoo.App.Main failed") so
    // a reader of frontend.log doesn't have to infer the connection.
    QObject::connect(
        &engine, &QQmlApplicationEngine::objectCreationFailed, &engine, [](const QUrl& url)
        { qCritical("QML object creation failed for %s", qUtf8Printable(url.toString())); });

    startupTrace("cpp:loading QML root module");
    engine.loadFromModule("Zaparoo.App", "Main");

    if (engine.rootObjects().isEmpty())
    {
        qCritical("QML engine produced no root objects; startup aborted (see earlier errors)");
        return EXIT_FAILURE;
    }
    startupTrace("cpp:QML root object created");

    auto* rootWindow = qobject_cast<QQuickWindow*>(engine.rootObjects().first());
    if (rootWindow != nullptr)
    {
        QObject::connect(rootWindow, &QQuickWindow::frameSwapped, rootWindow,
                         [logged = false]() mutable
                         {
                             if (logged)
                             {
                                 return;
                             }
                             logged = true;
                             startupTrace("cpp:first frame swapped");
                         });
    }

    if (crtNativePathEnabled)
    {
        qInfo("CRT startup decision: initialising native video writer");
        initNativeVideoWriter();
        startupTrace("cpp:native video writer initialized");
        // Drive the fb0 -> DDR copy from Qt's render-finish signal so
        // we mirror exactly one frame per actual scenegraph render
        // (idle scenes produce no `frameSwapped` and therefore no
        // copy and no CPU work).
        //
        // Qt::QueuedConnection is load-bearing: the linuxfb QPA
        // doesn't write /dev/fb0 inside `QPlatformBackingStore::flush()`.
        // It calls `QFbScreen::scheduleUpdate()` which only posts a
        // `QEvent::UpdateRequest`; the actual blit to fb0 happens
        // later, in `QFbScreen::doRedraw()`, on a subsequent event-
        // loop iteration. `frameSwapped` is emitted *before* that
        // posted event drains, so a DirectConnection slot would read
        // stale fb0 and we'd publish the previous frame to the FPGA
        // (one-frame-behind CRT output). Posting our copy via a
        // queued connection puts it FIFO behind the UpdateRequest,
        // so `doRedraw()` runs first and we then read the freshly
        // updated fb0.
        if (rootWindow != nullptr)
        {
            QObject::connect(
                rootWindow, &QQuickWindow::frameSwapped, rootWindow,
                []() { copyFrameNativeVideoWriter(); }, Qt::QueuedConnection);
        }
        else
        {
            qCritical("CRT startup decision: QML root is not a QQuickWindow; per-frame copy "
                      "hook not installed (FPGA will stay on the zero-initialised slot)");
        }
    }

    // Drain the tokio runtime and detach the Qt-to-Rust log bridge while
    // the main thread is still alive and every thread's TLS storage is
    // still addressable. `aboutToQuit` fires after `Qt.quit()` (or the
    // last window close) but before `exec()` returns, which is the only
    // window where we can run `Runtime::shutdown_timeout` and uninstall
    // the message handler ahead of `__cxa_finalize`.
    //
    // The handler matters because Qt's own destruction emits log
    // messages from internal threads (QThreadPool workers, plugin
    // teardown). Left installed, those calls re-enter `zaparoo_log_qt`
    // and the tracing dispatcher whose TLS is mid-destruction, panic
    // with `AccessError`, and the panic-hook's own `tracing::error!`
    // re-panics into SIGABRT (exit 134).
    QObject::connect(&app, &QGuiApplication::aboutToQuit, &app,
                     []()
                     {
                         zaparoo_rust_shutdown();
                         stopNativeVideoWriter();
                         qInstallMessageHandler(nullptr);
                     });

    const int exitCode = QGuiApplication::exec();
    if (exitCode != kRestartExitCode)
    {
        // Any other code propagates to the parent. On MiSTer, exit 42 is
        // the Main_MiSTer fork's "re-read zaparoo_launcher_crt.bin and
        // respawn me" protocol; it must reach the parent untouched.
        return exitCode;
    }

    // Restart as a fresh process so the Rust globals and cached config are
    // rebuilt from scratch. Re-entering main() in-process panics on the
    // OnceLock-backed runtime/store singletons. Use the unfiltered argv so
    // `--crt` survives the restart.
    const char* programPath = parsedArgs.originalArgv.front();
    ::execvp(programPath, parsedArgs.originalArgv.data());
    std::fprintf(stderr, "Failed to restart frontend via execvp(%s): %s\n", programPath,
                 std::strerror(errno));
    return EXIT_FAILURE;
}
