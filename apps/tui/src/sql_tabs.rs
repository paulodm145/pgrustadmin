//! In-memory SQL tab state for the TUI session.

use uuid::Uuid;

use crate::sql_editor::SqlEditor;

pub const MAX_SQL_TABS: usize = 20;

/// Session-unique tab identifier; never reused after a tab closes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TabId(u64);

impl TabId {
    pub const fn get(self) -> u64 {
        self.0
    }
}

pub struct SqlTab {
    pub id: TabId,
    pub editor: SqlEditor,
    /// Content of the last saved state; empty until files exist (SQL-003).
    pub saved_sql: String,
    pub file: Option<String>,
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "identifies the bound connection for SQL execution (SQL-004)"
        )
    )]
    pub connection_id: Uuid,
    pub connection_name: String,
    pub database: String,
}

impl SqlTab {
    /// A tab is dirty while its SQL differs from the last saved content.
    pub fn is_dirty(&self) -> bool {
        self.editor.text() != self.saved_sql
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct TabLimitReached;

#[derive(Default)]
pub struct SqlTabs {
    tabs: Vec<SqlTab>,
    active: usize,
    last_id: u64,
}

impl SqlTabs {
    /// Opens an empty tab bound to `connection_id`/`database` and activates it.
    pub fn open(
        &mut self,
        connection_id: Uuid,
        connection_name: &str,
        database: &str,
    ) -> Result<TabId, TabLimitReached> {
        if self.tabs.len() >= MAX_SQL_TABS {
            return Err(TabLimitReached);
        }
        self.last_id += 1;
        let id = TabId(self.last_id);
        self.tabs.push(SqlTab {
            id,
            editor: SqlEditor::default(),
            saved_sql: String::new(),
            file: None,
            connection_id,
            connection_name: connection_name.to_owned(),
            database: database.to_owned(),
        });
        self.active = self.tabs.len() - 1;
        Ok(id)
    }

    /// Closes the active tab, activating the next one (or the previous one
    /// when the closed tab was last).
    pub fn close_active(&mut self) -> Option<SqlTab> {
        if self.tabs.is_empty() {
            return None;
        }
        let closed = self.tabs.remove(self.active);
        self.active = self.active.min(self.tabs.len().saturating_sub(1));
        Some(closed)
    }

    pub fn select_next(&mut self) {
        if !self.tabs.is_empty() {
            self.active = (self.active + 1) % self.tabs.len();
        }
    }

    pub fn select_previous(&mut self) {
        if !self.tabs.is_empty() {
            self.active = (self.active + self.tabs.len() - 1) % self.tabs.len();
        }
    }

    pub fn active(&self) -> Option<&SqlTab> {
        self.tabs.get(self.active)
    }

    pub fn active_mut(&mut self) -> Option<&mut SqlTab> {
        self.tabs.get_mut(self.active)
    }

    pub fn dirty_count(&self) -> usize {
        self.tabs.iter().filter(|tab| tab.is_dirty()).count()
    }

    pub fn active_index(&self) -> usize {
        self.active
    }

    pub fn tabs(&self) -> &[SqlTab] {
        &self.tabs
    }

    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::{MAX_SQL_TABS, SqlTabs, TabLimitReached};

    #[test]
    fn opening_creates_empty_bound_tab_and_activates_it() {
        let mut tabs = SqlTabs::default();
        let connection_id = Uuid::new_v4();
        tabs.open(connection_id, "local", "app")
            .expect("open first tab");
        let second = tabs
            .open(connection_id, "local", "analytics")
            .expect("open second tab");

        let active = tabs.active().expect("active tab");
        assert_eq!(active.id, second);
        assert_eq!(tabs.active_index(), 1);
        assert_eq!(active.connection_id, connection_id);
        assert_eq!(active.connection_name, "local");
        assert_eq!(active.database, "analytics");
        assert!(active.editor.text().is_empty());
        assert!(active.file.is_none());
        assert!(!active.is_dirty());
    }

    #[test]
    fn ids_are_never_reused_after_closing() {
        let mut tabs = SqlTabs::default();
        let first = tabs.open(Uuid::new_v4(), "local", "app").expect("open tab");
        tabs.close_active().expect("close tab");
        let second = tabs.open(Uuid::new_v4(), "local", "app").expect("open tab");
        assert_ne!(first, second);
        assert!(second.get() > first.get());
    }

    #[test]
    fn refuses_tabs_beyond_the_limit_without_changing_state() {
        let mut tabs = SqlTabs::default();
        for _ in 0..MAX_SQL_TABS {
            tabs.open(Uuid::new_v4(), "local", "app").expect("open tab");
        }
        tabs.select_next();
        assert_eq!(
            tabs.open(Uuid::new_v4(), "local", "app"),
            Err(TabLimitReached)
        );
        assert_eq!(tabs.tabs().len(), MAX_SQL_TABS);
        assert_eq!(tabs.active_index(), 0);
    }

    #[test]
    fn closing_activates_next_tab_or_previous_when_last() {
        let mut tabs = SqlTabs::default();
        let connection_id = Uuid::new_v4();
        let first = tabs.open(connection_id, "local", "a").expect("open tab");
        let second = tabs.open(connection_id, "local", "b").expect("open tab");
        let third = tabs.open(connection_id, "local", "c").expect("open tab");

        tabs.select_next();
        tabs.select_next();
        assert_eq!(tabs.active().map(|tab| tab.id), Some(second));
        assert_eq!(tabs.close_active().map(|tab| tab.id), Some(second));
        assert_eq!(tabs.active().map(|tab| tab.id), Some(third));

        assert_eq!(tabs.close_active().map(|tab| tab.id), Some(third));
        assert_eq!(tabs.active().map(|tab| tab.id), Some(first));

        assert_eq!(tabs.close_active().map(|tab| tab.id), Some(first));
        assert!(tabs.is_empty());
        assert!(tabs.active().is_none());
        assert!(tabs.close_active().is_none());
    }

    #[test]
    fn switching_tabs_wraps_around() {
        let mut tabs = SqlTabs::default();
        tabs.select_next();
        tabs.select_previous();
        assert!(tabs.active().is_none());

        let connection_id = Uuid::new_v4();
        let first = tabs.open(connection_id, "local", "a").expect("open tab");
        let second = tabs.open(connection_id, "local", "b").expect("open tab");
        tabs.select_next();
        assert_eq!(tabs.active().map(|tab| tab.id), Some(first));
        tabs.select_previous();
        assert_eq!(tabs.active().map(|tab| tab.id), Some(second));
    }

    #[test]
    fn dirty_tracks_difference_from_saved_content() {
        let mut tabs = SqlTabs::default();
        let connection_id = Uuid::new_v4();
        tabs.open(connection_id, "local", "a").expect("open tab");
        tabs.open(connection_id, "local", "b").expect("open tab");
        assert_eq!(tabs.dirty_count(), 0);

        let tab = tabs.active_mut().expect("active tab");
        tab.editor.insert_char('x');
        assert!(tab.is_dirty());
        assert_eq!(tabs.dirty_count(), 1);

        let tab = tabs.active_mut().expect("active tab");
        tab.editor.backspace();
        assert!(!tab.is_dirty());
        assert_eq!(tabs.dirty_count(), 0);
    }
}
