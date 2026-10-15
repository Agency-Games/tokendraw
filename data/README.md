# Data

The values tokendraw's estimates come from, each with its sources. The
files are built into the binary byte for byte; `tokendraw data` names each
file and its SHA-256, and `tokendraw data export <dir>` writes them out.
You can [override or extend](#overriding-and-extending-the-data) any record
with your own files.

| File | Holds |
| --- | --- |
| `data.toml` | The data version, the review interval and the sources the other files cite |
| `energy/parameter-sets.toml` | The parameter sets of the per-call energy model |
| `energy/open-weight-sets.toml` | Parameter sets of open-weight models from their published configurations, written by `cargo xtask open-weight-sets` |
| `energy/price-calibration.toml` | The weight energy per USD at open-weight hosts' prices and the markup band, written by `cargo xtask price-calibration` |
| `energy/prices.toml` | Closed models' list prices, with long-context tiers |
| `energy/models.toml` | Model-name patterns, each with a parameter set, optionally a price, and whether its weights are open, closed or unknown |
| `providers.toml` | Per provider: the grid geographies it serves from, and its PUE |
| `grid/ember.toml` | Grid intensity per country and US state, from Ember (CC BY 4.0, see `grid/README.md`) |
| `discovery.toml` | Where agents without an adapter keep their logs, so a report can say it found them but did not read them |
| `findings.toml` | The parameters of the report's findings, such as the cache-read tokens above which a call is long-context, and the steps a user of each client can take |

## Overriding and extending the data

To replace or add a record, put TOML files in the `data` directory of the
configuration directory: `$XDG_CONFIG_HOME/tokendraw/data` (default
`~/.config/tokendraw/data`) on Linux,
`~/Library/Application Support/tokendraw/data` on macOS and
`%APPDATA%\tokendraw\data` on Windows. They use the schema of the shipped
files and are read in file name order. A record with a shipped id replaces
that record whole; a record with a new id adds one. Each record you write
carries its own sources and review date. The same id in two of your files
and an unknown key are errors that name the file.

For example, to estimate calls to a model you run yourself in France:

```toml
[sources.my-setup]
title = "Our inference server"
publisher = "Example studio"
accessed = 2026-10-01

[[provider]]
id = "ollama"
sites = ["ember:FRA"]
label = "M"
sources = ["my-setup"]
basis = "One workstation in Paris"
reviewed = 2026-10-01

[provider.pue]
lower_bound = 1
low_case = 1
base_case = 1
high_case = 1
upper_bound = 1
label = "M"
sources = ["my-setup"]
basis = "No data centre, so no facility overhead"
```

`tokendraw data` shows the result and names every override with its
file's SHA-256. `report --json` names the data version: the date of the
last change to the shipped data, a SHA-256 of the shipped files and the
SHA-256 of each of your files. `report --verbose` shows the date, the
first 12 characters of the shipped files' hash and your files.

## Records

Every record has an `id` (an action has none, see below), its values, a
quality label, the ids of its sources, a review date (`reviewed`) and,
where a value is derived, its `basis` in words. Labels: `M` disclosed by
the provider for this model, `C` published by a company and not
verified, `I` independent research, `D` the tool's default with a range.
`S`, spend-based, is refused: such figures are a cross-check only and
never enter an estimate. A report labels each estimate with the weakest
label among the records it uses.

A parameter or a PUE has five values, lowest first: `lower_bound`,
`low_case`, `base_case`, `high_case`, `upper_bound`. The bounds are what
non-disclosure leaves open; the low and high case are the spread across
documented designs and hardware. A report computes each case with every
value at the same point, so its figures are cases, not confidence
intervals.

A source (`[sources.<id>]`) has a `title`, `publisher` and `accessed`
date, and where they exist a `url`, a `published` date (a year, month or
day), the `licence` of data taken from it and the `sha256` of a downloaded
file.

## Models

A model's `pattern` is an exact model name or a prefix followed by `*`; an
exact match wins, then the longest prefix. A model that no pattern matches
uses the `frontier` parameter set and is reported as unmatched.

## Providers

A provider's `id` is its name as the logs record it; a provider that no
record names uses `unknown`. Its grid intensity is computed from its
`sites`: the base case is their mean, the low and high case are the
lowest and the highest, and the bounds also cover `bounds_sites`,
which is a list, `"listed"` (every geography a shipped provider lists) or
`"all"` (every geography in the shipped grid data). `unknown`'s `sites =
"named"` is every shipped provider's sites. These come from the shipped
data only, so a provider or geography you add changes no other record;
a shipped geography you replace reaches every provider that uses it.

## Locations

A `[[location]]` record names a client without an adapter (`id`, as the
report shows it) and the `paths` where its logs with per-call usage may
be, each with the OSes it applies to (`linux`, `macos`, `windows`). A
path starts with `~` for the home directory, or with `$NAME` or `%NAME%`
for an environment variable, and is skipped when the variable is unset,
empty or relative; `/` separates components on every OS. The first
existing path for the OS is the location the report shows. A record with
the id of a user adapter's client is ignored, since that adapter reports
its own location.

A `[[finding]]` record holds the parameters of one finding of the report,
by its `id`. `long-context` has `cached_tokens`: a call whose cache-read
tokens exceed it is long-context, which the report's "Where the energy
goes" counts. `fixed-context` has no parameters.

An `[[action]]` record holds the `steps` a user of one `client` can take
about one `finding`, each naming the client's command or setting. It has
no `id`: the finding and the client identify it, so a record for a shipped
pair replaces that record, and a record for another client adds its
steps. A finding counts only the calls of clients it has steps for. For
example, for a user adapter's client:

```toml
[sources.my-agent-docs]
title = "My agent's commands"
publisher = "Example studio"
accessed = 2026-10-15

[[action]]
finding = "long-context"
client = "my-agent"
steps = ["/summarize during a task", "/reset between tasks"]
label = "C"
sources = ["my-agent-docs"]
basis = "The agent's command list"
reviewed = 2026-10-15
```

## Open-weight models

A model whose weights and configuration are published has its own
parameter set in `energy/open-weight-sets.toml`, derived from the attention
layout in `xtask/open-weight-sets.toml` and its active parameters.
Attention that grows with every earlier token (full attention, compressed
attention without a top-k cap, lightning indexers) sets `a_p` and `a_d`;
attention capped at a sliding window, a top-k selection or a linear
layer's fixed state costs the same per token beyond its cap and is added
to `b_in` and `b_out` at the cap, so below the cap the set counts slightly
more than the model does. The weight terms scale the `frontier` anchor by
the model's active parameters, `c0` reloads the key-value bytes a cached
token holds, and `idle` is `frontier`'s. Since the design is published and
only the serving hardware is not, the base case is a B200 node, and
the low and high case and the bounds all run from a GB200-class node with
FP8 attention to an H100 node.

## Prices

No lab discloses the energy of its closed models, but each lists a price,
and across open-weight models price tracks the energy of the model's
weights. A model record that names a `price` uses its parameter set with
`b_in` and `b_out` taken from that price; its attention terms, reload term
and `idle` stay the set's.

- The reference call, 1,000 new input and 500 output tokens without cache,
  costs `p` USD at the list price.
- Its weight energy is `p` times the calibration's `wh_per_usd` divided by
  its `markup`; the low values divide by the high markup, so the bands stay
  in order.
- `b_in` and `b_out` split that energy in the ratio `anchor_in_j` :
  `anchor_out_j`.
- When a call's prompt (uncached input, cache writes and cache reads)
  exceeds a tier's `above_prompt_tokens`, both scale by the tier's prices
  over the base prices.

The derived sets are named `<set>@<price>`, and `<set>@<price>>N` above a
tier at N tokens; `tokendraw data` lists them. They are computed when the
data loads, so overriding a price, the calibration or the base set changes
every set derived from it. Parameter set and price ids may hold only
ASCII letters, digits, `.`, `-` and `_`.

The calibration, `wh_per_usd`, is the weight energy at load that a dollar
buys at the hosts of ten open-weight models listed on OpenRouter: for
every host and model, the reference call's weight energy from the model's
active parameters over its price there. `markup` is how far a closed
lab's price may sit above those hosts' for the same serving cost, which no
lab or host publishes. Both bands are wide, so priced models have wider
bands than `frontier`; what prices add is the difference between models.

The calibration can be rerun but not reproduced exactly: OpenRouter's
terms keep its prices out of this repository, and its endpoints list only
the prices in force, so a rerun gives the value of its day. The URL and
SHA-256 of each file used are in `energy/price-calibration.toml`.

## Updating the shipped data

- Change a value together with its sources, `basis` and `reviewed` date in
  one commit, and set `id` in `data.toml` to the date of the change.
- Review every record within the review interval in `data.toml`, and every
  price within `price_review_interval_days`, since labs change prices
  within weeks; `tokendraw data` lists the records that are overdue.
- Refresh the grid data from Ember's latest CSV files at the review:
  download the yearly global file and the US yearly file from
  https://ember-energy.org/data/, then run

  ```
  cargo xtask ember global.csv us.csv --global-url <url> --us-url <url> --accessed <date>
  ```

  which rewrites `grid/ember.toml` with the files' URLs and SHA-256. Do
  not commit the CSV files.
- Regenerate the price calibration when its models or their prices change:
  download `https://openrouter.ai/api/v1/models/<slug>/endpoints` for every
  slug in `xtask/price-calibration.toml`, one file each, then run

  ```
  cargo xtask price-calibration <files> --accessed <date>
  ```

  which rewrites `energy/price-calibration.toml` with only the five values
  and each file's URL and SHA-256. OpenRouter's terms do not allow copying
  its data, so do not commit the files or any price from them.
- Regenerate the open-weight sets when a model's configuration, its active
  parameters in `xtask/price-calibration.toml`, the hardware scenarios in
  `xtask/open-weight-sets.toml` or the `frontier` set's `idle` change:

  ```
  cargo xtask open-weight-sets --reviewed <date>
  ```

  which rewrites `energy/open-weight-sets.toml`. A test fails while the
  shipped file differs from what the command writes.
