//! What the database browser screen is holding, and how it reads.
//!
//! State and presentation ONLY — the commands sent to an engine and the parsing
//! of what it answers live in `crate::dbms`, where they are testable without a
//! TUI at all. Same split `BackupUi` / `crate::backup` uses.

use ratatui::widgets::TableState;

/// Which step of the walk is on screen.
///
/// `Query` is not a deeper level: it is a result that replaces the grid, and Esc
/// from it goes back to the level it was run from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Level {
    Databases,
    Tables,
    Rows,
    Query,
}

/// The database browser's state: what it is pointed at, what came back, and
/// what is wrong with it.
pub(super) struct DbmsUi {
    /// The service being browsed: (project, service, type). None = closed.
    pub(super) target: Option<(String, String, String)>,
    pub(super) level: Level,
    /// The database (mongo: the db) currently open, empty at the top level.
    pub(super) database: String,
    /// The table/collection currently previewed, empty above that level.
    pub(super) table: String,
    pub(super) columns: Vec<String>,
    pub(super) rows: Vec<Vec<String>>,
    pub(super) row: TableState,
    /// The engine's own complaint. Kept ALONGSIDE the grid rather than replacing
    /// it: a failed query must not also erase what you were looking at, and an
    /// empty grid on its own can never be the only sign that something broke.
    pub(super) error: Option<String>,
    /// The preview hit its LIMIT — there are more rows than these.
    pub(super) capped: bool,
    /// The captured output was cut at the byte budget.
    pub(super) truncated: bool,
    /// Waiting for the worker. Distinguishes "nothing yet" from "nothing".
    pub(super) loading: bool,
    /// The last thing typed into the query box, so reopening it does not start
    /// from a blank line.
    pub(super) last_query: String,
}

impl Default for DbmsUi {
    fn default() -> Self {
        Self {
            target: None,
            level: Level::Databases,
            database: String::new(),
            table: String::new(),
            columns: Vec::new(),
            rows: Vec::new(),
            row: TableState::default(),
            error: None,
            capped: false,
            truncated: false,
            loading: false,
            last_query: String::new(),
        }
    }
}

impl DbmsUi {
    pub(super) fn engine(&self) -> Option<crate::dbms::Engine> {
        self.target
            .as_ref()
            .and_then(|(_, _, t)| crate::dbms::Engine::from_service_type(t))
    }

    /// The value in the first column of the selected row — the database or table
    /// a drill-down acts on. Read from the DATA, never parsed back off the
    /// screen.
    pub(super) fn selected_name(&self) -> Option<String> {
        self.row
            .selected()
            .and_then(|i| self.rows.get(i))
            .and_then(|r| r.first())
            .cloned()
    }

    /// Where we are, as the pane title says it: service, engine, and the path
    /// walked so far, plus how much of the data is actually shown.
    pub(super) fn title(&self) -> String {
        let (project, service, stype) = match &self.target {
            Some(t) => t,
            None => return " Database ".into(),
        };
        let mut path = format!(" {project}/{service} · {stype}");
        if !self.database.is_empty() {
            path.push_str(&format!(" · {}", self.database));
        }
        if !self.table.is_empty() {
            path.push_str(&format!(" · {}", self.table));
        }
        if self.level == Level::Query {
            path.push_str(" · query");
        }
        format!("{path} · {} ", self.count_note())
    }

    /// How many rows are shown, and whether that is all of them. A cap the user
    /// cannot see is a lie about how much data is there.
    pub(super) fn count_note(&self) -> String {
        if self.loading {
            return "loading…".into();
        }
        let n = self.rows.len();
        let mut note = match (self.level, n) {
            (Level::Databases, _) => format!("{n} databases"),
            (Level::Tables, _) => format!("{n} tables"),
            (_, 0) => "0 rows".to_string(),
            (_, n) => format!("{n} rows"),
        };
        if self.capped {
            note.push_str(&format!(
                " (the {}-row cap — there is more)",
                crate::dbms::PREVIEW_LIMIT
            ));
        }
        if self.truncated {
            note.push_str(" · output cut");
        }
        note
    }

    /// The keys this level offers, written on the pane's bottom border — so the
    /// hint cannot claim an action the level does not have.
    pub(super) fn hint(&self) -> &'static str {
        match self.level {
            Level::Databases => " Enter open · e query · r refresh · Esc back ",
            Level::Tables => " Enter preview rows · e query · r refresh · Esc databases ",
            Level::Rows => " e query · r refresh · Esc tables ",
            Level::Query => " e query again · Esc back ",
        }
    }

    /// Take one level back, returning the level to fetch — or None when there is
    /// nothing above this and the screen should close.
    pub(super) fn back(&mut self) -> Option<Level> {
        match self.level {
            Level::Databases => None,
            Level::Tables => {
                self.database.clear();
                self.table.clear();
                Some(Level::Databases)
            }
            // A query was run FROM a level; Esc returns to the table list it was
            // reachable from — unless no database was ever opened (a server-wide
            // query, run straight from the database list), in which case the
            // table list would have nothing to list.
            Level::Rows | Level::Query => {
                self.table.clear();
                if self.database.is_empty() {
                    Some(Level::Databases)
                } else {
                    Some(Level::Tables)
                }
            }
        }
    }

    /// Adopt a result: the grid, where it belongs, and what is imperfect about it.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn show(
        &mut self,
        level: Level,
        database: String,
        table: String,
        columns: Vec<String>,
        rows: Vec<Vec<String>>,
        capped: bool,
        truncated: bool,
    ) {
        self.level = level;
        self.database = database;
        self.table = table;
        self.columns = columns;
        self.rows = rows;
        self.capped = capped;
        self.truncated = truncated;
        self.loading = false;
        self.error = None;
        self.row = TableState::default();
        super::table::select_first(&mut self.row, self.rows.len());
    }
}
