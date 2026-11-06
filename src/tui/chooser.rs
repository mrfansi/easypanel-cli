use ratatui::layout::Rect;
use ratatui::widgets::ListState;

/// A dropdown for a Choice field: the list of options + a type-to-filter box.
///
/// Cycling options with space doesn't scale to long lists (11 services), so a
/// Choice field opens a real, searchable list.
pub(super) struct Chooser {
    pub(super) field: usize,
    pub(super) label: &'static str,
    pub(super) options: Vec<String>,
    pub(super) filter: String,
    pub(super) state: ListState,
    /// The dropdown box as drawn (filled in at render), for click/hover hit-testing.
    pub(super) rect: Rect,
    /// Ticking mode: Space ticks a row and Enter takes every ticked one, instead
    /// of Enter taking the single highlighted row.
    ///
    /// The same widget either way. A second list overlay with its own keys would
    /// have been a second thing to learn for the same idea, and the ticks read
    /// exactly like the backup picker's (`BackupUi::picker_lines`) — ✓ inside the
    /// row, so ticking one does not shift the names sideways.
    pub(super) multi: bool,
    /// The ticked options, when `multi`.
    pub(super) marked: std::collections::HashSet<String>,
}

impl Chooser {
    pub(super) fn new(
        field: usize,
        label: &'static str,
        options: Vec<String>,
        current: &str,
    ) -> Self {
        let mut state = ListState::default();
        state.select(Some(options.iter().position(|o| o == current).unwrap_or(0)));
        Self {
            field,
            label,
            options,
            filter: String::new(),
            state,
            rect: Rect::default(),
            multi: false,
            marked: std::collections::HashSet::new(),
        }
    }

    /// A ticking dropdown for a `Multi` field, with `current` already ticked.
    pub(super) fn multi(
        field: usize,
        label: &'static str,
        options: Vec<String>,
        current: &[String],
    ) -> Self {
        let mut ch = Self::new(
            field,
            label,
            options,
            current.first().map_or("", String::as_str),
        );
        ch.multi = true;
        ch.marked = current.iter().cloned().collect();
        ch
    }

    /// Tick or untick the option at `i` within the filtered rows.
    pub(super) fn toggle(&mut self, i: usize) {
        let Some(name) = self.matches().get(i).cloned() else {
            return;
        };
        if !self.marked.remove(&name) {
            self.marked.insert(name);
        }
    }

    /// The ticked options in the list's own order, so the stored value does not
    /// depend on the order they happened to be ticked in.
    pub(super) fn ticked(&self) -> Vec<String> {
        self.options
            .iter()
            .filter(|o| self.marked.contains(*o))
            .cloned()
            .collect()
    }

    /// The option index (within `matches()`) under (col,row), None if outside. The
    /// box's first & last rows are borders; the list may be scrolled (offset).
    pub(super) fn item_at(&self, col: u16, row: u16) -> Option<usize> {
        let r = self.rect;
        let inside = col >= r.x
            && col < r.x.saturating_add(r.width)
            && row > r.y
            && row < r.y.saturating_add(r.height).saturating_sub(1);
        if !inside {
            return None;
        }
        let idx = (row - r.y - 1) as usize + self.state.offset();
        (idx < self.matches().len()).then_some(idx)
    }

    /// The options that pass the filter (case-insensitive, substring).
    pub(super) fn matches(&self) -> Vec<String> {
        let f = self.filter.to_lowercase();
        self.options
            .iter()
            .filter(|o| f.is_empty() || o.to_lowercase().contains(&f))
            .cloned()
            .collect()
    }

    pub(super) fn selected(&self) -> Option<String> {
        let m = self.matches();
        self.state.selected().and_then(|i| m.get(i).cloned())
    }

    /// Keep the selected index valid after the filter changes.
    pub(super) fn clamp(&mut self) {
        let len = self.matches().len();
        let i = self.state.selected().unwrap_or(0);
        self.state
            .select(if len == 0 { None } else { Some(i.min(len - 1)) });
    }
}
