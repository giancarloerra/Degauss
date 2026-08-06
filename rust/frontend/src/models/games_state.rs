// Zaparoo Frontend
// Copyright (c) 2026 Wizzo Pty Ltd and the Zaparoo Project contributors.
// SPDX-License-Identifier: LicenseRef-PolyForm-Noncommercial-1.0.0
//
// `Browse.GamesState` — persisted state owned by the games screen. Tracks
// the system currently scoped to the games grid plus a path stack the
// user has descended into (folder navigation). For each level we also
// record the highlighted entry so the grid can restore selection on
// relaunch. Schema version is checked independently from other screens
// on load (see `zaparoo_core::persist`).
//
// Stack invariant: `path_stack` and `selected_at_level` always have the
// same length, and length is always at least 1. Index 0 represents the
// initial games-screen view for the current system: an empty path string
// means "let the model decide" (browse with `systems` filter, then
// auto-navigate into a single root if there is one). Higher indices are
// real folder paths from `_navigateIntoFolder`.

use crate::models::{with_persist_mut, with_persist_read};
use cxx_qt::{CxxQtType, Initialize};
use cxx_qt_lib::{QString, QStringList};
use std::pin::Pin;
use zaparoo_core::persist::{self, GamesState};

#[derive(Default)]
pub struct GamesStateRust {
    system_id: QString,
    path_stack: QStringList,
    selected_at_level: QStringList,
    favorites_filter: bool,
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
        #[qproperty(QString, system_id, READ, WRITE = set_system_id, NOTIFY)]
        #[qproperty(QStringList, path_stack, READ, NOTIFY)]
        #[qproperty(QStringList, selected_at_level, READ, NOTIFY)]
        #[qproperty(bool, favorites_filter, READ, WRITE = set_favorites_filter, NOTIFY)]
        type GamesState = super::GamesStateRust;

        /// Persisted favorites-only projection for folder listings,
        /// restored by the games screen before its first browse.
        #[qinvokable]
        fn set_favorites_filter(self: Pin<&mut GamesState>, value: bool);

        #[qinvokable]
        fn set_system_id(self: Pin<&mut GamesState>, value: QString);

        /// Push a new level on the stack. `path` is the directory path the
        /// user has navigated into; `selected` is the highlighted entry
        /// inside that directory (empty if not yet known).
        #[qinvokable]
        fn push_level(self: Pin<&mut GamesState>, path: &QString, selected: &QString);

        /// Pop the deepest level off the stack. No-op if the stack is at
        /// its minimum size of 1 (the root level).
        #[qinvokable]
        fn pop_level(self: Pin<&mut GamesState>);

        /// Update the highlighted entry for the current (deepest) level.
        #[qinvokable]
        fn set_selected_at_top(self: Pin<&mut GamesState>, selected: &QString);
    }

    impl cxx_qt::Initialize for GamesState {}
}

impl Initialize for ffi::GamesState {
    fn initialize(mut self: Pin<&mut Self>) {
        let started = std::time::Instant::now();
        crate::startup_trace("rust:model GamesState init start");
        let snapshot: GamesState = with_persist_read(|s| s.games.clone());
        self.as_mut().rust_mut().system_id = QString::from(snapshot.system_id.as_str());
        let (path_stack, selected_at_level) =
            normalize_persisted(&snapshot.path_stack, &snapshot.selected_at_level);
        self.as_mut().rust_mut().path_stack = vec_to_qstringlist(&path_stack);
        self.as_mut().rust_mut().selected_at_level = vec_to_qstringlist(&selected_at_level);
        self.as_mut().rust_mut().favorites_filter = snapshot.favorites_filter;
        crate::startup_trace(format!(
            "rust:model GamesState init end dur_ms={}",
            started.elapsed().as_millis()
        ));
    }
}

impl ffi::GamesState {
    fn set_favorites_filter(mut self: Pin<&mut Self>, value: bool) {
        if self.favorites_filter == value {
            return;
        }
        self.as_mut().rust_mut().favorites_filter = value;
        self.as_mut().favorites_filter_changed();
        persist_games(|g| {
            g.favorites_filter = value;
        });
    }

    fn set_system_id(mut self: Pin<&mut Self>, value: QString) {
        if self.system_id == value {
            return;
        }
        let value_str = value.to_string();
        self.as_mut().rust_mut().system_id = value;
        self.as_mut().system_id_changed();
        let reset_stack = vec![String::new()];
        let reset_selected = vec![String::new()];
        self.as_mut().rust_mut().path_stack = vec_to_qstringlist(&reset_stack);
        self.as_mut().path_stack_changed();
        self.as_mut().rust_mut().selected_at_level = vec_to_qstringlist(&reset_selected);
        self.as_mut().selected_at_level_changed();
        persist_games(|g| {
            g.system_id = value_str;
            g.path_stack = reset_stack;
            g.selected_at_level = reset_selected;
        });
    }

    fn push_level(mut self: Pin<&mut Self>, path: &QString, selected: &QString) {
        let path_str = path.to_string();
        let selected_str = selected.to_string();
        let mut stack: Vec<String> = qstringlist_to_vec(&self.path_stack);
        let mut sel: Vec<String> = qstringlist_to_vec(&self.selected_at_level);
        stack.push(path_str);
        sel.push(selected_str);
        self.as_mut().rust_mut().path_stack = vec_to_qstringlist(&stack);
        self.as_mut().path_stack_changed();
        self.as_mut().rust_mut().selected_at_level = vec_to_qstringlist(&sel);
        self.as_mut().selected_at_level_changed();
        persist_games(|g| {
            g.path_stack = stack;
            g.selected_at_level = sel;
        });
    }

    fn pop_level(mut self: Pin<&mut Self>) {
        let mut stack: Vec<String> = qstringlist_to_vec(&self.path_stack);
        let mut sel: Vec<String> = qstringlist_to_vec(&self.selected_at_level);
        if stack.len() <= 1 {
            return;
        }
        stack.pop();
        sel.pop();
        self.as_mut().rust_mut().path_stack = vec_to_qstringlist(&stack);
        self.as_mut().path_stack_changed();
        self.as_mut().rust_mut().selected_at_level = vec_to_qstringlist(&sel);
        self.as_mut().selected_at_level_changed();
        persist_games(|g| {
            g.path_stack = stack;
            g.selected_at_level = sel;
        });
    }

    fn set_selected_at_top(mut self: Pin<&mut Self>, selected: &QString) {
        let selected_str = selected.to_string();
        let mut sel: Vec<String> = qstringlist_to_vec(&self.selected_at_level);
        if let Some(last) = sel.last_mut() {
            if *last == selected_str {
                return;
            }
            last.clone_from(&selected_str);
        } else {
            sel.push(selected_str);
        }
        self.as_mut().rust_mut().selected_at_level = vec_to_qstringlist(&sel);
        self.as_mut().selected_at_level_changed();
        persist_games(|g| g.selected_at_level = sel);
    }
}

fn persist_games<F: FnOnce(&mut GamesState)>(mutator: F) {
    let snapshot = with_persist_mut(|s| {
        mutator(&mut s.games);
        s.clone()
    });
    persist::save(&snapshot);
}

fn vec_to_qstringlist(v: &[String]) -> QStringList {
    let mut list = QStringList::default();
    for s in v {
        list.append(QString::from(s.as_str()));
    }
    list
}

fn qstringlist_to_vec(list: &QStringList) -> Vec<String> {
    list.iter().map(String::from).collect()
}

/// Repair a persisted stack that's shorter than length 1 or whose two
/// vecs disagree on length. Disk corruption or a hand-edited state file
/// shouldn't crash; reset to a one-level empty stack instead.
fn normalize_persisted(
    path_stack: &[String],
    selected_at_level: &[String],
) -> (Vec<String>, Vec<String>) {
    if path_stack.is_empty() || path_stack.len() != selected_at_level.len() {
        return (vec![String::new()], vec![String::new()]);
    }
    (path_stack.to_vec(), selected_at_level.to_vec())
}

#[cfg(test)]
mod tests {
    use super::normalize_persisted;

    #[test]
    fn normalize_passes_through_consistent_state() {
        let stack = vec![String::new(), "/roms/snes/rpgs".into()];
        let sel = vec!["/roms/snes/rpgs".into(), "/roms/snes/rpgs/ff6.smc".into()];
        let (s, l) = normalize_persisted(&stack, &sel);
        assert_eq!(s, stack);
        assert_eq!(l, sel);
    }

    #[test]
    fn normalize_resets_empty_stack_to_root_level() {
        let (s, l) = normalize_persisted(&[], &[]);
        assert_eq!(s, vec![String::new()]);
        assert_eq!(l, vec![String::new()]);
    }

    #[test]
    fn normalize_resets_when_lengths_disagree() {
        let stack = vec![String::new(), "/a".into()];
        let sel = vec![String::new()];
        let (s, l) = normalize_persisted(&stack, &sel);
        assert_eq!(s, vec![String::new()]);
        assert_eq!(l, vec![String::new()]);
    }
}
