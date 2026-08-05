// Zaparoo Frontend
// Copyright (c) 2026 Wizzo Pty Ltd and the Zaparoo Project contributors.
// SPDX-License-Identifier: LicenseRef-PolyForm-Noncommercial-1.0.0
//
// `Browse.FavoritesState` — persisted state owned by the favorites screen.
// Holds the path of the entry that last had focus, so a kill-resume
// puts the highlight back on the same row. The favorites list itself
// lives in `FavoritesModel` (Core's `media.search` filtered by
// `user:favorite`); this singleton just remembers the path.

use crate::models::{with_persist_mut, with_persist_read};
use cxx_qt::{CxxQtType, Initialize};
use cxx_qt_lib::QString;
use std::pin::Pin;
use zaparoo_core::persist::{self, FavoritesState};

#[derive(Default)]
pub struct FavoritesStateRust {
    selected_path: QString,
    sort: QString,
    filter: QString,
}

#[cxx_qt::bridge]
pub mod ffi {
    unsafe extern "C++" {
        include!("model_includes.h");

        type QString = cxx_qt_lib::QString;
    }

    unsafe extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        #[qproperty(QString, selected_path, READ, WRITE = set_selected_path, NOTIFY)]
        #[qproperty(QString, sort, READ, WRITE = set_sort, NOTIFY)]
        #[qproperty(QString, filter, READ, WRITE = set_filter, NOTIFY)]
        type FavoritesState = super::FavoritesStateRust;

        #[qinvokable]
        fn set_selected_path(self: Pin<&mut FavoritesState>, value: QString);

        #[qinvokable]
        fn set_sort(self: Pin<&mut FavoritesState>, value: QString);

        #[qinvokable]
        fn set_filter(self: Pin<&mut FavoritesState>, value: QString);
    }

    impl cxx_qt::Initialize for FavoritesState {}
}

impl Initialize for ffi::FavoritesState {
    fn initialize(mut self: Pin<&mut Self>) {
        let snapshot: FavoritesState = with_persist_read(|s| s.favorites.clone());
        self.as_mut().rust_mut().selected_path = QString::from(snapshot.selected_path.as_str());
        self.as_mut().rust_mut().sort = QString::from(snapshot.sort.as_str());
        self.as_mut().rust_mut().filter = QString::from(snapshot.filter.as_str());
    }
}

impl ffi::FavoritesState {
    fn set_selected_path(mut self: Pin<&mut Self>, value: QString) {
        if self.selected_path == value {
            return;
        }
        let value_str = value.to_string();
        self.as_mut().rust_mut().selected_path = value;
        self.as_mut().selected_path_changed();
        persist_favorites(|r| r.selected_path = value_str);
    }

    fn set_sort(mut self: Pin<&mut Self>, value: QString) {
        if self.sort == value {
            return;
        }
        let value_str = value.to_string();
        self.as_mut().rust_mut().sort = value;
        self.as_mut().sort_changed();
        persist_favorites(|r| r.sort = value_str);
    }

    fn set_filter(mut self: Pin<&mut Self>, value: QString) {
        if self.filter == value {
            return;
        }
        let value_str = value.to_string();
        self.as_mut().rust_mut().filter = value;
        self.as_mut().filter_changed();
        persist_favorites(|r| r.filter = value_str);
    }
}

fn persist_favorites<F: FnOnce(&mut FavoritesState)>(mutator: F) {
    let snapshot = with_persist_mut(|s| {
        mutator(&mut s.favorites);
        s.clone()
    });
    persist::save(&snapshot);
}
