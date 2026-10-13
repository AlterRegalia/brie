# The terminal UI

Run `brie` with no subcommand to open the TUI. It reads the store, and it refreshes the list every
second.

A brief that turns `done` or `blocked` rings the bell and shows in the footer.
Badges: `○` todo, `◐` working, `●` done, `✕` blocked.

## Keys

| Key | Action |
| --- | --- |
| `j` `k` / `h` `l` | Move / change pane |
| `tab` | Switch Briefs and Prototypes |
| `enter` | Brief: built-in editor. Prototype: serve and open the browser |
| `e` | Open in `$VISUAL` / `$EDITOR` |
| `n` `x` `a` | New brief / Trash (asks first) / add repo |
| `m` `s` `v` | Memory / serve / servers modal |
| `r` `R` | Reveal in the file manager / reload |
| `J` `K`, PageUp PageDown | Scroll the preview. They do not move the list |
| `1` `2` `3` `4` `5` | Projects, Briefs tab, Prototypes tab, preview, status. Press again to hide a pane (never both projects and items) |
| `?` `q` | Help / quit |

## The items pane

- `/` finds. The terms are ANDed, and each one matches the name or, for a file, its first 64 KB.
  `status:working` matches the status. `enter` keeps the filter, and `esc` clears it.
- `o` sets the order: newest, oldest, A-Z, Z-A.
- `d` sets the modified window: all, 24h, 7d, 30d.
- `f` filters the briefs by status: all, todo, working, done, blocked.
- `esc` with a filter on clears the filters. With no filter it asks to quit.

The bottom edge shows `shown/total`, the sort and the active filters.

## Panes and layout

The layout follows the terminal size and the panes you turned on. Projects and Status share a column
on the left. The column is 22% of the width, 18 to 28 columns. Status sits under Projects when the
column is 14 rows tall or more.

The list and the preview go side by side when the area right of Projects is 100 columns wide. On a
terminal under 20 rows, that width is 80. Otherwise they stack.

A pane under 10 columns or 3 rows is dropped: Status first, then the preview.

The header is two rows. The first row holds the mark, the name and the version. The second row holds
the project, brief and prototype counts and the running servers. The Status pane (`5`) counts the
briefs for each status.

The TUI saves the panes, the sort, the date window, the tab and the project in `state` in the store.

## The mouse

Everything has a click as well as a key. A click selects the project or the list row under it, and a
second click opens it. The tabs are click targets. The footer hints, the servers count in the header,
and the rows of the Status box are click targets (a row filters by status). The buttons of a window
are click targets too. Each row of the `?` help runs its key when you click it. In the editor a click
puts the cursor there, and the footer hints press their keys.

The wheel acts on the pane under it: the list moves its selection and the preview scrolls.
Projects and Status ignore the wheel.

The TUI captures the mouse, in the TUI and in `brie view`. Hold `option` (macOS) or `shift` to select text.

## The editor

`enter` on a brief opens the editor, and `e` opens `$EDITOR`. Both edit only while the brief is `todo`.

After the hand-off, `enter` opens the same window read only (the title is `VIEW`): move, find
(`ctrl-f`), copy, side preview, and it follows the file as a worker reports. `e` refuses, because a
brief only grows at the bottom.

The editor renders Markdown as you type. The cursor line, or each selected line, shows the raw source.

| Key | Action |
| --- | --- |
| `ctrl-s` | Save. If the file changed on disk, the first press warns and the second overwrites |
| `ctrl-r` | Reload from disk (the edits stay on the undo stack) |
| `ctrl-z` | Undo, 200 steps. A run of typing is one step |
| `shift` + arrows, `home`, `end` | Select |
| `ctrl-c` `ctrl-x` `ctrl-v` | Copy, cut (the whole line with no selection), paste. System clipboard |
| `ctrl-f` | Find: type to jump, `enter` for the next, `esc` to close |
| `ctrl-p` | Side pane with the rendered document |
| `esc` | Clear the selection, then close (a second `esc` discards unsaved changes) |

The editor reloads by itself when the file changes on disk and you have no unsaved edits. Otherwise
the title shows `changed on disk`. On save, tabs become two spaces and CRLF becomes LF.

## `brie view`

`brie view <path>` draws a file in the whole terminal with the brie Markdown renderer, read only.
It is the same renderer as the brief window of the menu bar daemon, in the terminal.

The wheel scrolls, and the command captures the mouse. Hold `option` (macOS) or `shift` to select text.

## The memory window

`m` opens the project's memory full screen, with four tabs. See [Memory](memory.md#the-memory-window).
