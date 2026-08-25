//! Selection and windowing, kept separate from the UI.
//!
//! All of the arithmetic that decides what is on screen lives here so it can
//! be tested without a framebuffer, a Slint platform, or a device.

/// Selection over a list that is shown `visible` items at a time, moving in
/// steps of `stride` items per visual row (1 for a list, the column count
/// for a grid).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListState {
    count: usize,
    visible: usize,
    stride: usize,
    selected: usize,
}

impl ListState {
    pub fn new(count: usize, visible: usize) -> Self {
        ListState {
            count,
            visible: visible.max(1),
            stride: 1,
            selected: 0,
        }
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Put the selection on a given item, clamped to the list.
    pub fn select(&mut self, index: usize) {
        self.selected = index.min(self.count.saturating_sub(1));
    }

    #[allow(dead_code)]
    pub fn visible(&self) -> usize {
        self.visible
    }

    pub fn count(&self) -> usize {
        self.count
    }

    pub fn stride(&self) -> usize {
        self.stride
    }

    /// Reshape for a different layout: how many items fit, and how many sit
    /// on one visual row.
    pub fn reshape(&mut self, visible: usize, stride: usize) {
        self.visible = visible.max(1);
        self.stride = stride.max(1);
    }

    /// First item on screen. The selection is kept near the middle so a held
    /// direction produces continuous movement rather than the cursor walking
    /// to the edge and the list then jumping.
    ///
    /// In a grid the top is snapped to a row boundary, or the tiles would
    /// shuffle sideways as it scrolls.
    pub fn top(&self) -> usize {
        let last_top = self.count.saturating_sub(self.visible);
        let centred = self.selected.saturating_sub(self.visible / 2);
        let top = centred.min(last_top);
        if self.stride > 1 {
            let aligned = top - (top % self.stride);
            // Snapping down must not leave the selection below the window.
            // On a last row that is not a whole row, aligning down drops the
            // cursor's own tile off the bottom, so take the next boundary up.
            if self.selected >= aligned + self.visible {
                aligned + self.stride
            } else {
                aligned
            }
        } else {
            top
        }
    }

    /// The items to hand the UI, and where the selection sits within them.
    pub fn window(&self) -> (std::ops::Range<usize>, usize) {
        let top = self.top();
        let end = (top + self.visible).min(self.count);
        (top..end, self.selected.saturating_sub(top))
    }

    /// Move by whole visual rows. Returns true when something changed.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn move_rows(&mut self, rows: isize) -> bool {
        self.move_items(rows * self.stride as isize)
    }

    /// Move, and wrap around the ends.
    ///
    /// Wrapping happens only from the edge: a move that starts anywhere else
    /// stops at the end of the list. Fast scrolling would otherwise shoot
    /// past the last row and land in the middle of the first screen, which
    /// reads as the list having jumped somewhere arbitrary. Standing at the
    /// end and pressing again is unambiguous, and is what the stock menu
    /// does.
    pub fn move_items(&mut self, delta: isize) -> bool {
        if self.count == 0 || delta == 0 {
            return false;
        }
        let last = (self.count - 1) as isize;
        let current = self.selected as isize;
        let next = if delta > 0 && current == last {
            0
        } else if delta < 0 && current == 0 {
            last as usize
        } else {
            current.saturating_add(delta).clamp(0, last) as usize
        };
        let changed = next != self.selected;
        self.selected = next;
        changed
    }

    pub fn go_first(&mut self) -> bool {
        let changed = self.selected != 0;
        self.selected = 0;
        changed
    }

    pub fn go_last(&mut self) -> bool {
        let last = self.count.saturating_sub(1);
        let changed = self.selected != last;
        self.selected = last;
        changed
    }
}

#[cfg(test)]
mod tests {
    /// Snapping the top down to a row boundary must never push the
    /// selection out of the window. On the last, partial row of a grid it
    /// did: the tile the cursor was on stopped being drawn at all.
    #[test]
    fn the_last_partial_row_of_a_grid_still_shows_the_selection() {
        let mut state = ListState::new(100, 6);
        state.reshape(6, 3);
        state.go_last();
        let (range, index) = state.window();
        assert!(
            range.start + index == state.selected(),
            "selection {} not inside window {range:?}",
            state.selected()
        );
        assert!(index < state.visible(), "index {index} outside the window");
    }

    use super::*;

    #[test]
    fn the_ends_of_the_list_join_up() {
        let mut state = ListState::new(10, 4);
        assert!(state.move_rows(-1), "up from the top wraps");
        assert_eq!(state.selected(), 9, "to the last item");

        assert!(state.move_rows(1), "down from the end wraps");
        assert_eq!(state.selected(), 0, "back to the first");
    }

    #[test]
    fn a_fast_move_stops_at_the_end_before_it_wraps() {
        // Held-direction scrolling covers many rows per step. Wrapping in
        // the middle of one of those would land the selection an arbitrary
        // distance into the other end of the list, which reads as the list
        // having jumped somewhere by itself.
        let mut state = ListState::new(100, 8);
        state.move_items(95);
        assert_eq!(state.selected(), 95);
        state.move_items(20);
        assert_eq!(state.selected(), 99, "stops at the end");
        state.move_items(20);
        assert_eq!(state.selected(), 0, "only then does it wrap");
    }

    #[test]
    fn a_grid_can_be_walked_one_tile_at_a_time_in_reading_order() {
        // Three across. Stepping by one goes along the row and then down to
        // the start of the next, the way a page is read, so every cover is
        // reachable with up and down alone.
        let mut state = ListState::new(9, 6);
        state.reshape(6, 3);
        for expected in 1..9 {
            assert!(state.move_items(1));
            assert_eq!(state.selected(), expected);
        }
        // Back up crosses the row boundary the other way.
        state.select(3);
        assert!(state.move_items(-1));
        assert_eq!(
            state.selected(),
            2,
            "up from the first of a row is the last of the one above"
        );
    }

    #[test]
    fn a_list_of_one_has_nowhere_to_wrap_to() {
        let mut state = ListState::new(1, 4);
        assert!(!state.move_rows(1));
        assert!(!state.move_rows(-1));
        assert_eq!(state.selected(), 0);
    }

    #[test]
    fn an_empty_library_is_not_a_crash() {
        let mut state = ListState::new(0, 8);
        assert!(!state.move_rows(1));
        let (range, selected) = state.window();
        assert!(range.is_empty());
        assert_eq!(selected, 0);
    }

    #[test]
    fn the_window_follows_the_selection_and_stops_at_the_end() {
        let mut state = ListState::new(100, 10);
        assert_eq!(state.top(), 0, "at the start the window sits at the top");

        state.move_rows(20);
        assert_eq!(
            state.top(),
            15,
            "the selection sits mid-window while scrolling"
        );

        state.go_last();
        assert_eq!(
            state.top(),
            90,
            "at the end the window stops so the last screen stays full"
        );
    }

    #[test]
    fn the_selection_is_always_inside_the_rows_handed_to_the_ui() {
        // If this drifts, the highlight lands on the wrong row or out of
        // bounds, which looks like a rendering bug.
        let mut state = ListState::new(2400, 12);
        for step in [0, 1, 7, 50, 1200, 2399] {
            state.selected = step;
            let (range, index_in_window) = state.window();
            assert!(
                index_in_window < range.len(),
                "selection {step} outside window"
            );
            assert_eq!(
                range.start + index_in_window,
                step,
                "the highlighted row must be the selected entry"
            );
        }
    }

    #[test]
    fn a_library_shorter_than_the_window_shows_everything_from_the_top() {
        let mut state = ListState::new(3, 12);
        state.go_last();
        assert_eq!(state.top(), 0);
        let (range, index) = state.window();
        assert_eq!(range, 0..3);
        assert_eq!(index, 2);
    }

    #[test]
    fn a_grid_moves_a_whole_row_at_a_time_and_stays_row_aligned() {
        // Without row alignment the tiles slide sideways as it scrolls,
        // which looks broken and makes the motion impossible to judge.
        let mut state = ListState::new(100, 9);
        state.reshape(9, 3);

        assert!(state.move_rows(1));
        assert_eq!(state.selected(), 3, "one row down is three tiles");

        state.move_rows(5);
        assert_eq!(state.selected(), 18);
        assert_eq!(
            state.top() % 3,
            0,
            "the window must start on a row boundary"
        );

        let (range, index) = state.window();
        assert_eq!(range.start + index, state.selected());
    }

    #[test]
    fn reshaping_for_a_new_layout_keeps_the_selection() {
        let mut state = ListState::new(500, 10);
        state.move_rows(30);
        let before = state.selected();
        state.reshape(9, 3);
        assert_eq!(
            state.selected(),
            before,
            "switching layout must not lose your place in the library"
        );
    }
}
