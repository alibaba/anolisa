//! Shared path expansion for file-discovery tools.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use glob::{glob_with, MatchOptions, Pattern};

const MAX_MATCHES: usize = 100;

pub(super) struct FileMatches {
    pub paths: Vec<PathBuf>,
    pub truncated: bool,
}

pub(super) fn expand_file_patterns(
    patterns: &[String],
    cwd: &Path,
    max_matches: usize,
) -> Result<FileMatches, String> {
    let mut matches = BTreeSet::new();
    let mut truncated = false;
    let max_matches = max_matches.min(MAX_MATCHES);

    for pattern in patterns {
        if expand_pattern(pattern, cwd, &mut matches, max_matches)? {
            truncated = true;
            break;
        }
    }

    Ok(FileMatches {
        paths: matches.into_iter().collect(),
        truncated,
    })
}

fn expand_pattern(
    pattern: &str,
    cwd: &Path,
    matches: &mut BTreeSet<PathBuf>,
    max_matches: usize,
) -> Result<bool, String> {
    if pattern.trim().is_empty() {
        return Err("file pattern must not be empty".to_string());
    }

    let candidate = resolve_path(pattern, cwd);
    if candidate.is_file() {
        return Ok(insert_match(matches, candidate, max_matches));
    }

    let glob_pattern = if candidate.is_dir() {
        join_glob(&candidate, "**/*")?
    } else if Path::new(pattern).is_absolute() {
        pattern.to_string()
    } else {
        join_glob(cwd, pattern)?
    };
    expand_glob(&glob_pattern, matches, max_matches)
}

fn join_glob(base: &Path, pattern: &str) -> Result<String, String> {
    let base = base
        .to_str()
        .ok_or_else(|| format!("file pattern is not valid UTF-8: {}", base.display()))?;
    let base = Pattern::escape(base);
    let separator = if base.ends_with(std::path::MAIN_SEPARATOR) {
        ""
    } else {
        std::path::MAIN_SEPARATOR_STR
    };
    Ok(format!("{base}{separator}{pattern}"))
}

fn expand_glob(
    pattern: &str,
    matches: &mut BTreeSet<PathBuf>,
    max_matches: usize,
) -> Result<bool, String> {
    Pattern::new(pattern).map_err(|error| format!("invalid glob pattern '{pattern}': {error}"))?;

    let options = MatchOptions {
        case_sensitive: !cfg!(any(target_os = "macos", target_os = "windows")),
        require_literal_separator: false,
        require_literal_leading_dot: false,
    };
    let entries = glob_with(pattern, options)
        .map_err(|error| format!("invalid glob pattern '{pattern}': {error}"))?;

    for entry in entries {
        match entry {
            Ok(path) if path.is_file() => {
                if insert_match(matches, path, max_matches) {
                    return Ok(true);
                }
            }
            Ok(_) => {}
            Err(_) => continue,
        }
    }
    Ok(false)
}

fn insert_match(matches: &mut BTreeSet<PathBuf>, path: PathBuf, max_matches: usize) -> bool {
    if matches.contains(&path) {
        return false;
    }
    if matches.len() >= max_matches {
        return true;
    }
    matches.insert(path);
    false
}

pub(super) fn resolve_path(path: &str, cwd: &Path) -> PathBuf {
    let path = Path::new(path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_exact_files_and_globs_without_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/nested")).unwrap();
        std::fs::write(dir.path().join("src/lib.rs"), "lib").unwrap();
        std::fs::write(dir.path().join("src/nested/mod.rs"), "mod").unwrap();
        std::fs::write(dir.path().join("src/readme.md"), "readme").unwrap();

        let matches = expand_file_patterns(
            &[
                "src/**/*.rs".to_string(),
                "src/lib.rs".to_string(),
                "src/readme.md".to_string(),
            ],
            dir.path(),
            10,
        )
        .unwrap();

        assert_eq!(matches.paths.len(), 3);
        assert!(!matches.truncated);
    }

    #[test]
    fn caps_matches() {
        let dir = tempfile::tempdir().unwrap();
        for index in 0..3 {
            std::fs::write(dir.path().join(format!("{index}.txt")), "text").unwrap();
        }

        let matches = expand_file_patterns(&["*.txt".to_string()], dir.path(), 2).unwrap();

        assert_eq!(matches.paths.len(), 2);
        assert!(matches.truncated);
    }

    #[test]
    fn treats_glob_metacharacters_in_base_path_as_literals() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("work[tree]");
        std::fs::create_dir(&base).unwrap();
        std::fs::write(base.join("lib.rs"), "lib").unwrap();

        let matches = expand_file_patterns(&["*.rs".to_string()], &base, 10).unwrap();

        assert_eq!(matches.paths, [base.join("lib.rs")]);
    }
}
