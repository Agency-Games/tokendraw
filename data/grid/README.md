# Grid data

`ember.toml` holds the emissions intensity of electricity generation per
country and US state, from Ember:

- Ember, Yearly Electricity Data,
  https://ember-energy.org/data/yearly-electricity-data/
- Ember, US Electricity Data,
  https://ember-energy.org/data/us-electricity-data/

Ember releases this data under the Creative Commons Attribution 4.0
International licence (CC BY 4.0,
https://creativecommons.org/licenses/by/4.0/), and so is `ember.toml`.

Changes from Ember's files:

- only the "Total generation" rows of countries and US states are kept,
  Puerto Rico and the United States from the country data only
- each keeps its latest year with a total intensity of at least zero; a
  year with a negative total is left out and listed in the file's header
  with the year used instead
- kept years in which a fuel's generation is negative are listed in the
  header too

Values are Ember's lifecycle emissions intensity in g CO2e per kWh,
unchanged. The file's source records name the URLs and SHA-256 of the CSV
files it was generated from.

## Negative values

Some of Ember's rows give a fuel negative generation:

- In the yearly data by country, negative fuel rows occur only in the
  latest year, which Ember estimates by "applying absolute changes by fuel
  from available annualised monthly data to historical annual values"
  (Ember, Electricity Data Methodology,
  https://files.ember-energy.org/public-downloads/ember_electricity_data_methodology.pdf).
  Costa Rica's 2025 row for oil and other fossil fuels is -1.134 TWh, which
  makes its total -36.353 g CO2e per kWh; `ember.toml` uses its 2024 value
  instead.
- In the US data, which Ember takes from the EIA, the rows are net
  generation, gross generation less station use. Ember's "Hydro" includes
  pumped storage, whose pumping energy the EIA deducts from generation
  (EIA glossary, "Net generation",
  https://www.eia.gov/tools/glossary/index.php?id=Net%20generation), and
  its "Other fossil" includes the EIA's "Other", which includes batteries
  (EIA, Electric Power Monthly, Table 1.1 notes,
  https://www.eia.gov/electricity/monthly/epm_table_grapher.php?t=epmt_1_01).
  Storage losses then reduce a state's emissions at that fuel's factor,
  so its total is slightly low; the header gives the intensity of the
  fuels with positive generation for comparison.

`cargo xtask ember` generates the file; see [Updating the shipped
data](../README.md#updating-the-shipped-data).
