# Contributing

## Building and checking a change

Install Rust with rustup, which picks up the toolchain pinned in
`rust-toolchain.toml`. The build compiles SQLite, so it also needs a C
compiler. Before opening a pull request, run what CI runs:

```sh
cargo fmt
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

Report and JSON output is tested with [insta](https://insta.rs/) snapshots.
When a change alters output on purpose, review and accept the new snapshots
with `cargo insta review` (`cargo install --locked cargo-insta`).

## Rules

- No estimation factors in code. Every factor comes from the files in
  `data/` with its source, and a test fails for a value without one; `data/README.md`
  describes the format.
- No network access and no telemetry: tokendraw reads private session logs.
- Test fixtures are synthetic logs, never real ones, because real logs
  contain private conversations.
- `--json` output only grows: removing or changing a field is a breaking
  change.
- Messages are lowercase without a trailing period, state facts and end with
  what to do. They name a command, flag or file in backticks, such as
  ``run `tokendraw projects` ``.
- A comment says only why the code is this way or what to know before
  changing it. Code that reads clearly gets none. No `TODO` comments; open
  work goes in issues.
- Test names are sentences that describe behaviour.
- A new dependency gets a comment in `Cargo.toml` saying why it is there.

## Where things go

- Support for another agent: most agents can be read with an adapter file
  or command ([docs/adapters.md](docs/adapters.md)), without a change to
  tokendraw. A shipped adapter implements the `Adapter` trait in
  `src/adapters/mod.rs`.
- New or corrected data: the files in `data/`, with their sources
  ([data/README.md](data/README.md)).

## Pull requests

Pull requests are squash-merged, so each becomes one commit; the subject is
a plain sentence in the imperative, such as "Read forked Codex sessions".
Maintainers write `CHANGELOG.md` for each release.
