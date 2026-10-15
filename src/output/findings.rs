use std::io::{self, Write};

use super::count;
use super::report::{INDENT, header, percent, significant, wrapped, write_wrapped};
use crate::commands::findings::FindingsReport;
use crate::commands::report::Report;
use crate::findings::{Facts, Finding};

pub fn write(out: &mut impl Write, findings: &FindingsReport) -> io::Result<()> {
    writeln!(
        out,
        "{}",
        header(
            "tokendraw findings",
            findings.project.as_deref(),
            findings.directory.as_deref(),
            &findings.period
        )
    )?;
    match &findings.findings {
        None => {
            writeln!(out)?;
            write_wrapped(
                out,
                "No findings: no energy was estimated for these calls, so no share can be computed.",
            )
        }
        Some(list) if list.is_empty() => {
            writeln!(out)?;
            write_wrapped(out, "No finding applies to these calls.")
        }
        Some(list) => list
            .iter()
            .try_for_each(|finding| write_finding(out, finding)),
    }
}

/// The findings at or above the threshold, then one line for the others.
pub(super) fn write_report_findings(out: &mut impl Write, report: &Report) -> io::Result<()> {
    let shown = report.findings.as_deref().unwrap_or_default();
    for finding in shown {
        write_finding(out, finding)?;
    }
    let Some(hidden) = report
        .hidden_findings
        .as_ref()
        .filter(|hidden| hidden.count > 0)
    else {
        return Ok(());
    };
    let noun = if hidden.count == 1 {
        "finding"
    } else {
        "findings"
    };
    let more = if shown.is_empty() { "" } else { " more" };
    // Six decimal places hide the binary rounding of a share times 100.
    let threshold = (hidden.threshold_share_of_base_case * 1e8).round() / 1e6;
    writeln!(out)?;
    write_wrapped(
        out,
        &format!(
            "{}{more} {noun} below {threshold}% of the energy: `{}`",
            count(hidden.count),
            hidden.command
        ),
    )
}

fn write_finding(out: &mut impl Write, finding: &Finding) -> io::Result<()> {
    writeln!(out)?;
    writeln!(out, "{}", finding.code)?;
    let facts = match &finding.facts {
        Facts::LongContext {
            calls: 0,
            threshold_cached_tokens,
            ..
        } => format!(
            "No calls carried more than {} cached tokens.",
            count(*threshold_cached_tokens)
        ),
        Facts::LongContext {
            share_of_calls,
            share_of_base_case,
            threshold_cached_tokens,
            ..
        } => format!(
            "{} of calls carried more than {} cached tokens and used {} of the energy. Every new and output token re-reads that context.",
            percent(*share_of_calls),
            count(*threshold_cached_tokens),
            percent(*share_of_base_case)
        ),
        Facts::FixedContext { calls: 0, .. } => {
            "No call carried context from the start of its session.".to_owned()
        }
        Facts::FixedContext {
            mean_fixed_tokens,
            share_of_base_case,
            ..
        } => format!(
            "Each call re-read on average {} tokens its session started with: the system prompt, tool definitions, instruction files and first prompt. That used {} of the energy.",
            fixed_tokens(*mean_fixed_tokens),
            percent(*share_of_base_case)
        ),
        Facts::ModelChoice {
            calls: 0,
            roles,
            models,
            ..
        } => format!(
            "No {} used a model pricier than {}.",
            listed(&role_nouns(roles), "or"),
            compared(models)
        ),
        Facts::ModelChoice {
            calls_share_of_base_case,
            share_of_base_case,
            roles,
            models,
            ..
        } => format!(
            "{} on models pricier than {} used {} of the energy. On those models, the same tokens would have made the total {} lower.",
            capitalized(&listed(&role_nouns(roles), "and")),
            compared(models),
            percent(*calls_share_of_base_case),
            percent(*share_of_base_case)
        ),
    };
    let mut lines = wrapped(&facts, INDENT);
    // Without calls there is nothing to change; the JSON still names the
    // steps, so a later period can be compared.
    if let Some(measure) = &finding.measure {
        lines.push(format!("{INDENT}What you can do:"));
        let nested = format!("{INDENT}{INDENT}");
        let step_indent = format!("{nested}{INDENT}");
        for (client, steps) in &finding.actions {
            lines.push(format!("{nested}{client}:"));
            for step in steps {
                lines.extend(wrapped(step.trim_end_matches('.'), &step_indent));
            }
        }
        lines.push(format!(
            "{INDENT}To see the effect, run this after the change:"
        ));
        lines.push(format!("{INDENT}`{}`", measure.command));
    }
    for line in lines {
        writeln!(out, "{line}")?;
    }
    Ok(())
}

/// `a, b and c`, with `and` or `or` before the last.
fn listed(items: &[String], last: &str) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., final_item] => format!("{} {last} {final_item}", rest.join(", ")),
    }
}

/// `subagents`, `headless runs`, `interactive sessions`.
fn role_nouns(roles: &[String]) -> Vec<String> {
    roles
        .iter()
        .map(|role| match role.as_str() {
            "subagent" => "subagents".to_owned(),
            "headless" => "headless runs".to_owned(),
            other => format!("{other} sessions"),
        })
        .collect()
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// Each compared model with its client: `claude-haiku-5-5 (claude-code)`.
fn compared(models: &std::collections::BTreeMap<String, String>) -> String {
    let models: Vec<String> = models
        .iter()
        .map(|(client, model)| format!("{model} ({client})"))
        .collect();
    listed(&models, "or")
}

/// A mean of estimated sizes, to three significant figures.
fn fixed_tokens(tokens: u64) -> String {
    #[expect(
        clippy::cast_precision_loss,
        reason = "three significant figures are shown"
    )]
    let tokens = tokens as f64;
    significant(tokens, 3)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn model_choice(calls: u64, models: &[(&str, &str)]) -> Finding {
        Finding {
            code: "model-choice",
            facts: Facts::ModelChoice {
                calls,
                calls_share_of_base_case: 0.5,
                share_of_base_case: 0.2,
                roles: vec!["subagent".to_owned(), "headless".to_owned()],
                models: models
                    .iter()
                    .map(|(client, model)| ((*client).to_owned(), (*model).to_owned()))
                    .collect(),
            },
            actions: BTreeMap::new(),
            measure: None,
        }
    }

    fn written(finding: &Finding) -> io::Result<String> {
        let mut out = Vec::new();
        write_finding(&mut out, finding)?;
        Ok(String::from_utf8_lossy(&out).into_owned())
    }

    #[test]
    fn model_choice_names_the_roles_and_each_compared_model() -> io::Result<()> {
        let text = written(&model_choice(
            3,
            &[("a", "small-a"), ("b", "small-b"), ("c", "small-c")],
        ))?;
        assert!(
            text.starts_with(
                "\nmodel-choice\n  Subagents and headless runs on models pricier than small-a (a), small-b\n  (b) or small-c (c) used 50% of the energy. On those models, the same\n  tokens would have made the total 20% lower.\n"
            ),
            "{text}"
        );
        let none = written(&model_choice(0, &[("a", "small-a")]))?;
        assert_eq!(
            none,
            "\nmodel-choice\n  No subagents or headless runs used a model pricier than small-a (a).\n"
        );
        Ok(())
    }
}
