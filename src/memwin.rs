//! The memory window (`m` in the TUI): browse a project's memory and run its operations. It reads the
//! files without the lock, so it works while a turn runs; operations that write go through the same code
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
    View,
    Agents,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Op {
    Add,
    Turn,
    Sub,
}

/// A list of nodes on screen and the selected one. The first level is the memory's view; each zoom adds a
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

/// The kind a line starts with: `user: the login bounce...` is `user`.
fn leading_kind(text: &str) -> &str {
    text.split_once(':').map_or("", |(k, _)| k)
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
    /// otherwise keeps its line; deeper levels never change.
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
        let Some(m) = &self.mem else { return };
        let fresh = m.view_lines();
        match self.levels.first_mut() {
            None => {
                let sel = fresh.len().saturating_sub(1);
                self.levels.push(Level { lines: fresh, sel });
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
                root.lines = fresh;
            }
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

    fn cur(&self) -> Option<(u32, u64)> {
        let level = self.levels.last()?;
        level.lines.get(level.sel).copied()
    }

    fn step(&mut self, by: i32) {
        match self.tab {
            Tab::View => {
                if let Some(level) = self.levels.last_mut() {
                    let max = level.lines.len().saturating_sub(1) as i32;
                    level.sel = (level.sel as i32 + by).clamp(0, max) as usize;
                }
            }
            Tab::Agents => {
                let max = self.agents.len().saturating_sub(1) as i32;
                self.agent_sel = (self.agent_sel as i32 + by).clamp(0, max) as usize;
            }
        }
        self.scroll = 0;
    }

    fn jump(&mut self, end: bool) {
        let len = match self.tab {
            Tab::View => self.levels.last().map_or(0, |l| l.lines.len()),
            Tab::Agents => self.agents.len(),
        };
        self.step(if end { len as i32 } else { -(len as i32) });
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
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => self.zoom_in(),
            KeyCode::Backspace | KeyCode::Left | KeyCode::Char('h') => self.zoom_out(),
            KeyCode::Tab => {
                self.tab = if self.tab == Tab::View {
                    Tab::Agents
                } else {
                    Tab::View
                };
                self.scroll = 0;
            }
            KeyCode::Char(c @ ('1' | '2')) => {
                let tab = if c == '1' { Tab::View } else { Tab::Agents };
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
        let (Some(m), Some((l, i))) = (&self.mem, self.cur()) else {
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
            out.push(Line::styled("enter opens it into its two lines", muted));
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
        let parent = near("parent", l + 1, i / 2);
        let sibling = near("sibling", l, i ^ 1);
        if parent.is_some() || sibling.is_some() {
            out.push(Line::raw(""));
            out.extend(parent);
            out.extend(sibling);
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
            let picked = match self.tab {
                Tab::View => self
                    .levels
                    .last_mut()
                    .filter(|l| row < l.lines.len())
                    .map(|l| l.sel = row),
                Tab::Agents => (row < self.agents.len()).then(|| self.agent_sel = row),
            };
            if picked.is_some() {
                self.scroll = 0;
                if double {
                    self.zoom_in();
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
        let tab = |label: &'static str, active: bool| {
            if active {
                Span::styled(
                    label,
                    Style::new()
                        .fg(t.accent_ink)
                        .bg(t.accent)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                Span::styled(label, Style::new().fg(t.muted))
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
            _ => (vec![], 0, String::new()),
        };
        // The two tabs are click targets, like the keys 1 and 2; so are the rows.
        let (w1, w2) = (
            " 1 VIEW ".chars().count() as u16,
            " 2 AGENTS ".chars().count() as u16,
        );
        let key = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
        self.hits
            .push((Rect::new(area.x + 1, area.y, w1, 1), key('1')));
        self.hits
            .push((Rect::new(area.x + 2 + w1, area.y, w2, 1), key('2')));
        self.list_area = area;
        let block = Block::bordered()
            .title(Line::from(vec![
                tab(" 1 VIEW ", self.tab == Tab::View),
                Span::raw(" "),
                tab(" 2 AGENTS ", self.tab == Tab::Agents),
                Span::raw(" "),
            ]))
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
            for (k, what, code) in [
                ("enter", "zoom in", KeyCode::Enter),
                ("⌫", "out", KeyCode::Backspace),
                ("1 2 tab", "view/agents", KeyCode::Tab),
                ("a", "add", KeyCode::Char('a')),
                ("t", "turn", KeyCode::Char('t')),
                ("u", "subagent", KeyCode::Char('u')),
                ("s", "sync", KeyCode::Char('s')),
                ("b", "build", KeyCode::Char('b')),
                ("r", "reload", KeyCode::Char('r')),
                ("esc", "close", KeyCode::Esc),
            ] {
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
        press(&mut w, KeyCode::Tab);
        assert_eq!(w.tab, Tab::Agents);
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
        let list = w.list_area;
        w.click(click(list.x + 12, list.y));
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
        let demo = [
            ("user", "the login bounce is losing sessions on redirect"),
            (
                "agent",
                "Confirmed on main: login.rs drops the session on 302",
            ),
            ("tool", "Bash {\"command\":\"cargo test auth\"} - 2 failed"),
            ("echo", "running 12 tests ... test result FAILED"),
            ("work", "[Scout] report: the open-redirect check is missing"),
            ("user", "good catch. validate against the staging dump"),
            (
                "agent",
                "Done in brief fix-login-redirect, ready for review",
            ),
            (
                "user",
                "next: csv export for /reports. stream it, the big accounts have 2M rows",
            ),
            (
                "agent",
                "Wrote add-csv-export. Worker streams with a cursor",
            ),
            ("user", "the sessions migration needs a maintenance window"),
            ("note", "Maintenance window decisions: Sundays 02:00 UTC"),
        ];
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
        if let Some(root) = w.levels.first_mut() {
            root.sel = 7;
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
