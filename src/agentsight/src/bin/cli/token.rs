//! Token query subcommand

use agentsight::{
    SqliteConfig, TimePeriod, TokenQueryResult, TokenStore, Trend, format_tokens_with_commas,
};
use structopt::StructOpt;

/// Token query subcommand
#[derive(Debug, StructOpt, Clone)]
pub struct TokenCommand {
    /// Query by fixed time period
    #[structopt(long, possible_values = &["today", "yesterday", "week", "last_week", "month", "last_month"])]
    pub period: Option<String>,

    /// Query last N hours
    #[structopt(long)]
    pub hours: Option<u64>,

    /// Compare with previous period
    #[structopt(long)]
    pub compare: bool,

    /// Output as JSON
    #[structopt(long)]
    pub json: bool,

    /// Group token usage by process type (agent/sub_agent/tool)
    #[structopt(long)]
    pub by_type: bool,

    /// Custom data file path
    #[structopt(long)]
    pub data_file: Option<String>,
}

impl TokenCommand {
    pub fn execute(&self) {
        if self.by_type {
            self.execute_by_type();
            return;
        }
        let data_path = self
            .data_file
            .as_ref()
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| SqliteConfig::default().db_path());

        self.execute_summary(&data_path);
    }

    fn execute_by_type(&self) {
        use agentsight::storage::sqlite::GenAISqliteStore;

        let genai_path = self
            .data_file
            .as_ref()
            .map(std::path::PathBuf::from)
            .unwrap_or_else(GenAISqliteStore::default_path);
        let store = match GenAISqliteStore::new_with_path(&genai_path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Failed to open GenAI store: {e}");
                return;
            }
        };

        let hours = self.hours.unwrap_or(24);
        let now_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as i64)
            .unwrap_or(0);
        let start_ns = now_ns - (hours as i64) * 3_600_000_000_000;

        match store.token_usage_by_process_type(start_ns, now_ns) {
            Ok(rows) if rows.is_empty() => {
                println!("No LLM calls with process_type in the last {hours} hours.");
            }
            Ok(rows) => {
                if self.json {
                    println!("{}", agentsight::lineage::format_token_by_type_json(&rows));
                } else {
                    print!(
                        "{}",
                        agentsight::lineage::format_token_by_type_table(&rows, hours)
                    );
                }
            }
            Err(e) => {
                eprintln!("Query failed: {e}");
            }
        }
    }

    fn execute_summary(&self, data_path: &std::path::Path) {
        // Open token store
        let store = TokenStore::new(data_path);
        let query = agentsight::TokenQuery::new(&store);

        // Execute query
        let result = if let Some(hours) = self.hours {
            if self.compare {
                query.by_hours_with_compare(hours)
            } else {
                query.by_hours(hours)
            }
        } else if let Some(ref period_str) = self.period {
            let period = super::parse_period(period_str);
            if self.compare {
                query.by_period_with_compare(period)
            } else {
                query.by_period(period)
            }
        } else if self.compare {
            query.by_period_with_compare(TimePeriod::Today)
        } else {
            query.by_period(TimePeriod::Today)
        };

        // Output result
        if self.json {
            println!("{}", serde_json::to_string_pretty(&result).unwrap());
        } else {
            print_human_readable(&result, self.compare);
        }
    }
}

/// Print human-readable summary output
fn print_human_readable(result: &TokenQueryResult, show_compare: bool) {
    // Main result
    println!(
        "{}共消耗 {} tokens。",
        result.period,
        format_tokens_with_commas(result.total_tokens)
    );

    // Comparison
    #[allow(clippy::collapsible_if)]
    if show_compare {
        if let Some(ref comp) = result.comparison {
            let trend = match comp.trend {
                Trend::Up => "增长",
                Trend::Down => "下降",
                Trend::Flat => "持平",
            };

            println!(
                "比上一时段（{}）{}了 {}。",
                format_tokens_with_commas(comp.previous_total),
                trend,
                comp.formatted_change()
            );
        }
    }

    // Additional details
    if result.request_count > 0 {
        println!();
        println!(
            "共 {} 次请求，输入 {} tokens，输出 {} tokens。",
            result.request_count,
            format_tokens_with_commas(result.input_tokens),
            format_tokens_with_commas(result.output_tokens)
        );
    }
}
