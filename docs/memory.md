# Memory

The memory keeps a project's whole history in a view of constant size. Every message is logged.
Then a tree of one-line summaries covers the log: fine near the end, coarse far back.

The memory lives in `<store>/<project>/memory/`.

## Off by default

Turn the memory on with `brie add --memory` in the repo. `brie add --no-memory` turns it off again
and keeps the log. A project with a `memory/` folder from before this setting counts as on, until you
pass `--no-memory`.

While the memory is off:

- the instruction block holds no memory line,
- `brie memory` and the `memory_*` MCP tools refuse,
- `brie wait` skips the sync,
- `m` in the TUI says that the memory is off.

## Files

- `main/DAY.jsonl` holds every message, word for word, append only. The kinds are `user`, `agent`,
  `tool`, `echo`, `work` and `note`. Long text splits over several messages. A tool result keeps its
  head and its tail, 30,000 characters in all.
- `tree/DAY.jsonl` holds a binary tree of one-line summaries, 512 bytes at most. Each message is a
  line. Adjacent lines merge into one, and so on. The memory builds each line once, up to 8 at a
  time. The first call of each batch goes alone, so it writes the shared prompt prefix to the cache
  and the other seven read it.
- `view.json` holds two views of the tree that cover the whole log, fine near the end and coarse far
  back. The chat view is 64 KB to 128 KB, and past 128 KB one batch merges it back to 64 KB. The
  compaction view is 16 KB to 32 KB, and it is the context of the summarizer. The merge order takes
  the most due pair first: how long ago it ended, in its own line size, checked over 20,000 steps.
- `inbox/` holds the messages that wait for the process that owns the log.
- `agents/<Name>/` holds a subagent's task and chat.

## Commands

```
brie memory sync              log what the briefs gained since last time, then build the tree
brie memory add <kind> [text] log a message (stdin if no text)
brie memory view              print the view: <chat> id+n|summary ... </chat>
brie memory find <words>      where the words appear: the messages, and the lines over them as id+n
brie memory zoom <id> <n>     open line id+n into its two lines, or id 1 into the whole message
brie memory zoom <Name>       a subagent's whole chat
brie memory since <id>        the messages after id, whole
brie memory date <id>         the date and time of message id
brie memory build             build the tree and merge the views down
brie memory status            the state of the memory
brie memory turn <text>       one turn: the view and the message go to $BRIE_AGENT
brie memory sub <Name> <task> start a subagent in the background
```

The MCP tools are `memory_view`, `memory_find`, `memory_zoom`, `memory_date`, `memory_since`,
`memory_status`, `memory_add` and `memory_sub`. See [MCP](mcp.md).

## Sync

`brie memory sync` reads only what the briefs gained since the last run. A brief is a `user` message.
The text after its `## Report`, and each change of its `Status:` line, is a `work` message.

## Summaries

Set `BRIE_SUMMARIZER` to a shell command that reads a prompt on stdin and prints one line, for example
`claude -p --model claude-haiku-5-5`. Without it, the memory cuts a long line instead of summarizing it.

Set the command before the first `build`, because the memory builds each line once. A line over
512 bytes is asked again, up to 5 times, and the shortest answer wins.

## Turns

Set `BRIE_AGENT` to a shell command that reads a prompt on stdin and prints its work, for example
`claude -p --output-format stream-json --verbose`.

A turn builds the tree, takes the view, logs your message, and runs the agent in the repo on
`[instructions] [view] [your message]`. With `stream-json`, the replies, the tool calls and the tool
results are logged live. The memory does not log thoughts. Any other command is logged as one reply.

A `brie memory add user <text>` during a turn is queued, and the turn files it. The agent sees the
message with `brie memory since <id>`. The agent reads the memory with `find`, `zoom`, `date` and
`since`, by shell or by MCP.

## Finding

`memory_view` is the way in by topic, but it is not enough. A summary line holds 512 bytes, and it
often does not hold the words a reader searches for: on a real project, 137 of the 325 files the log
named were in no line at all.

`brie memory find <words>` searches the lines and the messages together. It answers with coordinates:
the lines that hold the words, reduced to the coarsest ones over the hits, each as `id+n` for `zoom`.
It also gives the messages that hold the words, each as an id for `zoom <id> 1`.

The options are `--limit n`, `--log`, `--lines`, `--exact` and `--full`. The command needs no index,
because it reads the two folders that are already there.

## Subagents

`brie memory sub Scout "task"` starts a fresh agent in the background. Its first message is the view,
and its second is the task. Its steps stay in `agents/Scout/chat.log`. Its final reply returns to the
chat as one `work` message, `[Scout] report: ...`.

## One writer

One process writes to the memory at a time. The other processes queue their messages.

Nearly every token of a turn stays a cache read: 98.6% of the prefix of a turn in its replay. The
view is saved in `view.json`, and the memory never builds it again from the log. Between batches the
view only grows at its end, and the merges happen in one batch, from 128 KB down to 64 KB. The
per-turn state, the date, goes after the view and not in the prompt head.

The cache marks (blocks of 4 lines) and the system prompt that the turns and the compactions share
belong to the API client. brie runs the model through your shell commands, so those belong to the
command you set.

## The memory window

`m` opens the project's memory full screen. It reads without the lock, so it works during a turn, and
it refreshes every second. The window has four tabs.

### VIEW (`1`)

One `id+n  summary` line for each line of the tree, oldest first, with the newest selected.
`enter`, `l` or `→` zoom into a line, that is, into its two source lines. `backspace`, `h` or `←` go
back. A message cannot be zoomed.

Each line starts with a glyph and a color for its kind: `▌` user, `◆` agent, `⚙` tool, `⎘` echo,
`◐` work, `✎` note. Each kind has its own glyph and its own color (amber, white, violet, grey, blue,
green), so the tab reads without color too. A terminal 110 columns wide or more shows the key in the
header.

Each line also has a bar for how much of the biggest line in the list it covers. A row above the list
shows the way down: `view › 0+1024 › 0+16`.

The preview shows a message as it was said: kind, date, words. A line that covers 8 messages or fewer
lists them below its summary, as a conversation. A summary also names its parent (the line it merges
into) and its sibling.

### OUTLINE (`2`)

The same lines as VIEW, and each line opens in place into the two that made it. Press `enter` or `→`
to open, and `←` or `backspace` to close. When you close a line that is already shut, the cursor goes
back up to the line it merges into.

The preview puts the line above the two it stands for. Thus you can read what a compression kept and
what it dropped. A line with no summary yet does not open, and the preview says so.

### AGENTS (`3`)

The subagents, `◐` running and `●` done. The preview is the chat of the agent.

### THREADS (`4`)

THREADS shows the briefs the log names, each as the window it was worked in.

A thread is a brief's own name, `DD_MM_YYYY_HH:MM_slug`, wherever a message spells it: bare, in
brackets, or inside a `.briefs/…md` path. A code fragment in brackets, such as `[N-2]`, is not a
brief and is not a thread.

One row for each brief: `├` at its first mention, `┤` at its last, and `─` between them. A brief that
is named once is a single `┼`. A tick marks a mention between the ends, in the color of that message's
kind. The ends carry the same color. A tick that collides with another is dropped, so read the window,
not the ticks.

The rows are widest window first, and the top row is a day ruler. The bottom border counts the briefs,
the mentions, and how many briefs were open at the busiest column.

`enter` narrows the timeline to the window of the selected brief. That is the only way to see a brief
that lasted one column of the whole log. `backspace` returns to the whole log.

The preview lists the mentions of the brief, each with the view line that the log kept over it. That
pairing is what a word search cannot give.

### Keys

- `a` adds a message (`tab` picks the kind), `t` runs a turn, `u` starts a subagent (`Name task`),
  `s` syncs and builds, and `b` builds.
- `s`, `b` and `t` run in a thread, and the header shows the progress. The header also shows whether
  `BRIE_SUMMARIZER` and `BRIE_AGENT` are set.
- `1` `2` `3` `4` pick a tab, `tab` cycles, `j` `k` move, and `g` `G` go to the first and the last.
- `J` `K` scroll the preview, `r` reloads, and `esc` or `m` closes.
- The footer names the keys of the tab you are on. It is longer than a hundred columns, so a narrow
  terminal shows the first keys and cuts the rest.
