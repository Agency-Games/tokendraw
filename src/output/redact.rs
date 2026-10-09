//! `report --redact`: the report without names from the user's machine, so
//! it can be shared. Ids of clients, models, providers and data records
//! stay, as do versions, counts and figures.

use crate::commands::report::Report;
use crate::data::{Data, Kind};

pub const REDACTED: &str = "redacted";

/// Shipped records are public, so only the free text of records an
/// override added or replaced is redacted.
#[must_use]
pub fn data(data: &Data) -> Data {
    let mut redacted = data.clone();
    for record in data.version.overrides.iter().flat_map(|file| &file.records) {
        match record.kind {
            Kind::Source => {
                if let Some(source) = redacted.sources.get_mut(&record.id) {
                    REDACTED.clone_into(&mut source.title);
                }
            }
            Kind::Geography => {
                if let Some(geography) = redacted.geographies.get_mut(&record.id) {
                    REDACTED.clone_into(&mut geography.basis);
                }
            }
            _ => {}
        }
    }
    redacted
}

pub fn report(report: &mut Report) {
    for name in [&mut report.project, &mut report.directory]
        .into_iter()
        .flatten()
    {
        REDACTED.clone_into(name);
    }
    for (index, project) in report.projects.iter_mut().flatten().enumerate() {
        project.project = format!("project {}", index + 1);
    }
    for source in &mut report.sources {
        let coverage = &mut source.coverage;
        if coverage.location.is_some() {
            coverage.location = Some(
                coverage
                    .shareable_location
                    .clone()
                    .unwrap_or_else(|| REDACTED.to_owned()),
            );
        }
    }
    for (index, file) in report.data_version.overrides.iter_mut().enumerate() {
        file.file.path = format!("override {}", index + 1);
    }
}
