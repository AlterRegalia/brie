# For agents

An agent reads the block that `brie add` wrote into `CLAUDE.md` or `AGENTS.md`. The block names the
two roles and points to `brie contract worker` and `brie contract orchestrator` for the rules.

## Recipe

```
brie ls                                # is this repo onboarded?
brie add                               # no: onboard it from the repo root
f=$(brie new "cup pin server")         # Orchestrator: create a brief, get its path
brie wait cup-pin-server -t 1800 &     # Orchestrator: wait for the worker
brie status                            # Orchestrator: counts and open briefs
d="$(brie path prototypes)/<brief name without .md>"
mkdir -p "$d" && brie serve "$d"       # serve a prototype
```

## Rules

- Write briefs and Scratch output only in `brie path briefs` and `brie path prototypes`.
  Never commit them.
- Never run a bare `brie`. It opens a full-screen TUI and blocks.
- Run `brie serve` and `brie wait` in the background, and stop them when the work ends.
- Pass `-p <project>` outside an onboarded repo.
- When a write through `.briefs/` or `.prototype/` asks for permission, tell the human to run
  `brie add` again. Never edit the permission settings.

## Status is the short call

`brie status` prints one line of counts (`3 todo · 2 working · 1 blocked · 94 done`), then up to 20
open briefs, newest first.

`--all` adds the done briefs. `--status working` lists one status. `--limit n` changes the cap, and
`--json` gives structured output. The MCP tool `brief_status` takes the same options.

## Token use

The agent surface is small on purpose.

- The instruction block is about 750 bytes, and it points to `brie contract worker|orchestrator` for
  the rules. Run `brie add` again to replace an older, longer block.
- `brie read <brief> --section Report` returns one part of a brief: a heading, a comma list, or `top`
  for the lines before the first heading. `--head n` and `--tail n` cut by lines. A Report check is
  hundreds of bytes and not thousands. The MCP `brief_read` tool takes `section`, `head` and `tail`.
- `brie status` ends with `state <hash>`. Pass the hash back with `--if-changed <hash>`, and an
  unchanged project answers `unchanged`. The MCP `brief_status` tool takes `if_changed`.
- `brie contract` takes a role: `worker` (about 1.1 KB), `orchestrator` (about 3 KB), `short`
  (about 400 bytes), or nothing for all of it.
- Every error is one line, and `brie help <command>` prints that command alone.
- `brief_set_status` and `brief_report` each do one thing. There is no generic update tool.
