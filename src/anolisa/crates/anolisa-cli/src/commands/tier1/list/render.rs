use crate::color::{Palette, pad_right};
use crate::commands::tier1::list::Row;

pub(super) fn human_header(rows: &[Row]) -> String {
    let widths = HumanWidths::for_rows(rows);
    format!(
        "{:<name_width$}{:<backends_width$}{:<local_state_width$}{:<ownership_width$}{}",
        "NAME",
        "BACKENDS",
        "LOCAL STATE",
        "OWNERSHIP",
        "ACTION",
        name_width = widths.name,
        backends_width = widths.backends,
        local_state_width = widths.local_state,
        ownership_width = widths.ownership,
    )
}

struct HumanWidths {
    name: usize,
    backends: usize,
    local_state: usize,
    ownership: usize,
}

impl HumanWidths {
    fn for_rows(rows: &[Row]) -> Self {
        Self {
            name: rows.iter().map(|r| r.name.len()).max().unwrap_or(4).max(4) + 4,
            backends: rows
                .iter()
                .map(|r| {
                    if r.backends.is_empty() {
                        1
                    } else {
                        r.backends.join(",").len()
                    }
                })
                .max()
                .unwrap_or(8)
                .max(8)
                + 4,
            local_state: rows
                .iter()
                .map(|r| r.local_state.len())
                .max()
                .unwrap_or(11)
                .max(11)
                + 4,
            ownership: rows
                .iter()
                .map(|r| r.ownership.len())
                .max()
                .unwrap_or(9)
                .max(9)
                + 4,
        }
    }
}

pub(super) fn render_human(rows: &[Row], no_color: bool) {
    let color = Palette::new(no_color);
    if rows.is_empty() {
        println!("{}", color.muted("no components found"));
        return;
    }

    let widths = HumanWidths::for_rows(rows);

    println!("{}", color.header(human_header(rows)));
    for row in rows {
        let backend_str = if row.backends.is_empty() {
            "-".to_string()
        } else {
            row.backends.join(",")
        };
        println!(
            "{:<name_width$}{:<backends_width$}{}{:<ownership_width$}{}",
            row.name,
            backend_str,
            pad_right(color.status(&row.local_state), widths.local_state),
            row.ownership,
            row.action,
            name_width = widths.name,
            backends_width = widths.backends,
            ownership_width = widths.ownership,
        );
    }
}
