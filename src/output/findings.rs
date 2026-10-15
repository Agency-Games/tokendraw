use std::io::{self, Write};

use super::count;
use super::report::{INDENT, header, percent, wrapped, write_wrapped};
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
    let facts = match finding.facts {
        Facts::LongContext {
            calls: 0,
            threshold_cached_tokens,
            ..
        } => format!(
            "No calls carried more than {} cached tokens.",
            count(threshold_cached_tokens)
        ),
        Facts::LongContext {
            share_of_calls,
            share_of_base_case,
            threshold_cached_tokens,
            ..
        } => format!(
            "{} of calls carried more than {} cached tokens and used {} of the energy. Every new and output token re-reads that context.",
            percent(share_of_calls),
            count(threshold_cached_tokens),
            percent(share_of_base_case)
        ),
    };
    let mut lines = wrapped(&facts, INDENT);
    // Without calls there is nothing to change; the JSON still names the
    // steps, so a later period can be compared.
    if let Some(measure) = &finding.measure {
        lines.push(format!("{INDENT}What you can do:"));
        let nested = format!("{INDENT}{INDENT}");
        for (client, steps) in &finding.actions {
            let steps: Vec<&str> = steps
                .iter()
                .map(|step| step.trim_end_matches('.'))
                .collect();
            lines.extend(wrapped(&format!("{client}: {}", steps.join(", ")), &nested));
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
