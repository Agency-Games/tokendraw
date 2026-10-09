# How tokendraw counts

## The ledger

The ledger is an SQLite database, `tokendraw/ledger.sqlite` in
`$XDG_STATE_HOME` (default `~/.local/state`) on Linux,
`~/Library/Application Support` on macOS and `%LOCALAPPDATA%` on Windows.
It holds numbers, identifiers and the paths of working directories and log
files, never the content of a conversation. Calls stay in it after the
client deletes its logs, so running tokendraw regularly keeps a history
the logs alone would lose. Deleting the file deletes that history and
your labels; the next run reads the logs that remain.

## What the logs do not show

Some calls never reach the logs tokendraw reads, or reach them without
their final usage, so tokendraw counts fewer tokens than these clients
used:

- Claude Code writes each part of a response on its own line, and only
  the last line, which records why the response stopped, carries the final
  usage. For some subagent calls that line is never written. `tokens`
  counts their output as an earlier line records it, often a few tokens,
  and prints how many calls lack the line and an estimate of the output
  they miss. `report` adds that estimate to energy and emissions (see
  [Estimated tokens](#estimated-tokens)).
- Claude Code records the usage of some calls only in a session's running
  totals and never as a message, among them its calls to a small model
  beside the conversation. Tokendraw does not read those totals yet.
- Codex rollouts written before Codex logged each response's usage
  (before September 2026) are counted from the increases of the session's
  totals, which leave out the calls that compact the conversation.
  Tokendraw counts each compaction such a rollout records as a call
  without tokens, and `tokens` and `report` print how many there are and
  an estimate of their tokens, which `report` adds to energy and
  emissions.

## Estimated tokens

A call without its final usage line gets an estimated output: the output
its first line records plus the mean growth from first line to final line
of the complete calls of the same client, role and model that stopped to
use a tool, across the whole ledger. A call keeps its logged output when
that is higher, and gets no estimate when fewer than 30 comparable calls
exist. `report`'s usage table counts the logged output; its energy and
emissions use the estimate, a note says how many calls and tokens it
adds, and those estimates carry the activity label `E` instead of `P` in
the JSON.

A Codex compaction without a usage record gets an estimated input and
output from the compactions Codex did record, across the whole ledger:

- input: the input of the session's call before it, split into uncached
  input and cache read as the recorded compactions of the same model split
  theirs, or all recorded compactions when that model has fewer than 30;
  none when the compaction has no earlier call
- output: the mean output of the recorded compactions of the same model;
  none when the model has fewer than 30

`report` uses these estimates the same way, with their own note.

## JSON

Every `--json` output has a `schema_version`, which changes only when a
field is removed or changed; within a version fields are only added.
Values are unrounded, energy in Wh and emissions in g CO2e. An estimate's
`activity_quality` is `P` when its token counts come from the logs and `E`
when they include [estimated tokens](#estimated-tokens).
