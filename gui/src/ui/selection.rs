//! Explorer-style row selection: a set of rows plus an anchor (start of
//! shift ranges) and a focus (the row keyboard moves and Details follow).
//!
//! All operations take the rows in display order, because ranges and
//! navigation depend on the current sort and filter.

use std::collections::HashSet;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub ctrl: bool,
    pub shift: bool,
}

#[derive(Debug, Default)]
pub struct Selection {
    selected: HashSet<Uuid>,
    anchor: Option<Uuid>,
    focus: Option<Uuid>,
}

impl Selection {
    pub fn click(&mut self, id: Uuid, rows: &[Uuid], mods: Mods) {
        // A range needs an anchor that is still visible; otherwise the click
        // starts a new one like a plain click.
        let range_anchor = self
            .anchor
            .filter(|anchor| rows.contains(anchor) && rows.contains(&id));
        match (mods.shift, range_anchor) {
            (true, Some(anchor)) => {
                if !mods.ctrl {
                    self.selected.clear();
                }
                self.select_range(rows, anchor, id);
                self.focus = Some(id);
            }
            _ if mods.ctrl => {
                if !self.selected.remove(&id) {
                    self.selected.insert(id);
                }
                self.anchor = Some(id);
                self.focus = Some(id);
            }
            _ => self.select_only(id),
        }
    }

    /// Right click keeps a multi-selection so the menu can act on all of it.
    pub fn secondary_click(&mut self, id: Uuid) {
        if !self.selected.contains(&id) {
            self.select_only(id);
        }
        self.focus = Some(id);
    }

    /// Up/Down/PgUp/PgDn. Moving from no focus lands on the first row.
    pub fn move_focus(&mut self, delta: isize, rows: &[Uuid], mods: Mods) {
        if rows.is_empty() {
            return;
        }
        let last = rows.len() - 1;
        let target = match self.focus_index(rows) {
            Some(current) => current.saturating_add_signed(delta).min(last),
            None => 0,
        };
        self.move_to(target, rows, mods);
    }

    /// Home/End.
    pub fn focus_edge(&mut self, end: bool, rows: &[Uuid], mods: Mods) {
        if rows.is_empty() {
            return;
        }
        self.move_to(if end { rows.len() - 1 } else { 0 }, rows, mods);
    }

    pub fn toggle_focused(&mut self) {
        let Some(id) = self.focus else {
            return;
        };
        if !self.selected.remove(&id) {
            self.selected.insert(id);
        }
        self.anchor = Some(id);
    }

    // Used by the Select All command and by actions on the selection.
    #[allow(dead_code)]
    pub fn select_all(&mut self, rows: &[Uuid]) {
        self.selected = rows.iter().copied().collect();
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Drops rows that left the view (deleted, moved, filtered out).
    pub fn retain(&mut self, rows: &[Uuid]) {
        let present: HashSet<Uuid> = rows.iter().copied().collect();
        self.selected.retain(|id| present.contains(id));
        self.anchor = self.anchor.filter(|id| present.contains(id));
        self.focus = self.focus.filter(|id| present.contains(id));
    }

    /// Rows an action applies to: the selection in display order, or just the
    /// focused row when nothing is selected.
    #[allow(dead_code)]
    pub fn targets(&self, rows: &[Uuid]) -> Vec<Uuid> {
        let selected: Vec<Uuid> = rows
            .iter()
            .copied()
            .filter(|id| self.selected.contains(id))
            .collect();
        if selected.is_empty() {
            self.focus.into_iter().collect()
        } else {
            selected
        }
    }

    pub fn focus(&self) -> Option<Uuid> {
        self.focus
    }

    pub fn is_selected(&self, id: Uuid) -> bool {
        self.selected.contains(&id)
    }

    pub fn len(&self) -> usize {
        self.selected.len()
    }

    fn select_only(&mut self, id: Uuid) {
        self.selected.clear();
        self.selected.insert(id);
        self.anchor = Some(id);
        self.focus = Some(id);
    }

    fn focus_index(&self, rows: &[Uuid]) -> Option<usize> {
        let focus = self.focus?;
        rows.iter().position(|id| *id == focus)
    }

    fn select_range(&mut self, rows: &[Uuid], a: Uuid, b: Uuid) {
        let (Some(a), Some(b)) = (
            rows.iter().position(|id| *id == a),
            rows.iter().position(|id| *id == b),
        ) else {
            return;
        };
        self.selected
            .extend(rows[a.min(b)..=a.max(b)].iter().copied());
    }

    fn move_to(&mut self, index: usize, rows: &[Uuid], mods: Mods) {
        let id = rows[index];
        if mods.shift {
            self.click(id, rows, mods);
        } else if mods.ctrl {
            // Ctrl moves the focus without touching the selection.
            self.focus = Some(id);
        } else {
            self.select_only(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Mods = Mods { ctrl: false, shift: false };
    const CTRL: Mods = Mods { ctrl: true, shift: false };
    const SHIFT: Mods = Mods { ctrl: false, shift: true };
    const CTRL_SHIFT: Mods = Mods { ctrl: true, shift: true };

    fn ids(n: usize) -> Vec<Uuid> {
        (0..n).map(|_| Uuid::new_v4()).collect()
    }

    fn selected_in_order(selection: &Selection, rows: &[Uuid]) -> Vec<usize> {
        rows.iter()
            .enumerate()
            .filter(|(_, id)| selection.is_selected(**id))
            .map(|(i, _)| i)
            .collect()
    }

    #[test]
    fn plain_click_selects_only() {
        let rows = ids(4);
        let mut sel = Selection::default();
        sel.click(rows[0], &rows, NONE);
        sel.click(rows[2], &rows, CTRL);
        sel.click(rows[1], &rows, NONE);
        assert_eq!(selected_in_order(&sel, &rows), [1]);
        assert_eq!(sel.focus(), Some(rows[1]));
    }

    #[test]
    fn ctrl_click_toggles() {
        let rows = ids(4);
        let mut sel = Selection::default();
        sel.click(rows[0], &rows, NONE);
        sel.click(rows[2], &rows, CTRL);
        assert_eq!(selected_in_order(&sel, &rows), [0, 2]);
        sel.click(rows[0], &rows, CTRL);
        assert_eq!(selected_in_order(&sel, &rows), [2]);
        assert_eq!(sel.focus(), Some(rows[0]));
    }

    #[test]
    fn shift_click_selects_range_from_anchor() {
        let rows = ids(6);
        let mut sel = Selection::default();
        sel.click(rows[1], &rows, NONE);
        sel.click(rows[4], &rows, SHIFT);
        assert_eq!(selected_in_order(&sel, &rows), [1, 2, 3, 4]);

        // The anchor stays, so a second shift click replaces the range, also upwards.
        sel.click(rows[0], &rows, SHIFT);
        assert_eq!(selected_in_order(&sel, &rows), [0, 1]);
        assert_eq!(sel.focus(), Some(rows[0]));
    }

    #[test]
    fn ctrl_shift_click_adds_range() {
        let rows = ids(8);
        let mut sel = Selection::default();
        sel.click(rows[0], &rows, NONE);
        sel.click(rows[2], &rows, CTRL);
        sel.click(rows[5], &rows, CTRL_SHIFT);
        assert_eq!(selected_in_order(&sel, &rows), [0, 2, 3, 4, 5]);
    }

    #[test]
    fn shift_click_without_anchor_acts_like_plain_click() {
        let rows = ids(3);
        let mut sel = Selection::default();
        sel.click(rows[2], &rows, SHIFT);
        assert_eq!(selected_in_order(&sel, &rows), [2]);
        sel.click(rows[0], &rows, SHIFT);
        assert_eq!(selected_in_order(&sel, &rows), [0, 1, 2]);
    }

    #[test]
    fn secondary_click_keeps_existing_selection() {
        let rows = ids(4);
        let mut sel = Selection::default();
        sel.click(rows[0], &rows, NONE);
        sel.click(rows[1], &rows, CTRL);

        sel.secondary_click(rows[1]);
        assert_eq!(selected_in_order(&sel, &rows), [0, 1]);
        assert_eq!(sel.focus(), Some(rows[1]));

        sel.secondary_click(rows[3]);
        assert_eq!(selected_in_order(&sel, &rows), [3]);
    }

    #[test]
    fn retain_drops_vanished_ids() {
        let rows = ids(4);
        let mut sel = Selection::default();
        sel.click(rows[0], &rows, NONE);
        sel.click(rows[3], &rows, SHIFT);
        assert_eq!(sel.len(), 4);

        sel.retain(&[rows[1], rows[2]]);
        assert_eq!(selected_in_order(&sel, &rows), [1, 2]);
        assert_eq!(sel.focus(), None);

        // The anchor went with row 0, so a shift click now starts over.
        sel.click(rows[2], &rows[1..3], SHIFT);
        assert_eq!(sel.len(), 1);
    }

    #[test]
    fn targets_fall_back_to_focus() {
        let rows = ids(4);
        let mut sel = Selection::default();
        assert!(sel.targets(&rows).is_empty());

        sel.click(rows[1], &rows, NONE);
        sel.click(rows[1], &rows, CTRL);
        assert_eq!(sel.len(), 0);
        assert_eq!(sel.targets(&rows), [rows[1]]);

        sel.click(rows[3], &rows, CTRL);
        sel.click(rows[0], &rows, CTRL);
        // Display order, not click order.
        assert_eq!(sel.targets(&rows), [rows[0], rows[3]]);
    }

    #[test]
    fn keyboard_moves_follow_the_modifiers() {
        let rows = ids(5);
        let mut sel = Selection::default();

        sel.move_focus(1, &rows, NONE);
        assert_eq!(sel.focus(), Some(rows[0]));
        sel.move_focus(1, &rows, NONE);
        assert_eq!(selected_in_order(&sel, &rows), [1]);

        sel.move_focus(2, &rows, SHIFT);
        assert_eq!(selected_in_order(&sel, &rows), [1, 2, 3]);
        sel.move_focus(-1, &rows, SHIFT);
        assert_eq!(selected_in_order(&sel, &rows), [1, 2]);

        // Ctrl moves the focus only; Ctrl+Space then adds that row.
        sel.move_focus(1, &rows, CTRL);
        assert_eq!(sel.focus(), Some(rows[3]));
        assert_eq!(selected_in_order(&sel, &rows), [1, 2]);
        sel.toggle_focused();
        assert_eq!(selected_in_order(&sel, &rows), [1, 2, 3]);

        sel.focus_edge(true, &rows, NONE);
        assert_eq!(selected_in_order(&sel, &rows), [4]);
        sel.move_focus(10, &rows, NONE);
        assert_eq!(sel.focus(), Some(rows[4]));
        sel.focus_edge(false, &rows, SHIFT);
        assert_eq!(selected_in_order(&sel, &rows), [0, 1, 2, 3, 4]);
    }

    #[test]
    fn select_all_and_clear() {
        let rows = ids(3);
        let mut sel = Selection::default();
        sel.select_all(&rows);
        assert_eq!(sel.len(), 3);
        sel.clear();
        assert_eq!(sel.len(), 0);
        assert_eq!(sel.focus(), None);
    }
}
