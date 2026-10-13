use std::error::Error;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

use crate::contract;

pub type Res<T> = Result<T, Box<dyn Error>>;

#[derive(Clone)]
pub struct Project {
    pub name: String,
    pub repo: PathBuf,
}

#[derive(Clone)]
pub struct Item {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub modified: SystemTime,
    /// The `Status:` of a brief (todo, working, done, blocked). `None` for anything else.
    pub status: Option<String>,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    Briefs,
    Prototypes,
}

impl Kind {
    pub fn dir(self) -> &'static str {
        match self {
            Kind::Briefs => "briefs",
            Kind::Prototypes => "prototypes",
        }
    }
}

pub const LINKS: [(&str, Kind); 2] = [(".briefs", Kind::Briefs), (".prototype", Kind::Prototypes)];

pub fn home() -> PathBuf {
    // ponytail: still panics when HOME is entirely missing; return Res up the stack if that bites.
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
        .expect("HOME is not set (set HOME or BRIE_HOME)")
}

pub fn root() -> PathBuf {
    std::env::var_os("BRIE_HOME")
        .filter(|h| !h.is_empty())
        .map(|h| expand(&h.to_string_lossy()))
        .unwrap_or_else(|| home().join(".alter-store"))
}

pub fn expand(s: &str) -> PathBuf {
    match s.strip_prefix("~/") {
        Some(rest) => home().join(rest),
        None if s == "~" => home(),
        None => PathBuf::from(s),
    }
}

pub fn kind_dir(project: &str, kind: Kind) -> PathBuf {
    root().join(project).join(kind.dir())
}

fn index_file() -> PathBuf {
    root().join("projects")
}

pub fn projects() -> Vec<Project> {
    let text = fs::read_to_string(index_file()).unwrap_or_default();
    let mut list: Vec<Project> = text
        .lines()
        .filter_map(|l| {
            let (name, repo) = l.split_once('\t')?;
            Some(Project {
                name: name.into(),
                repo: repo.into(),
            })
        })
        .collect();
    list.sort_by(|a, b| a.name.cmp(&b.name));
    list
}

fn save(list: &[Project]) -> Res<()> {
    fs::create_dir_all(root())?;
    let text: String = list
        .iter()
        .map(|p| format!("{}\t{}\n", p.name, p.repo.display()))
        .collect();
    let tmp = root().join("projects.tmp");
    fs::write(&tmp, text)?;
    fs::rename(tmp, index_file())?;
    Ok(())
}

pub fn find(name: &str) -> Option<Project> {
    projects().into_iter().find(|p| p.name == name)
}

/// Project whose repo contains the current directory. The deepest repo wins.
pub fn from_cwd() -> Option<Project> {
    let cwd = fs::canonicalize(std::env::current_dir().ok()?).ok()?;
    projects()
        .into_iter()
        .filter(|p| cwd.starts_with(&p.repo))
        .max_by_key(|p| p.repo.components().count())
}

pub fn add(path: &Path, name: Option<&str>) -> Res<Project> {
    let repo = fs::canonicalize(path)?;
    if !repo.is_dir() {
        return Err(format!("{} is not a directory", repo.display()).into());
    }
    let name = match name {
        Some(n) => n.to_string(),
        None => repo
            .file_name()
            .ok_or("cannot derive a project name from this path")?
            .to_string_lossy()
            .into_owned(),
    };
    if name.is_empty() || name.starts_with('.') || name.contains('/') {
        return Err(format!("bad project name: {name}").into());
    }
    let mut list = projects();
    if let Some(p) = list.iter().find(|p| p.name == name)
        && p.repo != repo
    {
        return Err(format!("name {name} is used by {}", p.repo.display()).into());
    }

    let plan: Vec<(PathBuf, PathBuf)> = LINKS
        .iter()
        .map(|(link, kind)| (repo.join(link), kind_dir(&name, *kind)))
        .collect();
    for (link, target) in &plan {
        check_adopt(link, target)?;
    }
    for (link, target) in &plan {
        fs::create_dir_all(target)?;
        adopt(link, target)?;
    }

    let project = Project { name, repo };
    list.retain(|p| p.name != project.name);
    list.push(project.clone());
    save(&list)?;
    Ok(project)
}

fn check_adopt(link: &Path, target: &Path) -> Res<()> {
    let Ok(meta) = fs::symlink_metadata(link) else {
        return Ok(());
    };
    if meta.file_type().is_symlink() {
        if fs::canonicalize(link).ok() != fs::canonicalize(target).ok() {
            return Err(format!("{} is a link to somewhere else", link.display()).into());
        }
    } else if meta.is_dir() {
        for e in fs::read_dir(link)? {
            let dest = target.join(e?.file_name());
            if dest.exists() {
                return Err(format!("{} already exists in the store", dest.display()).into());
            }
        }
    } else {
        return Err(format!("{} is not a directory", link.display()).into());
    }
    Ok(())
}

fn adopt(link: &Path, target: &Path) -> Res<()> {
    match fs::symlink_metadata(link) {
        Ok(m) if m.file_type().is_symlink() => {
            if fs::canonicalize(link).ok() == fs::canonicalize(target).ok() {
                return Ok(());
            }
            fs::remove_file(link)?;
        }
        Ok(m) if m.is_dir() => {
            for e in fs::read_dir(link)? {
                let e = e?;
                fs::rename(e.path(), target.join(e.file_name()))?;
            }
            fs::remove_dir(link)?;
        }
        _ => {}
    }
    symlink(fs::canonicalize(target)?, link)?;
    Ok(())
}

pub fn timestamp() -> String {
    Command::new("date")
        .arg("+%d_%m_%Y_%H:%M")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "undated".into())
}

pub fn slugify(s: &str) -> String {
    let mut out = String::new();
    for c in s.trim().to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

pub fn new_brief(project: &Project, slug: &str) -> Res<PathBuf> {
    let slug = slugify(slug);
    if slug.is_empty() {
        return Err("brief needs a name".into());
    }
    let dir = kind_dir(&project.name, Kind::Briefs);
    fs::create_dir_all(&dir)?;
    let base = format!("{}_{slug}", timestamp());
    let mut stem = base.clone();
    let mut n = 0;
    while dir.join(format!("{stem}.md")).exists() {
        n += 1;
        stem = format!("{base}_{n}");
    }
    let path = dir.join(format!("{stem}.md"));
    let title = slug.replace('-', " ");
    fs::write(
        &path,
        contract::fill(&template_text(project), &project.repo, &stem, &title),
    )?;
    fs::create_dir_all(kind_dir(&project.name, Kind::Prototypes).join(&stem))?;
    Ok(path)
}

pub fn template_path(project: &str) -> PathBuf {
    root().join(project).join("template.md")
}

pub fn rules_path(project: &str) -> PathBuf {
    root().join(project).join("rules.md")
}

/// The project's own template if it has one, else the default.
pub fn template_text(project: &Project) -> String {
    fs::read_to_string(template_path(&project.name))
        .unwrap_or_else(|_| contract::DEFAULT_TEMPLATE.to_string())
}

/// A warning when the project's own template has `## ` sections: a brief is never filled in place,
/// so the template is a header only.
pub fn template_warning(project: &Project) -> Option<String> {
    sections_warning(&fs::read_to_string(template_path(&project.name)).ok()?)
}

fn sections_warning(template: &str) -> Option<String> {
    let sections: Vec<&str> = template.lines().filter(|l| l.starts_with("## ")).collect();
    (!sections.is_empty()).then(|| {
        format!(
            "template.md has sections ({}); briefs are never filled in place, so keep only the header lines and put project rules in rules.md",
            sections.join(", ")
        )
    })
}

/// Move a file or folder to the system Trash, never overwriting what is already there.
pub fn trash(path: &Path) -> Res<PathBuf> {
    let dir = if cfg!(target_os = "macos") {
        home().join(".Trash")
    } else {
        home().join(".local/share/Trash/files")
    };
    fs::create_dir_all(&dir)?;
    let name = path
        .file_name()
        .ok_or("nothing to remove")?
        .to_string_lossy()
        .into_owned();
    let mut dest = dir.join(&name);
    let mut n = 0;
    while dest.exists() {
        n += 1;
        dest = dir.join(format!("{name}.{n}"));
    }
    fs::rename(path, &dest)?;
    Ok(dest)
}

/// Trash a brief and its Scratch directory.
pub fn remove_brief(project: &Project, brief: &Path) -> Res<()> {
    let stem = brief
        .file_stem()
        .ok_or("not a brief")?
        .to_string_lossy()
        .into_owned();
    let seal = seal_path(brief);
    trash(brief)?;
    if let Some(seal) = seal {
        let _ = fs::remove_file(seal);
    }
    let scratch = kind_dir(&project.name, Kind::Prototypes).join(stem);
    if scratch.is_dir() {
        trash(&scratch)?;
    }
    Ok(())
}

pub fn opener() -> &'static str {
    if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    }
}

/// Show a path in the file manager.
pub fn reveal(path: &Path) {
    if cfg!(target_os = "macos") {
        let _ = Command::new("open").arg("-R").arg(path).status();
    } else {
        let dir = if path.is_dir() {
            path
        } else {
            path.parent().unwrap_or(path)
        };
        let _ = Command::new("xdg-open").arg(dir).status();
    }
}

/// Entries of a store folder, newest first.
pub fn list(project: &str, kind: Kind) -> Vec<Item> {
    let Ok(rd) = fs::read_dir(kind_dir(project, kind)) else {
        return vec![];
    };
    let mut items: Vec<Item> = rd
        .flatten()
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .map(|e| {
            let path = e.path();
            let mtime = e
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            let is_dir = path.is_dir();
            let status = (kind == Kind::Briefs && !is_dir).then(|| status_of(&path));
            Item {
                name: e.file_name().to_string_lossy().into_owned(),
                is_dir,
                modified: mtime,
                status,
                path,
            }
        })
        .collect();
    items.sort_by_key(|a| std::cmp::Reverse(a.modified));
    items
}

/// The value of the `Status:` line near the top of a brief: todo, working, done, blocked.
pub fn status_of(path: &Path) -> String {
    let mut buf = vec![0u8; 2048];
    let n = fs::File::open(path)
        .and_then(|mut f| std::io::Read::read(&mut f, &mut buf))
        .unwrap_or(0);
    String::from_utf8_lossy(&buf[..n])
        .lines()
        .take(12)
        .find_map(|l| l.strip_prefix("Status:"))
        .and_then(|v| v.split_whitespace().next().map(str::to_lowercase))
        .unwrap_or_else(|| "unknown".into())
}

fn memory_setting(project: &str) -> PathBuf {
    root().join(project).join("memory.setting")
}

/// Whether the project keeps a memory. Off unless `brie add --memory` turned it on. A project from before the
/// setting existed counts as on when it already has a memory folder.
pub fn memory_enabled(project: &str) -> bool {
    match fs::read_to_string(memory_setting(project))
        .as_deref()
        .map(str::trim)
    {
        Ok("on") => true,
        Ok("off") => false,
        _ => root().join(project).join("memory").is_dir(),
    }
}

/// Turn the memory on or off. Turning it off keeps the log on disk.
pub fn set_memory(project: &str, on: bool) -> Res<()> {
    fs::create_dir_all(root().join(project))?;
    fs::write(memory_setting(project), if on { "on\n" } else { "off\n" })?;
    Ok(())
}

/// Whether a brief can be edited in place: only a draft (`todo`), or a file with no Status line.
/// After hand-off a brief only grows at the bottom, through `brie report`.
pub fn is_draft(path: &Path) -> bool {
    matches!(status_of(path).as_str(), "todo" | "unknown")
}

pub const STATUSES: [&str; 4] = ["todo", "working", "done", "blocked"];

fn is_status_line(i: usize, line: &str) -> bool {
    i < 12 && line.starts_with("Status:")
}

/// The brief without its `Status:` line, the one line that changes in place.
fn core(text: &str) -> String {
    let mut found = false;
    text.split_inclusive('\n')
        .enumerate()
        .filter(|(i, l)| {
            let skip = !found && is_status_line(*i, l);
            found |= skip;
            !skip
        })
        .map(|(_, l)| l)
        .collect()
}

fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x100000001b3)
    })
}

/// Where the seal of a brief lives: `<project>/seals/<stem>`, beside `briefs/`, never inside it.
fn seal_path(brief: &Path) -> Option<PathBuf> {
    let real = fs::canonicalize(brief).ok()?;
    let stem = real.file_stem()?;
    Some(real.parent()?.parent()?.join("seals").join(stem))
}

/// Record how long the brief is, and a hash of it, apart from its `Status:` line. From then on the
/// text up to that length must stay as it is. Only the end can grow.
fn seal(brief: &Path) {
    let (Some(at), Ok(text)) = (seal_path(brief), fs::read_to_string(brief)) else {
        return;
    };
    let core = core(&text);
    if let Some(dir) = at.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let _ = fs::write(
        at,
        format!("{} {:016x}\n", core.len(), fnv(core.as_bytes())),
    );
}

/// A message when text that was sealed at hand-off has changed in place. `None` when it is intact,
/// or when the brief was never handed off.
pub fn drift(brief: &Path) -> Option<String> {
    let sealed = fs::read_to_string(seal_path(brief)?).ok()?;
    let (len, hash) = sealed.trim().split_once(' ')?;
    let len: usize = len.parse().ok()?;
    let text = fs::read_to_string(brief).ok()?;
    let core = core(&text);
    let intact = core
        .as_bytes()
        .get(..len)
        .is_some_and(|head| format!("{:016x}", fnv(head)) == hash);
    (!intact).then(|| {
        format!(
            "{}: the first {len} bytes changed after hand-off; a brief only grows at the bottom",
            brief.file_stem().unwrap_or_default().to_string_lossy()
        )
    })
}

/// Called after every write by brie. A brief with no seal is sealed when it leaves `todo`. A sealed one
/// is sealed again only if it had not drifted, so a hand edit is never blessed.
fn after_write(brief: &Path, drifted: bool, still_draft: bool) {
    if drifted {
        return;
    }
    let sealed = seal_path(brief).is_some_and(|p| p.is_file());
    if sealed || !still_draft {
        seal(brief);
    }
}

/// Set the `Status:` line. It is the only line of a brief that changes in place.
/// Returns a warning when the brief changed in place earlier.
pub fn set_status(path: &Path, status: &str) -> Res<Option<String>> {
    let status = status.trim().to_lowercase();
    if !STATUSES.contains(&status.as_str()) {
        return Err("status is todo, working, done or blocked".into());
    }
    let warning = drift(path);
    let body = fs::read_to_string(path)?;
    let mut out = String::with_capacity(body.len() + 8);
    let mut found = false;
    for (i, line) in body.split_inclusive('\n').enumerate() {
        if !found && is_status_line(i, line) {
            found = true;
            out.push_str(&format!("Status: {status}"));
            out.push_str(&line[line.trim_end_matches(['\r', '\n']).len()..]);
        } else {
            out.push_str(line);
        }
    }
    if !found {
        return Err("the brief has no Status line".into());
    }
    fs::write(path, out)?;
    after_write(path, warning.is_some(), status == "todo");
    Ok(warning)
}

/// Append to the bottom of a brief. The `## Report` heading is added once. Later calls add under it.
/// Returns a warning when the brief changed in place earlier.
pub fn append_report(path: &Path, text: &str) -> Res<Option<String>> {
    let text = text.trim();
    if text.is_empty() {
        return Err("the report is empty".into());
    }
    let warning = drift(path);
    let mut body = fs::read_to_string(path)?;
    if !body.ends_with('\n') {
        body.push('\n');
    }
    let has_heading = body.starts_with("## Report") || body.contains("\n## Report");
    if !has_heading {
        body.push_str("\n## Report\n");
    }
    body.push('\n');
    body.push_str(text);
    body.push('\n');
    fs::write(path, body)?;
    after_write(path, warning.is_some(), false);
    Ok(warning)
}

/// A brief by path, file name, or a part of its name that matches exactly one brief.
pub fn resolve_brief(project: &Project, arg: &str) -> Res<PathBuf> {
    let direct = Path::new(arg);
    if direct.is_file() {
        return Ok(direct.to_path_buf());
    }
    let briefs = list(&project.name, Kind::Briefs);
    let exact: Vec<&Item> = briefs
        .iter()
        .filter(|i| i.name == arg || i.name.strip_suffix(".md") == Some(arg))
        .collect();
    let found: Vec<&Item> = if exact.is_empty() {
        briefs.iter().filter(|i| i.name.contains(arg)).collect()
    } else {
        exact
    };
    match found.as_slice() {
        [one] => Ok(one.path.clone()),
        [] => Err(format!("no brief matches {arg}").into()),
        many => {
            let names: Vec<&str> = many.iter().map(|i| i.name.as_str()).collect();
            Err(format!(
                "{arg} matches {} briefs:\n  {}",
                many.len(),
                names.join("\n  ")
            )
            .into())
        }
    }
}

const ORDER: [&str; 5] = ["todo", "working", "blocked", "done", "unknown"];

/// A short report on a project's briefs, so an agent need not read the whole list: counts by status, then
/// the briefs that are not done (or those with one `filter` status, or all with `all`), newest first.
pub fn summary_text(
    project: &Project,
    filter: Option<&str>,
    all: bool,
    limit: usize,
    if_changed: Option<&str>,
) -> String {
    let (counts, shown, total) = summary_data(project, filter, all);
    let mut head: Vec<String> = ORDER
        .iter()
        .zip(counts)
        .filter(|(_, n)| *n > 0)
        .map(|(name, n)| format!("{n} {name}"))
        .collect();
    if head.is_empty() {
        head.push("no briefs".into());
    }
    let mut lines = vec![format!(
        "{}: {} ({total} briefs)",
        project.name,
        head.join(" · ")
    )];
    for i in shown.iter().take(limit) {
        lines.push(format!(
            "{:<8} {}",
            i.status.as_deref().unwrap_or("unknown"),
            i.name
        ));
    }
    if shown.len() > limit {
        lines.push(format!("({} more: raise --limit)", shown.len() - limit));
    }
    if filter.is_none() && !all && counts[3] > 0 {
        lines.push(format!(
            "({} done not shown: --all, or --status done)",
            counts[3]
        ));
    }
    let body = lines.join("\n");
    let hash = short_hash(&body);
    if if_changed == Some(hash.as_str()) {
        return "unchanged".into();
    }
    format!("{body}\nstate {hash}")
}

/// Six hex digits that change when the text does. An agent passes it back to ask "has anything changed?".
fn short_hash(text: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in text.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{:06x}", h & 0xffffff)
}

pub fn summary_json(
    project: &Project,
    filter: Option<&str>,
    all: bool,
    limit: usize,
    if_changed: Option<&str>,
) -> String {
    let (counts, shown, total) = summary_data(project, filter, all);
    let counts_json: Vec<String> = ORDER
        .iter()
        .zip(counts)
        .map(|(name, n)| format!("{}:{n}", crate::json::quote(name)))
        .collect();
    let briefs: Vec<String> = shown
        .iter()
        .take(limit)
        .map(|i| {
            format!(
                "{{\"name\":{},\"status\":{}}}",
                crate::json::quote(&i.name),
                crate::json::quote(i.status.as_deref().unwrap_or("unknown"))
            )
        })
        .collect();
    let body = format!(
        "\"project\":{},\"total\":{total},\"counts\":{{{}}},\"briefs\":[{}],\"not_shown\":{}",
        crate::json::quote(&project.name),
        counts_json.join(","),
        briefs.join(","),
        shown.len().saturating_sub(limit)
    );
    let hash = short_hash(&body);
    if if_changed == Some(hash.as_str()) {
        return "{\"unchanged\":true}".into();
    }
    format!("{{{body},\"state\":\"{hash}\"}}")
}

fn summary_data(
    project: &Project,
    filter: Option<&str>,
    all: bool,
) -> ([usize; 5], Vec<Item>, usize) {
    let briefs = list(&project.name, Kind::Briefs);
    let mut counts = [0; 5];
    for i in &briefs {
        let name = i.status.as_deref().unwrap_or("unknown");
        let at = ORDER.iter().position(|o| *o == name).unwrap_or(4);
        counts[at] += 1;
    }
    let total = briefs.len();
    let shown = briefs
        .into_iter()
        .filter(|i| {
            let st = i.status.as_deref().unwrap_or("unknown");
            match filter {
                Some(f) => st == f,
                None => all || st != "done",
            }
        })
        .collect();
    (counts, shown, total)
}

/// Part of a brief: `sections` is a comma list of `## ` headings (or `top` for what comes before the first one),
/// then `head` or `tail` lines of the result.
pub fn read_part(
    path: &Path,
    sections: Option<&str>,
    head: Option<usize>,
    tail: Option<usize>,
) -> Res<String> {
    let text = fs::read_to_string(path)?;
    let mut out = match sections {
        None => text,
        Some(names) => {
            let lines: Vec<&str> = text.lines().collect();
            let headings: Vec<(usize, String)> = lines
                .iter()
                .enumerate()
                .filter_map(|(i, l)| l.strip_prefix("## ").map(|h| (i, h.trim().to_lowercase())))
                .collect();
            let first = headings.first().map_or(lines.len(), |h| h.0);
            let mut picked: Vec<String> = vec![];
            for want in names
                .split(',')
                .map(|n| n.trim().to_lowercase())
                .filter(|n| !n.is_empty())
            {
                if want == "top" || want == "header" {
                    picked.push(lines[..first].join("\n"));
                } else if let Some(k) = headings.iter().position(|(_, h)| h.starts_with(&want)) {
                    let end = headings.get(k + 1).map_or(lines.len(), |h| h.0);
                    picked.push(lines[headings[k].0..end].join("\n"));
                } else {
                    let have: Vec<&str> = headings.iter().map(|(_, h)| h.as_str()).collect();
                    return Err(
                        format!("no section {want}; sections: top, {}", have.join(", ")).into(),
                    );
                }
            }
            picked.join("\n\n")
        }
    };
    if head.is_some() || tail.is_some() {
        let lines: Vec<&str> = out.lines().collect();
        let kept: Vec<&str> = match (head, tail) {
            (Some(n), _) => lines.iter().take(n).copied().collect(),
            (None, Some(n)) => lines[lines.len().saturating_sub(n)..].to_vec(),
            _ => lines,
        };
        out = kept.join("\n");
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn brief(name: &str, body: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("brie-brief-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("briefs")).unwrap();
        let path = dir.join("briefs/b.md");
        fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn set_status_changes_only_the_status_line() {
        let path = brief(
            "status",
            "# Brief\n\nStatus: todo\nRepo: /r\n\n## Do\n\nStatus: not me\n",
        );
        set_status(&path, "Working").unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "# Brief\n\nStatus: working\nRepo: /r\n\n## Do\n\nStatus: not me\n"
        );
        assert_eq!(status_of(&path), "working");
        assert!(set_status(&path, "fixed").is_err());
        let _ = fs::remove_dir_all(path.parent().unwrap().parent().unwrap());
    }

    #[test]
    fn set_status_needs_a_status_line() {
        let path = brief("nostatus", "# Brief\n");
        assert!(set_status(&path, "done").is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "# Brief\n");
        let _ = fs::remove_dir_all(path.parent().unwrap().parent().unwrap());
    }

    #[test]
    fn report_adds_one_heading_and_only_appends() {
        let head = "# Brief\n\nStatus: todo\n\n## Do\n\n1. x";
        let path = brief("report", head);
        append_report(&path, "Files: a.rs").unwrap();
        append_report(&path, "Open: none\n").unwrap();
        let body = fs::read_to_string(&path).unwrap();
        assert!(body.starts_with(head));
        assert_eq!(body.matches("## Report").count(), 1);
        assert!(body.ends_with("## Report\n\nFiles: a.rs\n\nOpen: none\n"));
        assert!(append_report(&path, "  \n").is_err());
        let _ = fs::remove_dir_all(path.parent().unwrap().parent().unwrap());
    }

    #[test]
    fn template_with_sections_is_flagged() {
        assert!(
            sections_warning("# Brief\n\nStatus: todo\n\n## Do\n")
                .unwrap()
                .contains("## Do")
        );
        assert!(sections_warning("# Brief\n\nStatus: todo\n").is_none());
        assert!(sections_warning(contract::DEFAULT_TEMPLATE).is_none());
    }

    #[test]
    fn drift_is_flagged_after_hand_off_only() {
        let path = brief("drift", "# Brief\n\nStatus: todo\n\n## Do\n\n1. x\n");
        // A draft can change freely.
        fs::write(&path, "# Brief v2\n\nStatus: todo\n\n## Do\n\n1. y\n").unwrap();
        assert_eq!(set_status(&path, "todo").unwrap(), None);
        assert!(drift(&path).is_none());
        // Hand-off seals it. Growing at the bottom and the Status line are fine.
        assert_eq!(set_status(&path, "working").unwrap(), None);
        let mut body = fs::read_to_string(&path).unwrap();
        body.push_str("\n2. z\n");
        fs::write(&path, &body).unwrap();
        assert!(drift(&path).is_none());
        assert_eq!(append_report(&path, "Files: a").unwrap(), None);
        assert_eq!(set_status(&path, "done").unwrap(), None);
        assert!(drift(&path).is_none());
        // An edit above the sealed length is flagged and never re-sealed.
        let edited = fs::read_to_string(&path)
            .unwrap()
            .replace("1. y", "1. HACKED");
        fs::write(&path, edited).unwrap();
        assert!(drift(&path).unwrap().contains("changed after hand-off"));
        let warning = append_report(&path, "Open: none").unwrap();
        assert!(warning.is_some());
        assert!(drift(&path).is_some());
        let _ = fs::remove_dir_all(path.parent().unwrap().parent().unwrap());
    }

    #[test]
    fn only_a_draft_is_editable() {
        let path = brief("draft", "# Brief\n\nStatus: todo\n");
        assert!(is_draft(&path));
        set_status(&path, "working").unwrap();
        assert!(!is_draft(&path));
        set_status(&path, "done").unwrap();
        assert!(!is_draft(&path));
        fs::write(&path, "# no status\n").unwrap();
        assert!(is_draft(&path));
        let _ = fs::remove_dir_all(path.parent().unwrap().parent().unwrap());
    }

    #[test]
    fn block_has_the_memory_line_only_when_on() {
        assert!(contract::block(true).contains("memory_view"));
        assert!(!contract::block(false).contains("memory"));
        assert!(contract::block(false).ends_with("<!-- brie:end -->"));
    }

    #[test]
    fn a_shorter_brief_is_drift() {
        let path = brief("short", "# Brief\n\nStatus: todo\n\n## Do\n\n1. x\n");
        set_status(&path, "working").unwrap();
        fs::write(&path, "# Brief\n\nStatus: working\n").unwrap();
        assert!(drift(&path).is_some());
        let _ = fs::remove_dir_all(path.parent().unwrap().parent().unwrap());
    }

    #[test]
    fn default_template_has_no_sections() {
        let t = contract::fill(contract::DEFAULT_TEMPLATE, Path::new("/r"), "s", "t");
        assert!(!t.lines().any(|l| l.starts_with("## ")));
        assert!(t.contains("Status: todo"));
    }
}
