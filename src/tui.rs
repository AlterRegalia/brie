use std::cell::{Cell, RefCell};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime};

use ratatui::crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};

use crate::editor::{Editor, Outcome};
use crate::logo;
use crate::maint;
use crate::md;
use crate::memwin::MemWin;
use crate::run;
use crate::store::{self, Item, Kind, Project, Res};
use crate::theme::{self, Theme};

/// The key a row of the help stands for, when it is one key: a click on the row runs it.
fn help_code(key: &str) -> Option<KeyCode> {
    match key {
        "tab" => Some(KeyCode::Tab),
        "enter" => Some(KeyCode::Enter),
        k if k.chars().count() == 1 && k != "?" => k.chars().next().map(KeyCode::Char),
        _ => None,
    }
}

/// Every shortcut, for the `?` modal.
const HELP: [(&str, &str); 24] = [
    ("j/k · ↑/↓", "move in the focused list"),
    ("h/l · ←/→", "focus projects / items"),
    ("tab", "switch briefs and prototypes"),
    (
        "enter",
        "open (brief: editor, read only once handed off · prototype: serve or reveal)",
    ),
    ("e", "open in $VISUAL/$EDITOR"),
    ("n", "new brief"),
    ("x", "move to the Trash (asks first)"),
    ("/", "find (AND terms · status:working · live)"),
    (
        "f",
        "status filter: all → todo → working → done → blocked (briefs)",
    ),
    ("o", "order: newest → oldest → A-Z → Z-A"),
    ("d", "date window: all → 24h → 7d → 30d"),
    ("esc", "clear filter, then ask to quit"),
    (
        "1/2/3/4/5",
        "projects · briefs · prototypes · preview · status (again hides)",
    ),
    ("a", "onboard a repo"),
    ("s", "serve the prototype"),
    ("v", "servers (x kill · X kill all · enter open · l log)"),
    (
        "m",
        "memory window (zoom the log · a add · t turn · u subagent · s sync · b build)",
    ),
    ("r", "reveal in the file manager"),
    ("R", "reload"),
    ("J/K · PgUp/PgDn", "scroll the preview"),
    (
        "click · wheel",
        "click selects a row (twice opens it) · wheel: list moves, preview scrolls · option/shift selects text",
    ),
    ("?", "this help"),
    ("q", "quit (asks first)"),
    ("ctrl-c", "quit at once"),
];

#[derive(Clone, Copy, Debug, PartialEq)]
enum Sort {
    Newest,
    Oldest,
    AtoZ,
    ZtoA,
}

impl Sort {
    fn next(self) -> Sort {
        match self {
            Sort::Newest => Sort::Oldest,
            Sort::Oldest => Sort::AtoZ,
            Sort::AtoZ => Sort::ZtoA,
            Sort::ZtoA => Sort::Newest,
        }
    }

    fn parse(s: &str) -> Sort {
        [Sort::Newest, Sort::Oldest, Sort::AtoZ, Sort::ZtoA]
            .into_iter()
            .find(|x| x.label() == s)
            .unwrap_or(Sort::Newest)
    }

    fn label(self) -> &'static str {
        match self {
            Sort::Newest => "newest",
            Sort::Oldest => "oldest",
            Sort::AtoZ => "A-Z",
            Sort::ZtoA => "Z-A",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Window {
    All,
    Day,
    Week,
    Month,
}

impl Window {
    fn next(self) -> Window {
        match self {
            Window::All => Window::Day,
            Window::Day => Window::Week,
            Window::Week => Window::Month,
            Window::Month => Window::All,
        }
    }

    fn parse(s: &str) -> Window {
        [Window::All, Window::Day, Window::Week, Window::Month]
            .into_iter()
            .find(|x| x.label() == s)
            .unwrap_or(Window::All)
    }

    fn label(self) -> &'static str {
        match self {
            Window::All => "all dates",
            Window::Day => "24h",
            Window::Week => "7d",
            Window::Month => "30d",
        }
    }

    fn span(self) -> Option<Duration> {
        let days = match self {
            Window::All => return None,
            Window::Day => 1,
            Window::Week => 7,
            Window::Month => 30,
        };
        Some(Duration::from_secs(days * 86_400))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum StatusFilter {
    All,
    Todo,
    Working,
    Done,
    Blocked,
}

impl StatusFilter {
    fn next(self) -> StatusFilter {
        match self {
            StatusFilter::All => StatusFilter::Todo,
            StatusFilter::Todo => StatusFilter::Working,
            StatusFilter::Working => StatusFilter::Done,
            StatusFilter::Done => StatusFilter::Blocked,
            StatusFilter::Blocked => StatusFilter::All,
        }
    }

    fn parse(s: &str) -> StatusFilter {
        [
            StatusFilter::All,
            StatusFilter::Todo,
            StatusFilter::Working,
            StatusFilter::Done,
            StatusFilter::Blocked,
        ]
        .into_iter()
        .find(|x| x.label() == s)
        .unwrap_or(StatusFilter::All)
    }

    fn label(self) -> &'static str {
        match self {
            StatusFilter::All => "all",
            StatusFilter::Todo => "todo",
            StatusFilter::Working => "working",
            StatusFilter::Done => "done",
            StatusFilter::Blocked => "blocked",
        }
    }

    fn matches(self, status: &Option<String>) -> bool {
        match self {
            StatusFilter::All => true,
            StatusFilter::Todo => status.as_deref() == Some("todo"),
            StatusFilter::Working => status.as_deref() == Some("working"),
            StatusFilter::Done => status.as_deref() == Some("done"),
            StatusFilter::Blocked => status.as_deref() == Some("blocked"),
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Focus {
    Projects,
    Items,
}

enum Input {
    None,
    AddProject(String),
    NewBrief(String),
    Search(String),
    Confirm(String),
    ConfirmQuit(String),
    Approve(String),
}

struct App {
    projects: Vec<Project>,
    project_state: ListState,
    kind: Kind,
    all: Vec<Item>,
    items: Vec<Item>,
    query: String,
    sort: Sort,
    window: Window,
    status_filter: StatusFilter,
    item_state: ListState,
    /// briefs, prototypes
    counts: (usize, usize),
    /// todo, working, done, blocked
    by_status: [usize; 4],
    focus: Focus,
    show: [bool; 4],
    /// Where the panes were drawn last: projects, list, preview, status. The mouse wheel goes to the one under it.
    areas: [Option<Rect>; 4],
    /// What a click does where it lands, as the key it stands for: the footer hints, the tabs, the header's servers.
    /// Rebuilt on every draw. `top` is the same for the window on top (a question, the servers, the help).
    hits: RefCell<Vec<(Rect, KeyCode)>>,
    top: RefCell<Vec<(Rect, KeyCode)>>,
    /// The rows of the status box and of the servers list, for clicks.
    status_rows: Cell<Option<Rect>>,
    modal_rows: Cell<Option<Rect>>,
    /// The last left click, for double clicks.
    last_click: Option<(Instant, u16, u16)>,
    input: Input,
    status: String,
    preview: Vec<Line<'static>>,
    scroll: u16,
    /// How far the preview can scroll: its wrapped rows minus the rows on screen. Set when it is drawn.
    scroll_max: std::cell::Cell<Option<u16>>,
    /// Prototype servers started by brie, from the registry (any process: this TUI, the CLI, an agent).
    running: Vec<run::Entry>,
    modal: Option<ListState>,
    modal_log: bool,
    help: bool,
    pending_run: Option<(PathBuf, run::Manifest, bool)>,
    editor: Option<Editor>,
    memwin: Option<MemWin>,
    pending_rm: Option<PathBuf>,
    saved_state: String,
    quit: bool,
}

pub fn start() -> Res<()> {
    let mut app = App::new();
    let mut terminal = enter();
    let result = app.event_loop(&mut terminal);
    leave();
    let left = run::running().len();
    if left > 0 {
        println!("{left} prototype server(s) still running: brie serve ls, brie serve stop all");
    }
    result
}

fn enter() -> DefaultTerminal {
    let terminal = ratatui::init();
    let _ =
        ratatui::crossterm::execute!(std::io::stdout(), EnableBracketedPaste, EnableMouseCapture);
    terminal
}

fn leave() {
    let _ = ratatui::crossterm::execute!(
        std::io::stdout(),
        DisableBracketedPaste,
        DisableMouseCapture
    );
    ratatui::restore();
}

impl App {
    fn new() -> App {
        let mut app = App {
            projects: store::projects(),
            project_state: ListState::default(),
            kind: Kind::Briefs,
            all: vec![],
            items: vec![],
            query: String::new(),
            sort: Sort::Newest,
            window: Window::All,
            status_filter: StatusFilter::All,
            item_state: ListState::default(),
            counts: (0, 0),
            by_status: [0; 4],
            focus: Focus::Projects,
            show: [true; 4],
            areas: [None; 4],
            hits: RefCell::new(vec![]),
            top: RefCell::new(vec![]),
            status_rows: Cell::new(None),
            modal_rows: Cell::new(None),
            last_click: None,
            input: Input::None,
            status: String::new(),
            preview: vec![],
            scroll: 0,
            scroll_max: std::cell::Cell::new(None),
            running: vec![],
            modal: None,
            modal_log: false,
            help: false,
            pending_run: None,
            editor: None,
            memwin: None,
            pending_rm: None,
            saved_state: String::new(),
            quit: false,
        };
        if !app.projects.is_empty() {
            app.project_state.select(Some(0));
        }
        app.restore_state();
        app.load_items();
        app
    }

    fn project(&self) -> Option<&Project> {
        self.projects.get(self.project_state.selected()?)
    }

    fn item(&self) -> Option<&Item> {
        self.items.get(self.item_state.selected()?)
    }

    fn load_items(&mut self) {
        self.all = match self.project() {
            Some(p) => store::list(&p.name, self.kind),
            None => vec![],
        };
        self.recount();
        self.apply();
    }

    fn recount(&mut self) {
        let Some(name) = self.project().map(|p| p.name.clone()) else {
            self.counts = (0, 0);
            self.by_status = [0; 4];
            return;
        };
        let briefs = store::list(&name, Kind::Briefs);
        let count = |name: &str| {
            briefs
                .iter()
                .filter(|i| i.status.as_deref() == Some(name))
                .count()
        };
        self.by_status = [
            count("todo"),
            count("working"),
            count("done"),
            count("blocked"),
        ];
        self.counts = (briefs.len(), store::list(&name, Kind::Prototypes).len());
    }

    /// Rebuild the visible list from `all`: status, date window, search, then sort. Keeps the selected item if it survives.
    fn apply(&mut self) {
        let keep = self.item().map(|i| i.path.clone());
        let now = SystemTime::now();
        let terms: Vec<String> = self
            .query
            .split_whitespace()
            .map(str::to_lowercase)
            .collect();
        let span = self.window.span();
        let status_only_briefs = self.kind == Kind::Briefs;
        let mut shown: Vec<Item> = self
            .all
            .iter()
            .filter(|i| {
                if status_only_briefs && !self.status_filter.matches(&i.status) {
                    return false;
                }
                let in_window = span.is_none_or(|s| {
                    now.duration_since(i.modified)
                        .map(|age| age <= s)
                        .unwrap_or(true)
                });
                in_window && matches_terms(i, &terms)
            })
            .cloned()
            .collect();
        match self.sort {
            // Newest first by mtime, Z-A by name as the tiebreak; oldest mirrors it.
            Sort::Newest => shown.sort_by(|a, b| {
                b.modified
                    .cmp(&a.modified)
                    .then_with(|| b.name.to_lowercase().cmp(&a.name.to_lowercase()))
            }),
            Sort::Oldest => shown.sort_by(|a, b| {
                a.modified
                    .cmp(&b.modified)
                    .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            }),
            Sort::AtoZ => shown.sort_by_key(|i| i.name.to_lowercase()),
            Sort::ZtoA => shown.sort_by_key(|i| std::cmp::Reverse(i.name.to_lowercase())),
        }
        self.items = shown;
        let at = keep
            .and_then(|k| self.items.iter().position(|i| i.path == k))
            .or((!self.items.is_empty()).then_some(0));
        self.item_state.select(at);
        self.load_preview();
    }

    fn load_preview(&mut self) {
        self.scroll = 0;
        self.preview = match self.item() {
            None if self.projects.is_empty() => {
                vec![Line::styled(
                    "No projects yet. Press a to onboard a repo.",
                    Style::new().fg(theme::get().muted),
                )]
            }
            None => vec![],
            Some(i) if i.is_dir => dir_preview(&i.path),
            Some(i) => file_preview(&i.path),
        };
    }

    fn scroll_by(&mut self, delta: i32) {
        let max =
            self.scroll_max
                .get()
                .unwrap_or_else(|| self.preview.len().saturating_sub(1) as u16) as i32;
        self.scroll = (self.scroll as i32 + delta).clamp(0, max) as u16;
    }

    fn event_loop(&mut self, terminal: &mut DefaultTerminal) -> Res<()> {
        while !self.quit {
            terminal.draw(|f| self.draw(f))?;
            if !event::poll(Duration::from_millis(1000))? {
                self.tick();
                continue;
            }
            match event::read()? {
                Event::Paste(text) => self.paste(&text),
                Event::Mouse(mouse) => {
                    let left = matches!(
                        mouse.kind,
                        MouseEventKind::Down(ratatui::crossterm::event::MouseButton::Left)
                    );
                    if left && let Some(editor) = &mut self.editor {
                        if let Outcome::Close = editor.click(mouse) {
                            self.editor = None;
                            self.load_items();
                        }
                        continue;
                    }
                    if left && let Some(win) = &mut self.memwin {
                        if let Outcome::Close = win.click(mouse) {
                            self.memwin = None;
                        }
                        continue;
                    }
                    if let Some(code) = self.mouse(mouse) {
                        if self.modal.is_some() {
                            self.modal_key(code);
                        } else if matches!(self.input, Input::None) {
                            self.normal_key(code, terminal);
                        } else {
                            self.input_key(code);
                        }
                    }
                    self.save_state();
                }
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if let Some(editor) = &mut self.editor {
                        if let Outcome::Close = editor.key(key) {
                            self.editor = None;
                            self.load_items();
                        }
                        continue;
                    }
                    if key.modifiers.contains(KeyModifiers::CONTROL)
                        && key.code == KeyCode::Char('c')
                    {
                        break;
                    }
                    if let Some(win) = &mut self.memwin {
                        if let Outcome::Close = win.key(key) {
                            self.memwin = None;
                        }
                        continue;
                    }
                    if self.help {
                        if matches!(key.code, KeyCode::Esc | KeyCode::Char('q' | '?')) {
                            self.help = false;
                        }
                        continue;
                    }
                    if self.modal.is_some() {
                        self.modal_key(key.code);
                        continue;
                    }
                    match self.input {
                        Input::None => self.normal_key(key.code, terminal),
                        _ => self.input_key(key.code),
                    }
                    self.save_state();
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// A left click. It returns the key the click stands for, if any, for the caller to run as if it were typed:
    /// the footer hints, the tabs, the servers count and the buttons of a window are all keys. A click on a row of
    /// the projects or the list selects it, a second one on the same row opens it, and a click on a row of the
    /// status box filters by that status. The wheel is in `wheel`.
    fn mouse(&mut self, mouse: MouseEvent) -> Option<KeyCode> {
        let MouseEventKind::Down(ratatui::crossterm::event::MouseButton::Left) = mouse.kind else {
            self.wheel(mouse);
            return None;
        };
        if self.editor.is_some() || self.memwin.is_some() {
            return None;
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
        // A window on top takes the click: the help closes, a question and the servers have buttons.
        if self.help {
            self.help = false;
            return hit_at(&self.top, at);
        }
        if self.modal.is_some() {
            if let Some(code) = hit_at(&self.top, at) {
                return Some(code);
            }
            let rows = self.modal_rows.get().filter(|r| r.contains(at))?;
            let row = (mouse.row - rows.y) as usize + self.modal.as_ref().map_or(0, |s| s.offset());
            if row < self.running.len() {
                self.modal.as_mut()?.select(Some(row));
                return double.then_some(KeyCode::Enter);
            }
            return None;
        }
        if matches!(
            self.input,
            Input::Confirm(_) | Input::ConfirmQuit(_) | Input::Approve(_)
        ) {
            return hit_at(&self.top, at);
        }
        if !matches!(self.input, Input::None) {
            return None;
        }
        if let Some(code) = hit_at(&self.hits, at) {
            return Some(code);
        }
        if let Some(rows) = self.status_rows.get().filter(|r| r.contains(at)) {
            let pick = [
                StatusFilter::Todo,
                StatusFilter::Working,
                StatusFilter::Done,
                StatusFilter::Blocked,
            ][(mouse.row - rows.y) as usize];
            if self.kind != Kind::Briefs {
                self.kind = Kind::Briefs;
                self.show[1] = true;
                self.load_items();
            }
            self.status_filter = if self.status_filter == pick {
                StatusFilter::All
            } else {
                pick
            };
            self.apply();
            return None;
        }
        for (pane, focus, offset) in [
            (0, Focus::Projects, self.project_state.offset()),
            (1, Focus::Items, self.item_state.offset()),
        ] {
            let Some(area) = self.areas[pane].filter(|r| r.contains(at)) else {
                continue;
            };
            // The rows sit inside the frame.
            let (top, len) = (area.y + 1, [self.projects.len(), self.items.len()][pane]);
            if mouse.row < top || mouse.row + 1 >= area.y + area.height || mouse.column == area.x {
                return None;
            }
            let row = offset + (mouse.row - top) as usize;
            self.focus = focus;
            if row < len {
                self.select_in(focus, row);
                // A second click on a brief or prototype opens it.
                return (double && focus == Focus::Items).then_some(KeyCode::Enter);
            }
            return None;
        }
        None
    }

    /// The mouse wheel. A terminal in the alternate screen turns the wheel into arrow keys, which moved whatever pane
    /// had focus. With the mouse captured the wheel acts on the pane under it: the list moves its selection, the
    /// preview scrolls, and the projects and status panes ignore it. Windows on top (editor, memory, servers) get arrows.
    fn wheel(&mut self, mouse: MouseEvent) {
        let down = match mouse.kind {
            MouseEventKind::ScrollDown => true,
            MouseEventKind::ScrollUp => false,
            _ => return,
        };
        let arrow = KeyEvent::new(
            if down { KeyCode::Down } else { KeyCode::Up },
            KeyModifiers::NONE,
        );
        if let Some(editor) = &mut self.editor {
            editor.key(arrow);
            return;
        }
        if let Some(win) = &mut self.memwin {
            win.key(arrow);
            return;
        }
        if self.help {
            return;
        }
        if self.modal.is_some() {
            self.modal_key(arrow.code);
            return;
        }
        if !matches!(self.input, Input::None) {
            return;
        }
        let over = |area: Option<Rect>| {
            area.is_some_and(|r| {
                r.contains(ratatui::layout::Position::new(mouse.column, mouse.row))
            })
        };
        let delta = if down { 1 } else { -1 };
        if over(self.areas[2]) {
            self.scroll_by(delta * 3);
        } else if over(self.areas[1]) {
            self.step_in(Focus::Items, delta as isize);
        }
    }

    fn paste(&mut self, text: &str) {
        if let Some(editor) = &mut self.editor {
            editor.paste(text);
            return;
        }
        if let Some(win) = &mut self.memwin {
            win.paste(text);
            return;
        }
        let line = text.lines().next().unwrap_or("");
        if let Input::AddProject(s) | Input::NewBrief(s) | Input::Search(s) = &mut self.input {
            s.push_str(line);
            if matches!(self.input, Input::Search(_)) {
                self.query = line.to_string();
            }
        }
    }

    /// Once a second: pick up changes made by workers and agents, and tell about finished briefs.
    fn tick(&mut self) {
        if let Some(editor) = &mut self.editor {
            editor.tick();
            return;
        }
        if let Some(win) = &mut self.memwin {
            win.tick();
            return;
        }
        if self.modal.is_some() {
            self.refresh_modal();
        } else if !self.help {
            self.running = run::running();
        }
        let names: Vec<String> = store::projects().into_iter().map(|p| p.name).collect();
        if names
            != self
                .projects
                .iter()
                .map(|p| p.name.clone())
                .collect::<Vec<_>>()
        {
            let keep = self.project().map(|p| p.name.clone());
            self.projects = store::projects();
            let at = keep.and_then(|k| self.projects.iter().position(|p| p.name == k));
            self.project_state
                .select(at.or((!self.projects.is_empty()).then_some(0)));
            self.load_items();
            return;
        }
        let Some(project) = self.project().map(|p| p.name.clone()) else {
            return;
        };
        let fresh = store::list(&project, self.kind);
        let same = fresh.len() == self.all.len()
            && fresh
                .iter()
                .zip(&self.all)
                .all(|(a, b)| a.path == b.path && a.modified == b.modified && a.status == b.status);
        if same {
            return;
        }
        for item in &fresh {
            let before = self
                .all
                .iter()
                .find(|o| o.path == item.path)
                .and_then(|o| o.status.clone());
            let now = item.status.as_deref();
            if before.is_some()
                && before.as_deref() != now
                && matches!(now, Some("done" | "blocked"))
            {
                self.status = format!("{}: {}", item.name, now.unwrap_or(""));
                maint::push(
                    &format!("{project}: {} {}", item.name, now.unwrap_or("")),
                    false,
                );
                print!("\x07");
                let _ = std::io::Write::flush(&mut std::io::stdout());
            }
        }
        let scroll = self.scroll;
        self.all = fresh;
        self.apply();
        self.scroll = scroll.min(self.preview.len().saturating_sub(1) as u16);
        self.recount();
    }

    fn state_text(&self) -> String {
        let panes: String = self
            .show
            .iter()
            .map(|&b| if b { '1' } else { '0' })
            .collect();
        format!(
            "panes={panes}\nsort={}\nwindow={}\nstatus={}\nkind={}\nproject={}\n",
            self.sort.label(),
            self.window.label(),
            self.status_filter.label(),
            self.kind.dir(),
            self.project().map(|p| p.name.as_str()).unwrap_or("")
        )
    }

    fn save_state(&mut self) {
        let text = self.state_text();
        if text != self.saved_state {
            let _ = std::fs::write(store::root().join("state"), &text);
            self.saved_state = text;
        }
    }

    fn restore_state(&mut self) {
        let text = std::fs::read_to_string(store::root().join("state")).unwrap_or_default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            match key {
                "panes" if value.len() == 3 || value.len() == 4 => {
                    let flags: Vec<bool> = value.chars().map(|c| c == '1').collect();
                    if flags[0] || flags[1] {
                        // Older state files have three flags: the status pane stays on.
                        self.show = [
                            flags[0],
                            flags[1],
                            flags[2],
                            flags.get(3).copied().unwrap_or(true),
                        ];
                        if !self.show[0] {
                            self.focus = Focus::Items;
                        }
                    }
                }
                "sort" => self.sort = Sort::parse(value),
                "window" => self.window = Window::parse(value),
                "status" => self.status_filter = StatusFilter::parse(value),
                "kind" => {
                    self.kind = if value == "prototypes" {
                        Kind::Prototypes
                    } else {
                        Kind::Briefs
                    }
                }
                "project" => {
                    if let Some(at) = self.projects.iter().position(|p| p.name == value) {
                        self.project_state.select(Some(at));
                    }
                }
                _ => {}
            }
        }
        self.saved_state = self.state_text();
    }

    fn normal_key(&mut self, code: KeyCode, terminal: &mut DefaultTerminal) {
        self.status.clear();
        match code {
            KeyCode::Esc if self.filtered() => {
                self.query.clear();
                self.window = Window::All;
                self.status_filter = StatusFilter::All;
                self.apply();
            }
            KeyCode::Char('q') | KeyCode::Esc => {
                let msg = if self.running.is_empty() {
                    "quit brie?".to_string()
                } else {
                    format!("quit with {} server(s) still running?", self.running.len())
                };
                self.input = Input::ConfirmQuit(msg);
            }
            KeyCode::Char('/') => self.input = Input::Search(self.query.clone()),
            KeyCode::Char('v') => self.open_modal(),
            KeyCode::Char('m') => match self.project().cloned() {
                Some(p) if !store::memory_enabled(&p.name) => {
                    self.status = "memory is off for this project: brie add --memory".into();
                }
                Some(p) => self.memwin = Some(MemWin::new(p)),
                None => self.status = "onboard a repo first (a)".into(),
            },
            KeyCode::Char('?') => self.help = true,
            KeyCode::Char('x') => {
                if let Some((path, name)) = self.item().map(|i| (i.path.clone(), i.name.clone())) {
                    let what = if self.kind == Kind::Briefs {
                        "and its Scratch directory "
                    } else {
                        ""
                    };
                    self.pending_rm = Some(path);
                    self.input = Input::Confirm(format!("move {name} {what}to the Trash?"));
                }
            }
            KeyCode::Char('o') => {
                self.sort = self.sort.next();
                self.apply();
            }
            KeyCode::Char('f') => {
                self.status_filter = self.status_filter.next();
                self.apply();
            }
            KeyCode::Char('d') => {
                self.window = self.window.next();
                self.apply();
            }
            KeyCode::Down | KeyCode::Char('j') => self.step(1),
            KeyCode::Up | KeyCode::Char('k') => self.step(-1),
            KeyCode::Char('J') | KeyCode::PageDown => self.scroll_by(8),
            KeyCode::Char('K') | KeyCode::PageUp => self.scroll_by(-8),
            KeyCode::Char(c @ '1'..='5') => match c {
                '1' => self.toggle_pane(0),
                '2' => self.goto_tab(Kind::Briefs),
                '3' => self.goto_tab(Kind::Prototypes),
                '4' => self.toggle_pane(2),
                _ => self.toggle_pane(3),
            },
            KeyCode::Left | KeyCode::Char('h') if self.show[0] => self.focus = Focus::Projects,
            KeyCode::Right | KeyCode::Char('l') if self.show[1] => self.focus = Focus::Items,
            KeyCode::Tab => {
                self.kind = match self.kind {
                    Kind::Briefs => Kind::Prototypes,
                    Kind::Prototypes => Kind::Briefs,
                };
                self.load_items();
            }
            KeyCode::Char('R') => {
                self.projects = store::projects();
                self.load_items();
            }
            KeyCode::Char('a') => {
                let cwd = std::env::current_dir().unwrap_or_default();
                self.input = Input::AddProject(cwd.display().to_string());
            }
            KeyCode::Char('n') if self.project().is_some() => {
                self.input = Input::NewBrief(String::new());
            }
            KeyCode::Enter if self.focus == Focus::Projects => {
                if self.show[1] {
                    self.focus = Focus::Items;
                }
            }
            KeyCode::Enter => match (self.kind, self.item().map(|i| i.path.clone())) {
                (Kind::Briefs, Some(path)) if store::is_draft(&path) => self.open_editor(&path),
                (Kind::Briefs, Some(path)) => self.open_viewer(&path),
                (Kind::Prototypes, Some(path)) => self.open_prototype(&path),
                _ => {}
            },
            KeyCode::Char('e') => {
                if let Some(path) = self.item().filter(|i| !i.is_dir).map(|i| i.path.clone())
                    && (self.kind != Kind::Briefs || self.draft_only(&path))
                {
                    self.edit(&path, terminal);
                }
            }
            KeyCode::Char('s') if self.kind == Kind::Prototypes => {
                if let Some(path) = self.item().map(|i| i.path.clone()) {
                    self.serve(&path, false);
                }
            }
            KeyCode::Char('r') => {
                let target = match self.focus {
                    Focus::Items => self.item().map(|i| i.path.clone()),
                    Focus::Projects => self.project().map(|p| store::kind_dir(&p.name, self.kind)),
                };
                if let Some(path) = target {
                    store::reveal(&path);
                }
            }
            _ => {}
        }
    }

    fn input_key(&mut self, code: KeyCode) {
        if let Input::Approve(_) = self.input {
            self.input = Input::None;
            if code == KeyCode::Char('y')
                && let Some((dir, manifest, open)) = self.pending_run.take()
            {
                match run::approve(&manifest) {
                    Ok(()) => self.serve(&dir, open),
                    Err(e) => self.status = format!("approve failed: {e}"),
                }
            }
            self.pending_run = None;
            return;
        }
        if let Input::ConfirmQuit(_) = self.input {
            self.input = Input::None;
            if code == KeyCode::Char('y') {
                self.quit = true;
            }
            return;
        }
        if let Input::Confirm(_) = self.input {
            self.input = Input::None;
            if code == KeyCode::Char('y') {
                self.remove_pending();
            }
            self.pending_rm = None;
            return;
        }
        let (mut text, searching) = match &self.input {
            Input::AddProject(s) | Input::NewBrief(s) => (s.clone(), false),
            Input::Search(s) => (s.clone(), true),
            Input::Confirm(_) | Input::ConfirmQuit(_) | Input::Approve(_) | Input::None => return,
        };
        match code {
            KeyCode::Esc => {
                self.input = Input::None;
                if searching {
                    self.query.clear();
                    self.apply();
                }
                return;
            }
            KeyCode::Enter => {
                match std::mem::replace(&mut self.input, Input::None) {
                    Input::AddProject(_) => self.add_project(&text),
                    Input::NewBrief(_) => self.new_brief(&text),
                    Input::Search(_)
                    | Input::Confirm(_)
                    | Input::ConfirmQuit(_)
                    | Input::Approve(_)
                    | Input::None => {}
                }
                return;
            }
            KeyCode::Backspace => {
                text.pop();
            }
            KeyCode::Char(c) => text.push(c),
            _ => return,
        }
        if searching {
            self.query = text.clone();
            self.apply();
        }
        if let Input::AddProject(s) | Input::NewBrief(s) | Input::Search(s) = &mut self.input {
            *s = text;
        }
    }

    /// 2/3 jump to a tab (showing the items pane); pressed on the active tab they hide it again.
    fn goto_tab(&mut self, kind: Kind) {
        if self.kind == kind && self.show[1] {
            self.toggle_pane(1);
        } else {
            self.kind = kind;
            self.show[1] = true;
            self.focus = Focus::Items;
            self.load_items();
        }
    }

    /// Like btop's 1-5 keys. The projects and items panes cannot both be hidden: j/k needs one list.
    fn toggle_pane(&mut self, pane: usize) {
        let mut next = self.show;
        next[pane] = !next[pane];
        if !next[0] && !next[1] {
            return;
        }
        self.show = next;
        if !next[0] && self.focus == Focus::Projects {
            self.focus = Focus::Items;
        }
        if !next[1] && self.focus == Focus::Items {
            self.focus = Focus::Projects;
        }
    }

    fn remove_pending(&mut self) {
        let (Some(path), Some(project)) = (self.pending_rm.clone(), self.project().cloned()) else {
            return;
        };
        let result = if self.kind == Kind::Briefs {
            store::remove_brief(&project, &path)
        } else {
            store::trash(&path).map(|_| ())
        };
        self.status = match result {
            Ok(()) => "moved to the Trash".into(),
            Err(e) => format!("trash failed: {e}"),
        };
        self.load_items();
    }

    fn filtered(&self) -> bool {
        !self.query.is_empty()
            || self.window != Window::All
            || self.status_filter != StatusFilter::All
    }

    fn step(&mut self, delta: isize) {
        self.step_in(self.focus, delta);
    }

    fn step_in(&mut self, focus: Focus, delta: isize) {
        let (state, len) = match focus {
            Focus::Projects => (&mut self.project_state, self.projects.len()),
            Focus::Items => (&mut self.item_state, self.items.len()),
        };
        if len == 0 {
            return;
        }
        let next = state.selected().unwrap_or(0) as isize + delta;
        self.select_in(focus, next.clamp(0, len as isize - 1) as usize);
    }

    /// Select row `at` of the projects or the list, and load what it shows.
    fn select_in(&mut self, focus: Focus, at: usize) {
        match focus {
            Focus::Projects => self.project_state.select(Some(at)),
            Focus::Items => self.item_state.select(Some(at)),
        }
        if focus == Focus::Projects {
            self.item_state.select(Some(0));
            self.load_items();
        } else {
            self.load_preview();
        }
    }

    fn add_project(&mut self, path: &str) {
        match store::add(&store::expand(path.trim()), None) {
            Ok(p) => {
                self.status = format!("onboarded {}", p.name);
                self.projects = store::projects();
                let at = self.projects.iter().position(|x| x.name == p.name);
                self.project_state.select(at);
                self.load_items();
            }
            Err(e) => self.status = format!("add failed: {e}"),
        }
    }

    fn new_brief(&mut self, name: &str) {
        let Some(project) = self.project().cloned() else {
            return;
        };
        match store::new_brief(&project, name) {
            Ok(path) => {
                self.kind = Kind::Briefs;
                self.load_items();
                let at = self.items.iter().position(|i| i.path == path);
                self.item_state.select(at);
                self.load_preview();
                self.open_editor(&path);
            }
            Err(e) => self.status = format!("new failed: {e}"),
        }
    }

    /// A brief is edited in place only while it is a draft. Says why when it is not.
    fn draft_only(&mut self, path: &Path) -> bool {
        let draft = store::is_draft(path);
        if !draft {
            self.status = format!(
                "{} is handed off: a brief only grows at the bottom (brie report)",
                store::status_of(path)
            );
        }
        draft
    }

    fn open_editor(&mut self, path: &Path) {
        match Editor::open(path) {
            Ok(editor) => self.editor = Some(editor),
            Err(e) => self.status = format!("cannot edit: {e}"),
        }
    }

    /// Enter on a brief that was handed off: the editor window, read only.
    fn open_viewer(&mut self, path: &Path) {
        match Editor::view(path) {
            Ok(editor) => self.editor = Some(editor),
            Err(e) => self.status = format!("cannot open: {e}"),
        }
    }

    fn edit(&mut self, path: &Path, terminal: &mut DefaultTerminal) {
        let editor = std::env::var("VISUAL")
            .or_else(|_| std::env::var("EDITOR"))
            .unwrap_or_else(|_| "vi".into());
        leave();
        let _ = Command::new("sh")
            .arg("-c")
            .arg(format!("{editor} \"$1\""))
            .arg("sh")
            .arg(path)
            .status();
        *terminal = enter();
        self.load_items();
    }

    /// Enter on a prototype: a web page or folder is served; any other file opens in the system viewer.
    fn open_prototype(&mut self, path: &Path) {
        let web = path.is_dir()
            || path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "html" | "htm" | "md"));
        if web {
            self.serve(path, true);
        } else {
            let _ = Command::new(store::opener()).arg(path).status();
            self.status = "opened with the system viewer".into();
        }
    }

    fn serve(&mut self, path: &Path, open: bool) {
        let (dir, file) = if path.is_dir() {
            (path.to_path_buf(), String::new())
        } else {
            let parent = path.parent().unwrap_or(path).to_path_buf();
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            (parent, name.into_owned())
        };
        let dir = std::fs::canonicalize(&dir).unwrap_or(dir);
        let dir_text = dir.display().to_string();
        self.running = run::running();
        let port = match self.running.iter().find(|e| e.dir == dir_text) {
            Some(e) => e.port,
            None => {
                if let Some(manifest) = run::needs_approval(&dir) {
                    let msg = format!("brie.run wants to run `{}`. Run it?", manifest.cmd);
                    self.pending_run = Some((dir, manifest, open));
                    self.input = Input::Approve(msg);
                    return;
                }
                let mut args = vec![dir_text];
                if let Some(p) = self.project() {
                    args.extend(["-p".into(), p.name.clone()]);
                }
                match run::start_detached(&args) {
                    Ok(entry) => {
                        let port = entry.port;
                        self.running.push(entry);
                        port
                    }
                    Err(e) => {
                        self.status = format!(
                            "serve failed: {}",
                            e.to_string().lines().next().unwrap_or("")
                        );
                        return;
                    }
                }
            }
        };
        if port == 0 {
            self.status = "running (no port)".into();
            return;
        }
        let url = format!("http://localhost:{port}/{file}");
        if open {
            let _ = Command::new(store::opener()).arg(&url).status();
        }
        self.status = format!("serving {url}");
    }

    // ---- the servers modal ----

    fn open_modal(&mut self) {
        self.running = run::running();
        let mut state = ListState::default();
        state.select((!self.running.is_empty()).then_some(0));
        self.modal = Some(state);
        self.modal_log = false;
    }

    fn modal_key(&mut self, code: KeyCode) {
        let Some(state) = &mut self.modal else { return };
        let n = self.running.len();
        match code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('v') => self.modal = None,
            KeyCode::Down | KeyCode::Char('j') if n > 0 => {
                state.select(Some((state.selected().unwrap_or(0) + 1).min(n - 1)));
            }
            KeyCode::Up | KeyCode::Char('k') if n > 0 => {
                state.select(Some(state.selected().unwrap_or(0).saturating_sub(1)));
            }
            KeyCode::Char('l') => self.modal_log = !self.modal_log,
            KeyCode::Enter => {
                if let Some(e) = state.selected().and_then(|i| self.running.get(i))
                    && e.port > 0
                {
                    let _ = Command::new(store::opener())
                        .arg(format!("http://localhost:{}", e.port))
                        .status();
                }
            }
            KeyCode::Char('x') => {
                if let Some(pid) = state
                    .selected()
                    .and_then(|i| self.running.get(i))
                    .map(|e| e.pid)
                {
                    run::stop(run::Target::Pid(pid));
                    self.refresh_modal();
                    self.status = "stopped".into();
                }
            }
            KeyCode::Char('X') => {
                let stopped = run::stop(run::Target::All);
                self.refresh_modal();
                self.status = format!("stopped {stopped}");
            }
            _ => {}
        }
    }

    fn refresh_modal(&mut self) {
        self.running = run::running();
        if let Some(state) = &mut self.modal {
            let n = self.running.len();
            state.select(if n == 0 {
                None
            } else {
                Some(state.selected().unwrap_or(0).min(n - 1))
            });
        }
    }

    fn draw_modal(&mut self, f: &mut Frame, t: &Theme) {
        let area = f.area();
        let log_rows: u16 = if self.modal_log { 10 } else { 0 };
        let want = 7 + self.running.len().max(1) as u16 + log_rows;
        let (w, h) = (
            area.width.saturating_sub(6).min(96),
            want.min(area.height.saturating_sub(2)),
        );
        let rect = Rect::new(
            area.x + (area.width - w) / 2,
            area.y + (area.height - h) / 2,
            w,
            h,
        );
        f.render_widget(ratatui::widgets::Clear, rect);
        let block = Block::bordered()
            .title(Line::from(vec![
                Span::styled(
                    " SERVERS ",
                    Style::new().fg(t.accent).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("{} running ", self.running.len()),
                    Style::new().fg(t.muted),
                ),
            ]))
            .border_style(Style::new().fg(t.accent))
            .style(Style::new().bg(t.surface));
        let inner = block.inner(rect);
        f.render_widget(block, rect);

        let [list_area, log_area, buttons] = Layout::vertical([
            Constraint::Min(2),
            Constraint::Length(if self.modal_log { log_rows } else { 0 }),
            Constraint::Length(2),
        ])
        .areas(inner);

        if self.running.is_empty() {
            f.render_widget(
                Paragraph::new(Line::styled(
                    "No prototype servers running.",
                    Style::new().fg(t.muted),
                )),
                list_area,
            );
        } else {
            let rows: Vec<ListItem> = self
                .running
                .iter()
                .map(|e| {
                    let port = if e.port == 0 {
                        "  -  ".to_string()
                    } else {
                        format!(":{:<4}", e.port)
                    };
                    let name = std::path::Path::new(&e.dir)
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    ListItem::new(Line::from(vec![
                        Span::styled(
                            format!("{port}  "),
                            Style::new().add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(format!("{:<7}", e.kind), Style::new().fg(t.info)),
                        Span::raw(format!(" {name}")),
                        Span::styled(format!("   pid {}", e.pid), Style::new().fg(t.muted)),
                    ]))
                })
                .collect();
            let list = List::new(rows).highlight_style(self.highlight(true, t));
            self.modal_rows.set(Some(list_area));
            if let Some(state) = &mut self.modal {
                f.render_stateful_widget(list, list_area, state);
            }
        }

        if self.modal_log {
            let selected = self
                .modal
                .as_ref()
                .and_then(|s| s.selected())
                .and_then(|i| self.running.get(i));
            let text = match selected {
                Some(e) if e.log != "-" => run::tail(
                    std::path::Path::new(&e.log),
                    (log_rows as usize).saturating_sub(1),
                ),
                Some(_) => "(this server writes no log)".to_string(),
                None => String::new(),
            };
            let lines: Vec<Line> = text
                .lines()
                .map(|l| Line::styled(l.to_string(), Style::new().fg(t.muted)))
                .collect();
            f.render_widget(
                Paragraph::new(lines).block(
                    Block::new()
                        .borders(Borders::TOP)
                        .border_style(Style::new().fg(t.line)),
                ),
                log_area,
            );
        }

        let pill = |key: &str, what: &str, bg| {
            vec![
                Span::styled(
                    format!(" {key} "),
                    Style::new()
                        .fg(t.accent_ink)
                        .bg(bg)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!(" {what}   "), Style::new().fg(t.muted)),
            ]
        };
        let (mut x, mut spans) = (buttons.x, vec![]);
        let y = buttons.y + 1;
        for (key, what, bg, code) in [
            ("x", "kill", t.bad, KeyCode::Char('x')),
            ("X", "kill all", t.bad, KeyCode::Char('X')),
            ("enter", "open", t.accent, KeyCode::Enter),
            ("l", "log", t.accent, KeyCode::Char('l')),
            ("esc", "close", t.line, KeyCode::Esc),
        ] {
            hit_span(
                &self.top,
                &mut x,
                y,
                &mut spans,
                pill(key, what, bg),
                Some(code),
            );
        }
        f.render_widget(
            Paragraph::new(vec![Line::raw(""), Line::from(spans)]),
            buttons,
        );
    }

    fn draw_help(&self, f: &mut Frame, t: &Theme) {
        let area = f.area();
        let key_width = HELP
            .iter()
            .map(|(k, _)| k.chars().count())
            .max()
            .unwrap_or(0);
        let want = (HELP.len() + 4) as u16;
        let width: u16 = ((key_width + 44) as u16)
            .min(area.width.saturating_sub(6))
            .clamp(40, 96);
        let height = want.min(area.height.saturating_sub(2));
        let rect = Rect::new(
            area.x + (area.width - width) / 2,
            area.y + (area.height - height) / 2,
            width,
            height,
        );
        f.render_widget(ratatui::widgets::Clear, rect);
        let block = Block::bordered()
            .title(Line::from(vec![
                Span::styled(
                    " HELP ",
                    Style::new().fg(t.accent).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    "click a row to run it · ? closes ",
                    Style::new().fg(t.muted),
                ),
            ]))
            .border_style(Style::new().fg(t.accent))
            .style(Style::new().bg(t.surface));
        let inner = block.inner(rect);
        f.render_widget(block, rect);
        let rows: Vec<Line> = HELP
            .iter()
            .enumerate()
            .map(|(n, (key, what))| {
                if let Some(code) = help_code(key) {
                    self.top
                        .borrow_mut()
                        .push((Rect::new(inner.x, inner.y + n as u16, inner.width, 1), code));
                }
                Line::from(vec![
                    Span::styled(
                        format!(" {key:<key_width$} ", key_width = key_width),
                        Style::new().fg(t.accent),
                    ),
                    Span::styled((*what).to_string(), Style::new().fg(t.muted)),
                ])
            })
            .collect();
        f.render_widget(Paragraph::new(rows), inner);
    }

    fn draw_confirm(&self, f: &mut Frame, title: &str, msg: &str, color: Color, t: &Theme) {
        let area = f.area();
        let width: u16 = ((msg.chars().count() as u16) + 8)
            .min(area.width.saturating_sub(6))
            .clamp(30, 100);
        let inner_w = width.saturating_sub(2).max(1);
        let lines = (msg.chars().count() as u16).div_ceil(inner_w).max(1);
        let height = (lines + 4).min(area.height.saturating_sub(2)).max(5);
        let rect = Rect::new(
            area.x + (area.width - width) / 2,
            area.y + (area.height - height) / 2,
            width,
            height,
        );
        f.render_widget(ratatui::widgets::Clear, rect);
        let block = Block::bordered()
            .title(Line::from(vec![
                Span::styled(title, Style::new().fg(color).add_modifier(Modifier::BOLD)),
                Span::styled("y/n ", Style::new().fg(t.muted)),
            ]))
            .border_style(Style::new().fg(color))
            .style(Style::new().bg(t.surface));
        let inner = block.inner(rect);
        f.render_widget(block, rect);
        let [msg_area, _, btn_area] = Layout::vertical([
            Constraint::Length(lines),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .areas(inner);
        f.render_widget(
            Paragraph::new(Line::styled(format!(" {msg}"), Style::new().fg(t.text)))
                .wrap(Wrap { trim: false }),
            msg_area,
        );
        let pill = |key: &str, what: &str| {
            vec![
                Span::styled(
                    format!(" {key} "),
                    Style::new()
                        .fg(t.accent_ink)
                        .bg(color)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!(" {what}   "), Style::new().fg(t.muted)),
            ]
        };
        let (mut x, mut spans) = (btn_area.x, vec![]);
        hit_span(
            &self.top,
            &mut x,
            btn_area.y,
            &mut spans,
            vec![Span::raw(" ")],
            None,
        );
        hit_span(
            &self.top,
            &mut x,
            btn_area.y,
            &mut spans,
            pill("y", "yes"),
            Some(KeyCode::Char('y')),
        );
        hit_span(
            &self.top,
            &mut x,
            btn_area.y,
            &mut spans,
            pill("n", "no"),
            Some(KeyCode::Char('n')),
        );
        f.render_widget(Paragraph::new(Line::from(spans)), btn_area);
    }

    fn draw(&mut self, f: &mut Frame) {
        if let Some(editor) = &mut self.editor {
            editor.draw(f);
            return;
        }
        if let Some(win) = &mut self.memwin {
            win.draw(f);
            return;
        }
        let t = theme::get();
        self.hits.borrow_mut().clear();
        self.top.borrow_mut().clear();
        self.status_rows.set(None);
        self.modal_rows.set(None);
        f.render_widget(
            Block::new().style(Style::new().fg(t.text).bg(t.bg)),
            f.area(),
        );
        let [header, body, footer] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .areas(f.area());
        self.draw_header(f, header, t);
        self.areas = pane_rects(body, self.show);
        let [projects, list, preview, status] = self.areas;
        if let Some(rect) = projects {
            self.draw_projects(f, rect, t);
        }
        if let Some(rect) = status {
            self.draw_stats(f, rect, t);
        }
        if let Some(rect) = list {
            self.draw_items(f, rect, t);
        }
        if let Some(rect) = preview {
            self.draw_preview(f, rect, t);
        }
        self.draw_footer(f, footer, t);
        match &self.input {
            Input::Confirm(msg) => self.draw_confirm(f, " CONFIRM ", msg, t.bad, t),
            Input::ConfirmQuit(msg) => self.draw_confirm(f, " QUIT ", msg, t.accent, t),
            Input::Approve(msg) => self.draw_confirm(f, " APPROVE ", msg, t.accent, t),
            _ => {}
        }
        if self.help {
            self.draw_help(f, t);
        } else if self.modal.is_some() {
            self.draw_modal(f, t);
        }
    }

    /// The header, two rows: the mark on the left. Beside it the name and version, then the counts.
    fn draw_header(&self, f: &mut Frame, area: Rect, t: &Theme) {
        let (briefs, protos) = self.counts;
        let [mut top, mut bottom] = logo::mark(t).map(|l| l.spans);
        top.insert(0, Span::raw(" "));
        bottom.insert(0, Span::raw(" "));
        top.extend([
            Span::raw("  "),
            Span::styled(
                "brie",
                Style::new().fg(t.accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("  v{}", env!("CARGO_PKG_VERSION")),
                Style::new().fg(t.muted),
            ),
        ]);
        bottom.extend([
            Span::raw("  "),
            Span::styled(self.projects.len().to_string(), Style::new().fg(t.accent)),
            Span::styled(" projects  ", Style::new().fg(t.muted)),
            Span::styled(briefs.to_string(), Style::new().fg(t.accent)),
            Span::styled(" briefs  ", Style::new().fg(t.muted)),
            Span::styled(protos.to_string(), Style::new().fg(t.accent)),
            Span::styled(" prototypes", Style::new().fg(t.muted)),
        ]);
        if !self.running.is_empty() {
            // The servers count is a click target: it opens the servers window, like v.
            let before: u16 = bottom.iter().map(|s| s.width() as u16).sum();
            let text = format!("{} servers (v)", self.running.len());
            self.hits.borrow_mut().push((
                Rect::new(
                    area.x + before + 2,
                    area.y + 1,
                    text.chars().count() as u16,
                    1,
                ),
                KeyCode::Char('v'),
            ));
            bottom.push(Span::styled(format!("  {text}"), Style::new().fg(t.info)));
        }
        let [row0, row1] =
            Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(area);
        f.render_widget(Paragraph::new(Line::from(top)), row0);
        f.render_widget(Paragraph::new(Line::from(bottom)), row1);
    }

    /// Four bars, one for each status, scaled to the largest.
    fn draw_stats(&self, f: &mut Frame, area: Rect, t: &Theme) {
        let max = self.by_status.iter().copied().max().unwrap_or(0).max(1);
        let bar_width = (area.width as usize).saturating_sub(18).clamp(4, 12);
        let rows: Vec<Line> = [
            ("todo", "○", t.muted),
            ("working", "◐", t.accent),
            ("done", "●", t.text),
            ("blocked", "✕", t.bad),
        ]
        .iter()
        .zip(self.by_status)
        .map(|((name, glyph, color), n)| {
            let filled = (n * bar_width).div_ceil(max).min(bar_width);
            Line::from(vec![
                Span::styled(format!("{glyph} "), Style::new().fg(*color)),
                Span::styled(format!("{name:<8}"), Style::new().fg(t.muted)),
                Span::styled("█".repeat(filled), Style::new().fg(*color)),
                Span::styled("░".repeat(bar_width - filled), Style::new().fg(t.line)),
                Span::styled(format!(" {n}"), Style::new().fg(t.text)),
            ])
        })
        .collect();
        // Each row is a click target: it filters the briefs by that status.
        self.status_rows.set(Some(Rect::new(
            area.x + 1,
            area.y + 1,
            area.width.saturating_sub(2),
            4,
        )));
        f.render_widget(
            Paragraph::new(rows).block(self.pane(title_line("5", "STATUS", t), false, t)),
            area,
        );
    }

    /// A framed pane on the surface colour. The focused pane has an amber frame.
    fn pane<'a>(&self, title: Line<'a>, on: bool, t: &Theme) -> Block<'a> {
        Block::bordered()
            .title(title)
            .border_style(Style::new().fg(if on { t.accent } else { t.line }))
            .style(Style::new().bg(t.surface))
    }

    fn highlight(&self, on: bool, t: &Theme) -> Style {
        if on {
            Style::new()
                .fg(t.accent_ink)
                .bg(t.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(t.text).bg(t.line)
        }
    }

    fn draw_projects(&mut self, f: &mut Frame, area: Rect, t: &Theme) {
        let on = self.focus == Focus::Projects;
        let names: Vec<ListItem> = self
            .projects
            .iter()
            .map(|p| ListItem::new(p.name.as_str()))
            .collect();
        // The title is a click target: it focuses the projects.
        self.hits
            .borrow_mut()
            .push((Rect::new(area.x + 1, area.y, 12, 1), KeyCode::Char('h')));
        let list = List::new(names)
            .block(self.pane(title_line("1", "PROJECTS", t), on, t))
            .highlight_style(self.highlight(on, t));
        f.render_stateful_widget(list, area, &mut self.project_state);
    }

    fn draw_items(&mut self, f: &mut Frame, area: Rect, t: &Theme) {
        let on = self.focus == Focus::Items;
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
        let mut summary = format!(
            " {}/{} · {}",
            self.items.len(),
            self.all.len(),
            self.sort.label()
        );
        if self.kind == Kind::Briefs && self.status_filter != StatusFilter::All {
            summary.push_str(&format!(" · {}", self.status_filter.label()));
        }
        if self.window != Window::All {
            summary.push_str(&format!(" · {}", self.window.label()));
        }
        if !self.query.is_empty() {
            summary.push_str(&format!(" · /{}", self.query));
        }
        summary.push(' ');
        let title = Line::from(vec![
            tab(" 2 BRIEFS ", self.kind == Kind::Briefs),
            Span::raw(" "),
            tab(" 3 PROTOTYPES ", self.kind == Kind::Prototypes),
            Span::raw(" "),
        ]);
        // The two tab labels are click targets, like the keys 2 and 3.
        let (w1, w2) = (
            " 2 BRIEFS ".chars().count() as u16,
            " 3 PROTOTYPES ".chars().count() as u16,
        );
        {
            let mut hits = self.hits.borrow_mut();
            hits.push((Rect::new(area.x + 1, area.y, w1, 1), KeyCode::Char('2')));
            hits.push((
                Rect::new(area.x + 2 + w1, area.y, w2, 1),
                KeyCode::Char('3'),
            ));
        }
        let selected = self.item_state.selected();
        let rows: Vec<ListItem> = self
            .items
            .iter()
            .enumerate()
            .map(|(n, i)| {
                let served = if self.running.is_empty() {
                    String::new()
                } else {
                    let canon = std::fs::canonicalize(if i.is_dir {
                        &i.path
                    } else {
                        i.path.parent().unwrap_or(&i.path)
                    })
                    .map(|p| p.display().to_string())
                    .unwrap_or_default();
                    self.running
                        .iter()
                        .find(|e| e.dir == canon)
                        .map(|e| {
                            if e.port == 0 {
                                "  running".to_string()
                            } else {
                                format!("  :{}", e.port)
                            }
                        })
                        .unwrap_or_default()
                };
                let suffix = if i.is_dir { "/" } else { "" };
                let mut spans = vec![];
                if let Some(status) = &i.status {
                    let (glyph, color) = match status.as_str() {
                        "todo" => ("○", t.muted),
                        "working" => ("◐", t.accent),
                        "done" => ("●", t.text),
                        "blocked" => ("✕", t.bad),
                        _ => ("?", t.muted),
                    };
                    let style = if selected == Some(n) {
                        Style::new()
                    } else {
                        Style::new().fg(color)
                    };
                    spans.push(Span::styled(format!("{glyph} "), style));
                }
                spans.push(Span::raw(format!("{}{suffix}{served}", i.name)));
                ListItem::new(Line::from(spans))
            })
            .collect();
        let list = List::new(rows)
            .block(self.pane(title, on, t).title_bottom(Line::styled(
                summary,
                Style::new().fg(if self.filtered() { t.accent } else { t.muted }),
            )))
            .highlight_style(self.highlight(on, t));
        f.render_stateful_widget(list, area, &mut self.item_state);
    }

    fn draw_preview(&self, f: &mut Frame, area: Rect, t: &Theme) {
        let name = self.item().map(|i| i.name.as_str()).unwrap_or("");
        let title = Line::from(vec![
            Span::styled(" 4 ", Style::new().fg(t.accent)),
            Span::styled(
                "PREVIEW ",
                Style::new().fg(t.muted).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("{name} "), Style::new().fg(t.muted)),
        ]);
        let block = self
            .pane(title, false, t)
            .padding(ratatui::widgets::Padding::horizontal(1));
        let inner = block.inner(area);
        let preview = Paragraph::new(self.preview.clone()).wrap(Wrap { trim: false });
        // Wrapped lines take more rows than there are lines: scroll by rows, so the last one can reach the bottom.
        let rows = preview.line_count(inner.width).min(u16::MAX as usize) as u16;
        let max = rows.saturating_sub(inner.height);
        self.scroll_max.set(Some(max));
        let preview = preview.block(block).scroll((self.scroll.min(max), 0));
        f.render_widget(preview, area);
    }

    fn draw_footer(&self, f: &mut Frame, area: Rect, t: &Theme) {
        let line = match &self.input {
            Input::AddProject(s) => prompt("repo path", s, t),
            Input::NewBrief(s) => prompt("brief name", s, t),
            Input::Search(s) => prompt("find", s, t),
            Input::None | Input::Confirm(_) | Input::ConfirmQuit(_) | Input::Approve(_)
                if !self.status.is_empty() =>
            {
                Line::styled(format!(" {}", self.status), Style::new().fg(t.accent))
            }
            Input::None | Input::Confirm(_) | Input::ConfirmQuit(_) | Input::Approve(_) => {
                // Minimal on purpose: the full list lives under `?`. Each hint is a click target for its key.
                let (mut x, mut spans) = (area.x, vec![]);
                hit_span(
                    &self.hits,
                    &mut x,
                    area.y,
                    &mut spans,
                    vec![Span::raw(" ")],
                    None,
                );
                for (key, what, code) in [
                    ("?", " shortcuts  ", KeyCode::Char('?')),
                    ("o", " order  ", KeyCode::Char('o')),
                    ("d", " date  ", KeyCode::Char('d')),
                    ("enter", " open  ", KeyCode::Enter),
                    ("/", " find  ", KeyCode::Char('/')),
                    ("q", " quit", KeyCode::Char('q')),
                ] {
                    let parts = vec![
                        Span::styled(key, Style::new().fg(t.accent)),
                        Span::styled(what, Style::new().fg(t.muted)),
                    ];
                    hit_span(&self.hits, &mut x, area.y, &mut spans, parts, Some(code));
                }
                Line::from(spans)
            }
        };
        f.render_widget(Paragraph::new(line), area);
    }
}

type Hits = RefCell<Vec<(Rect, KeyCode)>>;

/// Put `parts` on row `y` from column `*x`, and make them one click target for `code`.
fn hit_span(
    hits: &Hits,
    x: &mut u16,
    y: u16,
    spans: &mut Vec<Span<'static>>,
    parts: Vec<Span<'static>>,
    code: Option<KeyCode>,
) {
    let width: u16 = parts.iter().map(|p| p.width() as u16).sum();
    if let Some(code) = code {
        hits.borrow_mut().push((Rect::new(*x, y, width, 1), code));
    }
    *x += width;
    spans.extend(parts);
}

fn hit_at(hits: &Hits, at: ratatui::layout::Position) -> Option<KeyCode> {
    hits.borrow()
        .iter()
        .find(|(r, _)| r.contains(at))
        .map(|&(_, code)| code)
}

fn title_line(key: &'static str, label: &'static str, t: &Theme) -> Line<'static> {
    let key = if key.is_empty() {
        " ".to_string()
    } else {
        format!(" {key} ")
    };
    Line::from(vec![
        Span::styled(key, Style::new().fg(t.accent)),
        Span::styled(
            format!("{label} "),
            Style::new().fg(t.muted).add_modifier(Modifier::BOLD),
        ),
    ])
}

fn prompt(label: &str, text: &str, t: &Theme) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!(" {label} "),
            Style::new().fg(t.accent_ink).bg(t.accent),
        ),
        Span::styled(format!(" {text}"), Style::new().fg(t.text)),
        Span::styled("█", Style::new().fg(t.accent)),
    ])
}

fn file_preview(path: &Path) -> Vec<Line<'static>> {
    let t = theme::get();
    let text = read_head(path);
    if text.contains('\0') {
        return vec![Line::styled("(binary file)", Style::new().fg(t.muted))];
    }
    let is_md = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("md") || e.eq_ignore_ascii_case("markdown"));
    if is_md {
        md::render(&text)
    } else {
        text.lines().map(|l| Line::raw(l.to_string())).collect()
    }
}

fn dir_preview(path: &Path) -> Vec<Line<'static>> {
    let t = theme::get();
    let Ok(rd) = fs::read_dir(path) else {
        return vec![];
    };
    let mut entries: Vec<(String, bool)> = rd
        .flatten()
        .map(|e| {
            (
                e.file_name().to_string_lossy().into_owned(),
                e.path().is_dir(),
            )
        })
        .collect();
    entries.sort();
    let mut out: Vec<Line<'static>> = entries
        .into_iter()
        .take(200)
        .map(|(name, dir)| {
            if dir {
                Line::styled(format!("{name}/"), Style::new().fg(t.info))
            } else {
                Line::raw(name)
            }
        })
        .collect();
    out.push(Line::raw(""));
    if let Some(m) = run::manifest(path) {
        let state = if run::is_approved(&m) {
            "approved"
        } else {
            "needs your approval"
        };
        out.push(Line::styled(
            format!("brie.run: {} ({state})", m.cmd),
            Style::new().fg(t.accent),
        ));
        out.push(Line::styled("enter runs it", Style::new().fg(t.muted)));
    } else if ["ts", "js", "mts", "mjs", "cjs"]
        .iter()
        .any(|e| path.join(format!("vite.config.{e}")).is_file())
    {
        out.push(Line::styled(
            "vite.config found: enter runs Vite",
            Style::new().fg(t.accent),
        ));
    } else if path.join("index.html").is_file() {
        out.push(Line::styled(
            "index.html found: enter serves and opens it",
            Style::new().fg(t.accent),
        ));
    }
    out
}

/// Every term must appear in the name or, for a file, in its first 64 KB.
fn matches_terms(item: &Item, terms: &[String]) -> bool {
    if terms.is_empty() {
        return true;
    }
    let name = item.name.to_lowercase();
    let mut body: Option<String> = None;
    terms.iter().all(|term| {
        if let Some(wanted) = term.strip_prefix("status:") {
            return item.status.as_deref() == Some(wanted);
        }
        name.contains(term.as_str())
            || (!item.is_dir
                && body
                    .get_or_insert_with(|| read_head(&item.path).to_lowercase())
                    .contains(term.as_str()))
    })
}

fn read_head(path: &Path) -> String {
    let mut buf = vec![0u8; 64 * 1024];
    let n = fs::File::open(path)
        .and_then(|mut f| f.read(&mut buf))
        .unwrap_or(0);
    String::from_utf8_lossy(&buf[..n]).into_owned()
}

/// The projects column is this share of the width, within these bounds.
const PROJECTS_SHARE: u16 = 22;
const PROJECTS_MIN: u16 = 18;
const PROJECTS_MAX: u16 = 28;
/// The list and the preview go side by side when the area right of the projects is this wide, or this wide on a short terminal.
const SIDE_BY_SIDE: u16 = 100;
const SIDE_BY_SIDE_SHORT: u16 = 80;
const SHORT: u16 = 20;
/// A pane smaller than this is dropped.
const PANE_MIN_W: u16 = 10;
const PANE_MIN_H: u16 = 3;
/// The status box sits under the projects only when the left column is this tall.
const STATUS_MIN_BODY: u16 = 14;

/// Where the panes go: projects, list, preview, status. `None` for a pane that is off or does not fit.
/// Projects and status share a column on the left, status under projects. List and preview share the rest,
/// side by side when it is wide, stacked when not. A pane under the minimum size is dropped: the status first,
/// then the preview, then the list, then the projects.
fn pane_rects(body: Rect, show: [bool; 4]) -> [Option<Rect>; 4] {
    let mut show = show;
    loop {
        let rects = place_panes(body, show);
        let too_small =
            |r: &Option<Rect>| r.is_some_and(|r| r.width < PANE_MIN_W || r.height < PANE_MIN_H);
        match [3, 2, 1, 0].into_iter().find(|&k| too_small(&rects[k])) {
            // The list is how you navigate, so a cramped list gives way to the preview going first.
            Some(1) if show[2] => show[2] = false,
            Some(k) => show[k] = false,
            None => return rects,
        }
    }
}

fn place_panes(body: Rect, show: [bool; 4]) -> [Option<Rect>; 4] {
    let right = show[1] || show[2];
    let (left, rest) = match (show[0] || show[3], right) {
        (true, true) => {
            let width = (body.width * PROJECTS_SHARE / 100).clamp(PROJECTS_MIN, PROJECTS_MAX);
            let [l, r] =
                Layout::horizontal([Constraint::Length(width), Constraint::Min(0)]).areas(body);
            (Some(l), r)
        }
        (true, false) => (Some(body), body),
        _ => (None, body),
    };
    let (projects, status) = match (left, show[0], show[3]) {
        (Some(l), true, true) if l.height >= STATUS_MIN_BODY => {
            let [p, s] = Layout::vertical([Constraint::Min(3), Constraint::Length(6)]).areas(l);
            (Some(p), Some(s))
        }
        (Some(l), true, _) => (Some(l), None),
        (Some(l), false, true) => {
            let [s, _] = Layout::vertical([Constraint::Length(6), Constraint::Min(0)]).areas(l);
            (None, Some(s))
        }
        _ => (None, None),
    };
    if !right {
        return [projects, None, None, status];
    }
    if !(show[1] && show[2]) {
        return [
            projects,
            show[1].then_some(rest),
            show[2].then_some(rest),
            status,
        ];
    }
    let side =
        rest.width >= SIDE_BY_SIDE || (rest.width >= SIDE_BY_SIDE_SHORT && body.height + 3 < SHORT);
    let [list, preview] = if side {
        Layout::horizontal([Constraint::Fill(38), Constraint::Fill(62)]).areas(rest)
    } else {
        Layout::vertical([Constraint::Fill(2), Constraint::Fill(3)]).areas(rest)
    };
    [projects, Some(list), Some(preview), status]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(width: u16, height: u16) -> Rect {
        Rect::new(0, 2, width, height - 3)
    }

    #[test]
    fn wide_terminal_puts_list_and_preview_side_by_side() {
        let [p, l, v, s] = pane_rects(body(140, 32), [true; 4]).map(Option::unwrap);
        assert_eq!(s.width, p.width);
        assert_eq!(p.width, 28);
        assert_eq!((l.width, v.width), (43, 69));
        assert_eq!((l.height, v.height), (29, 29));
        assert_eq!(l.x + l.width, v.x);
    }

    #[test]
    fn status_sits_under_projects_and_can_be_hidden() {
        let [p, _, _, s] = pane_rects(body(140, 32), [true; 4]);
        let (p, s) = (p.unwrap(), s.unwrap());
        assert_eq!(p.y + p.height, s.y);
        assert_eq!(s.height, 6);
        assert_eq!(
            pane_rects(body(140, 32), [true, true, true, false])[3],
            None
        );
        // On its own it takes the left column, whatever the projects do.
        let [p, l, _, s] = pane_rects(body(140, 32), [false, true, true, true]);
        assert!(p.is_none() && s.is_some() && l.unwrap().x == s.unwrap().width);
        // A short terminal has no room for it under the projects.
        assert_eq!(pane_rects(body(140, 14), [true; 4])[3], None);
    }

    #[test]
    fn narrow_area_stacks_list_over_preview() {
        let [p, l, v, _] = pane_rects(body(110, 32), [true; 4]).map(Option::unwrap);
        assert_eq!(p.width, 24);
        assert_eq!(l.width, v.width);
        assert_eq!(l.y + l.height, v.y);
        assert!(l.height < v.height);
    }

    #[test]
    fn short_terminal_goes_side_by_side_from_80_columns() {
        let [_, l, v, _] = pane_rects(body(90, 16), [false, true, true, false]);
        let (l, v) = (l.unwrap(), v.unwrap());
        assert_eq!(l.y, v.y);
        let [_, l, v, _] = pane_rects(body(90, 24), [false, true, true, false]);
        let (l, v) = (l.unwrap(), v.unwrap());
        assert_eq!(l.x, v.x);
    }

    #[test]
    fn a_single_pane_fills_the_area() {
        let b = body(100, 30);
        assert_eq!(
            pane_rects(b, [false, false, true, false]),
            [None, None, Some(b), None]
        );
        assert_eq!(
            pane_rects(b, [true, false, false, false]),
            [Some(b), None, None, None]
        );
        let [p, l, v, _] = pane_rects(b, [true, true, false, false]);
        assert!(p.is_some() && v.is_none());
        assert_eq!(l.unwrap().x, p.unwrap().width);
        assert_eq!(pane_rects(b, [false; 4]), [None; 4]);
    }

    #[test]
    fn a_pane_that_does_not_fit_is_dropped_preview_first() {
        let [p, l, v, _] = pane_rects(body(40, 32), [true; 4]);
        assert!(p.is_some() && l.is_some() && v.is_some());
        // 12 columns: projects take 18, nothing is left for the others, so the preview and the list go.
        let [p, l, v, _] = pane_rects(body(24, 32), [true; 4]);
        assert!(p.is_some() && l.is_none() && v.is_none());
        // Three rows of body split 2:3 leave the stacked panes too short.
        let [_, l, v, _] = pane_rects(body(60, 8), [false, true, true, false]);
        assert!(l.is_some() && v.is_none());
    }

    /// Renders the TUI with demo data to HTML, for the screenshot in `docs/index.html`.
    /// `BRIE_SHOT_OUT=shot.html cargo test --release docs_screenshot -- --ignored`
    #[test]
    #[ignore]
    fn docs_screenshot() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        use std::time::{Duration, SystemTime};

        let Some(out) = std::env::var_os("BRIE_SHOT_OUT") else {
            return;
        };
        let home = std::env::temp_dir().join(format!("brie-shot-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        let project = home.join("harbor");
        fs::create_dir_all(project.join("briefs")).unwrap();
        for n in 0..8 {
            fs::create_dir_all(project.join("prototypes").join(format!("demo-{n}"))).unwrap();
        }
        fs::write(home.join("projects"), "harbor\t/home/dev/harbor\n").unwrap();
        let briefs = [
            ("upgrade-tokio-to-1-40", "todo"),
            ("retry-flaky-payment-test", "working"),
            ("migrate-sessions-table", "blocked"),
            ("fix-login-redirect", "done"),
            ("document-the-webhook-retries", "todo"),
            ("add-csv-export", "working"),
        ];
        for (n, (slug, status)) in briefs.iter().enumerate() {
            let stem = format!("08_10_2026_16:03_{slug}");
            let text = crate::contract::fill(
                crate::contract::DEFAULT_TEMPLATE,
                Path::new("/home/dev/harbor"),
                &stem,
                &slug.replace('-', " "),
            )
            .replace("Status: todo", &format!("Status: {status}"));
            let path = project.join("briefs").join(format!("{stem}.md"));
            fs::write(
                &path,
                format!("{text}\n## Read\n\n- Cargo.toml and Cargo.lock\n"),
            )
            .unwrap();
            let when = SystemTime::now() - Duration::from_secs(60 * n as u64);
            fs::File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(when)
                .unwrap();
        }
        unsafe { std::env::set_var("BRIE_HOME", &home) };

        let mut app = App::new();
        let mut term = Terminal::new(TestBackend::new(104, 26)).unwrap();
        term.draw(|f| app.draw(f)).unwrap();
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
        let glyph = |ch: char| {
            ('\u{2800}'..='\u{28ff}').contains(&ch)
                || "\u{25cb}\u{25d0}\u{25cf}\u{2715}".contains(ch)
        };
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
        fs::write(out, html).unwrap();
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn preview_scrolls_to_its_last_wrapped_row() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = App::new();
        app.show = [true, false, true, true];
        let long = "word ".repeat(120);
        app.preview = vec![
            Line::raw(long.clone()),
            Line::raw(long),
            Line::raw("THE END"),
        ];
        let mut term = Terminal::new(TestBackend::new(70, 20)).unwrap();
        term.draw(|f| app.draw(f)).unwrap();
        app.scroll_by(10_000);
        term.draw(|f| app.draw(f)).unwrap();
        let buf = term.backend().buffer();
        let screen: String = (0..buf.area.height)
            .flat_map(|y| (0..buf.area.width).map(move |x| (x, y)))
            .map(|(x, y)| buf[(x, y)].symbol().to_string())
            .collect();
        assert!(screen.contains("THE END"), "the last line is out of reach");
    }

    #[test]
    fn the_wheel_acts_on_the_pane_under_it() {
        let wheel = |column, row, kind| MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        };
        let mut app = App::new();
        app.areas = [
            Some(Rect::new(0, 0, 10, 10)),
            Some(Rect::new(10, 0, 10, 10)),
            Some(Rect::new(20, 0, 10, 10)),
            None,
        ];
        app.preview = vec![Line::raw("x"); 50];
        app.scroll_max.set(Some(40));
        app.wheel(wheel(25, 5, MouseEventKind::ScrollDown));
        assert_eq!(app.scroll, 3);
        app.wheel(wheel(25, 5, MouseEventKind::ScrollUp));
        assert_eq!(app.scroll, 0);
        // Over the projects, or outside every pane, the wheel does nothing.
        app.wheel(wheel(5, 5, MouseEventKind::ScrollDown));
        app.wheel(wheel(60, 5, MouseEventKind::ScrollDown));
        assert_eq!(app.scroll, 0);
        // Only the wheel counts: a click on the preview does not scroll it.
        app.wheel(wheel(25, 5, MouseEventKind::Moved));
        assert_eq!(app.scroll, 0);
    }

    #[test]
    fn a_click_selects_the_row_under_it() {
        use ratatui::crossterm::event::MouseButton;
        let click = |column, row| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        };
        let mut app = App::new();
        app.projects = ["a", "b", "c"]
            .map(|n| Project {
                name: format!("brie-click-test-{n}"),
                repo: PathBuf::new(),
            })
            .to_vec();
        app.project_state.select(Some(0));
        app.areas = [Some(Rect::new(0, 2, 20, 10)), None, None, None];
        // Row 2 is the frame, rows 3, 4, 5 are the three projects.
        app.mouse(click(5, 4));
        assert_eq!(app.project_state.selected(), Some(1));
        assert!(app.focus == Focus::Projects);
        app.mouse(click(5, 5));
        assert_eq!(app.project_state.selected(), Some(2));
        // The frame, an empty row and a click outside change nothing.
        app.mouse(click(5, 2));
        app.mouse(click(5, 8));
        app.mouse(click(40, 4));
        assert_eq!(app.project_state.selected(), Some(2));
    }

    #[test]
    fn footer_tabs_and_status_rows_are_click_targets() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        use ratatui::crossterm::event::MouseButton;
        let click = |column, row| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        };
        let mut app = App::new();
        let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
        term.draw(|f| app.draw(f)).unwrap();
        // The footer is the last row: " ? shortcuts  o order  d date  enter open  / find  q quit".
        let footer = 29;
        assert_eq!(app.mouse(click(1, footer)), Some(KeyCode::Char('?')));
        assert_eq!(app.mouse(click(14, footer)), Some(KeyCode::Char('o')));
        assert_eq!(app.mouse(click(120 - 3, footer)), None);
        // The tab labels press 2 and 3.
        let list = app.areas[1].unwrap();
        assert_eq!(
            app.mouse(click(list.x + 2, list.y)),
            Some(KeyCode::Char('2'))
        );
        assert_eq!(
            app.mouse(click(list.x + 14, list.y)),
            Some(KeyCode::Char('3'))
        );
        // A row of the status box filters by it; the same row again clears the filter.
        let rows = app.status_rows.get().unwrap();
        app.mouse(click(rows.x + 2, rows.y + 1));
        assert!(app.status_filter == StatusFilter::Working);
        app.mouse(click(rows.x + 2, rows.y + 1));
        assert!(app.status_filter == StatusFilter::All);
        app.mouse(click(rows.x + 2, rows.y + 3));
        assert!(app.status_filter == StatusFilter::Blocked);
        // With the help open, a click closes it, and a row that is one key runs that key.
        app.help = true;
        term.draw(|f| app.draw(f)).unwrap();
        assert_eq!(app.mouse(click(0, 0)), None);
        assert!(!app.help);
        app.help = true;
        term.draw(|f| app.draw(f)).unwrap();
        let new_brief = app
            .top
            .borrow()
            .iter()
            .find(|(_, c)| *c == KeyCode::Char('n'))
            .unwrap()
            .0;
        assert_eq!(
            app.mouse(click(new_brief.x + 1, new_brief.y)),
            Some(KeyCode::Char('n'))
        );
        assert!(!app.help);
    }

    #[test]
    fn sort_cycles_and_round_trips() {
        assert_eq!(Sort::Newest.next(), Sort::Oldest);
        assert_eq!(Sort::Oldest.next(), Sort::AtoZ);
        assert_eq!(Sort::AtoZ.next(), Sort::ZtoA);
        assert_eq!(Sort::ZtoA.next(), Sort::Newest);
        for sort in [Sort::Newest, Sort::Oldest, Sort::AtoZ, Sort::ZtoA] {
            assert_eq!(Sort::parse(sort.label()), sort);
        }
        assert_eq!(Sort::parse("bogus"), Sort::Newest);
    }

    #[test]
    fn window_cycles_and_spans() {
        assert_eq!(Window::All.next(), Window::Day);
        assert_eq!(Window::Day.next(), Window::Week);
        assert_eq!(Window::Week.next(), Window::Month);
        assert_eq!(Window::Month.next(), Window::All);
        assert_eq!(Window::All.span(), None);
        assert_eq!(Window::Day.span(), Some(Duration::from_secs(86_400)));
        assert_eq!(Window::Week.span(), Some(Duration::from_secs(7 * 86_400)));
        assert_eq!(Window::Month.span(), Some(Duration::from_secs(30 * 86_400)));
        for window in [Window::All, Window::Day, Window::Week, Window::Month] {
            assert_eq!(Window::parse(window.label()), window);
        }
        assert_eq!(Window::parse("bogus"), Window::All);
    }

    #[test]
    fn status_filter_cycles_and_matches() {
        assert_eq!(StatusFilter::All.next(), StatusFilter::Todo);
        assert_eq!(StatusFilter::Todo.next(), StatusFilter::Working);
        assert_eq!(StatusFilter::Working.next(), StatusFilter::Done);
        assert_eq!(StatusFilter::Done.next(), StatusFilter::Blocked);
        assert_eq!(StatusFilter::Blocked.next(), StatusFilter::All);
        let some = |s: &str| Some(s.to_string());
        assert!(StatusFilter::All.matches(&None));
        assert!(StatusFilter::All.matches(&some("working")));
        assert!(StatusFilter::Todo.matches(&some("todo")));
        assert!(!StatusFilter::Todo.matches(&some("done")));
        assert!(!StatusFilter::Todo.matches(&None));
        assert!(StatusFilter::Working.matches(&some("working")));
        assert!(StatusFilter::Done.matches(&some("done")));
        assert!(StatusFilter::Blocked.matches(&some("blocked")));
        for filter in [
            StatusFilter::All,
            StatusFilter::Todo,
            StatusFilter::Working,
            StatusFilter::Done,
            StatusFilter::Blocked,
        ] {
            assert_eq!(StatusFilter::parse(filter.label()), filter);
        }
        assert_eq!(StatusFilter::parse("bogus"), StatusFilter::All);
    }
}
