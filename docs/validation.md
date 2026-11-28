# Estimates against published figures

Three providers have published the energy or emissions of one chat
request. This note runs requests like theirs through tokendraw and shows
where each published figure falls in tokendraw's cases: the base case,
the low and high case and the bounds (see `tokendraw report --help`).

None of the three says how many tokens its request had, and the energy
of a request depends on them. The comparisons below therefore rest on
assumed token counts. They show whether a published figure is consistent
with tokendraw's cases at those counts; they do not show that the
base case or the prices behind it are right.

## The published figures

- **Google, 0.24 Wh** for the median Gemini Apps text prompt in May 2025
  (Elsworth et al., "Measuring the environmental impact of delivering AI
  at Google Scale", https://arxiv.org/abs/2508.15734, 2025-08-21).
  Measured on Google's fleet, it covers the accelerators, host CPU and
  memory, idle machines held in reserve, and data-centre overhead at
  Google's fleet PUE of 1.09: the same parts as tokendraw's energy
  estimate. The median is the average energy per prompt of the model
  that serves the middle prompt when Google's models are ranked by
  energy per prompt, and it includes supporting models such as routing.
  Not stated: token counts, context length, cache use and which models
  served the prompts.
- **OpenAI, about 0.34 Wh** for the average ChatGPT query (Sam Altman,
  "The Gentle Singularity", https://blog.samaltman.com/the-gentle-singularity,
  June 2025). Not stated: what the figure includes, the model and the
  token counts.
- **Mistral, 1.14 g CO2e** for a 400-token response of its Le Chat
  assistant, as the marginal impact of inference, excluding the user's
  device ("Our contribution to a global environmental standard for AI",
  https://mistral.ai/news/our-contribution-to-a-global-environmental-standard-for-ai,
  2025-07-22, a life-cycle analysis of Mistral Large 2 with Carbone 4
  and ADEME). It is an emissions figure that includes upstream impacts
  such as manufacturing the servers, which tokendraw does not estimate.
  Not stated: the energy, the electricity grid, the input length and the
  model behind Le Chat.

## How the requests are run

Each request has no cache and runs at 100, 1,000 and 10,000 input
tokens. Its output is 500 tokens for Google and OpenAI, the typical
query that Epoch AI assumed for its estimate of ChatGPT's energy
(https://epoch.ai/gradient-updates/how-much-energy-does-chatgpt-use,
2025-02-07), and 400 tokens for Mistral, as disclosed.

Each runs with more than one parameter set, since the models are not
disclosed:

- `frontier`, what tokendraw uses for a closed model without a price;
  here for Gemini Apps, for `chatgpt-4o-latest` and for Mistral Large 2
  (a dense model of 123 billion parameters, near the set's assumption of
  about 100 billion active parameters).
- `frontier@<price>`, the same set with the weight terms taken from a
  list price ([data/README.md](../data/README.md#prices)): Gemini 2.5
  Flash ($0.30 / $2.50 per million input and output tokens), Gemini 2.5
  Pro (the shipped price), GPT-4o ($2.50 / $10) and Mistral Large 2
  ($2 / $6, its price from September 2024).

OpenAI's calls use the shipped `openai` provider. Google's use a
`google` provider with Google's fleet PUE of 1.09, which its figure
includes. Mistral's use the shipped `unknown` provider, since Mistral
states neither its sites nor its PUE, so its emissions span every grid
in the data.

The requests are [validation/calls.jsonl](validation/calls.jsonl) and
the prices and Google's PUE [validation/data.toml](validation/data.toml),
a data override file. The test `validation_tables_match_the_tool` in
`tests/cli.rs` reads both with an adapter in a temporary directory, runs
`tokendraw report --json` for each request and fails if the tables below
differ from what it computes, printing the tables it got. `cargo test
validation` reruns it.

## Results

Google, energy in Wh:

| Input tokens | Parameter set | Lower bound | Low case | Base case | High case | Upper bound | 0.24 Wh is |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 100 | `frontier` | 0.0257 | 0.0515 | 0.128 | 0.242 | 0.680 | base case to high case |
| 1000 | `frontier` | 0.0530 | 0.106 | 0.251 | 0.551 | 1.51 | low case to base case |
| 10000 | `frontier` | 0.326 | 0.653 | 1.51 | 3.69 | 10.1 | below the lower bound |
| 100 | `frontier@gemini-2.5-flash` | 0.00300 | 0.00622 | 0.0164 | 0.0743 | 0.345 | between high case and upper bound |
| 1000 | `frontier@gemini-2.5-flash` | 0.00593 | 0.0123 | 0.0332 | 0.155 | 0.714 | between high case and upper bound |
| 10000 | `frontier@gemini-2.5-flash` | 0.0352 | 0.0742 | 0.225 | 1.02 | 4.74 | base case to high case |
| 100 | `frontier@gemini-2.5-pro` | 0.0121 | 0.0251 | 0.0651 | 0.283 | 1.33 | base case to high case |
| 1000 | `frontier@gemini-2.5-pro` | 0.0239 | 0.0494 | 0.129 | 0.568 | 2.66 | base case to high case |
| 10000 | `frontier@gemini-2.5-pro` | 0.142 | 0.294 | 0.794 | 3.46 | 16.3 | between lower bound and low case |

At 1,000 input tokens the `frontier` base case, 0.251 Wh, is close
to Google's figure. At 10,000 input tokens Google's figure is below the
`frontier` lower bound: with that set's assumptions, no values tokendraw
allows serve such a prompt for 0.24 Wh. The figure is consistent with
tokendraw if the median prompt is shorter, or if a smaller model serves
it, as the Gemini 2.5 Flash rows show; Google discloses neither. With
Gemini 2.5 Flash's price, the figure is above the high case for the
shorter prompts.

OpenAI, energy in Wh:

| Input tokens | Parameter set | Lower bound | Low case | Base case | High case | Upper bound | 0.34 Wh is |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 100 | `frontier` | 0.0257 | 0.0529 | 0.137 | 0.271 | 0.974 | between high case and upper bound |
| 1000 | `frontier` | 0.0530 | 0.109 | 0.269 | 0.616 | 2.16 | base case to high case |
| 10000 | `frontier` | 0.326 | 0.671 | 1.62 | 4.12 | 14.5 | between lower bound and low case |
| 100 | `frontier@gpt-4o` | 0.0145 | 0.0309 | 0.0838 | 0.380 | 2.29 | base case to high case |
| 1000 | `frontier@gpt-4o` | 0.0286 | 0.0609 | 0.166 | 0.758 | 4.55 | base case to high case |
| 10000 | `frontier@gpt-4o` | 0.170 | 0.363 | 1.01 | 4.60 | 27.7 | between lower bound and low case |

OpenAI's figure is inside every set of bounds, and between the low and
the high case except with `frontier` at 100 and 10,000 input tokens and
with GPT-4o's price at 10,000. Since OpenAI does not say what its figure
includes, this comparison is the weakest of the three.

Mistral, emissions in g CO2e:

| Input tokens | Parameter set | Lower bound | Low case | Base case | High case | Upper bound | 1.14 g CO2e is |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 100 | `frontier` | 0.000454 | 0.00243 | 0.0582 | 0.184 | 1.05 | above the upper bound |
| 1000 | `frontier` | 0.00104 | 0.00555 | 0.127 | 0.465 | 2.58 | between high case and upper bound |
| 10000 | `frontier` | 0.00688 | 0.0368 | 0.824 | 3.32 | 18.5 | base case to high case |
| 100 | `frontier@mistral-large-2` | 0.000170 | 0.000943 | 0.0238 | 0.172 | 1.64 | between high case and upper bound |
| 1000 | `frontier@mistral-large-2` | 0.000372 | 0.00206 | 0.0523 | 0.381 | 3.63 | between high case and upper bound |
| 10000 | `frontier@mistral-large-2` | 0.00239 | 0.0133 | 0.351 | 2.52 | 24.2 | base case to high case |

At 100 input tokens Mistral's figure is above the `frontier` upper
bound, and up to 1,000 input tokens it is above every high case. Possible
contributors are the server manufacturing Mistral's figure includes and
tokendraw's estimate does not, the grid and input length Mistral does not
state, and tokendraw's assumptions about the model and its serving. The
figure does not say which; the cause is unresolved.

## What this does not test

- **The base case.** A published figure near a base case at an
  assumed token count does not show that the base case is right for
  other requests, models or hardware.
- **Prices as a measure of energy.** None of the providers says which
  model served its figure, so the `frontier@<price>` rows cannot confirm
  that a model's list price tracks its energy.
- **Coding agent calls.** Each published figure is for a short chat
  request. A coding agent call usually re-reads a long cached context,
  and for such calls much of tokendraw's estimate comes from attention
  over that context. No provider has published a figure for them. Energy
  per call at stated input, cached and output token counts would test
  that part of the estimate.
