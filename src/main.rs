mod agent;
mod contract;
mod daemon;
mod editor;
mod html;
mod json;
mod logo;
mod maint;
mod mcp;
mod md;
mod memory;
mod memwin;
mod run;
mod serve;
mod store;
mod theme;
mod tui;
mod view;

use std::path::PathBuf;
use std::process::ExitCode;
use store::{Kind, Project, Res};

const HELP: &str = "brie: keep project briefs and prototypes outside the repo

usage:
  brie                          open the TUI
  brie add [path] [--name n] [--no-inject] [--no-allow] [--memory|--no-memory]
                                onboard a repo (default: current dir), write the agent
                                instructions into its CLAUDE.md / AGENTS.md, and allow the
                                store folder in .claude/settings.local.json. The project memory
                                is off unless you pass --memory (--no-memory turns it off again)
  brie ls                       list projects
  brie new <name> [-p project]  create a brief, print its path
  brie status [brief] [--all] [--status s] [--limit n] [--json] [--if-changed hash]
                                with no brief: counts by status, then the briefs not done (newest
                                first, 20 at most). --all adds done ones. One brief: its status
  brie set <brief> <status>     set the Status line: todo, working, done or blocked
  brie report <brief> [text]    append to the bottom of a brief, under one ## Report heading;
                                text from stdin if missing
  brie wait <brief> [-t secs] [-p project]
                                block until Status is done (0), blocked (2) or timeout (1),
                                then run `brie memory sync`
  brie wait <brief>... [--any] [--notify] [-t secs]
                                wait on several briefs: all done by default, --any for the first
  brie rm <brief> [-p project]  move a brief and its Scratch directory to the Trash
  brie link [worktree]          point a worktree's .briefs and .prototype at the store
  brie template [init]          show this project's brief template, or write it for editing
  brie doctor                   check links, instruction blocks and the store
  brie backup                   commit briefs, templates and the index to a git repo in the store
  brie read <brief> [--section a,b] [--head n] [--tail n]
                                part of a brief: headings such as Report or Do, or `top`
  brie contract [worker|orchestrator|short]
                                the brief contract, whole or for one role, plus the project's rules.md
  brie path [briefs|prototypes] [-p project]
  brie serve <dir> [--cmd '...' [--no-port]] [--vite|--static]
                                run a prototype. Order: --cmd, a brie.run file (asks first), Vite when
                                there is a vite.config.*, else the built-in static server (Markdown,
                                SPA fallback, byte ranges). The command gets $PORT, $BRIE_DIR, $BRIE_REPO
  brie serve start <dir> ...    the same, detached, so it outlives this shell
  brie serve ls [--json]        list the servers started by brie serve
  brie serve logs <port|pid:N> [-f]
                                show the output of a server
  brie serve stop <port|pid:N|all>
                                stop servers and everything they started
  brie serve approve <dir>      approve the brie.run in a directory (needs a terminal)
  brie view <path>              draw a file in the whole terminal: Markdown rendered, read only
  brie open <path>              open a file with the system viewer (PDF, image, design, ...)
  brie mcp [--worker]           the brief commands as MCP tools over stdio (--worker: status, read,
                                set status, report, scratch, contract only)
  brie notify set <target>|off|show|test
                                push target for done/blocked: ntfy:<topic> or url:<https webhook>
  brie daemon [start|stop|status]
                                macOS menu bar item: the icon shows the working count; a click opens a panel
                                with search, a row for each project that expands to its open briefs
                                (click to reveal, copy path), Recently done, Refresh, Open store,
                                Send test push, Start at login (launchd) and Quit.
                                With no subcommand in a terminal: starts it in the background, then
                                asks: 1 quit it, 2 keep it in the background
  brie sync [--remote url]      back up, then pull and push a git remote
  brie memory add <kind> [text] log a message (user, agent, tool, echo, work, note); text from stdin if missing.
                                While a turn runs it is queued and the turn files it
  brie memory sync              log what the briefs gained, then build the summary tree
  brie memory build             build the summary tree and merge the views down (64 KB and 16 KB)
  brie memory view              the view: summary lines that cover the whole log (128 KB at most)
  brie memory find <words>      where the words appear: the messages, and the lines over them as id+n
                                (--limit n, --log, --lines, --exact, --full)
  brie memory zoom <id> <n>     open line id+n into its two lines; n = 1 gives message id whole
  brie memory zoom <Name>       a subagent's whole chat
  brie memory since <id>        the messages after id, whole
  brie memory turn <text>       one turn: view + message to $BRIE_AGENT, everything it does is logged
  brie memory sub <Name> <task> start a subagent in the background; its report comes back as a work message
  brie memory date <id> | status

store: $BRIE_HOME or ~/.alter-store
project: -p, else the repo that holds the current dir";

fn main() -> ExitCode {
    match run(std::env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("brie: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Remove a flag and its value from `args`.
fn take_flag(args: &mut Vec<String>, long: &str, short: Option<&str>) -> Option<String> {
    let i = args
        .iter()
        .position(|a| a == long || Some(a.as_str()) == short)?;
    if i + 1 >= args.len() {
        return None;
    }
    let value = args.remove(i + 1);
    args.remove(i);
    Some(value)
}

/// The lines of the help that belong to one command: its usage line and the indented lines under it.
fn help_for(topic: &str) -> String {
    let mut out: Vec<&str> = vec![];
    let mut on = false;
    for line in HELP.lines() {
        if let Some(rest) = line.strip_prefix("  brie ") {
            on = rest.split_whitespace().next() == Some(topic);
        } else if !line.starts_with("  ") {
            on = false;
        }
        if on {
            out.push(line);
        }
    }
    if out.is_empty() {
        format!("no help for {topic}; `brie help` lists every command")
    } else {
        out.join("\n")
    }
}

fn take_switch(args: &mut Vec<String>, name: &str) -> bool {
    let before = args.len();
    args.retain(|a| a != name);
    args.len() != before
}

fn project_for(flag: Option<String>) -> Res<Project> {
    match flag {
        Some(name) => store::find(&name).ok_or_else(|| format!("no project named {name}").into()),
        None => {
            store::from_cwd().ok_or_else(|| "not in an onboarded repo; use -p or `brie add`".into())
        }
    }
}

fn run(mut args: Vec<String>) -> Res<()> {
    if args.is_empty() {
        return tui::start();
    }
    let cmd = args.remove(0);
    let project_flag = take_flag(&mut args, "--project", Some("-p"));
    match cmd.as_str() {
        "add" => {
            let name = take_flag(&mut args, "--name", None);
            let inject = !take_switch(&mut args, "--no-inject");
            let allow = !take_switch(&mut args, "--no-allow");
            let memory_on = take_switch(&mut args, "--memory");
            let memory_off = take_switch(&mut args, "--no-memory");
            if memory_on && memory_off {
                return Err("give --memory or --no-memory, not both".into());
            }
            let path = args
                .first()
                .map(|s| store::expand(s))
                .unwrap_or_else(|| PathBuf::from("."));
            let p = store::add(&path, name.as_deref())?;
            println!("{} -> {}", p.name, store::root().join(&p.name).display());
            if memory_on || memory_off {
                store::set_memory(&p.name, memory_on)?;
            }
            let memory = store::memory_enabled(&p.name);
            println!("memory: {}", if memory { "on" } else { "off" });
            if inject {
                for file in contract::inject(&p.repo, memory)? {
                    println!("instructions written to {}", file.display());
                }
            }
            if allow {
                let dir = store::root().join(&p.name);
                match contract::allow_store(&p.repo, &dir) {
                    Ok(Some(file)) => println!("store allowed in {}", file.display()),
                    Ok(None) => {}
                    Err(e) => eprintln!("permission not set: {e}"),
                }
            }
        }
        "ls" => {
            for p in store::projects() {
                println!("{}\t{}", p.name, p.repo.display());
            }
        }
        "new" => {
            if args.is_empty() {
                return Err("usage: brie new <name>".into());
            }
            let p = project_for(project_flag)?;
            println!("{}", store::new_brief(&p, &args.join(" "))?.display());
            if let Some(w) = store::template_warning(&p) {
                eprintln!("warning: {w}");
            }
        }
        "read" => {
            let section = take_flag(&mut args, "--section", None);
            let head = take_flag(&mut args, "--head", None)
                .map(|n| n.parse::<usize>())
                .transpose()
                .map_err(|_| "head is a number")?;
            let tail = take_flag(&mut args, "--tail", None)
                .map(|n| n.parse::<usize>())
                .transpose()
                .map_err(|_| "tail is a number")?;
            let arg = args
                .first()
                .ok_or("usage: brie read <brief> [--section a,b] [--head n] [--tail n]")?;
            let path = store::resolve_brief(&project_for(project_flag)?, arg)?;
            println!(
                "{}",
                store::read_part(&path, section.as_deref(), head, tail)?
            );
        }
        "contract" => {
            let role = args.first().map(String::as_str).unwrap_or("full");
            print!("{}", contract::view(role));
            if role == "short" {
                return Ok(());
            }
            if let Ok(p) = project_for(project_flag)
                && let Ok(rules) = std::fs::read_to_string(store::rules_path(&p.name))
            {
                print!("\nPROJECT RULES ({})\n\n{}", p.name, rules.trim_end());
                println!();
            }
        }
        "status" => {
            let p = project_for(project_flag)?;
            let json = take_switch(&mut args, "--json");
            let all = take_switch(&mut args, "--all");
            let filter = take_flag(&mut args, "--status", None);
            let changed = take_flag(&mut args, "--if-changed", None);
            let limit = match take_flag(&mut args, "--limit", None) {
                Some(n) => n.parse::<usize>().map_err(|_| "limit is a number")?,
                None => 20,
            };
            if let Some(arg) = args.first() {
                let path = store::resolve_brief(&p, arg)?;
                println!("{}", store::status_of(&path));
            } else if json {
                println!(
                    "{}",
                    store::summary_json(&p, filter.as_deref(), all, limit, changed.as_deref())
                );
            } else {
                println!(
                    "{}",
                    store::summary_text(&p, filter.as_deref(), all, limit, changed.as_deref())
                );
            }
        }
        "set" => {
            let (brief, status) = match args.as_slice() {
                [b, s] => (b, s),
                _ => return Err("usage: brie set <brief> <todo|working|done|blocked>".into()),
            };
            let path = store::resolve_brief(&project_for(project_flag)?, brief)?;
            let warning = store::set_status(&path, status)?;
            println!("Status: {}", store::status_of(&path));
            if let Some(w) = warning {
                eprintln!("warning: {w}");
            }
        }
        "report" => {
            let brief = args.first().ok_or("usage: brie report <brief> [text]")?;
            let path = store::resolve_brief(&project_for(project_flag)?, brief)?;
            let text = if args.len() > 1 {
                args[1..].join(" ")
            } else {
                let mut t = String::new();
                std::io::Read::read_to_string(&mut std::io::stdin(), &mut t)?;
                t
            };
            let warning = store::append_report(&path, &text)?;
            println!("appended");
            if let Some(w) = warning {
                eprintln!("warning: {w}");
            }
        }
        "wait" => {
            let any = take_switch(&mut args, "--any");
            let notify = take_switch(&mut args, "--notify");
            let timeout = take_flag(&mut args, "--timeout", Some("-t"))
                .map(|v| v.parse::<u64>())
                .transpose()
                .map_err(|_| "timeout is a number of seconds")?;
            if args.is_empty() {
                return Err("usage: brie wait <brief>...".into());
            }
            let p = project_for(project_flag)?;
            let paths: Vec<PathBuf> = args
                .iter()
                .map(|a| store::resolve_brief(&p, a))
                .collect::<Res<_>>()?;
            let names: Vec<String> = paths
                .iter()
                .map(|x| {
                    x.file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned()
                })
                .collect();
            let start = std::time::Instant::now();
            let mut last = vec![String::new(); paths.len()];
            let ended = |s: &String| s == "done" || s == "blocked";
            loop {
                let now: Vec<String> = paths.iter().map(|x| store::status_of(x)).collect();
                for i in 0..now.len() {
                    if now[i] != last[i] {
                        if paths.len() > 1 {
                            println!("{}: {}", names[i], now[i]);
                        } else {
                            println!("{}", now[i]);
                        }
                    }
                }
                last = now.clone();
                let finished = if any {
                    now.iter().any(ended)
                } else {
                    now.iter().all(ended)
                };
                if finished {
                    let blocked = now.iter().any(|s| s == "blocked");
                    for w in paths.iter().filter_map(|x| store::drift(x)) {
                        eprintln!("warning: {w}");
                    }
                    if let Some(line) = memory::sync_quiet(&p) {
                        eprintln!("{line}");
                    }
                    if notify {
                        maint::notify(&format!(
                            "{}: {}",
                            names.join(", "),
                            if blocked { "blocked" } else { "done" }
                        ));
                    }
                    if blocked {
                        std::process::exit(2);
                    }
                    return Ok(());
                }
                if timeout.is_some_and(|t| start.elapsed().as_secs() >= t) {
                    if let Some(line) = memory::sync_quiet(&p) {
                        eprintln!("{line}");
                    }
                    if notify {
                        maint::notify("wait timed out");
                    }
                    std::process::exit(1);
                }
                std::thread::sleep(std::time::Duration::from_secs(2));
            }
        }
        "rm" => {
            let arg = args.first().ok_or("usage: brie rm <brief>")?;
            let p = project_for(project_flag)?;
            let path = store::resolve_brief(&p, arg)?;
            store::remove_brief(&p, &path)?;
            println!(
                "moved to the Trash: {}",
                path.file_name().unwrap_or_default().to_string_lossy()
            );
        }
        "link" => {
            let p = project_for(project_flag)?;
            let tree = args
                .first()
                .map(|s| store::expand(s))
                .unwrap_or_else(|| PathBuf::from("."));
            let done = maint::link(&tree, &p)?;
            if done.is_empty() {
                println!("already linked");
            }
            for line in done {
                println!("{line}");
            }
        }
        "template" => {
            let p = project_for(project_flag)?;
            match args.first().map(String::as_str) {
                Some("init") => {
                    let path = store::template_path(&p.name);
                    if path.exists() {
                        return Err(format!("{} already exists", path.display()).into());
                    }
                    std::fs::write(&path, contract::DEFAULT_TEMPLATE)?;
                    println!("{}", path.display());
                }
                None => print!("{}", store::template_text(&p)),
                Some(other) => return Err(format!("unknown template command {other}").into()),
            }
        }
        "doctor" => {
            if maint::doctor() > 0 {
                std::process::exit(1);
            }
        }
        "backup" => println!("{}", maint::backup()?),
        "path" => {
            let p = project_for(project_flag)?;
            let kind = match args.first().map(String::as_str) {
                None | Some("briefs") => Kind::Briefs,
                Some("prototypes") => Kind::Prototypes,
                Some(other) => return Err(format!("unknown folder {other}").into()),
            };
            println!("{}", store::kind_dir(&p.name, kind).display());
        }
        "serve" => match args.first().map(String::as_str) {
            Some("ls") => {
                if take_switch(&mut args, "--json") {
                    println!("{}", run::running_json());
                    return Ok(());
                }
                let servers = run::running();
                if servers.is_empty() {
                    println!("no servers started by brie");
                }
                for e in servers {
                    println!("{}", run::describe(&e));
                }
            }
            Some("stop") => {
                let target = args
                    .get(1)
                    .ok_or("usage: brie serve stop <port|pid:N|all>")?;
                let target = run::Target::parse(target).ok_or("give a port, pid:N or all")?;
                println!("stopped {}", run::stop(target));
            }
            Some("logs") => {
                let follow = take_switch(&mut args, "-f");
                let target = args
                    .get(1)
                    .ok_or("usage: brie serve logs <port|pid:N> [-f]")?;
                let target = run::Target::parse(target).ok_or("give a port or pid:N")?;
                let log = run::log_of(&target).ok_or("no running server with a log matches")?;
                run::show_log(&log, follow)?;
            }
            Some("approve") => {
                let dir = args.get(1).ok_or("usage: brie serve approve <dir>")?;
                let m = run::manifest(&std::fs::canonicalize(store::expand(dir))?)
                    .ok_or("that directory has no brie.run")?;
                if run::is_approved(&m) {
                    println!("already approved");
                } else if run::confirm(&format!("run `{}` from {}?", m.cmd, m.path.display())) {
                    run::approve(&m)?;
                    println!("approved");
                } else {
                    return Err("not approved (approval needs a terminal and a y)".into());
                }
            }
            first => {
                let detach = first == Some("start");
                if detach {
                    args.remove(0);
                }
                let cmd = take_flag(&mut args, "--cmd", None);
                let no_port = take_switch(&mut args, "--no-port");
                let vite = take_switch(&mut args, "--vite");
                let stat = take_switch(&mut args, "--static");
                let mode = if let Some(cmd) = cmd.clone() {
                    run::Mode::Cmd {
                        cmd,
                        port: !no_port,
                    }
                } else if vite {
                    run::Mode::Vite
                } else if stat {
                    run::Mode::Static
                } else {
                    run::Mode::Auto
                };
                let dir = args.first().ok_or("usage: brie serve <dir>")?.clone();
                if detach {
                    let mut again = vec![dir];
                    if let Some(c) = cmd {
                        again.extend(["--cmd".into(), c]);
                    }
                    for (on, flag) in [(no_port, "--no-port"), (vite, "--vite"), (stat, "--static")]
                    {
                        if on {
                            again.push(flag.into());
                        }
                    }
                    if let Some(p) = &project_flag {
                        again.extend(["-p".into(), p.clone()]);
                    }
                    let e = run::start_detached(&again)?;
                    let url = if e.port == 0 {
                        "-".to_string()
                    } else {
                        format!("http://localhost:{}", e.port)
                    };
                    println!("{url}\t{}\tpid {}", e.kind, e.pid);
                    return Ok(());
                }
                let dir = std::fs::canonicalize(store::expand(&dir))?;
                let repo = project_for(project_flag).ok().map(|p| p.repo);
                let opts = run::Opts {
                    own_group: false,
                    wait_ms: 30_000,
                };
                let server = loop {
                    match run::launch(&dir, repo.as_deref(), mode.clone(), opts)? {
                        run::Launch::Up(server) => break server,
                        run::Launch::NeedsApproval(m) => {
                            if !run::confirm(&format!(
                                "{} runs `{}`. Run it?",
                                m.path.display(),
                                m.cmd
                            )) {
                                return Err(format!(
                                    "{} is not approved. Approve it in a terminal: brie serve approve {}",
                                    m.path.display(),
                                    dir.display()
                                )
                                .into());
                            }
                            run::approve(&m)?;
                        }
                    }
                };
                run::register(&server, &dir);
                let url = if server.port == 0 {
                    "-".to_string()
                } else {
                    format!("http://localhost:{}", server.port)
                };
                println!(
                    "{url}\t{}{}",
                    server.kind,
                    if server.starting {
                        "\tstarting: the port is not open yet"
                    } else {
                        ""
                    }
                );
                if let Some(log) = &server.log {
                    println!("log: {}", log.display());
                }
                loop {
                    std::thread::park();
                }
            }
        },
        "view" => {
            let arg = args.first().ok_or("usage: brie view <path>")?;
            view::run(&store::expand(arg))?;
        }
        "open" => {
            let arg = args.first().ok_or("usage: brie open <path>")?;
            let path = std::fs::canonicalize(store::expand(arg))?;
            let status = std::process::Command::new(store::opener())
                .arg(&path)
                .status()?;
            if !status.success() {
                return Err(format!("{} failed", store::opener()).into());
            }
        }
        "mcp" => {
            let role = if take_switch(&mut args, "--worker") {
                mcp::Role::Worker
            } else {
                mcp::Role::All
            };
            return mcp::serve(role);
        }
        "notify" => match args.first().map(String::as_str) {
            Some("set") => {
                let target = args.get(1).ok_or("usage: brie notify set <target>")?;
                maint::set_notify(target)?;
                println!("push target set");
            }
            Some("off") => {
                maint::clear_notify();
                println!("push target removed");
            }
            Some("test") => {
                if maint::notify_target().is_none() {
                    return Err("no push target: brie notify set <target>".into());
                }
                maint::push("brie test message", true);
                println!("sent");
            }
            Some("show") | None => match maint::notify_target() {
                Some(t) => println!("{t}"),
                None => println!("no push target"),
            },
            Some(other) => return Err(format!("unknown notify command {other}").into()),
        },
        "memory" => {
            let p = project_for(project_flag)?;
            let out = memory::run(&p, &args)?;
            if !out.is_empty() {
                println!("{}", out.trim_end());
            }
        }
        "sync" => {
            let remote = take_flag(&mut args, "--remote", None);
            println!("{}", maint::sync(remote.as_deref())?);
        }
        "daemon" => {
            let out = daemon::run(&args)?;
            if !out.is_empty() {
                println!("{out}");
            }
        }
        "version" | "-V" | "--version" => println!("brie {}", env!("CARGO_PKG_VERSION")),
        "help" | "-h" | "--help" => match args.first() {
            Some(topic) => println!("{}", help_for(topic)),
            None => println!("{HELP}"),
        },
        other => {
            return Err(format!(
                "unknown command {other}. Try `brie help` or `brie help <command>`"
            )
            .into());
        }
    }
    Ok(())
}
