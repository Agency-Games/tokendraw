//! The findings: behaviours in the user's calls that the user can change,
//! each with its share of the base case L1 energy and the steps a user of
//! each client can take. A client the data names no steps for has its calls
//! left out, since a finding without an action is not shown.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::data::{Data, FIXED_CONTEXT, LONG_CONTEXT};
use crate::estimate::L1;

#[derive(Clone, Debug, Serialize)]
pub struct Finding {
    pub code: &'static str,
    pub facts: Facts,
    /// By client: the clients whose calls the finding counts, or, without
    /// calls, those it would count.
    pub actions: BTreeMap<String, Vec<String>>,
    /// How to see the effect of a change; left out without calls.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub measure: Option<Measure>,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(untagged)]
pub enum Facts {
    LongContext {
        calls: u64,
        /// Of every call in the period.
        share_of_calls: f64,
        share_of_base_case: f64,
        threshold_cached_tokens: u64,
    },
    FixedContext {
        calls: u64,
        /// The mean over calls of the context their thread started with,
        /// up to their own.
        mean_fixed_tokens: u64,
        share_of_base_case: f64,
    },
}

impl Facts {
    #[must_use]
    pub fn share_of_base_case(&self) -> f64 {
        match self {
            Facts::LongContext {
                share_of_base_case, ..
            }
            | Facts::FixedContext {
                share_of_base_case, ..
            } => *share_of_base_case,
        }
    }

    #[must_use]
    pub fn calls(&self) -> u64 {
        match self {
            Facts::LongContext { calls, .. } | Facts::FixedContext { calls, .. } => *calls,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Measure {
    /// Without backticks.
    pub command: String,
    /// The fact to compare.
    pub compare: &'static str,
}

/// Every finding that applies to the calls, largest first; `None` without
/// base case energy, which no share can divide. A finding applies when a
/// client with calls has an action for it, also when none of its calls
/// shows the behaviour, so a later period can be measured against it.
#[must_use]
pub fn every(data: &Data, l1: &L1) -> Option<Vec<Finding>> {
    let total = l1.energy.base_case.0;
    if total <= 0.0 {
        return None;
    }
    let mut findings: Vec<Finding> = [
        long_context(data, l1, total),
        fixed_context(data, l1, total),
    ]
    .into_iter()
    .flatten()
    .collect();
    findings.sort_by(|a, b| {
        b.facts
            .share_of_base_case()
            .total_cmp(&a.facts.share_of_base_case())
            .then_with(|| a.code.cmp(b.code))
    });
    Some(findings)
}

/// The findings with calls at or above `threshold`, a share of the base
/// case energy, and how many with calls are below it. A finding without
/// calls has no behaviour to change, so it is neither shown nor counted.
#[must_use]
pub fn shown(findings: &[Finding], threshold: f64) -> (Vec<Finding>, u64) {
    let mut shown = Vec::new();
    let mut hidden = 0;
    for finding in findings.iter().filter(|finding| finding.facts.calls() > 0) {
        if finding.facts.share_of_base_case() >= threshold {
            shown.push(finding.clone());
        } else {
            hidden += 1;
        }
    }
    (shown, hidden)
}

/// The steps of `finding` by client, for the clients with calls; `None`
/// when no such client has any.
fn applicable(data: &Data, l1: &L1, finding: &str) -> Option<BTreeMap<String, Vec<String>>> {
    let applicable: BTreeMap<String, Vec<String>> = l1
        .breakdown
        .by_client
        .keys()
        .filter_map(|client| {
            data.steps(finding, client)
                .map(|steps| (client.clone(), steps.to_vec()))
        })
        .collect();
    (!applicable.is_empty()).then_some(applicable)
}

fn long_context(data: &Data, l1: &L1, total: f64) -> Option<Finding> {
    let parts = &l1.breakdown;
    let applicable = applicable(data, l1, LONG_CONTEXT)?;
    let long: Vec<_> = parts
        .cached_above_by_client
        .iter()
        .filter(|(client, part)| part.calls > 0 && applicable.contains_key(*client))
        .collect();
    let calls: u64 = long.iter().map(|(_, part)| part.calls).sum();
    let energy: f64 = long.iter().map(|(_, part)| part.energy.0).sum();
    let actions = if calls == 0 {
        applicable
    } else {
        applicable
            .into_iter()
            .filter(|(client, _)| long.iter().any(|(id, _)| *id == client))
            .collect()
    };
    #[expect(
        clippy::cast_precision_loss,
        reason = "call counts stay far below 2^53"
    )]
    let share_of_calls = calls as f64 / l1.calls as f64;
    Some(Finding {
        code: LONG_CONTEXT,
        facts: Facts::LongContext {
            calls,
            share_of_calls,
            share_of_base_case: energy / total,
            threshold_cached_tokens: data.long_context_tokens(),
        },
        actions,
        measure: None,
    })
}

/// Every call carries the context its thread started with, so the
/// finding has the calls of every client it has steps for.
fn fixed_context(data: &Data, l1: &L1, total: f64) -> Option<Finding> {
    let actions = applicable(data, l1, FIXED_CONTEXT)?;
    let fixed: Vec<_> = l1
        .breakdown
        .fixed_by_client
        .iter()
        .filter(|(client, _)| actions.contains_key(*client))
        .map(|(_, fixed)| fixed)
        .collect();
    let calls: u64 = fixed.iter().map(|fixed| fixed.calls).sum();
    let tokens: u64 = fixed.iter().map(|fixed| fixed.tokens.0).sum();
    let energy: f64 = fixed.iter().map(|fixed| fixed.energy.0).sum();
    Some(Finding {
        code: FIXED_CONTEXT,
        facts: Facts::FixedContext {
            calls,
            mean_fixed_tokens: (tokens + calls / 2).checked_div(calls).unwrap_or(0),
            share_of_base_case: energy / total,
        },
        actions,
        measure: None,
    })
}

#[cfg(test)]
mod tests;
