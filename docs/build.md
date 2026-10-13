# Build and release

## From source

```
brew install rustup && rustup default stable
cargo build --release
```

The release profile is tuned for size: `opt-level = "z"`, LTO, `strip` and `panic = "abort"`.
The binary lands in `target/release/brie`.

## From Releases

Download one of these files from Releases:

- `brie-macos-arm64.tar.gz` for Apple silicon,
- `brie-linux-x86_64.tar.gz` for Linux x86_64.

```
tar -xzf brie-macos-arm64.tar.gz
mkdir -p ~/.local/bin && mv brie ~/.local/bin/
```

The release workflow builds both files when you push a tag that starts with `v`.

## Release

Each commit bumps the minor version in `Cargo.toml`, and `Cargo.lock` follows it. Commit both with
the change.

To publish a release, push `main`, then tag the release commit and push the tag. The tag is the
version from `Cargo.toml` with a leading `v`, for example `v0.3.0`:

```
git push origin main
git tag v0.3.0
git push origin v0.3.0
```

`v0.1.0` is a lightweight tag, so match it with a lightweight tag.

## The site

`docs/` is the GitHub Pages site: plain HTML and CSS, and no build. Publish Pages from `main`, and
from the `/docs` folder.

The figures in `docs/index.html` are real renders of the brie TUI on a demo project, converted cell
by cell to HTML. Regenerate them with the ignored tests:

```
BRIE_SHOT_OUT=/tmp/shot.html cargo test --release -- --ignored --exact tui::tests::docs_screenshot
BRIE_SHOT_OUT=/tmp/mem.html cargo test --release -- --ignored --exact memwin::tests::memwin_docs_screenshot
```

- `BRIE_SHOT_TAB=view|outline|agents|threads` picks the tab of the memory window.
- `BRIE_SHOT_BIG=1` prepends 1500 long messages, so the tree gets coarse lines.
- A `--ignored` filter without `--exact` matches both tests, and the second run overwrites the output
  of the first.

Splice each figure into `docs/index.html` as the inner HTML of a `<pre>`, with a leading and a
trailing newline. Then compare it with a fresh render. The demo clock dates the two memory figures,
so normalize the times before you compare them. The TUI figure is byte-identical.
