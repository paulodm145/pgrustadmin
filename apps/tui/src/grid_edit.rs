//! Pending cell changes and the cell editor of an editable data grid.
//! See ADR-0005 and `specs/data-grid.md`.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Primary key column names with the text of one row's key values.
pub type RowKey = Vec<(String, String)>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingCell {
    pub column: String,
    /// Value read from the server, checked again when saving.
    pub original: Option<String>,
    pub value: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingRow {
    pub key: RowKey,
    pub cells: Vec<PendingCell>,
}

/// Changed cells waiting to be saved, grouped by row key so they survive
/// reloads, paging, sorting and searching.
#[derive(Debug, Default)]
pub struct PendingChanges {
    rows: Vec<PendingRow>,
}

impl PendingChanges {
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn cell_count(&self) -> usize {
        self.rows.iter().map(|row| row.cells.len()).sum()
    }

    pub fn rows(&self) -> &[PendingRow] {
        &self.rows
    }

    /// Pending value of a cell; `Some(None)` is a pending `NULL`.
    pub fn value(&self, key: &RowKey, column: &str) -> Option<&Option<String>> {
        self.cell(key, column).map(|cell| &cell.value)
    }

    fn cell(&self, key: &RowKey, column: &str) -> Option<&PendingCell> {
        self.rows
            .iter()
            .find(|row| row.key == *key)
            .and_then(|row| row.cells.iter().find(|cell| cell.column == column))
    }

    /// Records a new value; going back to the original drops the change.
    pub fn set(
        &mut self,
        key: RowKey,
        column: &str,
        original: Option<String>,
        value: Option<String>,
    ) {
        let original = self
            .cell(&key, column)
            .map_or(original, |cell| cell.original.clone());
        self.discard(&key, column);
        if value == original {
            return;
        }
        let cell = PendingCell {
            column: column.to_owned(),
            original,
            value,
        };
        match self.rows.iter_mut().find(|row| row.key == key) {
            Some(row) => row.cells.push(cell),
            None => self.rows.push(PendingRow {
                key,
                cells: vec![cell],
            }),
        }
    }

    /// Drops the change of one cell; returns whether there was one.
    pub fn discard(&mut self, key: &RowKey, column: &str) -> bool {
        let Some(position) = self.rows.iter().position(|row| row.key == *key) else {
            return false;
        };
        let cells = &mut self.rows[position].cells;
        let before = cells.len();
        cells.retain(|cell| cell.column != column);
        let removed = cells.len() != before;
        if cells.is_empty() {
            self.rows.remove(position);
        }
        removed
    }

    pub fn clear(&mut self) {
        self.rows.clear();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditorKey {
    Continue,
    Commit,
    Cancel,
}

/// Single-line text input for one cell, with `NULL` as a separate state.
#[derive(Debug)]
pub struct CellEditor {
    pub key: RowKey,
    pub column: String,
    pub original: Option<String>,
    text: Vec<char>,
    /// Cursor position in characters.
    cursor: usize,
    null: bool,
}

impl CellEditor {
    pub fn new(
        key: RowKey,
        column: String,
        original: Option<String>,
        current: Option<&str>,
    ) -> Self {
        let text: Vec<char> = current.unwrap_or_default().chars().collect();
        Self {
            key,
            column,
            original,
            cursor: text.len(),
            text,
            null: current.is_none(),
        }
    }

    /// Edited value; `None` is SQL `NULL`.
    pub fn value(&self) -> Option<String> {
        (!self.null).then(|| self.text.iter().collect())
    }

    pub fn is_null(&self) -> bool {
        self.null
    }

    /// Text before and after the cursor.
    pub fn split_at_cursor(&self) -> (String, String) {
        (
            self.text[..self.cursor].iter().collect(),
            self.text[self.cursor..].iter().collect(),
        )
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> EditorKey {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Enter => return EditorKey::Commit,
            KeyCode::Esc => return EditorKey::Cancel,
            KeyCode::Char('n') if control => {
                self.null = true;
                self.text.clear();
                self.cursor = 0;
            }
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(self.text.len()),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.text.len(),
            KeyCode::Backspace if self.cursor > 0 => {
                self.cursor -= 1;
                self.text.remove(self.cursor);
                self.null = false;
            }
            KeyCode::Delete if self.cursor < self.text.len() => {
                self.text.remove(self.cursor);
                self.null = false;
            }
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.text.insert(self.cursor, character);
                self.cursor += 1;
                self.null = false;
            }
            _ => {}
        }
        EditorKey::Continue
    }

    /// Inserts pasted text at the cursor; line breaks become spaces.
    pub fn paste(&mut self, text: &str) {
        for character in text.chars() {
            let character = if matches!(character, '\n' | '\r') {
                ' '
            } else {
                character
            };
            self.text.insert(self.cursor, character);
            self.cursor += 1;
        }
        self.null = false;
    }
}

/// Editor used for a column's cells.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ColumnEditor {
    #[default]
    Text,
    /// `[x]`/`[ ]` toggled in place.
    Boolean,
    /// One of these labels, picked from a list (enums).
    Choice(Vec<String>),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ColumnType {
    pub editor: ColumnEditor,
    pub nullable: bool,
}

/// Whether a PostgreSQL boolean text is true; `None` for other text.
pub fn boolean_value(text: &str) -> Option<bool> {
    match text {
        "true" | "t" => Some(true),
        "false" | "f" => Some(false),
        _ => None,
    }
}

/// Next value when toggling a boolean: true, false, then `NULL` when the
/// column accepts it.
pub fn toggled_boolean(current: Option<&str>, nullable: bool) -> Option<String> {
    let next = match current.and_then(boolean_value) {
        Some(true) => Some(false),
        Some(false) if nullable => None,
        Some(false) | None => Some(true),
    };
    next.map(|value| value.to_string())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickerKey {
    Continue,
    Pick,
    Cancel,
}

/// List of allowed values for one cell, filtered by typing.
#[derive(Debug)]
pub struct ChoicePicker {
    pub key: RowKey,
    pub column: String,
    pub original: Option<String>,
    /// Allowed values; `None` is `NULL`, offered for nullable columns.
    options: Vec<Option<String>>,
    filter: String,
    /// Position in [`ChoicePicker::visible`].
    selected: usize,
}

impl ChoicePicker {
    pub fn new(
        key: RowKey,
        column: String,
        original: Option<String>,
        labels: &[String],
        nullable: bool,
        current: Option<&str>,
    ) -> Self {
        let mut options: Vec<Option<String>> = labels.iter().cloned().map(Some).collect();
        if nullable {
            options.push(None);
        }
        let selected = options
            .iter()
            .position(|option| option.as_deref() == current)
            .unwrap_or(0);
        Self {
            key,
            column,
            original,
            options,
            filter: String::new(),
            selected,
        }
    }

    pub fn filter(&self) -> &str {
        &self.filter
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Options matching the filter, case-insensitively.
    pub fn visible(&self) -> Vec<&Option<String>> {
        let needle = self.filter.to_lowercase();
        self.options
            .iter()
            .filter(|option| {
                option
                    .as_deref()
                    .unwrap_or("NULL")
                    .to_lowercase()
                    .contains(&needle)
            })
            .collect()
    }

    /// The highlighted value; `None` inside is `NULL`.
    pub fn choice(&self) -> Option<Option<String>> {
        self.visible()
            .get(self.selected)
            .map(|option| (*option).clone())
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> PickerKey {
        let count = self.visible().len();
        match key.code {
            KeyCode::Enter if count > 0 => return PickerKey::Pick,
            KeyCode::Esc => return PickerKey::Cancel,
            KeyCode::Up if count > 0 => self.selected = (self.selected + count - 1) % count,
            KeyCode::Down if count > 0 => self.selected = (self.selected + 1) % count,
            KeyCode::Home => self.selected = 0,
            KeyCode::End => self.selected = count.saturating_sub(1),
            KeyCode::Backspace => {
                self.filter.pop();
                self.selected = 0;
            }
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.filter.push(character);
                self.selected = 0;
            }
            _ => {}
        }
        PickerKey::Continue
    }

    pub fn paste(&mut self, text: &str) {
        self.filter
            .extend(text.chars().filter(|character| !character.is_control()));
        self.selected = 0;
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::{
        CellEditor, ChoicePicker, EditorKey, PendingChanges, PickerKey, RowKey, boolean_value,
        toggled_boolean,
    };

    #[test]
    fn booleans_toggle_through_null_only_when_nullable() {
        assert_eq!(boolean_value("t"), Some(true));
        assert_eq!(boolean_value("false"), Some(false));
        assert_eq!(boolean_value("yes"), None);
        assert_eq!(
            toggled_boolean(Some("true"), false).as_deref(),
            Some("false")
        );
        assert_eq!(
            toggled_boolean(Some("false"), false).as_deref(),
            Some("true")
        );
        assert_eq!(toggled_boolean(Some("f"), true), None);
        assert_eq!(toggled_boolean(None, true).as_deref(), Some("true"));
    }

    #[test]
    fn choice_picker_filters_moves_and_offers_null_for_nullable_columns() {
        let press = |picker: &mut ChoicePicker, code: KeyCode| {
            picker.handle_key(KeyEvent::new(code, KeyModifiers::NONE))
        };
        let labels = ["sad".to_owned(), "ok".to_owned(), "happy".to_owned()];
        let mut picker =
            ChoicePicker::new(key("1"), "mood".to_owned(), None, &labels, true, Some("ok"));
        assert_eq!(
            picker.choice(),
            Some(Some("ok".to_owned())),
            "starts on the value"
        );
        assert_eq!(picker.visible().len(), 4);
        press(&mut picker, KeyCode::Down);
        press(&mut picker, KeyCode::Down);
        assert_eq!(picker.choice(), Some(None), "NULL is the last option");
        press(&mut picker, KeyCode::Down);
        assert_eq!(
            picker.choice(),
            Some(Some("sad".to_owned())),
            "wraps around"
        );

        press(&mut picker, KeyCode::Char('A'));
        assert_eq!(picker.visible().len(), 2, "sad and happy match");
        picker.paste("pp");
        assert_eq!(picker.choice(), Some(Some("happy".to_owned())));
        assert_eq!(press(&mut picker, KeyCode::Enter), PickerKey::Pick);
        picker.paste("zzz");
        assert_eq!(picker.choice(), None);
        assert_eq!(
            press(&mut picker, KeyCode::Enter),
            PickerKey::Continue,
            "nothing to pick"
        );
        assert_eq!(press(&mut picker, KeyCode::Esc), PickerKey::Cancel);

        let required = ChoicePicker::new(key("1"), "mood".to_owned(), None, &labels, false, None);
        assert_eq!(required.visible().len(), 3, "no NULL for NOT NULL columns");
    }

    fn key(id: &str) -> RowKey {
        vec![("id".to_owned(), id.to_owned())]
    }

    #[test]
    fn pending_changes_group_by_row_and_drop_reverted_values() {
        let mut pending = PendingChanges::default();
        pending.set(
            key("1"),
            "name",
            Some("ana".to_owned()),
            Some("bia".to_owned()),
        );
        pending.set(key("1"), "city", None, Some("Recife".to_owned()));
        pending.set(key("2"), "name", Some("caio".to_owned()), None);
        assert_eq!(pending.rows().len(), 2);
        assert_eq!(pending.cell_count(), 3);
        assert_eq!(pending.value(&key("2"), "name"), Some(&None));

        pending.set(
            key("1"),
            "name",
            Some("bia".to_owned()),
            Some("eva".to_owned()),
        );
        assert_eq!(
            pending.rows()[0].cells[1].original.as_deref(),
            Some("ana"),
            "the first original is kept"
        );
        pending.set(
            key("1"),
            "name",
            Some("eva".to_owned()),
            Some("ana".to_owned()),
        );
        assert_eq!(
            pending.value(&key("1"), "name"),
            None,
            "back to the original"
        );
        assert_eq!(pending.cell_count(), 2);

        assert!(pending.discard(&key("2"), "name"));
        assert!(!pending.discard(&key("2"), "name"));
        assert_eq!(pending.rows().len(), 1);
        pending.clear();
        assert!(pending.is_empty());
    }

    #[test]
    fn cell_editor_edits_text_at_the_cursor_and_sets_null() {
        let press = |editor: &mut CellEditor, code: KeyCode, modifiers: KeyModifiers| {
            editor.handle_key(KeyEvent::new(code, modifiers))
        };
        let mut editor = CellEditor::new(key("1"), "name".to_owned(), None, Some("ação"));
        assert_eq!(editor.split_at_cursor(), ("ação".to_owned(), String::new()));
        press(&mut editor, KeyCode::Home, KeyModifiers::NONE);
        press(&mut editor, KeyCode::Right, KeyModifiers::NONE);
        press(&mut editor, KeyCode::Delete, KeyModifiers::NONE);
        press(&mut editor, KeyCode::Char('ç'), KeyModifiers::NONE);
        assert_eq!(editor.value().as_deref(), Some("ação"));
        press(&mut editor, KeyCode::Backspace, KeyModifiers::NONE);
        assert_eq!(editor.value().as_deref(), Some("aão"));

        press(&mut editor, KeyCode::Char('n'), KeyModifiers::CONTROL);
        assert!(editor.is_null());
        assert_eq!(editor.value(), None);
        editor.paste("a\nb");
        assert_eq!(editor.value().as_deref(), Some("a b"), "typing leaves NULL");
        assert_eq!(
            press(&mut editor, KeyCode::Enter, KeyModifiers::NONE),
            EditorKey::Commit
        );
        assert_eq!(
            press(&mut editor, KeyCode::Esc, KeyModifiers::NONE),
            EditorKey::Cancel
        );

        let empty = CellEditor::new(key("1"), "name".to_owned(), None, None);
        assert!(empty.is_null(), "NULL cells open as NULL");
    }
}
