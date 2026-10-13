# Prototype servers

`brie serve <dir>` runs a prototype and prints its URL. The command picks the first rule that applies:

1. `--cmd '<command>'` runs any command in the prototype directory. The command gets `$PORT`
   (a free port from 4100), `$BRIE_DIR` and `$BRIE_REPO`. The text `{port}` is replaced too.
   `--no-port` is for a command that does not listen, such as a worker, a script or a native app:
   brie keeps the pid and the log, and prints no URL.
2. A `brie.run` file in the directory, after a person approves it.
3. A `vite.config.*` in the directory runs under the Vite of the repo, for live reload. brie falls
   back to the static server when Vite does not start. `--vite` forces Vite and fails instead.
4. The built-in static server on `127.0.0.1`. `--static` forces it. It renders a `.md` file as a page
   (`?raw` gives the source), serves `index.html` for a path with no extension (SPA), and answers
   byte ranges.

## Servers that outlive the shell

`brie serve start <dir>` runs detached, so the server outlives the shell. A server that the TUI starts
appears in `brie serve ls`, in the MCP tools and in the modal, and it survives a TUI quit.

`brie serve logs <port|pid:N> [-f]` shows the output, kept in `logs/` in the store.
`brie serve stop <port|pid:N|all>` stops a server and its children. `brie serve ls --json` is for scripts.

The TUI `v` opens the servers modal with the port, the kind and the directory of each server.
`x` kills the selected server, `X` kills all of them, `enter` opens one in the browser, and `l` shows
the log. Quitting the TUI with servers running says so.

## `brie.run`

```
run: uvicorn app:app --port $PORT     # one plain line is the command, or use keys
port: no                              # optional: the command does not listen
```

A `brie.run` runs code that an agent wrote, so a person must approve it first. The CLI asks `[y/N]`
on the terminal, and the TUI asks in the footer.

With no terminal (an agent, or a pipe) the answer is no. `brie serve approve <dir>` also needs a
terminal, so an agent cannot approve its own file. Approval is tied to the path and the content, so an
edit to the file asks again. The approvals are in `approved` in the store.

`--cmd` runs at once, because whoever typed it can run it in a shell anyway.

## Files that are not web pages

`brie open <path>` opens a PDF, an image, a design file or audio in the system viewer. In the TUI,
`enter` on such a file does the same. `enter` on an `.html` file, an `.md` file or a folder serves it.

## Commands

```
brie serve <dir> [--cmd '...' [--no-port]] [--vite|--static]
brie serve start <dir> ...            the same, detached
brie serve ls [--json]                the servers that brie serve started
brie serve logs <port|pid:N> [-f]     the output of a server
brie serve stop <port|pid:N|all>      stop servers and everything they started
brie serve approve <dir>              approve the brie.run in a directory (needs a terminal)
brie open <path>                      open a file in the system viewer
```

To serve a prototype of a brief:

```
d="$(brie path prototypes)/<brief name without .md>"
mkdir -p "$d" && brie serve "$d"
```
