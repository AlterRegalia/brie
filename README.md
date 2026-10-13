<p align="center"><img src="assets/logo.svg" alt="brie" width="136"></p>

# brie

Keep project briefs and prototypes on your machine, outside the repo. Browse them in a TUI.

One brief is one Markdown file for one task. An Orchestrator writes it, a worker agent does it, and
`Status` tells you where it stands. The CLI, the terminal UI and the MCP server read the same files.

## Install

Download `brie-macos-arm64.tar.gz` (Apple silicon) or `brie-linux-x86_64.tar.gz` (Linux x86_64) from
Releases:

```
tar -xzf brie-macos-arm64.tar.gz
mkdir -p ~/.local/bin && mv brie ~/.local/bin/
```

Or build from source: see [Build and release](docs/build.md).

## Quick start

```
cd your-repo
brie add          # onboard: move .briefs/ and .prototype/ into the store, leave symlinks
brie              # open the TUI
```

`brie add` is safe to repeat. It changes nothing when the links are right.

## Common commands

```
brie                        open the TUI
brie add [path]             onboard a repo
brie new "<slug>"           create a brief, and print its path
brie status [brief]         the counts, then the open briefs
brie set <brief> <status>   todo | working | done | blocked
brie report <brief> [text]  append under ## Report
brie wait <brief>...        exit 0 on done, 2 on blocked, 1 on timeout
brie serve <dir>            run a prototype
brie mcp [--worker]         the MCP server over stdio
brie help <command>         the help for one command
```

Without `-p`, the project is the onboarded repo that holds the current directory.
A bare `brie` opens the TUI and blocks.

## Documentation

| Document | Contents |
| --- | --- |
| [Briefs](docs/briefs.md) | The life cycle of a brief, the statuses, the seal, the report. |
| [The store](docs/store.md) | The store layout, `brie add`, the instruction block, worktrees. |
| [The terminal UI](docs/tui.md) | The keys, the panes, the layout, the editor, `brie view`. |
| [Memory](docs/memory.md) | The log, the summary tree, the views, the turns, the memory window. |
| [Prototype servers](docs/serve.md) | `brie serve`, `brie.run` and its approval, non-web files. |
| [For agents](docs/agents.md) | The recipe for an agent, the rules, and the token cost. |
| [MCP](docs/mcp.md) | The tool sets of the MCP server and what they cost. |
| [Menu bar and notifications](docs/daemon.md) | The macOS menu bar panel and the push target. |
| [Backup and sync](docs/sync.md) | `brie backup`, `brie sync`, and what stays local. |
| [Build and release](docs/build.md) | Build, install, release, and the site captures. |

## License

MIT. See `LICENSE`.
