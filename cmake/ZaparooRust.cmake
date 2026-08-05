# Zaparoo Frontend
# Copyright (c) 2026 Wizzo Pty Ltd and the Zaparoo Project contributors.
# SPDX-License-Identifier: LicenseRef-PolyForm-Noncommercial-1.0.0
#
# Rust/Cargo integration via Corrosion. Builds zaparoo_frontend_rs as a
# staticlib and links it into a thin C++ executable (frontend) so that Qt's
# CMake machinery (qt_import_qml_plugins) handles all static-plugin and
# qmldir-resource-init wiring correctly — the documented CXX-Qt static-Qt
# topology (topology B: C++ exe + Rust staticlib).

include_guard(GLOBAL)

include(FetchContent)

# Tell Corrosion the Rust target triple explicitly for cross-builds. CMake's processor names are not
# enough for Rust target selection: MiSTer is ARMv7 hard-float, while ARM64 builds use aarch64
# Linux.
if(CMAKE_CROSSCOMPILING AND NOT Rust_CARGO_TARGET)
    if(CMAKE_SYSTEM_PROCESSOR STREQUAL "arm")
        set(Rust_CARGO_TARGET
            "armv7-unknown-linux-gnueabihf"
            CACHE STRING "Cargo target triple for ARM32 cross-build" FORCE
        )
    elseif(CMAKE_SYSTEM_PROCESSOR STREQUAL "aarch64")
        set(Rust_CARGO_TARGET
            "aarch64-unknown-linux-gnu"
            CACHE STRING "Cargo target triple for arm64 cross-build" FORCE
        )
    endif()
endif()

FetchContent_Declare(
    Corrosion
    GIT_REPOSITORY https://github.com/corrosion-rs/corrosion.git
    GIT_TAG v0.6.1
)
FetchContent_MakeAvailable(Corrosion)

# Import the Rust workspace staticlib. Corrosion creates a CMake IMPORTED STATIC LIBRARY target
# named "zaparoo_frontend_rs" (the [lib] name).
corrosion_import_crate(
    MANIFEST_PATH "${CMAKE_SOURCE_DIR}/rust/Cargo.toml" CRATES zaparoo-frontend-rs
)

corrosion_set_features(zaparoo_frontend_rs NO_DEFAULT_FEATURES)
if(ZAPAROO_WITH_UPDATE)
    corrosion_set_features(zaparoo_frontend_rs FEATURES update)
endif()

# ── Environment variables for cxx_qt_build's build.rs ─────────────────────── QMAKE: cxx_qt_build
# (via qt-build-utils) uses qmake to locate Qt headers and libraries. For static cross-builds the
# system qmake points to x86_64 Qt; override with the target qmake.
get_target_property(_rs_qt6_core_type Qt6::Core TYPE)
if(_rs_qt6_core_type STREQUAL "STATIC_LIBRARY")
    if(CMAKE_SYSTEM_PROCESSOR STREQUAL "aarch64")
        set(_rs_qmake "/opt/qt6-arm64/bin/qmake6")
    else()
        set(_rs_qmake "/opt/qt6-arm32/bin/qmake6")
    endif()
    if(NOT EXISTS "${_rs_qmake}")
        message(
            FATAL_ERROR
                "Target qmake not found at ${_rs_qmake}. "
                "Expected /opt/qt6-arm64/bin/qmake6 or /opt/qt6-arm32/bin/qmake6 "
                "in static Qt cross-build images."
        )
    endif()
else()
    find_program(_rs_qmake NAMES qmake6 qmake REQUIRED)
endif()

corrosion_set_env_vars(zaparoo_frontend_rs "QMAKE=${_rs_qmake}")
if(_rs_qt6_core_type STREQUAL "STATIC_LIBRARY" AND CMAKE_SYSTEM_PROCESSOR STREQUAL "arm")
    corrosion_set_env_vars(zaparoo_frontend_rs "ZAPAROO_RUNTIME=mister")
endif()
if(ZAPAROO_DEV)
    corrosion_set_env_vars(zaparoo_frontend_rs "ZAPAROO_DEV_BUILD=1")
endif()

# ── C++ executable ─────────────────────────────────────────────────────────── Using
# qt_add_executable (not add_executable) so that Qt's CMake sets up the target with all the
# properties needed by qt_import_qml_plugins.
qt_add_executable(
    frontend
    "${CMAKE_SOURCE_DIR}/src/app/main.cpp"
    "${CMAKE_SOURCE_DIR}/src/app/media_image_provider.h"
    "${CMAKE_SOURCE_DIR}/src/app/media_image_provider.cpp"
    "${CMAKE_SOURCE_DIR}/src/app/tinted_svg_image_provider.h"
    "${CMAKE_SOURCE_DIR}/src/app/tinted_svg_image_provider.cpp"
    "${CMAKE_SOURCE_DIR}/src/app/custom_image_provider.h"
    "${CMAKE_SOURCE_DIR}/src/app/custom_image_provider.cpp"
    "${CMAKE_SOURCE_DIR}/src/app/native_video_writer.h"
    "${CMAKE_SOURCE_DIR}/src/app/native_video_writer.cpp"
)
target_include_directories(
    frontend
    PRIVATE "${CMAKE_SOURCE_DIR}/src/app"
)

target_compile_definitions(frontend PRIVATE ZAPAROO_VERSION="${CMAKE_PROJECT_VERSION}")

# For static Qt (ARM32): define QT_STATIC so main.cpp's #ifdef fires. Qt itself defines this in its
# headers, but the compiler may not see it before the first #include unless we make it explicit here
# too.
if(_rs_qt6_core_type STREQUAL "STATIC_LIBRARY")
    target_compile_definitions(frontend PRIVATE QT_STATIC)
endif()

if(ZAPAROO_DEV)
    target_compile_definitions(frontend PRIVATE ZAPAROO_DEV_BUILD)
endif()

# Load Qt QML plugin CMake configs so that qt_import_qml_plugins can find and link the correct
# static plugin archives. These are not loaded by find_package(Qt6 ...) by default.
if(_rs_qt6_core_type STREQUAL "STATIC_LIBRARY")
    get_filename_component(_rs_qt_prefix "${Qt6_DIR}/../../.." ABSOLUTE)
    file(GLOB _rs_qml_plugin_configs
         "${_rs_qt_prefix}/lib/cmake/Qt6Qml/QmlPlugins/Qt6*Config.cmake"
    )
    foreach(_rs_config IN LISTS _rs_qml_plugin_configs)
        include("${_rs_config}" OPTIONAL)
    endforeach()
    include("${_rs_qt_prefix}/lib/cmake/Qt6Gui/Qt6QLinuxFbIntegrationPluginConfig.cmake" OPTIONAL)
    include("${_rs_qt_prefix}/lib/cmake/Qt6Gui/Qt6QEglFSIntegrationPluginConfig.cmake" OPTIONAL)
    include("${_rs_qt_prefix}/lib/cmake/Qt6Gui/Qt6QEglFSKmsGbmIntegrationPluginConfig.cmake"
            OPTIONAL
    )
    include("${_rs_qt_prefix}/lib/cmake/Qt6Gui/Qt6QWebpPluginConfig.cmake" OPTIONAL)
    foreach(_rs_qml_plugin IN ITEMS qtquickcontrols2plugin qtquickcontrols2basicstyleplugin
                                    qtquickcontrols2implplugin qtquicktemplates2plugin quickwindow
    )
        include("${_rs_qt_prefix}/lib/cmake/Qt6Qml/QmlPlugins/Qt6${_rs_qml_plugin}Config.cmake"
                OPTIONAL
        )
    endforeach()
endif()

target_link_libraries(
    frontend
    PRIVATE zaparoo_frontend_rs zaparoo_ui_appplugin Qt6::Quick Qt6::QuickControls2 Qt6::Svg
)
if(ZAPAROO_WITH_UPDATE)
    target_link_libraries(frontend PRIVATE zaparoo_update_qmlplugin)
    zaparoo_update_runtime_qml_import_path(_rs_update_runtime_qml_import_path)
    if(_rs_update_runtime_qml_import_path)
        target_compile_definitions(
            frontend
            PRIVATE ZAPAROO_UPDATE_RUNTIME_QML_IMPORT_PATH=\"${_rs_update_runtime_qml_import_path}\"
        )
    endif()
endif()

# Dummy CMake target satisfying qmlimportscanner's lookup for the cxx-qt plugin.
# build/qml/Zaparoo/Browse/qmldir declares `optional plugin Zaparoo_Browse`, and
# qt_import_qml_plugins() warns when that name has no matching CMake target. The real plugin code is
# baked into zaparoo_frontend_rs (already linked above); the INTERFACE target here is a no-op
# link-wise but silences the "plugin will not be linked" warning. Defined at global scope so
# tests/ui sees it too.
if(NOT TARGET Zaparoo_Browse)
    add_library(Zaparoo_Browse INTERFACE)
endif()

# Critical: documented Qt static-plugin machinery. Runs qmlimportscanner, traverses the QML module
# dependency graph, and emits correct Q_IMPORT_QML_PLUGIN calls + --whole-archive link lines for
# every Qt static QML plugin and qmldir resource init .o.
qt_import_qml_plugins(frontend)

# For static Qt (ARM32): the Controls chain _init OBJECT targets carry the Q_IMPORT_QML_PLUGIN
# static-init factories. Not propagated automatically from a cross-compiled Qt toolchain, so link
# them explicitly. The image-format plugin entries register SVG/WebP with QImageReader so `Image`
# sources and Core-returned WebP covers work in the static build (the shared desktop build picks
# plugins up automatically from the Qt install).
if(_rs_qt6_core_type STREQUAL "STATIC_LIBRARY")
    if(TARGET Qt6::QLinuxFbIntegrationPlugin)
        target_link_libraries(
            frontend PRIVATE Qt6::QLinuxFbIntegrationPlugin Qt6::QLinuxFbIntegrationPlugin_init
        )
    endif()
    if(TARGET Qt6::QEglFSIntegrationPlugin)
        target_link_libraries(
            frontend PRIVATE Qt6::QEglFSIntegrationPlugin Qt6::QEglFSIntegrationPlugin_init
        )
        target_compile_definitions(frontend PRIVATE ZAPAROO_STATIC_EGLFS_PLUGIN)
    endif()
    if(TARGET Qt6::QEglFSKmsGbmIntegrationPlugin)
        target_link_libraries(
            frontend PRIVATE Qt6::QEglFSKmsGbmIntegrationPlugin
                             Qt6::QEglFSKmsGbmIntegrationPlugin_init
        )
        target_compile_definitions(frontend PRIVATE ZAPAROO_STATIC_EGLFS_KMS_GBM_PLUGIN)
    endif()
    if(TARGET Qt6::QSvgPlugin)
        target_link_libraries(frontend PRIVATE Qt6::QSvgPlugin Qt6::QSvgPlugin_init)
    endif()
    if(TARGET Qt6::QWebpPlugin)
        target_link_libraries(frontend PRIVATE Qt6::QWebpPlugin Qt6::QWebpPlugin_init)
    endif()
    foreach(_rs_qml_plugin IN ITEMS qtquickcontrols2plugin qtquickcontrols2basicstyleplugin
                                    qtquickcontrols2implplugin qtquicktemplates2plugin quickwindow
    )
        if(TARGET Qt6::${_rs_qml_plugin})
            target_link_libraries(
                frontend PRIVATE Qt6::${_rs_qml_plugin} Qt6::${_rs_qml_plugin}_init
            )
        endif()
    endforeach()
endif()

set_target_properties(frontend PROPERTIES RUNTIME_OUTPUT_DIRECTORY "${CMAKE_BINARY_DIR}/bin")

# ── Translations ───────────────────────────────────────────────────────────── lrelease compiles
# .ts → .qm and qt_add_translations embeds them into the frontend binary under qrc:/i18n/ (the
# default RESOURCE_PREFIX). main.cpp loads the locale-matching .qm with `QTranslator::load(locale,
# "frontend", "_", ":/i18n")` before the QML engine runs.
#
# IMMEDIATE_CALL runs source collection inline instead of deferring to the end of the top-level
# directory scope. Without it, qt_add_translations defers until CMake finalises PROJECT_SOURCE_DIR
# and the generated resource targets land after link-time, producing missing-dependency errors on
# parallel builds (Corrosion-provided staticlibs are visited out of order).
qt_add_translations(
    frontend
    TS_FILES
    "${CMAKE_SOURCE_DIR}/src/ui/translations/frontend_en.ts"
    "${CMAKE_SOURCE_DIR}/src/ui/translations/frontend_it.ts"
    "${CMAKE_SOURCE_DIR}/src/ui/translations/frontend_es.ts"
    "${CMAKE_SOURCE_DIR}/src/ui/translations/frontend_eu.ts"
    "${CMAKE_SOURCE_DIR}/src/ui/translations/frontend_de.ts"
    "${CMAKE_SOURCE_DIR}/src/ui/translations/frontend_el.ts"
    "${CMAKE_SOURCE_DIR}/src/ui/translations/frontend_ja.ts"
    "${CMAKE_SOURCE_DIR}/src/ui/translations/frontend_ko.ts"
    "${CMAKE_SOURCE_DIR}/src/ui/translations/frontend_nl.ts"
    "${CMAKE_SOURCE_DIR}/src/ui/translations/frontend_ro.ts"
    "${CMAKE_SOURCE_DIR}/src/ui/translations/frontend_sk.ts"
    "${CMAKE_SOURCE_DIR}/src/ui/translations/frontend_uk.ts"
    "${CMAKE_SOURCE_DIR}/src/ui/translations/frontend_zh_CN.ts"
    "${CMAKE_SOURCE_DIR}/src/ui/translations/frontend_he.ts"
    "${CMAKE_SOURCE_DIR}/src/ui/translations/frontend_ar.ts"
    "${CMAKE_SOURCE_DIR}/src/ui/translations/frontend_hi.ts"
    RESOURCE_PREFIX
    "/i18n"
    IMMEDIATE_CALL
)

# ── cxx-qt QML module sync for tooling ─────────────────────────────────────── cxx-qt writes qmldir
# + plugin.qmltypes under cargo's OUT_DIR, which qmllint does not search. Copy them into
# ${QT_QML_OUTPUT_DIRECTORY}/<module>/ so qmllint's -I path resolves types (Browse.QAppState,
# Browse.GamesModel, …) exposed by the Rust staticlib. The cmake script globs at build time because
# Corrosion's hash-segmented path is not known at configure time.
add_custom_target(
    zaparoo_cxxqt_qml_sync
    COMMAND
        ${CMAKE_COMMAND} -DCARGO_DIR=${CMAKE_BINARY_DIR}/cargo
        -DDEST_QML_DIR=${CMAKE_BINARY_DIR}/qml -P
        ${CMAKE_SOURCE_DIR}/cmake/SyncCxxqtQmlModules.cmake
    DEPENDS cargo-build_zaparoo_frontend_rs
    COMMENT "Syncing cxx-qt QML module manifests into ${CMAKE_BINARY_DIR}/qml"
    VERBATIM
)
