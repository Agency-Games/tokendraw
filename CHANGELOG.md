# Changelog

All notable changes to tokendraw are recorded in this file. The format is
based on Keep a Changelog 1.1.0 (https://keepachangelog.com/en/1.1.0/),
and tokendraw follows Semantic Versioning (https://semver.org/).

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
