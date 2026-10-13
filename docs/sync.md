# Backup and sync

`brie backup` commits the briefs and the templates to a git repo inside the store. The command
creates the repo on the first run.

`brie sync` adds a remote to the same repo:

```
brie sync --remote <git url>    # the first time
brie sync                       # after that
```

`brie sync` commits the store, runs `git pull --rebase`, and pushes.

## What stays local

The index (`projects`), the view state (`state`), the server list (`servers`), the push target
(`notify`) and every `prototypes/` folder stay on the machine. Prototypes are big and you can build
them again. The push target is a secret.

## A clash

When two machines edited the same lines, the rebase is undone, nothing changes, and the command names
the files. Briefs only grow at the bottom, so a clash is rare. Fix one side, then sync again.

## A new machine

```
git clone <url> ~/.alter-store
brie add                        # in each repo
```

The project names must match on both machines.

Syncthing also works. Skip `*/prototypes/` there as well.
