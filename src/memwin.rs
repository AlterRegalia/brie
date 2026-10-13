//! The memory window (`m` in the TUI): browse a project's memory and run its operations. It reads the
//! files without the lock, so it works while a turn runs. Operations that write go through the same code
//! as `brie memory`, in a thread, so the window stays alive.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, List, ListItem, ListState, Padding, Paragraph, Wrap};

use crate::agent;
use crate::editor::Outcome;
use crate::memory::{self, KINDS, Memory};
use crate::store::Project;
use crate::theme::{self, Theme};

#[derive(Clone, Copy, PartialEq, Debug)]
enum Tab {
    /// The view the agent reads, and zoom, one level at a time.
    View,
    /// The same lines, opened in place: a line shows the two it was made from.
    Outline,
    /// The briefs the log names, each as the window it was worked in.
    Threads,
    Agents,
}

impl Tab {
    /// The four tabs in order, and the key that goes straight to one.
    const ALL: [Tab; 4] = [Tab::View, Tab::Outline, Tab::Agents, Tab::Threads];

    fn next(self) -> Tab {
        let at = Tab::ALL.iter().position(|t| *t == self).unwrap_or(0);
        Tab::ALL[(at + 1) % Tab::ALL.len()]
    }

    fn label(self) -> &'static str {
        match self {
            Tab::View => " VIEW ",
            Tab::Outline => " OUTLINE ",
            Tab::Agents => " AGENTS ",
            Tab::Threads => " THREADS ",
        }
    }

    fn key(self) -> KeyCode {
        KeyCode::Char(match self {
            Tab::View => '1',
            Tab::Outline => '2',
            Tab::Agents => '3',
            Tab::Threads => '4',
        })
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Op {
    Add,
    Turn,
    Sub,
}

/// A list of nodes on screen and the selected one. The first level is the memory's view. Each zoom adds a
/// level with the two lines of the selected node.
struct Level {
    lines: Vec<(u32, u64)>,
    sel: usize,
}

struct Job {
    what: &'static str,
    rx: Receiver<Result<String, String>>,
    since: Instant,
}

/// The width of the coverage bar in the list.
const COVERAGE_CELLS: usize = 5;

/// The THREADS tab's label gutter: how much of a brief's slug fits before the timeline starts.
const SLUG_CELLS: usize = 18;

/// A line that covers at most this many messages shows them in the preview.
const CHAT_MAX: u64 = 8;

/// The glyph and color of a message kind, in the list and the preview.
fn kind_look(kind: &str, t: &Theme) -> (&'static str, Color) {
    match kind {
        "user" => ("▌", t.accent),
        "agent" => ("◆", t.text),
        "tool" => ("⚙", t.violet),
        "echo" => ("⎘", t.muted),
        "work" => ("◐", t.info),
        "note" => ("✎", t.ok),
        _ => ("·", t.muted),
    }
}

/// A bar `cells` wide for how much of the biggest line this one covers, in eighths of a cell, at least one.
fn coverage_bar(n: u64, biggest: u64, cells: usize) -> String {
    const PARTIAL: [char; 8] = ['▏', '▎', '▍', '▌', '▋', '▊', '▉', '█'];
    let eighths = ((n * cells as u64 * 8).div_ceil(biggest.max(1)) as usize).clamp(1, cells * 8);
    let mut bar = "█".repeat(eighths / 8);
    if !eighths.is_multiple_of(8) {
        bar.push(PARTIAL[eighths % 8 - 1]);
    }
    format!("{bar:<cells$}")
}

/// `1 brief` but `2 briefs`: a count that reads as English.
fn count(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("{n} {one}")
    } else {
        format!("{n} {many}")
    }
}

/// One outline row for a line, then the two it was made from when it is open.
fn push_rows(
    open: &std::collections::HashSet<(u32, u64)>,
    (l, i): (u32, u64),
    depth: usize,
    out: &mut Vec<((u32, u64), usize, bool)>,
) {
    let opened = l > 0 && open.contains(&(l, i));
    out.push(((l, i), depth, opened));
    if opened {
        push_rows(open, (l - 1, 2 * i), depth + 1, out);
        push_rows(open, (l - 1, 2 * i + 1), depth + 1, out);
    }
}

/// The kind a line starts with: `user: the login bounce...` is `user`.
fn leading_kind(text: &str) -> &str {
    text.split_once(':').map_or("", |(k, _)| k)
}

/// Message `id` as a column in `width` cells, across the message range `lo..=hi`.
fn column(id: u64, lo: u64, hi: u64, width: usize) -> usize {
    let span = hi.saturating_sub(lo).max(1);
    let width = width.max(1);
    (id.saturating_sub(lo) as usize * (width - 1) / span as usize).min(width - 1)
}

/// A run of cells with one color, as one span. `on` drops the color so the row's highlight shows.
fn run_span(run: &str, color: Option<Color>, on: bool) -> Span<'static> {
    let style = match (on, color) {
        (true, _) | (false, None) => Style::new(),
        (false, Some(c)) => Style::new().fg(c),
    };
    Span::styled(run.to_string(), style)
}

/// Cells to spans, one span per run of one color.
fn spans_of(cells: &[(char, Option<Color>)], on: bool) -> Vec<Span<'static>> {
    let mut out: Vec<Span<'static>> = vec![];
    let mut run = String::new();
    let mut key: Option<Color> = None;
    for &(c, color) in cells {
        if color != key && !run.is_empty() {
            out.push(run_span(&run, key, on));
            run.clear();
        }
        key = color;
        run.push(c);
    }
    if !run.is_empty() {
        out.push(run_span(&run, key, on));
    }
    out
}

/// One brief's window across `width` cells: `├` at its first mention, `┤` at its last, `─` between,
/// and a tick for each mention whose column is still free.
///
/// A tick that collides with another is dropped — that is what a cell grid must do, and it is why
/// the window, not the tick, is the shape that survives. A brief named once is a single `┼`.
fn lane_cells(
    m: &Memory,
    thread: &memory::Thread,
    lo: u64,
    hi: u64,
    width: usize,
    t: &Theme,
) -> Vec<(char, Option<Color>)> {
    let (x0, x1) = (
        column(thread.mentions[0], lo, hi, width),
        column(
            *thread.mentions.last().unwrap_or(&thread.mentions[0]),
            lo,
            hi,
            width,
        ),
    );
    let mut cells: Vec<(char, Option<Color>)> = (0..width).map(|_| (' ', None)).collect();
    for cell in &mut cells[x0..=x1] {
        *cell = ('─', Some(t.line));
    }
    // The ends are the first and last mention, so their color is the kind of the message that
    // names the brief there. A brief named once is one column, and says so.
    let kind_at = |id: u64| match m.message(id) {
        Some((kind, _, _)) => kind_look(kind, t).1,
        None => t.muted,
    };
    let first = thread.mentions[0];
    let last = *thread.mentions.last().unwrap_or(&first);
    if x0 == x1 {
        cells[x0] = ('┼', Some(kind_at(first)));
    } else {
        cells[x0] = ('├', Some(kind_at(first)));
        cells[x1] = ('┤', Some(kind_at(last)));
    }
    // A tick for every mention whose column is still free: the ones between the ends.
    for &id in &thread.mentions {
        let x = column(id, lo, hi, width);
        if cells[x].0 == '─' {
            let (glyph, color) = match m.message(id) {
                Some((kind, _, _)) => kind_look(kind, t),
                None => ("·", t.muted),
            };
            cells[x] = (glyph.chars().next().unwrap_or('·'), Some(color));
        }
    }
    cells
}

/// The day ruler for the timeline: `┄` across, `┤` where a day starts, and the day where it fits.
/// The lanes are all on this one axis, so it is the only place a date is written.
fn ruler_cells(
    m: &Memory,
    lo: u64,
    hi: u64,
    width: usize,
    t: &Theme,
) -> Vec<(char, Option<Color>)> {
    // The days first: a label must not be written where a later day's mark lands, and the log can
    // put two days one column apart.
    let mut days: Vec<(usize, String)> = vec![];
    let mut last = String::new();
    for id in lo..=hi {
        let Some((_, date, _)) = m.message(id) else {
            continue;
        };
        let d = &date[..date.len().min(10)];
        if d.len() < 10 || d == last {
            continue;
        }
        last = d.to_string();
        days.push((
            column(id, lo, hi, width),
            format!("{}/{}", &d[8..10], &d[5..7]),
        ));
    }
    let mut cells: Vec<(char, Option<Color>)> = (0..width).map(|_| ('┄', Some(t.line))).collect();
    for &(x, _) in &days {
        cells[x] = ('┤', Some(t.muted));
    }
    let mut placed: Option<usize> = None;
    for (x, label) in &days {
        let (lx, end) = (x + 1, x + 1 + label.chars().count());
        let clear = end <= width && days.iter().all(|&(dx, _)| dx < lx || dx >= end);
        if clear && placed.is_none_or(|p| p < lx) {
            for (k, c) in label.chars().enumerate() {
                cells[lx + k] = (c, Some(t.muted));
            }
            placed = Some(end);
        }
    }
    cells
}

pub struct MemWin {
    project: Project,
    dir: PathBuf,
    mem: Option<Memory>,
    error: String,
    levels: Vec<Level>,
    tab: Tab,
    agents: Vec<(String, bool)>,
    agent_sel: usize,
    /// Outline: the lines opened, and the line the cursor is on.
    open: std::collections::HashSet<(u32, u64)>,
    osel: Option<(u32, u64)>,
    /// Threads: the brief the cursor is on, and the message range the timeline shows. `None` is the
    /// whole log. A range is one brief's own window, which is the only way a short one is visible.
    tsel: usize,
    trange: Option<(u64, u64)>,
    /// The briefs the log names, read once per reload. A draw must not walk the log: the timeline
    /// is drawn from this, and a draw happens on every key.
    threads: Vec<memory::Thread>,
    list: ListState,
    scroll: u16,
    input: Option<(Op, String)>,
    kind_ix: usize,
    job: Option<Job>,
    note: String,
    /// Click targets from the last draw: the tabs and footer hints (each a key), the breadcrumb stops (how many
    /// levels to keep), and the list rows.
    hits: Vec<(Rect, KeyEvent)>,
    crumbs: Vec<(Rect, usize)>,
    list_area: Rect,
    last_click: Option<(Instant, u16, u16)>,
}

impl MemWin {
    pub fn new(project: Project) -> MemWin {
        let dir = memory::dir_of(&project);
        MemWin::at(project, dir)
    }

    fn at(project: Project, dir: PathBuf) -> MemWin {
        let mut w = MemWin {
            project,
            dir,
            mem: None,
            error: String::new(),
            levels: vec![],
            tab: Tab::View,
            agents: vec![],
            agent_sel: 0,
            open: std::collections::HashSet::new(),
            osel: None,
            tsel: 0,
            trange: None,
            threads: vec![],
            list: ListState::default(),
            scroll: 0,
            input: None,
            kind_ix: 0,
            job: None,
            note: String::new(),
            hits: vec![],
            crumbs: vec![],
            list_area: Rect::default(),
            last_click: None,
        };
        w.reload();
        w
    }

    /// Read the memory again. The first level follows the end of the log when it was on the last line, and
    /// otherwise keeps its line. Deeper levels never change.
    fn reload(&mut self) {
        match Memory::open(&self.dir, false) {
            Ok(m) => {
                self.error.clear();
                self.mem = Some(m);
            }
            Err(e) => self.error = e.to_string(),
        }
        self.agents = agent::agents(&self.dir);
        self.agent_sel = self.agent_sel.min(self.agents.len().saturating_sub(1));
        // The log is walked here, once a reload, and not on every draw.
        self.threads = self.mem.as_ref().map_or_else(Vec::new, Memory::threads);
        let Some(m) = &self.mem else { return };
        let fresh = m.view_lines();
        match self.levels.first_mut() {
            None => {
                let sel = fresh.len().saturating_sub(1);
                self.levels.push(Level {
                    lines: fresh.clone(),
                    sel,
                });
            }
            Some(root) => {
                let was_last = root.sel + 1 >= root.lines.len();
                let key = root.lines.get(root.sel).copied();
                root.sel = if was_last {
                    fresh.len().saturating_sub(1)
                } else {
                    key.and_then(|k| fresh.iter().position(|x| *x == k))
                        .unwrap_or(root.sel.min(fresh.len().saturating_sub(1)))
                };
                root.lines = fresh.clone();
            }
        }
        // The outline starts on the newest line, as the view does.
        if self.osel.is_none_or(|k| !fresh.contains(&k)) {
            self.osel = fresh.last().copied();
        }
    }

    pub fn tick(&mut self) {
        if let Some(job) = &self.job {
            match job.rx.try_recv() {
                Ok(done) => {
                    self.note = match done {
                        Ok(text) => text.lines().next().unwrap_or("done").to_string(),
                        Err(e) => format!("{} failed: {e}", job.what),
                    };
                    self.job = None;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.note = format!("{} ended", job.what);
                    self.job = None;
                }
            }
        }
        self.reload();
    }

    pub fn paste(&mut self, text: &str) {
        if let Some((_, s)) = &mut self.input {
            s.push_str(text.lines().next().unwrap_or(""));
        }
    }

    /// The line the window is about, whichever tab is showing.
    fn selected(&self) -> Option<(u32, u64)> {
        match self.tab {
            Tab::View => self.cur(),
            Tab::Outline => self.osel,
            Tab::Agents | Tab::Threads => None,
        }
    }

    fn cur(&self) -> Option<(u32, u64)> {
        let level = self.levels.last()?;
        level.lines.get(level.sel).copied()
    }

    /// The rows the outline shows: the view's lines, each opened into the two it was made from.
    /// `(line, depth, opened)` in the order they are drawn.
    fn outline_rows(&self) -> Vec<((u32, u64), usize, bool)> {
        // The first level is the view itself, so the outline opens exactly what the agent reads.
        let Some(root) = self.levels.first() else {
            return vec![];
        };
        let mut out = vec![];
        for &(l, i) in &root.lines {
            push_rows(&self.open, (l, i), 0, &mut out);
        }
        out
    }

    /// The message range the THREADS timeline shows: the whole log, or one brief's own window.
    fn range(&self) -> (u64, u64) {
        let last = self
            .mem
            .as_ref()
            .map_or(0, |m| m.messages().saturating_sub(1));
        match self.trange {
            Some((lo, hi)) => {
                let lo = lo.min(last);
                (lo, hi.min(last).max(lo))
            }
            None => (0, last),
        }
    }

    /// How wide a thread's window is inside the range, in messages.
    fn window(thread: &memory::Thread) -> u64 {
        thread
            .mentions
            .last()
            .unwrap_or(&thread.mentions[0])
            .saturating_sub(thread.mentions[0])
    }

    /// The briefs the timeline lists: those named inside the range, widest window first, so the rows
    /// the cell grid can show are at the top. Each thread's mentions are only the ones in the range.
    fn thread_rows(&self) -> Vec<memory::Thread> {
        let (lo, hi) = self.range();
        let mut rows: Vec<memory::Thread> = self
            .threads
            .iter()
            .filter_map(|t| {
                let mentions: Vec<u64> = t
                    .mentions
                    .iter()
                    .copied()
                    .filter(|&i| lo <= i && i <= hi)
                    .collect();
                (!mentions.is_empty()).then_some(memory::Thread {
                    name: t.name.clone(),
                    mentions,
                })
            })
            .collect();
        rows.sort_by(|a, b| {
            Self::window(b)
                .cmp(&Self::window(a))
                .then_with(|| a.mentions[0].cmp(&b.mentions[0]))
                .then_with(|| a.name.cmp(&b.name))
        });
        rows
    }

    /// The brief the THREADS tab is about.
    fn cur_thread(&self) -> Option<memory::Thread> {
        self.thread_rows().into_iter().nth(self.tsel)
    }

    fn step(&mut self, by: i32) {
        match self.tab {
            Tab::View => {
                if let Some(level) = self.levels.last_mut() {
                    let max = level.lines.len().saturating_sub(1) as i32;
                    level.sel = (level.sel as i32 + by).clamp(0, max) as usize;
                }
            }
            Tab::Outline => {
                let rows = self.outline_rows();
                let at = rows
                    .iter()
                    .position(|(k, _, _)| Some(*k) == self.osel)
                    .unwrap_or(0) as i32;
                let at = (at + by).clamp(0, rows.len().saturating_sub(1) as i32) as usize;
                self.osel = rows.get(at).map(|(k, _, _)| *k);
            }
            Tab::Agents => {
                let max = self.agents.len().saturating_sub(1) as i32;
                self.agent_sel = (self.agent_sel as i32 + by).clamp(0, max) as usize;
            }
            Tab::Threads => {
                let max = self.thread_rows().len().saturating_sub(1) as i32;
                self.tsel = (self.tsel as i32 + by).clamp(0, max) as usize;
            }
        }
        self.scroll = 0;
    }

    fn jump(&mut self, end: bool) {
        let len = match self.tab {
            Tab::View => self.levels.last().map_or(0, |l| l.lines.len()),
            Tab::Outline => self.outline_rows().len(),
            Tab::Agents => self.agents.len(),
            Tab::Threads => self.thread_rows().len(),
        };
        self.step(if end { len as i32 } else { -(len as i32) });
    }

    /// Right, and enter: open. The view zooms a level, the outline opens the line in place.
    fn forward(&mut self) {
        match self.tab {
            Tab::View => self.zoom_in(),
            Tab::Outline => {
                let Some((l, i)) = self.osel else { return };
                if l == 0 {
                    self.note = format!("message {i} is whole; nothing below it");
                    return;
                }
                self.open.insert((l, i));
                self.scroll = 0;
            }
            Tab::Agents => {}
            Tab::Threads => {
                let Some(thread) = self.cur_thread() else {
                    return;
                };
                let first = thread.mentions[0];
                let last = *thread.mentions.last().unwrap_or(&first);
                if first == last {
                    self.note = format!(
                        "{} is named in one message: it has no window",
                        thread.slug()
                    );
                    return;
                }
                self.trange = Some((first, last));
                self.tsel = 0;
                self.scroll = 0;
                self.note = format!("msgs {first}..{last} · ⌫ goes back to the whole log");
            }
        }
    }

    /// Left, and backspace: close what is open.
    fn back(&mut self) {
        match self.tab {
            Tab::View => self.zoom_out(),
            Tab::Outline => {
                if let Some((l, i)) = self.osel {
                    if !self.open.remove(&(l, i)) {
                        // Nothing open here: go back up to the line this one merges into. The
                        // outline shows the view's lines, and a parent is often not one of them,
                        // so walk up to the nearest ancestor that is on screen. Moving to a row
                        // that is not there would drop the highlight to the top of the list.
                        let shown: Vec<(u32, u64)> =
                            self.outline_rows().iter().map(|(k, _, _)| *k).collect();
                        let top = self.mem.as_ref().map_or(0, |m| m.top_level());
                        let mut up = (l + 1, i / 2);
                        while up.0 <= top {
                            if shown.contains(&up) {
                                self.osel = Some(up);
                                break;
                            }
                            up = (up.0 + 1, up.1 / 2);
                        }
                    }
                    self.scroll = 0;
                }
            }
            Tab::Agents => {}
            Tab::Threads => {
                if self.trange.take().is_some() {
                    self.tsel = 0;
                    self.scroll = 0;
                } else {
                    self.note = "the whole log is already showing".to_string();
                }
            }
        }
    }

    fn zoom_in(&mut self) {
        let Some((l, i)) = self.cur() else { return };
        if self.tab != Tab::View {
            return;
        }
        if l == 0 {
            self.note = format!("message {i} is whole; nothing below it");
            return;
        }
        self.levels.push(Level {
            lines: vec![(l - 1, 2 * i), (l - 1, 2 * i + 1)],
            sel: 0,
        });
        *self.list.offset_mut() = 0;
        self.scroll = 0;
    }

    fn zoom_out(&mut self) {
        if self.tab == Tab::View && self.levels.len() > 1 {
            self.levels.pop();
            *self.list.offset_mut() = 0;
            self.scroll = 0;
        }
    }

    /// Run `f` in a thread. One job at a time.
    fn spawn(
        &mut self,
        what: &'static str,
        f: impl FnOnce() -> Result<String, String> + Send + 'static,
    ) {
        if let Some(job) = &self.job {
            self.note = format!("{} is still running", job.what);
            return;
        }
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(f());
        });
        self.job = Some(Job {
            what,
            rx,
            since: Instant::now(),
        });
        self.note.clear();
    }

    fn run_memory(&mut self, what: &'static str) {
        let (project, dir) = (self.project.clone(), self.dir.clone());
        self.spawn(what, move || {
            memory::run_in(&dir, &project, &[what.to_string()]).map_err(|e| e.to_string())
        });
    }

    fn submit(&mut self, op: Op, text: String) {
        let text = text.trim().to_string();
        if text.is_empty() {
            return;
        }
        match op {
            Op::Add => {
                self.note = match memory::deliver(&self.dir, KINDS[self.kind_ix], &text) {
                    Ok(true) => format!("logged as {}", KINDS[self.kind_ix]),
                    Ok(false) => "queued: another process owns the log and will file it".into(),
                    Err(e) => format!("add failed: {e}"),
                };
                self.reload();
            }
            Op::Turn => match agent::command() {
                Err(e) => self.note = e.to_string(),
                Ok(cmd) => {
                    let (repo, dir) = (self.project.repo.clone(), self.dir.clone());
                    self.spawn("turn", move || {
                        let (s, _) = memory::summarizer();
                        let mut reply = String::new();
                        agent::turn(&repo, &dir, &cmd, &*s, &text, &mut |r| {
                            reply = r.to_string()
                        })
                        .map_err(|e| e.to_string())?;
                        Ok(format!("turn done: {}", reply.replace('\n', " ")))
                    });
                }
            },
            Op::Sub => {
                let Some((name, task)) = text.split_once(' ') else {
                    self.note = "type: Name task".into();
                    return;
                };
                self.note = match agent::sub(&self.project, &self.dir, name, task.trim()) {
                    Ok(m) => m,
                    Err(e) => format!("sub failed: {e}"),
                };
                self.reload();
            }
        }
    }

    fn input_key(&mut self, key: KeyEvent) {
        let Some((op, text)) = &mut self.input else {
            return;
        };
        match key.code {
            KeyCode::Esc => self.input = None,
            KeyCode::Enter => {
                let (op, text) = (*op, std::mem::take(text));
                self.input = None;
                self.submit(op, text);
            }
            KeyCode::Backspace => {
                text.pop();
            }
            KeyCode::Tab if *op == Op::Add => self.kind_ix = (self.kind_ix + 1) % KINDS.len(),
            KeyCode::Char(c) => text.push(c),
            _ => {}
        }
    }

    pub fn key(&mut self, key: KeyEvent) -> Outcome {
        if self.input.is_some() {
            self.input_key(key);
            return Outcome::Open;
        }
        self.note.clear();
        match key.code {
            KeyCode::Esc | KeyCode::Char('q' | 'm') => return Outcome::Close,
            KeyCode::Down | KeyCode::Char('j') => self.step(1),
            KeyCode::Up | KeyCode::Char('k') => self.step(-1),
            KeyCode::Char('g') | KeyCode::Home => self.jump(false),
            KeyCode::Char('G') | KeyCode::End => self.jump(true),
            KeyCode::Char('J') | KeyCode::PageDown => self.scroll = self.scroll.saturating_add(8),
            KeyCode::Char('K') | KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(8),
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => self.forward(),
            KeyCode::Backspace | KeyCode::Left | KeyCode::Char('h') => self.back(),
            KeyCode::Tab => {
                self.tab = self.tab.next();
                self.scroll = 0;
            }
            KeyCode::Char(c @ ('1' | '2' | '3' | '4')) => {
                let at = (c as u8 - b'1') as usize;
                let tab = Tab::ALL[at.min(Tab::ALL.len() - 1)];
                if self.tab != tab {
                    self.tab = tab;
                    self.scroll = 0;
                }
            }
            KeyCode::Char('a') => self.input = Some((Op::Add, String::new())),
            KeyCode::Char('t') => self.input = Some((Op::Turn, String::new())),
            KeyCode::Char('u') => self.input = Some((Op::Sub, String::new())),
            KeyCode::Char('s') => self.run_memory("sync"),
            KeyCode::Char('b') => self.run_memory("build"),
            KeyCode::Char('r') => self.reload(),
            _ => {}
        }
        Outcome::Open
    }

    fn preview(&self, t: &Theme) -> Vec<Line<'static>> {
        let muted = Style::new().fg(t.muted);
        let body = |text: &str| -> Vec<Line<'static>> {
            text.lines().map(|l| Line::from(l.to_string())).collect()
        };
        if self.tab == Tab::Agents {
            return match self.agents.get(self.agent_sel) {
                Some((name, _)) => {
                    body(&agent::chat(&self.dir, name).unwrap_or_else(|e| e.to_string()))
                }
                None => vec![Line::styled("No subagents. u starts one.", muted)],
            };
        }
        if self.tab == Tab::Threads {
            return self.thread_preview(t);
        }
        let (Some(m), Some((l, i))) = (&self.mem, self.selected()) else {
            return vec![Line::styled(
                if self.error.is_empty() {
                    "The memory is empty. a adds a message."
                } else {
                    &self.error
                }
                .to_string(),
                muted,
            )];
        };
        let (first, n) = (i << l, 1u64 << l);
        let mut out = vec![Line::styled(
            format!("{first}+{n}  messages {first}..{}", first + n - 1),
            muted,
        )];
        if let Some((vl, vi, steps)) = m.view_over(l, i) {
            let coord = format!("{}+{}", vi << vl, 1u64 << vl);
            out.push(Line::styled(
                match steps {
                    s if s > 0 => format!("the view has {coord} over it: {s} zoom(s) down"),
                    0 => format!("the view line is this one: {coord}"),
                    _ => format!("the view is already finer here: {coord}"),
                },
                Style::new().fg(t.line),
            ));
        }
        // One message as it was said: its kind, its date, then its words indented.
        let chat = |id: u64, clip: usize| -> Vec<Line<'static>> {
            let Some((kind, date, text)) = m.message(id) else {
                return vec![];
            };
            let (glyph, color) = kind_look(kind, t);
            let mut lines = vec![Line::from(vec![
                Span::styled(
                    format!("{glyph} {kind}"),
                    Style::new().fg(color).add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!("  {date}"), muted),
            ])];
            let all: Vec<&str> = text.lines().collect();
            for l in all.iter().take(clip) {
                lines.push(Line::from(format!("  {l}")));
            }
            if all.len() > clip {
                lines.push(Line::styled(
                    format!(
                        "  … {} more lines (enter opens the message whole)",
                        all.len() - clip
                    ),
                    muted,
                ));
            }
            lines
        };
        if l == 0 {
            out.push(Line::raw(""));
            out.extend(chat(i, usize::MAX));
        } else {
            let date = |id: u64| m.message(id).map(|x| x.1.to_string()).unwrap_or_default();
            out.push(Line::styled(
                format!("{} → {}", date(first), date(first + n - 1)),
                muted,
            ));
            out.push(Line::raw(""));
            match m.node(l, i) {
                Some(text) => out.extend(body(text)),
                None => out.push(Line::styled("(not summarized yet: b builds)", muted)),
            }
            out.push(Line::raw(""));
            if self.tab == Tab::Outline {
                out.push(Line::styled("it stands in for these two lines", muted));
                for half in 0..2 {
                    let (cl, ci) = (l - 1, i * 2 + half);
                    out.push(Line::from(vec![
                        Span::styled(
                            format!("{}+{}  ", ci << cl, 1u64 << cl),
                            Style::new().fg(t.text),
                        ),
                        Span::raw(
                            m.node(cl, ci)
                                .map(|x| x.chars().take(7).collect::<String>())
                                .unwrap_or_default(),
                        ),
                    ]));
                    match m.node(cl, ci) {
                        Some(text) => out.extend(body(text)),
                        None => out.push(Line::styled("  (not summarized yet)", muted)),
                    }
                }
            } else {
                out.push(Line::styled("enter opens it into its two lines", muted));
            }
            // A small line also shows the messages it stands for, as a conversation.
            if n <= CHAT_MAX {
                out.push(Line::raw(""));
                out.push(Line::styled("messages", muted));
                for id in first..first + n {
                    out.push(Line::raw(""));
                    out.extend(chat(id, 6));
                }
            }
        }
        // Where the line sits in the tree: the line it merges into, and the one it merges with.
        let near = |label: &str, l: u32, i: u64| -> Option<Line<'static>> {
            let text = m.node(l, i)?;
            let text: String = text.replace('\n', " ").chars().take(90).collect();
            Some(Line::from(vec![
                Span::styled(format!("{label:<8}"), muted),
                Span::styled(
                    format!("{}+{}  ", i << l, 1u64 << l),
                    Style::new().fg(t.accent),
                ),
                Span::raw(text),
            ]))
        };
        // Where the line sits in the tree: the line it merges into, and the one it merges with.
        let parent = near("parent", l + 1, i / 2);
        let sibling = near("sibling", l, i ^ 1);
        if parent.is_some() || sibling.is_some() {
            out.push(Line::raw(""));
            out.extend(parent);
            out.extend(sibling);
        }
        out
    }

    /// One brief, told: its mentions, and for each one the view line the log kept over it.
    ///
    /// That pairing is the thing a word search cannot give, because a search looks for words and a
    /// thread is a name. It is also the only place the window shows a message and its summary together.
    fn thread_preview(&self, t: &Theme) -> Vec<Line<'static>> {
        let muted = Style::new().fg(t.muted);
        let Some(m) = &self.mem else {
            return vec![Line::styled(
                if self.error.is_empty() {
                    "The memory is empty. a adds a message."
                } else {
                    &self.error
                }
                .to_string(),
                muted,
            )];
        };
        let Some(thread) = self.cur_thread() else {
            return vec![Line::styled(
                "No brief is named in this range.".to_string(),
                muted,
            )];
        };
        let (lo, hi) = self.range();
        let first = thread.mentions[0];
        let last = *thread.mentions.last().unwrap_or(&first);
        let when = |id: u64| m.message(id).map(|x| x.1.to_string()).unwrap_or_default();
        let one_line = |text: &str| -> String {
            let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
            match flat.chars().count() > 90 {
                true => format!("{}…", flat.chars().take(90).collect::<String>()),
                false => flat,
            }
        };
        let mut out = vec![
            Line::from(vec![
                Span::styled(
                    thread.slug().to_string(),
                    Style::new().fg(t.text).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("  {}", count(thread.mentions.len(), "mention", "mentions")),
                    muted,
                ),
            ]),
            Line::styled(
                format!(
                    "msgs {first}..{last}, first {}{}",
                    when(first),
                    if (lo, hi) == (first, last) {
                        String::new()
                    } else {
                        format!(" · the timeline shows msgs {lo}..{hi}")
                    }
                ),
                muted,
            ),
            Line::raw(""),
        ];
        for &id in &thread.mentions {
            let Some((kind, at, text)) = m.message(id) else {
                continue;
            };
            let (glyph, color) = kind_look(kind, t);
            out.push(Line::from(vec![
                Span::styled(format!("{id:<5} "), Style::new().fg(t.accent)),
                Span::styled(format!("{glyph} {kind:<5}"), Style::new().fg(color)),
                Span::styled(format!(" {at}"), muted),
            ]));
            out.push(Line::from(format!("      {}", one_line(text))));
            match m.view_over(0, id) {
                Some((vl, vi, 0)) => out.push(Line::styled(
                    format!("      the view line is this one: {}+1", vi << vl),
                    Style::new().fg(t.line),
                )),
                Some((vl, vi, steps)) if steps > 0 => out.push(Line::styled(
                    format!(
                        "      the view has {}+{} over it: {steps} zoom(s) down",
                        vi << vl,
                        1u64 << vl
                    ),
                    Style::new().fg(t.line),
                )),
                _ => {}
            }
        }
        out
    }

    /// A left click: a tab, a footer hint or a breadcrumb stop does what its key or stop says, a click on a row
    /// selects it and a second one zooms into it.
    pub fn click(&mut self, mouse: MouseEvent) -> Outcome {
        if !matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) || self.input.is_some() {
            return Outcome::Open;
        }
        let at = ratatui::layout::Position::new(mouse.column, mouse.row);
        let now = Instant::now();
        let double = self.last_click.is_some_and(|(when, c, r)| {
            c == mouse.column
                && r == mouse.row
                && now.duration_since(when) < Duration::from_millis(450)
        });
        self.last_click = if double {
            None
        } else {
            Some((now, mouse.column, mouse.row))
        };
        if let Some(&(_, event)) = self.hits.iter().find(|(r, _)| r.contains(at)) {
            return self.key(event);
        }
        if let Some(&(_, keep)) = self.crumbs.iter().find(|(r, _)| r.contains(at)) {
            if self.tab == Tab::View && keep < self.levels.len() {
                self.levels.truncate(keep.max(1));
                *self.list.offset_mut() = 0;
                self.scroll = 0;
            }
            return Outcome::Open;
        }
        let area = self.list_area;
        if area.contains(at) && mouse.row > area.y && mouse.row + 1 < area.y + area.height {
            let row = self.list.offset() + (mouse.row - area.y - 1) as usize;
            let outline = self.outline_rows();
            let picked = match self.tab {
                Tab::View => self
                    .levels
                    .last_mut()
                    .filter(|l| row < l.lines.len())
                    .map(|l| l.sel = row),
                Tab::Outline => outline.get(row).map(|(k, _, _)| self.osel = Some(*k)),
                Tab::Agents => (row < self.agents.len()).then(|| self.agent_sel = row),
                Tab::Threads => {
                    // Row 0 is the ruler, so a brief is one row below its index.
                    let len = self.thread_rows().len();
                    (row > 0 && row - 1 < len).then(|| self.tsel = row - 1)
                }
            };
            if picked.is_some() {
                self.scroll = 0;
                if double {
                    match self.tab {
                        Tab::Threads => self.forward(),
                        _ => self.zoom_in(),
                    }
                }
            }
        }
        Outcome::Open
    }

    pub fn draw(&mut self, f: &mut Frame) {
        let t = theme::get();
        f.render_widget(
            Block::new().style(Style::new().fg(t.text).bg(t.bg)),
            f.area(),
        );
        self.hits.clear();
        let [header, crumb, body, footer] = Layout::vertical([
            Constraint::Length(4),
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .areas(f.area());
        self.draw_header(f, header, t);
        self.draw_crumb(f, crumb, t);
        let [list, preview] = if body.width < 90 {
            Layout::vertical([Constraint::Fill(1), Constraint::Fill(1)]).areas(body)
        } else if self.tab == Tab::Threads {
            // A lane is the picture here, so the timeline gets the width; the preview is one brief.
            Layout::horizontal([Constraint::Fill(66), Constraint::Fill(34)]).areas(body)
        } else {
            Layout::horizontal([Constraint::Fill(42), Constraint::Fill(58)]).areas(body)
        };
        self.draw_list(f, list, t);
        let lines = self.preview(t);
        let max = lines.len().saturating_sub(1) as u16;
        self.scroll = self.scroll.min(max);
        let block = Block::bordered()
            .title(Line::from(vec![Span::styled(
                " PREVIEW ",
                Style::new().fg(t.muted).add_modifier(Modifier::BOLD),
            )]))
            .border_style(Style::new().fg(t.line))
            .style(Style::new().bg(t.surface))
            .padding(Padding::horizontal(1));
        f.render_widget(
            Paragraph::new(lines)
                .block(block)
                .wrap(Wrap { trim: false })
                .scroll((self.scroll, 0)),
            preview,
        );
        self.draw_footer(f, footer, t);
    }

    /// The way down the tree: `view › 0+1024 › 0+16`, one stop for each line zoomed into.
    fn draw_crumb(&mut self, f: &mut Frame, area: Rect, t: &Theme) {
        // Each stop is a click target: it goes back up to that line.
        self.crumbs.clear();
        self.crumbs.push((Rect::new(area.x + 1, area.y, 4, 1), 1));
        let mut x = area.x + 5;
        let mut spans = vec![Span::styled(" view", Style::new().fg(t.muted))];
        if self.tab == Tab::View {
            let stops = self.levels.len().saturating_sub(1);
            for (k, level) in self.levels.iter().take(stops).enumerate() {
                let Some(&(l, i)) = level.lines.get(level.sel) else {
                    continue;
                };
                let last = k + 1 == stops;
                let label = format!("{}+{}", i << l, 1u64 << l);
                self.crumbs.push((
                    Rect::new(x + 3, area.y, label.chars().count() as u16, 1),
                    k + 2,
                ));
                x += 3 + label.chars().count() as u16;
                spans.push(Span::styled(" › ", Style::new().fg(t.line)));
                spans.push(Span::styled(
                    label,
                    if last {
                        Style::new().fg(t.text).add_modifier(Modifier::BOLD)
                    } else {
                        Style::new().fg(t.accent)
                    },
                ));
            }
            if stops == 0 {
                spans.push(Span::styled("  the whole chat", Style::new().fg(t.line)));
            }
        }
        f.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    fn draw_header(&self, f: &mut Frame, area: Rect, t: &Theme) {
        let style = |on: bool| {
            if on {
                Style::new().fg(t.accent)
            } else {
                Style::new().fg(t.muted)
            }
        };
        let first = match &self.mem {
            Some(m) => Line::styled(format!(" {}", m.status()), Style::new().fg(t.text)),
            None => Line::styled(format!(" {}", self.error), Style::new().fg(t.bad)),
        };
        let set = |k: &str| std::env::var(k).is_ok_and(|v| !v.trim().is_empty());
        let mut second = vec![
            Span::styled(" summarizer: ", style(false)),
            Span::styled(
                if set("BRIE_SUMMARIZER") {
                    "model"
                } else {
                    "cut (BRIE_SUMMARIZER not set)"
                },
                style(set("BRIE_SUMMARIZER")),
            ),
            Span::styled("  agent: ", style(false)),
            Span::styled(
                if set("BRIE_AGENT") {
                    "set"
                } else {
                    "not set (BRIE_AGENT)"
                },
                style(set("BRIE_AGENT")),
            ),
        ];
        if let Some(job) = &self.job {
            second.push(Span::styled(
                format!("  ◐ {} {}s", job.what, job.since.elapsed().as_secs()),
                Style::new().fg(t.accent),
            ));
        } else if self.dir.join(".lock").exists() {
            second.push(Span::styled(
                "  ◐ a process owns the log",
                Style::new().fg(t.accent),
            ));
        }
        // The key to the kind glyphs, when there is room.
        if area.width >= 110 {
            second.push(Span::raw("   "));
            for (n, kind) in KINDS.iter().enumerate() {
                let (glyph, color) = kind_look(kind, t);
                let gap = if n == 0 { "" } else { " " };
                second.push(Span::styled(
                    format!("{gap}{glyph}{kind}"),
                    Style::new().fg(color),
                ));
            }
        }
        let block = Block::bordered()
            .title(Line::from(vec![
                Span::styled(
                    " MEMORY ",
                    Style::new().fg(t.accent).add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!("{} ", self.project.name), Style::new().fg(t.muted)),
            ]))
            .border_style(Style::new().fg(t.line))
            .style(Style::new().bg(t.surface));
        f.render_widget(
            Paragraph::new(vec![first, Line::from(second)]).block(block),
            area,
        );
    }

    fn draw_list(&mut self, f: &mut Frame, area: Rect, t: &Theme) {
        let tab = |label: &str, active: bool| -> Span<'static> {
            if active {
                Span::styled(
                    label.to_string(),
                    Style::new()
                        .fg(t.accent_ink)
                        .bg(t.accent)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                Span::styled(label.to_string(), Style::new().fg(t.muted))
            }
        };
        let (rows, sel, bottom): (Vec<ListItem>, usize, String) = match (self.tab, &self.mem) {
            (Tab::View, Some(m)) => {
                let level = self.levels.last();
                let lines = level.map_or(&[][..], |l| &l.lines[..]);
                let sel = level.map_or(0, |l| l.sel);
                let biggest = lines.iter().map(|&(l, _)| 1u64 << l).max().unwrap_or(1);
                let rows = lines
                    .iter()
                    .enumerate()
                    .map(|(n, &(l, i))| {
                        let line = m.line(l, i);
                        let (id, text) = line.split_once('|').unwrap_or((&line, ""));
                        let built = m.node(l, i).is_some();
                        let on = n == sel;
                        let id_style = if on {
                            Style::new()
                        } else {
                            Style::new().fg(t.accent)
                        };
                        let text_style = if on || built {
                            Style::new()
                        } else {
                            Style::new().fg(t.muted)
                        };
                        let (glyph, color) = kind_look(leading_kind(text), t);
                        let glyph_style = if on {
                            Style::new()
                        } else {
                            Style::new().fg(color)
                        };
                        ListItem::new(Line::from(vec![
                            Span::styled(format!("{glyph} "), glyph_style),
                            Span::styled(format!("{id:<9} "), id_style),
                            Span::styled(
                                format!("{} ", coverage_bar(1 << l, biggest, COVERAGE_CELLS)),
                                if on {
                                    Style::new()
                                } else {
                                    Style::new().fg(t.line)
                                },
                            ),
                            Span::styled(text.to_string(), text_style),
                        ]))
                    })
                    .collect();
                let depth = self.levels.len() - 1;
                let bottom = if depth > 0 {
                    format!(" zoom {depth} · backspace goes up · {} lines ", lines.len())
                } else {
                    format!(" {} lines ", lines.len())
                };
                (rows, sel, bottom)
            }
            (Tab::Outline, Some(m)) => {
                let rows = self.outline_rows();
                let biggest = rows
                    .iter()
                    .map(|&((l, _), _, _)| 1u64 << l)
                    .max()
                    .unwrap_or(1);
                let sel = rows
                    .iter()
                    .position(|(k, _, _)| Some(*k) == self.osel)
                    .unwrap_or(0);
                let rows = rows
                    .iter()
                    .enumerate()
                    .map(|(n, &((l, i), depth, open))| {
                        let line = m.line(l, i);
                        let (id, text) = line.split_once('|').unwrap_or((&line, ""));
                        let built = m.node(l, i).is_some();
                        let on = n == sel;
                        let (glyph, color) = kind_look(leading_kind(text), t);
                        // A line that can open says so; a message is a leaf.
                        let mark = match (l, open) {
                            (0, _) => " ",
                            (_, true) => "▾",
                            _ => "▸",
                        };
                        ListItem::new(Line::from(vec![
                            Span::raw(" ".repeat(depth.saturating_mul(2))),
                            Span::styled(
                                format!("{mark} "),
                                if on {
                                    Style::new().fg(t.accent_ink)
                                } else {
                                    Style::new().fg(t.line)
                                },
                            ),
                            Span::styled(
                                format!("{glyph} "),
                                if on {
                                    Style::new().fg(t.accent_ink)
                                } else {
                                    Style::new().fg(color)
                                },
                            ),
                            Span::styled(
                                format!("{id:<9} "),
                                if on {
                                    Style::new().fg(t.accent_ink)
                                } else {
                                    Style::new().fg(t.accent)
                                },
                            ),
                            Span::styled(
                                format!("{} ", coverage_bar(1 << l, biggest, COVERAGE_CELLS)),
                                if on {
                                    Style::new().fg(t.accent_ink)
                                } else {
                                    Style::new().fg(t.line)
                                },
                            ),
                            Span::styled(
                                text.to_string(),
                                if on || built {
                                    Style::new()
                                } else {
                                    Style::new().fg(t.muted)
                                },
                            ),
                        ]))
                    })
                    .collect::<Vec<ListItem>>();
                let right = if !self.open.is_empty() {
                    format!(
                        " {} lines, {} open · enter opens in place ",
                        rows.len(),
                        self.open.len()
                    )
                } else {
                    format!(" {} lines over the whole chat ", rows.len())
                };
                (rows, sel, right)
            }
            (Tab::Agents, _) => {
                let rows = self
                    .agents
                    .iter()
                    .enumerate()
                    .map(|(n, (name, done))| {
                        let (glyph, color) = if *done {
                            ("●", t.text)
                        } else {
                            ("◐", t.accent)
                        };
                        let g = if n == self.agent_sel {
                            Style::new()
                        } else {
                            Style::new().fg(color)
                        };
                        ListItem::new(Line::from(vec![
                            Span::styled(format!("{glyph} "), g),
                            Span::raw(name.clone()),
                        ]))
                    })
                    .collect();
                (
                    rows,
                    self.agent_sel,
                    format!(" {} subagents ", self.agents.len()),
                )
            }
            (Tab::Threads, Some(m)) => {
                let (lo, hi) = self.range();
                let inner = area.width.saturating_sub(2) as usize;
                let lane = inner.saturating_sub(SLUG_CELLS + 1).max(1);
                let rows = self.thread_rows();
                let sel = self.tsel.min(rows.len().saturating_sub(1));
                // The ruler is the first row, so it is on screen however long the list is; no brief
                // is ever on row 0, so the highlight cannot land on it.
                let mut items: Vec<ListItem> = vec![ListItem::new(Line::from({
                    let mut spans = vec![Span::styled(
                        format!("{:<w$} ", "days", w = SLUG_CELLS),
                        Style::new().fg(t.muted),
                    )];
                    spans.extend(spans_of(&ruler_cells(m, lo, hi, lane, t), false));
                    spans
                }))];
                items.extend(rows.iter().enumerate().map(|(n, thread)| {
                    let on = n == sel;
                    let slug: String = thread.slug().chars().take(SLUG_CELLS).collect();
                    let mut spans = vec![Span::styled(
                        format!("{slug:<w$} ", w = SLUG_CELLS),
                        if on {
                            Style::new()
                        } else {
                            Style::new().fg(t.text)
                        },
                    )];
                    spans.extend(spans_of(&lane_cells(m, thread, lo, hi, lane, t), on));
                    ListItem::new(Line::from(spans))
                }));
                // How many of the briefs on screen were open at the busiest column: the knot a
                // force layout draws, as a count.
                let busiest = (0..lane)
                    .map(|x| {
                        rows.iter()
                            .filter(|thread| {
                                let (a, b) = (
                                    column(thread.mentions[0], lo, hi, lane),
                                    column(
                                        *thread.mentions.last().unwrap_or(&thread.mentions[0]),
                                        lo,
                                        hi,
                                        lane,
                                    ),
                                );
                                a <= x && x <= b
                            })
                            .count()
                    })
                    .max()
                    .unwrap_or(0);
                let mentions: usize = rows.iter().map(|t| t.mentions.len()).sum();
                let briefs = count(rows.len(), "brief", "briefs");
                let mentions = count(mentions, "mention", "mentions");
                let bottom = if rows.is_empty() {
                    " no brief is named in the log ".to_string()
                } else if self.trange.is_some() {
                    format!(" msgs {lo}..{hi} · {briefs} · {mentions} · ⌫ the whole log ")
                } else {
                    format!(" {briefs} · {mentions} · busiest: {busiest} open at once ")
                };
                (items, sel + 1, bottom)
            }
            _ => (vec![], 0, String::new()),
        };
        // The tabs are click targets, like the keys 1 to 4; so are the rows.
        let mut titles: Vec<Span> = vec![];
        let mut x = area.x + 1;
        for (n, each) in Tab::ALL.iter().enumerate() {
            let label = format!(" {} {} ", n + 1, each.label().trim());
            let width = label.chars().count() as u16;
            self.hits.push((
                Rect::new(x, area.y, width, 1),
                KeyEvent::new(each.key(), KeyModifiers::NONE),
            ));
            x += width + 1;
            if n > 0 {
                titles.push(Span::raw(" "));
            }
            titles.push(tab(&label, self.tab == *each));
        }
        self.list_area = area;
        // The THREADS list has the day ruler as its first row, and that row is never selected, so
        // ratatui would never scroll back up to it. Set the offset here instead: the ruler returns
        // when the list is near the top, and the selection stays on screen when it is not. Every
        // other tab's rows are all selectable, so ratatui can reach row 0 by itself.
        if self.tab == Tab::Threads {
            let visible = area.height.saturating_sub(2).max(1) as usize;
            *self.list.offset_mut() = if sel < visible { 0 } else { sel + 1 - visible };
        }
        let block = Block::bordered()
            .title(Line::from(titles))
            .title_bottom(Line::styled(bottom, Style::new().fg(t.muted)))
            .border_style(Style::new().fg(t.accent))
            .style(Style::new().bg(t.surface));
        self.list.select(Some(sel));
        f.render_stateful_widget(
            List::new(rows).block(block).highlight_style(
                Style::new()
                    .fg(t.accent_ink)
                    .bg(t.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            area,
            &mut self.list,
        );
    }

    fn draw_footer(&mut self, f: &mut Frame, area: Rect, t: &Theme) {
        let key = |k: &'static str, what: &'static str| {
            [
                Span::styled(k, Style::new().fg(t.accent)),
                Span::styled(format!(" {what}  "), Style::new().fg(t.muted)),
            ]
        };
        let line = if let Some((op, text)) = &self.input {
            let label = match op {
                Op::Add => format!("add {} (tab: kind)", KINDS[self.kind_ix]),
                Op::Turn => "turn".to_string(),
                Op::Sub => "subagent: Name task".to_string(),
            };
            Line::from(vec![
                Span::styled(
                    format!(" {label} "),
                    Style::new().fg(t.accent_ink).bg(t.accent),
                ),
                Span::styled(format!(" {text}"), Style::new().fg(t.text)),
                Span::styled("█", Style::new().fg(t.accent)),
            ])
        } else if !self.note.is_empty() {
            Line::styled(format!(" {}", self.note), Style::new().fg(t.accent))
        } else {
            let mut spans = vec![Span::raw(" ")];
            let mut x = area.x + 1;
            let plain = |c| KeyEvent::new(c, KeyModifiers::NONE);
            // What the keys do depends on the view: say so, rather than repeat one list everywhere.
            let keys: Vec<(&'static str, &'static str, KeyCode)> = match self.tab {
                Tab::View => vec![
                    ("enter", "zoom in", KeyCode::Enter),
                    ("⌫", "out", KeyCode::Backspace),
                ],
                Tab::Outline => vec![
                    ("enter", "open", KeyCode::Enter),
                    ("⌫", "close", KeyCode::Backspace),
                    ("← →", "open/close", KeyCode::Right),
                ],
                Tab::Threads => vec![
                    ("enter", "window", KeyCode::Enter),
                    ("⌫", "back", KeyCode::Backspace),
                ],
                Tab::Agents => vec![],
            }
            .into_iter()
            .chain([
                ("1-4", "tabs", KeyCode::Tab),
                ("a", "add", KeyCode::Char('a')),
                ("t", "turn", KeyCode::Char('t')),
                ("u", "subagent", KeyCode::Char('u')),
                ("s", "sync", KeyCode::Char('s')),
                ("b", "build", KeyCode::Char('b')),
                ("r", "reload", KeyCode::Char('r')),
                ("esc", "close", KeyCode::Esc),
            ])
            .collect();
            for (k, what, code) in keys {
                let width = (k.chars().count() + what.chars().count() + 3) as u16;
                self.hits
                    .push((Rect::new(x, area.y, width, 1), plain(code)));
                x += width;
                spans.extend(key(k, what));
            }
            Line::from(spans)
        };
        f.render_widget(Paragraph::new(line), area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Res;
    use ratatui::crossterm::event::KeyModifiers;

    struct Fake;
    impl memory::Summarizer for Fake {
        fn run(&self, _prompt: &str, parts: &[&str]) -> Res<String> {
            Ok(parts.join(" | ").chars().take(100).collect())
        }
    }

    /// The window as text, one line per row.
    fn screen(w: &mut MemWin, cols: u16, rows: u16) -> String {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut term = Terminal::new(TestBackend::new(cols, rows)).unwrap();
        term.draw(|f| w.draw(f)).unwrap();
        let buf = term.backend().buffer();
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn preview_text(w: &MemWin) -> String {
        w.preview(theme::get())
            .iter()
            .map(|l| l.to_string() + "\n")
            .collect()
    }

    fn press(w: &mut MemWin, code: KeyCode) {
        w.key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    fn typed(w: &mut MemWin, text: &str) {
        for c in text.chars() {
            press(w, KeyCode::Char(c));
        }
    }

    fn window(name: &str, messages: usize) -> MemWin {
        let dir = std::env::temp_dir().join(format!("brie-memwin-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut m = Memory::open(&dir, true).unwrap();
        for i in 0..messages {
            m.append("user", &format!("message {i}")).unwrap();
        }
        m.build(&Fake).unwrap();
        m.maintain().unwrap();
        drop(m);
        let project = Project {
            name: name.into(),
            repo: dir.clone(),
        };
        MemWin::at(project, dir)
    }

    /// A memory that names one brief three times, another once, and carries a code fragment in
    /// brackets that is not a brief at all. The kinds differ, so the lanes' colors can be checked.
    fn thread_window(name: &str) -> MemWin {
        let dir = std::env::temp_dir().join(format!("brie-memwin-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut m = Memory::open(&dir, true).unwrap();
        let demo = [
            ("user", "message 0"),
            ("work", "[01_10_2026_09:00_retry-payment] status: working"),
            ("user", "message 2"),
            ("user", "message 3: [N-2] a code fragment in brackets"),
            (
                "agent",
                "message 4: 01_10_2026_09:00_retry-payment is under way",
            ),
            ("user", "message 5: 10_10_2026_08:00_once-only"),
            ("user", "message 6"),
            ("user", "message 7"),
            ("note", "[01_10_2026_09:00_retry-payment] report: done"),
            ("user", "message 9"),
        ];
        for (kind, text) in demo {
            m.append(kind, text).unwrap();
        }
        m.build(&Fake).unwrap();
        m.maintain().unwrap();
        drop(m);
        let project = Project {
            name: name.into(),
            repo: dir.clone(),
        };
        let mut w = MemWin::at(project, dir);
        w.tab = Tab::Threads;
        w
    }

    #[test]
    fn a_lane_runs_from_the_first_mention_to_the_last() {
        let w = thread_window("lane");
        let m = w.mem.as_ref().unwrap();
        let thread = w.cur_thread().unwrap();
        assert_eq!(thread.name, "01_10_2026_09:00_retry-payment");
        assert_eq!(thread.mentions, vec![1, 4, 8]);
        let t = theme::get();
        let cells = lane_cells(m, &thread, 0, 9, 10, t);
        let chars: String = cells.iter().map(|(c, _)| *c).collect();
        // The window is the two ends; the mention between them is a tick of its own kind.
        assert_eq!(chars, " ├──◆───┤ ");
        // Each end is the first and last mention, so it carries that message's kind color.
        assert_eq!(
            cells[1].1,
            Some(t.info),
            "the first mention is a work message"
        );
        assert_eq!(cells[4].1, Some(t.text), "the tick is the agent message");
        assert_eq!(cells[8].1, Some(t.ok), "the last mention is a note");
        // A brief named once is one column, and says so rather than pretending to be a window.
        let once = w
            .thread_rows()
            .into_iter()
            .find(|t| t.slug() == "once-only")
            .unwrap();
        let chars: String = lane_cells(m, &once, 0, 9, 10, t)
            .iter()
            .map(|(c, _)| *c)
            .collect();
        assert_eq!(chars, "     ┼    ");
    }

    #[test]
    fn the_threads_tab_lists_the_briefs_and_not_the_code_fragments() {
        let mut w = thread_window("threads-list");
        let text = screen(&mut w, 120, 24);
        assert!(text.contains("4 THREADS"), "the tab is named: {text}");
        assert!(
            text.contains("retry-payment"),
            "the brief is a lane: {text}"
        );
        assert!(
            text.contains("once-only"),
            "and so is the brief named once: {text}"
        );
        assert!(
            !text.contains("N-2"),
            "a code fragment in brackets is not a thread: {text}"
        );
        assert!(
            text.contains("2 briefs · 4 mentions"),
            "both are counted: {text}"
        );
        assert!(
            text.contains("busiest: "),
            "the whole log says how busy it was: {text}"
        );
    }

    #[test]
    fn enter_narrows_the_timeline_to_one_briefs_window() {
        let mut w = thread_window("threads-zoom");
        let whole = screen(&mut w, 120, 24);
        assert!(
            whole.contains("busiest: "),
            "the whole log to start: {whole}"
        );
        press(&mut w, KeyCode::Enter);
        let zoomed = screen(&mut w, 120, 24);
        assert!(
            zoomed.contains("msgs 1..8 · 2 briefs · 4 mentions · ⌫ the whole log"),
            "the range is the brief's own window, and the other brief open in it: {zoomed}"
        );
        assert!(
            zoomed.contains("├") && zoomed.contains("┤"),
            "and the lane spans it: {zoomed}"
        );
        press(&mut w, KeyCode::Backspace);
        let back = screen(&mut w, 120, 24);
        assert!(
            back.contains("busiest: "),
            "⌫ returns to the whole log: {back}"
        );
        // A brief named once has no window to open, and the window says so instead of pretending.
        press(&mut w, KeyCode::Char('j'));
        press(&mut w, KeyCode::Enter);
        assert!(
            screen(&mut w, 120, 24).contains("has no window"),
            "a single mention is not a window"
        );
    }

    #[test]
    fn the_threads_preview_pairs_a_mention_with_the_view_line_over_it() {
        let w = thread_window("threads-preview");
        let text = preview_text(&w);
        assert!(text.contains("retry-payment"), "{text}");
        assert!(text.contains("3 mentions"), "{text}");
        assert!(
            text.contains("◐ work") && text.contains("◆ agent") && text.contains("✎ note"),
            "{text}"
        );
        assert!(
            text.contains("message 4: 01_10_2026_09:00_retry-payment is under way"),
            "{text}"
        );
        // Every mention says which view line the log kept over it, which is what a word search cannot.
        assert_eq!(
            text.matches("the view line is this one:").count(),
            3,
            "one line of evidence for each mention: {text}"
        );
        // A range the user is not zoomed into says what the timeline is showing.
        assert!(text.contains("the timeline shows msgs 0..9"), "{text}");
    }

    #[test]
    fn the_ruler_marks_every_day_and_never_cuts_a_label() {
        let dir = std::env::temp_dir().join(format!("brie-memwin-ruler-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("main")).unwrap();
        // Six days, two messages each, written by hand because `append` dates with today.
        let days = [
            "2026-10-08",
            "2026-10-09",
            "2026-10-10",
            "2026-10-11",
            "2026-10-12",
            "2026-10-13",
        ];
        let mut log = String::new();
        for (d, day) in days.iter().enumerate() {
            for k in 0..2 {
                log.push_str(&format!(
                    "{{\"i\":{},\"kind\":\"user\",\"text\":\"message {d}-{k}\",\"size\":8,\
                     \"date\":\"{day} 09:00\"}}\n",
                    d * 2 + k
                ));
            }
        }
        std::fs::write(dir.join("main/2026-10-08.jsonl"), log).unwrap();
        let m = Memory::open(&dir, false).unwrap();
        let t = theme::get();
        // 12 messages over 40 cells: the days are 7 columns apart, so the labels nearly touch.
        let cells = ruler_cells(&m, 0, 11, 40, t);
        let text: String = cells.iter().map(|(c, _)| *c).collect();
        assert_eq!(
            text.matches('┤').count(),
            6,
            "one mark for each day: {text}"
        );
        for label in ["08/10", "09/10", "10/10", "11/10", "12/10", "13/10"] {
            if text.contains(label) {
                continue;
            }
            // A label that is absent is absent whole: a mark landed on it instead of through it.
            let cut: String = label.chars().take(label.chars().count() - 1).collect();
            assert!(!text.contains(&cut), "{label} is cut by a mark: {text}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_log_that_names_no_brief_says_so() {
        // `window` builds plain messages, so no brief name is anywhere in the log.
        let mut w = window("threads-empty", 6);
        w.tab = Tab::Threads;
        let text = screen(&mut w, 120, 24);
        assert!(
            text.contains("no brief is named in the log"),
            "an empty threads tab says so: {text}"
        );
        assert!(
            preview_text(&w).contains("No brief is named in this range."),
            "and the preview does too"
        );
        // Enter on an empty list does nothing rather than panicking.
        press(&mut w, KeyCode::Enter);
        press(&mut w, KeyCode::Backspace);
        assert_eq!(w.tsel, 0);
    }

    /// A log whose timeline has more rows than a small window shows: `briefs` briefs, each named in
    /// two of the three messages it spans.
    fn many_threads(name: &str, briefs: usize) -> MemWin {
        let dir = std::env::temp_dir().join(format!("brie-memwin-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut m = Memory::open(&dir, true).unwrap();
        for i in 0..briefs * 3 {
            let b = i / 3;
            let text = match i % 3 {
                2 => format!("message {i}"),
                _ => format!("message {i} 01_10_2026_09:{b:02}_brief-{b:02}"),
            };
            m.append("user", &text).unwrap();
        }
        m.build(&Fake).unwrap();
        m.maintain().unwrap();
        drop(m);
        let project = Project {
            name: name.into(),
            repo: dir.clone(),
        };
        let mut w = MemWin::at(project, dir);
        w.tab = Tab::Threads;
        w
    }

    #[test]
    fn the_ruler_comes_back_when_the_threads_list_returns_to_the_top() {
        // 40 briefs, and a 14-row window: the list must scroll to reach them all.
        let mut w = many_threads("ruler-return", 40);
        // The view tab opens scrolled to its newest line, which leaves the list's offset high.
        press(&mut w, KeyCode::Char('1'));
        let _ = screen(&mut w, 100, 14);
        press(&mut w, KeyCode::Char('4'));
        let opened = screen(&mut w, 100, 14);
        assert!(
            opened.contains("days"),
            "the ruler is on screen when the tab opens: {opened}"
        );
        for _ in 0..6 {
            press(&mut w, KeyCode::Char('j'));
        }
        let scrolled = screen(&mut w, 100, 14);
        assert!(
            !scrolled.contains("days"),
            "it scrolls off with the list: {scrolled}"
        );
        for _ in 0..20 {
            press(&mut w, KeyCode::Char('k'));
        }
        let back = screen(&mut w, 100, 14);
        assert!(back.contains("days"), "and comes back at the top: {back}");
        assert_eq!(w.tsel, 0, "the cursor is on the first brief again");
        // `G` then `g` is the same promise from the other end.
        press(&mut w, KeyCode::Char('G'));
        press(&mut w, KeyCode::Char('g'));
        assert!(
            screen(&mut w, 100, 14).contains("days"),
            "g returns to the ruler too"
        );
    }

    #[test]
    fn the_thread_list_is_read_once_a_reload_and_not_once_a_draw() {
        let w = thread_window("threads-cache");
        assert_eq!(w.threads.len(), 2, "a reload fills the list");
        assert_eq!(w.thread_rows().len(), 2);
        // The rows are the cached list filtered, not a fresh walk of the log: take one away and
        // the timeline loses it without the memory changing at all.
        let mut w = w;
        w.threads.retain(|t| t.slug() == "once-only");
        let rows = w.thread_rows();
        assert_eq!(rows.len(), 1, "the rows come from the cached list");
        assert_eq!(rows[0].slug(), "once-only");
    }

    #[test]
    fn starts_on_the_newest_line_and_zooms_in_and_out() {
        let mut w = window("zoom", 8);
        // 8 messages, all short: the view is the 8 message lines, and the newest is selected.
        assert_eq!(w.cur(), Some((0, 7)));
        press(&mut w, KeyCode::Enter);
        assert!(
            w.note.contains("whole"),
            "a message cannot be zoomed: {}",
            w.note
        );
        // Make a coarser view by hand: one node covering messages 0..4.
        w.levels[0] = Level {
            lines: vec![(2, 0), (2, 1)],
            sel: 0,
        };
        press(&mut w, KeyCode::Enter);
        assert_eq!(w.levels.len(), 2);
        assert_eq!(w.levels[1].lines, vec![(1, 0), (1, 1)]);
        press(&mut w, KeyCode::Char('j'));
        assert_eq!(w.cur(), Some((1, 1)));
        press(&mut w, KeyCode::Enter);
        assert_eq!(w.levels[2].lines, vec![(0, 2), (0, 3)]);
        press(&mut w, KeyCode::Backspace);
        press(&mut w, KeyCode::Backspace);
        press(&mut w, KeyCode::Backspace);
        assert_eq!(w.levels.len(), 1, "the first level stays");
        let text: String = w
            .preview(theme::get())
            .iter()
            .map(|l| l.to_string() + "\n")
            .collect();
        assert!(text.contains("messages 0..3"), "{text}");
        assert!(text.contains("message 0"), "{text}");
    }

    #[test]
    fn the_first_level_follows_new_messages_only_from_the_end() {
        let mut w = window("follow", 3);
        assert_eq!(w.cur(), Some((0, 2)));
        memory::deliver(&w.dir, "note", "later").unwrap();
        w.reload();
        assert_eq!(w.cur(), Some((0, 3)), "was on the last line, so it follows");
        press(&mut w, KeyCode::Char('g'));
        memory::deliver(&w.dir, "note", "even later").unwrap();
        w.reload();
        assert_eq!(w.cur(), Some((0, 0)), "was on the first line, so it stays");
        press(&mut w, KeyCode::Char('G'));
        assert_eq!(w.cur(), Some((0, 4)));
    }

    #[test]
    fn a_adds_a_message_with_the_chosen_kind() {
        let mut w = window("add", 1);
        press(&mut w, KeyCode::Char('a'));
        press(&mut w, KeyCode::Tab); // user -> agent
        press(&mut w, KeyCode::Tab); // agent -> tool
        press(&mut w, KeyCode::Tab); // tool -> echo
        press(&mut w, KeyCode::Tab); // echo -> work
        press(&mut w, KeyCode::Tab); // work -> note
        typed(&mut w, "remember this");
        press(&mut w, KeyCode::Backspace);
        press(&mut w, KeyCode::Enter);
        assert!(w.note.contains("logged as note"), "{}", w.note);
        let m = Memory::open(&w.dir, false).unwrap();
        assert!(m.since(0).contains("note"));
        assert!(m.since(0).contains("remember thi\n"));
        // Esc cancels an input without logging.
        press(&mut w, KeyCode::Char('a'));
        typed(&mut w, "nope");
        press(&mut w, KeyCode::Esc);
        assert!(w.input.is_none());
        assert_eq!(Memory::open(&w.dir, false).unwrap().len(), 2);
    }

    #[test]
    fn s_runs_a_sync_in_a_thread_and_reports_back() {
        let mut w = window("job", 2);
        // `sync` needs an onboarded project; here it fails, and the window says so instead of dying.
        press(&mut w, KeyCode::Char('b'));
        assert!(w.job.is_some());
        press(&mut w, KeyCode::Char('b'));
        assert!(w.note.contains("still running") || w.job.is_none());
        for _ in 0..50 {
            w.tick();
            if w.job.is_none() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert!(w.job.is_none());
        assert!(!w.note.is_empty());
    }

    #[test]
    fn tab_lists_subagents_and_esc_closes() {
        let mut w = window("agents", 1);
        let ad = w.dir.join("agents").join("Scout");
        std::fs::create_dir_all(&ad).unwrap();
        std::fs::write(ad.join("task.md"), "find it").unwrap();
        std::fs::write(ad.join("chat.log"), "agent: looking\n\n").unwrap();
        w.reload();
        for want in [Tab::Outline, Tab::Agents] {
            press(&mut w, KeyCode::Tab);
            assert_eq!(w.tab, want, "tab cycles through every view");
        }
        press(&mut w, KeyCode::Char('1'));
        assert_eq!(w.tab, Tab::View);
        press(&mut w, KeyCode::Char('3'));
        assert_eq!(w.tab, Tab::Agents, "and a digit goes straight there");
        let text: String = w
            .preview(theme::get())
            .iter()
            .map(|l| l.to_string() + "\n")
            .collect();
        assert!(
            text.contains("Scout (running)") && text.contains("agent: looking"),
            "{text}"
        );
        assert!(matches!(
            w.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            Outcome::Close
        ));
    }

    #[test]
    fn the_outline_opens_a_line_into_the_two_it_stands_for() {
        let mut w = window("outline", 8);
        // By hand, a view with two coarse lines over 8 messages: 0..3 and 4..7.
        w.levels[0] = Level {
            lines: vec![(2, 0), (2, 1)],
            sel: 0,
        };
        w.osel = Some((2, 1));
        press(&mut w, KeyCode::Char('2'));
        assert_eq!(w.tab, Tab::Outline);
        assert_eq!(w.outline_rows().len(), 2, "closed: one row per view line");
        assert_eq!(w.outline_rows()[0], ((2, 0), 0, false));
        assert_eq!(
            w.outline_rows()[1].1,
            0,
            "the view's lines are not indented"
        );

        press(&mut w, KeyCode::Char('k'));
        assert_eq!(w.osel, Some((2, 0)), "the cursor walks the rows");
        press(&mut w, KeyCode::Enter);
        let rows = w.outline_rows();
        assert_eq!(rows.len(), 4, "open: the two it stands for appear under it");
        assert_eq!(rows[1].0, (1, 0));
        assert_eq!(rows[1].1, 1, "and they are indented");
        assert!(!rows[1].2, "closed until asked");
        let text = preview_text(&w);
        assert!(
            text.contains("it stands in for these two lines"),
            "the inspector compares the line with its halves: {text}"
        );

        // A half opens in turn, and closes with it.
        press(&mut w, KeyCode::Char('j'));
        assert_eq!(w.osel, Some((1, 0)), "down goes into the half");
        press(&mut w, KeyCode::Enter);
        assert_eq!(w.outline_rows().len(), 6);
        press(&mut w, KeyCode::Backspace);
        assert_eq!(w.outline_rows().len(), 4, "the half closes");
        // With nothing open under it, back goes up to the line this one merges into.
        press(&mut w, KeyCode::Backspace);
        assert_eq!(w.osel, Some((2, 0)), "up to the line it merges into");
        press(&mut w, KeyCode::Backspace);
        assert_eq!(w.outline_rows().len(), 2, "and the view is as it was");

        // It is drawn: the markers, the ids and the indent are all on screen.
        let text = screen(&mut w, 100, 24);
        assert!(text.contains("OUTLINE"), "{text}");
        assert!(
            text.contains("0+4") && text.contains('▸'),
            "the marker and the id are both on screen: {text}"
        );
    }

    #[test]
    fn back_with_nothing_open_goes_up_only_to_a_row_that_is_on_screen() {
        let mut w = window("outline-up", 8);
        // A view that mixes levels, as a real one does: messages 0..3, 4..5, 6..7.
        w.levels[0] = Level {
            lines: vec![(2, 0), (1, 2), (1, 3)],
            sel: 1,
        };
        w.osel = Some((1, 2));
        press(&mut w, KeyCode::Char('2'));
        let shown = |w: &MemWin| -> Vec<(u32, u64)> {
            w.outline_rows().iter().map(|(k, _, _)| *k).collect()
        };
        assert_eq!(shown(&w), vec![(2, 0), (1, 2), (1, 3)]);
        // (1,2) merges into (2,1), and this view does not show (2,1): the cursor must stay on a
        // row that exists rather than move to one that is not in the list.
        press(&mut w, KeyCode::Backspace);
        assert_eq!(
            w.osel,
            Some((1, 2)),
            "the cursor stays on a row that exists"
        );
        // A parent that is a view line is still reached: open (1,2), step into its half, close.
        press(&mut w, KeyCode::Enter);
        assert_eq!(shown(&w), vec![(2, 0), (1, 2), (0, 4), (0, 5), (1, 3)]);
        press(&mut w, KeyCode::Char('j'));
        assert_eq!(w.osel, Some((0, 4)), "down goes into the half");
        press(&mut w, KeyCode::Backspace);
        assert_eq!(
            w.osel,
            Some((1, 2)),
            "and back goes up to the line it merges into"
        );
        assert!(
            shown(&w).contains(&w.osel.unwrap()),
            "which is a row of the list"
        );
    }

    #[test]
    fn the_footer_says_what_the_keys_do_on_this_tab() {
        let mut w = window("footer", 8);
        let footer = |w: &mut MemWin| {
            screen(w, 130, 24)
                .lines()
                .last()
                .unwrap_or_default()
                .to_string()
        };
        let text = footer(&mut w);
        assert!(text.contains("zoom in"), "the view zooms: {text}");
        press(&mut w, KeyCode::Char('2'));
        let text = footer(&mut w);
        assert!(text.contains("open"), "the outline opens: {text}");
        assert!(text.contains("close"), "{text}");
        press(&mut w, KeyCode::Char('3'));
        let text = footer(&mut w);
        assert!(
            text.contains("esc"),
            "the agents tab lists the window's keys: {text}"
        );
        assert!(
            !text.contains("zoom in"),
            "and not what another tab does: {text}"
        );
        // Every key the window answers to is in the footer.
        for key in ["1-4", "a", "t", "u", "s", "b", "r", "esc"] {
            assert!(text.contains(key), "the footer names {key}: {text}");
        }
        press(&mut w, KeyCode::Char('4'));
        let text = footer(&mut w);
        assert!(
            text.contains("window") && text.contains("back"),
            "the threads tab names its own keys: {text}"
        );
        press(&mut w, KeyCode::Char('1'));
        let narrow = screen(&mut w, 100, 24);
        let text = narrow.lines().last().unwrap_or_default();
        // The footer is longer than a hundred columns, so this checks the keys that far reaches
        // rather than the whole line. See the THREADS section of docs/memory.md.
        assert!(
            text.contains("esc") && text.contains("subagent"),
            "the keys a hundred columns reach are on screen: {text}"
        );
    }

    #[test]
    fn the_preview_shows_what_a_small_line_stands_for() {
        let mut w = window("chat", 8);
        w.levels[0] = Level {
            lines: vec![(1, 3)],
            sel: 0,
        };
        let t = theme::get();
        let text: String = w
            .preview(t)
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("messages"), "{text}");
        assert!(text.contains("▌ user"), "{text}");
        assert!(
            text.contains("  message 6") && text.contains("  message 7"),
            "{text}"
        );
        // A single message is shown the same way, with no summary above it.
        w.levels[0] = Level {
            lines: vec![(0, 7)],
            sel: 0,
        };
        let one = w.preview(t);
        assert!(
            one.iter()
                .any(|l| l.spans.iter().any(|s| s.content.contains("▌ user")))
        );
    }

    #[test]
    fn the_coverage_bar_is_relative_to_the_biggest_line() {
        assert_eq!(coverage_bar(8, 8, 5), "█████");
        assert_eq!(coverage_bar(4, 8, 4), "██  ");
        // Never empty, however small the line.
        assert_eq!(coverage_bar(1, 1024, 5), "▏    ");
        assert_eq!(coverage_bar(1, 1, 5), "█████");
    }

    #[test]
    fn the_breadcrumb_names_each_line_zoomed_into() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut w = window("crumb", 8);
        let screen = |w: &mut MemWin| {
            let mut term = Terminal::new(TestBackend::new(100, 24)).unwrap();
            term.draw(|f| w.draw(f)).unwrap();
            let buf = term.backend().buffer();
            (0..buf.area.height)
                .map(|y| {
                    (0..buf.area.width)
                        .map(|x| buf[(x, y)].symbol().to_string())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        assert!(screen(&mut w).contains("the whole chat"));
        w.levels[0] = Level {
            lines: vec![(2, 0), (2, 1)],
            sel: 1,
        };
        press(&mut w, KeyCode::Enter);
        let text = screen(&mut w);
        assert!(text.contains("view › 4+4"), "{text}");
        press(&mut w, KeyCode::Enter);
        let text = screen(&mut w);
        assert!(
            text.contains("view › 4+4 › 4+2") || text.contains("› 6+2"),
            "{text}"
        );
    }

    #[test]
    fn tabs_rows_footer_and_breadcrumb_are_click_targets() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let click = |column, row| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        };
        let mut w = window("clicks", 8);
        w.levels[0] = Level {
            lines: vec![(2, 0), (2, 1)],
            sel: 0,
        };
        let mut term = Terminal::new(TestBackend::new(100, 24)).unwrap();
        let mut draw = |w: &mut MemWin| {
            term.draw(|f| w.draw(f)).unwrap();
        };
        draw(&mut w);
        // A click on a row selects it; a second one zooms in.
        let list = w.list_area;
        w.click(click(list.x + 4, list.y + 2));
        assert_eq!(w.levels[0].sel, 1);
        w.click(click(list.x + 4, list.y + 2));
        assert_eq!(w.levels.len(), 2);
        draw(&mut w);
        // The breadcrumb's `view` goes back up.
        let crumb = w.crumbs[0].0;
        w.click(click(crumb.x, crumb.y));
        assert_eq!(w.levels.len(), 1);
        draw(&mut w);
        // The AGENTS tab switches the tab; the footer hint `a` opens the add prompt.
        let agents = w
            .hits
            .iter()
            .find(|(_, e)| e.code == KeyCode::Char('3'))
            .map(|(r, _)| *r)
            .unwrap();
        w.click(click(agents.x + 1, agents.y));
        assert!(w.tab == Tab::Agents);
        let footer = w
            .hits
            .iter()
            .find(|(_, e)| e.code == KeyCode::Char('a'))
            .unwrap()
            .0;
        w.click(click(footer.x, footer.y));
        assert!(w.input.is_some());
        // The footer's esc hint closes the window.
        w.input = None;
        draw(&mut w);
        let esc = w
            .hits
            .iter()
            .find(|(_, e)| e.code == KeyCode::Esc)
            .unwrap()
            .0;
        assert!(matches!(w.click(click(esc.x, esc.y)), Outcome::Close));
    }

    #[test]
    fn draws_at_wide_and_narrow_sizes() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut w = window("draw", 5);
        for (cols, rows) in [(120, 30), (60, 20), (20, 6)] {
            let mut term = Terminal::new(TestBackend::new(cols, rows)).unwrap();
            term.draw(|f| w.draw(f)).unwrap();
            let screen: String = term
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|c| c.symbol())
                .collect();
            if cols >= 60 {
                assert!(
                    screen.contains("MEMORY") && screen.contains("VIEW"),
                    "{cols}x{rows}"
                );
                assert!(screen.contains("user: message 4"), "{cols}x{rows}");
            }
        }
        press(&mut w, KeyCode::Char('a'));
        let mut term = Terminal::new(TestBackend::new(100, 20)).unwrap();
        term.draw(|f| w.draw(f)).unwrap();
        let screen: String = term
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(screen.contains("add user"), "the prompt shows");
    }

    /// Renders the memory window with demo data to HTML, for the screenshot in `docs/index.html`.
    /// `BRIE_SHOT_OUT=memshot.html cargo test --release memwin_docs_screenshot -- --ignored`
    ///
    /// `BRIE_SHOT_TAB=view|outline|agents|threads` picks the tab, and `BRIE_SHOT_BIG=1` appends a
    /// few hundred long messages first, so the view has coarse lines to open in the outline.
    #[test]
    #[ignore]
    fn memwin_docs_screenshot() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        use ratatui::style::{Color, Modifier};

        let Some(out) = std::env::var_os("BRIE_SHOT_OUT") else {
            return;
        };
        unsafe {
            std::env::set_var("BRIE_SUMMARIZER", "model");
            std::env::set_var("BRIE_AGENT", "set");
        }
        let dir = std::env::temp_dir().join(format!("brie-memshot-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut m = Memory::open(&dir, true).unwrap();
        // The demo names its briefs the way `brie wait` logs them, so the THREADS tab has windows
        // to draw: `fix-login-redirect` opens first and closes last, the other two are short.
        let demo = [
            ("user", "the login bounce is losing sessions on redirect"),
            (
                "agent",
                "Confirmed on main: login.rs drops the session on 302 \
                 (08_10_2026_16:04_fix-login-redirect)",
            ),
            ("tool", "Bash {\"command\":\"cargo test auth\"} - 2 failed"),
            ("echo", "running 12 tests ... test result FAILED"),
            (
                "work",
                "[08_10_2026_16:04_fix-login-redirect] report: the open-redirect check is missing",
            ),
            (
                "user",
                "good catch. validate against the staging dump \
                 (08_10_2026_17:02_sessions-window)",
            ),
            (
                "agent",
                "Done in brief 08_10_2026_16:04_fix-login-redirect, ready for review",
            ),
            (
                "user",
                "next: csv export for /reports. stream it, the big accounts have 2M rows \
                 (08_10_2026_16:31_add-csv-export)",
            ),
            (
                "agent",
                "Wrote 08_10_2026_16:31_add-csv-export. Worker streams with a cursor",
            ),
            (
                "user",
                "the sessions migration needs a maintenance window \
                 (08_10_2026_17:02_sessions-window)",
            ),
            ("note", "Maintenance window decisions: Sundays 02:00 UTC"),
        ];
        // A tree to look at: without it the view is one line per message and there is nothing to open.
        if std::env::var_os("BRIE_SHOT_BIG").is_some() {
            for i in 0..1500 {
                m.append(
                    "user",
                    &format!("wave {i}: {}", "more of the same, at length. ".repeat(30)),
                )
                .unwrap();
            }
        }
        for (kind, text) in demo {
            m.append(kind, text).unwrap();
        }
        m.build(&Fake).unwrap();
        m.maintain().unwrap();
        drop(m);
        let project = Project {
            name: "harbor".into(),
            repo: dir.clone(),
        };
        let mut w = MemWin::at(project, dir.clone());
        w.tab = match std::env::var("BRIE_SHOT_TAB").unwrap_or_default().as_str() {
            "outline" => Tab::Outline,
            "agents" => Tab::Agents,
            "threads" => Tab::Threads,
            _ => Tab::View,
        };
        if let Some(root) = w.levels.first_mut() {
            root.sel = 7;
        }
        if w.tab == Tab::Outline {
            // open two of the view's own lines, so the shot shows lines standing in for their two
            let coarse: Vec<(u32, u64)> = w
                .mem
                .as_ref()
                .map(|m| m.view_lines().into_iter().filter(|(l, _)| *l > 0).collect())
                .unwrap_or_default();
            for k in coarse.iter().take(2) {
                w.open.insert(*k);
            }
            w.osel = coarse.first().copied();
        }
        let mut term = Terminal::new(TestBackend::new(112, 24)).unwrap();
        term.draw(|f| w.draw(f)).unwrap();
        let t = theme::get();
        let class = |fg: Color, bg: Color, bold: bool| {
            let mut c: Vec<&str> = vec![];
            for (color, name) in [
                (t.muted, "f-muted"),
                (t.accent, "f-accent"),
                (t.bad, "f-bad"),
                (t.info, "f-info"),
                (t.line, "f-line"),
                (t.accent_ink, "f-ink"),
                (t.text, "f-text"),
                (t.ok, "f-ok"),
                (t.violet, "f-violet"),
            ] {
                if fg == color {
                    c.push(name);
                }
            }
            for (color, name) in [
                (t.surface, "b-surface"),
                (t.accent, "b-accent"),
                (t.line, "b-line"),
                (t.bad, "b-bad"),
            ] {
                if bg == color {
                    c.push(name);
                }
            }
            if bold {
                c.push("bold");
            }
            c.join(" ")
        };
        let glyph = |ch: char| ('\u{2800}'..='\u{28ff}').contains(&ch) || "○◐●✕▌◆⚙⎘✎⌫".contains(ch);
        let buf = term.backend().buffer();
        let mut html = String::new();
        for y in 0..buf.area.height {
            let mut run = String::new();
            let mut key = (String::new(), 0usize);
            let flush = |html: &mut String, run: &mut String, class: &str| {
                if run.is_empty() {
                    return;
                }
                if class.is_empty() {
                    html.push_str(run);
                } else {
                    html.push_str(&format!("<span class=\"{class}\">{run}</span>"));
                }
                run.clear();
            };
            for x in 0..buf.area.width {
                let cell = &buf[(x, y)];
                let c = class(cell.fg, cell.bg, cell.modifier.contains(Modifier::BOLD));
                let ch = cell.symbol().chars().next().unwrap_or(' ');
                if c != key.0 {
                    flush(&mut html, &mut run, &key.0);
                    key.0 = c;
                }
                let text = match ch {
                    '&' => "&amp;".to_string(),
                    '<' => "&lt;".to_string(),
                    '>' => "&gt;".to_string(),
                    _ => ch.to_string(),
                };
                if glyph(ch) {
                    run.push_str(&format!("<span class=\"g\">{text}</span>"));
                } else {
                    run.push_str(&text);
                }
            }
            flush(&mut html, &mut run, &key.0);
            html.push('\n');
        }
        std::fs::write(out, html).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
