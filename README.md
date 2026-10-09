<p align="center"><img src="assets/logo.svg" alt="brie" width="136"></p>

# brie

Keep project briefs and prototypes on your machine, outside the repo. Browse them in a TUI.

## Install

Download `brie-macos-arm64.tar.gz` (Apple silicon) or `brie-linux-x86_64.tar.gz` (Linux x86_64) from Releases:

```
tar -xzf brie-macos-arm64.tar.gz
mkdir -p ~/.local/bin && mv brie ~/.local/bin/
```

Or build from source (see Build).

## Quick start

```
cd your-repo
brie add          # onboard: move .briefs/ and .prototype/ into the store, leave symlinks
brie              # open the TUI
```

`brie add` is safe to repeat. It changes nothing when the links are right.

## Store

```
~/.alter-store/<project>/briefs/
~/.alter-store/<project>/prototypes/
~/.alter-store/projects            # name<TAB>repo path, one line per project
```

Set `BRIE_HOME` to use another root.

## Commands

```
brie                           TUI
brie add [path] [--name n] [--no-inject] [--no-allow] [--memory|--no-memory]
                               onboard a repo (default: current dir)
brie ls                        list projects
brie new <name> [-p project]   create a brief and its Scratch directory, print the path
brie path [briefs|prototypes] [-p project]
brie status [brief] [--all] [--status s] [--limit n] [--json]
brie set <brief> <status>      todo | working | done | blocked
brie report <brief> [text]     append under ## Report (stdin if no text)
brie read <brief> [--section s] [--head n] [--tail n]
brie wait <brief>... [--any] [--notify] [-t secs]
                               exit 0 all done, 2 blocked, 1 timeout
brie rm <brief>                move brief and Scratch to the Trash
brie link [worktree]           point a worktree's .briefs and .prototype at the store
brie template [init]           show the brief template, or write it to the store
brie contract [role]           print the rules: worker | orchestrator | short
brie doctor                    check links, instruction blocks and store; exit 1 on problem
brie backup                    commit briefs and templates to a git repo in the store
brie sync [--remote url]       backup, then pull and push
brie serve <dir> [--cmd '...' [--no-port]] [--vite|--static]
brie serve start|ls|logs|stop|approve
brie open <path>               open a file in the system viewer
brie memory ...               see Memory
brie notify set|off|show|test  push target for done and blocked briefs
brie mcp [--worker]            MCP server over stdio
brie daemon [start|stop|status]  macOS menu bar item
brie help <command>
```

Without `-p`, the project is the onboarded repo that holds the current directory.

## Briefs

`brie contract` prints the rules. In short:

- `brie new "<slug>"` writes a brief with `Status: todo`, a Repo fence and a Scratch line, and makes the Scratch directory in `prototypes/`.
  The brief has no sections. The Orchestrator appends `## Read`, `## Do` and `## Do not` once, at the bottom.
- The worker runs `brie set <brief> working` first and `brie set <brief> done` last, with `brie report` between.
  If it cannot continue it sets `blocked` and says why.
- A brief is append only. `brie set` changes the `Status:` line. `brie report` appends under one `## Report` heading.
- The first `brie set` that leaves `todo` seals the brief (length and hash, ignoring Status, saved in `<store>/<project>/seals/`).
  `brie set`, `report`, `wait` and `doctor` warn if sealed text changes. A drifted brief is not sealed again.
- The Orchestrator runs `brie wait <brief>` in the background. It prints each Status change and exits 0 on `done`, 2 on `blocked`, 1 on timeout.
  If memory is on, it runs `brie memory sync` first. A failed sync does not change the exit code.
  `<brief>` is a path, a file name, or part of a name that matches one brief.

### What `brie add` writes

- A block in the repo's `CLAUDE.md` and `AGENTS.md` (creates `AGENTS.md` if neither exists), between `<!-- brie:start -->` and
  `<!-- brie:end -->`. Running `brie add` again replaces it. Skip with `--no-inject`.
- The store folder in `permissions.additionalDirectories` of `.claude/settings.local.json`, so Claude Code does not ask on each write
  through `.briefs/` and `.prototype/`. A file that is not a JSON object is left alone, and the command says why. Skip with `--no-allow`.

## For agents

Read the block in `CLAUDE.md` or `AGENTS.md`.

```
brie ls                                # is this repo onboarded?
brie add                               # no: onboard it from the repo root
f=$(brie new "cup pin server")         # Orchestrator: create a brief, get its path
brie wait cup-pin-server -t 1800 &     # Orchestrator: wait for the worker
brie status                            # Orchestrator: counts and open briefs
d="$(brie path prototypes)/<brief name without .md>"
mkdir -p "$d" && brie serve "$d"       # serve a prototype
```

Rules:

- Write briefs and scratch output only in `brie path briefs` and `brie path prototypes`. Never commit them.
- Never run bare `brie`. It opens a full-screen TUI and blocks.
- `brie serve` and `brie wait` block. Run them in the background and stop them when done.
- Pass `-p <project>` outside an onboarded repo.
- If a write through `.briefs/` or `.prototype/` asks for permission, tell the human to run `brie add` again. Never edit permission settings.

## TUI

| Key | Action |
| --- | --- |
| `j` `k` / `h` `l` | move / change pane |
| `tab` | switch Briefs and Prototypes |
| `enter` | brief: built-in editor. Prototype: serve and open the browser |
| `e` | open in `$VISUAL` / `$EDITOR` |
| `n` `x` `a` | new brief / Trash (asks first) / add repo |
| `m` `s` `v` | memory / serve / servers modal |
| `r` `R` | reveal in file manager / reload |
| `J` `K`, PageUp PageDown | scroll preview |
| `1` `2` `3` `4` `5` | projects, Briefs tab, Prototypes tab, preview, status. Press again to hide a pane (not both projects and items) |
| `?` `q` | help / quit |

Everything has a click as well as a key. A click selects the project or list row under it, and a second click opens it. The tabs, the footer hints, the servers count in the header, the rows of the Status box (they filter by status) and the buttons of a window are click targets, and each row of the `?` help runs its key when clicked. In the memory window the tabs, rows (a second click zooms in), breadcrumb stops and footer hints are click targets. In the editor a click puts the cursor there, and the footer hints press their keys. The wheel acts on the pane under it: the list moves its selection and the preview scrolls. Projects and Status ignore the wheel.
Brie captures the mouse for this, so select text with `option` (macOS) or `shift` held down.
The list refreshes every second. A brief that turns `done` or `blocked` rings the bell and shows in the footer.
Badges: `○` todo, `◐` working, `●` done, `✕` blocked. The two-row header shows the mark, name and version, then the project, brief and prototype counts and running servers. The Status pane (`5`) counts briefs for each status.
The layout follows the terminal size and the panes you toggled on. Projects and Status share a column on the left, 22% of the width (18 to 28 columns), Status under Projects when the column is 14 rows tall or more.
List and preview go side by side when the area right of projects is 100 columns wide (80 on a terminal under 20 rows), and stack otherwise.
A pane that would fall under 10 columns or 3 rows is dropped: Status first, then the preview.
Panes, sort, date window, tab and project are saved in `state` in the store.

In the items pane:

- `/` find. Terms are ANDed and match the name or, for a file, its first 64 KB. `status:working` matches status. `enter` keeps the filter, `esc` clears it.
- `o` order: newest, oldest, A-Z, Z-A.
- `d` modified window: all, 24h, 7d, 30d.
- `f` status filter (briefs): all, todo, working, done, blocked.
- `esc` with a filter on clears all filters. Without one it asks to quit.

The bottom edge shows `shown/total`, the sort and active filters.

### Editor

`enter` on a brief opens the editor; `e` opens `$EDITOR`. Both edit only while the brief is `todo`.
After hand-off, `enter` opens the same window read only (title `VIEW`): move, find (`ctrl-f`), copy, side preview, and it follows the file as a worker reports.
`e` refuses, because a brief only grows at the bottom. The editor renders Markdown as you type. The cursor line, or each selected line, shows raw source.

| Key | Action |
| --- | --- |
| `ctrl-s` | save. If the file changed on disk, the first press warns, the second overwrites |
| `ctrl-r` | reload from disk (edits stay on the undo stack) |
| `ctrl-z` | undo, 200 steps; a run of typing is one step |
| `shift` + arrows, `home`, `end` | select |
| `ctrl-c` `ctrl-x` `ctrl-v` | copy, cut (whole line if no selection), paste. System clipboard |
| `ctrl-f` | find: type to jump, `enter` next, `esc` close |
| `ctrl-p` | side pane with the rendered document |
| `esc` | clear selection, then close (a second `esc` discards unsaved changes) |

It reloads by itself when the file changes on disk and you have no unsaved edits. Otherwise the title shows `changed on disk`.
On save, tabs become two spaces and CRLF becomes LF.

### Memory window

`m` opens the project's memory full screen. It reads without the lock, so it works during a turn, and refreshes every second.

- **VIEW** tab (`1`): one `id+n  summary` line per node, oldest first, newest selected. `enter`, `l` or `→` zoom into a line (its two source lines).
  `backspace`, `h` or `←` go back. A message cannot be zoomed.
  Each line starts with a glyph and color for its kind: `▌` user, `◆` agent, `⚙` tool, `⎘` echo, `◐` work, `✎` note.
  Each kind has its own glyph and its own color (amber, white, violet, grey, blue, green), so it reads without color too. A terminal 110 columns wide or more shows the key in the header.
  Each line also has a bar for how much of the biggest line in the list it covers, and a row above the list shows the way down: `view › 0+1024 › 0+16`.
  The preview shows a message as it was said (kind, date, words). A line that covers 8 messages or fewer lists them below its summary, as a conversation. A summary also names its parent (the line it merges into) and its sibling.
- **AGENTS** tab (`2`; `tab` switches between the two): subagents, `◐` running, `●` done. The preview is the agent's chat.
- `a` add message (`tab` picks kind), `t` run a turn, `u` start a subagent (`Name task`), `s` sync and build, `b` build.
  `s`, `b`, `t` run in a thread; the header shows progress. The header also shows whether `BRIE_SUMMARIZER` and `BRIE_AGENT` are set.
- `j` `k` move, `g` `G` first and last, `J` `K` scroll preview, `r` reload, `esc` or `m` close.

## Status for agents

`brie status` is the short call: one line of counts (`3 todo · 2 working · 1 blocked · 94 done`), then up to 20 open briefs, newest first.
`--all` adds done briefs, `--status working` lists one status, `--limit n` changes the cap, `--json` gives structured output.
The MCP tool `brief_status` takes the same options.

## Memory

`brie memory` keeps a project's whole history in a view of constant size: every message is logged,
then a tree of one-line summaries covers the log, fine near the end and coarse far back.
It lives in `<store>/<project>/memory/`.

**Off by default.** Turn on with `brie add --memory` in the repo. `--no-memory` turns it off and keeps the log.
While off: no memory line in the instruction block, `brie memory` and `memory_*` MCP tools refuse, `brie wait` skips sync,
and `m` says it is off. A project with a memory folder from before this setting counts as on until you pass `--no-memory`.

Files:

- `main/DAY.jsonl`: every message, word for word, append only. Kinds: `user`, `agent`, `tool`, `echo`, `work`, `note`.
  Long text splits over several messages. A tool result keeps its head and tail (30,000 characters).
- `tree/DAY.jsonl`: a binary tree of one-line summaries (512 bytes max). Each message is a line; adjacent lines merge into one, and so on.
  Each node is built once, up to 8 at a time. The first call of each batch goes alone, so it writes the shared prompt prefix to the cache and the other seven read it.
- `view.json`: two views of the tree that cover the whole log, fine near the end and coarse far back.
  The chat view is 64 to 128 KB (past 128 KB, one batch merges back to 64 KB). The compaction view is 16 to 32 KB and is the summarizer's context.
   Merge order picks the most due pair first (how long ago it ended, in its own line size), checked over 20,000 steps.
- `inbox/`: messages waiting for the process that owns the log. `agents/<Name>/`: a subagent's task and chat.

```
brie memory sync              # log what briefs gained since last time, then build the tree
brie memory add note "text"   # log a message (stdin if no text)
brie memory view              # print the view: <chat> id+n|summary ... </chat>
brie memory zoom 40 8         # open line 40+8 into its two lines; zoom 40 1 shows message 40 whole
brie memory since 41          # messages after 41, whole
brie memory build | status | date <id>
brie memory turn "text"       # one turn: view and message go to $BRIE_AGENT
brie memory sub Scout "task"  # background subagent
```

- **sync** reads only what briefs gained since the last run. A brief is a `user` message. Text after its `## Report`, and each `Status:` change, is a `work` message.
- **Summaries.** Set `BRIE_SUMMARIZER` to a shell command that reads a prompt on stdin and prints one line, such as
  `claude -p --model claude-haiku-5-5`. Without it, long lines are cut, not summarized. Set it before the first `build`: a node is built once.
  A line over 512 bytes is asked again, up to 5 times; the shortest wins.
- **Turns.** Set `BRIE_AGENT` to a shell command that reads a prompt on stdin and prints its work, such as
  `claude -p --output-format stream-json --verbose`. A turn builds the tree, takes the view, logs your message, and runs the agent in the repo
  on `[instructions] [view] [your message]`. With `stream-json`, replies, tool calls and results are logged live; thoughts are not.
  Other commands are logged as one reply. `brie memory add user ...` during a turn is queued; the agent sees it with `brie memory since`.
  The agent reads memory with `zoom`, `date`, `since`, by shell or MCP (`memory_view`, `memory_zoom`, `memory_date`, `memory_since`, `memory_status`).
- **Subagents.** `brie memory sub Scout "task"` starts a fresh agent in the background. Its first message is the view, then the task.
  Its steps stay in `agents/Scout/chat.log`. Its final reply returns as one `work` message, `[Scout] report: ...`. MCP: `memory_add`, `memory_sub`.

One process writes at a time. The others queue their messages.

**Prompt cache.** Nearly every token stays a cache read (98.6% of a turn's prefix in its replay):
the view is saved in `view.json`, never rebuilt from the log; between batches it only grows at its end; merges happen in one batch
from 128 KB down to 64 KB; and per-turn state (the date) goes after the view, not in the prompt head.
The cache marks themselves (blocks of 4 lines) and a system prompt shared by turns and compactions belong to the API client, and brie
runs the model through your shell commands, so those are up to the command you set.

## Prototype servers

`brie serve <dir>` picks the first that applies:

1. `--cmd '<command>'`: any command, run in the prototype directory with `$PORT` (free port from 4100), `$BRIE_DIR`, `$BRIE_REPO`.
   `{port}` in the text is replaced too. `--no-port` is for commands that do not listen (worker, script, native app): pid and log, no URL.
2. `brie.run` in the directory, after a person approves it (below).
3. `vite.config.*` in the directory: runs under the repo's own Vite for live reload. Falls back to static if Vite will not start. `--vite` forces it and fails instead.
4. Built-in static server on 127.0.0.1. `--static` forces it. It renders `.md` as pages (`?raw` for source), serves `index.html` for paths without an extension (SPA), and answers byte ranges.

`brie serve start <dir>` runs detached and outlives the shell. TUI-started servers appear in `brie serve ls`, the MCP tools and the modal, and survive TUI quit.
`brie serve logs <port|pid:N> [-f]` shows output (kept in `logs/` in the store).
`brie serve stop <port|pid:N|all>` stops a server and its children. `brie serve ls --json` is for scripts.

TUI `v` opens the servers modal: port, kind, directory. `x` kills the selected, `X` kills all, `enter` opens in the browser, `l` shows the log.
Quitting the TUI with servers running says so.

### brie.run

```
run: uvicorn app:app --port $PORT     # one plain line is the command, or use keys
port: no                              # optional: the command does not listen
```

A `brie.run` runs code an agent may have written, so a person must approve it first. The CLI asks `[y/N]` on the terminal; the TUI asks in the footer.
With no terminal (agent, pipe) the answer is no, and `brie serve approve <dir>` also needs a terminal, so an agent cannot approve its own file.
Approval is tied to path and content: edit the file and it asks again. Approvals are stored in `approved` in the store.
`--cmd` runs at once, because whoever typed it could run it in a shell anyway.

### Files that are not web pages

`brie open <path>` opens a PDF, image, design file or audio in the system viewer. In the TUI, `enter` on such a file does the same.
`enter` on `.html`, `.md` or a folder serves it.

## MCP

`brie mcp` speaks MCP over stdio. It is opt-in.

```json
{ "mcpServers": { "brie": { "command": "brie", "args": ["mcp"] } } }
```

- **Worker** (`"args": ["mcp", "--worker"]`), 11 tools: `brief_status`, `brief_read`, `brief_set_status`, `brief_report`, `scratch_path`, `contract`,
  and read-only `memory_view`, `memory_zoom`, `memory_date`, `memory_since`, `memory_status`.
- **Orchestrator** (no flag), 18 tools: the worker set plus `brief_new`, `brief_wait` (returns after `timeout_secs`, max 300, with `finished: false` if not ended),
  `servers`, `server_stop`, `projects`, `memory_add`, `memory_sub`.
- Tools enforce the contract: `brief_set_status` sets only `Status:`, `brief_report` only appends, and every tool refuses a file outside the project's briefs folder.
- Memory tools are hidden for a repo with memory off.

Cost: one idle process per client session (a few MB, no CPU). The real cost is tool definitions in context: about 4.9 KB for 18 tools, 2.6 KB for the worker set.
The CLI costs nothing until used, so it stays the default. Pick MCP for structured results and no risk of launching the TUI.

## Token use

- The instruction block is about 600 bytes and points to `brie contract worker|orchestrator` for rules. Run `brie add` again to replace an older, longer block.
- `brie read <brief> --section Report` returns one part: a heading, a comma list, or `top` (lines before the first heading). `--head n` and `--tail n` cut by lines.
  A Report check is about 200 bytes, not thousands. MCP `brief_read` takes `section`, `head`, `tail`.
- `brie status` ends with `state <hash>`. Pass it back with `--if-changed <hash>` and an unchanged project answers `unchanged`. MCP `brief_status` takes `if_changed`.
- `brie contract` takes a role: `worker` (about 750 bytes), `orchestrator`, `short`, or nothing for all.
- Errors are one line. `brie help <command>` prints just that command.
- `brief_set_status` and `brief_report` each do one thing. There is no generic update tool.

## Templates and rules

`brie template init` writes the default brief template to `<store>/<project>/template.md`. `{{title}}`, `{{repo}}` and `{{stem}}` are filled in.
Keep it free of `## ` sections: `brie new` and `brie doctor` warn if it has any.
`rules.md` in the same folder is printed at the end of `brie contract`.

## Push notifications

```
brie notify set ntfy:<topic>      # or ntfy:<https url>, or url:<https webhook>
brie notify test | show | off
```

Sends a message when `brie wait --notify` ends and when the TUI sees a brief turn `done` or `blocked`. The message holds project, brief name and status, never brief text.
The target is stored in `notify` in the store and never backed up or synced, because an ntfy topic works like a password. Needs `curl`.

## Menu bar (macOS)

`brie daemon` puts an icon in the menu bar. Beside it, an amber pill shows the working count and a red pill the blocked count (none when 0).
The **Pills in menu bar** switch changes this to a dot on the icon (red if blocked, else amber if working) plus the working count as text. The choice is saved in `barstyle` in the store.
A click opens a panel with:

- search, and a row per project that expands to its open briefs (click to reveal in Finder, or copy the path)
- Recently done (last 3), Refresh, Open store
- Send test push (shown when a target is set; never the target itself)
- Pills in menu bar switch
- Start at login switch (a launchd agent), Quit

In a terminal, plain `brie daemon` starts it in the background and asks: 1 quit, 2 keep running (Enter keeps it).
Push on and off stays in the CLI, because `off` deletes the secret target.

## Sync between machines

```
brie sync --remote <git url>    # first time
brie sync                       # after that
```

`brie sync` commits briefs and templates, runs `git pull --rebase`, and pushes. `projects`, `state`, `servers`, `notify` and every `prototypes/` folder stay local.
If two machines edited the same lines, the rebase is undone, nothing changes, and the files are named. Briefs only grow at the bottom, so this is rare.

New machine: `git clone <url> ~/.alter-store`, then `brie add` in each repo. Names must match. `brie backup` commits without a remote.
Syncthing also works; skip `*/prototypes/`.

## Site

`docs/` is the GitHub Pages site: plain HTML and CSS, no build. Publish Pages from `main`, `/docs`.
The two screenshots are real renders of brie's TUI from a demo project, converted cell by cell to HTML.

## Build

```
brew install rustup && rustup default stable
cargo build --release
```

The release profile is tuned for size (`opt-level = "z"`, LTO, `strip`, `panic = "abort"`).

## License

MIT. See `LICENSE`.
