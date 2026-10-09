# Adding an adapter

An adapter for another agent, proxy or tool is a TOML file in the
`adapters` directory of the configuration directory:
`$XDG_CONFIG_HOME/tokendraw/adapters` (default
`~/.config/tokendraw/adapters`) on Linux,
`~/Library/Application Support/tokendraw/adapters` on macOS and
`%APPDATA%\tokendraw\adapters` on Windows. Every command that reads the
logs loads each `*.toml` file there and ignores other files. Each file
names the `client` its calls are stored under, which cannot be a shipped
adapter's, and either reads log files or runs a command.

## An adapter for JSON lines logs

For logs with one JSON object per line, the file says where the logs are,
which lines are calls and which fields hold what. Fields are JSON Pointers
(RFC 6901), such as `/usage/input_tokens`.

```toml
client = "example-agent"

[files]
root = "~/.example-agent/logs"   # absolute, or ~/ for the home directory
extension = "jsonl"              # the default

[when]                           # a line is a call when all of these hold
"/type" = "response"

[fields]
id = "/id"
timestamp = "/time"              # an RFC 3339 time
session = "/session"
cwd = "/cwd"
model = "/model"

[values]                         # the same for every call
provider = "example-cloud"
role = "interactive"             # the default; or headless, subagent

[tokens]
input = "/usage/prompt_tokens"
input_includes_cache = true      # input counts the cache reads and writes
cache_read = "/usage/cached_tokens"
output = "/usage/completion_tokens"
reasoning = "/usage/reasoning_tokens"
```

- `timestamp`, `model` and `provider` are required; `model` and
  `provider` come either from a field in `[fields]` or from `[values]`.
  `[tokens]` needs `input` and `output`; `cache_read`, `cache_write` and
  `reasoning` are optional, and a count missing from a line is zero.
- `input_includes_cache` (default `false`) says that `input` counts the
  cache reads and writes, so uncached input is input minus both.
  `output_includes_reasoning` (default `true`) says that `output` counts
  the reasoning; with `false`, the reasoning is added to the output.
- Without `session`, a call's session is its file's path below the root.
- `usage = "per_call"` (the default, written before the first table):
  each call line holds one call's own usage. Lines with the same `id` are
  one call, with the highest value of each token class, as for a response
  logged again while it streams.
- `usage = "cumulative"`: each call line holds the running totals of its
  file, and a call is the increase over the previous call line. When the
  totals fall, counting started again, and the call is the new totals. A
  line whose totals did not grow is no call.
- Set `id` when the log has one; every call line must then have it.
  Without `id`, a call's ID is made from its file's path below the root,
  its line (or its increase of the totals) and everything read from the
  line, so a file that was replaced counts its new calls, and a file
  rewritten with the same lines is not counted twice. Two calls at the
  same place in a file that match in every field count once, and a file
  that is moved or renamed is counted again.
- A line that is not JSON, or a call line missing a required field, is
  counted as unreadable. A call line whose tokens are all zero is no call.

## An adapter that runs a command

For anything a file cannot describe, the adapter runs a command that
prints usage records:

```toml
client = "example-proxy"
command = ["python3", "example-proxy.py", "--records"]
```

The command runs without a shell, in the `adapters` directory, so a
script beside the file needs no path. It prints every record it has, one
JSON object per line, on every run; call IDs make printing a record again
harmless. Its stderr is shown. If it cannot start or exits with an error,
tokendraw stops and keeps nothing it printed.

```json
{"id": "gen-1", "timestamp": "2026-09-29T12:00:00Z", "provider": "example-cloud", "model": "example-medium", "session_id": "job-1", "cwd": "/work", "role": "headless", "tokens": {"uncached_input": 300, "cache_write": 0, "cache_read": 200, "output": 50, "reasoning": 5}}
```

`cwd`, `role` (default `interactive`), `cache_write`, `cache_read` and
`reasoning` are optional, and other fields are ignored. The token classes
are those of `tokendraw tokens --json`: they do not overlap, except that
`output` includes `reasoning`. A record that cannot be read is counted as
an unreadable line, and the command's output counts as one file.

## Checking and changing an adapter

```sh
tokendraw adapter check example-agent.toml
```

Check a new adapter file before you add it to the `adapters` directory:
the ledger keeps the highest value it has read for each call, so calls
read with a wrong file stay wrong. `tokendraw adapter check` reads the
adapter's logs, or runs its command, and prints what it counts, as
`tokendraw tokens` does, without writing the ledger.

The ledger knows an adapter's calls by its `client`. Changing the `client`
in a file makes a new client: its logs are read again under the new name,
and the calls under the old name stay, so they count twice. Run
`tokendraw adapter reset <old client>` to remove the old calls, or change
the name back.
