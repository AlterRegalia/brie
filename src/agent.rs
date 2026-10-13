//! Turns and subagents for `brie memory`. An agent is a shell command (`$BRIE_AGENT`) that reads a prompt on
//! stdin and prints its work on stdout. With `claude -p --output-format stream-json --verbose` every reply,
//! tool call and tool result is logged as it happens. Any other command is logged as one reply.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

use crate::json::{self, Json};
use crate::memory::{self, Memory, Summarizer};
use crate::store::{Project, Res};

const TURN_PROMPT: &str = "\
You are an AI agent that works for one user, in a single chat that never ends. Each call to you is a turn:
the view below comes first, then the new message from the user.

# The view

Your memory: the whole chat between you and the user, oldest first, inside <chat> tags, as one-line summaries:

  id+n|text   the n messages from id on, summarized (newlines as spaces)

Each message has a kind:
- user: the user's words
- agent: your replies
- tool: your tool calls
- echo: tool results
- work: a worker's report, starting \"[Name]\"
- note: memories from before this chat

The summaries form a binary tree. The memory compresses each message into a line (a short message is its own
line), then merges adjacent lines in pairs, then merges those results in pairs again. Thus recent lines cover
one message each, and older lines cover more. A message not summarized yet shows as \"(not summarized yet:
zoom it)\". A text too long for one message is split over several in a row.

Tools (the brie MCP tools memory_find, memory_zoom, memory_date and memory_since, or the same commands in a shell):
- `brie memory find <words>`: use it when the view does not say where something is. It gives the messages
  that hold the words, and the lines over them as id+n.
- `brie memory zoom <id> <n>` opens line id+n into the two lines it was made from. `<n> = 1` gives message
  id whole. `<Name>` alone gives a subagent's whole chat.
- `brie memory date <id>` gives the date and time of message id.
- `brie memory since <id>` gives the messages after id, whole.

# Turns

Do the user's tasks yourself, with your tools. Use subagents (`brie memory sub <Name> <task>`) only when the user
asks for them.

The view is your memory, and its latest word on a subject is the truth. When you need information, find its latest
mention in the view first, and zoom until you have it whole. Do that before you use another source, and before you
act, guess or ask. Zoom is the way down the tree. Summaries keep little of tool output, so say in your reply what
you learned that will matter later.

Messages the user sends while you work are filed in the chat. Between tool calls, run `brie memory since <id>` with
the id of the last message you have seen. A subagent's report reaches the chat as a message starting \"[Name]\".
Go on with your own work while a subagent runs, or end your turn and tell the user what is running.
";

/// The agent command, from `$BRIE_AGENT`.
pub fn command() -> Res<String> {
    match std::env::var("BRIE_AGENT") {
        Ok(c) if !c.trim().is_empty() => Ok(c),
        _ => Err(
            "BRIE_AGENT is not set. It is a shell command that reads a prompt on stdin and prints its work, \
for example `claude -p --output-format stream-json --verbose`"
                .into(),
        ),
    }
}

pub enum Ev {
    Agent(String),
    Tool(String),
    Echo(String),
}

impl Ev {
    pub fn kind(&self) -> &'static str {
        match self {
            Ev::Agent(_) => "agent",
            Ev::Tool(_) => "tool",
            Ev::Echo(_) => "echo",
        }
    }

    pub fn text(&self) -> &str {
        match self {
            Ev::Agent(t) | Ev::Tool(t) | Ev::Echo(t) => t,
        }
    }
}

struct Parsed {
    events: Vec<Ev>,
    result: Option<String>,
}

fn block_text(content: Option<&Json>) -> String {
    match content {
        Some(Json::Str(s)) => s.clone(),
        Some(Json::Arr(items)) => items
            .iter()
            .filter_map(|b| b.get("text").and_then(Json::str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// One line of Claude Code's `stream-json` output. `None` when the line is not an event. Thoughts are
/// never logged.
fn parse_event(line: &str) -> Option<Parsed> {
    let j = json::parse(line)?;
    let kind = j.get("type")?.str()?;
    let mut out = Parsed {
        events: vec![],
        result: None,
    };
    let blocks = j
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(Json::arr)
        .unwrap_or_default();
    match kind {
        "assistant" => {
            for b in blocks {
                match b.get("type").and_then(Json::str) {
                    Some("text") => {
                        let t = b.get("text").and_then(Json::str).unwrap_or_default();
                        if !t.trim().is_empty() {
                            out.events.push(Ev::Agent(t.to_string()));
                        }
                    }
                    Some("tool_use") => {
                        let name = b.get("name").and_then(Json::str).unwrap_or("tool");
                        let input = b.get("input").map(Json::write).unwrap_or_default();
                        out.events.push(Ev::Tool(format!("{name} {input}")));
                    }
                    _ => {}
                }
            }
        }
        "user" => {
            for b in blocks {
                if b.get("type").and_then(Json::str) == Some("tool_result") {
                    out.events.push(Ev::Echo(block_text(b.get("content"))));
                }
            }
        }
        "result" => out.result = j.get("result").and_then(Json::str).map(str::to_string),
        _ => {}
    }
    Some(out)
}

/// Run the agent command in `cwd` with `prompt` on stdin. `handle` gets each event as it happens, and `None`
/// about every 300 ms while the agent is quiet. Returns the final reply.
fn run_agent(
    cmd: &str,
    cwd: &Path,
    prompt: &str,
    mut handle: impl FnMut(Option<&Ev>) -> Res<()>,
) -> Res<String> {
    let mut child = Command::new("sh")
        .args(["-c", cmd])
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;
    let mut stdin = child.stdin.take().ok_or("no stdin")?;
    let prompt = prompt.to_string();
    std::thread::spawn(move || stdin.write_all(prompt.as_bytes()));
    let stdout = child.stdout.take().ok_or("no stdout")?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let (mut stream, mut plain, mut last, mut result) = (None, String::new(), String::new(), None);
    let looped = (|| -> Res<()> {
        loop {
            match rx.recv_timeout(Duration::from_millis(300)) {
                Ok(line) => match (stream, parse_event(&line)) {
                    (None | Some(true), Some(p)) => {
                        stream = Some(true);
                        for ev in &p.events {
                            if let Ev::Agent(t) = ev {
                                last = t.clone();
                            }
                            handle(Some(ev))?;
                        }
                        result = p.result.or(result.take());
                    }
                    (Some(true), None) => {}
                    _ => {
                        stream = Some(false);
                        plain.push_str(&line);
                        plain.push('\n');
                    }
                },
                Err(RecvTimeoutError::Timeout) => handle(None)?,
                Err(RecvTimeoutError::Disconnected) => return Ok(()),
            }
        }
    })();
    if looped.is_err() {
        let _ = child.kill();
    }
    let status = child.wait()?;
    looped?;
    if stream != Some(true) && !plain.trim().is_empty() {
        last = plain.trim().to_string();
        handle(Some(&Ev::Agent(last.clone())))?;
    }
    if !status.success() {
        return Err(format!("the agent exited with {status}").into());
    }
    Ok(result.unwrap_or(last))
}

/// One turn: build the tree, take the view, log the user's message, run the agent on `[view] [message]`,
/// log what it does as it does it, and file the messages that others send meanwhile. `show` gets the reply as
/// soon as it is known. Then the tree is built for the messages of this turn, so the next one starts at once.
pub fn turn(
    repo: &Path,
    dir: &Path,
    agent: &str,
    s: &dyn Summarizer,
    text: &str,
    show: &mut dyn FnMut(&str),
) -> Res<()> {
    let mut m = Memory::open(dir, true).map_err(|e| {
        format!("{e}. A turn is running: leave your message with `brie memory add user <text>`")
    })?;
    m.build(s)?;
    m.maintain()?;
    let view = m.render();
    let id = m.len();
    m.append("user", text)?;
    let prompt = format!(
        "{TURN_PROMPT}\n{view}\ndate: {}\nThe last message you have seen is {}.\n\nMessage {id} from the user:\n{text}\n",
        memory::now(),
        id.saturating_sub(1)
    );
    let reply = run_agent(agent, repo, &prompt, |ev| {
        if let Some(e) = ev {
            m.append(e.kind(), e.text())?;
        }
        m.drain_inbox()?;
        Ok(())
    })?;
    show(&reply);
    m.drain_inbox()?;
    m.build(s)?;
    m.maintain()?;
    Ok(())
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 40
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Start a subagent in the background: a fresh call whose first message is the view, then its task. Its own
/// steps go to `agents/<Name>/chat.log`. Its final reply comes back to the chat as one `work` message.
pub fn sub(project: &Project, dir: &Path, name: &str, task: &str) -> Res<String> {
    if !valid_name(name) {
        return Err("a name is letters, digits, - and _".into());
    }
    let ad = dir.join("agents").join(name);
    if ad.join("task.md").exists() {
        return Err(format!("an agent named {name} exists; pick another name").into());
    }
    let view = Memory::open(dir, false)?.render();
    fs::create_dir_all(&ad)?;
    fs::write(
        ad.join("task.md"),
        format!(
            "{TURN_PROMPT}\nYou are the subagent {name}. Your final reply goes to the chat as one message, \"[{name}] report\".\n\n{view}\ndate: {}\n\nYour task:\n{task}\n",
            memory::now()
        ),
    )?;
    let log = fs::File::create(ad.join("run.log"))?;
    Command::new(std::env::current_exe()?)
        .args(["memory", "sub-run", name, "-p", &project.name])
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .process_group(0)
        .spawn()?;
    Ok(format!(
        "{name} started. Its report arrives as a work message; `brie memory zoom {name}` shows its chat"
    ))
}

/// The body of a subagent process.
pub fn sub_run(repo: &Path, dir: &Path, agent: &str, name: &str) -> Res<()> {
    let ad = dir.join("agents").join(name);
    let prompt = fs::read_to_string(ad.join("task.md"))?;
    let mut chat = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(ad.join("chat.log"))?;
    let reply = run_agent(agent, repo, &prompt, |ev| {
        if let Some(e) = ev {
            write!(chat, "{}: {}\n\n", e.kind(), e.text())?;
            chat.flush()?;
        }
        Ok(())
    });
    let report = match reply {
        Ok(r) => format!("[{name}] report: {r}"),
        Err(e) => format!("[{name}] failed: {e}"),
    };
    memory::deliver(dir, "work", &report)?;
    fs::write(ad.join("done"), "")?;
    Ok(())
}

/// The subagents of a memory, by name, and whether each is done.
pub fn agents(dir: &Path) -> Vec<(String, bool)> {
    let mut out: Vec<(String, bool)> = fs::read_dir(dir.join("agents"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().join("task.md").exists())
        .map(|e| {
            (
                e.file_name().to_string_lossy().into_owned(),
                e.path().join("done").exists(),
            )
        })
        .collect();
    out.sort();
    out
}

/// A subagent's whole chat.
pub fn chat(dir: &Path, name: &str) -> Res<String> {
    let ad = dir.join("agents").join(name);
    if !ad.join("task.md").exists() {
        return Err(format!("no agent named {name}").into());
    }
    let log = fs::read_to_string(ad.join("chat.log")).unwrap_or_default();
    let state = if ad.join("done").exists() {
        "done"
    } else {
        "running"
    };
    Ok(format!("{name} ({state})\n\n{log}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake;
    impl Summarizer for Fake {
        fn run(&self, _prompt: &str, parts: &[&str]) -> Res<String> {
            Ok(parts.join(" | ").chars().take(100).collect())
        }
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("brie-agent-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn stream_json_events_become_messages_and_thoughts_are_dropped() {
        let text = parse_event(
            r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"hmm"},{"type":"text","text":"on it"},{"type":"tool_use","name":"Bash","input":{"command":"ls"}}]}}"#,
        )
        .unwrap();
        let got: Vec<(&str, &str)> = text.events.iter().map(|e| (e.kind(), e.text())).collect();
        assert_eq!(
            got,
            vec![("agent", "on it"), ("tool", "Bash {\"command\":\"ls\"}")]
        );
        let echo = parse_event(
            r#"{"type":"user","message":{"content":[{"type":"tool_result","content":[{"type":"text","text":"a.rs"}]}]}}"#,
        )
        .unwrap();
        assert_eq!(echo.events[0].text(), "a.rs");
        let done = parse_event(r#"{"type":"result","result":"all done"}"#).unwrap();
        assert_eq!(done.result.as_deref(), Some("all done"));
        assert!(parse_event("just text").is_none());
    }

    #[test]
    fn a_turn_logs_the_messages_and_the_agents_work() {
        let (repo, dir) = (scratch("turn"), scratch("turn-mem"));
        let agent = r#"cat >/dev/null; printf '%s\n' '{"type":"assistant","message":{"content":[{"type":"text","text":"checking"},{"type":"tool_use","name":"Bash","input":{"command":"ls"}}]}}' '{"type":"user","message":{"content":[{"type":"tool_result","content":"a.rs"}]}}' '{"type":"result","result":"found a.rs"}'"#;
        let ask = |agent: &str, text: &str| {
            let mut reply = String::new();
            turn(&repo, &dir, agent, &Fake, text, &mut |r| {
                reply = r.to_string()
            })
            .map(|()| reply)
        };
        assert_eq!(ask(agent, "what files are here?").unwrap(), "found a.rs");
        let m = Memory::open(&dir, false).unwrap();
        let kinds: Vec<&str> = m.msgs_kinds();
        assert_eq!(kinds, vec!["user", "agent", "tool", "echo"]);
        // The next turn sees the first one in its view, and a plain agent is logged as one reply.
        let plain = r#"prompt=$(cat); case "$prompt" in *"user: what files are here?"*) echo "saw the view";; *) echo "no view";; esac"#;
        assert_eq!(ask(plain, "again").unwrap(), "saw the view");
        // A finished turn leaves nothing unsummarized; a failed one still logs what the user said.
        let unbuilt = |dir: &Path| Memory::open(dir, false).unwrap().status();
        assert!(unbuilt(&dir).contains(" 0 lines not summarized"));
        assert!(ask("exit 3", "boom").is_err());
        assert!(unbuilt(&dir).contains(" 1 lines not summarized"));
        let _ = fs::remove_dir_all(&repo);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_message_sent_during_a_turn_is_filed_by_the_turn() {
        let (repo, dir) = (scratch("mid"), scratch("mid-mem"));
        let d2 = dir.clone();
        let sender = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(500));
            memory::deliver(&d2, "user", "also fix the footer").unwrap()
        });
        let mut reply = String::new();
        let script = "cat >/dev/null; sleep 1.5; echo ok";
        turn(&repo, &dir, script, &Fake, "start", &mut |r| {
            reply = r.to_string()
        })
        .unwrap();
        assert_eq!(reply, "ok");
        assert!(
            !sender.join().unwrap(),
            "the turn owned the log, so the message was queued"
        );
        let m = Memory::open(&dir, false).unwrap();
        assert!(m.since(0).contains("also fix the footer"));
        let _ = fs::remove_dir_all(&repo);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_subagent_reports_back_to_the_chat() {
        let (repo, dir) = (scratch("sub"), scratch("sub-mem"));
        let ad = dir.join("agents").join("Scout");
        fs::create_dir_all(&ad).unwrap();
        fs::write(ad.join("task.md"), "find the bug").unwrap();
        sub_run(
            &repo,
            &dir,
            "cat >/dev/null; echo 'bug is in login.rs'",
            "Scout",
        )
        .unwrap();
        let m = Memory::open(&dir, false).unwrap();
        assert_eq!(m.len(), 1);
        assert!(
            m.zoom(0, 1)
                .unwrap()
                .contains("[Scout] report: bug is in login.rs")
        );
        assert!(chat(&dir, "Scout").unwrap().starts_with("Scout (done)"));
        assert!(chat(&dir, "Nobody").is_err());
        assert!(!valid_name("a b") && valid_name("Scout-2"));
        let _ = fs::remove_dir_all(&repo);
        let _ = fs::remove_dir_all(&dir);
    }
}
