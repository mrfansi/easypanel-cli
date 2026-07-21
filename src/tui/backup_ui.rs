//! What the backup and restore screens are holding.
//!
//! Ten fields for one feature had accumulated in `App` — the picker's contents,
//! the ticks, what a confirmation is about to act on, and the panel's storage
//! providers — mixed in among the tabs, the filter and the terminal. They belong
//! together, and the rules that read them (which rows to draw, what a tick
//! means, what Enter acts on) belong next to them rather than in the middle of a
//! 2,600-line file.
//!
//! This is state and presentation ONLY. What a backup *is* — which provider to
//! prefer, which service types can have one, how to read a database listing —
//! stays in `crate::backup`, where it can be tested without a TUI at all.

use std::collections::HashSet;

use super::table::row_marker;

/// A backup that exists and could be restored: (database, providerId, path).
pub(super) type BackupFileRow = (String, String, String);

#[derive(Default)]
pub(super) struct BackupUi {
    // ---- restore side ----
    /// The backups on screen in the restore picker, in the SAME order as the
    /// viewer's rows.
    ///
    /// Kept as data rather than re-read from the text, so a restore cannot be
    /// aimed by parsing a row back — the mistake that once sent a delete to the
    /// wrong index.
    pub(super) files: Vec<BackupFileRow>,
    /// Which service the restore picker would restore INTO.
    pub(super) restore_into: Option<(String, String)>,
    /// The backup a confirmation is currently asking about.
    pub(super) pending_restore: Option<BackupFileRow>,

    // ---- restore from an object-storage dump this tool wrote (mirrors above) ----
    /// Which service the object-storage restore picker restores INTO.
    pub(super) r2_restore_into: Option<(String, String)>,
    /// The dump object keys on screen, in the SAME order as the picker's rows.
    pub(super) r2_dumps: Vec<String>,
    /// The dump a confirmation is about to restore (its object key).
    pub(super) pending_r2_restore: Option<String>,

    // ---- backup side ----
    /// The databases the picker is offering.
    pub(super) names: Vec<String>,
    /// The ones ticked. Marking uses `v`, the same key that marks a service in
    /// the table — one word for one idea.
    pub(super) marked: HashSet<String>,
    /// Which service those databases live in.
    pub(super) backup_from: Option<(String, String)>,
    /// The picker's first line, kept so the rows can be rebuilt when a tick
    /// changes without reassembling the sentence.
    pub(super) header: String,
    /// Where a backup will go: (id, how it is described to the user).
    pub(super) provider: Option<(String, String)>,
    /// The databases a confirmation is about to back up.
    pub(super) pending: Vec<String>,
    /// True when the picker is for a NON-LOCKING dump straight to object storage
    /// (`db dump`), false for EasyPanel's native per-database backup. Both share
    /// the same database picker; this decides what the confirmation runs.
    pub(super) r2_mode: bool,

    /// The storage providers this panel has: (id, name, type). EasyPanel exposes
    /// a list but no way to create one, so this is whatever the dashboard
    /// already has; empty until it loads, or if none is configured at all.
    pub(super) providers: Vec<(String, String, String)>,
}

impl BackupUi {
    /// The database picker's rows, rebuilt whenever a tick changes.
    ///
    /// The tick sits INSIDE the row rather than widening it, so ticking one does
    /// not shift every name sideways.
    pub(super) fn picker_lines(&self) -> Vec<String> {
        let mut lines = vec![self.header.clone(), String::new()];
        lines.push(format!(
            "{} All {} databases",
            row_marker(0),
            self.names.len()
        ));
        lines.extend(self.names.iter().enumerate().map(|(i, n)| {
            let tick = if self.marked.contains(n) {
                "✓ "
            } else {
                "  "
            };
            format!("{} {tick}{n}", row_marker(i + 1))
        }));
        lines
    }

    /// What the status line says while the database picker is open.
    pub(super) fn hint(&self) -> String {
        match self.marked.len() {
            0 => "[v] tick several · [Enter] back up the selected one · [Esc] cancel".into(),
            n => format!("{n} ticked — [Enter] backs up those · [v] untick · [Esc] cancel"),
        }
    }

    /// Tick or untick the database at picker row `i` (row 0 is "All", which is
    /// not a database). Returns false when the row is not one.
    pub(super) fn toggle(&mut self, i: usize) -> bool {
        let Some(name) = i.checked_sub(1).and_then(|n| self.names.get(n)).cloned() else {
            return false;
        };
        if !self.marked.remove(&name) {
            self.marked.insert(name);
        }
        true
    }

    /// Which databases Enter acts on, from picker row `i`.
    ///
    /// Ticks win when there are any: they were made deliberately, while the row
    /// the cursor happens to rest on is not a choice the user made. Otherwise
    /// row 0 means all of them and the rest mean themselves.
    pub(super) fn chosen(&self, i: usize) -> Vec<String> {
        if !self.marked.is_empty() {
            return self
                .names
                .iter()
                .filter(|n| self.marked.contains(*n))
                .cloned()
                .collect();
        }
        match i.checked_sub(1) {
            None => self.names.clone(),
            Some(n) => self.names.get(n).cloned().into_iter().collect(),
        }
    }

    /// Forget whichever picker was open. Called when the viewer is left, so a
    /// later Enter elsewhere cannot fire an action aimed at a screen the user
    /// has already walked away from.
    pub(super) fn close_pickers(&mut self) {
        self.restore_into = None;
        self.files.clear();
        self.backup_from = None;
        self.names.clear();
        self.marked.clear();
        self.r2_restore_into = None;
        self.r2_dumps.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ui() -> BackupUi {
        BackupUi {
            names: vec!["one".into(), "two".into(), "three".into()],
            ..Default::default()
        }
    }

    #[test]
    fn row_zero_is_all_and_is_not_tickable() {
        let mut b = ui();
        assert!(!b.toggle(0), "row 0 is the All heading, not a database");
        assert!(b.marked.is_empty());
        assert_eq!(b.chosen(0), vec!["one", "two", "three"]);
    }

    #[test]
    fn ticks_beat_whatever_the_cursor_is_resting_on() {
        let mut b = ui();
        assert!(b.toggle(1) && b.toggle(3));
        // Cursor sits on row 2 ("two"), which is NOT ticked — the ticks win.
        assert_eq!(b.chosen(2), vec!["one", "three"]);
        // Untick, and the cursor decides again.
        assert!(b.toggle(1) && b.toggle(3));
        assert_eq!(b.chosen(2), vec!["two"]);
    }

    #[test]
    fn a_tick_shows_without_shifting_the_names() {
        let mut b = ui();
        b.header = "h".into();
        let plain = b.picker_lines();
        b.toggle(2);
        let ticked = b.picker_lines();
        assert!(ticked[4].contains("✓ two"), "{:?}", ticked[4]);
        // The name starts at the same COLUMN whether ticked or not — counted in
        // characters, because ✓ is three bytes and a byte offset would compare
        // two different things.
        let col = |l: &str| l.chars().take_while(|c| *c != 't').count();
        assert_eq!(col(&plain[4]), col(&ticked[4]));
    }
}
