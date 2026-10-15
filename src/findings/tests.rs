use jiff::civil::date;

use super::*;
use crate::data::{self, ClientAction, Error, Five, Label};
use crate::estimate::{Breakdown, Calls, Fixed, LargerModel};
use crate::quantity::{GramsCo2e, Tokens, WattHours};

/// The shipped data with steps for exactly `clients`, for `long-context`.
fn data_with_steps(clients: &[&str]) -> Result<Data, Error> {
    data_with_steps_for(LONG_CONTEXT, clients)
}

fn data_with_steps_for(finding: &str, clients: &[&str]) -> Result<Data, Error> {
    let mut data = data::load(None)?;
    data.actions.clear();
    add_steps(&mut data, finding, clients);
    Ok(data)
}

fn add_steps(data: &mut Data, finding: &str, clients: &[&str]) {
    for client in clients {
        let action = ClientAction {
            finding: finding.to_owned(),
            client: (*client).to_owned(),
            steps: vec![format!("/compact in {client}")],
            model: None,
            label: Label::C,
            sources: Vec::new(),
            basis: String::new(),
            reviewed: date(2026, 10, 15),
        };
        data.actions.insert(action.id(), action);
    }
}

/// `clients` holds each client's base case energy in Wh, and its calls and
/// energy above the long-context threshold.
fn l1(calls: u64, clients: &[(&str, f64, Calls)]) -> L1 {
    let total: f64 = clients.iter().map(|(_, energy, _)| energy).sum();
    L1 {
        groups: Vec::new(),
        energy: Five::same(WattHours(total)),
        facility: Five::same(WattHours(0.0)),
        emissions: Five::same(GramsCo2e(0.0)),
        energy_quality: None,
        emissions_quality: None,
        parameter_sets: Vec::new(),
        unmatched_models: Vec::new(),
        unmatched_providers: Vec::new(),
        breakdown: Breakdown {
            by_client: clients
                .iter()
                .map(|(client, energy, _)| ((*client).to_owned(), WattHours(*energy)))
                .collect(),
            cached_above_by_client: clients
                .iter()
                .map(|(client, _, long)| ((*client).to_owned(), *long))
                .collect(),
            ..Breakdown::default()
        },
        calls,
    }
}

fn long(calls: u64, energy: f64) -> Calls {
    Calls {
        calls,
        energy: WattHours(energy),
    }
}

#[test]
fn long_context_counts_only_the_calls_of_clients_with_an_action() -> Result<(), Error> {
    let data = data_with_steps(&["claude-code", "codex"])?;
    let calls = l1(
        100,
        &[
            ("claude-code", 60.0, long(10, 30.0)),
            ("codex", 30.0, long(0, 0.0)),
            ("example-agent", 10.0, long(5, 10.0)),
        ],
    );
    let findings = every(&data, &calls).unwrap_or_default();
    assert_eq!(findings.len(), 1);
    let finding = &findings[0];
    assert_eq!(finding.code, "long-context");
    assert_eq!(finding.facts.calls(), 10);
    assert!((finding.facts.share_of_base_case() - 0.3).abs() < 1e-12);
    let Facts::LongContext { share_of_calls, .. } = finding.facts else {
        panic!("expected long-context");
    };
    assert!((share_of_calls - 0.1).abs() < 1e-12);
    assert_eq!(
        finding.actions.keys().collect::<Vec<_>>(),
        ["claude-code"],
        "only the clients whose calls it counts"
    );
    Ok(())
}

#[test]
fn a_finding_without_calls_lists_every_client_it_would_count_and_is_not_shown() -> Result<(), Error>
{
    let data = data_with_steps(&["claude-code", "codex"])?;
    let calls = l1(
        20,
        &[
            ("claude-code", 60.0, long(0, 0.0)),
            ("codex", 30.0, long(0, 0.0)),
        ],
    );
    let findings = every(&data, &calls).unwrap_or_default();
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].facts.calls(), 0);
    assert!(findings[0].facts.share_of_base_case().abs() < f64::EPSILON);
    assert_eq!(findings[0].actions.len(), 2);
    let (shown, hidden) = shown(&findings, 0.0);
    assert!(shown.is_empty());
    assert_eq!(hidden, 0);
    Ok(())
}

#[test]
fn no_finding_applies_without_a_client_that_has_an_action() -> Result<(), Error> {
    let data = data_with_steps(&[])?;
    let calls = l1(5, &[("claude-code", 60.0, long(5, 60.0))]);
    assert_eq!(every(&data, &calls).map(|findings| findings.len()), Some(0));
    Ok(())
}

#[test]
fn without_base_case_energy_there_is_no_share_to_compute() -> Result<(), Error> {
    let data = data_with_steps(&["claude-code"])?;
    assert!(every(&data, &l1(0, &[])).is_none());
    Ok(())
}

#[test]
fn a_finding_at_the_threshold_is_shown_and_one_below_is_counted() -> Result<(), Error> {
    let data = data_with_steps(&["claude-code"])?;
    let calls = l1(10, &[("claude-code", 100.0, long(2, 25.0))]);
    let findings = every(&data, &calls).unwrap_or_default();
    let (at, hidden) = shown(&findings, 0.25);
    assert_eq!((at.len(), hidden), (1, 0));
    let (above, hidden) = shown(&findings, 0.26);
    assert_eq!((above.len(), hidden), (0, 1));
    Ok(())
}

#[test]
fn fixed_context_counts_only_clients_with_steps_and_sorts_with_long_context() -> Result<(), Error> {
    let mut data = data_with_steps_for(FIXED_CONTEXT, &["claude-code", "codex"])?;
    add_steps(&mut data, LONG_CONTEXT, &["claude-code"]);
    let mut calls = l1(
        100,
        &[
            ("claude-code", 60.0, long(10, 10.0)),
            ("codex", 30.0, long(0, 0.0)),
            ("example-agent", 10.0, long(0, 0.0)),
        ],
    );
    let fixed = |calls, tokens, energy| Fixed {
        calls,
        tokens: Tokens(tokens),
        energy: WattHours(energy),
    };
    calls.breakdown.fixed_by_client = [
        ("claude-code".to_owned(), fixed(60, 2_400_000, 12.0)),
        ("codex".to_owned(), fixed(30, 600_001, 3.0)),
        ("example-agent".to_owned(), fixed(10, 100_000, 5.0)),
    ]
    .into();
    let findings = every(&data, &calls).unwrap_or_default();
    let codes: Vec<&str> = findings.iter().map(|finding| finding.code).collect();
    assert_eq!(codes, [FIXED_CONTEXT, LONG_CONTEXT]);
    let Facts::FixedContext {
        calls,
        mean_fixed_tokens,
        share_of_base_case,
    } = findings[0].facts
    else {
        panic!("expected fixed-context");
    };
    assert_eq!((calls, mean_fixed_tokens), (90, 33_333));
    assert!((share_of_base_case - 0.15).abs() < 1e-12);
    assert_eq!(
        findings[0].actions.keys().collect::<Vec<_>>(),
        ["claude-code", "codex"]
    );
    Ok(())
}

/// The shipped `model-choice` steps only, with pi compared with
/// `claude-haiku-5-5` as an override would add it.
fn model_choice_data() -> Result<Data, Error> {
    let mut data = data::load(None)?;
    data.actions
        .retain(|_, action| action.finding == MODEL_CHOICE);
    let mut pi = data.actions[&format!("{MODEL_CHOICE}/claude-code")].clone();
    "pi".clone_into(&mut pi.client);
    data.actions.insert(pi.id(), pi);
    Ok(data)
}

fn larger_calls(calls: u64, energy: f64, difference: f64) -> LargerModel {
    LargerModel {
        calls,
        energy: WattHours(energy),
        difference: WattHours(difference),
    }
}

#[test]
fn model_choice_sizes_the_difference_and_names_the_clients_it_counts() -> Result<(), Error> {
    let data = model_choice_data()?;
    let mut calls = l1(
        100,
        &[
            ("claude-code", 50.0, long(0, 0.0)),
            ("codex", 30.0, long(0, 0.0)),
            ("pi", 10.0, long(0, 0.0)),
            ("example-agent", 10.0, long(0, 0.0)),
        ],
    );
    calls.breakdown.larger_model_by_client = [
        ("claude-code".to_owned(), larger_calls(20, 40.0, 10.0)),
        ("pi".to_owned(), larger_calls(5, 10.0, 2.0)),
        ("example-agent".to_owned(), larger_calls(5, 10.0, 8.0)),
    ]
    .into();
    let findings = every(&data, &calls).unwrap_or_default();
    assert_eq!(findings.len(), 1);
    let Facts::ModelChoice {
        calls,
        calls_share_of_base_case,
        share_of_base_case,
        roles,
        models,
    } = &findings[0].facts
    else {
        panic!("expected model-choice");
    };
    assert_eq!(*calls, 25);
    assert!((calls_share_of_base_case - 0.5).abs() < 1e-12);
    assert!((share_of_base_case - 0.12).abs() < 1e-12);
    assert_eq!(roles, &["subagent", "headless"]);
    let expected: BTreeMap<String, String> = [
        ("claude-code".to_owned(), "claude-haiku-5-5".to_owned()),
        ("pi".to_owned(), "claude-haiku-5-5".to_owned()),
    ]
    .into();
    assert_eq!(models, &expected);
    assert_eq!(
        findings[0].actions.keys().collect::<Vec<_>>(),
        ["claude-code", "pi"]
    );
    Ok(())
}

#[test]
fn model_choice_without_calls_names_every_client_it_would_compare() -> Result<(), Error> {
    let data = model_choice_data()?;
    let calls = l1(
        10,
        &[
            ("claude-code", 50.0, long(0, 0.0)),
            ("codex", 30.0, long(0, 0.0)),
        ],
    );
    let findings = every(&data, &calls).unwrap_or_default();
    let Facts::ModelChoice { calls, models, .. } = &findings[0].facts else {
        panic!("expected model-choice");
    };
    assert_eq!(*calls, 0);
    assert_eq!(
        models.iter().collect::<Vec<_>>(),
        [
            (&"claude-code".to_owned(), &"claude-haiku-5-5".to_owned()),
            (&"codex".to_owned(), &"gpt-6-luna".to_owned())
        ]
    );
    Ok(())
}
