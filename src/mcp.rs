//! `brie mcp`: the brief commands as MCP tools over stdio, for agents that prefer tools to a shell.
//! One JSON message for each line. Nothing but protocol messages goes to stdout.

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::json::{self, Json};
use crate::store::{self, Kind, Project};
use crate::{contract, memory, run, store::Res};

#[derive(Clone, Copy, PartialEq)]
pub enum Role {
    All,
    Worker,
}

struct Tool {
    name: &'static str,
    worker: bool,
    desc: &'static str,
    /// Each property is (name, type, description). `array` means an array of strings.
    props: &'static [(&'static str, &'static str, &'static str)],
    /// With `--worker`, the tool shows only these properties. The rest still work if a client sends them.
    worker_props: Option<&'static [&'static str]>,
    required: &'static [&'static str],
}

const TOOLS: &[Tool] = &[
    Tool {
        name: "brief_status",
        worker: true,
        desc: "One brief's status, or with no brief: counts, then the briefs not done. Short.",
        props: &[
            ("brief", "string", ""),
            ("status", "string", "only this status"),
            ("all", "boolean", "include done"),
            ("limit", "number", "default 20"),
            (
                "if_changed",
                "string",
                "the state hash from last time; returns unchanged if same",
            ),
            ("project", "string", ""),
        ],
        worker_props: Some(&["brief"]),
        required: &[],
    },
    Tool {
        name: "brief_read",
        worker: true,
        desc: "Read a brief, or part of it.",
        props: &[
            ("brief", "string", ""),
            (
                "section",
                "string",
                "headings such as Report or Do, comma list; top = before the first heading",
            ),
            ("head", "number", "first n lines"),
            ("tail", "number", "last n lines"),
        ],
        worker_props: Some(&["brief", "section"]),
        required: &["brief"],
    },
    Tool {
        name: "brief_set_status",
        worker: true,
        desc: "Set the Status line of a brief. The only line of a brief that changes in place.",
        props: &[
            ("brief", "string", ""),
            ("status", "string", "todo, working, done or blocked"),
        ],
        worker_props: None,
        required: &["brief", "status"],
    },
    Tool {
        name: "brief_report",
        worker: true,
        desc: "Append to the bottom of a brief, under one ## Report heading (added once). Set the status to done after it.",
        props: &[("brief", "string", ""), ("text", "string", "Markdown")],
        worker_props: None,
        required: &["brief", "text"],
    },
    Tool {
        name: "scratch_path",
        worker: true,
        desc: "The Scratch directory of a brief; every temporary file goes there.",
        props: &[("brief", "string", "")],
        worker_props: None,
        required: &["brief"],
    },
    Tool {
        name: "contract",
        worker: true,
        desc: "The brief rules.",
        props: &[(
            "role",
            "string",
            "worker, orchestrator or short; default all",
        )],
        worker_props: None,
        required: &[],
    },
    Tool {
        name: "brief_new",
        worker: false,
        desc: "Create a brief and its Scratch directory; returns both paths. The brief has only a header; append your Read, Do and Do not sections at the bottom, once.",
        props: &[
            ("slug", "string", "lowercase words, three at most"),
            ("project", "string", ""),
        ],
        worker_props: None,
        required: &["slug"],
    },
    Tool {
        name: "brief_wait",
        worker: false,
        desc: "Wait for briefs to be done or blocked, then sync the memory. Returns after timeout_secs with finished=false; call again.",
        props: &[
            ("briefs", "array", ""),
            ("any", "boolean", "return on the first"),
            ("timeout_secs", "number", "default 30, at most 300"),
            ("project", "string", ""),
        ],
        worker_props: None,
        required: &["briefs"],
    },
    Tool {
        name: "servers",
        worker: false,
        desc: "Running prototype servers (JSON).",
        props: &[],
        worker_props: None,
        required: &[],
    },
    Tool {
        name: "server_stop",
        worker: false,
        desc: "Stop prototype servers.",
        props: &[("target", "string", "a port, pid:N or all")],
        worker_props: None,
        required: &["target"],
    },
    Tool {
        name: "memory_view",
        worker: true,
        desc: "The project's memory: summary lines that cover the whole chat, oldest first, as <chat> id+n|text. Zoom to open one.",
        props: &[("project", "string", "")],
        worker_props: None,
        required: &[],
    },
    Tool {
        name: "memory_find",
        worker: true,
        desc: "Search the project's memory for words, all of which must appear (\"two words\" stays one). Gives the messages that mention them, and the summary lines over them, each as id+n for memory_zoom. Use it when memory_view does not say where something is: a word the compactions dropped is still in the messages.",
        props: &[
            ("words", "string", "the words to look for"),
            ("limit", "number", "how many hits each way, default 10"),
            ("log", "boolean", "only the messages"),
            ("lines", "boolean", "only the summary lines"),
            (
                "exact",
                "boolean",
                "every line that matched, not only the coarsest over them",
            ),
            (
                "full",
                "boolean",
                "the whole line, not a window around the hit",
            ),
            ("project", "string", ""),
        ],
        worker_props: None,
        required: &["words"],
    },
    Tool {
        name: "memory_zoom",
        worker: true,
        desc: "Open memory line id+n into the two lines it was made from; n = 1 gives message id whole. Or give agent to read a subagent's whole chat.",
        props: &[
            ("id", "number", "a multiple of n"),
            ("n", "number", "a power of 2"),
            ("agent", "string", "a subagent's name"),
            ("project", "string", ""),
        ],
        worker_props: None,
        required: &[],
    },
    Tool {
        name: "memory_date",
        worker: true,
        desc: "The date and time of memory message id.",
        props: &[("id", "number", ""), ("project", "string", "")],
        worker_props: None,
        required: &["id"],
    },
    Tool {
        name: "memory_since",
        worker: true,
        desc: "The memory messages after id, whole. Use it between tool calls to see what the user sent meanwhile.",
        props: &[("id", "number", ""), ("project", "string", "")],
        worker_props: None,
        required: &["id"],
    },
    Tool {
        name: "memory_status",
        worker: true,
        desc: "Memory sizes: messages, nodes, view bytes, lines not summarized yet.",
        props: &[("project", "string", "")],
        worker_props: None,
        required: &[],
    },
    Tool {
        name: "memory_add",
        worker: false,
        desc: "Log a message in the project's memory. kind: user, agent, tool, echo, work or note.",
        props: &[
            ("kind", "string", ""),
            ("text", "string", ""),
            ("project", "string", ""),
        ],
        worker_props: None,
        required: &["kind", "text"],
    },
    Tool {
        name: "memory_sub",
        worker: false,
        desc: "Start a subagent in the background. Its report arrives in the memory as a work message starting [Name]. Do not wait for it.",
        props: &[
            ("name", "string", "letters, digits, - and _"),
            ("task", "string", ""),
            ("project", "string", ""),
        ],
        worker_props: None,
        required: &["name", "task"],
    },
    Tool {
        name: "projects",
        worker: false,
        desc: "The onboarded projects.",
        props: &[],
        worker_props: None,
        required: &[],
    },
];

fn schema(t: &Tool, role: Role) -> String {
    let props: Vec<String> = t
        .props
        .iter()
        .filter(|(name, _, _)| match (role, t.worker_props) {
            (Role::Worker, Some(only)) => only.contains(name),
            _ => true,
        })
        .map(|(name, ty, desc)| {
            let ty_json = if *ty == "array" {
                "{\"type\":\"array\",\"items\":{\"type\":\"string\"}".to_string()
            } else {
                format!("{{\"type\":{}", json::quote(ty))
            };
            if desc.is_empty() {
                format!("{}:{}}}", json::quote(name), ty_json)
            } else {
                format!(
                    "{}:{},\"description\":{}}}",
                    json::quote(name),
                    ty_json,
                    json::quote(desc)
                )
            }
        })
        .collect();
    let required: Vec<String> = t.required.iter().map(|r| json::quote(r)).collect();
    format!(
        "{{\"name\":{},\"description\":{},\"inputSchema\":{{\"type\":\"object\",\"properties\":{{{}}},\"required\":[{}]}}}}",
        json::quote(t.name),
        json::quote(t.desc),
        props.join(","),
        required.join(",")
    )
}

pub fn serve(role: Role) -> Res<()> {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let Some(msg) = json::parse(&line) else {
            writeln!(
                out,
                "{{\"jsonrpc\":\"2.0\",\"id\":null,\"error\":{{\"code\":-32700,\"message\":\"parse error\"}}}}"
            )?;
            out.flush()?;
            continue;
        };
        let Some(id) = msg.get("id") else { continue }; // a notification: no reply
        let method = msg.get("method").and_then(Json::str).unwrap_or("");
        let params = msg.get("params");
        let reply = match method {
            "initialize" => {
                let version = params
                    .and_then(|p| p.get("protocolVersion"))
                    .and_then(Json::str)
                    .unwrap_or("2024-11-05");
                Ok(format!(
                    "{{\"protocolVersion\":{},\"capabilities\":{{\"tools\":{{}}}},\"serverInfo\":{{\"name\":\"brie\",\"version\":{}}}}}",
                    json::quote(version),
                    json::quote(env!("CARGO_PKG_VERSION"))
                ))
            }
            "ping" => Ok("{}".into()),
            "tools/list" => {
                // The memory tools are hidden when this directory's project keeps no memory.
                let no_memory = store::from_cwd().is_some_and(|p| !store::memory_enabled(&p.name));
                let tools: Vec<String> = TOOLS
                    .iter()
                    .filter(|t| role == Role::All || t.worker)
                    .filter(|t| !(no_memory && t.name.starts_with("memory_")))
                    .map(|t| schema(t, role))
                    .collect();
                Ok(format!("{{\"tools\":[{}]}}", tools.join(",")))
            }
            "tools/call" => {
                let name = params
                    .and_then(|p| p.get("name"))
                    .and_then(Json::str)
                    .unwrap_or("");
                let empty = Json::Obj(vec![]);
                let args = params.and_then(|p| p.get("arguments")).unwrap_or(&empty);
                let allowed = TOOLS
                    .iter()
                    .any(|t| t.name == name && (role == Role::All || t.worker));
                let result = if allowed {
                    call(name, args)
                } else {
                    Err(format!("unknown tool {name}"))
                };
                let (text, is_error) = match result {
                    Ok(t) => (t, false),
                    Err(e) => (e, true),
                };
                Ok(format!(
                    "{{\"content\":[{{\"type\":\"text\",\"text\":{}}}],\"isError\":{is_error}}}",
                    json::quote(&text)
                ))
            }
            other => Err(format!("method not found: {other}")),
        };
        match reply {
            Ok(result) => writeln!(
                out,
                "{{\"jsonrpc\":\"2.0\",\"id\":{},\"result\":{result}}}",
                id.write()
            )?,
            Err(message) => writeln!(
                out,
                "{{\"jsonrpc\":\"2.0\",\"id\":{},\"error\":{{\"code\":-32601,\"message\":{}}}}}",
                id.write(),
                json::quote(&message)
            )?,
        }
        out.flush()?;
    }
    Ok(())
}

fn text(args: &Json, key: &str) -> Option<String> {
    args.get(key).and_then(Json::str).map(str::to_string)
}

fn project(args: &Json) -> Result<Project, String> {
    match text(args, "project") {
        Some(name) => store::find(&name).ok_or(format!("no project named {name}")),
        None => store::from_cwd().ok_or("not in an onboarded repo; pass project".to_string()),
    }
}

/// A brief inside the project's briefs folder only. A path outside it is refused.
fn brief(args: &Json) -> Result<(Project, PathBuf), String> {
    let p = project(args)?;
    let name = text(args, "brief").ok_or("brief is required")?;
    let path = store::resolve_brief(&p, &name).map_err(|e| e.to_string())?;
    let dir =
        std::fs::canonicalize(store::kind_dir(&p.name, Kind::Briefs)).map_err(|e| e.to_string())?;
    let real = std::fs::canonicalize(&path).map_err(|e| e.to_string())?;
    if !real.starts_with(&dir) {
        return Err("that file is not a brief of this project".into());
    }
    Ok((p, real))
}

fn call(name: &str, args: &Json) -> Result<String, String> {
    match name {
        "brief_status" => {
            let p = project(args)?;
            if text(args, "brief").is_some() {
                let (_, path) = brief(args)?;
                return Ok(store::status_of(&path));
            }
            let all = args.get("all").and_then(Json::bool).unwrap_or(false);
            let limit = args
                .get("limit")
                .and_then(Json::num)
                .map_or(20, |n| n.clamp(1.0, 200.0) as usize);
            Ok(store::summary_text(
                &p,
                text(args, "status").as_deref(),
                all,
                limit,
                text(args, "if_changed").as_deref(),
            ))
        }
        "brief_read" => {
            let (_, path) = brief(args)?;
            let count = |k: &str| args.get(k).and_then(Json::num).map(|n| n.max(0.0) as usize);
            store::read_part(
                &path,
                text(args, "section").as_deref(),
                count("head"),
                count("tail"),
            )
            .map_err(|e| e.to_string())
        }
        "brief_set_status" => {
            let (_, path) = brief(args)?;
            let status = text(args, "status").ok_or("status is required")?;
            let warning = store::set_status(&path, &status).map_err(|e| e.to_string())?;
            let mut out = format!("Status: {}", store::status_of(&path));
            if let Some(w) = warning {
                out.push_str(&format!("\nwarning: {w}"));
            }
            Ok(out)
        }
        "brief_report" => {
            let (_, path) = brief(args)?;
            let body = text(args, "text").ok_or("text is required")?;
            let warning = store::append_report(&path, &body).map_err(|e| e.to_string())?;
            Ok(match warning {
                Some(w) => format!("appended\nwarning: {w}"),
                None => "appended".into(),
            })
        }
        "scratch_path" => {
            let (p, path) = brief(args)?;
            let stem = path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let dir = store::kind_dir(&p.name, Kind::Prototypes).join(stem);
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            Ok(dir.display().to_string())
        }
        "contract" => {
            let mut out = contract::view(text(args, "role").as_deref().unwrap_or("full"));
            if let Ok(p) = project(args)
                && let Ok(rules) = std::fs::read_to_string(store::rules_path(&p.name))
            {
                out.push_str(&format!(
                    "\nPROJECT RULES ({})\n\n{}\n",
                    p.name,
                    rules.trim_end()
                ));
            }
            Ok(out)
        }
        "brief_new" => {
            let p = project(args)?;
            let slug = text(args, "slug").ok_or("slug is required")?;
            let path = store::new_brief(&p, &slug).map_err(|e| e.to_string())?;
            let stem = path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let scratch = store::kind_dir(&p.name, Kind::Prototypes).join(stem);
            let mut out = format!("brief: {}\nscratch: {}", path.display(), scratch.display());
            if let Some(w) = store::template_warning(&p) {
                out.push_str(&format!("\nwarning: {w}"));
            }
            Ok(out)
        }
        "brief_wait" => {
            let p = project(args)?;
            let names: Vec<String> = args
                .get("briefs")
                .and_then(Json::arr)
                .ok_or("briefs is required")?
                .iter()
                .filter_map(|j| j.str().map(str::to_string))
                .collect();
            if names.is_empty() {
                return Err("briefs is empty".into());
            }
            let paths: Vec<PathBuf> = names
                .iter()
                .map(|n| store::resolve_brief(&p, n).map_err(|e| e.to_string()))
                .collect::<Result<_, _>>()?;
            let any = args.get("any").and_then(Json::bool).unwrap_or(false);
            let secs = args
                .get("timeout_secs")
                .and_then(Json::num)
                .unwrap_or(30.0)
                .clamp(1.0, 300.0);
            let start = Instant::now();
            loop {
                let now: Vec<String> = paths.iter().map(|x| store::status_of(x)).collect();
                let ended = |s: &String| s == "done" || s == "blocked";
                let finished = if any {
                    now.iter().any(ended)
                } else {
                    now.iter().all(ended)
                };
                if finished || start.elapsed().as_secs_f64() >= secs {
                    let mut lines: Vec<String> = paths
                        .iter()
                        .zip(&now)
                        .map(|(x, s)| {
                            format!(
                                "{}: {s}",
                                x.file_stem().unwrap_or_default().to_string_lossy()
                            )
                        })
                        .collect();
                    lines.push(format!("finished: {finished}"));
                    if finished {
                        lines.extend(
                            paths
                                .iter()
                                .filter_map(|x| store::drift(x))
                                .map(|w| format!("warning: {w}")),
                        );
                        lines.extend(memory::sync_quiet(&p));
                    }
                    return Ok(lines.join("\n"));
                }
                std::thread::sleep(Duration::from_secs(1));
            }
        }
        "servers" => Ok(run::running_json()),
        "server_stop" => {
            let target = text(args, "target").ok_or("target is required")?;
            let target = run::Target::parse(&target).ok_or("target is a port, pid:N or all")?;
            Ok(format!("stopped {}", run::stop(target)))
        }
        "memory_view" | "memory_status" => {
            let sub = name.trim_start_matches("memory_");
            memory::run(&project(args)?, &[sub.to_string()]).map_err(|e| e.to_string())
        }
        "memory_find" => {
            let words = text(args, "words").ok_or("words is required")?;
            let mut a = vec!["find".to_string(), words];
            if let Some(n) = args.get("limit").and_then(Json::num) {
                a.push("--limit".to_string());
                a.push((n.max(0.0) as u64).to_string());
            }
            if Json::bool(args.get("log").unwrap_or(&Json::Null)).unwrap_or(false) {
                a.push("--log".to_string());
            }
            if Json::bool(args.get("lines").unwrap_or(&Json::Null)).unwrap_or(false) {
                a.push("--lines".to_string());
            }
            if Json::bool(args.get("exact").unwrap_or(&Json::Null)).unwrap_or(false) {
                a.push("--exact".to_string());
            }
            if Json::bool(args.get("full").unwrap_or(&Json::Null)).unwrap_or(false) {
                a.push("--full".to_string());
            }
            memory::run(&project(args)?, &a).map_err(|e| e.to_string())
        }
        "memory_zoom" | "memory_date" | "memory_since" | "memory_add" | "memory_sub" => {
            let num = |k: &str| {
                args.get(k)
                    .and_then(Json::num)
                    .map(|n| (n.max(0.0) as u64).to_string())
            };
            let need = |v: Option<String>, k: &str| v.ok_or(format!("{k} is required"));
            let mut a = vec![name.trim_start_matches("memory_").to_string()];
            match name {
                "memory_zoom" => match text(args, "agent") {
                    Some(agent) => a.push(agent),
                    None => {
                        a.push(need(num("id"), "id")?);
                        a.push(need(num("n"), "n")?);
                    }
                },
                "memory_add" => {
                    a.push(need(text(args, "kind"), "kind")?);
                    a.push(need(text(args, "text"), "text")?);
                }
                "memory_sub" => {
                    a.push(need(text(args, "name"), "name")?);
                    a.push(need(text(args, "task"), "task")?);
                }
                _ => a.push(need(num("id"), "id")?),
            }
            memory::run(&project(args)?, &a).map_err(|e| e.to_string())
        }
        "projects" => Ok(store::projects()
            .iter()
            .map(|p| format!("{}\t{}", p.name, p.repo.display()))
            .collect::<Vec<_>>()
            .join("\n")),
        other => Err(format!("unknown tool {other}")),
    }
}
