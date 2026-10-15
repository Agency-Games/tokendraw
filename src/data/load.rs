use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::derive::{ShippedSites, resolve};
use super::records::{
    ClientAction, Finding, Geography, Location, Model, ParameterSet, Price, PriceCalibration,
    Provider, Source,
};
use super::{Data, Error, Kind, SHIPPED};

const MANIFEST: &str = "data.toml";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    id: Option<String>,
    review_interval_days: Option<i32>,
    price_review_interval_days: Option<i32>,
    #[serde(default)]
    sources: BTreeMap<String, Source>,
    #[serde(default)]
    parameter_set: Vec<ParameterSet>,
    #[serde(default)]
    price_calibration: Vec<PriceCalibration>,
    #[serde(default)]
    price: Vec<Price>,
    #[serde(default)]
    model: Vec<Model>,
    #[serde(default)]
    provider: Vec<Provider>,
    #[serde(default)]
    geography: Vec<Geography>,
    #[serde(default)]
    location: Vec<Location>,
    #[serde(default)]
    finding: Vec<Finding>,
    #[serde(default)]
    action: Vec<ClientAction>,
}

#[derive(Clone, Debug, Serialize)]
pub struct FileHash {
    pub path: String,
    pub sha256: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Replaced,
    Added,
}

#[derive(Clone, Debug, Serialize)]
pub struct OverrideRecord {
    pub kind: Kind,
    pub id: String,
    pub action: Action,
}

#[derive(Clone, Debug, Serialize)]
pub struct Override {
    #[serde(flatten)]
    pub file: FileHash,
    pub records: Vec<OverrideRecord>,
}

/// Carries every override's content hash, so any change to the inputs
/// changes the version a report shows.
#[derive(Clone, Debug, Serialize)]
pub struct Version {
    pub id: String,
    pub files: Vec<FileHash>,
    /// The SHA-256 of the shipped files' `"{path} {sha256}\n"` lines in load
    /// order. Unlike `id`, it changes with any edit to the shipped data.
    pub shipped_sha256: String,
    pub overrides: Vec<Override>,
}

#[must_use]
pub fn sha256(bytes: &[u8]) -> String {
    use fmt::Write;
    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

/// Hashes one `"{path} {sha256}"` line per file, in order, so the hash
/// changes with a file's path, its content or the order of the files.
fn listing_sha256(files: &[FileHash]) -> String {
    use fmt::Write;
    let listing = files.iter().fold(String::new(), |mut listing, file| {
        let _ = writeln!(listing, "{} {}", file.path, file.sha256);
        listing
    });
    sha256(listing.as_bytes())
}

/// Override files apply in file name order; a missing directory means none.
pub fn load(overrides: Option<&Path>) -> Result<Data, Error> {
    let files = match overrides {
        Some(dir) => read_dir(dir)?,
        None => Vec::new(),
    };
    load_from(&SHIPPED, &files)
}

/// Not recursive.
pub(super) fn read_dir(dir: &Path) -> Result<Vec<(String, String)>, Error> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(Error::List {
                dir: dir.to_path_buf(),
                error,
            });
        }
    };
    let mut paths = Vec::new();
    for entry in entries {
        let path = entry
            .map_err(|error| Error::List {
                dir: dir.to_path_buf(),
                error,
            })?
            .path();
        if path
            .extension()
            .is_some_and(|extension| extension == "toml")
            && path.is_file()
        {
            paths.push(path);
        }
    }
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            fs::read_to_string(&path)
                .map(|text| (path.display().to_string(), text))
                .map_err(|error| Error::Read { file: path, error })
        })
        .collect()
}

pub(super) struct Origin {
    file: String,
    shipped: bool,
}

/// The `usize` indexes the record's [`Origin`].
pub(super) type Records<T> = BTreeMap<String, (T, usize)>;

#[derive(Default)]
pub(super) struct Merged {
    pub(super) sources: Records<Source>,
    pub(super) parameter_sets: Records<ParameterSet>,
    pub(super) price_calibrations: Records<PriceCalibration>,
    pub(super) prices: Records<Price>,
    pub(super) models: Records<Model>,
    pub(super) providers: Records<Provider>,
    pub(super) geographies: Records<Geography>,
    pub(super) locations: Records<Location>,
    pub(super) findings: Records<Finding>,
    pub(super) actions: Records<ClientAction>,
}

pub(super) const SHIPPED_RECOVERY: &str = "the shipped data must be fixed in tokendraw";
const OVERRIDE_RECOVERY: &str = "fix or remove the file";

pub(super) fn invalid(origin: &Origin, problem: String) -> Error {
    Error::Invalid {
        file: origin.file.clone(),
        problem,
        recovery: if origin.shipped {
            SHIPPED_RECOVERY
        } else {
            OVERRIDE_RECOVERY
        },
    }
}

/// Shipped paths are below `data/`; override paths are shown as given.
pub(super) fn load_from(
    shipped: &[(&str, &str)],
    overrides: &[(String, String)],
) -> Result<Data, Error> {
    let mut origins = Vec::new();
    let mut merged = Merged::default();
    let mut manifest = None;
    let mut files = Vec::new();
    for (path, text) in shipped {
        let origin = Origin {
            file: format!("data/{path}"),
            shipped: true,
        };
        let file = parse(&origin, text)?;
        files.push(FileHash {
            path: origin.file.clone(),
            sha256: sha256(text.as_bytes()),
        });
        match (
            *path == MANIFEST,
            file.id.clone(),
            file.review_interval_days,
            file.price_review_interval_days,
        ) {
            (true, Some(id), Some(days), Some(price_days)) if days > 0 && price_days > 0 => {
                manifest = Some((id, days, price_days));
            }
            (true, _, _, _) => {
                return Err(invalid(
                    &origin,
                    "needs id and a positive review_interval_days and price_review_interval_days"
                        .to_owned(),
                ));
            }
            (false, None, None, None) => {}
            (false, _, _, _) => {
                return Err(invalid(
                    &origin,
                    format!("id and the review intervals belong in data/{MANIFEST}"),
                ));
            }
        }
        origins.push(origin);
        add(&mut merged, file, origins.len() - 1, &origins, None)?;
    }
    let shipped_sha256 = listing_sha256(&files);
    let Some((id, review_interval_days, price_review_interval_days)) = manifest else {
        return Err(Error::Invalid {
            file: format!("data/{MANIFEST}"),
            problem: "the file is missing".to_owned(),
            recovery: SHIPPED_RECOVERY,
        });
    };
    let shipped_sites = ShippedSites::of(&merged);
    let mut applied = Vec::new();
    for (path, text) in overrides {
        let origin = Origin {
            file: path.clone(),
            shipped: false,
        };
        let file = parse(&origin, text)?;
        if file.id.is_some()
            || file.review_interval_days.is_some()
            || file.price_review_interval_days.is_some()
        {
            return Err(invalid(
                &origin,
                "id and the review intervals are set by the shipped data only".to_owned(),
            ));
        }
        origins.push(origin);
        let mut records = Vec::new();
        add(
            &mut merged,
            file,
            origins.len() - 1,
            &origins,
            Some(&mut records),
        )?;
        applied.push(Override {
            file: FileHash {
                path: path.clone(),
                sha256: sha256(text.as_bytes()),
            },
            records,
        });
    }
    let data = resolve(
        merged,
        &origins,
        &shipped_sites,
        Version {
            id,
            files,
            shipped_sha256,
            overrides: applied,
        },
        [review_interval_days, price_review_interval_days],
    )?;
    Ok(data)
}

/// A parse error names its line. Where the file is valid TOML and a record
/// does not fit its kind, the error is that record's own, with the line of
/// its header.
fn parse(origin: &Origin, text: &str) -> Result<File, Error> {
    toml::from_str(text).map_err(|error| {
        let problem = misfit(text).unwrap_or_else(|| match error.span() {
            Some(span) => format!("line {}: {}", line_of(text, span.start), error.message()),
            None => error.message().to_owned(),
        });
        invalid(origin, problem)
    })
}

fn line_of(text: &str, offset: usize) -> usize {
    text[..offset].matches('\n').count() + 1
}

/// The first record, in file order, that does not deserialize into its
/// kind, with its own error.
pub(super) fn misfit(text: &str) -> Option<String> {
    type Records = Vec<toml::Spanned<toml::Table>>;
    type Check = fn(&toml::Table) -> Option<String>;
    #[derive(Deserialize)]
    struct Kinds {
        #[serde(default)]
        sources: BTreeMap<String, toml::Spanned<toml::Table>>,
        #[serde(default)]
        parameter_set: Records,
        #[serde(default)]
        price_calibration: Records,
        #[serde(default)]
        price: Records,
        #[serde(default)]
        model: Records,
        #[serde(default)]
        provider: Records,
        #[serde(default)]
        geography: Records,
        #[serde(default)]
        location: Records,
        #[serde(default)]
        finding: Records,
        #[serde(default)]
        action: Records,
    }
    fn fails<T: for<'de> Deserialize<'de>>(record: &toml::Table) -> Option<String> {
        toml::Value::Table(record.clone())
            .try_into::<T>()
            .err()
            .map(|error| error.message().to_owned())
    }
    fn id(record: &toml::Table) -> String {
        let field = |key: &str| record.get(key).and_then(toml::Value::as_str);
        match (field("id"), field("finding"), field("client")) {
            (Some(id), _, _) => id.to_owned(),
            (None, Some(finding), Some(client)) => super::records::action_id(finding, client),
            _ => "without an id".to_owned(),
        }
    }
    let records: Kinds = toml::from_str(text).ok()?;
    let mut found: Vec<(usize, String)> = Vec::new();
    for (key, source) in &records.sources {
        if let Some(message) = fails::<Source>(source.get_ref()) {
            found.push((
                source.span().start,
                format!("{} {key}: {message}", Kind::Source),
            ));
        }
    }
    let kinds: [(Kind, &Records, Check); 9] = [
        (
            Kind::ParameterSet,
            &records.parameter_set,
            fails::<ParameterSet>,
        ),
        (
            Kind::PriceCalibration,
            &records.price_calibration,
            fails::<PriceCalibration>,
        ),
        (Kind::Price, &records.price, fails::<Price>),
        (Kind::Model, &records.model, fails::<Model>),
        (Kind::Provider, &records.provider, fails::<Provider>),
        (Kind::Geography, &records.geography, fails::<Geography>),
        (Kind::Location, &records.location, fails::<Location>),
        (Kind::Finding, &records.finding, fails::<Finding>),
        (Kind::Action, &records.action, fails::<ClientAction>),
    ];
    for (kind, list, check) in kinds {
        for record in list {
            if let Some(message) = check(record.get_ref()) {
                found.push((
                    record.span().start,
                    format!("{kind} {}: {message}", id(record.get_ref())),
                ));
            }
        }
    }
    found
        .into_iter()
        .min_by_key(|(start, _)| *start)
        .map(|(start, problem)| format!("line {}: {problem}", line_of(text, start)))
}

/// Adds a file's records. A shipped file (`applied` is `None`) may not
/// repeat an id; an override replaces a shipped record but may not repeat
/// an id another override set.
fn add(
    merged: &mut Merged,
    file: File,
    origin: usize,
    origins: &[Origin],
    applied: Option<&mut Vec<OverrideRecord>>,
) -> Result<(), Error> {
    let File {
        sources,
        parameter_set,
        price_calibration,
        price,
        model,
        provider,
        geography,
        location,
        finding,
        action,
        ..
    } = file;
    let mut adding = Adding {
        origin,
        origins,
        seen: BTreeSet::new(),
        applied,
    };
    adding.all(&mut merged.sources, Kind::Source, sources)?;
    adding.all(
        &mut merged.parameter_sets,
        Kind::ParameterSet,
        parameter_set.into_iter().map(|r| (r.id.clone(), r)),
    )?;
    adding.all(
        &mut merged.price_calibrations,
        Kind::PriceCalibration,
        price_calibration.into_iter().map(|r| (r.id.clone(), r)),
    )?;
    adding.all(
        &mut merged.prices,
        Kind::Price,
        price.into_iter().map(|r| (r.id.clone(), r)),
    )?;
    adding.all(
        &mut merged.models,
        Kind::Model,
        model.into_iter().map(|r| (r.id.clone(), r)),
    )?;
    adding.all(
        &mut merged.providers,
        Kind::Provider,
        provider.into_iter().map(|r| (r.id.clone(), r)),
    )?;
    adding.all(
        &mut merged.geographies,
        Kind::Geography,
        geography.into_iter().map(|r| (r.id.clone(), r)),
    )?;
    adding.all(
        &mut merged.locations,
        Kind::Location,
        location.into_iter().map(|r| (r.id.clone(), r)),
    )?;
    adding.all(
        &mut merged.findings,
        Kind::Finding,
        finding.into_iter().map(|r| (r.id.clone(), r)),
    )?;
    adding.all(
        &mut merged.actions,
        Kind::Action,
        action.into_iter().map(|r| (r.id(), r)),
    )?;
    Ok(())
}

struct Adding<'a> {
    origin: usize,
    origins: &'a [Origin],
    seen: BTreeSet<(Kind, String)>,
    applied: Option<&'a mut Vec<OverrideRecord>>,
}

impl Adding<'_> {
    fn all<T>(
        &mut self,
        records: &mut Records<T>,
        kind: Kind,
        items: impl IntoIterator<Item = (String, T)>,
    ) -> Result<(), Error> {
        for (id, record) in items {
            insert(
                records,
                kind,
                id,
                record,
                self.origin,
                self.origins,
                &mut self.seen,
                self.applied.as_deref_mut(),
            )?;
        }
        Ok(())
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "one insertion rule shared by every kind of record"
)]
fn insert<T>(
    records: &mut Records<T>,
    kind: Kind,
    id: String,
    record: T,
    origin: usize,
    origins: &[Origin],
    seen: &mut BTreeSet<(Kind, String)>,
    applied: Option<&mut Vec<OverrideRecord>>,
) -> Result<(), Error> {
    if id.is_empty() {
        return Err(invalid(
            &origins[origin],
            format!("a {kind} has an empty id"),
        ));
    }
    if !seen.insert((kind, id.clone())) {
        return Err(invalid(
            &origins[origin],
            format!("{kind} {id} appears twice"),
        ));
    }
    let previous = records.insert(id.clone(), (record, origin));
    match (applied, previous) {
        (None, None) => {}
        (None, Some((_, earlier))) => {
            return Err(invalid(
                &origins[origin],
                format!("{kind} {id} is also in {}", origins[earlier].file),
            ));
        }
        (Some(applied), None) => applied.push(OverrideRecord {
            kind,
            id,
            action: Action::Added,
        }),
        (Some(_), Some((_, earlier))) if !origins[earlier].shipped => {
            return Err(invalid(
                &origins[origin],
                format!("{kind} {id} is also set by {}", origins[earlier].file),
            ));
        }
        (Some(applied), Some(_)) => applied.push(OverrideRecord {
            kind,
            id,
            action: Action::Replaced,
        }),
    }
    Ok(())
}

pub(super) fn records<T>(records: Records<T>) -> BTreeMap<String, T> {
    records
        .into_iter()
        .map(|(id, (record, _))| (id, record))
        .collect()
}
