# tokendraw

Tokendraw is a command line tool for estimating the energy usage and
emissions of LLM coding agent use.

Tokendraw currently supports Claude Code, Codex, pi, as well as custom
adapters for other tools. It reviews the logs of these tools, counts
the tokens of model calls, estimates the electricity used to serve
those calls, and the emissions from producing that electricity. The
embodied emissions of hardware, training or water use are not estimated
yet.

## Getting started

```sh
npm install -g tokendraw
```

or `cargo install --locked tokendraw` (see [Building from
source](#building-from-source)). Then:

```sh
tokendraw report
```

The first run reads all your agents' logs, which can take a minute. The
coverage table shows where tokendraw looked for each agent's logs - if it
says `not found` for an agent you use, see [Where the logs
are](#where-the-logs-are) for the variable that points tokendraw to them.

On Windows, use PowerShell: the commands tokendraw suggests are quoted for
it.

Next, run `tokendraw projects` to assign calls that belong to no project
(see [Projects](#projects)), and if the report names agents or models
tokendraw cannot read or has no data for, [send us your
setup](#sending-us-your-setup).

## Configuring for your setup

### Where the logs are

Tokendraw reads each agent's logs where the agent writes them, from the
same environment variables:

- Claude Code: `$CLAUDE_CONFIG_DIR/projects`, else `~/.claude/projects`
- Codex: `$CODEX_HOME/sessions`, else `~/.codex/sessions`
- pi: `$PI_CODING_AGENT_SESSION_DIR`, else `$PI_CODING_AGENT_DIR/sessions`,
  else `~/.pi/agent/sessions`

Sessions pi keeps elsewhere through its `--session-dir` option or
`sessionDir` setting are not read.

### Projects

A call belongs to the project of the Git checkout its working directory
is in, including a directory that was removed if tokendraw saw it while it
existed. Calls it cannot place, such as those of worktrees removed before
tokendraw first ran, stay unresolved until you assign them.
`tokendraw projects` lists them in groups and, for directories named after
a checkout beside them, prints the command that assigns them:

```sh
tokendraw label dir '~/src/app-*' project=~/src/app
```

`tokendraw projects --help` and `tokendraw label dir --help` describe how
calls are resolved and how labels and patterns work.

### Other agents

An adapter for another agent, proxy or tool is a TOML file in the
`adapters` directory of the configuration directory:
`$XDG_CONFIG_HOME/tokendraw/adapters` (default
`~/.config/tokendraw/adapters`) on Linux,
`~/Library/Application Support/tokendraw/adapters` on macOS and
`%APPDATA%\tokendraw\adapters` on Windows.
[docs/adapters.md](docs/adapters.md) describes the file for JSON lines
logs and the one that runs a command, and `tokendraw adapter check` tests
one before you add it.

### The data

`tokendraw data` prints the data the estimates use: the files in
[`data/`](data/), built into the binary. To replace or add a record, for
example to declare where a model you run yourself is served, put your own
TOML files in the `data` directory of the configuration directory;
[data/README.md](data/README.md) describes the format.

## Sending us your setup

If tokendraw cannot read an agent you use, counts unreadable lines in its
logs, or has no data for your models or providers, run

```sh
tokendraw report --json --redact
```

and paste the output into a new issue in this repository with the
"Setup report" form. It shows the other agents' logs tokendraw found, the
models and providers it has no data for, and the versions of tokendraw and
your agents, without paths, project names or anything from your
conversations. Read it before you post it: issues are public.

## Privacy

Tokendraw reads your session logs on your machine. It makes no network
requests and sends no telemetry; a command adapter you add runs your
command, which may.

It keeps what it counts in a ledger, the file `tokendraw/ledger.sqlite`
in your system's state directory (see [The
ledger](docs/counting.md#the-ledger)): call and session identifiers, the
paths of working directories and log files, the definitions of the
adapters you add and your labels, never the content of a conversation. The ledger keeps them after your agents delete their logs.
Delete the file to remove them; the next run reads the logs that remain.

## Building from source

With Rust 1.97 or later (`rustup update` updates an older one):

```sh
cargo install --locked tokendraw
```

The build compiles SQLite, so it needs a C compiler: on Linux one such as
gcc, on macOS the Xcode Command Line Tools, on Windows the Microsoft C++
Build Tools. From a clone of this repository, where rustup uses the pinned
toolchain:

```sh
cargo install --locked --path .
```

Prebuilt archives for Linux (x86_64, aarch64), macOS (x86_64, Apple
silicon) and Windows (x86_64), with checksums and build attestations, are
on the GitHub releases page; the npm package installs the one for your
platform.

## Reference

- [docs/counting.md](docs/counting.md): the ledger, what the logs do not
  show, estimated tokens and the JSON schema
- [docs/adapters.md](docs/adapters.md): adapters for other agents
- [data/README.md](data/README.md): the data and its sources
- [docs/validation.md](docs/validation.md): the estimates against the
  energy and emissions providers have published

## How we build it

We use LLM coding agents to build tokendraw. The person who submits a
change answers for it.

## Contributing

[CONTRIBUTING.md](CONTRIBUTING.md) says how to build, check and submit a
change, and
[CHANGELOG.md](CHANGELOG.md) the changes in each release.

## License

The code and data are MIT, see [LICENSE](LICENSE), except the grid data
from Ember in [data/grid/](data/grid/), which is CC BY 4.0.
