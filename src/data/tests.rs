use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use super::load::{load_from, misfit, read_dir, sha256};
use super::*;

const MANIFEST_TOML: &str = r#"
id = "2026-10-06"
review_interval_days = 90
price_review_interval_days = 30

[sources.study]
title = "A study"
url = "https://example.org/study"
publisher = "Example"
accessed = 2026-10-06
"#;

const FINDINGS_TOML: &str = r#"
[[finding]]
id = "long-context"
cached_tokens = 200000
label = "D"
sources = ["study"]
basis = "A default"
reviewed = 2026-10-06
"#;

fn factor(values: &str) -> String {
    let [
        envelope_low,
        plausible_low,
        central,
        plausible_high,
        envelope_high,
    ] = values.split(' ').collect::<Vec<_>>()[..]
    else {
        panic!("five values expected");
    };
    format!(
        "envelope_low = {envelope_low}\nplausible_low = {plausible_low}\ncentral = {central}\nplausible_high = {plausible_high}\nenvelope_high = {envelope_high}\nlabel = \"D\"\nsources = [\"study\"]\nbasis = \"b\"\n"
    )
}

fn parameter_set(id: &str, idle: &str) -> String {
    let mut text =
        format!("[[parameter_set]]\nid = \"{id}\"\ndescription = \"d\"\nreviewed = 2026-10-06\n");
    for name in [
        "a_p_uj_per_pair",
        "a_d_uj_per_pair",
        "b_in_j_per_token",
        "b_out_j_per_token",
        "c0_mj_per_cached_token",
    ] {
        let _ = write!(text, "[parameter_set.{name}]\n{}", factor("1 2 3 4 5"));
    }
    let _ = write!(text, "[parameter_set.idle]\n{}", factor(idle));
    text
}

fn model(id: &str, pattern: &str) -> String {
    format!(
        "[[model]]\nid = \"{id}\"\npattern = \"{pattern}\"\nparameter_set = \"frontier\"\nweights = \"closed\"\nlabel = \"C\"\nsources = [\"study\"]\nbasis = \"b\"\nreviewed = 2026-10-06\n"
    )
}

fn provider(id: &str, sites: &str, envelope: &str, pue: &str) -> String {
    format!(
        "[[provider]]\nid = \"{id}\"\nsites = {sites}\nenvelope_sites = {envelope}\nlabel = \"D\"\nsources = [\"study\"]\nbasis = \"b\"\nreviewed = 2026-10-06\n[provider.pue]\n{}",
        factor(pue)
    )
}

fn geography(id: &str, value: &str, reviewed: &str) -> String {
    format!(
        "[[geography]]\nid = \"{id}\"\nname = \"{id}\"\nyear = 2025\ng_co2e_per_kwh = {value}\nbasis = \"lifecycle\"\nlabel = \"I\"\nsources = [\"study\"]\nreviewed = {reviewed}\n"
    )
}

fn calibration(id: &str, wh_per_usd: &str, markup: &str) -> String {
    format!(
        "[[price_calibration]]\nid = \"{id}\"\ndescription = \"d\"\nreviewed = 2026-10-06\nchat_input_tokens = 1000\nchat_output_tokens = 500\nanchor_in_j = 0.23\nanchor_out_j = 0.38\n[price_calibration.wh_per_usd]\n{}[price_calibration.markup]\n{}",
        factor(wh_per_usd),
        factor(markup)
    )
}

/// A price of `input` and `output` USD per million tokens with tiers of
/// (threshold, input, output).
fn price(id: &str, input: f64, output: f64, tiers: &[(u64, f64, f64)]) -> String {
    let mut text = format!(
        "[[price]]\nid = \"{id}\"\ncalibration = \"hosts\"\ninput_usd_per_mtok = {input:?}\noutput_usd_per_mtok = {output:?}\nlabel = \"C\"\nsources = [\"study\"]\nbasis = \"b\"\nreviewed = 2026-10-06\n"
    );
    for (above, input, output) in tiers {
        let _ = write!(
            text,
            "[[price.tier]]\nabove_prompt_tokens = {above}\ninput_usd_per_mtok = {input:?}\noutput_usd_per_mtok = {output:?}\n"
        );
    }
    text
}

fn priced_model(id: &str, pattern: &str, price: &str) -> String {
    model(id, pattern).replacen(
        "parameter_set = \"frontier\"\n",
        &format!("parameter_set = \"frontier\"\nprice = \"{price}\"\n"),
        1,
    )
}

/// A calibration, a price with one tier and a model that names it.
fn priced() -> String {
    calibration("hosts", "10 20 30 40 50", "1 1 2 4 4")
        + &price("p", 4.0, 20.0, &[(100, 8.0, 30.0)])
        + &priced_model("priced", "priced-model", "p")
}

struct Shipped {
    findings: &'static str,
    energy: String,
    providers: String,
    grid: String,
}

impl Shipped {
    fn new() -> Shipped {
        Shipped {
            findings: FINDINGS_TOML,
            energy: parameter_set("frontier", "1.1 1.1 1.2 1.3 1.5")
                + &model("claude", "claude-*")
                + &model("claude-opus", "claude-opus-*")
                + &model("exact", "claude-opus-5"),
            providers: provider(
                "alpha",
                r#"["g:a", "g:b"]"#,
                r#"["g:c"]"#,
                "1.1 1.1 1.2 1.3 1.5",
            ) + &provider("beta", r#"["g:d"]"#, "[]", "1.1 1.1 1.2 1.3 1.5")
                + &provider("unknown", r#""named""#, r#""all""#, "1.1 1.1 1.2 1.3 1.5"),
            grid: geography("g:a", "100", "2026-10-06")
                + &geography("g:b", "300", "2026-10-06")
                + &geography("g:c", "20", "2026-10-06")
                + &geography("g:d", "500", "2026-10-06")
                + &geography("g:e", "900", "2026-10-06"),
        }
    }

    fn load(&self, overrides: &[(&str, &str)]) -> Result<Data, Error> {
        let shipped = [
            ("data.toml", MANIFEST_TOML),
            ("energy.toml", self.energy.as_str()),
            ("providers.toml", self.providers.as_str()),
            ("grid.toml", self.grid.as_str()),
            ("findings.toml", self.findings),
        ];
        let overrides: Vec<(String, String)> = overrides
            .iter()
            .map(|(path, text)| ((*path).to_owned(), (*text).to_owned()))
            .collect();
        load_from(&shipped, &overrides)
    }
}

fn problem(result: Result<Data, Error>) -> String {
    match result {
        Ok(_) => panic!("the data loaded"),
        Err(error) => error.to_string(),
    }
}

#[test]
fn the_shipped_data_loads_and_every_source_has_a_url() -> Result<(), Error> {
    let data = load(None)?;
    assert!(data.parameter_sets.contains_key(FALLBACK_PARAMETER_SET));
    assert!(data.providers.contains_key(FALLBACK_PROVIDER));
    for (id, source) in &data.sources {
        assert!(source.url.is_some(), "source {id} has no url");
    }
    Ok(())
}

#[test]
fn open_weight_models_resolve_to_the_sets_of_their_configurations() -> Result<(), Error> {
    let data = load(None)?;
    for (name, set) in [
        ("deepseek-ai/DeepSeek-V4-Pro", "deepseek-v4-pro"),
        ("deepseek-v4-pro", "deepseek-v4-pro"),
        ("deepseek-ai/DeepSeek-V4.1-Flash", "deepseek-v4.1-flash"),
        ("deepseek-flash", "deepseek-v4.1-flash"),
        ("zai-org/GLM-5.3", "glm-5.3"),
        ("glm-5.3", "glm-5.3"),
        ("zai-glm-5-3", "glm-5.3"),
        ("moonshotai/Kimi-K3", "kimi-k3"),
        ("kimi-k3", "kimi-k3"),
        ("gpt-oss-120b", "gpt-oss-120b"),
        ("openai/gpt-oss-120b", "gpt-oss-120b"),
        ("Qwen/Qwen3.5-397B-A17B", "qwen3.5-397b-a17b"),
        ("Qwen/Qwen3.8-2.4T-A95B", "qwen3.8-2.4t-a95b"),
        (
            "meta-llama/Llama-3.3-70B-Instruct",
            "llama-3.3-70b-instruct",
        ),
        ("Llama-3.3-70B-Instruct", "llama-3.3-70b-instruct"),
        ("llama-3.3-70b-versatile", "llama-3.3-70b-instruct"),
        ("MiniMaxAI/MiniMax-M3", "minimax-m3"),
        ("MiniMax-M3", "minimax-m3"),
        ("XiaomiMiMo/MiMo-V2.6-Flash-RL", "mimo-v2.6-flash"),
        ("xiaomi/mimo-v2.6-flash", "mimo-v2.6-flash"),
        // Models without a configuration of their own keep frontier.
        ("gpt-oss-20b", "frontier"),
        ("glm-5.2", "frontier"),
        ("kimi-k2.6", "frontier"),
    ] {
        let found = data.parameter_set(name, 0);
        assert!(found.matched, "{name}");
        assert_eq!(found.record.id, set, "{name}");
    }
    assert!(!data.parameter_set("glm-5.3-flash", 0).matched);
    Ok(())
}

#[test]
fn every_file_in_the_data_directory_is_built_in() {
    fn walk(dir: &Path, base: &Path, found: &mut Vec<String>) {
        for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, base, found);
            } else if path.file_name().is_some_and(|name| name != "README.md") {
                let relative = path.strip_prefix(base).unwrap_or(&path);
                found.push(relative.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("data");
    let mut found = Vec::new();
    walk(&base, &base, &mut found);
    found.sort();
    let mut shipped: Vec<String> = SHIPPED.iter().map(|(path, _)| (*path).to_owned()).collect();
    shipped.sort();
    assert_eq!(found, shipped);
}

#[test]
fn the_version_names_every_shipped_file_by_its_sha256() -> Result<(), Error> {
    let data = load(None)?;
    for ((path, text), file) in SHIPPED.iter().zip(&data.version.files) {
        assert_eq!(file.path, format!("data/{path}"));
        assert_eq!(file.sha256, sha256(text.as_bytes()));
    }
    assert_eq!(
        sha256(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    Ok(())
}

#[test]
fn the_shipped_sha256_is_the_hash_of_the_file_listing_and_changes_with_any_file()
-> Result<(), Error> {
    let data = load(None)?;
    let mut listing = String::new();
    for file in &data.version.files {
        let _ = writeln!(listing, "{} {}", file.path, file.sha256);
    }
    assert_eq!(data.version.shipped_sha256, sha256(listing.as_bytes()));

    let edited = format!("{}\n# edited\n", SHIPPED[1].1);
    let mut changed: Vec<(&str, &str)> = SHIPPED.to_vec();
    changed[1].1 = &edited;
    let changed = load_from(&changed, &[])?;
    assert_ne!(changed.version.shipped_sha256, data.version.shipped_sha256);
    Ok(())
}

#[test]
fn a_provider_grid_is_the_mean_and_range_of_its_sites_and_envelope() -> Result<(), Error> {
    let data = Shipped::new().load(&[])?;
    let alpha = &data.providers["alpha"].grid;
    assert_eq!(alpha.central.0, 200.0);
    assert_eq!(alpha.plausible_low.0, 100.0);
    assert_eq!(alpha.plausible_high.0, 300.0);
    assert_eq!(alpha.envelope_low.0, 20.0);
    assert_eq!(alpha.envelope_high.0, 300.0);
    Ok(())
}

#[test]
fn a_one_site_provider_without_envelope_sites_has_five_equal_values() -> Result<(), Error> {
    let data = Shipped::new().load(&[(
        "self-hosted.toml",
        &provider("ollama", r#"["g:c"]"#, "[]", "1.2 1.2 1.2 1.2 1.2"),
    )])?;
    let grid = data.providers["ollama"].grid;
    assert_eq!(grid.values().map(|value| value.0), [20.0; 5]);
    Ok(())
}

#[test]
fn unknown_takes_every_named_site_and_every_geography_for_its_envelope() -> Result<(), Error> {
    let data = Shipped::new().load(&[])?;
    let unknown = &data.providers["unknown"];
    assert_eq!(unknown.site_ids, ["g:a", "g:b", "g:d"]);
    assert_eq!(unknown.grid.central.0, 300.0);
    assert_eq!(unknown.grid.plausible_low.0, 100.0);
    assert_eq!(unknown.grid.plausible_high.0, 500.0);
    assert_eq!(unknown.grid.envelope_low.0, 20.0);
    assert_eq!(
        unknown.grid.envelope_high.0, 900.0,
        "g:e is outside every shipped provider's lists"
    );
    Ok(())
}

#[test]
fn a_listed_envelope_covers_every_geography_a_named_provider_lists() -> Result<(), Error> {
    let data = Shipped::new().load(&[(
        "gamma.toml",
        &provider("gamma", r#"["g:b"]"#, r#""listed""#, "1.1 1.1 1.2 1.3 1.5"),
    )])?;
    let grid = data.providers["gamma"].grid;
    assert_eq!(grid.envelope_low.0, 20.0, "g:c is alpha's envelope site");
    assert_eq!(
        grid.envelope_high.0, 500.0,
        "g:d is beta's site; g:e no one lists"
    );
    Ok(())
}

#[test]
fn a_users_own_provider_and_geography_change_no_other_provider() -> Result<(), Error> {
    let home = geography("g:far", "2000", "2026-10-01")
        + &provider("home", r#"["g:far"]"#, "[]", "1.1 1.1 1.2 1.3 1.5");
    let gamma = provider("gamma", r#"["g:b"]"#, r#""listed""#, "1.1 1.1 1.2 1.3 1.5");
    let data = Shipped::new().load(&[("gamma.toml", &gamma), ("home.toml", &home)])?;
    assert_eq!(data.providers["home"].grid.central.0, 2000.0);
    assert_eq!(
        data.providers["gamma"].grid.envelope_high.0, 500.0,
        "listed is the shipped providers' geographies"
    );
    let unknown = &data.providers["unknown"];
    assert_eq!(unknown.site_ids, ["g:a", "g:b", "g:d"]);
    assert_eq!(unknown.grid.central.0, 300.0);
    assert_eq!(
        unknown.grid.envelope_high.0, 900.0,
        "all is the shipped grid data"
    );
    Ok(())
}

#[test]
fn an_unnamed_provider_uses_unknown_and_is_reported_unmatched() -> Result<(), Error> {
    let data = Shipped::new().load(&[])?;
    let found = data.provider("alpha");
    assert!(found.matched);
    let fallback = data.provider("litellm");
    assert!(!fallback.matched);
    assert_eq!(fallback.record.provider.id, "unknown");
    Ok(())
}

#[test]
fn an_exact_pattern_wins_then_the_longest_prefix() -> Result<(), Error> {
    let data = Shipped::new().load(&[])?;
    let id = |name: &str| data.model(name).map(|model| model.id.clone());
    assert_eq!(id("claude-opus-5"), Some("exact".to_owned()));
    assert_eq!(id("claude-opus-5-5"), Some("claude-opus".to_owned()));
    assert_eq!(id("claude-sonnet-5"), Some("claude".to_owned()));
    assert_eq!(id("gpt-6"), None);
    Ok(())
}

#[test]
fn an_unmatched_model_uses_the_fallback_set() -> Result<(), Error> {
    let data = Shipped::new().load(&[])?;
    let found = data.parameter_set("gpt-6", 0);
    assert!(!found.matched);
    assert_eq!(found.record.id, FALLBACK_PARAMETER_SET);
    assert!(data.parameter_set("claude-x", 0).matched);
    Ok(())
}

#[test]
fn two_models_with_the_same_pattern_are_refused() {
    let mut shipped = Shipped::new();
    shipped.energy.push_str(&model("again", "claude-*"));
    assert!(problem(shipped.load(&[])).contains("the same pattern as model"));
}

#[test]
fn a_pattern_with_a_star_inside_is_refused() {
    let mut shipped = Shipped::new();
    shipped.energy.push_str(&model("odd", "claude-*-5"));
    assert!(problem(shipped.load(&[])).contains("use an exact name or a prefix"));
}

#[test]
fn an_override_replaces_a_record_whole_and_adds_new_ones() -> Result<(), Error> {
    let text = geography("g:a", "50", "2026-10-01") + &geography("g:new", "10", "2026-10-01");
    let data = Shipped::new().load(&[("mine.toml", &text)])?;
    assert_eq!(data.geographies["g:a"].g_co2e_per_kwh.0, 50.0);
    assert_eq!(
        data.providers["alpha"].grid.central.0, 175.0,
        "the replaced geography reaches the provider"
    );
    let records = &data.version.overrides[0].records;
    assert_eq!(records[0].id, "g:a");
    assert_eq!(records[0].action, Action::Replaced);
    assert_eq!(records[1].id, "g:new");
    assert_eq!(records[1].action, Action::Added);
    Ok(())
}

#[test]
fn the_version_changes_with_an_override_and_is_stable_for_the_same_inputs() -> Result<(), Error> {
    let shipped = Shipped::new();
    let first = geography("g:a", "50", "2026-10-01");
    let second = geography("g:a", "51", "2026-10-01");
    let hash = |text: &str| -> Result<String, Error> {
        Ok(shipped.load(&[("mine.toml", text)])?.version.overrides[0]
            .file
            .sha256
            .clone())
    };
    assert_eq!(hash(&first)?, hash(&first)?);
    assert_ne!(hash(&first)?, hash(&second)?);
    Ok(())
}

#[test]
fn the_same_record_in_two_override_files_is_refused() {
    let text = geography("g:a", "50", "2026-10-01");
    let message = problem(Shipped::new().load(&[("a.toml", &text), ("b.toml", &text)]));
    assert!(message.starts_with("data file b.toml: geography g:a is also set by a.toml"));
    assert!(message.ends_with("fix or remove the file"));
}

#[test]
fn the_same_record_twice_in_one_file_is_refused() {
    let text = geography("g:x", "50", "2026-10-01") + &geography("g:x", "60", "2026-10-01");
    assert!(
        problem(Shipped::new().load(&[("a.toml", &text)])).contains("geography g:x appears twice")
    );
}

#[test]
fn the_same_record_in_two_shipped_files_is_refused() {
    let mut shipped = Shipped::new();
    shipped
        .providers
        .push_str(&geography("g:a", "1", "2026-10-06"));
    assert!(
        problem(shipped.load(&[]))
            .starts_with("data file data/grid.toml: geography g:a is also in data/providers.toml")
    );
}

#[test]
fn an_override_may_not_set_the_version() {
    let message = problem(Shipped::new().load(&[("a.toml", "id = \"mine\"\n")]));
    assert!(message.contains("set by the shipped data only"));
}

#[test]
fn an_unknown_key_names_the_file_and_the_record_with_its_line() {
    let text = geography("g:a", "50", "2026-10-01") + "colour = \"red\"\n";
    let message = problem(Shipped::new().load(&[("a.toml", &text)]));
    assert!(
        message.starts_with("data file a.toml: line 1: geography g:a: unknown field `colour`"),
        "{message}"
    );
}

#[test]
fn of_two_invalid_records_the_first_is_named_with_its_own_error() {
    let text = geography("g:a", "50", "2026-10-01")
        + &parameter_set("frontier", "1 1 1.1 1.2 1.3")
        + &geography("g:b", "50", "2026-10-01")
        + "colour = \"red\"\n";
    let text = text.replace("[parameter_set.idle]", "[parameter_set.k]");
    let header = text
        .lines()
        .position(|line| line == "[[parameter_set]]")
        .map_or(0, |index| index + 1);
    let message = problem(Shipped::new().load(&[("a.toml", &text)]));
    assert!(
        message.contains(&format!(
            "line {header}: parameter set frontier: unknown field `k`"
        )),
        "{message}"
    );
}

#[test]
fn an_unknown_table_after_a_record_is_not_blamed_on_the_record() {
    let text = geography("g:a", "50", "2026-10-01") + "[bogus]\nx = 1\n";
    let line = text.lines().count() - 1;
    let message = problem(Shipped::new().load(&[("a.toml", &text)]));
    assert!(
        message.starts_with(&format!(
            "data file a.toml: line {line}: unknown field `bogus`"
        )),
        "{message}"
    );
}

#[test]
fn a_parameter_set_with_k_instead_of_idle_is_named_with_its_line() {
    let text = parameter_set("frontier", "1 1 1.1 1.2 1.3")
        .replace("[parameter_set.idle]", "[parameter_set.k]");
    let line = text
        .lines()
        .position(|line| line == "[[parameter_set]]")
        .map_or(0, |index| index + 1);
    let message = problem(Shipped::new().load(&[("a.toml", &text)]));
    assert!(
        message.contains(&format!(
            "line {line}: parameter set frontier: unknown field `k`"
        )),
        "{message}"
    );
}

#[test]
fn a_value_without_a_source_is_refused() {
    let text =
        geography("g:a", "50", "2026-10-01").replace("sources = [\"study\"]", "sources = []");
    assert!(
        problem(Shipped::new().load(&[("a.toml", &text)]))
            .contains("geography g:a names no source")
    );
}

#[test]
fn a_source_that_no_file_defines_is_refused() {
    let text = geography("g:a", "50", "2026-10-01").replace("\"study\"", "\"missing\"");
    assert!(
        problem(Shipped::new().load(&[("a.toml", &text)]))
            .contains("names source missing, which no data file defines")
    );
}

#[test]
fn a_shipped_problem_says_the_shipped_data_must_be_fixed() {
    let mut shipped = Shipped::new();
    shipped.grid = shipped.grid.replace("\"study\"", "\"missing\"");
    assert!(problem(shipped.load(&[])).ends_with("the shipped data must be fixed in tokendraw"));
}

#[test]
fn values_out_of_order_are_refused() {
    let text = provider("gamma", r#"["g:a"]"#, "[]", "1.1 1.3 1.2 1.3 1.5");
    assert!(problem(Shipped::new().load(&[("a.toml", &text)])).contains("out of order"));
}

#[test]
fn a_pue_or_idle_factor_below_one_is_refused() {
    let text = provider("gamma", r#"["g:a"]"#, "[]", "0.9 1.1 1.2 1.3 1.5");
    assert!(
        problem(Shipped::new().load(&[("a.toml", &text)]))
            .contains("provider gamma, pue has the value 0.9")
    );
    let text = parameter_set("frontier", "0.5 1.1 1.2 1.3 1.5");
    assert!(
        problem(Shipped::new().load(&[("a.toml", &text)]))
            .contains("parameter set frontier, idle has the value 0.5")
    );
}

#[test]
fn a_negative_or_non_finite_value_is_refused() {
    let text = geography("g:a", "-1", "2026-10-01");
    assert!(
        problem(Shipped::new().load(&[("a.toml", &text)]))
            .contains("it must be a finite number, at least 0")
    );
    let text = geography("g:a", "nan", "2026-10-01");
    assert!(
        problem(Shipped::new().load(&[("a.toml", &text)])).contains("it must be a finite number")
    );
    let text = parameter_set("frontier", "1.1 1.1 1.2 1.3 inf");
    assert!(problem(Shipped::new().load(&[("a.toml", &text)])).contains("has the value inf"));
    let text = parameter_set("frontier", "1.1 1.1 1.2 1.3 1.5").replacen(
        "envelope_low = 1\n",
        "envelope_low = -1\n",
        1,
    );
    assert!(problem(Shipped::new().load(&[("a.toml", &text)])).contains("has the value -1"));
}

#[test]
fn a_provider_without_sites_is_refused() {
    let text = provider("gamma", "[]", "[]", "1.1 1.1 1.2 1.3 1.5");
    assert!(
        problem(Shipped::new().load(&[("a.toml", &text)])).contains("provider gamma has no sites")
    );
}

#[test]
fn a_site_that_no_file_defines_is_refused() {
    let text = provider("gamma", r#"["g:zz"]"#, "[]", "1.1 1.1 1.2 1.3 1.5");
    assert!(
        problem(Shipped::new().load(&[("a.toml", &text)]))
            .contains("provider gamma names geography g:zz, which no data file defines")
    );
}

#[test]
fn a_parameter_set_that_no_file_defines_is_refused() {
    let text =
        model("x", "x-*").replace("parameter_set = \"frontier\"", "parameter_set = \"none\"");
    assert!(
        problem(Shipped::new().load(&[("a.toml", &text)])).contains("names parameter set none")
    );
}

#[test]
fn a_date_with_a_time_is_refused() {
    let text = geography("g:a", "50", "2026-10-01T10:00:00");
    assert!(
        problem(Shipped::new().load(&[("a.toml", &text)]))
            .contains("expected a date such as 2026-10-06")
    );
}

#[test]
fn a_record_is_stale_only_after_the_review_interval() -> Result<(), Error> {
    let text = geography("g:a", "50", "2026-07-01");
    let data = Shipped::new().load(&[("a.toml", &text)])?;
    let on = |day: &str| -> Vec<String> {
        let today: Date = day.parse().unwrap_or_default();
        data.stale(today)
            .into_iter()
            .map(|stale| stale.id)
            .collect()
    };
    assert!(on("2026-09-29").is_empty(), "90 days after the review");
    assert_eq!(on("2026-09-30"), ["g:a"]);
    Ok(())
}

#[test]
fn an_override_directory_is_read_in_file_name_order_and_may_be_missing()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    assert_eq!(read_dir(&dir.path().join("missing"))?, Vec::new());
    fs::write(dir.path().join("b.toml"), "")?;
    fs::write(dir.path().join("a.toml"), "")?;
    fs::write(dir.path().join("notes.txt"), "")?;
    let names: Vec<String> = read_dir(dir.path())?
        .into_iter()
        .map(|(path, _)| path)
        .collect();
    assert_eq!(names.len(), 2);
    assert!(names[0].ends_with("a.toml") && names[1].ends_with("b.toml"));
    Ok(())
}

#[test]
fn export_writes_the_shipped_files_byte_for_byte_and_never_overwrites()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let written = export(dir.path())?;
    assert_eq!(written.len(), SHIPPED.len());
    for (path, text) in SHIPPED {
        assert_eq!(fs::read_to_string(dir.path().join(path))?, text);
    }
    assert!(export(dir.path()).is_err());
    Ok(())
}

/// The shipped providers' grid intensities worked by hand from their sites
/// and the grid data, in grams of `CO2e` per kWh: envelope low, plausible
/// low, central, plausible high, envelope high.
#[test]
fn the_shipped_provider_grids_match_the_hand_worked_figures() -> Result<(), Error> {
    let data = load(None)?;
    let expected = [
        (
            "anthropic",
            [27.756, 267.563, 1602.607 / 4.0, 642.875, 674.293],
        ),
        (
            "openai",
            [27.756, 311.362, 1890.065 / 5.0, 529.942, 674.293],
        ),
        ("nebius", [27.756, 57.242, 57.242, 57.242, 674.293]),
        ("moonshot", [27.756, 528.278, 528.278, 528.278, 674.293]),
        (
            "unknown",
            [21.429, 57.242, 3386.023 / 9.0, 642.875, 1306.15],
        ),
    ];
    for (id, values) in expected {
        let grid = data.providers[id].grid.values().map(|value| value.0);
        for (computed, worked) in grid.iter().zip(values) {
            assert!(
                (computed - worked).abs() < 1e-9,
                "{id}: {grid:?}, worked {values:?}"
            );
        }
    }
    Ok(())
}

#[test]
fn every_shipped_record_deserializes_on_its_own() {
    for (path, text) in SHIPPED {
        assert_eq!(misfit(text), None, "{path}");
    }
}

#[test]
fn a_spend_based_label_is_refused_on_every_kind_of_record() {
    let spend = |text: String, from: &str| text.replacen(from, "label = \"S\"", 1);
    let cases = [
        (
            spend(
                parameter_set("frontier", "1.1 1.1 1.2 1.3 1.5"),
                "label = \"D\"",
            ),
            "parameter set frontier, a_p_uj_per_pair has the label S",
        ),
        (
            spend(model("claude", "claude-*"), "label = \"C\""),
            "model claude has the label S",
        ),
        (
            spend(
                provider("beta", r#"["g:d"]"#, "[]", "1.1 1.1 1.2 1.3 1.5"),
                "label = \"D\"",
            ),
            "provider beta has the label S",
        ),
        (
            // The PUE factor is the provider's last table.
            provider("beta", r#"["g:d"]"#, "[]", "1.1 1.1 1.2 1.3 1.5")
                .rsplit_once("label = \"D\"")
                .map(|(before, after)| format!("{before}label = \"S\"{after}"))
                .unwrap_or_default(),
            "provider beta, pue has the label S",
        ),
        (
            spend(geography("g:a", "100", "2026-10-06"), "label = \"I\""),
            "geography g:a has the label S",
        ),
    ];
    for (text, expected) in cases {
        let problem = problem(Shipped::new().load(&[("a.toml", &text)]));
        assert!(problem.contains(expected), "{problem}");
        assert!(problem.contains("spend-based"), "{problem}");
    }
}

fn close(actual: f64, expected: f64) -> bool {
    (actual - expected).abs() <= 1e-12 * expected.abs().max(1.0)
}

#[test]
fn a_priced_model_gets_weight_terms_from_its_list_price() -> Result<(), Error> {
    let data = Shipped::new().load(&[("a.toml", &priced())])?;
    let found = data.parameter_set("priced-model", 0);
    assert!(found.matched);
    let set = found.record;
    assert_eq!(set.id, "frontier@p");
    // The reference call costs (1000 x 4 + 500 x 20) / 1e6 = 0.014 USD, so
    // central 0.014 x 30 / 2 x 3600 = 756 J, of which 0.23 / (1000 x 0.23
    // + 500 x 0.38) = 0.23 / 420 per new input token and 0.38 / 420 per
    // output token. The low values divide by the high markup, 4, and the
    // high values by the low one, 1: 126 J and 2520 J.
    let b_in = set.b_in_j_per_token.five();
    let b_out = set.b_out_j_per_token.five();
    assert!(close(b_in.central, 756.0 * 0.23 / 420.0));
    assert!(close(b_out.central, 756.0 * 0.38 / 420.0));
    assert!(close(b_in.envelope_low, 126.0 * 0.23 / 420.0));
    assert!(close(
        b_in.plausible_low,
        0.014 * 20.0 / 4.0 * 3600.0 * 0.23 / 420.0
    ));
    assert!(close(
        b_in.plausible_high,
        0.014 * 40.0 * 3600.0 * 0.23 / 420.0
    ));
    assert!(close(b_out.envelope_high, 2520.0 * 0.38 / 420.0));
    // Attention, reload and idle are the base set's.
    let frontier = &data.parameter_sets[FALLBACK_PARAMETER_SET];
    assert_eq!(set.a_p_uj_per_pair.five(), frontier.a_p_uj_per_pair.five());
    assert_eq!(set.idle.five(), frontier.idle.five());
    // The weakest label of the price (C) and the calibration (D), and the
    // sources of both.
    assert_eq!(set.b_in_j_per_token.label, Label::D);
    assert_eq!(set.b_in_j_per_token.sources, ["study"]);
    assert!(set.b_in_j_per_token.basis.contains("price p"));
    assert_eq!(
        data.set("frontier@p").map(|set| set.id.as_str()),
        Some("frontier@p")
    );
    Ok(())
}

#[test]
fn a_price_tier_applies_above_its_threshold() -> Result<(), Error> {
    let data = Shipped::new().load(&[("a.toml", &priced())])?;
    assert_eq!(data.prompt_thresholds(), [100]);
    let base = data.parameter_set("priced-model", 0).record;
    let tier = data.parameter_set("priced-model", 100).record;
    assert_eq!(tier.id, "frontier@p>100");
    assert_eq!(
        data.parameter_set("priced-model", 500).record.id,
        "frontier@p>100"
    );
    // Input price 8 over 4, output 30 over 20.
    assert!(close(
        tier.b_in_j_per_token.central,
        base.b_in_j_per_token.central * 2.0
    ));
    assert!(close(
        tier.b_out_j_per_token.central,
        base.b_out_j_per_token.central * 1.5
    ));
    // A model without a price ignores the threshold.
    assert_eq!(data.parameter_set("claude-x", 100).record.id, "frontier");
    Ok(())
}

#[test]
fn an_override_of_a_price_reaches_its_derived_sets() -> Result<(), Error> {
    let mut shipped = Shipped::new();
    shipped.energy += &priced();
    let data = shipped.load(&[("b.toml", &price("p", 8.0, 40.0, &[]))])?;
    assert_eq!(data.prompt_thresholds(), Vec::<u64>::new());
    let set = data.parameter_set("priced-model", 100).record;
    assert_eq!(set.id, "frontier@p");
    assert!(close(
        set.b_in_j_per_token.central,
        2.0 * 756.0 * 0.23 / 420.0
    ));
    Ok(())
}

#[test]
fn ids_that_could_make_derived_ids_collide_are_refused() {
    // Price "p>100000" at its base would have the id of price p above 100,000.
    let text =
        calibration("hosts", "10 20 30 40 50", "1 1 2 4 4") + &price("p>100000", 4.0, 20.0, &[]);
    assert!(
        problem(Shipped::new().load(&[("a.toml", &text)]))
            .contains("price p>100000 has an id with other characters")
    );
    let text = parameter_set("frontier@p", "1.1 1.1 1.2 1.3 1.5");
    assert!(
        problem(Shipped::new().load(&[("a.toml", &text)]))
            .contains("parameter set frontier@p has an id with other characters")
    );
}

#[test]
fn invalid_prices_and_calibrations_are_refused() {
    let calibrated = |more: String| calibration("hosts", "10 20 30 40 50", "1 1 2 4 4") + &more;
    let cases = [
        (
            priced_model("priced", "priced-model", "missing"),
            "model priced names price missing, which no data file defines",
        ),
        (
            price("p", 4.0, 20.0, &[]),
            "price p names price calibration hosts, which no data file defines",
        ),
        (
            calibrated(price("p", 0.0, 20.0, &[])),
            "price p, input_usd_per_mtok is 0",
        ),
        (
            calibrated(price("p", 4.0, 20.0, &[(200, 8.0, 30.0), (100, 8.0, 30.0)])),
            "price p, tier above 100: tiers need thresholds above 0, lowest first",
        ),
        (
            calibrated(price("p", 4.0, 20.0, &[(100, 8.0, -1.0)])),
            "price p, tier above 100, output_usd_per_mtok is -1",
        ),
        (
            calibration("hosts", "10 20 30 40 50", "0.5 1 2 4 4"),
            "price calibration hosts, markup has the value 0.5",
        ),
        (
            calibration("hosts", "10 20 30 40 50", "1 1 2 4 4").replacen(
                "anchor_out_j = 0.38",
                "anchor_out_j = 0",
                1,
            ),
            "price calibration hosts, anchor_out_j is 0",
        ),
    ];
    for (text, expected) in cases {
        let problem = problem(Shipped::new().load(&[("a.toml", &text)]));
        assert!(problem.contains(expected), "{problem}");
        assert!(problem.contains("a.toml"), "{problem}");
    }
}

#[test]
fn prices_are_stale_after_their_own_shorter_interval() -> Result<(), Error> {
    let text = priced().replace("reviewed = 2026-10-06", "reviewed = 2026-09-01");
    let data = Shipped::new().load(&[("a.toml", &text)])?;
    let on = |day: &str| -> Vec<(Kind, String)> {
        let today: Date = day.parse().unwrap_or_default();
        data.stale(today)
            .into_iter()
            .filter(|stale| stale.reviewed == Date::constant(2026, 9, 1))
            .map(|stale| (stale.kind, stale.id))
            .collect()
    };
    assert!(on("2026-10-01").is_empty(), "30 days after the review");
    assert_eq!(on("2026-10-02"), [(Kind::Price, "p".to_owned())]);
    assert_eq!(on("2026-11-30").len(), 1, "90 days after the review");
    assert_eq!(on("2026-12-01").len(), 3, "the calibration and the model");
    Ok(())
}

#[test]
fn every_ranked_closed_model_with_a_list_price_uses_its_own_price() -> Result<(), Error> {
    let data = load(None)?;
    let priced = [
        ("claude-fable-5-1", "claude-fable-5-1"),
        ("claude-opus-5-5", "claude-opus-5-5"),
        ("claude-opus-5", "claude-opus-5"),
        ("claude-sonnet-5-5", "claude-sonnet-5-5"),
        ("claude-sonnet-5", "claude-sonnet-5"),
        ("claude-haiku-5-5", "claude-haiku-5-5"),
        ("claude-haiku-4-5-20251001", "claude-haiku-4-5-20251001"),
        ("gpt-6-astra", "gpt-6-astra"),
        ("gpt-6.1-sol", "gpt-6.1-sol"),
        ("gpt-6-sol", "gpt-6-sol"),
        ("gpt-6-luna", "gpt-6-luna"),
        ("gpt-5.6-sol", "gpt-5.6-sol"),
        ("gpt-5.6-terra", "gpt-5.6-terra"),
        ("gpt-5.6-luna", "gpt-5.6-luna"),
        ("gpt-5.5", "gpt-5.5"),
        ("gpt-5.3-codex", "gpt-5.3-codex"),
        ("gemini-3.8-flash", "gemini-3.8-flash"),
        ("gemini-3.1-pro-preview", "gemini-3.1-pro-preview"),
        ("gemini-2.5-pro", "gemini-2.5-pro"),
    ];
    for (name, price) in priced {
        let found = data.parameter_set(name, 0);
        assert!(found.matched, "{name}");
        assert_eq!(found.record.id, format!("frontier@{price}"), "{name}");
    }
    for name in [
        "gpt-5.3-codex-spark",
        "codex-auto-review",
        "claude-mythos-5-1",
    ] {
        assert_eq!(data.parameter_set(name, 0).record.id, "frontier", "{name}");
    }
    Ok(())
}

#[test]
fn the_long_context_finding_loads_and_an_override_replaces_it() -> Result<(), Error> {
    let shipped = Shipped::new();
    assert_eq!(shipped.load(&[])?.long_context_tokens(), 200_000);
    let replaced = FINDINGS_TOML.replace("200000", "150000");
    let data = shipped.load(&[("a.toml", &replaced)])?;
    assert_eq!(data.long_context_tokens(), 150_000);
    assert!(data.is_overridden(Kind::Finding, "long-context"));
    Ok(())
}

#[test]
fn a_finding_without_its_parameter_or_unknown_or_missing_is_refused() {
    let shipped = Shipped::new();
    let zero = FINDINGS_TOML.replace("200000", "0");
    assert!(
        problem(shipped.load(&[("a.toml", &zero)]))
            .contains("finding long-context needs cached_tokens above 0")
    );
    let without = FINDINGS_TOML.replace("cached_tokens = 200000\n", "");
    assert!(
        problem(shipped.load(&[("a.toml", &without)]))
            .contains("finding long-context needs cached_tokens above 0")
    );
    let unknown = FINDINGS_TOML.replace("long-context", "short-context");
    assert!(
        problem(shipped.load(&[("a.toml", &unknown)]))
            .contains("finding short-context is not a finding tokendraw knows")
    );
    let missing = Shipped {
        findings: "",
        ..Shipped::new()
    };
    assert!(problem(missing.load(&[])).contains("finding long-context is missing"));
}
