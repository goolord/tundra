//! Click selection shared by the file list and the bulk auto-tag review list.

use std::collections::HashSet;

/// Selected row indices; rows are displayed in index order. Plain click
/// selects one row, Ctrl+click toggles, Shift+click selects the range from
/// the last plain or Ctrl click.
#[derive(Debug, Clone, Default)]
pub struct Selection {
    selected: HashSet<usize>,
    anchor: Option<usize>,
}

impl Selection {
    /// Applies a click on `key`. Shift ranges skip rows `selectable` rejects,
    /// and an anchor it rejects is ignored.
    pub fn click(&mut self, key: usize, shift: bool, control: bool, selectable: impl Fn(usize) -> bool) {
        if control {
            if !self.selected.remove(&key) {
                self.selected.insert(key);
            }
            self.anchor = Some(key);
            return;
        }
        match self.anchor.filter(|&anchor| shift && selectable(anchor)) {
            Some(anchor) => {
                self.selected = (anchor.min(key)..=anchor.max(key)).filter(|&row| selectable(row)).collect()
            }
            None => self.select_only(key),
        }
    }

    pub fn select_only(&mut self, key: usize) {
        self.set([key], Some(key));
    }

    /// Replaces the selection; the anchor becomes `anchor`.
    pub fn set(&mut self, keys: impl IntoIterator<Item = usize>, anchor: Option<usize>) {
        self.selected = keys.into_iter().collect();
        self.anchor = anchor;
    }

    pub fn insert(&mut self, key: usize) {
        self.selected.insert(key);
    }

    pub fn remove(&mut self, key: usize) {
        self.selected.remove(&key);
    }

    pub fn clear(&mut self) {
        self.set([], None);
    }

    pub fn contains(&self, key: usize) -> bool {
        self.selected.contains(&key)
    }

    pub fn len(&self) -> usize {
        self.selected.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = usize> + '_ {
        self.selected.iter().copied()
    }

    pub fn anchor(&self) -> Option<usize> {
        self.anchor
    }
}

#[cfg(test)]
mod tests {
    use super::Selection;

    fn click(selection: &mut Selection, key: usize, shift: bool, control: bool) -> Vec<usize> {
        selection.click(key, shift, control, |row| row < 5);
        let mut keys: Vec<usize> = selection.iter().collect();
        keys.sort_unstable();
        keys
    }

    #[test]
    fn plain_click_selects_one() {
        let mut selection = Selection::default();
        click(&mut selection, 1, false, false);
        assert_eq!(click(&mut selection, 3, false, false), [3]);
    }

    #[test]
    fn control_click_toggles_and_moves_the_anchor() {
        let mut selection = Selection::default();
        click(&mut selection, 1, false, true);
        click(&mut selection, 3, false, true);
        assert_eq!(click(&mut selection, 1, false, true), [3]);
        assert_eq!(selection.anchor(), Some(1));
    }

    #[test]
    fn shift_click_selects_the_range_from_the_anchor_either_way() {
        let mut selection = Selection::default();
        click(&mut selection, 3, false, false);
        assert_eq!(click(&mut selection, 1, true, false), [1, 2, 3]);
        assert_eq!(click(&mut selection, 4, true, false), [3, 4], "the anchor stays put across shift clicks");
    }

    #[test]
    fn shift_click_without_a_usable_anchor_selects_one() {
        let mut selection = Selection::default();
        assert_eq!(click(&mut selection, 2, true, false), [2]);
        click(&mut selection, 9, false, false);
        assert_eq!(click(&mut selection, 1, true, false), [1], "an anchor that left the list is ignored");
    }
}
