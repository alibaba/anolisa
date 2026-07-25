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
    } else if candidate.is_absolute() {
        let (base_str, glob_suffix) = split_path_pattern(pattern);
        let base = resolve_path(base_str, cwd);
        let escaped = Pattern::escape(
            base.to_str()
                .ok_or_else(|| format!("file pattern is not valid UTF-8: {}", base.display()))?,
        );
        if glob_suffix.is_empty() {
            escaped
        } else {
            let sep = if escaped.ends_with(std::path::MAIN_SEPARATOR) {
                ""
            } else {
                std::path::MAIN_SEPARATOR_STR
            };
            format!("{escaped}{sep}{glob_suffix}")
        }
    } else {
        join_glob(cwd, pattern)?
    };
    expand_glob(&glob_pattern, matches, max_matches)
}

/// Split a user-supplied pattern into `(literal_base, glob_suffix)`.
///
/// Splits the pattern at the first path segment that contains glob
/// metacharacters (`*`, `?`, `[`, `]`).  Everything above that segment
/// becomes the literal base (to be resolved and escaped); everything
/// from that segment onward is the glob suffix (preserved verbatim).
///
/// If no segment contains glob metacharacters, the entire pattern is
/// the literal base and the glob suffix is empty.
fn split_path_pattern(pattern: &str) -> (&str, String) {
    let sep = std::path::MAIN_SEPARATOR;
    let sep_len = sep.len_utf8();
    let is_glob_char = |c: char| matches!(c, '*' | '?' | '[' | ']');

    let Some(glob_index) = pattern
        .split(sep)
        .position(|segment| segment.chars().any(is_glob_char))
    else {
        return (pattern, String::new());
    };
    if glob_index == 0 {
        return ("", pattern.to_string());
    }

    let suffix_start = pattern
        .split(sep)
        .take(glob_index)
        .map(|segment| segment.len() + sep_len)
        .sum::<usize>();
    let base_end = suffix_start.saturating_sub(sep_len);
    // A leading separator forms the literal base when the first path component is a glob.
    let base = if base_end == 0 && pattern.starts_with(sep) {
        std::path::MAIN_SEPARATOR_STR
    } else {
        &pattern[..base_end]
    };
    (base, pattern[suffix_start..].to_string())
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

// resolve_path is provided by the parent module (super::resolve_path)
// and supports ~ expansion.
pub(super) use super::resolve_path;

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

    #[test]
    fn root_absolute_glob_preserves_literal_base() {
        assert_eq!(split_path_pattern("/*.rs"), ("/", "*.rs".to_string()));
        assert_eq!(
            split_path_pattern("/tmp*/file"),
            ("/", "tmp*/file".to_string())
        );
    }

    #[test]
    fn absolute_glob_uses_resolved_literal_base() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("src");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("lib.rs"), "lib").unwrap();
        std::fs::write(sub.join("main.rs"), "main").unwrap();
        std::fs::write(dir.path().join("readme.md"), "readme").unwrap();

        let abs_glob = dir.path().join("src").join("*.rs");
        let pattern = abs_glob.to_str().unwrap().to_string();

        let matches = expand_file_patterns(&[pattern], &PathBuf::from("/nonexistent"), 10).unwrap();

        assert_eq!(matches.paths.len(), 2);
        for p in &matches.paths {
            assert!(
                p.starts_with(&sub),
                "path {:?} should be under {:?}",
                p,
                sub
            );
            assert!(p.extension().is_some_and(|e| e == "rs"));
        }
    }
}
