//! `release.yml` is written by `dist generate` from the action hashes in
//! `dist-workspace.toml`, but Dependabot updates only the workflow. These
//! tests keep the two in step, so that generating the workflow again after a
//! `dist` upgrade does not bring back older hashes.

use std::collections::BTreeMap;

use anyhow::Context as _;

const WORKFLOW: &str = include_str!("../../.github/workflows/release.yml");
const DIST_WORKSPACE: &str = include_str!("../../dist-workspace.toml");

fn workflow_pins(workflow: &str) -> BTreeMap<String, Vec<String>> {
    let mut pins: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for line in workflow.lines() {
        let Some(rest) = line
            .trim_start()
            .trim_start_matches("- ")
            .strip_prefix("uses:")
        else {
            continue;
        };
        let reference = rest.split('#').next().unwrap_or_default().trim();
        if reference.starts_with("./") {
            continue;
        }
        if let Some((action, commit)) = reference.split_once('@') {
            pins.entry(action.to_owned())
                .or_default()
                .push(commit.to_owned());
        }
    }
    pins
}

fn dist_pins(dist_workspace: &str) -> anyhow::Result<BTreeMap<String, String>> {
    let document: toml::Table = toml::from_str(dist_workspace)?;
    let commits = document
        .get("dist")
        .and_then(|dist| dist.get("github-action-commits"))
        .and_then(toml::Value::as_table)
        .context("no [dist.github-action-commits] table")?;
    commits
        .iter()
        .map(|(action, commit)| {
            let commit = commit
                .as_str()
                .with_context(|| format!("the commit of {action} is not a string"))?;
            Ok((action.clone(), commit.to_owned()))
        })
        .collect()
}

fn mismatches(workflow: &str, dist_workspace: &str) -> anyhow::Result<Vec<String>> {
    let used = workflow_pins(workflow);
    let listed = dist_pins(dist_workspace)?;
    let mut problems = Vec::new();
    for (action, commits) in &used {
        for commit in commits {
            match listed.get(action) {
                None => problems.push(format!(
                    "{action}@{commit} is used in release.yml but missing from [dist.github-action-commits]"
                )),
                Some(expected) if expected != commit => problems.push(format!(
                    "{action} is pinned to {commit} in release.yml but to {expected} in dist-workspace.toml"
                )),
                Some(_) => {}
            }
        }
    }
    for action in listed.keys() {
        if !used.contains_key(action) {
            problems.push(format!(
                "{action} is listed in [dist.github-action-commits] but not used in release.yml"
            ));
        }
    }
    Ok(problems)
}

#[test]
fn release_workflow_uses_the_action_commits_dist_workspace_lists() -> anyhow::Result<()> {
    let problems = mismatches(WORKFLOW, DIST_WORKSPACE)?;
    assert!(
        problems.is_empty(),
        "copy each new hash in `release.yml`, with its version comment, into `[dist.github-action-commits]` in `dist-workspace.toml`:\n{}",
        problems.join("\n")
    );
    Ok(())
}

#[test]
fn a_hash_changed_only_in_the_workflow_is_reported() -> anyhow::Result<()> {
    let dist_workspace = "[dist.github-action-commits]\n\"actions/checkout\" = \"aaaa\"\n";
    let workflow = "steps:\n  - uses: actions/checkout@bbbb # v7.0.2\n  - uses: ./.github/workflows/publish-npm.yml\n";
    assert_eq!(
        mismatches(workflow, dist_workspace)?,
        ["actions/checkout is pinned to bbbb in release.yml but to aaaa in dist-workspace.toml"]
    );
    Ok(())
}

#[test]
fn an_action_missing_from_either_file_is_reported() -> anyhow::Result<()> {
    let dist_workspace = "[dist.github-action-commits]\n\"actions/attest\" = \"cccc\"\n";
    let workflow = "      - name: Fetch\n        uses: actions/download-artifact@dddd\n";
    assert_eq!(
        mismatches(workflow, dist_workspace)?,
        [
            "actions/download-artifact@dddd is used in release.yml but missing from [dist.github-action-commits]",
            "actions/attest is listed in [dist.github-action-commits] but not used in release.yml",
        ]
    );
    Ok(())
}
