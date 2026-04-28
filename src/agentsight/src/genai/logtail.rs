//! iLogtail 文件导出器
//!
//! 将 GenAI 语义事件以扁平化 JSON 格式写入指定路径，
//! 由 iLogtail 自动采集上传到 SLS，无需配置 AK/SK。
//!
//! 通过环境变量 `SLS_LOGTAIL_FILE` 指定输出路径。
//! 仅当该环境变量设置时才启用。

use std::path::PathBuf;
use std::fs::OpenOptions;
use std::io::{Write, BufWriter};

use super::semantic::GenAISemanticEvent;
use super::exporter::GenAIExporter;
use super::sls::events_to_flat_records;

/// 环境变量名称
pub const LOGTAIL_ENV_VAR: &str = "SLS_LOGTAIL_FILE";

/// 检查 Logtail 导出是否启用（环境变量 SLS_LOGTAIL_FILE 是否设置）
pub fn logtail_enabled() -> bool {
    std::env::var(LOGTAIL_ENV_VAR).is_ok()
}

/// 获取 Logtail 输出路径（从环境变量读取）
pub fn logtail_path() -> Option<String> {
    std::env::var(LOGTAIL_ENV_VAR).ok()
}

/// iLogtail 文件导出器
///
/// 将 GenAI 事件以扁平化 JSON 格式（每行一条记录）写入指定路径，
/// 由 iLogtail 自动采集上传到 SLS。字段命名与 SLS PutLogs 完全一致。
pub struct LogtailExporter {
    path: PathBuf,
}

impl LogtailExporter {
    /// 创建新的 Logtail 导出器
    ///
    /// 从环境变量 `SLS_LOGTAIL_FILE` 读取路径，自动创建父目录。
    /// 如果环境变量未设置，返回 `None`。
    pub fn new() -> Option<Self> {
        let path_str = logtail_path()?;
        let path = PathBuf::from(path_str);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        Some(LogtailExporter { path })
    }

    /// 返回导出文件路径
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// 将扁平化记录批量写入文件（append 模式）
    fn write_batch(&self, events: &[GenAISemanticEvent]) {
        let records = events_to_flat_records(events);
        if records.is_empty() {
            return;
        }

        let file = match OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            Ok(f) => f,
            Err(e) => {
                log::warn!("Failed to open logtail file {:?}: {}", self.path, e);
                return;
            }
        };

        let mut writer = BufWriter::new(file);
        for record in &records {
            match serde_json::to_string(record) {
                Ok(json_line) => {
                    if let Err(e) = writeln!(writer, "{}", json_line) {
                        log::warn!("Failed to write logtail record: {}", e);
                        return;
                    }
                }
                Err(e) => {
                    log::warn!("Failed to serialize logtail record: {}", e);
                }
            }
        }

        if let Err(e) = writer.flush() {
            log::warn!("Failed to flush logtail file: {}", e);
        }
    }
}

impl GenAIExporter for LogtailExporter {
    fn name(&self) -> &str {
        "logtail-file"
    }

    fn export(&self, events: &[GenAISemanticEvent]) {
        self.write_batch(events);
    }
}
