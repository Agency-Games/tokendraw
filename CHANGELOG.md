# Changelog

All notable changes to tokendraw are recorded in this file. The format is
based on Keep a Changelog 1.1.0 (https://keepachangelog.com/en/1.1.0/),
and tokendraw follows Semantic Versioning (https://semver.org/).

## [Unreleased]

### Changed

- Data version 2026-11-28. The attention and cache reload terms of the
  `frontier` set (`a_p`, `a_d` and `c0`), which every closed model uses,
  now come from the open-weight models whose configurations the data
  holds. The low case, base case and high case are the lowest, the median
  and the highest of the models with at least 40 billion active
  parameters, and the bounds span every documented design, from DeepSeek
  V4.1 Flash to Llama 3.1 405B on H100. Before, they were a hand estimate
  for a large model with full attention, whose base case was above every
  documented large model's in `a_p` and above their median in `a_d` and
  `c0`. No lab discloses the attention of its closed models, so the
  estimate now assumes they are no less efficient than the median
  documented large open-weight model. `cargo xtask open-weight-sets`
  writes these terms.
- Long-context price tiers no longer raise the weight energy of a call
  above their threshold. They stay in the data as listed prices: the labs
  do not say what the surcharge pays for, and the attention terms already
  grow with the context.
- The low and high case of the price markup are 1.351 and 2.465, the
  geometric means of the base case and each bound, instead of the bounds
  1 and 3.33.
- OpenAI's PUE runs from a low case of 1.12, Microsoft's design PUE for
  its newest datacenters, to a high case of 1.22, its design goal for the
  Americas, instead of 1.17 throughout.
- The `frontier` set's `b_out` base case is 0.67 J per output token, as
  its derivation gives, instead of 0.6.
- `claude-fable-5-2`, `claude-opus-4-8`, `claude-opus-4-7`,
  `gpt-6.2-astra` and `gpt-6.2-sol` have their list prices, so their
  weight terms come from them instead of the `frontier` set. Every list
  price was reviewed on 2026-11-28.

Together, on one user's Claude Code and Codex logs for September and
November 2026, these changes lower the base case by 37 and 41 percent,
the low case by 58 and 65 percent and the high case by 13 and 3 percent,
and raise the upper bound by 23 and 39 percent. Attention over the
context is a smaller share of the estimate, so the report's "rereading
the context" share falls and the `model-choice` finding's estimated
saving rises.

## [0.3.1] - 2026-11-28

### Security

- The bundled SQLite is 3.54.1, past RUSTSEC-2026-0459
  (https://rustsec.org/advisories/RUSTSEC-2026-0459): SQLite before 3.54.0
  can return uninitialised memory to a process that opens a database
  together with a `-wal` file of another page size. tokendraw opens only
  its own ledger, so only someone who can write files beside the ledger
  could cause it. A ledger written by 0.3.0 reads the same.

### Fixed

- `report --project` and `findings --project` count only a project's
  calls when unresolved calls were made in a directory of the same name,
  as when a label names a project by the path of such a directory. They
  counted both.

## [0.3.0] - 2026-10-15

### Added

- `report` shows the findings with at least 10% of the energy, set by
  `threshold_percent` in the new configuration file, with the steps for
  each client and a command to see the effect; `tokendraw findings` lists
  them all. `report --json` has them in a new `findings` field, and
  `findings --json` prints them with its own `schema_version`, 1.
- The `long-context` finding: calls that carry more than 200,000 cached
  tokens.
- The `fixed-context` finding: the context each call carries from the
  start of its session or subagent, such as the system prompt, tool
  definitions and instruction files.
- The `model-choice` finding: subagent and headless calls of Claude Code
  and Codex on models pricier than the lowest-priced model the client's
  docs list, and how much lower the total would have been on that model
  with the same tokens.

### Changed

- The first run reads Claude Code's logs again to record which subagent
  made each call.

## [0.2.0] - 2026-10-12

### Changed

- The report's columns `central`, `plausible` and `envelope` are now
  `base case`, `low to high case` and `bounds`, with a line under the
  tables saying they are cases, not confidence intervals.
- Breaking: `report --json` has schema version 2, and `data --json`,
  which had none, has `schema_version` 2. The fields
  `envelope_low`, `plausible_low`, `central`, `plausible_high` and
  `envelope_high` are now `lower_bound`, `low_case`, `base_case`,
  `high_case` and `upper_bound`, and `share_of_central` and
  `shares_of_central` are `share_of_base_case` and `shares_of_base_case`.
  `data --json` also names `bounds_sites` and `bounds_ids` for
  `envelope_sites` and `envelope_ids`.
- Breaking: data files use the same new keys, and `envelope_sites` is
  `bounds_sites`. Rename them in your data overrides; a file with the old
  keys is refused.
- The npm package installs the binary for your platform as an optional
  dependency, without an install script, so npm 12 installs it without a
  warning and nothing is downloaded when it first runs.
- pi's calls to Moonshot AI's API, which pi records as `moonshotai`
  (api.moonshot.ai) and `moonshotai-cn` (api.moonshot.cn), have their own
  provider records instead of `unknown`: Singapore, where Moonshot AI's
  privacy policy places its servers, and mainland China, both assumed.
  The `moonshot` record, which no client records, is removed; rename it
  in a user adapter or data override that names it.
- A provider without a record includes Singapore in its base case grid
  intensity, which moves from 376 to 388 g CO2e per kWh.
- `tokendraw data` prints values to four significant figures, so values
  computed from model configurations and prices no longer show digits
  such as `0.04710399999999999`. `data --json` keeps them exact.

## [0.1.0] - 2026-10-09

First release: `tokendraw report` estimates the electricity and emissions
of Claude Code, Codex and pi use from their local logs, with user adapters
for other agents and data you can override. The report shows where the
energy goes: the share of the terms that grow with the context, of
long-context calls, and of each client and role. See the README to get
started.
