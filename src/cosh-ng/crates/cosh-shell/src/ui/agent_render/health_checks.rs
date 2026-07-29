//! Checks coverage rendering shared by the boxed and plain health panel paths.
//!
//! Extracted from `health.rs` to keep the large health-banner renderer within
//! its budgeted line count while the per-severity split plan is addressed.

use ratatui::{style::Color, style::Style, text::Span};

use super::health::HealthBannerLine;
use super::wrap::wrap_plain_line;
use crate::diagnostics::health::HealthScanReport;

/// Sorted, deduplicated checks coverage text, matching the `checks: <names>`
/// line printed by `cosh-shell doctor` (`format_doctor_report_plain`).
pub(super) fn checks_summary_text(report: &HealthScanReport, i18n: crate::I18n) -> Option<String> {
    if report.checks_done.is_empty() {
        return None;
    }
    let mut checks = report.checks_done.clone();
    checks.sort();
    checks.dedup();
    Some(format!(
        "{}: {}",
        i18n.t(crate::MessageId::DoctorChecksLabel),
        checks.join(", ")
    ))
}

/// Checks coverage lines for the boxed panel renderer. Caps at two wrapped
/// lines to keep the panel compact when the checks list is long.
pub(super) fn checks_lines(
    report: &HealthScanReport,
    i18n: crate::I18n,
    content_width: usize,
) -> Vec<HealthBannerLine> {
    let Some(text) = checks_summary_text(report, i18n) else {
        return Vec::new();
    };
    wrap_plain_line(&text, content_width)
        .into_iter()
        .take(2)
        .map(|line| {
            HealthBannerLine::styled(vec![Span::styled(line, Style::default().fg(Color::Gray))])
        })
        .collect()
}
