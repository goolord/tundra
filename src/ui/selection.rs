//! Click selection shared by the file list and the bulk auto-tag review list.

use std::collections::HashSet;
use std::hash::Hash;

/// Plain click selects one item, Ctrl+click toggles, Shift+click selects the
/// range from the last plain or Ctrl click.
#[derive(Debug, Clone)]
pub struct Selection<K> {
    selected: HashSet<K>,
    anchor: Option<K>,
}

impl<K> Default for Selection<K> {
    fn default() -> Self {
        Self {
            selected: HashSet::new(),
            anchor: None,
        }
    }
}

impl<K: Clone + Eq + Hash> Selection<K> {
    /// Applies a click on `key`. `order` lists every selectable key in display
    /// order; Shift ranges follow it.
    pub fn click(&mut self, key: K, shift: bool, control: bool, order: &[K]) {
        if control {
            if !self.selected.remove(&key) {
                self.selected.insert(key.clone());
            }
            self.anchor = Some(key);
            return;
        }
        if shift && let Some(anchor) = &self.anchor {
            let position = |wanted: &K| order.iter().position(|candidate| candidate == wanted);
            if let Some(from) = position(anchor) {
                let Some(to) = position(&key) else {
                    return;
                };
                self.selected = order[from.min(to)..=from.max(to)].iter().cloned().collect();
                return;
            }
        }
        self.select_only(key);
    }

    pub fn select_only(&mut self, key: K) {
        self.selected.clear();
        self.selected.insert(key.clone());
        self.anchor = Some(key);
    }

    /// Replaces the selection; the anchor becomes `anchor`.
    pub fn set(&mut self, keys: impl IntoIterator<Item = K>, anchor: Option<K>) {
        self.selected = keys.into_iter().collect();
        self.anchor = anchor;
    }

    pub fn insert(&mut self, key: K) {
        self.selected.insert(key);
    }

    pub fn remove(&mut self, key: &K) {
        self.selected.remove(key);
    }

    pub fn clear(&mut self) {
        self.selected.clear();
        self.anchor = None;
    }

    pub fn contains(&self, key: &K) -> bool {
        self.selected.contains(key)
    }

    pub fn len(&self) -> usize {
        self.selected.len()
    }

    pub fn is_empty(&self) -> bool {
        self.selected.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &K> {
        self.selected.iter()
    }

    pub fn anchor(&self) -> Option<&K> {
        self.anchor.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::Selection;

    const ORDER: [u8; 5] = [0, 1, 2, 3, 4];

    fn sorted(selection: &Selection<u8>) -> Vec<u8> {
        let mut keys: Vec<u8> = selection.iter().copied().collect();
        keys.sort_unstable();
        keys
    }

    #[test]
    fn plain_click_selects_one() {
        let mut selection = Selection::default();
        selection.click(1, false, false, &ORDER);
        selection.click(3, false, false, &ORDER);
        assert_eq!(sorted(&selection), [3]);
    }

    #[test]
    fn control_click_toggles_and_moves_the_anchor() {
        let mut selection = Selection::default();
        selection.click(1, false, true, &ORDER);
        selection.click(3, false, true, &ORDER);
        selection.click(1, false, true, &ORDER);
        assert_eq!(sorted(&selection), [3]);
        assert_eq!(selection.anchor(), Some(&1));
    }

    #[test]
    fn shift_click_selects_the_range_from_the_anchor_either_way() {
        let mut selection = Selection::default();
        selection.click(3, false, false, &ORDER);
        selection.click(1, true, false, &ORDER);
        assert_eq!(sorted(&selection), [1, 2, 3]);
        selection.click(4, true, false, &ORDER);
        assert_eq!(sorted(&selection), [3, 4], "the anchor stays put across shift clicks");
    }

    #[test]
    fn shift_click_without_a_usable_anchor_selects_one() {
        let mut selection = Selection::default();
        selection.click(2, true, false, &ORDER);
        assert_eq!(sorted(&selection), [2]);
        selection.click(9, false, false, &ORDER);
        selection.click(1, true, false, &ORDER);
        assert_eq!(sorted(&selection), [1], "an anchor that left the list is ignored");
    }
}
