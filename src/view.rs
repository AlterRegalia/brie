//! `brie view <path>`: one file in the whole terminal, drawn by the same Markdown renderer as the
//! preview pane and the editor (`md`). Read only — nothing here writes, and q quits.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use ratatui::Frame;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, MouseEventKind,
};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Padding, Paragraph, Wrap};

use crate::md;
use crate::store::Res;
use crate::theme;

struct Viewer {
    path: PathBuf,
    name: String,
    lines: Vec<Line<'static>>,
    /// In wrapped rows, not source lines: one long line can take several rows.
    scroll: u16,
    max: u16,
    /// The rows the body shows. A page is one of these, not a share of the file.
    height: u16,
    quit: bool,
}

/// Open `path` full screen. The command renders a Markdown file, and shows anything else as it is.
pub fn run(path: &Path) -> Res<()> {
    let mut app = load(path)?;
    let mut terminal = ratatui::init();
    // The wheel scrolls the file, so the mouse is captured here too: select text with `option`
    // (macOS) or `shift`, the same as the TUI.
    let _ = ratatui::crossterm::execute!(std::io::stdout(), EnableMouseCapture);
    let result = app.event_loop(&mut terminal);
    let _ = ratatui::crossterm::execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
    result
}

/// Read the file and render it. Markdown goes by extension. Anything else renders line for line.
fn load(path: &Path) -> Res<Viewer> {
    let path = fs::canonicalize(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if path.is_dir() {
        return Err(format!("{} is a directory", path.display()).into());
    }
    let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    if text.contains('\0') {
        return Err(format!("{} is a binary file", path.display()).into());
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let markdown = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("md") || e.eq_ignore_ascii_case("markdown"));
    let lines = if markdown {
        md::render(&text)
    } else {
        text.lines().map(|l| Line::raw(l.to_string())).collect()
    };
    Ok(Viewer {
        path,
        name,
        lines,
        scroll: 0,
        max: 0,
        height: 0,
        quit: false,
    })
}

impl Viewer {
    fn at(&mut self, first: bool) {
        self.scroll = if first { 0 } else { self.max };
    }

    fn by(&mut self, delta: i32) {
        self.scroll = (self.scroll as i32 + delta).clamp(0, self.max as i32) as u16;
    }

    /// A screenful, less one row: enough to keep a line of context. A page must not grow with the
    /// file, so this is the height of the body and nothing else.
    fn page(&self) -> i32 {
        self.height.saturating_sub(1).max(1) as i32
    }

    /// The wheel scrolls the body, three rows a notch.
    fn wheel(&mut self, kind: MouseEventKind) {
        match kind {
            MouseEventKind::ScrollDown => self.by(3),
            MouseEventKind::ScrollUp => self.by(-3),
            _ => {}
        }
    }

    fn key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char('q') | KeyCode::Esc => self.quit = true,
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Enter => self.by(1),
            KeyCode::Up | KeyCode::Char('k') => self.by(-1),
            KeyCode::PageDown | KeyCode::Char(' ') | KeyCode::Char('J') => self.by(self.page()),
            KeyCode::PageUp | KeyCode::Char('K') | KeyCode::Char('b') => self.by(-self.page()),
            KeyCode::Char('g') | KeyCode::Home => self.at(true),
            KeyCode::Char('G') | KeyCode::End => self.at(false),
            _ => {}
        }
    }

    fn event_loop(&mut self, terminal: &mut ratatui::DefaultTerminal) -> Res<()> {
        while !self.quit {
            terminal.draw(|f| self.draw(f))?;
            if !event::poll(Duration::from_millis(500))? {
                continue;
            }
            match event::read()? {
                Event::Key(k) if k.kind == KeyEventKind::Press => self.key(k.code),
                Event::Mouse(m) => self.wheel(m.kind),
                _ => {}
            }
        }
        Ok(())
    }

    fn draw(&mut self, f: &mut Frame) {
        let t = theme::get();
        f.render_widget(
            Block::new().style(Style::new().fg(t.text).bg(t.bg)),
            f.area(),
        );
        let [body, footer] =
            Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(f.area());

        let block = Block::bordered()
            .title(Line::from(vec![
                Span::styled(
                    " VIEW ",
                    Style::new().fg(t.accent).add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!("{} ", self.name), Style::new().fg(t.muted)),
            ]))
            .border_style(Style::new().fg(t.accent))
            .style(Style::new().bg(t.surface))
            .padding(Padding::horizontal(1));
        let inner = block.inner(body);
        let text = Paragraph::new(self.lines.clone()).wrap(Wrap { trim: false });
        // Wrapped lines take more rows than there are lines, so scroll by rows, not by lines.
        let rows = text.line_count(inner.width).min(u16::MAX as usize) as u16;
        self.max = rows.saturating_sub(inner.height);
        self.height = inner.height;
        self.scroll = self.scroll.min(self.max);
        f.render_widget(text.block(block).scroll((self.scroll, 0)), body);

        let at = if self.max == 0 {
            "all of it".to_string()
        } else {
            format!("{:>3}%", 100 * self.scroll as usize / self.max as usize)
        };
        let mut spans = vec![
            Span::styled(
                format!(" {} ", self.name),
                Style::new().fg(t.accent_ink).bg(t.accent),
            ),
            Span::styled(format!(" {at}  "), Style::new().fg(t.text)),
        ];
        // Where the file lives is worth a line: the title carries the name only.
        let dir = self
            .path
            .parent()
            .map(|d| d.display().to_string())
            .unwrap_or_default();
        spans.push(Span::styled(format!("{dir}  "), Style::new().fg(t.muted)));
        for (k, what) in [
            ("q", "close"),
            ("j/k", "scroll"),
            ("space/b", "page"),
            ("g/G", "top/bottom"),
        ] {
            spans.push(Span::styled(k, Style::new().fg(t.accent)));
            spans.push(Span::styled(format!(" {what}  "), Style::new().fg(t.muted)));
        }
        f.render_widget(Paragraph::new(Line::from(spans)), footer);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn file(name: &str, text: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("brie-view-{}-{}", std::process::id(), name));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        fs::write(&path, text).unwrap();
        path
    }

    /// What the screen shows, as text.
    fn screen(app: &mut Viewer, w: u16, h: u16) -> String {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| app.draw(f)).unwrap();
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

    #[test]
    fn a_markdown_file_is_drawn_rendered_not_as_its_source() {
        let path = file(
            "brief.md",
            "# Plan — the podium\n\nStatus: working\n\n- one thing *said*\n- `code` here\n",
        );
        let mut app = load(&path).unwrap();
        assert_eq!(app.name, "brief.md");
        let out = screen(&mut app, 60, 12);
        assert!(
            out.contains("PLAN — THE PODIUM"),
            "the H1 is drawn, uppercased:\n{out}"
        );
        assert!(
            out.contains("• one thing said"),
            "a bullet loses its dash:\n{out}"
        );
        assert!(out.contains("code"), "{out}");
        assert!(!out.contains("# Plan"), "the markers are not shown:\n{out}");
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn the_footer_says_where_the_file_is() {
        let path = file("where.md", "# Hi\n");
        let mut app = load(&path).unwrap();
        // Wide, because the path is long and a terminal clips what does not fit.
        let out = screen(&mut app, 200, 8);
        assert!(out.contains(" VIEW where.md "), "{out}");
        assert!(
            out.contains(&path.parent().unwrap().display().to_string()),
            "{out}"
        );
        assert!(out.contains("q close"), "and how to leave: {out}");
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn anything_but_markdown_is_shown_as_it_is() {
        let path = file("notes.txt", "keep   the   spacing\n# not a heading\n");
        let mut app = load(&path).unwrap();
        let out = screen(&mut app, 60, 8);
        assert!(out.contains("# not a heading"), "{out}");
        assert!(out.contains("keep   the   spacing"), "{out}");
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn scrolling_stops_at_both_ends_and_the_last_line_can_be_reached() {
        let body: String = (0..200).map(|i| format!("line {i}\n")).collect();
        let path = file("long.md", &body);
        let mut app = load(&path).unwrap();
        let _ = screen(&mut app, 40, 10);
        assert!(
            app.max > 100,
            "200 lines in a 10-row window: max {}",
            app.max
        );
        app.key(KeyCode::Char('j'));
        assert_eq!(app.scroll, 1);
        app.key(KeyCode::Char('k'));
        assert_eq!(app.scroll, 0);
        app.key(KeyCode::Char('k'));
        assert_eq!(app.scroll, 0, "the top is the top");
        app.key(KeyCode::Char('G'));
        assert_eq!(app.scroll, app.max);
        assert!(
            screen(&mut app, 40, 10).contains("line 199"),
            "the end is reachable"
        );
        app.key(KeyCode::Char('G'));
        assert_eq!(app.scroll, app.max, "the bottom is the bottom");
        app.key(KeyCode::Char('g'));
        assert_eq!(app.scroll, 0);
        // Every key the footer promises works, including the pair under "space/b".
        app.key(KeyCode::Char(' '));
        let down = app.scroll;
        assert!(down > 0, "space turns a page");
        // A page is one screen, not a share of the file: 200 lines over a 10-row window must not
        // jump a quarter of the way down.
        assert_eq!(
            down as u16,
            app.height - 1,
            "a page is the body's height, less a row of context"
        );
        assert!(down < 10, "and it does not grow with the file: {down}");
        app.key(KeyCode::Char('b'));
        assert_eq!(app.scroll, 0, "b turns it back");
        app.key(KeyCode::Char('b'));
        assert_eq!(app.scroll, 0, "and stops at the top");
        // The wheel scrolls the body too, three rows a notch. (`run` captures the mouse for it;
        // that part is a terminal side effect and is not covered here.)
        app.wheel(MouseEventKind::ScrollDown);
        assert_eq!(app.scroll, 3);
        app.wheel(MouseEventKind::ScrollUp);
        assert_eq!(app.scroll, 0);
        app.wheel(MouseEventKind::Moved);
        assert_eq!(app.scroll, 0, "no other mouse event moves the file");
        app.key(KeyCode::Char('q'));
        assert!(app.quit);
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_directory_and_a_binary_file_are_refused() {
        let dir = std::env::temp_dir().join(format!("brie-view-dir-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        assert!(
            match load(&dir) {
                Err(e) => e.to_string(),
                Ok(_) => String::new(),
            }
            .contains("is a directory")
        );
        let bin = dir.join("thing.bin");
        fs::write(&bin, [0u8, 1, 2, 0]).unwrap();
        assert!(
            match load(&bin) {
                Err(e) => e.to_string(),
                Ok(_) => String::new(),
            }
            .contains("binary")
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
