# annodiff

A terminal UI for reviewing Git diffs, adding comments, and sending reviews to Codex. Built with Rust and Ratatui.

## Install

Requires Rust 1.88+, Git, and a UTF-8 terminal. Install from crates.io:

```sh
cargo install annodiff --locked
annodiff /path/to/repository
```

Omit the path to review the current directory.

Sending reviews requires an authenticated `codex` CLI. Browsing and saving comments work without it.

Reviews are sent directly as messages; large reviews use a temporary Markdown file.

## Build from source

```sh
git clone https://github.com/MelsRoughSketch/annodiff.git
cd annodiff
cargo build --release --locked
./target/release/annodiff
```

## Usage

Browse files and commits, select lines, and add comments. Preview reviews before sending or copying them. Press `?` for key bindings.

The Files pane groups files in a directory tree, combining chains of directories into paths such as `aaa/bbb/ccc/` when each intermediate directory has only one child directory and no changed files directly inside it. Use Up/Down or j/k to navigate and `-` to collapse and `=` to expand directories (Enter or Space toggles them). Clicking a directory selects it without changing its expansion state. Use `+` / `_` to expand/shrink panes. Path search (`/`) and comment filters (`o`, `u`) reveal matching files inside collapsed directories; clearing the filters restores your folds.

- [Configuration](config.example.toml)
- [Benchmarks](benchmarks/README.md)

## Releasing

After merging a version update, publish a stable GitHub Release with a matching tag (for example, `v0.2.0`). Promoting a pre-release to a stable release also starts the crates.io publish workflow.

To retry an existing stable release, open **Actions → Publish to crates.io → Run workflow**, select **main**, and enter its tag. The workflow validates the release, runs CI against the tag's exact commit, and publishes that same commit. Already-published crate versions are skipped. The `crates-io` environment must allow the `main` branch for manual retries, alongside its `v*` tag rule.

## License

Licensed under either [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
Dependencies retain their own licenses; see [third-party notices](THIRD_PARTY_NOTICES.md).

Run `annodiff --licenses` to display the included license texts and attributions.
