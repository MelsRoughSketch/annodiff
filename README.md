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

- [Configuration](config.example.toml)
- [Benchmarks](benchmarks/README.md)

## License

[MIT](LICENSE) · [Third-party notices](THIRD_PARTY_NOTICES.md)

Run `annodiff --licenses` to display the included license texts and attributions.
