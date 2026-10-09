//! The built-in Markdown editor. It renders the document as you edit; the cursor line (or the
//! selection) shows its raw source. Text wraps at word boundaries and counts wide characters.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::SystemTime;

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent};
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Wrap};
use unicode_width::UnicodeWidthChar;

use crate::store::Res;
use crate::{md, theme};

const MAX_BYTES: u64 = 2 * 1024 * 1024;
const UNDO_DEPTH: usize = 200;

#[derive(Clone, Copy, PartialEq)]
enum Edit {
    Type,
    Delete,
    Other,
}

struct Snapshot {
    lines: Vec<String>,
    row: usize,
    col: usize,
}

pub enum Outcome {
    Open,
    Close,
}

pub struct Editor {
    path: PathBuf,
    name: String,
    lines: Vec<String>,
    row: usize,
    col: usize,
    anchor: Option<(usize, usize)>,
    top: usize,
    first: usize,
    text_w: usize,
    page: usize,
    dirty: bool,
    read_only: bool,
    /// Where the text was drawn and where each logical line starts on screen, for clicks.
    inner: Rect,
    vstart: Vec<usize>,
    /// The footer hints and the key each stands for.
    hints: Vec<(Rect, KeyEvent)>,
    trailing_newline: bool,
    split: bool,
    armed: bool,
    overwrite_armed: bool,
    stale: bool,
    loaded: Option<SystemTime>,
    find: Option<String>,
    clip: String,
    status: String,
    undo: Vec<Snapshot>,
    last: Option<Edit>,
}

// ---- text geometry -------------------------------------------------------------------------

fn cw(c: char) -> usize {
    c.width().unwrap_or(0)
}

fn byte_at(line: &str, col: usize) -> usize {
    line.char_indices().nth(col).map_or(line.len(), |(b, _)| b)
}

fn chars_width(chars: &[char]) -> usize {
    chars.iter().map(|&c| cw(c)).sum()
}

/// Char index where each display row starts. Rows break after a space when one fits, else mid-word.
/// A row that is exactly full gets one more empty row after it, so a cursor at the end has a place.
fn row_starts(chars: &[char], w: usize) -> Vec<usize> {
    let w = w.max(2);
    let mut starts = vec![0];
    let mut used = 0;
    let mut brk: Option<usize> = None;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let width = cw(c);
        if used + width > w && used > 0 {
            let start = *starts.last().unwrap_or(&0);
            let at = match brk {
                Some(b) if b > start => b,
                _ => i,
            };
            starts.push(at);
            used = chars_width(&chars[at..i]);
            brk = None;
            continue;
        }
        used += width;
        i += 1;
        if c == ' ' {
            brk = Some(i);
        }
    }
    if used == w {
        starts.push(chars.len());
    }
    starts
}

fn row_of(starts: &[usize], col: usize) -> usize {
    starts.partition_point(|&s| s <= col).saturating_sub(1)
}

/// The char index in display row `r` closest to display column `vcol` (never past the row).
fn col_at(chars: &[char], starts: &[usize], r: usize, vcol: usize) -> usize {
    let start = starts[r];
    let max = if r + 1 < starts.len() {
        starts[r + 1] - 1
    } else {
        chars.len()
    };
    let (mut col, mut acc) = (start, 0);
    while col < max && acc + cw(chars[col]) <= vcol {
        acc += cw(chars[col]);
        col += 1;
    }
    col
}

type Cell = (char, Style);

fn cells(line: &Line) -> Vec<Cell> {
    let base = line.style;
    line.spans
        .iter()
        .flat_map(|s| {
            let style = base.patch(s.style);
            s.content.chars().map(move |c| (c, style))
        })
        .collect()
}

fn to_line(cells: &[Cell]) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = vec![];
    let mut buf = String::new();
    let mut style = Style::new();
    for &(c, s) in cells {
        if !buf.is_empty() && s != style {
            spans.push(Span::styled(std::mem::take(&mut buf), style));
        }
        style = s;
        buf.push(c);
    }
    if !buf.is_empty() {
        spans.push(Span::styled(buf, style));
    }
    Line::from(spans)
}

// ---- clipboard -----------------------------------------------------------------------------

fn sys_copy(text: &str) {
    let (cmd, args): (&str, &[&str]) = if cfg!(target_os = "macos") {
        ("pbcopy", &[])
    } else {
        ("xclip", &["-selection", "clipboard"])
    };
    if let Ok(mut child) = Command::new(cmd).args(args).stdin(Stdio::piped()).spawn() {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text.as_bytes());
        }
        let _ = child.wait();
    }
}

fn sys_paste() -> Option<String> {
    let (cmd, args): (&str, &[&str]) = if cfg!(target_os = "macos") {
        ("pbpaste", &[])
    } else {
        ("xclip", &["-selection", "clipboard", "-o"])
    };
    let out = Command::new(cmd).args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn mtime(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).and_then(|m| m.modified()).ok()
}

// ---- editor --------------------------------------------------------------------------------

impl Editor {
    pub fn open(path: &Path) -> Res<Editor> {
        if fs::metadata(path)?.len() > MAX_BYTES {
            return Err("file is larger than 2 MB".into());
        }
        let text = fs::read_to_string(path).map_err(|_| "file is not valid UTF-8 text")?;
        let mut editor = Editor {
            path: path.to_path_buf(),
            name: path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            lines: vec![],
            row: 0,
            col: 0,
            anchor: None,
            top: 0,
            first: 0,
            text_w: 40,
            page: 20,
            dirty: false,
            read_only: false,
            inner: Rect::default(),
            vstart: vec![],
            hints: vec![],
            trailing_newline: true,
            split: false,
            armed: false,
            overwrite_armed: false,
            stale: false,
            loaded: mtime(path),
            find: None,
            clip: String::new(),
            status: String::new(),
            undo: vec![],
            last: None,
        };
        editor.set_text(&text);
        Ok(editor)
    }

    /// The same window, read only: for a brief that was handed off. It moves, finds, copies and follows the file.
    pub fn view(path: &Path) -> Res<Editor> {
        let mut editor = Editor::open(path)?;
        editor.read_only = true;
        Ok(editor)
    }

    fn set_text(&mut self, text: &str) {
        self.lines = text.lines().map(|l| l.replace('\t', "  ")).collect();
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.trailing_newline = text.is_empty() || text.ends_with('\n');
        self.row = self.row.min(self.lines.len() - 1);
        self.col = self.col.min(self.line_len());
        self.anchor = None;
    }

    /// Called once a second: notice a change made on disk, for example by a worker's report.
    pub fn tick(&mut self) {
        if mtime(&self.path) == self.loaded {
            return;
        }
        if self.dirty {
            self.stale = true;
        } else {
            self.reload("reloaded: the file changed on disk");
        }
    }

    fn reload(&mut self, note: &str) {
        match fs::read_to_string(&self.path) {
            Ok(text) => {
                if self.dirty {
                    self.checkpoint(Edit::Other);
                }
                self.set_text(&text);
                self.loaded = mtime(&self.path);
                self.dirty = false;
                self.stale = false;
                self.status = note.into();
            }
            Err(e) => self.status = format!("reload failed: {e}"),
        }
    }

    fn checkpoint(&mut self, kind: Edit) {
        if kind == Edit::Other || self.last != Some(kind) {
            if self.undo.len() == UNDO_DEPTH {
                self.undo.remove(0);
            }
            self.undo.push(Snapshot {
                lines: self.lines.clone(),
                row: self.row,
                col: self.col,
            });
        }
        self.last = (kind != Edit::Other).then_some(kind);
        self.dirty = true;
    }

    fn save(&mut self) {
        if mtime(&self.path) != self.loaded && !self.overwrite_armed {
            self.overwrite_armed = true;
            self.status = "changed on disk: ctrl-s again overwrites, ctrl-r reloads".into();
            return;
        }
        let mut text = self.lines.join("\n");
        if self.trailing_newline {
            text.push('\n');
        }
        match fs::write(&self.path, text) {
            Ok(()) => {
                self.dirty = false;
                self.stale = false;
                self.loaded = mtime(&self.path);
                self.status = "saved".into();
            }
            Err(e) => self.status = format!("save failed: {e}"),
        }
    }

    fn undo(&mut self) {
        match self.undo.pop() {
            Some(s) => {
                self.lines = s.lines;
                self.row = s.row;
                self.col = s.col;
                self.anchor = None;
                self.dirty = true;
                self.last = None;
            }
            None => self.status = "nothing to undo".into(),
        }
    }

    fn line_len(&self) -> usize {
        self.lines[self.row].chars().count()
    }

    fn chars(&self, row: usize) -> Vec<char> {
        self.lines[row].chars().collect()
    }

    // ---- selection and clipboard ----

    fn selection(&self) -> Option<((usize, usize), (usize, usize))> {
        let a = self.anchor?;
        let b = (self.row, self.col);
        (a != b).then(|| if a < b { (a, b) } else { (b, a) })
    }

    fn selected_text(&self) -> Option<String> {
        let ((r1, c1), (r2, c2)) = self.selection()?;
        let mut out = String::new();
        for r in r1..=r2 {
            let line = &self.lines[r];
            let from = if r == r1 { byte_at(line, c1) } else { 0 };
            let to = if r == r2 {
                byte_at(line, c2)
            } else {
                line.len()
            };
            out.push_str(&line[from..to]);
            if r < r2 {
                out.push('\n');
            }
        }
        Some(out)
    }

    fn delete_selection(&mut self) -> bool {
        let Some(((r1, c1), (r2, c2))) = self.selection() else {
            self.anchor = None;
            return false;
        };
        if r1 == r2 {
            let line = &mut self.lines[r1];
            let (b1, b2) = (byte_at(line, c1), byte_at(line, c2));
            line.replace_range(b1..b2, "");
        } else {
            let tail = {
                let l = &self.lines[r2];
                l[byte_at(l, c2)..].to_string()
            };
            let head = &mut self.lines[r1];
            let b1 = byte_at(head, c1);
            head.truncate(b1);
            head.push_str(&tail);
            self.lines.drain(r1 + 1..=r2);
        }
        self.row = r1;
        self.col = c1;
        self.anchor = None;
        true
    }

    fn copy(&mut self, cut: bool) {
        let whole_line = cut && self.selection().is_none();
        if whole_line {
            self.anchor = Some((self.row, 0));
            self.col = self.line_len();
        }
        let Some(mut text) = self.selected_text() else {
            if whole_line {
                self.anchor = None;
            } else {
                self.status = "nothing selected".into();
            }
            return;
        };
        if whole_line {
            text.push('\n');
        }
        self.clip = text.clone();
        sys_copy(&text);
        if cut {
            self.checkpoint(Edit::Other);
            self.delete_selection();
            if whole_line && self.row + 1 < self.lines.len() {
                self.lines.remove(self.row);
            }
        }
        self.status = if cut { "cut" } else { "copied" }.into();
    }

    fn paste_clipboard(&mut self) {
        let text = sys_paste()
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| self.clip.clone());
        self.paste(&text);
    }

    /// Insert text from a terminal paste or the clipboard as one undo step.
    pub fn paste(&mut self, text: &str) {
        if self.find.is_some() || self.read_only {
            return;
        }
        self.insert_text(text, Edit::Other);
    }

    fn insert_text(&mut self, text: &str, kind: Edit) {
        let text = text.replace('\r', "").replace('\t', "  ");
        let replacing = self.selection().is_some();
        self.checkpoint(if replacing || text.contains('\n') {
            Edit::Other
        } else {
            kind
        });
        self.delete_selection();
        let at = byte_at(&self.lines[self.row], self.col);
        let tail = self.lines[self.row].split_off(at);
        let mut parts = text.split('\n');
        let first = parts.next().unwrap_or("");
        self.lines[self.row].push_str(first);
        self.col += first.chars().count();
        for part in parts {
            self.row += 1;
            self.lines.insert(self.row, part.to_string());
            self.col = part.chars().count();
        }
        self.lines[self.row].push_str(&tail);
    }

    // ---- find ----

    fn find_next(&mut self, from_start_of_match: bool) {
        let query = self.find.clone().unwrap_or_default().to_lowercase();
        if query.is_empty() {
            return;
        }
        let origin = if from_start_of_match {
            self.anchor.unwrap_or((self.row, self.col))
        } else {
            (self.row, self.col)
        };
        let n = self.lines.len();
        for i in 0..=n {
            let r = (origin.0 + i) % n;
            let lower = self.lines[r].to_lowercase();
            let start = if i == 0 { byte_at(&lower, origin.1) } else { 0 };
            if let Some(pos) = lower[start..].find(&query) {
                let c = lower[..start + pos].chars().count();
                self.row = r;
                self.anchor = Some((r, c));
                self.col = c + query.chars().count();
                self.status.clear();
                return;
            }
        }
        self.status = "no match".into();
    }

    fn find_key(&mut self, key: KeyEvent) {
        let Some(query) = &mut self.find else { return };
        match key.code {
            KeyCode::Esc => {
                self.find = None;
                self.anchor = None;
            }
            KeyCode::Enter => self.find_next(false),
            KeyCode::Backspace => {
                query.pop();
                self.find_next(true);
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                query.push(c);
                self.find_next(true);
            }
            _ => {}
        }
    }

    /// A left click: a footer hint presses its key, a click in the text puts the cursor there.
    pub fn click(&mut self, mouse: MouseEvent) -> Outcome {
        let at = Position::new(mouse.column, mouse.row);
        if let Some(&(_, event)) = self.hints.iter().find(|(r, _)| r.contains(at)) {
            return self.key(event);
        }
        if self.find.is_some() || !self.inner.contains(at) || self.vstart.is_empty() {
            return Outcome::Open;
        }
        let v = self.top + (mouse.row - self.inner.y) as usize;
        let line =
            (self.vstart.partition_point(|&s| s <= v).saturating_sub(1)).min(self.lines.len() - 1);
        let chars = self.chars(line);
        let starts = row_starts(&chars, self.text_w);
        let r = v.saturating_sub(self.vstart[line]).min(starts.len() - 1);
        self.row = line;
        self.col = col_at(&chars, &starts, r, (mouse.column - self.inner.x) as usize);
        self.anchor = None;
        self.last = None;
        self.armed = false;
        Outcome::Open
    }

    // ---- keys ----

    pub fn key(&mut self, key: KeyEvent) -> Outcome {
        if self.find.is_some() {
            self.find_key(key);
            return Outcome::Open;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let leaving = matches!(
            (key.code, ctrl),
            (KeyCode::Esc, _) | (KeyCode::Char('q'), true)
        );
        if !leaving {
            self.armed = false;
        }
        if !matches!((key.code, ctrl), (KeyCode::Char('s'), true)) {
            self.overwrite_armed = false;
        }
        self.status.clear();
        if self.read_only {
            match (key.code, ctrl) {
                (KeyCode::Char('q'), false) => return Outcome::Close,
                (KeyCode::Char('s' | 'z' | 'x' | 'v'), true)
                | (KeyCode::Enter | KeyCode::Backspace | KeyCode::Delete | KeyCode::Tab, _)
                | (KeyCode::Char(_), false) => {
                    self.status =
                        "read only: a brief only grows at the bottom (brie report)".into();
                    return Outcome::Open;
                }
                _ => {}
            }
        }

        let moving = matches!(
            key.code,
            KeyCode::Left
                | KeyCode::Right
                | KeyCode::Up
                | KeyCode::Down
                | KeyCode::Home
                | KeyCode::End
                | KeyCode::PageUp
                | KeyCode::PageDown
        );
        if moving {
            self.last = None;
            if shift && self.anchor.is_none() {
                self.anchor = Some((self.row, self.col));
            } else if !shift {
                self.anchor = None;
            }
        }

        match (key.code, ctrl) {
            (KeyCode::Char('s'), true) => self.save(),
            (KeyCode::Char('z'), true) => self.undo(),
            (KeyCode::Char('r'), true) => self.reload("reloaded from disk"),
            (KeyCode::Char('p'), true) => self.split = !self.split,
            (KeyCode::Char('f'), true) => self.find = Some(String::new()),
            (KeyCode::Char('c'), true) => self.copy(false),
            (KeyCode::Char('x'), true) => self.copy(true),
            (KeyCode::Char('v'), true) => self.paste_clipboard(),
            (KeyCode::Esc, _) | (KeyCode::Char('q'), true) => {
                if self.anchor.is_some() {
                    self.anchor = None;
                } else if self.dirty && !self.armed {
                    self.armed = true;
                    self.status = "unsaved changes: ctrl-s saves, esc again discards".into();
                } else {
                    return Outcome::Close;
                }
            }
            (KeyCode::Left, _) => {
                if self.col > 0 {
                    self.col -= 1;
                } else if self.row > 0 {
                    self.row -= 1;
                    self.col = self.line_len();
                }
            }
            (KeyCode::Right, _) => {
                if self.col < self.line_len() {
                    self.col += 1;
                } else if self.row + 1 < self.lines.len() {
                    self.row += 1;
                    self.col = 0;
                }
            }
            (KeyCode::Up, _) => self.step_visual(-1),
            (KeyCode::Down, _) => self.step_visual(1),
            (KeyCode::PageUp, _) => (0..self.page).for_each(|_| self.step_visual(-1)),
            (KeyCode::PageDown, _) => (0..self.page).for_each(|_| self.step_visual(1)),
            (KeyCode::Home, _) | (KeyCode::Char('a'), true) => self.col = 0,
            (KeyCode::End, _) | (KeyCode::Char('e'), true) => self.col = self.line_len(),
            (KeyCode::Enter, _) => self.insert_text("\n", Edit::Other),
            (KeyCode::Backspace, _) => {
                if self.selection().is_some() {
                    self.checkpoint(Edit::Other);
                    self.delete_selection();
                } else if self.col > 0 {
                    self.checkpoint(Edit::Delete);
                    let at = byte_at(&self.lines[self.row], self.col - 1);
                    self.lines[self.row].remove(at);
                    self.col -= 1;
                } else if self.row > 0 {
                    self.checkpoint(Edit::Other);
                    let cur = self.lines.remove(self.row);
                    self.row -= 1;
                    self.col = self.line_len();
                    self.lines[self.row].push_str(&cur);
                }
            }
            (KeyCode::Delete, _) => {
                if self.selection().is_some() {
                    self.checkpoint(Edit::Other);
                    self.delete_selection();
                } else if self.col < self.line_len() {
                    self.checkpoint(Edit::Delete);
                    let at = byte_at(&self.lines[self.row], self.col);
                    self.lines[self.row].remove(at);
                } else if self.row + 1 < self.lines.len() {
                    self.checkpoint(Edit::Other);
                    let next = self.lines.remove(self.row + 1);
                    self.lines[self.row].push_str(&next);
                }
            }
            (KeyCode::Tab, _) => self.insert_text("  ", Edit::Type),
            (KeyCode::Char(c), false) if !alt => self.insert_text(&c.to_string(), Edit::Type),
            _ => {}
        }
        Outcome::Open
    }

    /// Move one display row up or down, keeping the display column.
    fn step_visual(&mut self, dir: isize) {
        let chars = self.chars(self.row);
        let starts = row_starts(&chars, self.text_w);
        let r = row_of(&starts, self.col);
        let vcol = chars_width(&chars[starts[r]..self.col.min(chars.len())]);
        if dir < 0 {
            if r > 0 {
                self.col = col_at(&chars, &starts, r - 1, vcol);
            } else if self.row > 0 {
                self.row -= 1;
                let chars = self.chars(self.row);
                let starts = row_starts(&chars, self.text_w);
                self.col = col_at(&chars, &starts, starts.len() - 1, vcol);
            }
        } else if r + 1 < starts.len() {
            self.col = col_at(&chars, &starts, r + 1, vcol);
        } else if self.row + 1 < self.lines.len() {
            self.row += 1;
            let chars = self.chars(self.row);
            let starts = row_starts(&chars, self.text_w);
            self.col = col_at(&chars, &starts, 0, vcol);
        }
    }

    // ---- drawing ---------------------------------------------------------------------------

    pub fn draw(&mut self, f: &mut Frame) {
        let t = theme::get();
        f.render_widget(
            Block::new().style(Style::new().fg(t.text).bg(t.bg)),
            f.area(),
        );
        let [body, footer] =
            Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(f.area());
        let wide = self.split && f.area().width >= 100;
        let panes = if wide {
            Layout::horizontal([Constraint::Fill(1), Constraint::Fill(1)]).split(body)
        } else {
            Layout::horizontal([Constraint::Fill(1)]).split(body)
        };

        let mut title = vec![
            Span::styled(
                if self.read_only { " VIEW " } else { " EDIT " },
                Style::new().fg(t.accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("{} ", self.name), Style::new().fg(t.muted)),
        ];
        if self.dirty {
            title.push(Span::styled("● ", Style::new().fg(t.accent)));
        }
        if self.stale {
            title.push(Span::styled("changed on disk ", Style::new().fg(t.bad)));
        }
        let block = Block::bordered()
            .title(Line::from(title))
            .border_style(Style::new().fg(t.accent))
            .style(Style::new().bg(t.surface));
        let inner = block.inner(panes[0]);
        f.render_widget(block, panes[0]);

        let (h, w) = (inner.height as usize, inner.width as usize);
        if h > 0 && w >= 2 {
            self.page = h;
            self.text_w = w;
            let selection = self.selection();
            let raw = match selection {
                Some(((r1, _), (r2, _))) => r1..=r2,
                None => self.row..=self.row,
            };
            let live = md::live(&self.lines, raw);
            let mut grid: Vec<Vec<Cell>> = live.iter().map(cells).collect();
            if let Some(((r1, c1), (r2, c2))) = selection {
                let mark = Style::new().fg(t.accent_ink).bg(t.info);
                for (r, line) in grid.iter_mut().enumerate().take(r2 + 1).skip(r1) {
                    let len = line.len();
                    let from = if r == r1 { c1.min(len) } else { 0 };
                    let to = if r == r2 { c2.min(len) } else { len };
                    for cell in &mut line[from..to] {
                        cell.1 = cell.1.patch(mark);
                    }
                }
            }
            let row_sets: Vec<Vec<usize>> = grid
                .iter()
                .map(|line| row_starts(&line.iter().map(|c| c.0).collect::<Vec<_>>(), w))
                .collect();
            let mut vstart = Vec::with_capacity(row_sets.len());
            let mut acc = 0;
            for rows in &row_sets {
                vstart.push(acc);
                acc += rows.len();
            }

            let chars = self.chars(self.row);
            let starts = &row_sets[self.row];
            let cursor_r = row_of(starts, self.col);
            let vcol = chars_width(&chars[starts[cursor_r]..self.col.min(chars.len())]);
            let cursor_v = vstart[self.row] + cursor_r;
            if cursor_v < self.top {
                self.top = cursor_v;
            } else if cursor_v >= self.top + h {
                self.top = cursor_v + 1 - h;
            }
            self.first = vstart.partition_point(|&s| s <= self.top).saturating_sub(1);
            self.inner = inner;
            self.vstart = vstart.clone();
            let skip = self.top - vstart[self.first];
            let mut rows: Vec<Line<'static>> = vec![];
            'fill: for (line, starts) in grid.iter().zip(&row_sets).skip(self.first) {
                for (k, &from) in starts.iter().enumerate() {
                    let to = starts.get(k + 1).copied().unwrap_or(line.len());
                    rows.push(to_line(&line[from.min(line.len())..to.min(line.len())]));
                    if rows.len() >= skip + h {
                        break 'fill;
                    }
                }
            }
            let shown: Vec<Line<'static>> = rows.into_iter().skip(skip).take(h).collect();
            f.render_widget(Paragraph::new(shown), inner);
            f.set_cursor_position(Position::new(
                inner.x + vcol as u16,
                inner.y + (cursor_v - self.top) as u16,
            ));
        }

        if wide {
            let rendered = md::render(&self.lines.join("\n"));
            let scroll = self.first * rendered.len() / self.lines.len().max(1);
            let preview = Paragraph::new(rendered)
                .block(
                    Block::bordered()
                        .title(Line::styled(
                            " PREVIEW ",
                            Style::new().fg(t.muted).add_modifier(Modifier::BOLD),
                        ))
                        .border_style(Style::new().fg(t.line))
                        .style(Style::new().bg(t.surface))
                        .padding(ratatui::widgets::Padding::horizontal(1)),
                )
                .wrap(Wrap { trim: false })
                .scroll((scroll as u16, 0));
            f.render_widget(preview, panes[1]);
        }

        let [hints, pos] =
            Layout::horizontal([Constraint::Fill(1), Constraint::Length(16)]).areas(footer);
        self.hints.clear();
        let line = if let Some(q) = &self.find {
            let note = if self.status.is_empty() {
                "  enter next · esc close".to_string()
            } else {
                format!("  {}", self.status)
            };
            Line::from(vec![
                Span::styled(" find ", Style::new().fg(t.accent_ink).bg(t.accent)),
                Span::styled(format!(" {q}"), Style::new().fg(t.text)),
                Span::styled("█", Style::new().fg(t.accent)),
                Span::styled(note, Style::new().fg(t.muted)),
            ])
        } else if !self.status.is_empty() {
            Line::styled(format!(" {}", self.status), Style::new().fg(t.accent))
        } else {
            let mut spans = vec![Span::raw(" ")];
            let ctrl = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
            let list: Vec<(&str, &str, KeyEvent)> = if self.read_only {
                vec![
                    ("^F", "find", ctrl('f')),
                    ("^C", "copy", ctrl('c')),
                    ("^P", "side preview", ctrl('p')),
                    (
                        "esc",
                        "close",
                        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
                    ),
                ]
            } else {
                vec![
                    ("^S", "save", ctrl('s')),
                    ("^Z", "undo", ctrl('z')),
                    ("^F", "find", ctrl('f')),
                    ("^C", "copy", ctrl('c')),
                    ("^X", "cut", ctrl('x')),
                    ("^V", "paste", ctrl('v')),
                    ("^P", "side preview", ctrl('p')),
                    (
                        "esc",
                        "close",
                        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
                    ),
                ]
            };
            let mut x = hints.x + 1;
            self.hints.clear();
            for (key, what, event) in list {
                let width = (key.chars().count() + what.chars().count() + 3) as u16;
                self.hints.push((Rect::new(x, hints.y, width, 1), event));
                x += width;
                spans.push(Span::styled(key, Style::new().fg(t.accent)));
                spans.push(Span::styled(format!(" {what}  "), Style::new().fg(t.muted)));
            }
            Line::from(spans)
        };
        f.render_widget(Paragraph::new(line), hints);
        f.render_widget(
            Paragraph::new(Line::styled(
                format!("Ln {}, Col {} ", self.row + 1, self.col + 1),
                Style::new().fg(t.muted),
            ))
            .right_aligned(),
            pos,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::MouseEventKind;

    fn press(editor: &mut Editor, code: KeyCode) -> bool {
        matches!(
            editor.key(KeyEvent::new(code, KeyModifiers::NONE)),
            Outcome::Close
        )
    }

    #[test]
    fn a_click_places_the_cursor_and_the_footer_hints_press_their_keys() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        use ratatui::crossterm::event::MouseButton;
        let click = |column, row| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        };
        let path = std::env::temp_dir().join(format!("brie-click-{}.md", std::process::id()));
        fs::write(&path, "alpha\nbravo charlie\ndelta\n").unwrap();
        let mut e = Editor::open(&path).unwrap();
        let mut term = Terminal::new(TestBackend::new(80, 20)).unwrap();
        term.draw(|f| e.draw(f)).unwrap();
        let inner = e.inner;
        e.click(click(inner.x + 4, inner.y + 1));
        assert_eq!((e.row, e.col), (1, 4));
        // Past the end of a line the cursor goes to its end.
        e.click(click(inner.x + 30, inner.y + 2));
        assert_eq!((e.row, e.col), (2, 5));
        // The footer's ^F hint starts a find.
        let find = e
            .hints
            .iter()
            .find(|(_, k)| k.code == KeyCode::Char('f'))
            .unwrap()
            .0;
        e.click(click(find.x, find.y));
        assert!(e.find.is_some());
        fs::remove_file(&path).unwrap();
    }

    #[test]
    fn viewer_moves_but_does_not_edit() {
        let path = std::env::temp_dir().join(format!("brie-viewer-{}.md", std::process::id()));
        fs::write(&path, "one\ntwo\n").unwrap();
        let mut view = Editor::view(&path).unwrap();
        assert!(!press(&mut view, KeyCode::Char('x')));
        assert!(!press(&mut view, KeyCode::Enter));
        assert!(!press(&mut view, KeyCode::Backspace));
        view.key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
        assert!(!press(&mut view, KeyCode::Down));
        assert_eq!(view.row, 1);
        assert!(!view.dirty);
        view.paste("pasted");
        assert_eq!(view.lines, ["one", "two"]);
        assert_eq!(fs::read_to_string(&path).unwrap(), "one\ntwo\n");
        assert!(press(&mut view, KeyCode::Char('q')));
        let mut edit = Editor::open(&path).unwrap();
        assert!(!press(&mut edit, KeyCode::Char('q')));
        assert!(edit.dirty);
        fs::remove_file(&path).unwrap();
    }
}
