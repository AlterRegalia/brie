# The store

The store holds every brief and every prototype, outside the repo. It lives in `~/.alter-store` by
default. Set `BRIE_HOME` to use another root.

```
~/.alter-store/<project>/briefs/         the briefs
~/.alter-store/<project>/prototypes/     the Scratch directories of the briefs
~/.alter-store/<project>/template.md     the brief template
~/.alter-store/<project>/rules.md        extra rules for `brie contract`
~/.alter-store/<project>/seals/          the seals of the briefs
~/.alter-store/<project>/memory/         the memory, when it is on
~/.alter-store/<project>/memory.setting  whether the memory is on
~/.alter-store/projects                  the index: name<TAB>repo path, one line per project
~/.alter-store/state                     the saved TUI layout and filters
~/.alter-store/servers                   the running prototype servers
~/.alter-store/logs/                     the output of the prototype servers
~/.alter-store/approved                  the approved `brie.run` files
~/.alter-store/notify                    the push target
```

## Onboard a repo

```
cd your-repo
brie add
```

`brie add` is safe to repeat. It changes nothing when the links and the block are right.
`brie add [path] [--name n] [--no-inject] [--no-allow] [--memory|--no-memory]`.

For each repo, `brie add` does three things:

1. It moves `.briefs/` and `.prototype/` into the store, and leaves symlinks in their place.
2. It writes a block into `CLAUDE.md` and `AGENTS.md`, between `<!-- brie:start -->` and
   `<!-- brie:end -->`. When neither file exists, it creates `AGENTS.md`. A later `brie add`
   replaces the block. Skip this step with `--no-inject`.
3. It adds the store folder to `permissions.additionalDirectories` in
   `.claude/settings.local.json`, so Claude Code does not ask on each write through `.briefs/`
   and `.prototype/`. A file that is not a JSON object is left alone, and the command says why.
   Skip this step with `--no-allow`.

`brie doctor` checks the links, the instruction block and the store. It exits 1 on a problem.

## Memory is off

The memory is off unless you pass `--memory`. `--no-memory` turns it off again and keeps the log.
A project with a `memory/` folder from before this setting counts as on, until you pass `--no-memory`.

## Worktrees

`brie link [worktree]` points a worktree's `.briefs` and `.prototype` at the store, so every
worktree of a repo shares the same briefs.

## Backups

`brie backup` commits the briefs and the templates to a git repo in the store. `brie sync` adds a
remote. See [Sync between machines](sync.md).

## Commands

| Command | Result |
| --- | --- |
| `brie add [path] [...]` | Onboard a repo. The default path is the current directory. |
| `brie ls` | List the projects. |
| `brie path [briefs\|prototypes] [-p project]` | Print the store folder for the briefs or the prototypes. |
| `brie link [worktree]` | Point a worktree's `.briefs` and `.prototype` at the store. |
| `brie doctor` | Check the links, the instruction blocks and the store. |
| `brie backup` | Commit the briefs and the templates to a git repo in the store. |
