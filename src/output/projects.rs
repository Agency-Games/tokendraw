use std::io::{self, Write};

use super::report::{Align, INDENT, cells, period, wrapped, write_named_table, write_wrapped};
use super::{count, shown_path};
use crate::commands::projects::{Inventory, quoted};
use crate::projects::PROJECT;

pub fn write(out: &mut impl Write, inventory: &Inventory) -> io::Result<()> {
    writeln!(out, "tokendraw projects, {}", period(&inventory.period))?;
    let mut rows = vec![cells([
        "Projects", "calls", "label", "checkout", "recorded", "ancestor",
    ])];
    for project in &inventory.projects {
        let by = &project.resolved_by;
        rows.push(vec![
            format!("{INDENT}{}", shown_path(&project.project)),
            count(project.calls),
            count(by.label),
            count(by.checkout),
            count(by.recorded),
            count(by.ancestor),
        ]);
    }
    writeln!(out)?;
    let aligns = [
        Align::Left,
        Align::Right,
        Align::Right,
        Align::Right,
        Align::Right,
        Align::Right,
    ];
    write_named_table(out, &aligns, &rows)?;
    let unresolved = &inventory.unresolved;
    if unresolved.calls == 0 {
        return Ok(());
    }
    let mut rows = vec![cells([
        "Unresolved",
        "calls",
        "sessions",
        "directories",
        "clients",
    ])];
    for group in &unresolved.groups {
        rows.push(vec![
            format!("{INDENT}{}", shown_path(&group.directory)),
            count(group.calls),
            count(group.sessions),
            count(group.directories.len() as u64),
            group.clients.join(", "),
        ]);
        if let Some(suggestion) = &group.suggestion {
            let project = shown_path(&suggestion.project);
            if suggestion.command.is_some() {
                rows.push(vec![format!(
                    "{INDENT}{INDENT}May belong to {project}. To assign it, run:"
                )]);
                rows.push(vec![format!(
                    "{INDENT}{INDENT}`tokendraw label dir {} {PROJECT}={}`",
                    quoted(&shown_path(&group.directory)),
                    quoted(&project)
                )]);
            } else {
                let text = format!(
                    "May belong to {project}, but the pattern also matches other groups: label these directories one by one with `tokendraw label dir`."
                );
                rows.extend(
                    wrapped(&text, &format!("{INDENT}{INDENT}"))
                        .into_iter()
                        .map(|line| vec![line]),
                );
            }
        }
    }
    if let Some(without) = &unresolved.without_directory {
        rows.push(vec![
            format!("{INDENT}no working directory"),
            count(without.calls),
            count(without.sessions.len() as u64),
            String::new(),
            without.clients.join(", "),
        ]);
        rows.push(vec![format!("{INDENT}{INDENT}To assign a session, run:")]);
        rows.push(vec![format!(
            "{INDENT}{INDENT}`tokendraw label session <client> <session-id> {PROJECT}=<dir>`"
        )]);
    }
    rows.push(Vec::new());
    rows.push(vec![format!("{INDENT}total"), count(unresolved.calls)]);
    let aligns = [
        Align::Left,
        Align::Right,
        Align::Right,
        Align::Right,
        Align::Left,
    ];
    writeln!(out)?;
    write_named_table(out, &aligns, &rows)?;
    writeln!(out)?;
    write_wrapped(
        out,
        "The suggested labels are not applied. Run `tokendraw projects --json` to list every directory and session.",
    )
}
