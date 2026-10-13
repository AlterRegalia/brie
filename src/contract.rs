//! The brief contract, the brief template, and the block `brie add` writes into a repo's agent docs.

use std::fs;
use std::path::{Path, PathBuf};

use crate::json::{self, Json};
use crate::store::Res;

pub const CONTRACT: &str = "\
BRIEF CONTRACT

A brief is the whole task for one worker. The Orchestrator writes it. The worker reads it,
does it, and reports in it.

Create
  brie new \"<slug>\"   prints the path and makes the Scratch directory.
  Slug: lowercase words joined by hyphens, three words at most.
  Name: DD_MM_YYYY_HH:MM_<slug>.md. The name and the file stay for the life of the brief.
  The new brief has only a header: title, Status, Repo, Scratch. It has no sections.

Write
- Write one brief for each task. Keep it under one screen.
- Append your sections below the header of the brief that brie new made, once: ## Read, ## Do, ## Do not.
  Leave the lines already written as they are. Write a new task in a new brief.
- Write one imperative instruction for each numbered step. Put the condition before the command.
- Under ## Read, name exact paths and say why each one matters. Link an issue rather than restate it.
- Put hard limits under ## Do not. A worker obeys a listed limit and ignores a hinted one.
- Make step 1 of ## Do this: brie set <brief> working. The worker sets it before it reads anything.

Status
  todo      The Orchestrator wrote the brief.
  working   The worker sets it first.
  done      The worker sets it last, after it appends ## Report.
  blocked   The worker cannot continue. The Report says why.
Status is the only signal the Orchestrator trusts. A brief still on todo long after the spawn
means the worker dropped the prompt.

Append only
A brief grows only at the bottom. The Status line is the one line that changes in place, and only brie set changes it.
A correction is a new entry, and the newest entry is the truth.
The first brie set that leaves todo seals the brief. After that, brie set, brie report, brie wait and brie doctor all warn
when the text above the sealed length changes.
  brie set <brief> <status>   sets the Status line: todo, working, done or blocked.
  brie report <brief> <text>  appends to the bottom, under one ## Report heading (text from stdin if missing).
The worker reports with Files, Commands run, Decisions, Open. One line for each item, no preamble.

Wait
  brie wait <brief>... [--timeout seconds] [--any] [--notify]
      Blocks. Prints each Status change. Exit 0 = done, 2 = blocked, 1 = timeout.
      Waits for all the briefs by default. --any returns when the first one ends.
      Run it in the background.
  brie status [brief] [--all] [--status s] [--limit n] [--json]
      With no brief: counts by status, then the briefs that are not done, newest first, 20 at most.
      Short on purpose: use it instead of listing every brief. One brief: its status.
  <brief> is a path, a file name, or a part of the name that matches one brief.
When wait returns, check the Report against the real diff and the project checks. Then accept.

Scratch
Every temporary file goes in the Scratch directory the brief names.

Remove
  brie rm <brief>   moves the brief and its Scratch directory to the Trash.

Project rules
  brie template init   writes this project's brief template to the store, for editing.
  rules.md in the project's store folder adds project rules to this contract.
";

/// The default brief: a header and nothing else, so no section is ever filled in place.
/// `{{title}}`, `{{repo}}` and `{{stem}}` are filled in by `fill`.
/// A project can replace it: `brie template init` writes it to the store for editing.
pub const DEFAULT_TEMPLATE: &str = "\
# Brief — {{title}}

Status: todo
Repo: {{repo}}. Keep your reads and writes inside it.
Scratch: `.prototype/{{stem}}/`. Every temporary file goes there.
";

pub fn fill(template: &str, repo: &Path, stem: &str, title: &str) -> String {
    template
        .replace("{{title}}", title)
        .replace("{{repo}}", &repo.display().to_string())
        .replace("{{stem}}", stem)
}

pub const START: &str = "<!-- brie:start -->";
const END: &str = "<!-- brie:end -->";

const BLOCK_HEAD: &str = "\
<!-- brie:start -->
## Briefs and prototypes (brie)

`.briefs/` and `.prototype/` are symlinks into the brie store. Write through them, and never commit them.

- Orchestrator: `brie new`, `brie wait`, `brie status`. Rules: `brie contract orchestrator`.
- Worker: your brief is your whole task. `brie set <brief> working` first, `brie report <brief> <text>`, `brie set <brief> done` last. Rules: `brie contract worker`.
- Use the `brie` MCP tools when the client lists them. Otherwise run `brie` with a subcommand: a bare `brie` opens a blocking TUI.
";

/// Only for a project that turned the memory on with `brie add --memory`.
const BLOCK_MEMORY: &str = "\
- Memory: read the view first (`memory_view`). When a word is not in it, run `memory_find`. For detail, run `memory_zoom` and `memory_since`. The Orchestrator logs decisions and corrections with `memory_add` (kind `note`). Workers read only.
";

const BLOCK_TAIL: &str = "\
- When a write through `.briefs/` or `.prototype/` asks for permission, tell the human to run `brie add` again.

Managed by `brie add`. Edits inside this block are replaced.
<!-- brie:end -->";

pub fn block(memory: bool) -> String {
    format!(
        "{BLOCK_HEAD}{}{BLOCK_TAIL}",
        if memory { BLOCK_MEMORY } else { "" }
    )
}

fn splice(text: &str, block: &str) -> String {
    if let (Some(a), Some(b)) = (text.find(START), text.find(END))
        && a < b
    {
        return format!("{}{block}{}", &text[..a], &text[b + END.len()..]);
    }
    let mut out = text.trim_end().to_string();
    if !out.is_empty() {
        out.push_str("\n\n");
    }
    out.push_str(block);
    out.push('\n');
    out
}

/// Write the block into the repo's `CLAUDE.md` and `AGENTS.md`. Create `AGENTS.md` when neither exists.
/// Returns the files that changed.
pub fn inject(repo: &Path, memory: bool) -> Res<Vec<PathBuf>> {
    let block = block(memory);
    let mut targets: Vec<PathBuf> = ["CLAUDE.md", "AGENTS.md"]
        .iter()
        .map(|f| repo.join(f))
        .filter(|p| p.is_file())
        .collect();
    if targets.is_empty() {
        targets.push(repo.join("AGENTS.md"));
    }
    let mut changed = vec![];
    for target in targets {
        let text = fs::read_to_string(&target).unwrap_or_default();
        let next = splice(&text, &block);
        if next != text {
            fs::write(&target, next)?;
            changed.push(target);
        }
    }
    Ok(changed)
}

/// Let Claude Code write through `.briefs/` and `.prototype/` without a prompt: add the project's store
/// folder to `permissions.additionalDirectories` in the repo's `.claude/settings.local.json`.
/// Returns the file when it changed. A file that is not valid JSON is left alone.
pub fn allow_store(repo: &Path, dir: &Path) -> Res<Option<PathBuf>> {
    let file = repo.join(".claude").join("settings.local.json");
    let text = fs::read_to_string(&file).unwrap_or_default();
    let mut root = if text.trim().is_empty() {
        Json::Obj(vec![])
    } else {
        match json::parse(&text) {
            Some(j @ Json::Obj(_)) => j,
            _ => {
                return Err(
                    format!("{} is not a JSON object; left unchanged", file.display()).into(),
                );
            }
        }
    };
    let dir = Json::Str(dir.display().to_string());
    let Json::Obj(top) = &mut root else {
        unreachable!()
    };
    let perms = obj_entry(top, "permissions");
    let Json::Obj(perms) = perms else {
        return Err(format!(
            "`permissions` in {} is not an object; left unchanged",
            file.display()
        )
        .into());
    };
    let Json::Arr(dirs) = obj_entry(perms, "additionalDirectories") else {
        return Err(format!(
            "`additionalDirectories` in {} is not an array; left unchanged",
            file.display()
        )
        .into());
    };
    if dirs.iter().any(|d| d.str() == dir.str()) {
        return Ok(None);
    }
    dirs.push(dir);
    fs::create_dir_all(file.parent().unwrap_or(repo))?;
    fs::write(&file, root.write() + "\n")?;
    Ok(Some(file))
}

/// The value under `key`, added as an empty object or array (by the key's name) when missing.
fn obj_entry<'a>(items: &'a mut Vec<(String, Json)>, key: &str) -> &'a mut Json {
    let at = items.iter().position(|(k, _)| k == key).unwrap_or_else(|| {
        let empty = if key == "permissions" {
            Json::Obj(vec![])
        } else {
            Json::Arr(vec![])
        };
        items.push((key.to_string(), empty));
        items.len() - 1
    });
    &mut items[at].1
}

const TITLES: [&str; 8] = [
    "Create",
    "Write",
    "Status",
    "Append only",
    "Wait",
    "Scratch",
    "Remove",
    "Project rules",
];

/// The contract for one role: `worker`, `orchestrator`, `short`, or anything else for all of it.
pub fn view(role: &str) -> String {
    let mut parts: Vec<(&str, String)> = vec![];
    let mut intro = String::new();
    let mut current: Option<usize> = None;
    for line in CONTRACT.lines() {
        if let Some(at) = TITLES.iter().position(|t| *t == line) {
            parts.push((TITLES[at], String::new()));
            current = Some(parts.len() - 1);
        }
        match current {
            Some(i) => {
                parts[i].1.push_str(line);
                parts[i].1.push('\n');
            }
            None => {
                intro.push_str(line);
                intro.push('\n');
            }
        }
    }
    let pick = |names: &[&str]| -> String {
        names
            .iter()
            .filter_map(|n| {
                parts
                    .iter()
                    .find(|(t, _)| t == n)
                    .map(|(_, text)| text.clone())
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    match role {
        "worker" => format!("BRIEF CONTRACT (worker)\n\n{}", pick(&["Status", "Append only", "Scratch"])),
        "orchestrator" => format!(
            "BRIEF CONTRACT (orchestrator)\n\n{}",
            pick(&["Create", "Write", "Status", "Append only", "Wait", "Remove", "Project rules"])
        ),
        "short" => "Brief contract: one brief for each task. Status: todo, working, done, blocked. The worker runs brie set working first and done last, \
and brie report appends ## Report at the bottom. Nothing else in a brief changes. Temporary files go in the Scratch directory. \
`brie new` creates a brief, `brie wait` waits, `brie status` is the short report. Full text: `brie contract worker` or `brie contract orchestrator`.\n"
            .to_string(),
        _ => format!("{intro}{}", parts.iter().map(|(_, t)| t.clone()).collect::<Vec<_>>().join("\n")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("brie-allow-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join(".claude")).unwrap();
        dir
    }

    #[test]
    fn allow_store_keeps_settings_and_is_idempotent() {
        let dir = repo("merge");
        let file = dir.join(".claude/settings.local.json");
        fs::write(
            &file,
            r#"{"model":"opus","permissions":{"allow":["Bash(ls)"]}}"#,
        )
        .unwrap();
        let store = Path::new("/store/brie");
        assert_eq!(allow_store(&dir, store).unwrap(), Some(file.clone()));
        let json = json::parse(&fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(json.get("model").and_then(Json::str), Some("opus"));
        let perms = json.get("permissions").unwrap();
        assert_eq!(
            perms.get("allow").and_then(Json::arr).map(<[_]>::len),
            Some(1)
        );
        let dirs = perms
            .get("additionalDirectories")
            .and_then(Json::arr)
            .unwrap();
        assert_eq!(dirs.len(), 1);
        assert_eq!(dirs[0].str(), Some("/store/brie"));
        assert_eq!(allow_store(&dir, store).unwrap(), None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn allow_store_leaves_invalid_json_alone() {
        let dir = repo("invalid");
        let file = dir.join(".claude/settings.local.json");
        fs::write(&file, "{ not json").unwrap();
        assert!(allow_store(&dir, Path::new("/store/brie")).is_err());
        assert_eq!(fs::read_to_string(&file).unwrap(), "{ not json");
        let _ = fs::remove_dir_all(&dir);
    }
}
