use super::load::{Merged, Origin, Records, SHIPPED_RECOVERY, invalid};
use super::records::{ClientAction, Five, Label, Source};
use super::{
    Error, FALLBACK_PARAMETER_SET, FALLBACK_PROVIDER, FINDINGS, Kind, LONG_CONTEXT, MODEL_CHOICE,
};

pub(super) fn check(merged: &Merged, origins: &[Origin]) -> Result<(), Error> {
    let sources = &merged.sources;
    for (id, (source, origin)) in sources {
        if source.title.is_empty() || source.publisher.is_empty() {
            return Err(invalid(
                &origins[*origin],
                format!("source {id} needs a title and a publisher"),
            ));
        }
    }
    for (id, (set, origin)) in &merged.parameter_sets {
        let origin = &origins[*origin];
        check_id(origin, Kind::ParameterSet, id)?;
        for (name, factor) in set.factors() {
            let what = format!("parameter set {id}, {name}");
            check_sources(sources, origin, &what, &factor.sources)?;
            check_label(origin, &what, factor.label)?;
            let minimum = if name == "idle" { 1.0 } else { 0.0 };
            check_five(origin, &what, &factor.five(), minimum)?;
        }
    }
    check_prices(merged, origins)?;
    check_models(merged, origins)?;
    for (id, (provider, origin)) in &merged.providers {
        let origin = &origins[*origin];
        let what = format!("provider {id}");
        check_sources(sources, origin, &what, &provider.sources)?;
        check_label(origin, &what, provider.label)?;
        let what = format!("{what}, pue");
        check_sources(sources, origin, &what, &provider.pue.sources)?;
        check_label(origin, &what, provider.pue.label)?;
        check_five(origin, &what, &provider.pue.five(), 1.0)?;
    }
    for (id, (geography, origin)) in &merged.geographies {
        let origin = &origins[*origin];
        let what = format!("geography {id}");
        check_sources(sources, origin, &what, &geography.sources)?;
        check_label(origin, &what, geography.label)?;
        let value = geography.g_co2e_per_kwh.0;
        if !value.is_finite() || value < 0.0 {
            return Err(invalid(
                origin,
                format!("{what} has the intensity {value}; it must be a finite number, at least 0"),
            ));
        }
    }
    for (id, (location, origin)) in &merged.locations {
        let origin = &origins[*origin];
        let what = format!("location {id}");
        check_sources(sources, origin, &what, &location.sources)?;
        check_label(origin, &what, location.label)?;
        if let Some(path) = location
            .paths
            .iter()
            .find(|path| path.os.is_empty() || !crate::adapters::coverage::is_template(&path.path))
        {
            return Err(invalid(
                origin,
                format!(
                    "{what} has the path {:?}; each path needs at least one os and must start with ~/, $NAME/ or %NAME%/",
                    path.path
                ),
            ));
        }
    }
    check_findings(merged, origins)?;
    let missing = |file: &str, problem: String| Error::Invalid {
        file: file.to_owned(),
        problem,
        recovery: SHIPPED_RECOVERY,
    };
    if !merged.parameter_sets.contains_key(FALLBACK_PARAMETER_SET) {
        return Err(missing(
            "data/energy/parameter-sets.toml",
            format!("parameter set {FALLBACK_PARAMETER_SET} is missing"),
        ));
    }
    if !merged.providers.contains_key(FALLBACK_PROVIDER) {
        return Err(missing(
            "data/providers.toml",
            format!("provider {FALLBACK_PROVIDER} is missing"),
        ));
    }
    Ok(())
}

/// Each finding needs its own parameters.
fn check_findings(merged: &Merged, origins: &[Origin]) -> Result<(), Error> {
    for (id, (finding, origin)) in &merged.findings {
        let origin = &origins[*origin];
        let what = format!("finding {id}");
        if !FINDINGS.contains(&id.as_str()) {
            return Err(invalid(
                origin,
                format!(
                    "{what} is not a finding tokendraw knows; use one of {}",
                    FINDINGS.join(", ")
                ),
            ));
        }
        check_sources(&merged.sources, origin, &what, &finding.sources)?;
        check_label(origin, &what, finding.label)?;
        if id == LONG_CONTEXT && finding.cached_tokens.is_none_or(|tokens| tokens == 0) {
            return Err(invalid(
                origin,
                format!("{what} needs cached_tokens above 0"),
            ));
        }
        match (id == MODEL_CHOICE, &finding.roles) {
            (true, roles) if roles.as_ref().is_none_or(Vec::is_empty) => {
                return Err(invalid(origin, format!("{what} needs at least one role")));
            }
            (false, Some(_)) => {
                return Err(invalid(
                    origin,
                    format!("{what} has roles; only {MODEL_CHOICE} takes them"),
                ));
            }
            _ => {}
        }
    }
    for (id, (action, origin)) in &merged.actions {
        let origin = &origins[*origin];
        let what = format!("action {id}");
        if !FINDINGS.contains(&action.finding.as_str()) {
            return Err(invalid(
                origin,
                format!(
                    "{what} names the finding {}, which tokendraw does not know; use one of {}",
                    action.finding,
                    FINDINGS.join(", ")
                ),
            ));
        }
        if action.client.is_empty()
            || action.steps.is_empty()
            || action.steps.iter().any(|step| step.trim().is_empty())
        {
            return Err(invalid(
                origin,
                format!("{what} needs a client and at least one step, none of them empty"),
            ));
        }
        check_sources(&merged.sources, origin, &what, &action.sources)?;
        check_label(origin, &what, action.label)?;
        check_action_model(merged, origin, &what, action)?;
    }
    if !merged.findings.contains_key(LONG_CONTEXT) {
        return Err(Error::Invalid {
            file: "data/findings.toml".to_owned(),
            problem: format!("finding {LONG_CONTEXT} is missing"),
            recovery: SHIPPED_RECOVERY,
        });
    }
    Ok(())
}

/// A `model-choice` action compares the calls with a priced model, whose
/// weight terms its price sets; no other action names a model.
fn check_action_model(
    merged: &Merged,
    origin: &Origin,
    what: &str,
    action: &ClientAction,
) -> Result<(), Error> {
    match (action.finding == MODEL_CHOICE, &action.model) {
        (true, None) => Err(invalid(origin, format!("{what} needs a model"))),
        (false, Some(_)) => Err(invalid(
            origin,
            format!("{what} has a model; only {MODEL_CHOICE} actions take one"),
        )),
        (true, Some(model)) => match merged.models.get(model) {
            None => Err(invalid(
                origin,
                format!("{what} names the model {model}, which no data file defines"),
            )),
            Some((record, _)) if record.price.is_none() => Err(invalid(
                origin,
                format!("{what} names the model {model}, which has no price; name a priced model"),
            )),
            Some(_) => Ok(()),
        },
        (false, None) => Ok(()),
    }
}

fn check_models(merged: &Merged, origins: &[Origin]) -> Result<(), Error> {
    let sources = &merged.sources;
    for (id, (model, origin)) in &merged.models {
        let origin = &origins[*origin];
        let what = format!("model {id}");
        if let Some(price) = &model.price
            && !merged.prices.contains_key(price)
        {
            return Err(invalid(
                origin,
                format!("{what} names price {price}, which no data file defines"),
            ));
        }
        check_sources(sources, origin, &what, &model.sources)?;
        check_label(origin, &what, model.label)?;
        let prefix = model.pattern.strip_suffix('*').unwrap_or(&model.pattern);
        if prefix.is_empty() || prefix.contains('*') {
            return Err(invalid(
                origin,
                format!(
                    "{what} has the pattern {:?}; use an exact name or a prefix followed by one *",
                    model.pattern
                ),
            ));
        }
        if !merged.parameter_sets.contains_key(&model.parameter_set) {
            return Err(invalid(
                origin,
                format!(
                    "{what} names parameter set {}, which no data file defines",
                    model.parameter_set
                ),
            ));
        }
        if let Some((other, _)) = merged
            .models
            .iter()
            .find(|(other, (record, _))| *other < id && record.pattern == model.pattern)
        {
            return Err(invalid(
                origin,
                format!("{what} has the same pattern as model {other}"),
            ));
        }
    }
    Ok(())
}

fn check_prices(merged: &Merged, origins: &[Origin]) -> Result<(), Error> {
    let sources = &merged.sources;
    for (id, (calibration, origin)) in &merged.price_calibrations {
        let origin = &origins[*origin];
        let what = format!("price calibration {id}");
        if calibration.chat_input_tokens == 0 && calibration.chat_output_tokens == 0 {
            return Err(invalid(
                origin,
                format!("{what} has a reference call without tokens"),
            ));
        }
        for (name, value) in [
            ("anchor_in_j", calibration.anchor_in_j),
            ("anchor_out_j", calibration.anchor_out_j),
        ] {
            check_positive(origin, &format!("{what}, {name}"), value)?;
        }
        for (name, factor, minimum) in [
            ("wh_per_usd", &calibration.wh_per_usd, 0.0),
            ("markup", &calibration.markup, 1.0),
        ] {
            let what = format!("{what}, {name}");
            check_sources(sources, origin, &what, &factor.sources)?;
            check_label(origin, &what, factor.label)?;
            check_five(origin, &what, &factor.five(), minimum)?;
        }
    }
    for (id, (price, origin)) in &merged.prices {
        let origin = &origins[*origin];
        check_id(origin, Kind::Price, id)?;
        let what = format!("price {id}");
        check_sources(sources, origin, &what, &price.sources)?;
        check_label(origin, &what, price.label)?;
        if !merged.price_calibrations.contains_key(&price.calibration) {
            return Err(invalid(
                origin,
                format!(
                    "{what} names price calibration {}, which no data file defines",
                    price.calibration
                ),
            ));
        }
        check_positive(
            origin,
            &format!("{what}, input_usd_per_mtok"),
            price.input_usd_per_mtok,
        )?;
        check_positive(
            origin,
            &format!("{what}, output_usd_per_mtok"),
            price.output_usd_per_mtok,
        )?;
        let mut below = 0;
        for tier in &price.tier {
            let what = format!("{what}, tier above {}", tier.above_prompt_tokens);
            if tier.above_prompt_tokens <= below {
                return Err(invalid(
                    origin,
                    format!("{what}: tiers need thresholds above 0, lowest first, each once"),
                ));
            }
            below = tier.above_prompt_tokens;
            check_positive(
                origin,
                &format!("{what}, input_usd_per_mtok"),
                tier.input_usd_per_mtok,
            )?;
            check_positive(
                origin,
                &format!("{what}, output_usd_per_mtok"),
                tier.output_usd_per_mtok,
            )?;
        }
    }
    Ok(())
}

/// [`priced_set_id`] relies on written ids holding no `@` or `>`.
fn check_id(origin: &Origin, kind: Kind, id: &str) -> Result<(), Error> {
    if id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
    {
        return Ok(());
    }
    Err(invalid(
        origin,
        format!(
            "{kind} {id} has an id with other characters than ASCII letters, digits, '.', '-' and '_'"
        ),
    ))
}

fn check_positive(origin: &Origin, what: &str, value: f64) -> Result<(), Error> {
    if value.is_finite() && value > 0.0 {
        return Ok(());
    }
    Err(invalid(
        origin,
        format!("{what} is {value}; it must be a finite number above 0"),
    ))
}

fn check_sources(
    sources: &Records<Source>,
    origin: &Origin,
    what: &str,
    cited: &[String],
) -> Result<(), Error> {
    if cited.is_empty() {
        return Err(invalid(origin, format!("{what} names no source")));
    }
    match cited.iter().find(|id| !sources.contains_key(*id)) {
        Some(missing) => Err(invalid(
            origin,
            format!("{what} names source {missing}, which no data file defines"),
        )),
        None => Ok(()),
    }
}

/// Spend-based values are a cross-check only, and every record checked
/// here can enter an estimate.
fn check_label(origin: &Origin, what: &str, label: Label) -> Result<(), Error> {
    if label == Label::S {
        return Err(invalid(
            origin,
            format!(
                "{what} has the label S; spend-based values never enter an estimate, so use M, C, I or D"
            ),
        ));
    }
    Ok(())
}

fn check_five(origin: &Origin, what: &str, five: &Five<f64>, minimum: f64) -> Result<(), Error> {
    let values = five.values();
    if let Some(value) = values
        .iter()
        .find(|value| !value.is_finite() || **value < minimum)
    {
        return Err(invalid(
            origin,
            format!(
                "{what} has the value {value}; every value must be a finite number, at least {minimum}"
            ),
        ));
    }
    if values.windows(2).any(|pair| pair[0] > pair[1]) {
        return Err(invalid(
            origin,
            format!(
                "{what} has its values out of order; lower_bound <= low_case <= base_case <= high_case <= upper_bound"
            ),
        ));
    }
    Ok(())
}
