//! `npm/publish.sh` publishes one npm package per target in
//! `npm/platforms.json` from the archives `dist` builds for the targets in
//! `dist-workspace.toml`, so the two lists must name the same targets.

use std::collections::BTreeSet;

use anyhow::Context as _;

const PLATFORMS: &str = include_str!("../../npm/platforms.json");
const DIST_WORKSPACE: &str = include_str!("../../dist-workspace.toml");

#[test]
fn npm_packages_cover_the_dist_targets() -> anyhow::Result<()> {
    let platforms: serde_json::Map<String, serde_json::Value> = serde_json::from_str(PLATFORMS)?;
    let npm: BTreeSet<&str> = platforms.keys().map(String::as_str).collect();
    let document: toml::Table = toml::from_str(DIST_WORKSPACE)?;
    let dist = document
        .get("dist")
        .and_then(|dist| dist.get("targets"))
        .and_then(toml::Value::as_array)
        .context("no targets in [dist]")?
        .iter()
        .map(|target| target.as_str().context("a target is not a string"))
        .collect::<anyhow::Result<BTreeSet<&str>>>()?;
    assert_eq!(
        npm, dist,
        "add each dist target to npm/platforms.json, and remove the ones dist no longer builds"
    );
    Ok(())
}
