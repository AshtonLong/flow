//! Editing state for list settings (dictionary, snippets). Rows being typed can
//! be incomplete; only complete rows are written to the file, and the list
//! resets when the file changes underneath. Port of `lib/listDraft.ts`.
//!
//! Keep a `ListDraft` in the page struct, call [`ListDraft::sync`] with the
//! saved list every frame, and write whatever `add`/`update`/`remove` return.

use serde::Serialize;

#[derive(Debug, Clone, PartialEq)]
pub struct DraftRow<T> {
    /// Stable across edits: use it to key per-row widget ids.
    pub key: u64,
    pub value: T,
}

pub struct ListDraft<T> {
    rows: Vec<DraftRow<T>>,
    next_key: u64,
    committed: String,
    is_complete: fn(&T) -> bool,
}

fn signature<T: Serialize>(values: &[T]) -> String {
    serde_json::to_string(values).unwrap_or_default()
}

impl<T: Clone + Serialize> ListDraft<T> {
    pub fn new(saved: &[T], is_complete: fn(&T) -> bool) -> Self {
        let mut draft = Self { rows: Vec::new(), next_key: 0, committed: signature(saved), is_complete };
        draft.rows = draft.wrap(saved);
        draft
    }

    fn wrap(&mut self, values: &[T]) -> Vec<DraftRow<T>> {
        values
            .iter()
            .map(|value| {
                let key = self.next_key;
                self.next_key += 1;
                DraftRow { key, value: value.clone() }
            })
            .collect()
    }

    pub fn rows(&self) -> &[DraftRow<T>] {
        &self.rows
    }

    /// Reacts to the file changing: when `saved` differs from what was last
    /// committed and from the complete local rows, the draft resets to it.
    pub fn sync(&mut self, saved: &[T]) {
        let incoming = signature(saved);
        if incoming == self.committed {
            return;
        }
        self.committed = incoming.clone();
        if signature(&self.complete()) != incoming {
            self.rows = self.wrap(saved);
        }
    }

    fn complete(&self) -> Vec<T> {
        self.rows.iter().filter(|r| (self.is_complete)(&r.value)).map(|r| r.value.clone()).collect()
    }

    /// Replaces the rows; returns the complete rows to write when they changed.
    fn apply(&mut self, next: Vec<DraftRow<T>>) -> Option<Vec<T>> {
        self.rows = next;
        let complete = self.complete();
        let sig = signature(&complete);
        if sig != self.committed {
            self.committed = sig;
            return Some(complete);
        }
        None
    }

    pub fn add(&mut self, value: T) -> Option<Vec<T>> {
        let key = self.next_key;
        self.next_key += 1;
        let mut next = self.rows.clone();
        next.push(DraftRow { key, value });
        self.apply(next)
    }

    pub fn update(&mut self, key: u64, value: T) -> Option<Vec<T>> {
        let next = self
            .rows
            .iter()
            .map(|r| if r.key == key { DraftRow { key, value: value.clone() } } else { r.clone() })
            .collect();
        self.apply(next)
    }

    pub fn remove(&mut self, key: u64) -> Option<Vec<T>> {
        let next = self.rows.iter().filter(|r| r.key != key).cloned().collect();
        self.apply(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::ptr_arg)] // `ListDraft` takes `fn(&T)`, here `fn(&String)`.
    fn complete(s: &String) -> bool {
        !s.is_empty()
    }

    #[test]
    fn writes_only_complete_rows() {
        let mut draft = ListDraft::new(&["a".to_string()], complete);
        assert_eq!(draft.add(String::new()), None, "an empty row is not written");
        let key = draft.rows()[1].key;
        assert_eq!(draft.update(key, "b".into()), Some(vec!["a".to_string(), "b".to_string()]));
        assert_eq!(draft.remove(draft.rows()[0].key), Some(vec!["b".to_string()]));
    }

    #[test]
    fn resets_when_the_file_changes_underneath() {
        let mut draft = ListDraft::new(&["a".to_string()], complete);
        draft.add(String::new());
        // Our own write coming back keeps the incomplete row.
        draft.sync(&["a".to_string()]);
        assert_eq!(draft.rows().len(), 2);
        // Someone else's edit replaces it.
        draft.sync(&["x".to_string(), "y".to_string()]);
        let values: Vec<&str> = draft.rows().iter().map(|r| r.value.as_str()).collect();
        assert_eq!(values, vec!["x", "y"]);
    }
}
