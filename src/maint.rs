//! Maintenance commands: link a worktree, check the setup, back up the store, notify.

use std::fs;
use std::os::unix::fs::symlink;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::contract;
use crate::store::{self, Kind, LINKS, Project, Res};

/// Point `.briefs` and `.prototype` in a worktree at the project's store folders.
pub fn link(worktree: &Path, project: &Project) -> Res<Vec<String>> {
    let worktree = fs::canonicalize(worktree)?;
    let mut done = vec![];
    for (name, kind) in LINKS {
        let link = worktree.join(name);
        let target = store::kind_dir(&project.name, kind);
        fs::create_dir_all(&target)?;
        match fs::symlink_metadata(&link) {
            Ok(m) if m.file_type().is_symlink() => {
                if fs::canonicalize(&link).ok() != fs::canonicalize(&target).ok() {
                    return Err(format!("{} is a link to somewhere else", link.display()).into());
                }
            }
            Ok(_) => return Err(format!("{} exists and is not a link", link.display()).into()),
            Err(_) => {
                symlink(fs::canonicalize(&target)?, &link)?;
                done.push(format!("{} -> {}", link.display(), target.display()));
            }
        }
    }
    Ok(done)
}

/// Check every project. Prints one line each and returns the number of problems.
pub fn doctor() -> usize {
    let projects = store::projects();
    if projects.is_empty() {
        println!("no projects yet: run `brie add` in a repo");
        return 0;
    }
    let mut problems = 0;
    for p in projects {
        let mut found: Vec<String> = vec![];
        if !p.repo.is_dir() {
            found.push(format!("repo {} is gone", p.repo.display()));
        } else {
            for (name, kind) in LINKS {
                let link = p.repo.join(name);
                let target = store::kind_dir(&p.name, kind);
                match fs::symlink_metadata(&link) {
                    Ok(m) if m.file_type().is_symlink() => {
                        if fs::canonicalize(&link).ok() != fs::canonicalize(&target).ok() {
                            found.push(format!("{name} points elsewhere"));
                        }
                    }
                    Ok(_) => found.push(format!("{name} is a real folder: run `brie add`")),
                    Err(_) => found.push(format!("{name} is missing: run `brie add`")),
                }
            }
            let docs: Vec<_> = ["CLAUDE.md", "AGENTS.md"]
                .iter()
                .map(|f| p.repo.join(f))
                .filter(|f| f.is_file())
                .collect();
            if docs.is_empty() {
                found.push("no CLAUDE.md or AGENTS.md: run `brie add`".into());
            }
            for doc in docs {
                let text = fs::read_to_string(&doc).unwrap_or_default();
                if !text.contains(contract::START) {
                    found.push(format!(
                        "{} has no brie block: run `brie add`",
                        doc.file_name().unwrap_or_default().to_string_lossy()
                    ));
                }
            }
        }
        for kind in [Kind::Briefs, Kind::Prototypes] {
            if !store::kind_dir(&p.name, kind).is_dir() {
                found.push(format!("store folder {} is missing", kind.dir()));
            }
        }
        if let Some(w) = store::template_warning(&p) {
            found.push(w);
        }
        for item in store::list(&p.name, Kind::Briefs) {
            if !item.is_dir
                && let Some(w) = store::drift(&item.path)
            {
                found.push(w);
            }
        }
        let unknown = store::list(&p.name, Kind::Briefs)
            .iter()
            .filter(|i| i.status.as_deref() == Some("unknown"))
            .count();
        if unknown > 0 {
            found.push(format!("{unknown} briefs have no Status line"));
        }
        if found.is_empty() {
            println!("ok    {}", p.name);
        } else {
            problems += found.len();
            for line in found {
                println!("FAIL  {}: {line}", p.name);
            }
        }
    }
    problems
}

const IGNORED: [&str; 7] = [
    "*/prototypes/",
    ".DS_Store",
    "projects",
    "projects.tmp",
    "state",
    "servers",
    "notify",
];

fn git(args: &[&str]) -> Res<std::process::Output> {
    let out = Command::new("git")
        .arg("-C")
        .arg(store::root())
        .args(args)
        .output()?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr)
            .trim()
            .to_string()
            .into());
    }
    Ok(out)
}

fn git_ok(args: &[&str]) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(store::root())
        .args(args)
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Prototypes are big and can be rebuilt. The index, the view state, the server list and the notify
/// target belong to one machine, and the notify target is a secret. None of them is backed up.
fn prepare_repo() -> Res<()> {
    let root = store::root();
    if !root.join(".git").exists() {
        git(&["init", "-q", "-b", "main"])?;
    }
    let path = root.join(".gitignore");
    let have = fs::read_to_string(&path).unwrap_or_default();
    let missing: Vec<&str> = IGNORED
        .iter()
        .copied()
        .filter(|l| !have.lines().any(|h| h == *l))
        .collect();
    if !missing.is_empty() {
        let mut text = have;
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&missing.join("\n"));
        text.push('\n');
        fs::write(path, text)?;
    }
    for file in ["projects", "state", "servers", "notify"] {
        if git_ok(&["ls-files", "--error-unmatch", file]) {
            git(&["rm", "--cached", "-q", file])?;
        }
    }
    Ok(())
}

/// Commit the store's briefs and templates to a git repo inside the store.
pub fn backup() -> Res<String> {
    prepare_repo()?;
    git(&["add", "-A"])?;
    if git_ok(&["diff", "--cached", "--quiet"]) {
        return Ok("nothing to back up".into());
    }
    git(&[
        "commit",
        "-q",
        "-m",
        &format!("brie backup {}", store::timestamp()),
    ])?;
    Ok(format!("backed up to {}", store::root().display()))
}

/// Back up, then pull and push a git remote. Briefs only grow at the bottom, so a clash is rare.
/// When one happens, the rebase is undone and the files are named.
pub fn sync(remote: Option<&str>) -> Res<String> {
    let mut notes = vec![backup()?];
    if let Some(url) = remote {
        if git_ok(&["remote", "get-url", "origin"]) {
            git(&["remote", "set-url", "origin", url])?;
        } else {
            git(&["remote", "add", "origin", url])?;
        }
    }
    if !git_ok(&["remote", "get-url", "origin"]) {
        return Err("no remote: run `brie sync --remote <git url>`".into());
    }
    let branch = String::from_utf8_lossy(&git(&["rev-parse", "--abbrev-ref", "HEAD"])?.stdout)
        .trim()
        .to_string();
    let remote_has_branch = git(&["ls-remote", "--heads", "origin", &branch])
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(false);
    if remote_has_branch {
        if let Err(e) = git(&["pull", "--rebase", "--autostash", "origin", &branch]) {
            let files = git(&["diff", "--name-only", "--diff-filter=U"])
                .map(|o| {
                    String::from_utf8_lossy(&o.stdout)
                        .trim()
                        .replace('\n', ", ")
                })
                .unwrap_or_default();
            let _ = git(&["rebase", "--abort"]);
            let reason = e.to_string().lines().last().unwrap_or("").to_string();
            return Err(format!("both machines edited the same lines of: {files}. Nothing was changed. Fix one side and sync again. ({reason})").into());
        }
        notes.push("pulled".into());
    }
    git(&["push", "-q", "-u", "origin", &branch])?;
    notes.push(format!("pushed {branch}"));
    Ok(notes.join("; "))
}

// ---- notifications -------------------------------------------------------------------------

fn notify_path() -> std::path::PathBuf {
    store::root().join("notify")
}

pub fn notify_target() -> Option<String> {
    fs::read_to_string(notify_path())
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// `ntfy:<topic>`, `ntfy:<https url>`, or `url:<https webhook>`.
pub fn set_notify(target: &str) -> Res<()> {
    let ok = if let Some(topic) = target.strip_prefix("ntfy:") {
        topic.starts_with("https://")
            || (!topic.is_empty()
                && topic
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'))
    } else if let Some(url) = target.strip_prefix("url:") {
        url.starts_with("https://") || url.starts_with("http://localhost")
    } else {
        false
    };
    if !ok {
        return Err("use ntfy:<topic>, ntfy:<https url> or url:<https webhook>".into());
    }
    fs::create_dir_all(store::root())?;
    fs::write(notify_path(), format!("{target}\n"))?;
    Ok(())
}

pub fn clear_notify() {
    let _ = fs::remove_file(notify_path());
}

/// Send a push message to the configured target. With `wait`, return after curl ends.
pub fn push(message: &str, wait: bool) {
    let Some(target) = notify_target() else {
        return;
    };
    let mut cmd = Command::new("curl");
    cmd.args(["-fsS", "-m", "10", "-o", "/dev/null"]);
    if let Some(topic) = target.strip_prefix("ntfy:") {
        let url = if topic.starts_with("https://") {
            topic.to_string()
        } else {
            format!("https://ntfy.sh/{topic}")
        };
        cmd.args(["-H", "Title: brie", "-d", message, &url]);
    } else if let Some(url) = target.strip_prefix("url:") {
        let body = format!(
            "{{\"text\":{0},\"content\":{0}}}",
            crate::json::quote(message)
        );
        cmd.args(["-H", "Content-Type: application/json", "-d", &body, url]);
    } else {
        return;
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if wait {
        let _ = cmd.status();
    } else {
        let _ = cmd.spawn();
    }
}

/// A desktop notification, and a push message when a target is set.
pub fn notify(message: &str) {
    push(message, true);
    let clean = message.replace(['"', '\\'], "");
    if cfg!(target_os = "macos") {
        let script = format!("display notification \"{clean}\" with title \"brie\"");
        let _ = Command::new("osascript").arg("-e").arg(script).status();
    } else {
        let _ = Command::new("notify-send").arg("brie").arg(clean).status();
    }
}
