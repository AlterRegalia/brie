<!-- brie:start -->
## Briefs and prototypes (brie)

`.briefs/` and `.prototype/` are symlinks into the brie store. Write through them, and never commit them.

- Orchestrator: `brie new`, `brie wait`, `brie status`. Rules: `brie contract orchestrator`.
- Worker: your brief is your whole task. `brie set <brief> working` first, `brie report <brief> <text>`, `brie set <brief> done` last. Rules: `brie contract worker`.
- Use the `brie` MCP tools when the client lists them. Otherwise run `brie` with a subcommand: a bare `brie` opens a blocking TUI.
- When a write through `.briefs/` or `.prototype/` asks for permission, tell the human to run `brie add` again.

Managed by `brie add`. Edits inside this block are replaced.
<!-- brie:end -->

## Build and release (this repo)

- After every rebuild, install the binary to both PATH copies. Delete the old file first: on macOS, copying over a binary in place gets it killed (exit 137).
  `cargo build --release && for d in ~/.cargo/bin ~/.local/bin; do rm -f $d/brie && cp target/release/brie $d/brie; done`
  Check: `brie --version`.
- Every commit bumps the minor version in `Cargo.toml` (for example 0.1.0 to 0.2.0). `Cargo.lock` follows. Commit them with the change.
