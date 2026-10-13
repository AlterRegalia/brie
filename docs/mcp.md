# MCP

`brie mcp` speaks MCP over stdio. It is opt-in.

```json
{ "mcpServers": { "brie": { "command": "brie", "args": ["mcp"] } } }
```

## Tool sets

- **Worker** (`"args": ["mcp", "--worker"]`), 12 tools: `brief_status`, `brief_read`,
  `brief_set_status`, `brief_report`, `scratch_path`, `contract`, and the read-only `memory_view`,
  `memory_find`, `memory_zoom`, `memory_date`, `memory_since`, `memory_status`.
- **Orchestrator** (no flag), 19 tools: the worker set plus `brief_new`, `brief_wait`, `servers`,
  `server_stop`, `projects`, `memory_add` and `memory_sub`.

`brief_wait` returns after `timeout_secs`, 300 at most, with `finished: false` when the brief has not
ended.

The tools enforce the contract. `brief_set_status` sets only the `Status` line, `brief_report` only
appends, and every tool refuses a file outside the briefs folder of the project.

The memory tools are hidden for a repo with the memory off. That leaves 11 tools for the Orchestrator
and 6 for the worker.

## Cost

Each client session holds one idle process: a few MB, and no CPU.

The real cost is the tool definitions in context: about 5.8 KB for the 19 tools, and 3.6 KB for the
worker set, measured as the compact JSON that `tools/list` sends.

The CLI costs nothing until you use it, so it stays the default. Pick MCP for structured results and
for no risk of launching the TUI. See [For agents](agents.md) for the rest of the agent surface.
