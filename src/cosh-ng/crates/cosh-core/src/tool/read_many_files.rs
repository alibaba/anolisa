//! Bounded multi-file reads with optional glob expansion.

use std::path::Path;

use async_trait::async_trait;
use serde_json::Value;
use tokio::io::AsyncReadExt;

use super::file_patterns::expand_file_patterns;
use super::{Tool, ToolContext, ToolKind, ToolResult};

const MAX_FILES: usize = 50;
const MAX_FILE_BYTES: usize = 64 * 1024;
const MAX_TOTAL_BYTES: usize = 256 * 1024;

pub(super) struct ReadManyFilesTool;

#[async_trait]
impl Tool for ReadManyFilesTool {
    fn name(&self) -> &str {
        "read_many_files"
    }

    fn description(&self) -> &str {
        "Read and concatenate multiple text files. Paths may be exact files, directories, or glob patterns."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "paths": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "File paths, directories, or glob patterns to read"
                }
            },
            "required": ["paths"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::ReadOnly
    }

    async fn invoke(&self, params: Value, ctx: &ToolContext) -> Result<ToolResult, String> {
        let patterns = params
            .get("paths")
            .and_then(Value::as_array)
            .ok_or("missing 'paths' parameter")?
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(ToOwned::to_owned)
                    .ok_or_else(|| "'paths' entries must be strings".to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        if patterns.is_empty() {
            return Ok(ToolResult::error("'paths' must not be empty"));
        }

        let cwd = ctx.cwd.clone();
        let matches =
            tokio::task::spawn_blocking(move || expand_file_patterns(&patterns, &cwd, MAX_FILES))
                .await
                .map_err(|error| format!("File discovery task failed: {error}"))??;
        if matches.paths.is_empty() {
            return Ok(ToolResult::success("No matching files found."));
        }

        let mut output = String::new();
        let mut skipped = Vec::new();
        for path in &matches.paths {
            let (bytes, file_truncated) = match read_bounded(path).await {
                Ok(result) => result,
                Err(error) => {
                    skipped.push(format!("{}: {error}", path.display()));
                    continue;
                }
            };
            let available = MAX_TOTAL_BYTES.saturating_sub(output.len());
            if available == 0 {
                break;
            }
            let limit = bytes.len().min(MAX_FILE_BYTES).min(available);
            let content = match text_prefix(&bytes[..limit]) {
                Some(content) => content,
                None => {
                    skipped.push(format!("{}: not a UTF-8 text file", path.display()));
                    continue;
                }
            };
            output.push_str(&format!("--- {} ---\n", path.display()));
            output.push_str(content);
            if !content.ends_with('\n') {
                output.push('\n');
            }
            if file_truncated || limit < bytes.len() {
                output.push_str("[file content truncated]\n");
            }
        }

        if matches.truncated || output.len() >= MAX_TOTAL_BYTES {
            output.push_str("\n[additional files omitted by output limits]\n");
        }
        if !skipped.is_empty() {
            output.push_str("\nSkipped files:\n");
            output.push_str(&skipped.join("\n"));
            output.push('\n');
        }
        Ok(ToolResult::success(output))
    }
}

async fn read_bounded(path: &Path) -> Result<(Vec<u8>, bool), std::io::Error> {
    let file = tokio::fs::File::open(path).await?;
    let mut reader = file.take((MAX_FILE_BYTES + 1) as u64);
    let mut bytes = Vec::with_capacity(MAX_FILE_BYTES + 1);
    reader.read_to_end(&mut bytes).await?;
    let truncated = bytes.len() > MAX_FILE_BYTES;
    bytes.truncate(MAX_FILE_BYTES);
    Ok((bytes, truncated))
}

fn text_prefix(bytes: &[u8]) -> Option<&str> {
    if bytes.contains(&0) {
        return None;
    }
    match std::str::from_utf8(bytes) {
        Ok(content) => Some(content),
        Err(error) if error.error_len().is_none() => {
            std::str::from_utf8(&bytes[..error.valid_up_to()]).ok()
        }
        Err(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reads_multiple_files_with_boundaries() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "alpha").unwrap();
        std::fs::write(dir.path().join("b.txt"), "beta").unwrap();
        let ctx = ToolContext {
            cwd: dir.path().to_path_buf(),
            session_id: "test".to_string(),
            project_root: dir.path().to_path_buf(),
        };

        let result = ReadManyFilesTool
            .invoke(serde_json::json!({"paths": ["*.txt"]}), &ctx)
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(result.output.contains("--- "));
        assert!(result.output.contains("a.txt ---\nalpha"));
        assert!(result.output.contains("b.txt ---\nbeta"));
    }

    #[tokio::test]
    async fn bounds_large_files_during_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large.txt");
        std::fs::write(&path, vec![b'x'; MAX_FILE_BYTES * 4]).unwrap();

        let (bytes, truncated) = read_bounded(&path).await.unwrap();

        assert_eq!(bytes.len(), MAX_FILE_BYTES);
        assert!(truncated);
    }

    #[tokio::test]
    async fn skips_binary_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("binary.dat"), [0, 159, 146, 150]).unwrap();
        let ctx = ToolContext {
            cwd: dir.path().to_path_buf(),
            session_id: "test".to_string(),
            project_root: dir.path().to_path_buf(),
        };

        let result = ReadManyFilesTool
            .invoke(serde_json::json!({"paths": ["binary.dat"]}), &ctx)
            .await
            .unwrap();

        assert!(result.output.contains("not a UTF-8 text file"));
        assert!(!result.output.contains('\u{fffd}'));
    }
}
