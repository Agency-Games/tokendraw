use std::fs;

use serde_json::json;
use tempfile::TempDir;

use super::*;
use crate::adapters::Definition;
use crate::adapters::user::external;
use crate::ledger::Split;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// A directory tree with checkouts made of `.git` entries alone, under a
/// canonical root so paths compare on every platform.
struct Tree {
    _dir: TempDir,
    root: PathBuf,
}

impl Tree {
    fn new() -> TestResult<Self> {
        let dir = tempfile::tempdir()?;
        let root = canonical(dir.path()).ok_or("cannot resolve the temporary directory")?;
        Ok(Self { _dir: dir, root })
    }

    fn path(&self, relative: &str) -> PathBuf {
        relative
            .split('/')
            .fold(self.root.clone(), |path, part| path.join(part))
    }

    fn dir(&self, relative: &str) -> TestResult<PathBuf> {
        let path = self.path(relative);
        fs::create_dir_all(&path)?;
        Ok(path)
    }

    fn file(&self, relative: &str, text: &str) -> TestResult<PathBuf> {
        let path = self.path(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, text)?;
        Ok(path)
    }

    fn repository(&self, name: &str) -> TestResult<PathBuf> {
        self.dir(&format!("{name}/.git"))?;
        Ok(self.path(name))
    }

    fn worktree(&self, name: &str, common: &str) -> TestResult<PathBuf> {
        let base = name.rsplit('/').next().unwrap_or(name);
        let gitdir = format!("{common}/worktrees/{base}");
        self.file(&format!("{gitdir}/commondir"), "../..\n")?;
        self.file(
            &format!("{name}/.git"),
            &format!("gitdir: {}\n", self.path(&gitdir).display()),
        )?;
        Ok(self.path(name))
    }
}

fn s(path: &Path) -> String {
    display(path)
}

fn attributed(project: &Path, resolution: Resolution) -> Attributed {
    Attributed {
        project: Some(s(project)),
        resolution,
    }
}

#[test]
fn a_git_directory_makes_a_checkout_of_every_directory_below_it() -> TestResult {
    let tree = Tree::new()?;
    let repo = tree.repository("repo")?;
    assert_eq!(checkout(&tree.dir("repo/src/deep")?), Some(repo.clone()));
    assert_eq!(checkout(&repo), Some(repo));
    assert_eq!(checkout(&tree.dir("plain")?), None);
    assert_eq!(checkout(&tree.path("missing")), None);
    Ok(())
}

#[test]
fn a_linked_worktree_belongs_to_its_main_checkout() -> TestResult {
    let tree = Tree::new()?;
    let repo = tree.repository("repo")?;
    let worktree = tree.worktree("repo-trees/topic", "repo/.git")?;
    assert_eq!(checkout(&worktree), Some(repo.clone()));
    assert_eq!(checkout(&tree.dir("repo-trees/topic/public")?), Some(repo));
    Ok(())
}

#[test]
fn a_worktree_of_a_bare_repository_belongs_to_the_bare_repository() -> TestResult {
    let tree = Tree::new()?;
    let bare = tree.dir("app.git")?;
    let worktree = tree.worktree("app-main", "app.git")?;
    assert_eq!(checkout(&worktree), Some(bare));
    Ok(())
}

#[test]
fn a_submodule_is_its_own_checkout() -> TestResult {
    let tree = Tree::new()?;
    tree.repository("repo")?;
    tree.dir("repo/.git/modules/lib")?;
    let module = tree.file("repo/lib/.git", "gitdir: ../.git/modules/lib\n")?;
    let module = module.parent().ok_or("no parent")?;
    assert_eq!(checkout(module), Some(module.to_path_buf()));
    Ok(())
}

#[test]
fn a_worktree_whose_git_directory_is_gone_is_no_checkout() -> TestResult {
    let tree = Tree::new()?;
    tree.repository("repo")?;
    let worktree = tree.worktree("topic", "repo/.git")?;
    fs::remove_dir_all(tree.path("repo/.git/worktrees"))?;
    assert_eq!(checkout(&worktree), None);
    Ok(())
}

#[test]
fn labels_come_first_session_then_closest_directory() -> TestResult {
    let tree = Tree::new()?;
    let repo = tree.repository("repo")?;
    let inner = tree.dir("repo/inner")?;
    let mut ledger = Ledger::in_memory()?;
    ledger.set_labels(
        &Target::Directory(s(&repo)),
        &[(PROJECT.to_owned(), Some("/projects/outer".to_owned()))],
    )?;
    ledger.set_labels(
        &Target::Directory(s(&inner)),
        &[
            (PROJECT.to_owned(), Some("/projects/inner".to_owned())),
            ("topic".to_owned(), Some("t".to_owned())),
        ],
    )?;
    ledger.set_labels(
        &Target::Session {
            client: "codex".to_owned(),
            session_id: "s1".to_owned(),
        },
        &[(PROJECT.to_owned(), Some("/projects/session".to_owned()))],
    )?;
    let mut resolver = Resolver::new(&ledger)?;
    let label = |project: &str| attributed(Path::new(project), Resolution::Label);
    let deep = s(&tree.dir("repo/inner/deep")?);
    assert_eq!(
        resolver.attribute("codex", "s1", Some(&deep)),
        label("/projects/session")
    );
    assert_eq!(
        resolver.attribute("codex", "s1", None),
        label("/projects/session")
    );
    assert_eq!(
        resolver.attribute("claude-code", "s1", Some(&deep)),
        label("/projects/inner")
    );
    assert_eq!(
        resolver.attribute("codex", "s2", Some(&s(&repo))),
        label("/projects/outer")
    );
    Ok(())
}

#[test]
fn a_directory_label_reaches_removed_directories_below_it() -> TestResult {
    let tree = Tree::new()?;
    let container = tree.dir("repo-trees")?;
    let mut ledger = Ledger::in_memory()?;
    ledger.set_labels(
        &Target::Directory(s(&container)),
        &[(PROJECT.to_owned(), Some("/projects/repo".to_owned()))],
    )?;
    let mut resolver = Resolver::new(&ledger)?;
    let removed = s(&tree.path("repo-trees/gone/sub"));
    assert_eq!(
        resolver.attribute("pi", "s", Some(&removed)),
        attributed(Path::new("/projects/repo"), Resolution::Label)
    );
    Ok(())
}

#[test]
fn checkouts_then_records_then_ancestors_then_the_directory_itself() -> TestResult {
    let tree = Tree::new()?;
    let repo = tree.repository("repo")?;
    let worktree = tree.worktree("repo-trees/topic", "repo/.git")?;
    let mut ledger = Ledger::in_memory()?;
    ledger.record_checkouts(&[(s(&worktree), s(&repo))], "2026-10-01T00:00:00Z".parse()?)?;
    fs::remove_dir_all(&worktree)?;
    let plain = tree.dir("plain")?;
    let mut resolver = Resolver::new(&ledger)?;
    let mut attribute = |path: &Path| resolver.attribute("codex", "s", Some(&s(path)));
    assert_eq!(
        attribute(&tree.dir("repo/src")?),
        attributed(&repo, Resolution::Checkout)
    );
    assert_eq!(
        attribute(&worktree.join("public")),
        attributed(&repo, Resolution::Recorded)
    );
    assert_eq!(
        attribute(&tree.path("repo/.claude/worktrees/gone")),
        attributed(&repo, Resolution::Ancestor)
    );
    assert_eq!(
        attribute(&plain),
        attributed(&plain, Resolution::Unresolved)
    );
    let removed = tree.path("repo-trees/other");
    assert_eq!(
        attribute(&removed),
        attributed(&removed, Resolution::Unresolved)
    );
    assert_eq!(
        resolver.attribute("codex", "s", None),
        Attributed {
            project: None,
            resolution: Resolution::Unresolved
        }
    );
    Ok(())
}

/// One call of an external adapter's output.
fn call(id: &str, session: &str, cwd: Option<&Path>) -> String {
    json!({
        "id": id, "timestamp": "2026-09-01T10:00:00Z", "provider": "p", "model": "m",
        "session_id": session, "cwd": cwd.map(display),
        "tokens": {"uncached_input": 1, "output": 2},
    })
    .to_string()
}

fn ingest(ledger: &mut Ledger, dir: &Path, records: &[String]) -> TestResult {
    let definition = Definition {
        file: dir.join("command.toml"),
        text: "command".to_owned(),
    };
    ledger.ingest_output(
        "tool",
        &definition,
        &definition.file,
        records.join("\n").into_bytes(),
        external::reader(),
    )?;
    Ok(())
}

#[test]
fn a_worktree_recorded_while_it_existed_keeps_its_project() -> TestResult {
    let tree = Tree::new()?;
    let repo = tree.repository("repo")?;
    let worktree = tree.worktree("repo-trees/topic", "repo/.git")?;
    let mut ledger = Ledger::open(&tree.path("state/ledger.sqlite"))?;
    ingest(&mut ledger, &tree.root, &[call("r1", "s", Some(&worktree))])?;
    record(&mut ledger, "2026-09-02T00:00:00Z".parse()?)?;
    fs::remove_dir_all(&worktree)?;
    let mut resolver = Resolver::new(&ledger)?;
    let attributed = attribute(&mut ledger, &mut resolver, None, None)?;
    assert_eq!(attributed.len(), 1);
    assert_eq!(
        attributed[0].1,
        super::Attributed {
            project: Some(s(&repo)),
            resolution: Resolution::Recorded
        }
    );
    Ok(())
}

#[test]
fn a_project_is_named_by_its_checkout_and_labels_do_not_rename_it() -> TestResult {
    let tree = Tree::new()?;
    let a = tree.repository("a")?;
    let b = tree.repository("b")?;
    let mut ledger = Ledger::open(&tree.path("state/ledger.sqlite"))?;
    let mut resolver = Resolver::new(&ledger)?;
    assert_eq!(resolver.project(&tree.path("a/src")), s(&a));
    let gone = tree.path("gone");
    assert_eq!(resolver.project(&gone), s(&gone));
    ledger.set_labels(
        &Target::Directory(s(&a)),
        &[(PROJECT.to_owned(), Some(s(&b)))],
    )?;
    ledger.set_labels(
        &Target::Session {
            client: "tool".to_owned(),
            session_id: "labelled".to_owned(),
        },
        &[(PROJECT.to_owned(), Some(s(&a)))],
    )?;
    ingest(
        &mut ledger,
        &tree.root,
        &[
            call("r1", "labelled", Some(&b)),
            call("r2", "in-a", Some(&a)),
            call("r3", "in-a", None),
            call("r4", "plain", Some(&gone)),
        ],
    )?;
    let mut resolver = Resolver::new(&ledger)?;
    attribute(&mut ledger, &mut resolver, None, None)?;
    let calls = |project: &str| -> TestResult<u64> {
        Ok(ledger
            .calls(None, None, Some(project))?
            .iter()
            .map(|client| client.calls)
            .sum())
    };
    let a_name = resolver.project(&a);
    assert_eq!(a_name, s(&a));
    assert_eq!(calls(&a_name)?, 1, "only the session labelled a");
    assert_eq!(calls(&resolver.project(&b))?, 1, "the calls under a");
    assert_eq!(
        calls(&resolver.project(&gone))?,
        1,
        "an unresolved directory"
    );
    let by_project = ledger.usage_by_project(None, None, &Split::default())?;
    let unresolved: Vec<_> = by_project
        .iter()
        .filter(|row| row.resolution == Resolution::Unresolved.name())
        .map(|row| (row.project.clone(), row.usage.calls))
        .collect();
    assert_eq!(unresolved, [(None, 1), (Some(s(&gone)), 1)]);
    let total: u64 = by_project.iter().map(|row| row.usage.calls).sum();
    assert_eq!(total, 4);
    Ok(())
}

#[test]
fn suggestions_name_a_pattern_beside_the_longest_matching_checkout() {
    let checkouts: BTreeSet<PathBuf> = ["/w/acme", "/w/acme-tool", "/w/app"]
        .into_iter()
        .map(PathBuf::from)
        .collect();
    let suggest = |dir: &str| suggestion(Path::new(dir), &checkouts);
    let pair =
        |pattern: &str, project: &str| Some((PathBuf::from(pattern), PathBuf::from(project)));
    assert_eq!(
        suggest("/w/app-trees/topic/sub"),
        pair("/w/app-*", "/w/app")
    );
    assert_eq!(suggest("/w/app-fix"), pair("/w/app-*", "/w/app"));
    assert_eq!(suggest("/w/app_fix"), pair("/w/app_*", "/w/app"));
    assert_eq!(
        suggest("/w/acme-tool-trees/x"),
        pair("/w/acme-tool-*", "/w/acme-tool")
    );
    assert_eq!(suggest("/w/acme-other/x"), pair("/w/acme-*", "/w/acme"));
    assert_eq!(suggest("/w/appy/x"), None);
    assert_eq!(suggest("/w/app-"), None);
    assert_eq!(suggest("/elsewhere/app-trees/x"), None);
}

#[test]
fn a_star_matches_any_characters_within_one_component() {
    assert!(glob("app-*", "app-trees"));
    assert!(glob("a*bc", "abc"));
    assert!(glob("*", ""));
    assert!(glob("a*b*c", "axxbyyc"));
    assert!(!glob("app-*", "app"));
    assert!(!glob("a*c", "abd"));
    assert!(covers("/w/app-*", Path::new("/w/app-trees/topic")));
    assert!(!covers("/w/app-*", Path::new("/w/app/src")));
    assert!(!covers("/w/app-*", Path::new("/other/app-trees")));
}

fn labelled(ledger: &mut Ledger, labels: &[(&str, &str)]) -> TestResult<Resolver> {
    for (target, project) in labels {
        ledger.set_labels(
            &Target::Directory((*target).to_owned()),
            &[(PROJECT.to_owned(), Some((*project).to_owned()))],
        )?;
    }
    Ok(Resolver::new(ledger)?)
}

#[test]
fn a_pattern_assigns_only_what_no_evidence_or_exact_label_does() -> TestResult {
    let tree = Tree::new()?;
    let tools = tree.repository("app-tools")?;
    let recorded = tree.path("app-old");
    let exact = tree.path("app-exact");
    let gone = tree.path("app-gone/sub");
    let mut ledger = Ledger::in_memory()?;
    ledger.record_checkouts(
        &[(s(&recorded), "/w/old".to_owned())],
        "2026-10-01T00:00:00Z".parse()?,
    )?;
    let pattern = s(&tree.path("app-*"));
    let mut resolver = labelled(
        &mut ledger,
        &[(&pattern, "/w/app"), (&s(&exact), "/w/exact")],
    )?;
    let mut attribute = |dir: &Path| resolver.attribute("codex", "s", Some(&s(dir)));
    assert_eq!(
        attribute(&gone),
        attributed(Path::new("/w/app"), Resolution::Label)
    );
    assert_eq!(attribute(&tools), attributed(&tools, Resolution::Checkout));
    assert_eq!(
        attribute(&recorded),
        attributed(Path::new("/w/old"), Resolution::Recorded)
    );
    assert_eq!(
        attribute(&exact),
        attributed(Path::new("/w/exact"), Resolution::Label)
    );
    Ok(())
}

#[test]
fn the_deepest_match_wins_then_the_most_specific_pattern_in_any_order() -> TestResult {
    let tree = Tree::new()?;
    let path = |relative: &str| s(&tree.path(relative));
    let resolve = |labels: &[(String, &str)], dir: &str| -> TestResult<Option<String>> {
        let mut ledger = Ledger::in_memory()?;
        let labels: Vec<(&str, &str)> = labels
            .iter()
            .map(|(target, project)| (target.as_str(), *project))
            .collect();
        let mut resolver = labelled(&mut ledger, &labels)?;
        Ok(resolver.attribute("codex", "s", Some(&path(dir))).project)
    };
    let tie = [(path("a*bc"), "/p/first"), (path("ab*c"), "/p/second")];
    let reversed = [tie[1].clone(), tie[0].clone()];
    assert_eq!(resolve(&tie, "abc")?, Some("/p/second".to_owned()));
    assert_eq!(resolve(&reversed, "abc")?, Some("/p/second".to_owned()));
    let literal = [(path("*"), "/p/any"), (path("app-*"), "/p/app")];
    assert_eq!(resolve(&literal, "app-x")?, Some("/p/app".to_owned()));
    let deep = [(path("app-*"), "/p/app"), (path("app-x/*"), "/p/inner")];
    assert_eq!(resolve(&deep, "app-x/topic")?, Some("/p/inner".to_owned()));
    assert_eq!(resolve(&deep, "app-y/topic")?, Some("/p/app".to_owned()));
    Ok(())
}

#[test]
fn arguments_become_absolute_normalized_paths() -> TestResult {
    // On Windows a path without a drive is not absolute.
    let base = std::path::absolute("/")?;
    let cwd = base.join("work").join("repo");
    let home = base.join("users").join("me");
    let input = base.join("abs").join("..").join("b");
    assert!(input.is_absolute());
    let input = input.to_str().ok_or("the base is not UTF-8")?;
    let path = |arg: &str| absolute(arg, &cwd, Some(&home));
    let cases = [
        ("~/src/app/", home.join("src").join("app")),
        ("~", home.clone()),
        ("../other/./x", base.join("work").join("other").join("x")),
        (input, base.join("b")),
        ("~user/x", cwd.join("~user").join("x")),
    ];
    for (arg, expected) in cases {
        let found = path(arg).ok_or(arg)?;
        assert!(found.is_absolute(), "{arg} became {}", found.display());
        assert_eq!(found, expected, "{arg}");
    }
    assert_eq!(absolute("~/x", &cwd, None), None);
    Ok(())
}

#[test]
fn a_removed_nested_checkout_stays_selectable_by_its_path() -> TestResult {
    let tree = Tree::new()?;
    let outer = tree.repository("outer")?;
    let inner = tree.repository("outer/inner")?;
    let src = tree.dir("outer/inner/src")?;
    let mut ledger = Ledger::open(&tree.path("state/ledger.sqlite"))?;
    ingest(
        &mut ledger,
        &tree.root,
        &[
            call("r1", "in-inner", Some(&src)),
            call("r2", "in-outer", Some(&outer)),
        ],
    )?;
    record(&mut ledger, "2026-09-02T00:00:00Z".parse()?)?;
    fs::remove_dir_all(&inner)?;
    let mut resolver = Resolver::new(&ledger)?;
    let attributed = attribute(&mut ledger, &mut resolver, None, None)?;
    let inner_call = attributed
        .iter()
        .find(|(context, _)| context.session_id == "in-inner")
        .ok_or("no call in the inner checkout")?;
    assert_eq!(
        inner_call.1,
        super::Attributed {
            project: Some(s(&inner)),
            resolution: Resolution::Recorded
        }
    );
    let name = select(&ledger, &mut resolver, &inner)?.name;
    assert_eq!(name, s(&inner));
    let calls: u64 = ledger
        .calls(None, None, Some(&name))?
        .iter()
        .map(|client| client.calls)
        .sum();
    assert_eq!(calls, 1);
    Ok(())
}

#[test]
fn a_relative_working_directory_counts_as_none() -> TestResult {
    let ledger = Ledger::in_memory()?;
    let mut resolver = Resolver::new(&ledger)?;
    assert_eq!(
        resolver.attribute("tool", "s", Some("work/relative")),
        Attributed {
            project: None,
            resolution: Resolution::Unresolved
        }
    );
    Ok(())
}

#[test]
fn a_live_directory_names_its_checkout_even_where_a_removed_one_was() -> TestResult {
    let tree = Tree::new()?;
    let outer = tree.repository("outer")?;
    let inner = tree.repository("outer/inner")?;
    let mut ledger = Ledger::open(&tree.path("state/ledger.sqlite"))?;
    ingest(&mut ledger, &tree.root, &[call("r1", "old", Some(&inner))])?;
    record(&mut ledger, "2026-09-02T00:00:00Z".parse()?)?;
    fs::remove_dir_all(inner.join(".git"))?;
    ingest(&mut ledger, &tree.root, &[call("r2", "new", Some(&inner))])?;
    let mut resolver = Resolver::new(&ledger)?;
    assert_eq!(
        resolver.attribute("tool", "new", Some(&s(&inner))),
        attributed(&outer, Resolution::Checkout)
    );
    assert_eq!(resolver.project(&inner), s(&outer));
    Ok(())
}

#[test]
fn a_labelled_project_whose_checkout_is_gone_stays_selectable() -> TestResult {
    let tree = Tree::new()?;
    let outer = tree.repository("outer")?;
    let inner = tree.repository("outer/inner")?;
    let mut ledger = Ledger::open(&tree.path("state/ledger.sqlite"))?;
    ledger.set_labels(
        &Target::Session {
            client: "tool".to_owned(),
            session_id: "labelled".to_owned(),
        },
        &[(PROJECT.to_owned(), Some(s(&inner)))],
    )?;
    ingest(
        &mut ledger,
        &tree.root,
        &[
            call("r1", "labelled", Some(&outer)),
            call("r2", "other", Some(&outer)),
        ],
    )?;
    fs::remove_dir_all(inner.join(".git"))?;
    let mut resolver = Resolver::new(&ledger)?;
    assert_eq!(select(&ledger, &mut resolver, &inner)?.name, s(&inner));
    assert_eq!(select(&ledger, &mut resolver, &outer)?.name, s(&outer));
    assert_eq!(
        select(&ledger, &mut resolver, &outer.join("src"))?.name,
        s(&outer),
        "a path that names no project selects the one it is in"
    );
    Ok(())
}

#[test]
fn a_path_selects_a_project_unresolved_calls_or_nothing() -> TestResult {
    let tree = Tree::new()?;
    let app = tree.repository("app")?;
    let loose = tree.dir("loose")?;
    let mixed = tree.dir("mixed")?;
    let boxed = tree.dir("outer/box-1")?;
    let mut ledger = Ledger::open(&tree.path("state/ledger.sqlite"))?;
    ledger.set_labels(
        &Target::Session {
            client: "tool".to_owned(),
            session_id: "sent".to_owned(),
        },
        &[(PROJECT.to_owned(), Some(s(&app)))],
    )?;
    let mut resolver = labelled(
        &mut ledger,
        &[
            (&s(&tree.path("outer")), "/w/outer"),
            (&s(&tree.path("outer/box-*")), "/w/box"),
        ],
    )?;
    ingest(
        &mut ledger,
        &tree.root,
        &[
            call("r1", "app", Some(&app)),
            call("r2", "loose", Some(&loose)),
            call("r3", "unresolved", Some(&mixed.join("x"))),
            call("r4", "sent", Some(&mixed.join("y"))),
            call("r5", "boxed", Some(&boxed.join("topic"))),
        ],
    )?;
    let mut selected = |path: &Path| select(&ledger, &mut resolver, path);
    let selection = |path: &Path, selected: Selected| Selection {
        name: s(path),
        selected,
    };
    assert_eq!(selected(&app)?, selection(&app, Selected::Project));
    assert_eq!(
        selected(&app.join("src"))?,
        selection(&app, Selected::Project)
    );
    assert_eq!(selected(&loose)?, selection(&loose, Selected::Unresolved));
    let typo = tree.path("apq");
    assert_eq!(
        selected(&typo)?,
        selection(&typo, Selected::Nothing(Elsewhere::default()))
    );
    assert_eq!(
        selected(&mixed)?,
        selection(
            &mixed,
            Selected::Nothing(Elsewhere {
                projects: vec![(s(&app), 1)],
                unresolved: 1,
            })
        ),
        "a session label sends a call made there elsewhere"
    );
    assert_eq!(
        selected(&boxed)?,
        selection(
            &boxed,
            Selected::Nothing(Elsewhere {
                projects: vec![("/w/outer".to_owned(), 1)],
                unresolved: 0,
            })
        ),
        "an exact label above wins over a deeper pattern label"
    );
    Ok(())
}
