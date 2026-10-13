# Briefs

A brief is the whole task for one worker: one Markdown file in the store, outside the repo.

The Orchestrator writes the brief. The worker reads it, does the work, and reports in it.
The Status line is the only signal the Orchestrator trusts.

## Life cycle

1. The Orchestrator runs `brie new "<slug>"`, and the command prints the path.
   The new brief holds a title, a `Status` line, a `Repo` line and a `Scratch` line, and no sections.
2. The Orchestrator appends `## Read`, `## Do` and `## Do not` below the header, once.
3. The worker runs `brie set <brief> working` first, then does the work, then runs `brie report`,
   then runs `brie set <brief> done`.
4. The Orchestrator runs `brie wait <brief>`, compares the Report with the real diff, and accepts the work.

A brief that stays on `todo` long after the spawn means the worker dropped the prompt.

## Status

| Status | Meaning |
| --- | --- |
| `todo` | The Orchestrator wrote the brief. |
| `working` | The worker set it first. |
| `done` | The worker set it last, after the Report. |
| `blocked` | The worker cannot continue. The Report says why. |

## Append only

A brief grows at the bottom. The `Status` line is the one line that changes in place, and only
`brie set` changes it. Write a correction as a new entry, and the newest entry is the truth.

The first `brie set` that leaves `todo` seals the brief. The seal holds the length and a hash of the
text, and it ignores the `Status` line. The seal lives in `<store>/<project>/seals/`. After that,
`brie set`, `brie report`, `brie wait` and `brie doctor` warn when the sealed text changes.
A drifted brief is not sealed again.

## Write a brief

- Write one brief for each task. Keep it under one screen.
- Append the sections below the header that `brie new` made: `## Read`, `## Do`, `## Do not`.
  Lines already written stay as they are, and a new task goes in a new file.
- Write one imperative instruction for each numbered step. Put the condition before the command.
- Under `## Read`, name exact paths and say why each one matters. Link an issue instead of restating it.
- Put hard limits under `## Do not`. A worker obeys a listed limit and ignores a hinted one.
- Make step 1 of `## Do` this: `brie set <brief> working`, before the worker reads anything.

## Report

`brie report <brief> <text>` appends the text under one `## Report` heading at the bottom.
With no text, the command reads stdin.

The worker reports with `Files`, `Commands run`, `Decisions` and `Open`. One line for each item,
and no preamble.

## Commands

| Command | Result |
| --- | --- |
| `brie new "<slug>" [-p project]` | Create a brief and its Scratch directory, and print the path. The slug is lowercase words joined by hyphens, three words at most. |
| `brie set <brief> <status>` | Set the Status line: `todo`, `working`, `done` or `blocked`. |
| `brie report <brief> [text]` | Append to the bottom of a brief, under one `## Report` heading. |
| `brie read <brief> [--section a,b] [--head n] [--tail n]` | Print part of a brief: a heading such as `Report` or `Do`, or `top` for the lines before the first heading. `--head` and `--tail` cut by lines. |
| `brie status [brief] [--all] [--status s] [--limit n] [--json] [--if-changed hash]` | With no brief: the counts by status, then the briefs that are not done, newest first, 20 at most. With one brief: its status. |
| `brie wait <brief>... [--any] [--notify] [-t secs]` | Block until the briefs end. Exit 0 for `done`, 2 for `blocked`, 1 for a timeout. |
| `brie rm <brief>` | Move the brief and its Scratch directory to the Trash. |
| `brie template [init]` | Show the brief template, or write it to the store for editing. |
| `brie contract [worker\|orchestrator\|short]` | Print the contract, whole or for one role, and the project's `rules.md`. |

A `<brief>` argument is a path, a file name, or part of a name that matches one brief.
Without `-p`, the project is the onboarded repo that holds the current directory.

The name of a brief is `DD_MM_YYYY_HH:MM_<slug>.md`. The name and the file stay for the life of the brief.

## Wait

The Orchestrator runs `brie wait <brief>` in the background. The command prints each Status change,
and it exits 0 on `done`, 2 on `blocked` and 1 on a timeout.

With memory on, `brie wait` runs `brie memory sync` first. A failed sync does not change the exit code.

## Template and rules

`brie template init` writes the default brief template to `<store>/<project>/template.md`.
The command fills in `{{title}}`, `{{repo}}` and `{{stem}}`.

Keep the template free of `## ` sections. `brie new` and `brie doctor` warn when the template holds one.
`rules.md` in the same folder adds project rules to the end of `brie contract`.
