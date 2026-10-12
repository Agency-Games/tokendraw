# Changelog

All notable changes to tokendraw are recorded in this file. The format is
based on Keep a Changelog 1.1.0 (https://keepachangelog.com/en/1.1.0/),
and tokendraw follows Semantic Versioning (https://semver.org/).

## [Unreleased]

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

## [0.1.0] - 2026-10-09

First release: `tokendraw report` estimates the electricity and emissions
of Claude Code, Codex and pi use from their local logs, with user adapters
for other agents and data you can override. The report shows where the
energy goes: the share of the terms that grow with the context, of
long-context calls, and of each client and role. See the README to get
started.
