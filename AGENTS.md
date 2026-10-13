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
- CI runs `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo build` and `cargo test` on **Linux only**. A macOS-only clippy never sees the code behind `#[cfg(target_os = "macos")]`, so it reports clean while CI fails. Check the Linux target before every push: `rustup target add x86_64-unknown-linux-gnu && cargo clippy --target x86_64-unknown-linux-gnu --all-targets -- -D warnings`.
- CI has no terminal, so `COLORTERM` is unset there and `theme::get()` picks the 256-color palette. Run the suite that way too: `env -u COLORTERM cargo test`. A truecolor terminal hides every test that reads a color.
