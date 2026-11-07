//! Minimal multi-line SQL text editor state for the TUI.

use std::cell::Cell;

const TAB_SPACES: &str = "    ";
const DEFAULT_PAGE_ROWS: usize = 10;

/// Text buffer with a cursor. The cursor column counts Unicode scalar values.
pub struct SqlEditor {
    lines: Vec<String>,
    row: usize,
    col: usize,
    // View state updated while rendering, which only has shared access.
    scroll_row: Cell<usize>,
    scroll_col: Cell<usize>,
    page_rows: Cell<usize>,
}

impl Default for SqlEditor {
    fn default() -> Self {
        Self {
            lines: vec![String::new()],
            row: 0,
            col: 0,
            scroll_row: Cell::new(0),
            scroll_col: Cell::new(0),
            page_rows: Cell::new(DEFAULT_PAGE_ROWS),
        }
    }
}

impl SqlEditor {
    /// Creates an editor holding `text` with the cursor at the start.
    pub fn from_text(text: &str) -> Self {
        let mut editor = Self::default();
        editor.insert_str(text);
        editor.row = 0;
        editor.col = 0;
        editor
    }

    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    /// Cursor as `(row, column)`.
    pub fn cursor(&self) -> (usize, usize) {
        (self.row, self.col)
    }

    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    pub fn insert_char(&mut self, character: char) {
        let index = self.byte_index();
        self.lines[self.row].insert(index, character);
        self.col += 1;
    }

    pub fn insert_tab(&mut self) {
        self.insert_str(TAB_SPACES);
    }

    /// Inserts text at the cursor, normalizing `\r\n` and `\r` to new lines.
    pub fn insert_str(&mut self, text: &str) {
        let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
        for (index, part) in normalized.split('\n').enumerate() {
            if index > 0 {
                self.new_line();
            }
            let byte_index = self.byte_index();
            self.lines[self.row].insert_str(byte_index, part);
            self.col += part.chars().count();
        }
    }

    pub fn new_line(&mut self) {
        let index = self.byte_index();
        let rest = self.lines[self.row].split_off(index);
        self.row += 1;
        self.col = 0;
        self.lines.insert(self.row, rest);
    }

    pub fn backspace(&mut self) {
        if self.col > 0 {
            self.col -= 1;
            let index = self.byte_index();
            self.lines[self.row].remove(index);
        } else if self.row > 0 {
            let line = self.lines.remove(self.row);
            self.row -= 1;
            self.col = self.line_len(self.row);
            self.lines[self.row].push_str(&line);
        }
    }

    pub fn delete(&mut self) {
        if self.col < self.line_len(self.row) {
            let index = self.byte_index();
            self.lines[self.row].remove(index);
        } else if self.row + 1 < self.lines.len() {
            let next = self.lines.remove(self.row + 1);
            self.lines[self.row].push_str(&next);
        }
    }

    pub fn move_left(&mut self) {
        if self.col > 0 {
            self.col -= 1;
        } else if self.row > 0 {
            self.row -= 1;
            self.col = self.line_len(self.row);
        }
    }

    pub fn move_right(&mut self) {
        if self.col < self.line_len(self.row) {
            self.col += 1;
        } else if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = 0;
        }
    }

    pub fn move_up(&mut self) {
        self.move_rows_up(1);
    }

    pub fn move_down(&mut self) {
        self.move_rows_down(1);
    }

    pub fn page_up(&mut self) {
        self.move_rows_up(self.page_rows.get());
    }

    pub fn page_down(&mut self) {
        self.move_rows_down(self.page_rows.get());
    }

    pub fn move_home(&mut self) {
        self.col = 0;
    }

    pub fn move_end(&mut self) {
        self.col = self.line_len(self.row);
    }

    /// Returns the first visible `(row, column)` for a viewport of
    /// `height` x `width` cells, scrolling only as needed to show the cursor.
    pub fn viewport_offset(&self, height: usize, width: usize) -> (usize, usize) {
        let height = height.max(1);
        let width = width.max(1);
        self.page_rows.set(height);
        let scroll_row = keep_visible(self.scroll_row.get(), self.row, height);
        let scroll_col = keep_visible(self.scroll_col.get(), self.col, width);
        self.scroll_row.set(scroll_row);
        self.scroll_col.set(scroll_col);
        (scroll_row, scroll_col)
    }

    fn move_rows_up(&mut self, rows: usize) {
        self.row = self.row.saturating_sub(rows);
        self.col = self.col.min(self.line_len(self.row));
    }

    fn move_rows_down(&mut self, rows: usize) {
        self.row = (self.row + rows).min(self.lines.len() - 1);
        self.col = self.col.min(self.line_len(self.row));
    }

    fn line_len(&self, row: usize) -> usize {
        self.lines[row].chars().count()
    }

    fn byte_index(&self) -> usize {
        let line = &self.lines[self.row];
        line.char_indices()
            .nth(self.col)
            .map_or(line.len(), |(index, _)| index)
    }
}

fn keep_visible(scroll: usize, position: usize, size: usize) -> usize {
    if position < scroll {
        position
    } else if position >= scroll + size {
        position + 1 - size
    } else {
        scroll
    }
}

#[cfg(test)]
mod tests {
    use super::SqlEditor;

    fn editor_with(text: &str) -> SqlEditor {
        let mut editor = SqlEditor::default();
        editor.insert_str(text);
        editor
    }

    #[test]
    fn inserts_text_lines_and_unicode_at_cursor() {
        let mut editor = SqlEditor::default();
        for character in "SELECT ção".chars() {
            editor.insert_char(character);
        }
        editor.move_left();
        editor.move_left();
        editor.insert_char('x');
        assert_eq!(editor.text(), "SELECT çxão");
        editor.move_end();
        editor.new_line();
        editor.insert_tab();
        editor.insert_str("FROM t");
        assert_eq!(editor.text(), "SELECT çxão\n    FROM t");
        assert_eq!(editor.cursor(), (1, 10));
    }

    #[test]
    fn from_text_keeps_content_and_starts_at_origin() {
        let editor = SqlEditor::from_text("SELECT 1;\r\nSELECT 2;");
        assert_eq!(editor.text(), "SELECT 1;\nSELECT 2;");
        assert_eq!(editor.cursor(), (0, 0));
    }

    #[test]
    fn paste_normalizes_line_endings() {
        let editor = editor_with("SELECT 1;\r\nSELECT 2;\rSELECT 3;");
        assert_eq!(editor.text(), "SELECT 1;\nSELECT 2;\nSELECT 3;");
        assert_eq!(editor.cursor(), (2, 9));
    }

    #[test]
    fn backspace_and_delete_join_lines() {
        let mut editor = editor_with("ab\ncd");
        editor.move_home();
        editor.backspace();
        assert_eq!(editor.text(), "abcd");
        assert_eq!(editor.cursor(), (0, 2));
        editor.backspace();
        assert_eq!(editor.text(), "acd");

        let mut editor = editor_with("ab\ncd");
        editor.move_up();
        editor.move_end();
        editor.delete();
        assert_eq!(editor.text(), "abcd");
        editor.move_end();
        editor.delete();
        editor.move_home();
        editor.backspace();
        assert_eq!(editor.text(), "abcd");
        assert_eq!(editor.cursor(), (0, 0));
    }

    #[test]
    fn cursor_moves_across_lines_and_clamps_columns() {
        let mut editor = editor_with("long line\nab\nlonger line");
        assert_eq!(editor.cursor(), (2, 11));
        editor.move_up();
        assert_eq!(editor.cursor(), (1, 2));
        editor.move_right();
        assert_eq!(editor.cursor(), (2, 0));
        editor.move_left();
        assert_eq!(editor.cursor(), (1, 2));
        editor.page_up();
        assert_eq!(editor.cursor(), (0, 2));
        editor.move_up();
        assert_eq!(editor.cursor(), (0, 2));
        editor.page_down();
        assert_eq!(editor.cursor(), (2, 2));
        editor.move_down();
        assert_eq!(editor.cursor(), (2, 2));
    }

    #[test]
    fn viewport_scrolls_only_to_keep_cursor_visible() {
        let mut editor = editor_with(&"x\n".repeat(20));
        assert_eq!(editor.cursor(), (20, 0));
        assert_eq!(editor.viewport_offset(5, 10), (16, 0));
        editor.move_up();
        assert_eq!(editor.viewport_offset(5, 10), (16, 0));
        for _ in 0..5 {
            editor.move_up();
        }
        assert_eq!(editor.viewport_offset(5, 10), (14, 0));

        editor.insert_str("abcdefghijkl");
        assert_eq!(editor.viewport_offset(5, 10), (14, 3));
        editor.move_home();
        assert_eq!(editor.viewport_offset(5, 10), (14, 0));

        editor.page_up();
        assert_eq!(editor.cursor(), (9, 0));
    }
}
