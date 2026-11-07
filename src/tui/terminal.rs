//! A terminal inside the container — embedded in a TUI pane (not a full-screen
//! takeover).
//!
//! The protocol was verified with a live WebSocket round trip to the server:
//! `wss://{panel}/ws/containerShell?container={id}&command={b64}&token={apiToken}`,
//! exchanging JSON `{"input"}` / `{"resize":[cols,rows]}` (to the server) and
//! `{"output"}` (from the server). Auth uses the API token this tool already
//! has stored.
//!
//! The WebSocket runs on its own thread: its output is sent as `Resp::TermOutput`
//! to be fed into the vt100 parser (which becomes the on-screen grid), and
//! keystrokes from the event loop are sent back over a channel. So tabs & the
//! status bar stay visible — the terminal lives inside the content pane,
//! seamlessly.

use std::sync::mpsc::Sender;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::session::TermMsg;

/// The active container-terminal session's state, grouped out of `App` the same
/// way `ViewerUi`/`BackupUi`/`CredsUi` were — one sub-view, one struct, next to
/// the code that drives it.
///
/// `parser` and `input` are the live session (dropping `input` closes it);
/// `title` is the pane label and is deliberately NOT cleared alongside them —
/// the "Terminal {title} closed" message reads it after the session ends.
#[derive(Default)]
pub(super) struct TermUi {
    /// The screen emulator: a vt100 parser fed by WebSocket output.
    pub(super) parser: Option<vt100::Parser>,
    /// Keystrokes/resizes to the WebSocket thread. Dropping it = close the session.
    pub(super) input: Option<Sender<TermMsg>>,
    /// The terminal pane title (project/service).
    pub(super) title: String,
    /// The text selection being dragged (or held after release, until the next
    /// keystroke). None = nothing selected.
    pub(super) sel: Option<TermSel>,
}

/// A text selection over the terminal grid — what a mouse drag marks and its
/// release copies to the clipboard.
///
/// Rows are stored in the coordinate frame of `scrollback`: the offset in force
/// when the anchor was set. A line of output that sits at screen row `r` with
/// scrollback `s` sits at `r + 1` with scrollback `s + 1`, so `row - scrollback`
/// names the content rather than the viewport. Keeping the frame is what makes a
/// selection stay on the text it marked while the user scrolls (and while new
/// output arrives, which vt100 absorbs by bumping the offset).
///
/// The selection is linewise (reading order), not a block: whole rows between
/// the two ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct TermSel {
    /// Where the drag started, as (row, col) in this selection's frame.
    pub(super) anchor: (i32, u16),
    /// Where the pointer is now, same frame.
    pub(super) cursor: (i32, u16),
    /// The scrollback offset the rows above are measured against.
    pub(super) scrollback: usize,
}

impl TermSel {
    /// A selection that starts and ends on one cell (a click that has not been
    /// dragged yet), anchored to what is on screen at scrollback `sb`.
    pub(super) fn new(row: u16, col: u16, sb: usize) -> Self {
        let at = (i32::from(row), col);
        Self {
            anchor: at,
            cursor: at,
            scrollback: sb,
        }
    }

    /// Move the loose end to a screen cell seen at scrollback `sb`.
    pub(super) fn extend(&mut self, row: u16, col: u16, sb: usize) {
        self.cursor = (self.frame_row(row, sb), col);
    }

    /// A click with no drag selects one cell — which is not something anyone
    /// means to copy, so it counts as nothing selected.
    pub(super) fn is_empty(&self) -> bool {
        self.anchor == self.cursor
    }

    /// Is the cell at screen (row, col), seen at scrollback `sb`, selected?
    pub(super) fn contains(&self, row: u16, col: u16, sb: usize) -> bool {
        let r = self.frame_row(row, sb);
        let (start, end) = self.ordered();
        r >= start.0
            && r <= end.0
            && !(r == start.0 && col < start.1)
            && !(r == end.0 && col > end.1)
    }

    /// A screen row seen at scrollback `sb`, in this selection's frame.
    fn frame_row(&self, row: u16, sb: usize) -> i32 {
        i32::from(row) + self.scrollback as i32 - sb as i32
    }

    /// The inverse: a frame row as a screen row at scrollback `sb`. May fall
    /// outside the grid — the content has been scrolled away from.
    fn screen_row(&self, row: i32, sb: usize) -> i32 {
        row - self.scrollback as i32 + sb as i32
    }

    /// The two ends in reading order; a drag may run backwards or upwards.
    fn ordered(&self) -> ((i32, u16), (i32, u16)) {
        if self.anchor <= self.cursor {
            (self.anchor, self.cursor)
        } else {
            (self.cursor, self.anchor)
        }
    }
}

/// The selected text, read off the screen as it stands.
///
/// vt100's own `contents_between` is the clipboard primitive here: it skips
/// wide-character continuation cells, pads gaps inside a line, and suppresses
/// the newline on a row the shell *wrapped* — so a copied long command line
/// pastes back as one line. Rows are joined with `\n`.
///
/// Blanks the shell actually printed (column padding in `ls -l`, a cleared
/// prompt line) do survive that, and a pasted line ending in invisible junk is
/// nobody's intent, so each row is trimmed at its tail.
///
/// Only the viewport can be read back, so a selection whose ends have been
/// scrolled out of sight contributes the part still visible.
pub(super) fn selection_text(screen: &vt100::Screen, sel: &TermSel) -> String {
    let (rows, cols) = screen.size();
    if rows == 0 || cols == 0 || sel.is_empty() {
        return String::new();
    }
    let sb = screen.scrollback();
    let (start, end) = sel.ordered();
    let (top, bottom) = (sel.screen_row(start.0, sb), sel.screen_row(end.0, sb));
    let last = i32::from(rows) - 1;
    if bottom < 0 || top > last {
        return String::new();
    }
    // A clamped end loses its column bound: the visible fragment runs to the
    // edge of the grid.
    let start_col = if top < 0 { 0 } else { start.1.min(cols - 1) };
    let end_col = if bottom > last {
        cols
    } else {
        end.1.min(cols - 1) + 1
    };
    let raw = screen.contents_between(
        top.max(0) as u16,
        start_col,
        bottom.min(last) as u16,
        end_col,
    );
    let mut out = String::with_capacity(raw.len());
    for (i, line) in raw.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(line.trim_end());
    }
    out
}

/// Encode a KeyEvent into bytes to send to the shell (xterm encoding).
/// None = not sent (e.g. an unmapped key).
pub(super) fn encode_key(key: KeyEvent) -> Option<Vec<u8>> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let bytes = match key.code {
        KeyCode::Char(c) => {
            if ctrl {
                // Ctrl-A..Z → 0x01..0x1a; matches a real terminal.
                let b = (c.to_ascii_uppercase() as u8).wrapping_sub(b'@') & 0x7f;
                vec![b]
            } else if alt {
                let mut v = vec![0x1b];
                v.extend(c.to_string().into_bytes());
                v
            } else {
                c.to_string().into_bytes()
            }
        }
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::BackTab => vec![0x1b, b'[', b'Z'],
        KeyCode::Esc => vec![0x1b],
        KeyCode::Left => vec![0x1b, b'[', b'D'],
        KeyCode::Right => vec![0x1b, b'[', b'C'],
        KeyCode::Up => vec![0x1b, b'[', b'A'],
        KeyCode::Down => vec![0x1b, b'[', b'B'],
        KeyCode::Home => vec![0x1b, b'[', b'H'],
        KeyCode::End => vec![0x1b, b'[', b'F'],
        KeyCode::Delete => vec![0x1b, b'[', b'3', b'~'],
        KeyCode::Insert => vec![0x1b, b'[', b'2', b'~'],
        KeyCode::PageUp => vec![0x1b, b'[', b'5', b'~'],
        KeyCode::PageDown => vec![0x1b, b'[', b'6', b'~'],
        _ => return None,
    };
    Some(bytes)
}
