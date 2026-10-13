# Menu bar and notifications

## Menu bar (macOS)

`brie daemon` puts an icon in the menu bar. Beside the icon, an amber pill shows the working count and
a red pill shows the blocked count. A count of 0 shows no pill.

The **Pills in menu bar** switch changes this to a dot on the icon, plus the working count as text.
The dot is red when a brief is blocked, and amber when one is working. brie saves the choice in
`barstyle` in the store.

A click opens a panel with these items:

- A search box, and one row for each project that expands to its open briefs. A click opens the brief
  in a window of its own, which the brie Markdown renderer draws. A button on the row opens a Terminal
  on the file, another reveals it in Finder, and another copies its path.
- Recently done (the last 3), Refresh, and Open store.
- Send test push. It shows when a target is set, and never the target itself.
- The Pills in menu bar switch.
- The Start at login switch (a launchd agent), and Quit.

In a terminal, a plain `brie daemon` starts the daemon in the background and then asks: 1 quit it,
2 keep it running. Enter keeps it.

Turn push on and off in the CLI, because `off` deletes the secret target.

```
brie daemon [start|stop|status]
```

## Push notifications

```
brie notify set ntfy:<topic>      # or ntfy:<https url>, or url:<https webhook>
brie notify test | show | off
```

brie sends a message when `brie wait --notify` ends, and when the TUI sees a brief turn `done` or
`blocked`. The message holds the project, the brief name and the status, and never brief text.

The target is in `notify` in the store. brie never backs it up and never syncs it, because an ntfy
topic works like a password. The command needs `curl`.
