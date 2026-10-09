//! Running a prototype: the built-in static server, Vite, or any command from `--cmd` or a `brie.run`
//! manifest. A manifest runs code an agent may have written, so it needs a person's approval first.

use std::fs::{self, File};
use std::io::{BufRead, IsTerminal, Read, Seek, SeekFrom, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::serve;
use crate::store::{self, Res};

#[derive(Clone, PartialEq)]
pub enum Mode {
    Auto,
    Static,
    Vite,
    Cmd { cmd: String, port: bool },
}

/// How to start: whether the child gets a process group of its own (so the whole tree can be
/// stopped), and how long to wait for its port.
#[derive(Clone, Copy)]
pub struct Opts {
    pub own_group: bool,
    pub wait_ms: u64,
}

pub struct Server {
    pub port: u16,
    pub kind: &'static str,
    pub child: Option<Child>,
    pub log: Option<PathBuf>,
    /// The process is alive but did not open its port in time.
    pub starting: bool,
    group: bool,
}

impl Drop for Server {
    fn drop(&mut self) {
        let Some(child) = &mut self.child else { return };
        if self.group {
            let _ = Command::new("kill")
                .args(["-TERM", "--", &format!("-{}", child.id())])
                .stderr(Stdio::null())
                .status();
        } else {
            let _ = child.kill();
        }
        let _ = child.wait();
    }
}

pub enum Launch {
    Up(Server),
    NeedsApproval(Manifest),
}

// ---- manifest and approval -------------------------------------------------------------------

#[derive(Clone)]
pub struct Manifest {
    pub path: PathBuf,
    pub cmd: String,
    pub port: bool,
}

/// `brie.run`: `run: <command>` and optionally `port: no`. A single plain line is the command.
/// `$PORT`, `{port}`, `$BRIE_DIR` and `$BRIE_REPO` are available to the command.
pub fn manifest(dir: &Path) -> Option<Manifest> {
    let path = dir.join("brie.run");
    let text = fs::read_to_string(&path).ok()?;
    let (mut cmd, mut port) = (None, true);
    for line in text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
    {
        if let Some(v) = line.strip_prefix("run:") {
            cmd = Some(v.trim().to_string());
        } else if let Some(v) = line.strip_prefix("port:") {
            port = !matches!(v.trim(), "no" | "false" | "0");
        } else if cmd.is_none() {
            cmd = Some(line.to_string());
        }
    }
    cmd.filter(|c| !c.is_empty())
        .map(|cmd| Manifest { path, cmd, port })
}

fn digest(m: &Manifest) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    let path = fs::canonicalize(&m.path).unwrap_or_else(|_| m.path.clone());
    let body = fs::read(&m.path).unwrap_or_default();
    for byte in path
        .as_os_str()
        .as_encoded_bytes()
        .iter()
        .chain(&[0])
        .chain(&body)
    {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn approved_path() -> PathBuf {
    store::root().join("approved")
}

/// True when this exact file, at this exact path, was approved by a person.
pub fn is_approved(m: &Manifest) -> bool {
    let want = digest(m);
    fs::read_to_string(approved_path())
        .unwrap_or_default()
        .lines()
        .any(|l| l == want)
}

pub fn approve(m: &Manifest) -> Res<()> {
    if is_approved(m) {
        return Ok(());
    }
    fs::create_dir_all(store::root())?;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(approved_path())?;
    writeln!(file, "{}", digest(m))?;
    Ok(())
}

/// Ask on the terminal. With no terminal (an agent, a pipe) the answer is no.
pub fn confirm(question: &str) -> bool {
    if !std::io::stdin().is_terminal() {
        return false;
    }
    eprint!("{question} [y/N] ");
    let _ = std::io::stderr().flush();
    let mut answer = String::new();
    std::io::stdin().lock().read_line(&mut answer).is_ok()
        && answer.trim().eq_ignore_ascii_case("y")
}

// ---- launching -----------------------------------------------------------------------------

fn has_vite_config(dir: &Path) -> bool {
    ["ts", "js", "mts", "mjs", "cjs"]
        .iter()
        .any(|ext| dir.join(format!("vite.config.{ext}")).is_file())
}

/// Serve `dir`. `Auto` picks a `brie.run` (once approved), then Vite when there is a `vite.config.*`,
/// then the static server. A Vite that will not start falls back to static.
pub fn launch(dir: &Path, repo: Option<&Path>, mode: Mode, opts: Opts) -> std::io::Result<Launch> {
    match mode {
        Mode::Cmd { cmd, port } => launch_cmd(dir, repo, &cmd, port, "cmd", opts).map(Launch::Up),
        Mode::Static => Ok(Launch::Up(static_server(dir)?)),
        Mode::Vite => launch_vite(dir, repo, opts).map(Launch::Up),
        Mode::Auto => {
            if let Some(m) = manifest(dir) {
                if !is_approved(&m) {
                    return Ok(Launch::NeedsApproval(m));
                }
                return launch_cmd(dir, repo, &m.cmd, m.port, "cmd", opts).map(Launch::Up);
            }
            if has_vite_config(dir)
                && let Ok(server) = launch_vite(dir, repo, opts)
            {
                return Ok(Launch::Up(server));
            }
            Ok(Launch::Up(static_server(dir)?))
        }
    }
}

fn static_server(dir: &Path) -> std::io::Result<Server> {
    let port = serve::start(dir.to_path_buf())?;
    Ok(Server {
        port,
        kind: "static",
        child: None,
        log: None,
        starting: false,
        group: false,
    })
}

fn free_port() -> std::io::Result<u16> {
    (4100..4200)
        .find(|&p| TcpListener::bind(("127.0.0.1", p)).is_ok())
        .ok_or_else(|| std::io::Error::other("no free port"))
}

fn log_file(dir: &Path) -> std::io::Result<(File, PathBuf)> {
    let name: String = dir
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let logs = store::root().join("logs");
    fs::create_dir_all(&logs)?;
    let path = logs.join(format!("{name}.log"));
    Ok((File::create(&path)?, path))
}

pub fn tail(path: &Path, lines: usize) -> String {
    let text = fs::read_to_string(path).unwrap_or_default();
    let all: Vec<&str> = text.lines().collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}

/// Wait for `port`. Err when the child exits first; `Ok(false)` when it lives but the wait ran out.
fn wait_ready(child: &mut Child, port: u16, wait_ms: u64, log: &Path) -> std::io::Result<bool> {
    let deadline = Instant::now() + Duration::from_millis(wait_ms);
    loop {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return Ok(true);
        }
        if let Ok(Some(status)) = child.try_wait() {
            return Err(std::io::Error::other(format!(
                "exited with {status}\n{}",
                tail(log, 12)
            )));
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn launch_cmd(
    dir: &Path,
    repo: Option<&Path>,
    cmd: &str,
    has_port: bool,
    kind: &'static str,
    opts: Opts,
) -> std::io::Result<Server> {
    let port = if has_port { free_port()? } else { 0 };
    let (log, log_path) = log_file(dir)?;
    let mut command = Command::new("sh");
    command
        .arg("-c")
        .arg(cmd.replace("{port}", &port.to_string()))
        .current_dir(dir)
        .env("PORT", port.to_string())
        .env("BRIE_DIR", dir)
        .env("BRIE_REPO", repo.unwrap_or(dir))
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    if opts.own_group {
        command.process_group(0);
    }
    let mut child = command.spawn()?;
    let starting = if has_port {
        !wait_ready(&mut child, port, opts.wait_ms, &log_path)?
    } else {
        thread::sleep(Duration::from_millis(400));
        if let Ok(Some(status)) = child.try_wait()
            && !status.success()
        {
            return Err(std::io::Error::other(format!(
                "exited with {status}\n{}",
                tail(&log_path, 12)
            )));
        }
        false
    };
    Ok(Server {
        port,
        kind,
        child: Some(child),
        log: Some(log_path),
        starting,
        group: opts.own_group,
    })
}

fn launch_vite(dir: &Path, repo: Option<&Path>, opts: Opts) -> std::io::Result<Server> {
    let port = free_port()?;
    let (log, log_path) = log_file(dir)?;
    let mut command = Command::new("npx");
    command
        .args(["--no-install", "vite"])
        .arg(dir)
        .args([
            "--port",
            &port.to_string(),
            "--strictPort",
            "--host",
            "127.0.0.1",
        ])
        .current_dir(repo.unwrap_or(dir))
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    if opts.own_group {
        command.process_group(0);
    }
    let mut child = command.spawn()?;
    match wait_ready(&mut child, port, opts.wait_ms.max(6000), &log_path) {
        Ok(true) => Ok(Server {
            port,
            kind: "vite",
            child: Some(child),
            log: Some(log_path),
            starting: false,
            group: opts.own_group,
        }),
        other => {
            let reason = match other {
                Err(e) => e.to_string(),
                _ => "did not open its port in time".to_string(),
            };
            let server = Server {
                port,
                kind: "vite",
                child: Some(child),
                log: None,
                starting: false,
                group: opts.own_group,
            };
            drop(server);
            Err(std::io::Error::other(format!(
                "vite did not start (is it installed in the repo? npx --no-install vite): {reason}"
            )))
        }
    }
}

// ---- registry: servers started by `brie serve`, so `ls`, `logs` and `stop` can find them ------

#[derive(Clone)]
pub struct Entry {
    pub pid: u32,
    pub port: u16,
    pub kind: String,
    pub child: u32,
    pub log: String,
    pub dir: String,
}

fn registry_path() -> PathBuf {
    store::root().join("servers")
}

fn alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn read_entries() -> Vec<Entry> {
    fs::read_to_string(registry_path())
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let mut f = l.splitn(6, '\t');
            Some(Entry {
                pid: f.next()?.parse().ok()?,
                port: f.next()?.parse().ok()?,
                kind: f.next()?.to_string(),
                child: f.next()?.parse().ok()?,
                log: f.next()?.to_string(),
                dir: f.next()?.to_string(),
            })
        })
        .collect()
}

fn write_entries(entries: &[Entry]) {
    let text: String = entries
        .iter()
        .map(|e| {
            format!(
                "{}\t{}\t{}\t{}\t{}\t{}\n",
                e.pid, e.port, e.kind, e.child, e.log, e.dir
            )
        })
        .collect();
    let _ = fs::write(registry_path(), text);
}

pub fn register(server: &Server, dir: &Path) {
    let mut entries: Vec<Entry> = read_entries()
        .into_iter()
        .filter(|e| alive(e.pid))
        .collect();
    entries.push(Entry {
        pid: std::process::id(),
        port: server.port,
        kind: server.kind.to_string(),
        child: server.child.as_ref().map_or(0, Child::id),
        log: server
            .log
            .as_ref()
            .map_or("-".into(), |p| p.display().to_string()),
        dir: dir.display().to_string(),
    });
    write_entries(&entries);
}

/// The registered servers that still run. Dead ones are dropped from the file.
pub fn running() -> Vec<Entry> {
    let live: Vec<Entry> = read_entries()
        .into_iter()
        .filter(|e| alive(e.pid))
        .collect();
    write_entries(&live);
    live
}

pub enum Target {
    All,
    Port(u16),
    Pid(u32),
}

impl Target {
    pub fn parse(s: &str) -> Option<Target> {
        if s == "all" {
            Some(Target::All)
        } else if let Some(pid) = s.strip_prefix("pid:") {
            pid.parse().ok().map(Target::Pid)
        } else {
            s.parse().ok().map(Target::Port)
        }
    }

    fn matches(&self, e: &Entry) -> bool {
        match self {
            Target::All => true,
            Target::Port(p) => e.port == *p,
            Target::Pid(p) => e.pid == *p,
        }
    }
}

/// Stop the matching servers, their process groups included. Returns how many.
pub fn stop(target: Target) -> usize {
    let mut stopped = 0;
    let mut keep = vec![];
    for e in running() {
        if target.matches(&e) {
            for t in [
                format!("-{}", e.pid),
                e.pid.to_string(),
                format!("-{}", e.child),
                e.child.to_string(),
            ] {
                if t != "0" && t != "-0" {
                    let _ = Command::new("kill")
                        .args(["-TERM", "--", &t])
                        .stderr(Stdio::null())
                        .status();
                }
            }
            stopped += 1;
        } else {
            keep.push(e);
        }
    }
    write_entries(&keep);
    stopped
}

pub fn log_of(target: &Target) -> Option<PathBuf> {
    running()
        .into_iter()
        .find(|e| target.matches(e) && e.log != "-")
        .map(|e| PathBuf::from(e.log))
}

/// Print the end of a log; with `follow`, keep printing what is added.
pub fn show_log(path: &Path, follow: bool) -> Res<()> {
    println!("{}", tail(path, 80));
    if !follow {
        return Ok(());
    }
    let mut file = File::open(path)?;
    let mut at = file.seek(SeekFrom::End(0))?;
    loop {
        thread::sleep(Duration::from_millis(500));
        let len = fs::metadata(path)?.len();
        if len < at {
            at = 0;
        }
        if len > at {
            file.seek(SeekFrom::Start(at))?;
            let mut chunk = String::new();
            file.read_to_string(&mut chunk)?;
            print!("{chunk}");
            let _ = std::io::stdout().flush();
            at = len;
        }
    }
}

/// Start `brie serve <args>` as a detached process of its own, and wait for it to register.
pub fn start_detached(args: &[String]) -> Res<Entry> {
    let mut child = Command::new(std::env::current_exe()?)
        .arg("serve")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()?;
    for _ in 0..300 {
        if let Some(e) = read_entries().into_iter().find(|e| e.pid == child.id()) {
            return Ok(e);
        }
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!(
                "the server exited ({status}); run it in the foreground to see why"
            )
            .into());
        }
        thread::sleep(Duration::from_millis(100));
    }
    Err("the server did not start in time".into())
}

/// The registered servers as a JSON array, for agents.
pub fn running_json() -> String {
    let items: Vec<String> = running()
        .iter()
        .map(|e| {
            format!(
                "{{\"port\":{},\"kind\":{},\"pid\":{},\"dir\":{},\"log\":{}}}",
                e.port,
                crate::json::quote(&e.kind),
                e.pid,
                crate::json::quote(&e.dir),
                crate::json::quote(&e.log)
            )
        })
        .collect();
    format!("[{}]", items.join(","))
}

/// A `brie.run` in `dir` that nobody approved yet.
pub fn needs_approval(dir: &Path) -> Option<Manifest> {
    manifest(dir).filter(|m| !is_approved(m))
}

/// One line for each server, for people.
pub fn describe(e: &Entry) -> String {
    let url = if e.port == 0 {
        "-".to_string()
    } else {
        format!("http://localhost:{}", e.port)
    };
    format!("{url}\t{}\tpid {}\t{}", e.kind, e.pid, e.dir)
}
