use std::fs;

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn context(client: &str, session: &str, cwd: Option<&Path>, calls: u64, day: u8) -> Context {
    let time = |hour: u8| -> Timestamp {
        format!("2026-09-{day:02}T{hour:02}:00:00Z")
            .parse()
            .unwrap_or_default()
    };
    Context {
        client: client.to_owned(),
        session_id: session.to_owned(),
        cwd: cwd.map(projects::display),
        calls,
        earliest: time(1),
        latest: time(2),
    }
}

fn unresolved(dir: Option<&Path>) -> Attributed {
    Attributed {
        project: dir.map(projects::display),
        resolution: Resolution::Unresolved,
    }
}

fn period() -> Period {
    Period {
        start: None,
        end: None,
    }
}

#[test]
fn projects_count_calls_by_how_they_were_found_most_first() {
    let found = |project: &str, resolution| Attributed {
        project: Some(project.to_owned()),
        resolution,
    };
    let attributed = [
        (
            context("codex", "a", None, 2, 1),
            found("/p/small", Resolution::Label),
        ),
        (
            context("codex", "b", None, 5, 1),
            found("/p/large", Resolution::Checkout),
        ),
        (
            context("pi", "c", None, 3, 1),
            found("/p/large", Resolution::Recorded),
        ),
        (
            context("pi", "d", None, 1, 1),
            found("/p/large", Resolution::Ancestor),
        ),
    ];
    let inventory = build(&attributed, &BTreeSet::new(), &BTreeSet::new(), period());
    let projects: Vec<(&str, u64, ResolvedBy)> = inventory
        .projects
        .iter()
        .map(|project| (project.project.as_str(), project.calls, project.resolved_by))
        .collect();
    assert_eq!(
        projects,
        [
            (
                "/p/large",
                9,
                ResolvedBy {
                    label: 0,
                    checkout: 5,
                    recorded: 3,
                    ancestor: 1
                }
            ),
            (
                "/p/small",
                2,
                ResolvedBy {
                    label: 2,
                    ..ResolvedBy::default()
                }
            ),
        ]
    );
    assert_eq!(inventory.unresolved.calls, 0);
}

#[test]
fn unresolved_directories_group_under_a_suggestion_or_their_existing_parent() -> TestResult {
    let dir = tempfile::tempdir()?;
    let root = dir.path();
    let repo = root.join("repo");
    let container = root.join("repo-trees");
    fs::create_dir_all(&container)?;
    let plain = root.join("plain");
    fs::create_dir_all(&plain)?;
    let gone_a = container.join("a");
    let gone_b = container.join("b").join("sub");
    let gone_plain = plain.join("gone");
    let attributed = [
        (
            context("codex", "s1", Some(&gone_a), 4, 1),
            unresolved(Some(&gone_a)),
        ),
        (
            context("claude-code", "s2", Some(&gone_b), 6, 3),
            unresolved(Some(&gone_b)),
        ),
        (
            context("claude-code", "s2", Some(&gone_a), 1, 2),
            unresolved(Some(&gone_a)),
        ),
        (
            context("pi", "s3", Some(&gone_plain), 2, 1),
            unresolved(Some(&gone_plain)),
        ),
        (
            context("pi", "s3", Some(&plain), 1, 1),
            unresolved(Some(&plain)),
        ),
        (context("codex", "s4", None, 7, 5), unresolved(None)),
    ];
    let checkouts = BTreeSet::from([repo.clone()]);
    let all = attributed
        .iter()
        .filter_map(|(_, attributed)| attributed.project.clone())
        .collect();
    let inventory = build(&attributed, &checkouts, &all, period());
    let unresolved = &inventory.unresolved;
    assert_eq!(unresolved.calls, 21);
    assert_eq!(unresolved.groups.len(), 2);
    let worktrees = &unresolved.groups[0];
    let pattern = projects::display(&root.join("repo-*"));
    assert_eq!(worktrees.directory, pattern);
    assert_eq!(worktrees.calls, 11);
    assert_eq!(worktrees.sessions, 2);
    assert_eq!(worktrees.clients, ["claude-code", "codex"]);
    assert_eq!(worktrees.first_call, "2026-09-01T01:00:00Z".parse()?);
    assert_eq!(worktrees.last_call, "2026-09-03T02:00:00Z".parse()?);
    let directories: Vec<(String, u64)> = worktrees
        .directories
        .iter()
        .map(|dir| (dir.directory.clone(), dir.calls.calls))
        .collect();
    assert_eq!(
        directories,
        [
            (projects::display(&gone_b), 6),
            (projects::display(&gone_a), 5),
        ]
    );
    let suggestion = worktrees.suggestion.as_ref().ok_or("no suggestion")?;
    assert_eq!(suggestion.project, projects::display(&repo));
    assert_eq!(
        suggestion.command,
        Some(format!(
            "tokendraw label dir {} project={}",
            quoted(&pattern),
            quoted(&projects::display(&repo))
        ))
    );
    let plain_group = &unresolved.groups[1];
    assert_eq!(plain_group.directory, projects::display(&plain));
    assert_eq!(plain_group.calls, 3);
    assert!(plain_group.suggestion.is_none());
    let without = unresolved
        .without_directory
        .as_ref()
        .ok_or("no calls without a directory")?;
    assert_eq!(without.calls, 7);
    assert_eq!(
        without.sessions,
        [Session {
            client: "codex".to_owned(),
            session_id: "s4".to_owned()
        }]
    );
    Ok(())
}

#[test]
fn a_word_with_shell_characters_is_quoted() {
    let posix = |text| quoted_for(text, Shell::Posix);
    assert_eq!(
        posix("/users/me/src/app-worktrees"),
        "/users/me/src/app-worktrees"
    );
    assert_eq!(posix("/users/me/my repo"), "'/users/me/my repo'");
    assert_eq!(posix("/it's"), r"'/it'\''s'");
    assert_eq!(posix("/a,b"), "/a,b");
    assert_eq!(posix("~/src/app-*"), "'~/src/app-*'");
    assert_eq!(posix(""), "''");
}

#[test]
fn a_word_for_powershell_doubles_its_single_quotes() {
    let powershell = |text| quoted_for(text, Shell::PowerShell);
    assert_eq!(powershell("C:/work/app"), "C:/work/app");
    assert_eq!(powershell("~/src/my repo"), "'~/src/my repo'");
    assert_eq!(powershell("~/it's"), "'~/it''s'");
    assert_eq!(powershell("~/it\u{2019}s"), "'~/it\u{2019}\u{2019}s'");
    assert_eq!(powershell("~/a,b"), "'~/a,b'");
    assert_eq!(powershell("@app"), "'@app'");
    assert_eq!(powershell("~/src/app-*"), "'~/src/app-*'");
    assert_eq!(powershell(""), "''");
}

#[test]
fn the_native_shell_is_powershell_only_on_windows() {
    assert_eq!(Shell::NATIVE == Shell::PowerShell, cfg!(windows));
}

#[test]
fn a_suggestion_that_would_match_another_group_gets_no_command() {
    let checkouts = BTreeSet::from([PathBuf::from("/w/app"), PathBuf::from("/w/app-tool")]);
    let app = Path::new("/w/app-x");
    let tool = Path::new("/w/app-tool-y");
    let attributed = [
        (
            context("codex", "s1", Some(app), 3, 1),
            unresolved(Some(app)),
        ),
        (
            context("codex", "s2", Some(tool), 2, 1),
            unresolved(Some(tool)),
        ),
    ];
    let all = BTreeSet::from([app, tool].map(projects::display));
    let inventory = build(&attributed, &checkouts, &all, period());
    let pattern = projects::display(&Path::new("/w").join("app-tool-*"));
    let command = format!(
        "tokendraw label dir {} project=/w/app-tool",
        quoted(&pattern)
    );
    let app_pattern = projects::display(&Path::new("/w").join("app-*"));
    let suggestions: Vec<(&str, &str, Option<&str>)> = inventory
        .unresolved
        .groups
        .iter()
        .filter_map(|group| {
            let suggestion = group.suggestion.as_ref()?;
            Some((
                group.directory.as_str(),
                suggestion.project.as_str(),
                suggestion.command.as_deref(),
            ))
        })
        .collect();
    assert_eq!(
        suggestions,
        [
            (app_pattern.as_str(), "/w/app", None),
            (pattern.as_str(), "/w/app-tool", Some(command.as_str())),
        ]
    );
}

#[test]
fn an_overlap_outside_the_range_still_withholds_the_command() {
    let checkouts = BTreeSet::from([PathBuf::from("/w/app"), PathBuf::from("/w/app-tool")]);
    let app = Path::new("/w/app-x");
    let earlier = Path::new("/w/app-tool-y");
    let attributed = [(
        context("codex", "s1", Some(app), 3, 1),
        unresolved(Some(app)),
    )];
    let all = BTreeSet::from([app, earlier].map(projects::display));
    let inventory = build(&attributed, &checkouts, &all, period());
    let group = &inventory.unresolved.groups[0];
    let suggestion = group.suggestion.as_ref();
    assert_eq!(suggestion.map(|s| s.project.as_str()), Some("/w/app"));
    assert_eq!(suggestion.and_then(|s| s.command.as_deref()), None);
    let alone = build(
        &attributed,
        &checkouts,
        &BTreeSet::from([projects::display(app)]),
        period(),
    );
    let suggestion = alone.unresolved.groups[0].suggestion.as_ref();
    assert!(suggestion.and_then(|s| s.command.as_deref()).is_some());
}
