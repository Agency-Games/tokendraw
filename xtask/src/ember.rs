//! Extraction of each geography's latest usable emissions intensity from
//! Ember's yearly CSV files.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use anyhow::{Context as _, bail};
use csv::StringRecord;
use jiff::civil::Date;
use sha2::{Digest as _, Sha256};

const TOTAL: &str = "Total generation";
const COUNTRY: &str = "Country or economy";
const INTENSITY: &str = "Emissions intensity (gCO2e/kWh)";
/// Trade, not generation, so it has no place among the fuels.
const NET_IMPORTS: &str = "Net imports";
/// Codes in the US file that the global file has as a country or economy:
/// the whole country, and Puerto Rico. They come from the global file, so
/// that every country has one record from one source.
const US_CODES_IN_GLOBAL: [&str; 2] = ["US", "PR"];
const GLOBAL_SOURCE: &str = "ember-yearly";
const US_SOURCE: &str = "ember-us";
const WRAP: usize = 76;
const G_PER_KWH_PER_MT_PER_TWH: f64 = 1000.0;

/// A fuel whose generation Ember gives as negative for a year, either net
/// generation below zero or an artefact of estimating the latest year from
/// monthly changes per fuel.
#[derive(Debug, Clone)]
pub struct NegativeFuel {
    pub fuel: String,
    /// In terawatt-hours, as written in the CSV.
    pub generation: String,
}

#[derive(Debug, Clone)]
pub struct Geography {
    pub id: String,
    pub name: String,
    pub year: i32,
    /// Ember's figure as written in the CSV, in grams of carbon dioxide
    /// equivalent per kilowatt-hour.
    pub value: String,
    pub source: &'static str,
    /// Fuels with negative generation in this year, whose emissions are
    /// part of the total.
    pub negative_fuels: Vec<NegativeFuel>,
    /// The intensity of the fuels with positive generation alone, where
    /// some fuel is negative.
    pub without_negative_fuels: Option<f64>,
}

/// A year whose total intensity is negative, which no estimate can use.
#[derive(Debug)]
pub struct Rejected {
    pub geography: Geography,
    pub used_year: Option<i32>,
}

#[derive(Debug)]
pub struct Extract {
    /// Sorted by id, each at its latest year with a usable total.
    pub geographies: Vec<Geography>,
    /// Years newer than the one used, rejected for a negative total.
    pub rejected: Vec<Rejected>,
}

pub struct Meta {
    pub global_url: String,
    pub global_sha256: String,
    pub us_url: String,
    pub us_sha256: String,
    pub accessed: Date,
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .fold(String::new(), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

struct Year {
    name: String,
    source: &'static str,
    total: Option<String>,
    negative_fuels: Vec<NegativeFuel>,
    positive_generation: f64,
    positive_emissions: f64,
}

type Years = BTreeMap<String, BTreeMap<i32, Year>>;

/// Reads both files and keeps, per geography, its latest year whose total
/// intensity is not negative.
pub fn extract(global: &[u8], us: &[u8]) -> anyhow::Result<Extract> {
    let mut years = Years::new();
    read_rows(global, "global", &mut years, |row| {
        if row.get("Area type")? != COUNTRY {
            return Ok(None);
        }
        let iso = row.get("ISO 3 code")?;
        if iso.is_empty() {
            bail!("{} has an empty ISO 3 code", row.place());
        }
        Ok(Some((
            format!("ember:{iso}"),
            row.get("Area")?.to_owned(),
            GLOBAL_SOURCE,
        )))
    })
    .context("in the global file")?;
    read_rows(us, "US", &mut years, |row| {
        let code = row.get("State code")?;
        if code.is_empty() {
            bail!("{} has an empty state code", row.place());
        }
        if US_CODES_IN_GLOBAL.contains(&code) {
            return Ok(None);
        }
        Ok(Some((
            format!("ember:US-{code}"),
            row.get("State")?.to_owned(),
            US_SOURCE,
        )))
    })
    .context("in the US file")?;

    let mut geographies = Vec::new();
    let mut rejected = Vec::new();
    for (id, by_year) in years {
        let mut newer = Vec::new();
        let mut used = None;
        for (year, data) in by_year.into_iter().rev() {
            let Some(geography) = geography(&id, year, data) else {
                continue;
            };
            if geography.value.starts_with('-') {
                newer.push(geography);
            } else {
                used = Some(geography);
                break;
            }
        }
        let used_year = used.as_ref().map(|geography| geography.year);
        rejected.extend(newer.into_iter().map(|geography| Rejected {
            geography,
            used_year,
        }));
        geographies.extend(used);
    }
    Ok(Extract {
        geographies,
        rejected,
    })
}

fn geography(id: &str, year: i32, data: Year) -> Option<Geography> {
    let value = data.total?;
    let without_negative_fuels = (!data.negative_fuels.is_empty()
        && data.positive_generation > 0.0)
        .then(|| data.positive_emissions / data.positive_generation * G_PER_KWH_PER_MT_PER_TWH);
    Some(Geography {
        id: id.to_owned(),
        name: data.name,
        year,
        value,
        source: data.source,
        negative_fuels: data.negative_fuels,
        without_negative_fuels,
    })
}

struct Row<'a> {
    record: &'a StringRecord,
    headers: &'a StringRecord,
    file: &'a str,
}

impl Row<'_> {
    fn get(&self, column: &str) -> anyhow::Result<&str> {
        let index = self
            .headers
            .iter()
            .position(|header| header == column)
            .with_context(|| format!("the {} file has no column {column:?}", self.file))?;
        self.record
            .get(index)
            .map(str::trim)
            .with_context(|| format!("{} has no value for {column:?}", self.place()))
    }

    fn number(&self, column: &str) -> anyhow::Result<Option<f64>> {
        let text = self.get(column)?;
        if text.is_empty() {
            return Ok(None);
        }
        text.parse().map(Some).with_context(|| {
            format!(
                "{} has {column} {text:?}, which is not a number; check the file",
                self.place()
            )
        })
    }

    fn place(&self) -> String {
        let line = self.record.position().map_or(0, csv::Position::line);
        format!("line {line} of the {} file", self.file)
    }
}

/// `identify` returns the id, name and source of a row to keep, or `None` for
/// a row outside the extract.
fn read_rows(
    bytes: &[u8],
    file: &str,
    years: &mut Years,
    identify: impl Fn(&Row) -> anyhow::Result<Option<(String, String, &'static str)>>,
) -> anyhow::Result<()> {
    let mut reader = csv::Reader::from_reader(bytes);
    let headers = reader.headers()?.clone();
    for record in reader.records() {
        let record = record?;
        let row = Row {
            record: &record,
            headers: &headers,
            file,
        };
        let fuel = row.get("Electricity source")?;
        let is_total = fuel == TOTAL;
        let is_fuel = row.get("Is aggregated source")? == "False" && fuel != NET_IMPORTS;
        if !is_total && !is_fuel {
            continue;
        }
        let Some((id, name, source)) = identify(&row)? else {
            continue;
        };
        let year_text = row.get("Year")?;
        let year: i32 = year_text.parse().with_context(|| {
            format!(
                "{} has year {year_text:?}, which is not a year",
                row.place()
            )
        })?;
        let data = years
            .entry(id.clone())
            .or_default()
            .entry(year)
            .or_insert(Year {
                name,
                source,
                total: None,
                negative_fuels: Vec::new(),
                positive_generation: 0.0,
                positive_emissions: 0.0,
            });
        if is_total {
            let value = row.get(INTENSITY)?;
            if value.is_empty() {
                continue;
            }
            if !is_decimal(value) {
                bail!(
                    "{} has intensity {value:?}, which is not a decimal number; check the file",
                    row.place()
                );
            }
            if data.total.is_some() {
                bail!(
                    "{} repeats {id} for {year}; ids must be unique per year",
                    row.place()
                );
            }
            data.total = Some(value.to_owned());
        } else if let Some(generation) = row.number("Generation (TWh)")? {
            if generation < 0.0 {
                data.negative_fuels.push(NegativeFuel {
                    fuel: fuel.to_owned(),
                    generation: row.get("Generation (TWh)")?.to_owned(),
                });
            } else {
                data.positive_generation += generation;
                data.positive_emissions += row.number("Emissions (MtCO2e)")?.unwrap_or(0.0);
            }
        }
    }
    Ok(())
}

/// A plain decimal such as `-36.353` or `412`, which is also valid TOML once
/// it carries a fraction. Leading zeros and exponents are rejected because
/// TOML does not accept the former and Ember does not write the latter.
fn is_decimal(text: &str) -> bool {
    let unsigned = text.strip_prefix('-').unwrap_or(text);
    let (whole, fraction) = unsigned.split_once('.').unwrap_or((unsigned, "0"));
    let digits = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
    digits(whole) && digits(fraction) && (whole == "0" || !whole.starts_with('0'))
}

pub fn render(extract: &Extract, meta: &Meta) -> String {
    let mut out = String::new();
    comment(
        &mut out,
        "Generated by `cargo xtask ember` from Ember's yearly electricity data. \
         Do not edit by hand; rerun the command with newer CSV files.",
    );
    out.push_str("#\n");
    comment(
        &mut out,
        "Ember's data is released under the Creative Commons Attribution 4.0 \
         licence (CC BY 4.0, https://creativecommons.org/licenses/by/4.0/). \
         Source: Ember, Yearly Electricity Data and US Electricity Data, \
         https://ember-energy.org/data/. Values are lifecycle emissions \
         intensity of electricity generation, g CO2e per kWh, for each \
         geography's latest year in the files.",
    );
    out.push_str("#\n");
    rejected(&mut out, &extract.rejected);
    out.push_str("#\n");
    kept_with_negative_fuels(&mut out, &extract.geographies);
    // The array comes before the source tables: after a table header, every
    // key belongs to that table.
    out.push_str("\ngeography = [\n");
    for geography in &extract.geographies {
        let fraction = if geography.value.contains('.') {
            ""
        } else {
            ".0"
        };
        let _ = writeln!(
            out,
            "  {{ id = {}, name = {}, year = {}, g_co2e_per_kwh = {}{fraction}, \
             basis = \"lifecycle\", label = \"I\", sources = [{}], reviewed = {} }},",
            quote(&geography.id),
            quote(&geography.name),
            geography.year,
            geography.value,
            quote(geography.source),
            meta.accessed,
        );
    }
    out.push_str("]\n");
    source(
        &mut out,
        GLOBAL_SOURCE,
        "Ember Yearly Electricity Data (emissions intensity by country)",
        &meta.global_url,
        meta.accessed,
        &meta.global_sha256,
    );
    source(
        &mut out,
        US_SOURCE,
        "Ember US Electricity Data (emissions intensity by state)",
        &meta.us_url,
        meta.accessed,
        &meta.us_sha256,
    );
    out
}

fn rejected(out: &mut String, rejected_years: &[Rejected]) {
    if rejected_years.is_empty() {
        out.push_str("# Rejected: none\n");
    } else {
        comment(
            out,
            "Rejected, because the total intensity is negative, which no estimate \
             can use; the geography keeps its latest year with a usable total:",
        );
        for rejected in rejected_years {
            let geography = &rejected.geography;
            let used = rejected.used_year.map_or_else(
                || "no usable year".to_owned(),
                |year| format!("{year} used"),
            );
            entry(
                out,
                &format!(
                    "{} {}, {}: {} g CO2e per kWh; {used}",
                    geography.id, geography.name, geography.year, geography.value
                ),
            );
        }
    }
}

fn kept_with_negative_fuels(out: &mut String, geographies: &[Geography]) {
    let distorted: Vec<&Geography> = geographies
        .iter()
        .filter(|geography| !geography.negative_fuels.is_empty())
        .collect();
    if distorted.is_empty() {
        out.push_str("# Kept with a negative fuel: none\n");
    } else {
        comment(
            out,
            "Kept, although a fuel's generation is negative in that year (see \
             README.md), with the intensity of the fuels with positive generation \
             alone:",
        );
        for geography in distorted {
            let fuels = geography
                .negative_fuels
                .iter()
                .map(|fuel| format!("{} {} TWh", fuel.fuel, fuel.generation))
                .collect::<Vec<_>>()
                .join(", ");
            let without = geography
                .without_negative_fuels
                .map_or_else(String::new, |value| format!("; {value:.1} without it"));
            entry(
                out,
                &format!(
                    "{} {}, {}: {}; {fuels}{without}",
                    geography.id, geography.name, geography.year, geography.value
                ),
            );
        }
    }
}

fn source(out: &mut String, key: &str, title: &str, url: &str, accessed: Date, sha256: &str) {
    let _ = write!(
        out,
        "\n[sources.{key}]\ntitle = {}\nurl = {}\npublisher = \"Ember\"\n\
         accessed = {accessed}\nlicence = \"CC-BY-4.0\"\nsha256 = {}\n",
        quote(title),
        quote(url),
        quote(sha256),
    );
}

fn comment(out: &mut String, text: &str) {
    wrapped(out, text, "#", "#");
}

pub(crate) fn entry(out: &mut String, text: &str) {
    wrapped(out, text, "#  ", "#    ");
}

fn wrapped(out: &mut String, text: &str, first: &str, rest: &str) {
    let mut line = String::from(first);
    for word in text.split_whitespace() {
        if line.len() > rest.len() && line.len() + 1 + word.len() > WRAP {
            out.push_str(&line);
            out.push('\n');
            line = String::from(rest);
        }
        line.push(' ');
        line.push_str(word);
    }
    out.push_str(&line);
    out.push('\n');
}

fn quote(text: &str) -> String {
    let mut quoted = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\n' => quoted.push_str("\\n"),
            '\t' => quoted.push_str("\\t"),
            '\r' => quoted.push_str("\\r"),
            c if c.is_control() => {
                let _ = write!(quoted, "\\u{:04X}", u32::from(c));
            }
            c => quoted.push(c),
        }
    }
    quoted.push('"');
    quoted
}

#[cfg(test)]
mod tests {
    use super::*;

    const GLOBAL_HEADER: &str = "Area,ISO 3 code,Year,Area type,Electricity source,Is aggregated source,Generation (TWh),Emissions (MtCO2e),Emissions intensity (gCO2e/kWh)\n";
    const US_HEADER: &str = "Area,ISO 3 code,State,State code,Year,Electricity source,Is aggregated source,Generation (TWh),Emissions (MtCO2e),Emissions intensity (gCO2e/kWh)\n";

    fn global(rows: &str) -> String {
        format!("{GLOBAL_HEADER}{rows}")
    }

    fn us(rows: &str) -> String {
        format!("{US_HEADER}{rows}")
    }

    fn ids(extract: &Extract) -> Vec<&str> {
        extract.geographies.iter().map(|g| g.id.as_str()).collect()
    }

    fn find<'a>(extract: &'a Extract, id: &str) -> anyhow::Result<&'a Geography> {
        extract
            .geographies
            .iter()
            .find(|g| g.id == id)
            .with_context(|| id.to_owned())
    }

    fn meta() -> anyhow::Result<Meta> {
        Ok(Meta {
            global_url: "https://example.org/global.csv".to_owned(),
            global_sha256: sha256_hex(b"global"),
            us_url: "https://example.org/us.csv".to_owned(),
            us_sha256: sha256_hex(b"us"),
            accessed: "2026-10-06".parse()?,
        })
    }

    fn sample() -> anyhow::Result<Extract> {
        extract(
            global(
                "World,,2025,Region,Total generation,True,100,47,470.1\n\
                 Testland,TST,2025,Country or economy,Coal,False,1.0,0.9,900.0\n\
                 Testland,TST,2025,Country or economy,Fossil,True,1.0,0.9,900.0\n\
                 Testland,TST,2024,Country or economy,Total generation,True,2.0,0.6,300.5\n\
                 Testland,TST,2023,Country or economy,Total generation,True,2.0,0.7,350.25\n\
                 \"Quote \"\"Isle\"\", The\",QTE,2024,Country or economy,Total generation,True,1,0.012,12\n\
                 Below Zero,NEG,2024,Country or economy,Total generation,True,1,0.005,5.0\n\
                 Below Zero,NEG,2025,Country or economy,Hydro,False,10.0,0.24,24.0\n\
                 Below Zero,NEG,2025,Country or economy,Other fossil,False,-1.0,-0.7,700.0\n\
                 Below Zero,NEG,2025,Country or economy,Net imports,False,-0.5,0,0\n\
                 Below Zero,NEG,2025,Country or economy,Total generation,True,9.0,-0.46,-51.111\n\
                 Never Above,NVA,2025,Country or economy,Total generation,True,1,-0.01,-10.0\n\
                 Slightly Off,OFF,2025,Country or economy,Hydro,False,9.0,0.216,24.0\n\
                 Slightly Off,OFF,2025,Country or economy,Gas,False,1.0,0.5,500.0\n\
                 Slightly Off,OFF,2025,Country or economy,Other fossil,False,-0.1,-0.07,700.0\n\
                 Slightly Off,OFF,2025,Country or economy,Total generation,True,9.9,0.646,65.253\n\
                 Empty,EMP,2025,Country or economy,Total generation,True,1,,\n\
                 Empty,EMP,2024,Country or economy,Total generation,True,1,0.04,40.0\n\
                 United States,USA,2025,Country or economy,Total generation,True,4000,1520,380.0\n\
                 Alpha,ALP,2025,Country or economy,Total generation,True,1,0.1,100.0\n",
            )
            .as_bytes(),
            us(
                "United States,USA,United States,US,2025,Total generation,True,4000,1520,380.0\n\
                United States,USA,Puerto Rico,PR,2025,Total generation,True,20,13,650.0\n\
                United States,USA,Texas,TX,2025,Total generation,True,500,195,390.5\n\
                United States,USA,Alaska,AK,2024,Total generation,True,6,2.5,420.0\n",
            )
            .as_bytes(),
        )
    }

    #[test]
    fn regions_aggregates_and_us_rows_for_countries_are_left_out() -> anyhow::Result<()> {
        let extract = sample()?;
        assert_eq!(
            ids(&extract),
            [
                "ember:ALP",
                "ember:EMP",
                "ember:NEG",
                "ember:OFF",
                "ember:QTE",
                "ember:TST",
                "ember:US-AK",
                "ember:US-TX",
                "ember:USA"
            ]
        );
        assert_eq!(find(&extract, "ember:USA")?.source, GLOBAL_SOURCE);
        Ok(())
    }

    #[test]
    fn each_geography_keeps_its_latest_year_with_a_total() -> anyhow::Result<()> {
        let extract = sample()?;
        let testland = find(&extract, "ember:TST")?;
        assert_eq!((testland.year, testland.value.as_str()), (2024, "300.5"));
        let empty = find(&extract, "ember:EMP")?;
        assert_eq!((empty.year, empty.value.as_str()), (2024, "40.0"));
        let alaska = find(&extract, "ember:US-AK")?;
        assert_eq!((alaska.year, alaska.name.as_str()), (2024, "Alaska"));
        Ok(())
    }

    #[test]
    fn a_negative_total_falls_back_to_the_latest_usable_year_and_is_listed() -> anyhow::Result<()> {
        let extract = sample()?;
        let below = find(&extract, "ember:NEG")?;
        assert_eq!((below.year, below.value.as_str()), (2024, "5.0"));
        let text = render(&extract, &meta()?);
        assert!(
            text.contains("#   ember:NEG Below Zero, 2025: -51.111 g CO2e per kWh; 2024 used\n"),
            "{text}"
        );
        Ok(())
    }

    #[test]
    fn a_geography_without_a_usable_year_is_left_out_and_listed() -> anyhow::Result<()> {
        let extract = sample()?;
        assert!(!ids(&extract).contains(&"ember:NVA"));
        let text = render(&extract, &meta()?);
        assert!(
            text.contains(
                "#   ember:NVA Never Above, 2025: -10.0 g CO2e per kWh; no usable year\n"
            ),
            "{text}"
        );
        Ok(())
    }

    #[test]
    fn a_kept_year_with_a_negative_fuel_is_listed_with_the_other_fuels_alone() -> anyhow::Result<()>
    {
        let extract = sample()?;
        let off = find(&extract, "ember:OFF")?;
        assert_eq!((off.year, off.value.as_str()), (2025, "65.253"));
        let text = render(&extract, &meta()?);
        assert!(
            text.contains(
                "#   ember:OFF Slightly Off, 2025: 65.253; Other fossil -0.1 TWh; 71.6\n#     without it\n"
            ),
            "{text}"
        );
        assert!(
            !text.contains("Net imports"),
            "imports are trade, not a fuel"
        );
        Ok(())
    }

    #[test]
    fn the_output_is_toml_with_escaped_names_sorted_ids_and_dates() -> anyhow::Result<()> {
        let text = render(&sample()?, &meta()?);
        let table: toml::Table = text.parse()?;
        let rows = table["geography"].as_array().context("geography array")?;
        let ids: Vec<_> = rows.iter().filter_map(|row| row["id"].as_str()).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        assert_eq!(ids, sorted);
        let quoted = rows
            .iter()
            .find(|row| row["id"].as_str() == Some("ember:QTE"));
        let quoted = quoted.context("quoted row")?;
        assert_eq!(quoted["name"].as_str(), Some("Quote \"Isle\", The"));
        assert_eq!(quoted["g_co2e_per_kwh"].as_float(), Some(12.0));
        assert!(quoted["reviewed"].as_datetime().is_some());
        assert!(
            table["sources"]["ember-us"]["accessed"]
                .as_datetime()
                .is_some()
        );
        assert!(
            text.lines()
                .all(|line| !line.starts_with('#') || line.len() <= WRAP)
        );
        Ok(())
    }

    #[test]
    fn values_are_written_as_published() -> anyhow::Result<()> {
        let text = render(&sample()?, &meta()?);
        assert!(text.contains("g_co2e_per_kwh = 300.5,"));
        assert!(text.contains("g_co2e_per_kwh = 390.5,"));
        Ok(())
    }

    #[test]
    fn nothing_rejected_or_negative_says_none() -> anyhow::Result<()> {
        let extract = extract(
            global("Alpha,ALP,2025,Country or economy,Total generation,True,1,0.1,100.0\n")
                .as_bytes(),
            us("").as_bytes(),
        )?;
        let text = render(&extract, &meta()?);
        assert!(text.contains("# Rejected: none\n"));
        assert!(text.contains("# Kept with a negative fuel: none\n"));
        Ok(())
    }

    fn error(global_rows: &str) -> Option<String> {
        extract(global(global_rows).as_bytes(), us("").as_bytes())
            .map(|_| ())
            .err()
            .map(|e| format!("{e:#}"))
    }

    #[test]
    fn an_unparsable_intensity_is_an_error_naming_the_row() {
        let message = error(
            "Alpha,ALP,2025,Country or economy,Total generation,True,1,0.1,100.0\n\
             Beta,BET,2025,Country or economy,Total generation,True,1,0.1,n/a\n",
        );
        assert!(
            message
                .as_deref()
                .is_some_and(|m| m.contains("line 3 of the global file") && m.contains("\"n/a\"")),
            "{message:?}"
        );
    }

    #[test]
    fn an_unparsable_generation_is_an_error_naming_the_row() {
        let message = error("Alpha,ALP,2025,Country or economy,Hydro,False,lots,0.1,24.0\n");
        assert!(
            message
                .as_deref()
                .is_some_and(|m| m.contains("line 2 of the global file") && m.contains("\"lots\"")),
            "{message:?}"
        );
    }

    #[test]
    fn a_repeated_id_in_one_year_is_an_error() {
        let message = error(
            "Alpha,ALP,2025,Country or economy,Total generation,True,1,0.1,100.0\n\
             Alpha again,ALP,2025,Country or economy,Total generation,True,1,0.1,101.0\n",
        );
        assert!(
            message
                .as_deref()
                .is_some_and(|m| m.contains("repeats ember:ALP")),
            "{message:?}"
        );
    }

    #[test]
    fn decimals_are_checked_against_what_toml_accepts() {
        for valid in ["0", "0.5", "412", "-36.353"] {
            assert!(is_decimal(valid), "{valid}");
        }
        for invalid in ["", "-", ".5", "5.", "1e3", "007", "NaN", "inf", "1,5"] {
            assert!(!is_decimal(invalid), "{invalid}");
        }
    }
}
