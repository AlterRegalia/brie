//! `brie daemon`: a menu bar item (macOS). Beside the icon it shows the working and blocked counts as pills, or as a dot on the icon. Clicking it opens a
//! small panel (a borderless window with a web view, `panel.html`): a search field, one row for each project with open
//! briefs that expands to its briefs (click one to open it in a brie window of its own, or use a button to
//! open a Terminal, reveal it in Finder or copy its path), the last done briefs,
//! and the actions Refresh, Open store, Send test push, Pills in menu bar, Start at login and Quit.
//! From a terminal, `brie daemon` makes sure the daemon runs in the background, then asks: 1 quit it, 2 keep it.

// Off macOS only the tests and `brie daemon`'s stub use the menu bar code.
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use std::fs;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use crate::html;
use crate::maint;
use crate::store::{self, Kind, Res};

const LABEL: &str = "com.brie.daemon";

fn pid_file() -> PathBuf {
    store::root().join("daemon.pid")
}

fn plist_path() -> PathBuf {
    store::home()
        .join("Library/LaunchAgents")
        .join(format!("{LABEL}.plist"))
}

/// The pid of a running daemon, if the pid file names a live process.
fn running_pid() -> Option<u32> {
    let pid: u32 = fs::read_to_string(pid_file()).ok()?.trim().parse().ok()?;
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(Stdio::null())
        .status()
        .ok()?
        .success()
        .then_some(pid)
}

/// An open brief, for the dropdown.
#[derive(Clone, PartialEq)]
struct Row {
    status: String,
    project: String,
    name: String,
    path: PathBuf,
}

/// Everything the dropdown shows. It is rebuilt only when this changes.
#[derive(Clone, PartialEq, Default)]
struct Snapshot {
    /// Working, then todo, then blocked. Newest first in a group.
    open: Vec<Row>,
    /// The last few done briefs, newest first.
    done: Vec<Row>,
    /// The kind of the push target (`ntfy` or `url`), never the target itself: it is a secret.
    push: Option<String>,
}

const RECENT: usize = 3;

fn snapshot() -> Snapshot {
    let mut open = vec![];
    let mut done = vec![];
    for p in store::projects() {
        for item in store::list(&p.name, Kind::Briefs) {
            if item.is_dir {
                continue;
            }
            let status = item.status.clone().unwrap_or_default();
            let row = Row {
                status: status.clone(),
                project: p.name.clone(),
                name: short_name(&item.name),
                path: item.path,
            };
            if rank(&status).is_some() {
                open.push(row);
            } else if status == "done" {
                done.push((item.modified, row));
            }
        }
    }
    open.sort_by_key(|r| rank(&r.status));
    done.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    Snapshot {
        open,
        done: done.into_iter().take(RECENT).map(|(_, r)| r).collect(),
        push: maint::notify_target().map(|t| {
            t.split_once(':')
                .map_or(t.clone(), |(kind, _)| kind.to_string())
        }),
    }
}

fn rank(status: &str) -> Option<usize> {
    ["working", "todo", "blocked"]
        .iter()
        .position(|s| *s == status)
}

/// `08_10_2026_16:03_fix-login-redirect.md` -> `fix-login-redirect`.
fn short_name(file: &str) -> String {
    let stem = file.strip_suffix(".md").unwrap_or(file);
    let parts: Vec<&str> = stem.splitn(5, '_').collect();
    let dated = parts.len() == 5
        && parts[..3]
            .iter()
            .all(|p| p.bytes().all(|b| b.is_ascii_digit()))
        && parts[3].contains(':');
    if dated { parts[4] } else { stem }.to_string()
}

/// The first line of the dropdown: the version and the counts.
fn summary(rows: &[Row]) -> String {
    let count = |s: &str| rows.iter().filter(|r| r.status == s).count();
    let blocked = count("blocked");
    format!(
        "brie v{} ({} working, {} todo{})",
        env!("CARGO_PKG_VERSION"),
        count("working"),
        count("todo"),
        if blocked > 0 {
            format!(", {blocked} blocked")
        } else {
            String::new()
        }
    )
}

/// How the menu bar shows the counts: `Pills` draws a pill for working and one for blocked beside the icon.
/// `Icon` puts a dot on the icon (red if a brief is blocked, amber if one is working) and the working count as text.
#[derive(Clone, Copy, PartialEq, Debug)]
enum BarStyle {
    Pills,
    Icon,
}

fn style_file() -> PathBuf {
    store::root().join("barstyle")
}

fn bar_style() -> BarStyle {
    match fs::read_to_string(style_file()) {
        Ok(s) if s.trim() == "icon" => BarStyle::Icon,
        _ => BarStyle::Pills,
    }
}

fn set_bar_style(style: BarStyle) -> Res<()> {
    let word = if style == BarStyle::Icon {
        "icon"
    } else {
        "pills"
    };
    fs::write(style_file(), word)?;
    Ok(())
}

/// (working, blocked) among the open briefs.
fn bar_counts(rows: &[Row]) -> (usize, usize) {
    let count = |s: &str| rows.iter().filter(|r| r.status == s).count();
    (count("working"), count("blocked"))
}

/// The text beside the menu bar icon: only in the `Icon` style, and only the working count.
fn bar_title(rows: &[Row], style: BarStyle) -> Option<String> {
    let (working, _) = bar_counts(rows);
    (style == BarStyle::Icon && working > 0).then(|| working.to_string())
}

const AMBER: [u8; 3] = [255, 196, 0];
const RED: [u8; 3] = [255, 90, 79];

/// 3x5 digits, one row per `u8`, the low three bits are the pixels.
const DIGITS: [[u8; 5]; 10] = [
    [7, 5, 5, 5, 7],
    [2, 6, 2, 2, 7],
    [7, 1, 7, 4, 7],
    [7, 1, 7, 1, 7],
    [5, 5, 7, 1, 1],
    [7, 4, 7, 1, 7],
    [7, 4, 7, 5, 7],
    [7, 1, 1, 1, 1],
    [7, 5, 7, 5, 7],
    [7, 5, 7, 1, 7],
];

/// The menu bar image, RGBA: the briefcase, then (for `Pills`) a pill for each non-zero count, or (for `Icon`) a dot on the icon.
/// It is not a template image, because the pills and the dot have a color, so the briefcase is drawn light or dark to match the bar.
fn bar_image(style: BarStyle, working: usize, blocked: usize, dark: bool) -> (Vec<u8>, u32, u32) {
    const H: u32 = 44;
    const GAP: u32 = 4;
    let ink: [u8; 3] = if dark { [244, 245, 248] } else { [24, 24, 28] };
    let pills: Vec<(usize, [u8; 3], [u8; 3])> = match style {
        BarStyle::Pills => [
            (working, AMBER, [20, 14, 0]),
            (blocked, RED, [255, 255, 255]),
        ]
        .into_iter()
        .filter(|p| p.0 > 0)
        .collect(),
        BarStyle::Icon => vec![],
    };
    let digits = |n: usize| n.min(99).to_string().len() as u32;
    let text_w = |n: usize| digits(n) * 9 + (digits(n) - 1) * 3;
    let pill_w = |n: usize| 17 + text_w(n);
    let width = H + pills.iter().map(|p| GAP + pill_w(p.0)).sum::<u32>();
    let mut rgba = vec![0u8; (width * H * 4) as usize];
    let mut put = |x: u32, y: u32, c: [u8; 3], a: f32| {
        let i = ((y * width + x) * 4) as usize;
        let a0 = rgba[i + 3] as f32 / 255.0;
        let out = a + a0 * (1.0 - a);
        for k in 0..3 {
            let v = (c[k] as f32 * a + rgba[i + k] as f32 * a0 * (1.0 - a)) / out.max(1e-6);
            rgba[i + k] = v as u8;
        }
        rgba[i + 3] = (out * 255.0) as u8;
    };
    let inside =
        |x: f32, y: f32, x0: f32, y0: f32, x1: f32, y1: f32| x >= x0 && x < x1 && y >= y0 && y < y1;
    let dot = match style {
        BarStyle::Icon if blocked > 0 => Some(RED),
        BarStyle::Icon if working > 0 => Some(AMBER),
        _ => None,
    };
    for py in 0..H {
        for px in 0..H {
            let (x, y) = (px as f32 + 0.5, py as f32 + 0.5);
            let body = inside(x, y, 4.0, 14.0, 40.0, 38.0) && !inside(x, y, 7.0, 17.0, 37.0, 35.0);
            let handle =
                inside(x, y, 15.0, 6.0, 29.0, 14.0) && !inside(x, y, 18.0, 9.0, 26.0, 14.0);
            let wedge = inside(x, y, 11.0, 17.0, 33.0, 32.0) && y >= 31.0 - (x - 11.0) * 0.6;
            let near_dot = dot.is_some() && (x - 36.0).hypot(y - 9.0) < 9.5;
            if (body || handle || wedge) && !near_dot {
                put(px, py, ink, 1.0);
            }
            if let Some(c) = dot {
                let d = (x - 36.0).hypot(y - 9.0);
                put(px, py, c, (7.5 - d).clamp(0.0, 1.0));
            }
        }
    }
    let mut x0 = H + GAP;
    for (n, bg, fg) in pills {
        let (w, top, h) = (pill_w(n), 9u32, 26u32);
        let r = h as f32 / 2.0;
        for py in 0..h {
            for px in 0..w {
                let (x, y) = (px as f32 + 0.5, py as f32 + 0.5);
                let cx = x.clamp(r, w as f32 - r);
                let d = (x - cx).hypot(y - r);
                put(x0 + px, top + py, bg, (r - d + 0.5).clamp(0.0, 1.0));
            }
        }
        let text = n.min(99).to_string();
        let mut gx = x0 + (w - text_w(n)) / 2;
        for ch in text.chars() {
            let glyph = DIGITS[ch.to_digit(10).unwrap_or(0) as usize];
            for (row, bits) in glyph.iter().enumerate() {
                for col in 0..3u32 {
                    if bits >> (2 - col) & 1 == 1 {
                        for dy in 0..3 {
                            for dx in 0..3 {
                                put(gx + col * 3 + dx, top + 5 + row as u32 * 3 + dy, fg, 1.0);
                            }
                        }
                    }
                }
            }
            gx += 12;
        }
        x0 += w + GAP;
    }
    (rgba, width, H)
}

/// Whether the menu bar is dark (the system appearance).
#[cfg(target_os = "macos")]
fn bar_is_dark() -> bool {
    Command::new("defaults")
        .args(["read", "-g", "AppleInterfaceStyle"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("Dark"))
        .unwrap_or(false)
}

/// `2 working · 1 todo`
fn meta_label(rows: &[&Row]) -> String {
    let parts: Vec<String> = ["working", "todo", "blocked"]
        .iter()
        .filter_map(|s| {
            let n = rows.iter().filter(|r| r.status == *s).count();
            (n > 0).then(|| format!("{n} {s}"))
        })
        .collect();
    parts.join(" · ")
}

/// The dot of a group: red when a brief is blocked, amber when one is working, else muted.
fn group_dot(rows: &[&Row]) -> &'static str {
    let has = |s: &str| rows.iter().any(|r| r.status == s);
    if has("blocked") {
        "blocked"
    } else if has("working") {
        "working"
    } else {
        "todo"
    }
}

/// What the panel draws, as JSON: one group for each project with open briefs, then the last done briefs.
fn panel_json(snap: &Snapshot, auto: bool, pills: bool) -> String {
    use crate::json::Json;
    let brief = |r: &Row| {
        Json::Obj(vec![
            ("name".into(), Json::Str(r.name.clone())),
            ("status".into(), Json::Str(r.status.clone())),
            ("path".into(), Json::Str(r.path.display().to_string())),
        ])
    };
    let group = |name: &str, dot: &str, meta: String, rows: &[&Row]| {
        Json::Obj(vec![
            ("name".into(), Json::Str(name.into())),
            ("dot".into(), Json::Str(dot.into())),
            ("meta".into(), Json::Str(meta)),
            (
                "briefs".into(),
                Json::Arr(rows.iter().map(|r| brief(r)).collect()),
            ),
        ])
    };
    let mut projects: Vec<&str> = snap.open.iter().map(|r| r.project.as_str()).collect();
    projects.sort();
    projects.dedup();
    let mut groups = vec![];
    for name in projects {
        let mine: Vec<&Row> = snap.open.iter().filter(|r| r.project == name).collect();
        groups.push(group(name, group_dot(&mine), meta_label(&mine), &mine));
    }
    if !snap.done.is_empty() {
        let done: Vec<&Row> = snap.done.iter().collect();
        groups.push(group(
            "Recently done",
            "done",
            format!("last {}", done.len()),
            &done,
        ));
    }
    Json::Obj(vec![
        (
            "version".into(),
            Json::Str(format!("v{}", env!("CARGO_PKG_VERSION"))),
        ),
        ("total".into(), Json::Num(snap.open.len() as f64)),
        (
            "push".into(),
            snap.push.clone().map_or(Json::Null, Json::Str),
        ),
        ("auto".into(), Json::Bool(auto)),
        ("pills".into(), Json::Bool(pills)),
        ("groups".into(), Json::Arr(groups)),
    ])
    .write()
}

fn xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn autostart_on() -> bool {
    plist_path().exists()
}

/// Write or remove the launchd agent. It takes effect at the next login. This daemon keeps running.
fn set_autostart(on: bool) -> Res<()> {
    let path = plist_path();
    if !on {
        if path.exists() {
            fs::remove_file(&path)?;
        }
        return Ok(());
    }
    let exe = std::env::current_exe()?;
    let plist = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\">\n\
<dict>\n\
  <key>Label</key><string>{LABEL}</string>\n\
  <key>ProgramArguments</key><array><string>{}</string><string>daemon</string></array>\n\
  <key>RunAtLoad</key><true/>\n\
</dict>\n\
</plist>\n",
        xml(&exe.display().to_string())
    );
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(path, plist)?;
    Ok(())
}

#[derive(Debug, PartialEq)]
enum Choice {
    Quit,
    Stay,
}

/// The two keys of the prompt: 1 quits the daemon, 2 (or Enter) leaves it in the background.
/// Any other key means nothing, so a stray key never stops it.
fn choice(c: char) -> Option<Choice> {
    match c {
        '1' => Some(Choice::Quit),
        '2' | '\n' | '\r' => Some(Choice::Stay),
        _ => None,
    }
}

/// Wait for one key: 1, 2 or Enter. Esc, q and ctrl-c leave the daemon where it is.
fn ask() -> Res<Choice> {
    use ratatui::crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, read};
    use ratatui::crossterm::terminal::{disable_raw_mode, enable_raw_mode};
    enable_raw_mode()?;
    let picked = loop {
        match read() {
            Ok(Event::Key(k)) if k.kind == KeyEventKind::Press => match k.code {
                KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                    break Choice::Stay;
                }
                KeyCode::Esc | KeyCode::Char('q') => break Choice::Stay,
                KeyCode::Enter => break Choice::Stay,
                KeyCode::Char(c) => {
                    if let Some(p) = choice(c) {
                        break p;
                    }
                }
                _ => {}
            },
            Ok(_) => {}
            Err(_) => break Choice::Stay,
        }
    };
    let _ = disable_raw_mode();
    Ok(picked)
}

fn start() -> Res<String> {
    if let Some(pid) = running_pid() {
        return Ok(format!("brie daemon already runs (pid {pid})"));
    }
    let exe = std::env::current_exe()?;
    Command::new("nohup")
        .arg(exe)
        .arg("daemon")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    for _ in 0..30 {
        if let Some(pid) = running_pid() {
            return Ok(format!("brie daemon started (pid {pid})"));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    Err("brie daemon did not start".into())
}

fn stop() -> Res<String> {
    match running_pid() {
        Some(pid) => {
            Command::new("kill").arg(pid.to_string()).status()?;
            let _ = fs::remove_file(pid_file());
            Ok(format!("brie daemon stopped (pid {pid})"))
        }
        None => Ok("brie daemon does not run".into()),
    }
}

/// `brie daemon` in a terminal: run it in the background, then ask what to do with it.
fn interactive() -> Res<String> {
    println!("{}", start()?);
    println!("  1  quit it");
    println!("  2  keep it in the background");
    match ask()? {
        Choice::Quit => stop(),
        Choice::Stay => Ok("brie daemon stays in the background".into()),
    }
}

pub fn run(args: &[String]) -> Res<String> {
    match args.first().map(String::as_str) {
        None => {
            if cfg!(target_os = "macos")
                && std::io::stdin().is_terminal()
                && std::io::stdout().is_terminal()
            {
                return interactive();
            }
            if let Some(pid) = running_pid() {
                return Ok(format!("brie daemon already runs (pid {pid})"));
            }
            tray()?;
            Ok(String::new())
        }
        Some("start") => start(),
        Some("stop") => stop(),
        Some("status") => Ok(format!(
            "{}\nauto-start: {}",
            match running_pid() {
                Some(pid) => format!("running (pid {pid})"),
                None => "not running".into(),
            },
            if autostart_on() { "on" } else { "off" }
        )),
        Some(other) => {
            Err(format!("unknown daemon command {other}: use start, stop or status").into())
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn tray() -> Res<()> {
    Err("brie daemon needs macOS (a menu bar item)".into())
}

/// The menu bar icon: a briefcase with a wedge of brie in it, black on clear so macOS tints it.
#[cfg(target_os = "macos")]
fn icon() -> tray_icon::Icon {
    const N: u32 = 44;
    let inside =
        |x: f32, y: f32, x0: f32, y0: f32, x1: f32, y1: f32| x >= x0 && x < x1 && y >= y0 && y < y1;
    let mut rgba = vec![0u8; (N * N * 4) as usize];
    for py in 0..N {
        for px in 0..N {
            let (x, y) = (px as f32, py as f32);
            let body = inside(x, y, 4.0, 14.0, 40.0, 38.0) && !inside(x, y, 7.0, 17.0, 37.0, 35.0);
            let handle =
                inside(x, y, 15.0, 6.0, 29.0, 14.0) && !inside(x, y, 18.0, 9.0, 26.0, 14.0);
            let wedge = inside(x, y, 11.0, 17.0, 33.0, 32.0) && y >= 31.0 - (x - 11.0) * 0.6;
            if body || handle || wedge {
                let i = ((py * N + px) * 4) as usize;
                rgba[i + 3] = 255;
            }
        }
    }
    tray_icon::Icon::from_rgba(rgba, N, N).expect("icon is N by N pixels")
}

#[cfg(target_os = "macos")]
fn copy_to_clipboard(text: &str) {
    use std::io::Write;
    if let Ok(mut child) = Command::new("pbcopy").stdin(Stdio::piped()).spawn() {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text.as_bytes());
        }
        let _ = child.wait();
    }
}

/// The AppleScript that runs `cat` on a file in a Terminal window. When Terminal is not running yet it
/// uses the window that opens with it, so no second, empty window appears.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn terminal_script(path: &std::path::Path) -> String {
    let shell = format!(
        "clear; cat '{}'",
        path.display().to_string().replace('\'', "'\\''")
    );
    let command = shell.replace('\\', "\\\\").replace('"', "\\\"");
    format!(
        "set wasRunning to application \"Terminal\" is running\n\
tell application \"Terminal\"\n\
  activate\n\
  if wasRunning then\n\
    do script \"{command}\"\n\
  else\n\
    repeat 25 times\n\
      if (count of windows) > 0 then exit repeat\n\
      delay 0.2\n\
    end repeat\n\
    if (count of windows) > 0 then\n\
      do script \"{command}\" in window 1\n\
    else\n\
      do script \"{command}\"\n\
    end if\n\
  end if\n\
end tell"
    )
}

/// Show a brief with `cat` in a Terminal window.
#[cfg(target_os = "macos")]
fn open_in_terminal(path: &std::path::Path) {
    let _ = Command::new("osascript")
        .args(["-e", &terminal_script(path)])
        .status();
}

#[cfg(target_os = "macos")]
const PANEL_W: f64 = 340.0;
#[cfg(target_os = "macos")]
const PANEL_H: f64 = 520.0;

#[cfg(target_os = "macos")]
fn tray() -> Res<()> {
    use std::time::{Duration, Instant};
    use tao::dpi::{LogicalSize, PhysicalPosition};
    use tao::event::{Event, StartCause, WindowEvent};
    use tao::event_loop::{ControlFlow, EventLoopBuilder};
    use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS, WindowExtMacOS};
    use tao::window::WindowBuilder;
    use tray_icon::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
    use wry::WebViewBuilder;

    enum Ev {
        Tray(TrayIconEvent),
        Ipc(String),
    }

    fs::create_dir_all(store::root())?;
    fs::write(pid_file(), std::process::id().to_string())?;

    let mut event_loop = EventLoopBuilder::<Ev>::with_user_event().build();
    event_loop.set_activation_policy(ActivationPolicy::Accessory);
    let proxy = event_loop.create_proxy();
    TrayIconEvent::set_event_handler(Some({
        let proxy = proxy.clone();
        move |e| {
            let _ = proxy.send_event(Ev::Tray(e));
        }
    }));

    let window = WindowBuilder::new()
        .with_decorations(false)
        .with_transparent(true)
        .with_always_on_top(true)
        .with_resizable(false)
        .with_visible(false)
        .with_inner_size(LogicalSize::new(PANEL_W, PANEL_H))
        .build(&event_loop)
        .map_err(|e| e.to_string())?;
    // The system draws its own edge and shadow around a window; the panel has its own border.
    window.set_has_shadow(false);
    let panel = window.id();
    let webview = WebViewBuilder::new()
        .with_transparent(true)
        .with_html(include_str!("panel.html"))
        .with_ipc_handler({
            // the panel's own proxy; the one that outlives it opens brief windows
            let proxy = proxy.clone();
            move |req| {
                let _ = proxy.send_event(Ev::Ipc(req.body().clone()));
            }
        })
        .build(&window)
        .map_err(|e| e.to_string())?;

    let mut snap = snapshot();
    let tray = TrayIconBuilder::new()
        .with_icon(icon())
        .build()
        .map_err(|e| e.to_string())?;
    // Icon, title and tooltip follow the snapshot, the style and the system appearance.
    let paint = |tray: &tray_icon::TrayIcon, snap: &Snapshot| {
        let style = bar_style();
        let (w, b) = bar_counts(&snap.open);
        let (rgba, width, height) = bar_image(style, w, b, bar_is_dark());
        if let Ok(img) = tray_icon::Icon::from_rgba(rgba, width, height) {
            let _ = tray.set_icon(Some(img));
        }
        tray.set_title(bar_title(&snap.open, style));
        let _ = tray.set_tooltip(Some(summary(&snap.open)));
    };
    paint(&tray, &snap);
    let mut hidden_at = Instant::now() - Duration::from_secs(1);

    /// One brief in one window. Kept alive here for as long as the window is open.
    struct Viewer {
        token: String,
        window: tao::window::Window,
        _web: wry::WebView,
    }

    /// A brief in its own window, drawn by the same renderer as the TUI (`html`). The window is the
    /// panel's web view with a file in it. It closes with its own button or with escape.
    fn brief_window(
        target: &tao::event_loop::EventLoopWindowTarget<Ev>,
        path: &std::path::Path,
        token: String,
        proxy: tao::event_loop::EventLoopProxy<Ev>,
    ) -> Result<Viewer, String> {
        let source = fs::read_to_string(path).map_err(|e| e.to_string())?;
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let page = html::page(&name, &path.display().to_string(), &source, &token);
        let window = WindowBuilder::new()
            .with_title(format!("brie — {name}"))
            .with_inner_size(LogicalSize::new(780.0, 720.0))
            .build(target)
            .map_err(|e| e.to_string())?;
        let web = WebViewBuilder::new()
            .with_html(page)
            .with_ipc_handler(move |req| {
                let _ = proxy.send_event(Ev::Ipc(req.body().clone()));
            })
            .build(&window)
            .map_err(|e| e.to_string())?;
        Ok(Viewer {
            token,
            window,
            _web: web,
        })
    }

    let mut viewers: Vec<Viewer> = vec![];
    let mut tokens = 0;
    event_loop.run(move |event, target, flow| {
        *flow = ControlFlow::WaitUntil(Instant::now() + Duration::from_secs(5));
        let push_data = |snap: &Snapshot| {
            let _ = webview.evaluate_script(&format!(
                "render({})",
                panel_json(snap, autostart_on(), bar_style() == BarStyle::Pills)
            ));
        };
        match event {
            Event::NewEvents(StartCause::ResumeTimeReached { .. }) => {
                let now = snapshot();
                if now != snap {
                    snap = now;
                    paint(&tray, &snap);
                    push_data(&snap);
                } else {
                    paint(&tray, &snap);
                }
            }
            Event::WindowEvent {
                window_id,
                event: WindowEvent::Focused(false),
                ..
            } if window_id == panel => {
                if window.is_visible() {
                    window.set_visible(false);
                    hidden_at = Instant::now();
                }
            }
            Event::WindowEvent {
                window_id,
                event: WindowEvent::CloseRequested,
                ..
            } if window_id == panel => window.set_visible(false),
            Event::WindowEvent {
                window_id,
                event: WindowEvent::CloseRequested,
                ..
            } => viewers.retain(|v| v.window.id() != window_id),
            Event::UserEvent(Ev::Tray(TrayIconEvent::Click {
                rect,
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            })) => {
                // Clicking the icon while the panel is open first takes its focus, which hides it; do not reopen.
                if window.is_visible() {
                    window.set_visible(false);
                } else if hidden_at.elapsed() > Duration::from_millis(250) {
                    snap = snapshot();
                    paint(&tray, &snap);
                    push_data(&snap);
                    let scale = window.scale_factor();
                    let width = PANEL_W * scale;
                    let x = (rect.position.x + rect.size.width as f64 / 2.0 - width / 2.0).max(8.0);
                    let y = rect.position.y + rect.size.height as f64 + 4.0;
                    window.set_outer_position(PhysicalPosition::new(x, y));
                    window.set_visible(true);
                    window.set_focus();
                    let _ = webview.evaluate_script("shown()");
                }
            }
            Event::UserEvent(Ev::Ipc(msg)) => {
                let (action, arg) = msg.split_once('\t').unwrap_or((msg.as_str(), ""));
                // A path counts only when it is one the panel was given.
                let known = |arg: &str| {
                    snap.open
                        .iter()
                        .chain(&snap.done)
                        .any(|r| r.path.display().to_string() == arg)
                };
                match action {
                    "open" if known(arg) => open_in_terminal(std::path::Path::new(arg)),
                    "view" if known(arg) => {
                        tokens += 1;
                        let token = tokens.to_string();
                        match brief_window(target, std::path::Path::new(arg), token, proxy.clone())
                        {
                            Ok(v) => {
                                v.window.set_focus();
                                viewers.push(v);
                            }
                            Err(e) => eprintln!("brie: {arg}: {e}"),
                        }
                    }
                    "view-close" => viewers.retain(|v| v.token != arg),
                    "reveal" if known(arg) => store::reveal(std::path::Path::new(arg)),
                    "copy" if known(arg) => copy_to_clipboard(arg),
                    "refresh" => {
                        snap = snapshot();
                        paint(&tray, &snap);
                        push_data(&snap);
                    }
                    "style" => {
                        let next = if bar_style() == BarStyle::Pills {
                            BarStyle::Icon
                        } else {
                            BarStyle::Pills
                        };
                        let _ = set_bar_style(next);
                        paint(&tray, &snap);
                        push_data(&snap);
                    }
                    "store" => store::reveal(&store::root()),
                    "push" if snap.push.is_some() => {
                        std::thread::spawn(|| maint::push("brie test message", true));
                    }
                    "auto" => {
                        // The switch follows the real state: it stays put when the plist cannot be written.
                        let _ = set_autostart(!autostart_on());
                        push_data(&snap);
                    }
                    "hide" => window.set_visible(false),
                    "quit" => {
                        let _ = fs::remove_file(pid_file());
                        *flow = ControlFlow::Exit;
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_panel_asks_for_bries_window_and_keeps_a_way_to_the_terminal() {
        let panel = include_str!("panel.html");
        assert!(
            panel.contains(r#"post("view\t" + brief.dataset.path)"#),
            "a brief row opens brie's own window"
        );
        assert!(
            panel.contains(r#"post("open\t" + term.dataset.term)"#),
            "and a button still opens a Terminal"
        );
        assert!(
            panel.contains("data-term="),
            "the button exists on every row"
        );
    }

    #[test]
    fn one_quits_two_stays_and_other_keys_do_nothing() {
        assert_eq!(choice('1'), Some(Choice::Quit));
        assert_eq!(choice('2'), Some(Choice::Stay));
        assert_eq!(choice('\n'), Some(Choice::Stay));
        assert_eq!(choice('3'), None);
        assert_eq!(choice('x'), None);
    }

    #[test]
    fn short_name_drops_the_timestamp() {
        assert_eq!(
            short_name("08_10_2026_16:03_fix-login-redirect.md"),
            "fix-login-redirect"
        );
        assert_eq!(short_name("notes.md"), "notes");
        assert_eq!(short_name("a_b_c_d_e.md"), "a_b_c_d_e");
    }

    #[test]
    fn bar_title_is_the_working_count_in_icon_style_only() {
        let row = |status: &str| Row {
            status: status.into(),
            project: "p".into(),
            name: "n".into(),
            path: PathBuf::new(),
        };
        assert_eq!(bar_title(&[], BarStyle::Icon), None);
        assert_eq!(bar_title(&[row("blocked")], BarStyle::Icon), None);
        assert_eq!(
            bar_title(&[row("working"), row("working")], BarStyle::Icon),
            Some("2".into())
        );
        assert_eq!(bar_title(&[row("working")], BarStyle::Pills), None);
        assert_eq!(
            bar_counts(&[row("working"), row("blocked"), row("todo")]),
            (1, 1)
        );
    }

    #[test]
    fn bar_image_grows_by_one_pill_for_each_count() {
        let w = |s, a, b| bar_image(s, a, b, true).1;
        assert_eq!(w(BarStyle::Pills, 0, 0), 44);
        let one = w(BarStyle::Pills, 2, 0);
        assert!(one > 44);
        assert_eq!(w(BarStyle::Pills, 0, 3), one);
        assert!(w(BarStyle::Pills, 2, 3) > one);
        assert!(w(BarStyle::Pills, 12, 0) > one);
        assert_eq!(w(BarStyle::Icon, 5, 5), 44);
        let (rgba, width, height) = bar_image(BarStyle::Pills, 2, 0, true);
        assert_eq!(rgba.len() as u32, width * height * 4);
    }

    #[test]
    fn icon_dot_is_red_when_blocked_else_amber() {
        let at = |w, b| {
            let (rgba, width, _) = bar_image(BarStyle::Icon, w, b, true);
            let i = ((9 * width + 36) * 4) as usize;
            [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
        };
        assert_eq!(at(1, 1), [RED[0], RED[1], RED[2], 255]);
        assert_eq!(at(1, 0), [AMBER[0], AMBER[1], AMBER[2], 255]);
        assert_eq!(at(0, 0)[3], 0);
    }

    #[test]
    fn terminal_script_quotes_the_path_for_the_shell_and_applescript() {
        let script = terminal_script(std::path::Path::new("/s/it's \"x\"/a.md"));
        // The shell sees: cat '/s/it'\''s "x"/a.md'; AppleScript sees the backslashes and quotes escaped.
        assert!(script.contains(r#"do script "clear; cat '/s/it'\\''s \"x\"/a.md'""#));
        assert_eq!(script.matches("do script").count(), 3);
    }

    #[test]
    fn meta_label_counts_each_status() {
        let row = |s: &str| Row {
            status: s.into(),
            project: "p".into(),
            name: "n".into(),
            path: PathBuf::new(),
        };
        let (a, b, c) = (row("working"), row("todo"), row("todo"));
        assert_eq!(meta_label(&[&a, &b, &c]), "1 working · 2 todo");
        assert_eq!(group_dot(&[&b, &c]), "todo");
        assert_eq!(group_dot(&[&a, &b]), "working");
        assert_eq!(group_dot(&[&a, &row("blocked")]), "blocked");
    }

    #[test]
    fn panel_json_groups_by_project_and_adds_recent_done() {
        let row = |s: &str, p: &str, n: &str| Row {
            status: s.into(),
            project: p.into(),
            name: n.into(),
            path: PathBuf::from(format!("/s/{p}/{n}.md")),
        };
        let snap = Snapshot {
            open: vec![
                row("working", "harbor", "a"),
                row("todo", "harbor", "b"),
                row("todo", "zed", "c"),
            ],
            done: vec![row("done", "harbor", "d")],
            push: Some("ntfy".into()),
        };
        let j = crate::json::parse(&panel_json(&snap, true, true)).unwrap();
        assert_eq!(j.get("total").and_then(crate::json::Json::num), Some(3.0));
        assert_eq!(j.get("push").and_then(crate::json::Json::str), Some("ntfy"));
        let groups = j.get("groups").and_then(crate::json::Json::arr).unwrap();
        let names: Vec<&str> = groups
            .iter()
            .filter_map(|g| g.get("name").and_then(crate::json::Json::str))
            .collect();
        assert_eq!(names, ["harbor", "zed", "Recently done"]);
        assert_eq!(
            groups[0].get("meta").and_then(crate::json::Json::str),
            Some("1 working · 1 todo")
        );
    }

    #[test]
    fn summary_counts_and_names_blocked_only_when_there_are_some() {
        let row = |s: &str| Row {
            status: s.into(),
            project: "p".into(),
            name: "n".into(),
            path: PathBuf::new(),
        };
        let v = env!("CARGO_PKG_VERSION");
        assert_eq!(summary(&[]), format!("brie v{v} (0 working, 0 todo)"));
        let rows = [row("working"), row("working"), row("todo"), row("blocked")];
        assert_eq!(
            summary(&rows),
            format!("brie v{v} (2 working, 1 todo, 1 blocked)")
        );
    }
}
