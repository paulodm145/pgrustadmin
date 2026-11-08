//! Reusable data grid: paging, sorting, search, active row/cell and a
//! value popup, for query results (local rows) and relation previews
//! (pages requested from the server). See `specs/data-grid.md`.

use std::{cell::Cell as StateCell, cmp::Ordering};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, TableState, Wrap},
};

use crate::theme::Theme;

const MAX_COLUMN_WIDTH: usize = 40;
const MIN_COLUMN_WIDTH: usize = 4;
const MAX_PAGE_SIZE: usize = 100;
const FIXED_PAGE_SIZES: [usize; 4] = [10, 25, 50, 100];
/// Page numbers shown around the current one in the pager.
const PAGER_NEIGHBOURS: usize = 2;

pub type GridRow = Vec<Option<String>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageSize {
    /// As many rows as fit in the visible area.
    Auto,
    Fixed(usize),
}

impl PageSize {
    fn cycle(self, forward: bool) -> Self {
        let mut sizes = vec![Self::Auto];
        sizes.extend(FIXED_PAGE_SIZES.map(Self::Fixed));
        let index = sizes.iter().position(|size| *size == self).unwrap_or(0);
        let next = if forward {
            (index + 1) % sizes.len()
        } else {
            (index + sizes.len() - 1) % sizes.len()
        };
        sizes[next]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortOrder {
    Ascending,
    Descending,
}

/// What the grid needs from the server for a remote page.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GridRequest {
    /// 1-based page number.
    pub page: u64,
    pub per_page: u64,
    /// Column name and whether the order is descending.
    pub sort: Option<(String, bool)>,
    pub search: String,
}

/// Outcome of a key press for the screen that owns the grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GridKey {
    /// Not a grid key; the screen may handle it.
    Ignored,
    Handled,
    /// The remote page must be fetched again with [`Grid::request`].
    Reload,
}

#[derive(Debug)]
enum Source {
    /// Every row is held here; search, sort and paging are local.
    Local {
        rows: Vec<GridRow>,
        /// Indices of `rows` that pass the search, in sort order.
        view: Vec<usize>,
        /// Rows the query produced, when more than were stored.
        produced_rows: u64,
    },
    /// The server owns the rows; this holds the current page only.
    Remote {
        page_rows: Vec<GridRow>,
        total_rows: u64,
        loading: bool,
        error: Option<String>,
    },
}

#[derive(Debug)]
pub struct Grid {
    columns: Vec<String>,
    source: Source,
    /// 0-based page index.
    page: usize,
    page_size: PageSize,
    cursor_row: usize,
    cursor_column: usize,
    sort: Option<(usize, SortOrder)>,
    search: String,
    /// Text being typed after `/`, when the search field is open.
    search_input: Option<String>,
    show_value: bool,
    /// Spreadsheet-like lines between rows and columns.
    gridlines: bool,
    /// Page size of the latest remote request; remote pages hold this many rows.
    requested_size: StateCell<usize>,
    // View state updated while rendering, which only has shared access.
    visible_rows: StateCell<usize>,
    first_row: StateCell<usize>,
    first_column: StateCell<usize>,
}

impl Grid {
    /// Grid over rows already in memory; `produced_rows` is the total the
    /// query returned, which may exceed the stored rows.
    pub fn local(columns: Vec<String>, rows: Vec<GridRow>, produced_rows: u64) -> Self {
        let view = (0..rows.len()).collect();
        Self::new(
            columns,
            Source::Local {
                rows,
                view,
                produced_rows,
            },
        )
    }

    /// Grid whose pages come from the server; it starts loading.
    pub fn remote() -> Self {
        Self::new(
            Vec::new(),
            Source::Remote {
                page_rows: Vec::new(),
                total_rows: 0,
                loading: true,
                error: None,
            },
        )
    }

    fn new(columns: Vec<String>, source: Source) -> Self {
        Self {
            columns,
            source,
            page: 0,
            page_size: PageSize::Auto,
            cursor_row: 0,
            cursor_column: 0,
            sort: None,
            search: String::new(),
            search_input: None,
            show_value: false,
            gridlines: true,
            requested_size: StateCell::new(0),
            visible_rows: StateCell::new(20),
            first_row: StateCell::new(0),
            first_column: StateCell::new(0),
        }
    }

    /// Stores a page fetched for the latest [`Grid::request`].
    pub fn set_remote_page(&mut self, columns: Vec<String>, rows: Vec<GridRow>, total_rows: u64) {
        if let Source::Remote {
            page_rows,
            total_rows: total,
            loading,
            error,
        } = &mut self.source
        {
            self.columns = columns;
            *page_rows = rows;
            *total = total_rows;
            *loading = false;
            *error = None;
            self.clamp_cursor();
        }
    }

    pub fn set_remote_error(&mut self, message: String) {
        if let Source::Remote {
            page_rows,
            loading,
            error,
            ..
        } = &mut self.source
        {
            page_rows.clear();
            *loading = false;
            *error = Some(message);
        }
    }

    pub fn mark_loading(&mut self) {
        if let Source::Remote { loading, .. } = &mut self.source {
            *loading = true;
        }
    }

    /// Query for the next remote page; remembers its page size.
    pub fn request(&self) -> GridRequest {
        let per_page = self.wanted_page_size();
        self.requested_size.set(per_page);
        GridRequest {
            page: self.page as u64 + 1,
            per_page: per_page as u64,
            sort: self.sort.and_then(|(column, order)| {
                self.columns
                    .get(column)
                    .map(|name| (name.clone(), order == SortOrder::Descending))
            }),
            search: self.search.clone(),
        }
    }

    /// Rows per page for the current mode and visible area.
    fn wanted_page_size(&self) -> usize {
        match self.page_size {
            PageSize::Auto => self.visible_rows.get().clamp(1, MAX_PAGE_SIZE),
            PageSize::Fixed(size) => size,
        }
    }

    /// Rows per page in effect: remote pages keep the size they were requested with.
    pub fn page_size(&self) -> usize {
        match self.requested_size.get() {
            requested if self.is_remote() && requested > 0 => requested,
            _ => self.wanted_page_size(),
        }
    }

    /// Whether an Auto remote page no longer fits the visible area, after
    /// the first render or a terminal resize.
    pub fn needs_refit(&self) -> bool {
        matches!(
            self.source,
            Source::Remote {
                loading: false,
                error: None,
                ..
            }
        ) && self.page_size == PageSize::Auto
            && self.requested_size.get() != self.wanted_page_size()
    }

    /// Prepares a reload at the fitted size, keeping the first row shown.
    pub fn refit(&mut self) {
        let first_row = self.current_page() * self.page_size();
        self.page = first_row / self.wanted_page_size();
        self.cursor_row = 0;
        self.first_row.set(0);
        self.mark_loading();
    }

    /// Rows matching the search: all stored rows locally, the server count remotely.
    pub fn total_rows(&self) -> usize {
        match &self.source {
            Source::Local { view, .. } => view.len(),
            Source::Remote { total_rows, .. } => usize::try_from(*total_rows).unwrap_or(usize::MAX),
        }
    }

    pub fn page_count(&self) -> usize {
        self.total_rows().div_ceil(self.page_size()).max(1)
    }

    fn current_page(&self) -> usize {
        self.page.min(self.page_count() - 1)
    }

    /// Rows of the current page.
    pub fn page_rows(&self) -> Vec<&GridRow> {
        match &self.source {
            Source::Local { rows, view, .. } => {
                let start = self.current_page() * self.page_size();
                view.iter()
                    .skip(start)
                    .take(self.page_size())
                    .map(|index| &rows[*index])
                    .collect()
            }
            Source::Remote { page_rows, .. } => page_rows.iter().collect(),
        }
    }

    pub fn active_value(&self) -> Option<&str> {
        self.page_rows()
            .get(self.cursor_row)
            .and_then(|row| row.get(self.cursor_column))
            .map(|value| value.as_deref().unwrap_or("NULL"))
    }

    fn is_remote(&self) -> bool {
        matches!(self.source, Source::Remote { .. })
    }

    /// Reacts to a change of page, size, sort or search.
    fn changed(&mut self) -> GridKey {
        self.first_row.set(0);
        if self.is_remote() {
            self.mark_loading();
            GridKey::Reload
        } else {
            self.clamp_cursor();
            GridKey::Handled
        }
    }

    fn go_to_page(&mut self, page: usize) -> GridKey {
        let page = page.min(self.page_count() - 1);
        if page == self.current_page() {
            return GridKey::Handled;
        }
        self.page = page;
        self.cursor_row = 0;
        self.changed()
    }

    fn clamp_cursor(&mut self) {
        self.page = self.current_page();
        let rows = self.page_rows().len();
        self.cursor_row = self.cursor_row.min(rows.saturating_sub(1));
        self.cursor_column = self.cursor_column.min(self.columns.len().saturating_sub(1));
    }

    /// Rebuilds the local view after the search or the sort changed.
    fn rebuild_view(&mut self) {
        let Source::Local { rows, view, .. } = &mut self.source else {
            return;
        };
        let needle = self.search.to_lowercase();
        *view = (0..rows.len())
            .filter(|index| {
                needle.is_empty()
                    || rows[*index]
                        .iter()
                        .flatten()
                        .any(|value| value.to_lowercase().contains(&needle))
            })
            .collect();
        if let Some((column, order)) = self.sort {
            let kind = ValueKind::infer(rows.iter().map(|row| row.get(column).cloned().flatten()));
            view.sort_by(|left, right| {
                compare_values(
                    rows[*left].get(column).and_then(Option::as_deref),
                    rows[*right].get(column).and_then(Option::as_deref),
                    kind,
                    order,
                )
            });
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> GridKey {
        if let Some(input) = &mut self.search_input {
            match key.code {
                KeyCode::Enter => {
                    self.search = input.trim().to_owned();
                    self.search_input = None;
                    self.page = 0;
                    self.cursor_row = 0;
                    self.rebuild_view();
                    return self.changed();
                }
                KeyCode::Esc => {
                    self.search_input = None;
                    if self.search.is_empty() {
                        return GridKey::Handled;
                    }
                    self.search.clear();
                    self.page = 0;
                    self.rebuild_view();
                    return self.changed();
                }
                KeyCode::Backspace => {
                    input.pop();
                }
                KeyCode::Char(character)
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    input.push(character);
                }
                _ => {}
            }
            return GridKey::Handled;
        }
        if self.show_value {
            if matches!(key.code, KeyCode::Esc | KeyCode::Enter) {
                self.show_value = false;
            }
            return GridKey::Handled;
        }

        let rows = self.page_rows().len();
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                if self.cursor_row > 0 {
                    self.cursor_row -= 1;
                } else if self.current_page() > 0 {
                    let previous = self.current_page() - 1;
                    let key = self.go_to_page(previous);
                    self.cursor_row = self.page_size() - 1;
                    self.clamp_cursor_if_local();
                    return key;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.cursor_row + 1 < rows {
                    self.cursor_row += 1;
                } else if self.current_page() + 1 < self.page_count() {
                    return self.go_to_page(self.current_page() + 1);
                }
            }
            KeyCode::Left | KeyCode::Char('h') => {
                self.cursor_column = self.cursor_column.saturating_sub(1);
            }
            KeyCode::Right | KeyCode::Char('l') => {
                if self.cursor_column + 1 < self.columns.len() {
                    self.cursor_column += 1;
                }
            }
            KeyCode::Home => self.cursor_row = 0,
            KeyCode::End => self.cursor_row = rows.saturating_sub(1),
            KeyCode::PageUp | KeyCode::Char('[') => {
                return self.go_to_page(self.current_page().saturating_sub(1));
            }
            KeyCode::PageDown | KeyCode::Char(']') => {
                return self.go_to_page(self.current_page() + 1);
            }
            KeyCode::Char('g') => return self.go_to_page(0),
            KeyCode::Char('G') => return self.go_to_page(usize::MAX),
            KeyCode::Char('+') | KeyCode::Char('=') | KeyCode::Char('-') => {
                self.page_size = self.page_size.cycle(key.code != KeyCode::Char('-'));
                self.page = 0;
                self.cursor_row = 0;
                return self.changed();
            }
            KeyCode::Char('s') if !self.columns.is_empty() => {
                self.sort = match self.sort {
                    Some((column, SortOrder::Ascending)) if column == self.cursor_column => {
                        Some((column, SortOrder::Descending))
                    }
                    Some((column, SortOrder::Descending)) if column == self.cursor_column => None,
                    _ => Some((self.cursor_column, SortOrder::Ascending)),
                };
                self.page = 0;
                self.cursor_row = 0;
                self.rebuild_view();
                return self.changed();
            }
            KeyCode::Char('/') => self.search_input = Some(self.search.clone()),
            KeyCode::Char('L') => {
                self.gridlines = !self.gridlines;
                self.first_row.set(0);
            }
            KeyCode::Enter if rows > 0 => self.show_value = true,
            _ => return GridKey::Ignored,
        }
        GridKey::Handled
    }

    fn clamp_cursor_if_local(&mut self) {
        if !self.is_remote() {
            self.clamp_cursor();
        }
    }

    /// Draws the grid inside a bordered block titled `title`.
    pub fn render(
        &self,
        frame: &mut Frame<'_>,
        area: Rect,
        theme: &Theme,
        title: &str,
        focused: bool,
    ) {
        let border = if focused {
            theme.focus_border()
        } else {
            Style::default()
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(border)
            .title(title.to_owned());
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let show_search = self.search_input.is_some() || !self.search.is_empty();
        let parts = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(u16::from(show_search)),
                Constraint::Min(1),
                Constraint::Length(1),
            ])
            .split(inner);
        if show_search {
            let text = match &self.search_input {
                Some(input) => format!("/ {input}▏   Enter: apply   Esc: clear"),
                None => format!(
                    "Filter: {}   (/ to change, Esc in the field clears)",
                    self.search
                ),
            };
            frame.render_widget(Paragraph::new(text).style(theme.accent()), parts[0]);
        }
        self.render_table(frame, parts[1], theme, focused);
        self.render_footer(frame, parts[2], theme);
        if self.show_value {
            self.render_value(frame, area, theme);
        }
    }

    fn render_table(&self, frame: &mut Frame<'_>, area: Rect, theme: &Theme, focused: bool) {
        if let Source::Remote { loading, error, .. } = &self.source {
            let notice = match (error, loading) {
                (Some(error), _) => Some((error.as_str(), theme.error())),
                (None, true) if self.columns.is_empty() => Some(("Loading rows…", theme.muted())),
                _ => None,
            };
            if let Some((text, style)) = notice {
                frame.render_widget(Paragraph::new(text).style(style), area);
                return;
            }
        }
        if self.columns.is_empty() {
            frame.render_widget(
                Paragraph::new("The last statement returned no rows.").style(theme.muted()),
                area,
            );
            return;
        }

        // Horizontal grid lines are an underline on each row, so a record keeps
        // a single screen line and the line spans the whole table width.
        let body_rows = usize::from(area.height).saturating_sub(1).max(1);
        self.visible_rows.set(body_rows);
        let page_rows = self.page_rows();
        let first_row = keep_visible(self.first_row.get(), self.cursor_row, body_rows);
        self.first_row.set(first_row);

        let row_offset = self.current_page() * self.page_size();
        let number_width = (row_offset + page_rows.len()).max(1).to_string().len();
        let available = usize::from(area.width).saturating_sub(number_width + 1);
        let widths: Vec<usize> = (0..self.columns.len())
            .map(|column| self.column_width(column, &page_rows))
            .collect();
        let first_column = first_visible_column(
            self.first_column.get(),
            self.cursor_column,
            &widths,
            available,
        );
        self.first_column.set(first_column);
        let mut shown = Vec::new();
        let mut used = 0;
        for (column, width) in widths.iter().enumerate().skip(first_column) {
            if !shown.is_empty() && used + width > available {
                break;
            }
            used += width + 1;
            shown.push(column);
        }

        // Table columns: number, then a 1-cell separator before each shown column.
        let mut cell_widths = vec![number_width];
        for column in &shown {
            cell_widths.extend([1, widths[*column]]);
        }
        let vertical = if self.gridlines { "│" } else { " " };
        let with_separators = |cells: Vec<Cell<'static>>| -> Vec<Cell<'static>> {
            let mut cells = cells.into_iter();
            let mut joined: Vec<Cell<'static>> = cells.next().into_iter().collect();
            for cell in cells {
                joined.push(Cell::from(vertical).style(theme.muted()));
                joined.push(cell);
            }
            joined
        };

        let header_names = std::iter::once("#".to_owned()).chain(shown.iter().map(|column| {
            let marker = match self.sort {
                Some((sorted, SortOrder::Ascending)) if sorted == *column => " ▲",
                Some((sorted, SortOrder::Descending)) if sorted == *column => " ▼",
                _ => "",
            };
            format!("{}{marker}", self.columns[*column])
        }));
        let line_style = if self.gridlines {
            theme.grid_line()
        } else {
            Style::default()
        };
        let header_cells = header_names
            .map(|name| Cell::from(name).style(Style::default().add_modifier(Modifier::BOLD)))
            .collect();
        let header = Row::new(with_separators(header_cells)).style(line_style);
        let mut body = Vec::with_capacity(page_rows.len());
        for (index, row) in page_rows.iter().enumerate() {
            let number = Cell::from(format!("{:>number_width$}", row_offset + index + 1))
                .style(theme.muted());
            let values = std::iter::once(number)
                .chain(shown.iter().map(
                    |column| match row.get(*column).and_then(Option::as_deref) {
                        Some(text) => Cell::from(truncate(text, widths[*column])),
                        None => Cell::from("NULL").style(theme.muted()),
                    },
                ))
                .collect();
            body.push(Row::new(with_separators(values)).style(line_style));
        }
        let constraints: Vec<Constraint> = cell_widths
            .iter()
            .map(|width| Constraint::Length(u16::try_from(*width).unwrap_or(u16::MAX)))
            .collect();
        let mut table = Table::new(body, constraints)
            .header(header)
            .column_spacing(0);
        let mut state = TableState::default().with_offset(first_row);
        if focused && !page_rows.is_empty() {
            table = table
                .row_highlight_style(theme.selection())
                .cell_highlight_style(theme.active_cell());
            let cell_column = shown
                .iter()
                .position(|column| *column == self.cursor_column)
                .map(|position| 2 + 2 * position);
            state = state.with_selected(Some(self.cursor_row));
            state = state.with_selected_column(cell_column);
        }
        frame.render_stateful_widget(table, area, &mut state);
    }

    fn column_width(&self, column: usize, rows: &[&GridRow]) -> usize {
        let header = self.columns[column].chars().count() + 2;
        let content = rows
            .iter()
            .filter_map(|row| row.get(column))
            .map(|value| value.as_deref().map_or(4, |text| text.chars().count()))
            .max()
            .unwrap_or(0);
        header
            .max(content)
            .clamp(MIN_COLUMN_WIDTH, MAX_COLUMN_WIDTH)
    }

    fn render_footer(&self, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
        let total = self.total_rows();
        let page_rows = self.page_rows().len();
        let start = self.current_page() * self.page_size();
        let range = if page_rows == 0 {
            "no rows".to_owned()
        } else {
            format!("rows {}–{} of {total}", start + 1, start + page_rows)
        };
        let mut status = vec![range];
        if let Source::Local {
            rows,
            produced_rows,
            ..
        } = &self.source
            && *produced_rows > rows.len() as u64
        {
            status.push(format!("first {} of {produced_rows} stored", rows.len()));
        }
        if !self.search.is_empty() {
            status.push(format!("filter: {}", self.search));
        }
        if let Some((column, order)) = self.sort {
            let arrow = if order == SortOrder::Ascending {
                "▲"
            } else {
                "▼"
            };
            status.push(format!("sort: {} {arrow}", self.columns[column]));
        }
        if matches!(self.source, Source::Remote { loading: true, .. }) {
            status.push("loading…".to_owned());
        }
        let size = match self.page_size {
            PageSize::Auto => format!("auto ({})", self.page_size()),
            PageSize::Fixed(size) => size.to_string(),
        };
        status.push(format!("{size}/page"));

        let pager = pager_spans(self.current_page(), self.page_count(), theme);
        let pager_width: usize = pager.iter().map(|span| span.content.chars().count()).sum();
        let parts = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Min(1),
                Constraint::Length(u16::try_from(pager_width).unwrap_or(u16::MAX)),
            ])
            .split(area);
        frame.render_widget(
            Paragraph::new(status.join(" · ")).style(theme.muted()),
            parts[0],
        );
        frame.render_widget(Paragraph::new(Line::from(pager)), parts[1]);
    }

    fn render_value(&self, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
        let Some(value) = self.active_value() else {
            return;
        };
        let column = self
            .columns
            .get(self.cursor_column)
            .map_or("value", String::as_str);
        let width = area.width.saturating_mul(4) / 5;
        let height = area.height.saturating_mul(3) / 4;
        let popup = Rect {
            x: area.x + (area.width - width) / 2,
            y: area.y + (area.height - height) / 2,
            width,
            height,
        };
        frame.render_widget(Clear, popup);
        frame.render_widget(
            Paragraph::new(value.to_owned())
                .wrap(Wrap { trim: false })
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(theme.focus_border())
                        .style(theme.surface())
                        .title(format!(" {column} — Esc to close ")),
                ),
            popup,
        );
    }
}

/// Pager such as `« ‹ 1 … 4 [5] 6 … 40 › »`; arrows are dimmed at the ends.
fn pager_spans(current: usize, count: usize, theme: &Theme) -> Vec<Span<'static>> {
    let has_previous = current > 0;
    let has_next = current + 1 < count;
    let arrow = |text: &'static str, enabled: bool| {
        Span::styled(
            text,
            if enabled {
                theme.accent()
            } else {
                theme.muted()
            },
        )
    };
    let mut spans = vec![arrow("« ", has_previous), arrow("‹ ", has_previous)];
    let first = current.saturating_sub(PAGER_NEIGHBOURS);
    let last = (current + PAGER_NEIGHBOURS).min(count - 1);
    if first > 0 {
        spans.push(Span::styled("1 ", theme.muted()));
        if first > 1 {
            spans.push(Span::styled("… ", theme.muted()));
        }
    }
    for page in first..=last {
        if page == current {
            spans.push(Span::styled(format!("[{}]", page + 1), theme.selection()));
            spans.push(Span::raw(" "));
        } else {
            spans.push(Span::styled(format!("{} ", page + 1), theme.muted()));
        }
    }
    if last + 1 < count {
        if last + 2 < count {
            spans.push(Span::styled("… ", theme.muted()));
        }
        spans.push(Span::styled(format!("{count} "), theme.muted()));
    }
    spans.push(arrow("› ", has_next));
    spans.push(arrow("»", has_next));
    spans
}

fn keep_visible(first: usize, position: usize, size: usize) -> usize {
    if position < first {
        position
    } else if position >= first + size {
        position + 1 - size
    } else {
        first
    }
}

/// First column to draw so that `cursor` fits within `available` cells.
fn first_visible_column(first: usize, cursor: usize, widths: &[usize], available: usize) -> usize {
    if cursor < first {
        return cursor;
    }
    let fits = |start: usize| {
        widths[start..=cursor]
            .iter()
            .map(|width| width + 1)
            .sum::<usize>()
            <= available
            || start == cursor
    };
    (first..=cursor)
        .find(|start| fits(*start))
        .unwrap_or(cursor)
}

fn truncate(text: &str, width: usize) -> String {
    let single_line = text.replace(['\n', '\r', '\t'], " ");
    if single_line.chars().count() <= width {
        single_line
    } else {
        let mut shortened: String = single_line.chars().take(width.saturating_sub(1)).collect();
        shortened.push('…');
        shortened
    }
}

/// How a local column is compared when sorting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ValueKind {
    Number,
    /// ISO dates and timestamps, which order correctly as text.
    Date,
    Text,
}

impl ValueKind {
    fn infer(values: impl Iterator<Item = Option<String>>) -> Self {
        let mut number = true;
        let mut date = true;
        let mut any = false;
        for value in values.flatten() {
            any = true;
            number &= value.trim().parse::<f64>().is_ok();
            date &= is_iso_date(&value);
            if !number && !date {
                return Self::Text;
            }
        }
        match (any, number, date) {
            (false, _, _) => Self::Text,
            (true, true, _) => Self::Number,
            (true, false, true) => Self::Date,
            _ => Self::Text,
        }
    }
}

fn is_iso_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 10
        && bytes[..4].iter().all(u8::is_ascii_digit)
        && bytes[4] == b'-'
        && bytes[5..7].iter().all(u8::is_ascii_digit)
        && bytes[7] == b'-'
        && bytes[8..10].iter().all(u8::is_ascii_digit)
}

/// Orders two values of a column; nulls always come last.
fn compare_values(
    left: Option<&str>,
    right: Option<&str>,
    kind: ValueKind,
    order: SortOrder,
) -> Ordering {
    let (left, right) = match (left, right) {
        (None, None) => return Ordering::Equal,
        (None, Some(_)) => return Ordering::Greater,
        (Some(_), None) => return Ordering::Less,
        (Some(left), Some(right)) => (left, right),
    };
    let ordering = match kind {
        ValueKind::Number => {
            let parse = |value: &str| value.trim().parse::<f64>().unwrap_or(f64::NAN);
            parse(left).total_cmp(&parse(right))
        }
        ValueKind::Date => left.cmp(right),
        ValueKind::Text => left
            .to_lowercase()
            .cmp(&right.to_lowercase())
            .then_with(|| left.cmp(right)),
    };
    match order {
        SortOrder::Ascending => ordering,
        SortOrder::Descending => ordering.reverse(),
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::{Terminal, backend::TestBackend, style::Modifier};

    use super::{
        Grid, GridKey, GridRequest, GridRow, PageSize, SortOrder, ValueKind, compare_values,
    };
    use crate::theme::Theme;

    fn press(grid: &mut Grid, code: KeyCode) -> GridKey {
        grid.handle_key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn type_text(grid: &mut Grid, text: &str) {
        for character in text.chars() {
            press(grid, KeyCode::Char(character));
        }
    }

    fn rows(count: usize) -> Vec<GridRow> {
        (0..count)
            .map(|index| {
                vec![
                    Some(index.to_string()),
                    if index % 3 == 0 {
                        None
                    } else {
                        Some(format!("name {index}"))
                    },
                ]
            })
            .collect()
    }

    fn local(count: usize) -> Grid {
        let mut grid = Grid::local(
            vec!["id".to_owned(), "name".to_owned()],
            rows(count),
            count as u64,
        );
        grid.page_size = PageSize::Fixed(10);
        grid
    }

    fn first_ids(grid: &Grid) -> Vec<String> {
        grid.page_rows()
            .iter()
            .map(|row| row[0].clone().unwrap_or_default())
            .collect()
    }

    fn render(grid: &Grid, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
        terminal
            .draw(|frame| grid.render(frame, frame.area(), &Theme::default(), " Result ", true))
            .expect("render grid");
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn page_sizes_cycle_through_auto_and_fixed_sizes() {
        let order = [
            PageSize::Auto,
            PageSize::Fixed(10),
            PageSize::Fixed(25),
            PageSize::Fixed(50),
            PageSize::Fixed(100),
        ];
        for (index, size) in order.iter().enumerate() {
            assert_eq!(size.cycle(true), order[(index + 1) % order.len()]);
            assert_eq!(
                size.cycle(false),
                order[(index + order.len() - 1) % order.len()]
            );
        }
    }

    #[test]
    fn local_paging_moves_between_pages_and_clamps_the_cursor() {
        let mut grid = local(25);
        assert_eq!(grid.page_count(), 3);
        assert_eq!(press(&mut grid, KeyCode::Char(']')), GridKey::Handled);
        assert_eq!(first_ids(&grid)[0], "10");
        assert_eq!(press(&mut grid, KeyCode::Char('G')), GridKey::Handled);
        assert_eq!(first_ids(&grid), ["20", "21", "22", "23", "24"]);
        press(&mut grid, KeyCode::End);
        assert_eq!(grid.cursor_row, 4);
        press(&mut grid, KeyCode::Down);
        assert_eq!((grid.page, grid.cursor_row), (2, 4), "last row stays put");
        press(&mut grid, KeyCode::Char('g'));
        assert_eq!((grid.page, grid.cursor_row), (0, 0));
        for _ in 0..10 {
            press(&mut grid, KeyCode::Down);
        }
        assert_eq!(
            (grid.page, grid.cursor_row),
            (1, 0),
            "down past the page turns it"
        );
        press(&mut grid, KeyCode::Up);
        assert_eq!(
            (grid.page, grid.cursor_row),
            (0, 9),
            "up from the top turns back"
        );
        press(&mut grid, KeyCode::Char('+'));
        assert_eq!(grid.page_size, PageSize::Fixed(25));
        assert_eq!((grid.page, grid.page_count()), (0, 1));
    }

    #[test]
    fn local_search_filters_rows_case_insensitively() {
        let mut grid = local(25);
        press(&mut grid, KeyCode::Char('/'));
        type_text(&mut grid, "NAME 1");
        assert_eq!(press(&mut grid, KeyCode::Enter), GridKey::Handled);
        assert_eq!(
            first_ids(&grid),
            ["1", "10", "11", "13", "14", "16", "17", "19"]
        );
        assert_eq!(grid.total_rows(), 8);
        press(&mut grid, KeyCode::Char('/'));
        press(&mut grid, KeyCode::Esc);
        assert_eq!(grid.total_rows(), 25, "Esc in the field clears the filter");
        assert_eq!(
            press(&mut grid, KeyCode::Esc),
            GridKey::Ignored,
            "Esc is free otherwise"
        );
    }

    #[test]
    fn local_sort_cycles_and_uses_the_inferred_type() {
        let mut grid = local(12);
        assert_eq!(press(&mut grid, KeyCode::Char('s')), GridKey::Handled);
        assert_eq!(grid.sort, Some((0, SortOrder::Ascending)));
        assert_eq!(
            first_ids(&grid)[..3],
            ["0", "1", "2"],
            "numeric, not 0,1,10,11"
        );
        press(&mut grid, KeyCode::Char('s'));
        assert_eq!(first_ids(&grid)[..3], ["11", "10", "9"]);
        press(&mut grid, KeyCode::Char('s'));
        assert_eq!(grid.sort, None);

        press(&mut grid, KeyCode::Right);
        press(&mut grid, KeyCode::Char('s'));
        press(&mut grid, KeyCode::Char('G'));
        let last_page: Vec<_> = grid.page_rows().iter().map(|row| row[1].clone()).collect();
        assert_eq!(last_page, [None, None], "nulls last");
        press(&mut grid, KeyCode::Char('s'));
        press(&mut grid, KeyCode::Char('G'));
        let last_page: Vec<_> = grid.page_rows().iter().map(|row| row[1].clone()).collect();
        assert_eq!(last_page, [None, None], "nulls last when descending");
    }

    #[test]
    fn value_kinds_compare_numbers_dates_and_text() {
        let infer = |values: &[Option<&str>]| {
            ValueKind::infer(values.iter().map(|value| value.map(str::to_owned)))
        };
        assert_eq!(infer(&[Some("10"), Some("-2.5"), None]), ValueKind::Number);
        assert_eq!(
            infer(&[Some("2026-01-02"), Some("2025-12-31 23:00:00+00")]),
            ValueKind::Date
        );
        assert_eq!(infer(&[Some("10"), Some("abc")]), ValueKind::Text);
        assert_eq!(infer(&[None]), ValueKind::Text);
        let compare = |left, right, kind| compare_values(left, right, kind, SortOrder::Ascending);
        assert!(compare(Some("9"), Some("10"), ValueKind::Number).is_lt());
        assert!(compare(Some("b"), Some("A"), ValueKind::Text).is_gt());
        assert!(compare(None, Some("a"), ValueKind::Text).is_gt());
        assert!(compare_values(None, Some("a"), ValueKind::Text, SortOrder::Descending).is_gt());
    }

    #[test]
    fn remote_changes_request_a_reload_with_the_new_query() {
        let mut grid = Grid::remote();
        grid.page_size = PageSize::Fixed(25);
        grid.set_remote_page(vec!["id".to_owned(), "name".to_owned()], rows(25), 100);
        assert_eq!(grid.page_count(), 4);
        assert_eq!(press(&mut grid, KeyCode::Down), GridKey::Handled);
        assert_eq!(press(&mut grid, KeyCode::Char(']')), GridKey::Reload);
        press(&mut grid, KeyCode::Right);
        assert_eq!(press(&mut grid, KeyCode::Char('s')), GridKey::Reload);
        press(&mut grid, KeyCode::Char('/'));
        type_text(&mut grid, "ana");
        assert_eq!(press(&mut grid, KeyCode::Enter), GridKey::Reload);
        assert_eq!(
            grid.request(),
            GridRequest {
                page: 1,
                per_page: 25,
                sort: Some(("name".to_owned(), false)),
                search: "ana".to_owned(),
            }
        );
        grid.set_remote_error("Could not read rows.".to_owned());
        assert!(render(&grid, 80, 12).contains("Could not read rows."));
    }

    #[test]
    fn rendering_shows_numbers_markers_footer_and_value_popup() {
        let mut grid = Grid::local(
            vec!["id".to_owned(), "description".to_owned()],
            vec![
                vec![Some("1".to_owned()), Some("x".repeat(60))],
                vec![Some("2".to_owned()), None],
            ],
            1500,
        );
        press(&mut grid, KeyCode::Right);
        press(&mut grid, KeyCode::Char('s'));
        let screen = render(&grid, 90, 12);
        assert!(screen.contains("description ▲"));
        assert!(
            screen.contains(&format!("{}…", "x".repeat(39))),
            "long values truncated"
        );
        assert!(screen.contains("NULL"));
        assert!(screen.contains("rows 1–2 of 2"));
        assert!(screen.contains("first 2 of 1500 stored"));
        assert!(screen.contains("auto ("));
        assert!(screen.contains("[1]"));
        press(&mut grid, KeyCode::Enter);
        let popup = render(&grid, 90, 12);
        assert!(popup.contains("description — Esc to close"));
        assert_eq!(press(&mut grid, KeyCode::Esc), GridKey::Handled);
        assert!(!grid.show_value);
    }

    #[test]
    fn pager_marks_the_current_page_and_elides_distant_ones() {
        let theme = Theme::default();
        let text = |current, count| {
            super::pager_spans(current, count, &theme)
                .iter()
                .map(|span| span.content.to_string())
                .collect::<String>()
        };
        assert_eq!(text(0, 1), "« ‹ [1] › »");
        assert_eq!(text(4, 40), "« ‹ 1 … 3 4 [5] 6 7 … 40 › »");
        assert_eq!(text(39, 40), "« ‹ 1 … 38 39 [40] › »");
    }

    #[test]
    fn remote_auto_pages_refit_to_the_visible_rows() {
        let mut grid = Grid::remote();
        grid.gridlines = false;
        let first = grid.request();
        assert_eq!(first.per_page, 20, "before any render");
        grid.set_remote_page(vec!["id".to_owned()], rows(20), 120);
        assert_eq!(grid.page_count(), 6, "pages use the requested size");
        assert!(!grid.needs_refit(), "nothing rendered yet");

        render(&grid, 40, 12);
        assert!(grid.needs_refit());
        assert_eq!(grid.page_rows().len(), 20, "still the loaded page");
        assert_eq!(grid.page_size(), 20);
        grid.page = 2;
        grid.refit();
        let refit = grid.request();
        assert_eq!(refit.per_page, 8);
        assert_eq!(refit.page, 6, "row 41 stays on screen");
        grid.set_remote_page(vec!["id".to_owned()], rows(8), 120);
        assert!(!grid.needs_refit());
        assert_eq!(grid.page_count(), 15);

        grid.page_size = PageSize::Fixed(25);
        render(&grid, 40, 12);
        assert!(
            !grid.needs_refit(),
            "fixed sizes scroll instead of refitting"
        );
    }

    #[test]
    fn auto_page_size_follows_the_visible_rows() {
        let mut grid = Grid::local(vec!["id".to_owned()], rows(50), 50);
        grid.gridlines = false;
        render(&grid, 40, 12);
        let visible = grid.page_size();
        assert!((5..12).contains(&visible), "{visible}");
        assert_eq!(grid.page_rows().len(), visible);
        press(&mut grid, KeyCode::Char('G'));
        assert_eq!(grid.page, grid.page_count() - 1);
    }

    #[test]
    fn grid_lines_separate_rows_and_columns_and_can_be_toggled() {
        let mut grid = local(30);
        grid.page_size = PageSize::Auto;
        let (screen, underlined) = render_with_underlines(&grid, 60, 20);
        assert!(screen.contains("# │id  │name"), "{screen}");
        assert!(screen.contains(" 1│0   │NULL"));
        assert!(!screen.contains('┼'), "lines do not take screen rows");
        assert_eq!(grid.page_size(), 16, "one screen line per record");
        // Header (row 1, inside the border) and records are underlined across
        // the whole inner width.
        for row in [1, 2, 17] {
            assert!(
                (1..59).all(|column| underlined[row * 60 + column]),
                "row {row} underlined edge to edge"
            );
        }

        assert_eq!(press(&mut grid, KeyCode::Char('L')), GridKey::Handled);
        let (screen, underlined) = render_with_underlines(&grid, 60, 20);
        assert!(screen.contains("#  id   name"), "{screen}");
        assert!(screen.contains(" 1 0    NULL"));
        assert!(!underlined[2 * 60 + 5], "no underline without lines");
        assert_eq!(grid.page_size(), 16);
    }

    fn render_with_underlines(grid: &Grid, width: u16, height: u16) -> (String, Vec<bool>) {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
        terminal
            .draw(|frame| grid.render(frame, frame.area(), &Theme::default(), " Result ", false))
            .expect("render grid");
        let cells = &terminal.backend().buffer().content;
        (
            cells.iter().map(|cell| cell.symbol()).collect(),
            cells
                .iter()
                .map(|cell| cell.modifier.contains(Modifier::UNDERLINED))
                .collect(),
        )
    }
}
