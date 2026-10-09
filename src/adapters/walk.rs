use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use thiserror::Error;

#[derive(Debug, Error)]
#[error("cannot read {}: {error}; check that it is readable", path.display())]
pub struct Error {
    pub path: PathBuf,
    pub error: io::Error,
}

#[derive(Debug, Default)]
pub enum Root {
    #[default]
    Missing,
    Listed,
    Unlisted(Error),
}

#[derive(Debug, Default)]
pub struct Walk {
    pub root: Root,
    pub files: Vec<PathBuf>,
    /// A failure below the root does not stop the walk.
    pub failures: Vec<Error>,
}

/// Symbolic links are not followed, so a link cycle cannot make the walk
/// endless.
#[must_use]
pub fn files_with_extension(root: &Path, extension: &str) -> Walk {
    let mut walk = Walk::default();
    let mut pending = match fs::read_dir(root) {
        Ok(entries) => {
            walk.root = Root::Listed;
            vec![(root.to_path_buf(), entries)]
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => return walk,
        Err(error) => {
            walk.root = Root::Unlisted(Error {
                path: root.to_path_buf(),
                error,
            });
            return walk;
        }
    };
    while let Some((dir, entries)) = pending.pop() {
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    walk.failures.push(Error {
                        path: dir.clone(),
                        error,
                    });
                    continue;
                }
            };
            let path = entry.path();
            let kind = match entry.file_type() {
                Ok(kind) => kind,
                Err(error) => {
                    walk.failures.push(Error { path, error });
                    continue;
                }
            };
            if kind.is_dir() {
                match fs::read_dir(&path) {
                    Ok(entries) => pending.push((path, entries)),
                    // Removed since its parent was listed.
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => walk.failures.push(Error { path, error }),
                }
            } else if kind.is_file() && path.extension().is_some_and(|found| found == extension) {
                walk.files.push(path);
            }
        }
    }
    walk.files.sort();
    walk
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_matching_files_in_nested_directories_in_sorted_order()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        fs::create_dir_all(root.path().join("b/c"))?;
        fs::write(root.path().join("b/c/two.jsonl"), "")?;
        fs::write(root.path().join("one.jsonl"), "")?;
        fs::write(root.path().join("b/notes.txt"), "")?;
        let walk = files_with_extension(root.path(), "jsonl");
        assert!(matches!(walk.root, Root::Listed));
        assert!(walk.failures.is_empty());
        assert_eq!(
            walk.files,
            vec![
                root.path().join("b/c/two.jsonl"),
                root.path().join("one.jsonl")
            ]
        );
        Ok(())
    }

    #[test]
    fn a_missing_directory_has_no_files() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let walk = files_with_extension(&root.path().join("absent"), "jsonl");
        assert!(matches!(walk.root, Root::Missing));
        assert_eq!(walk.files, Vec::<PathBuf>::new());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_directory_is_a_failure_and_its_siblings_are_still_found()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir()?;
        let closed = root.path().join("closed");
        fs::create_dir_all(&closed)?;
        fs::create_dir_all(root.path().join("open"))?;
        fs::write(closed.join("hidden.jsonl"), "")?;
        fs::write(root.path().join("open/seen.jsonl"), "")?;
        fs::set_permissions(&closed, fs::Permissions::from_mode(0o000))?;
        // A superuser reads it regardless, so there is nothing to test.
        let readable = fs::read_dir(&closed).is_ok();
        let walk = files_with_extension(root.path(), "jsonl");
        fs::set_permissions(&closed, fs::Permissions::from_mode(0o755))?;
        if readable {
            return Ok(());
        }
        assert!(matches!(walk.root, Root::Listed));
        assert_eq!(walk.files, vec![root.path().join("open/seen.jsonl")]);
        assert_eq!(walk.failures.len(), 1);
        assert_eq!(walk.failures[0].path, closed);
        Ok(())
    }
}
