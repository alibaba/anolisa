use super::*;

pub(super) fn parse_ps_process_rows(output: &str) -> Vec<ProcessMemoryRow> {
    let lines: Vec<&str> = output
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    for (idx, line) in lines.iter().enumerate() {
        let tokens = split_tokens(line);
        let Some(header) = PsHeader::from_tokens(&tokens) else {
            continue;
        };
        return lines[idx + 1..]
            .iter()
            .filter_map(|row| parse_process_row(row, &header))
            .collect();
    }
    Vec::new()
}

pub(super) fn parse_top_process_rows(output: &str) -> Vec<ProcessMemoryRow> {
    let lines: Vec<&str> = output
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    for (idx, line) in lines.iter().enumerate() {
        let tokens = split_tokens(line);
        let Some(header) = TopHeader::from_tokens(&tokens) else {
            continue;
        };
        return lines[idx + 1..]
            .iter()
            .filter_map(|row| parse_top_process_row(row, &header))
            .collect();
    }
    Vec::new()
}

#[derive(Debug, Clone)]
struct PsHeader {
    pid_idx: usize,
    mem_idx: usize,
    rss_idx: Option<usize>,
    command_idx: Option<usize>,
    command_is_tail: bool,
}

impl PsHeader {
    pub(super) fn from_tokens(tokens: &[&str]) -> Option<Self> {
        let schema = detect_table_schema(
            tokens,
            &[
                ColumnSpec::first(ColumnSemantic::Pid, &["PID"]),
                ColumnSpec::first(ColumnSemantic::MemPct, &["%MEM", "PMEM", "MEM%"]),
                ColumnSpec::first(ColumnSemantic::Rss, &["RSS", "RSZ", "RSSIZE"]),
                ColumnSpec::last(ColumnSemantic::Command, &["COMMAND", "ARGS", "CMD", "COMM"]),
            ],
        );
        let pid_idx = schema.index(ColumnSemantic::Pid)?;
        let mem_idx = schema.index(ColumnSemantic::MemPct)?;
        let rss_idx = schema.index(ColumnSemantic::Rss);
        let command_idx = schema.index(ColumnSemantic::Command);
        if rss_idx.is_none() && command_idx.is_none() {
            return None;
        }
        let command_is_tail = command_idx
            .map(|idx| idx + 1 == tokens.len())
            .unwrap_or(false);
        Some(Self {
            pid_idx,
            mem_idx,
            rss_idx,
            command_idx,
            command_is_tail,
        })
    }
}

#[derive(Debug, Clone)]
struct TopHeader {
    pid_idx: usize,
    mem_idx: usize,
    res_idx: Option<usize>,
    command_idx: usize,
}

impl TopHeader {
    pub(super) fn from_tokens(tokens: &[&str]) -> Option<Self> {
        let schema = detect_table_schema(
            tokens,
            &[
                ColumnSpec::first(ColumnSemantic::Pid, &["PID"]),
                ColumnSpec::first(ColumnSemantic::MemPct, &["%MEM"]),
                ColumnSpec::first(ColumnSemantic::Res, &["RES"]),
                ColumnSpec::last(ColumnSemantic::Command, &["COMMAND", "ARGS", "CMD", "COMM"]),
            ],
        );
        Some(Self {
            pid_idx: schema.index(ColumnSemantic::Pid)?,
            mem_idx: schema.index(ColumnSemantic::MemPct)?,
            res_idx: schema.index(ColumnSemantic::Res),
            command_idx: schema.index(ColumnSemantic::Command)?,
        })
    }
}

fn parse_process_row(line: &str, header: &PsHeader) -> Option<ProcessMemoryRow> {
    let tokens = split_tokens(line);
    let pid = tokens.get(header.pid_idx)?.to_string();
    let mem_pct = parse_float(tokens.get(header.mem_idx)?)?;
    let rss_kib = header
        .rss_idx
        .and_then(|idx| tokens.get(idx))
        .and_then(|value| parse_u64(value));
    let command = header
        .command_idx
        .and_then(|idx| command_from_tokens(&tokens, idx, header.command_is_tail))
        .unwrap_or_else(|| pid.clone());
    Some(ProcessMemoryRow {
        pid,
        command,
        mem_pct,
        rss_kib,
    })
}

fn parse_top_process_row(line: &str, header: &TopHeader) -> Option<ProcessMemoryRow> {
    let tokens = split_tokens(line);
    let pid = tokens.get(header.pid_idx)?.to_string();
    let mem_pct = parse_float(tokens.get(header.mem_idx)?)?;
    let rss_kib = header
        .res_idx
        .and_then(|idx| tokens.get(idx))
        .and_then(|value| parse_memory_to_mib(value))
        .map(|mib| (mib * 1024.0).round() as u64);
    let command =
        command_from_tokens(&tokens, header.command_idx, true).unwrap_or_else(|| pid.clone());
    Some(ProcessMemoryRow {
        pid,
        command,
        mem_pct,
        rss_kib,
    })
}

pub(super) fn parse_free_memory_metrics(command: &str, output: &str) -> Option<MemoryMetrics> {
    let mut header_tokens: Option<Vec<&str>> = None;
    let mut mem_tokens: Option<Vec<&str>> = None;
    let mut swap_tokens: Option<Vec<&str>> = None;
    let mut header_row_count = 0;
    let mut mem_row_count = 0;
    for line in output.lines() {
        let tokens = split_tokens(line);
        if tokens.is_empty() {
            continue;
        }
        let first_token = tokens[0].trim_end_matches(':');
        if first_token.eq_ignore_ascii_case("mem") {
            mem_row_count += 1;
            mem_tokens = Some(tokens);
        } else if first_token.eq_ignore_ascii_case("swap") {
            swap_tokens = Some(tokens);
        } else if tokens
            .iter()
            .any(|token| token.eq_ignore_ascii_case("total"))
            && tokens
                .iter()
                .any(|token| token.eq_ignore_ascii_case("available"))
        {
            header_row_count += 1;
            header_tokens = Some(tokens);
        }
    }
    if header_row_count != 1 || mem_row_count != 1 {
        return None;
    }
    let header = header_tokens?;
    let mem = mem_tokens?;
    let schema = detect_table_schema(
        &header,
        &[
            ColumnSpec::first(ColumnSemantic::Total, &["total"]),
            ColumnSpec::first(ColumnSemantic::Available, &["available"]),
        ],
    );
    let total_idx = schema.index(ColumnSemantic::Total)? + 1;
    let available_idx = schema.index(ColumnSemantic::Available)? + 1;
    let unit_factors = free_unit_factors(command);
    let total_raw = mem.get(total_idx)?;
    let available_raw = mem.get(available_idx)?;
    let total_mib = parse_free_memory_value_to_mib(total_raw, unit_factors)?;
    let available_mib = parse_free_memory_value_to_mib(available_raw, unit_factors)?;
    let (swap_total_mib, swap_used_mib) = swap_tokens
        .as_ref()
        .and_then(|swap| {
            Some((
                parse_free_memory_value_to_mib(swap.get(1)?, unit_factors)?,
                parse_free_memory_value_to_mib(swap.get(2)?, unit_factors)?,
            ))
        })
        .map(|(total, used)| (Some(total), Some(used)))
        .unwrap_or((None, None));
    Some(MemoryMetrics {
        total_mib,
        available_mib,
        swap_total_mib,
        swap_used_mib,
        confidence: if free_output_uses_coarse_no_suffix_units(
            unit_factors,
            &[total_raw, available_raw],
        ) {
            MetricsConfidence::Low
        } else {
            MetricsConfidence::High
        },
    })
}

pub(super) fn parse_top_memory_metrics(output: &str) -> Option<MemoryMetrics> {
    let mut total_mib = None;
    let mut free_mib = None;
    let mut swap_total_mib = None;
    let mut swap_used_mib = None;
    let mut available_mib = None;

    for line in output.lines() {
        let trimmed = line.trim();
        if is_top_mem_line(trimmed) {
            let factor = top_unit_factor(trimmed)?;
            total_mib = top_metric_value(trimmed, "total").map(|v| v * factor);
            free_mib = top_metric_value(trimmed, "free").map(|v| v * factor);
        } else if is_top_swap_line(trimmed) {
            let factor = top_unit_factor(trimmed)?;
            swap_total_mib = top_metric_value(trimmed, "total").map(|v| v * factor);
            swap_used_mib = top_metric_value(trimmed, "used").map(|v| v * factor);
            available_mib = top_avail_mem_value(trimmed).map(|v| v * factor);
        }
    }

    let total_mib = total_mib?;
    if let Some(available_mib) = available_mib {
        return Some(MemoryMetrics {
            total_mib,
            available_mib,
            swap_total_mib,
            swap_used_mib,
            confidence: MetricsConfidence::High,
        });
    }
    Some(MemoryMetrics {
        total_mib,
        available_mib: free_mib?,
        swap_total_mib,
        swap_used_mib,
        confidence: MetricsConfidence::Low,
    })
}

pub(super) fn split_tokens(line: &str) -> Vec<&str> {
    line.split_whitespace().collect()
}

pub(super) fn memory_target_program(command: &str) -> &str {
    let mut seen_env = false;
    let mut skip_next_env_arg = false;
    let mut after_sudo = false;
    let mut skip_next_sudo_arg = false;
    for token in command.split_whitespace() {
        if skip_next_env_arg {
            skip_next_env_arg = false;
            continue;
        }
        if skip_next_sudo_arg {
            skip_next_sudo_arg = false;
            continue;
        }
        if is_env_assignment_token(token) {
            continue;
        }
        let basename = token
            .rsplit_once('/')
            .map(|(_, name)| name)
            .unwrap_or(token);
        if basename == "sudo" {
            after_sudo = true;
            continue;
        }
        if after_sudo && is_sudo_option_token(token, &mut skip_next_sudo_arg) {
            continue;
        }
        if !seen_env && basename == "env" {
            seen_env = true;
            continue;
        }
        if seen_env {
            match token {
                "-i" | "--ignore-environment" | "-0" | "--null" | "--" => continue,
                "-u" | "--unset" | "-C" | "--chdir" => {
                    skip_next_env_arg = true;
                    continue;
                }
                _ => {}
            }
            if token.starts_with("--unset=")
                || token.starts_with("--chdir=")
                || token.starts_with("--argv0=")
            {
                continue;
            }
            if token.starts_with('-') {
                return "";
            }
        }
        return basename;
    }
    ""
}

pub(super) fn is_batch_top_command(command: &str) -> bool {
    if memory_target_program(command) != "top" {
        return false;
    }

    let mut seen_program = false;
    let mut after_sudo = false;
    let mut skip_next_sudo_arg = false;
    let mut skip_next_iteration_arg = false;
    let mut seen_batch = false;
    let mut seen_one_shot_iteration = false;
    for token in command.split_whitespace() {
        if !seen_program {
            if skip_next_sudo_arg {
                skip_next_sudo_arg = false;
                continue;
            }
            if is_env_assignment_token(token) {
                continue;
            }
            let basename = token
                .rsplit_once('/')
                .map(|(_, name)| name)
                .unwrap_or(token);
            if basename == "sudo" {
                after_sudo = true;
                continue;
            }
            if after_sudo && is_sudo_option_token(token, &mut skip_next_sudo_arg) {
                continue;
            }
            if basename == "top" {
                seen_program = true;
            }
            continue;
        }

        if matches!(token, "|" | ";" | "&&" | "||") {
            break;
        }
        if skip_next_iteration_arg {
            seen_one_shot_iteration |= is_one_shot_top_iteration(token);
            skip_next_iteration_arg = false;
            continue;
        }
        if token == "--batch" {
            seen_batch = true;
            continue;
        }
        if matches!(token, "-n" | "--iterations") {
            skip_next_iteration_arg = true;
            continue;
        }
        if let Some(iterations) = token.strip_prefix("--iterations=") {
            seen_one_shot_iteration |= is_one_shot_top_iteration(iterations);
            continue;
        }
        if let Some(short_options) = token.strip_prefix('-') {
            if short_options.starts_with('-') {
                continue;
            }
            if short_options.contains('b') {
                seen_batch = true;
            }
            if short_options == "n" {
                skip_next_iteration_arg = true;
                continue;
            }
            if let Some(n_pos) = short_options.find('n') {
                let iterations = &short_options[n_pos + 1..];
                if iterations.is_empty() {
                    skip_next_iteration_arg = true;
                } else {
                    seen_one_shot_iteration |= is_one_shot_top_iteration(iterations);
                }
            }
        }
    }

    seen_batch && seen_one_shot_iteration
}

pub(super) fn is_one_shot_top_iteration(value: &str) -> bool {
    value.trim_start_matches('=').trim() == "1"
}

pub(super) fn is_free_sampling_command(command: &str) -> bool {
    if memory_target_program(command) != "free" {
        return false;
    }

    let mut seen_program = false;
    let mut after_sudo = false;
    let mut skip_next_sudo_arg = false;
    for token in command.split_whitespace() {
        if !seen_program {
            if skip_next_sudo_arg {
                skip_next_sudo_arg = false;
                continue;
            }
            if is_env_assignment_token(token) {
                continue;
            }
            let basename = token
                .rsplit_once('/')
                .map(|(_, name)| name)
                .unwrap_or(token);
            if basename == "sudo" {
                after_sudo = true;
                continue;
            }
            if after_sudo && is_sudo_option_token(token, &mut skip_next_sudo_arg) {
                continue;
            }
            if basename == "free" {
                seen_program = true;
            }
            continue;
        }

        if matches!(token, "|" | ";" | "&&" | "||") {
            break;
        }
        if matches!(token, "-s" | "--seconds" | "-c" | "--count")
            || token.starts_with("--seconds=")
            || token.starts_with("--count=")
        {
            return true;
        }
        if let Some(short_options) = token.strip_prefix('-') {
            if !short_options.starts_with('-')
                && (short_options.contains('s') || short_options.contains('c'))
            {
                return true;
            }
        }
    }
    false
}

pub(super) fn is_top_metadata_command(command: &str) -> bool {
    if memory_target_program(command) != "top" {
        return false;
    }

    let mut seen_program = false;
    let mut after_sudo = false;
    let mut skip_next_sudo_arg = false;
    for token in command.split_whitespace() {
        if !seen_program {
            if skip_next_sudo_arg {
                skip_next_sudo_arg = false;
                continue;
            }
            if is_env_assignment_token(token) {
                continue;
            }
            let basename = token
                .rsplit_once('/')
                .map(|(_, name)| name)
                .unwrap_or(token);
            if basename == "sudo" {
                after_sudo = true;
                continue;
            }
            if after_sudo && is_sudo_option_token(token, &mut skip_next_sudo_arg) {
                continue;
            }
            if basename == "top" {
                seen_program = true;
            }
            continue;
        }

        if matches!(token, "|" | ";" | "&&" | "||") {
            break;
        }
        if matches!(token, "-h" | "--help" | "-v" | "-V" | "-O" | "--version") {
            return true;
        }
    }

    false
}

pub(super) fn is_env_assignment_token(token: &str) -> bool {
    let Some(eq_pos) = token.find('=') else {
        return false;
    };
    if eq_pos == 0 {
        return false;
    }
    let name = &token[..eq_pos];
    name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        && !name.bytes().next().unwrap_or(0).is_ascii_digit()
}

pub(super) fn is_sudo_option_token(token: &str, skip_next_arg: &mut bool) -> bool {
    match token {
        "--" => return true,
        "-u" | "-g" | "-h" | "-p" | "-C" | "-T" | "--user" | "--group" | "--host" | "--prompt"
        | "--close-from" | "--command-timeout" => {
            *skip_next_arg = true;
            return true;
        }
        "--askpass" | "--background" | "--edit" | "--help" | "--login" | "--non-interactive"
        | "--preserve-env" | "--reset-timestamp" | "--remove-timestamp" | "--shell" | "--stdin"
        | "--validate" | "--version" | "-A" | "-b" | "-E" | "-e" | "-H" | "-K" | "-k" | "-l"
        | "-n" | "-P" | "-S" | "-s" | "-V" | "-v" => return true,
        _ => {}
    }
    if token.starts_with("--user=")
        || token.starts_with("--group=")
        || token.starts_with("--host=")
        || token.starts_with("--prompt=")
        || token.starts_with("--close-from=")
        || token.starts_with("--command-timeout=")
        || token.starts_with("--preserve-env=")
    {
        return true;
    }
    if token.len() > 2 && matches!(&token[..2], "-u" | "-g" | "-h" | "-p" | "-C" | "-T") {
        return true;
    }
    token
        .strip_prefix('-')
        .filter(|opts| !opts.starts_with('-') && !opts.is_empty())
        .is_some_and(|opts| opts.chars().all(|ch| "AbEeHKklnPSsVv".contains(ch)))
}

impl<'a> ColumnSpec<'a> {
    pub(super) fn first(semantic: ColumnSemantic, aliases: &'a [&'a str]) -> Self {
        Self {
            semantic,
            aliases,
            prefer_last: false,
        }
    }

    pub(super) fn last(semantic: ColumnSemantic, aliases: &'a [&'a str]) -> Self {
        Self {
            semantic,
            aliases,
            prefer_last: true,
        }
    }
}

impl TableSchema {
    pub(super) fn index(&self, semantic: ColumnSemantic) -> Option<usize> {
        self.columns
            .iter()
            .find(|column| column.semantic == semantic)
            .map(|column| column.index)
    }
}

pub(super) fn detect_table_schema(tokens: &[&str], specs: &[ColumnSpec<'_>]) -> TableSchema {
    let columns = specs
        .iter()
        .filter_map(|spec| {
            let matcher = |token: &&str| {
                spec.aliases
                    .iter()
                    .any(|alias| token.eq_ignore_ascii_case(alias))
            };
            let index = if spec.prefer_last {
                tokens.iter().rposition(matcher)
            } else {
                tokens.iter().position(matcher)
            }?;
            Some(ColumnBinding {
                semantic: spec.semantic,
                index,
                raw_name: tokens[index].to_string(),
            })
        })
        .collect();
    TableSchema { columns }
}

pub(super) fn free_unit_factors(command: &str) -> MemoryUnitFactors {
    let mut factors = MemoryUnitFactors {
        no_suffix: 1.0 / 1024.0,
        bare_suffix_mib: 1.0,
        bare_suffix_step: 1024.0,
    };
    let mut seen_program = false;
    let mut after_sudo = false;
    let mut skip_next_sudo_arg = false;
    for token in command.split_whitespace() {
        if !seen_program {
            if skip_next_sudo_arg {
                skip_next_sudo_arg = false;
                continue;
            }
            if is_env_assignment_token(token) {
                continue;
            }
            let basename = token
                .rsplit_once('/')
                .map(|(_, name)| name)
                .unwrap_or(token);
            if basename == "sudo" {
                after_sudo = true;
                continue;
            }
            if after_sudo && is_sudo_option_token(token, &mut skip_next_sudo_arg) {
                continue;
            }
            if basename == "free" {
                seen_program = true;
            }
            continue;
        }

        if matches!(token, "|" | ";" | "&&" | "||") {
            break;
        }
        match token {
            "--bytes" => factors.no_suffix = 1.0 / (1024.0 * 1024.0),
            "--kibi" => factors.no_suffix = 1.0 / 1024.0,
            "--mebi" => factors.no_suffix = 1.0,
            "--gibi" => factors.no_suffix = 1024.0,
            "--tebi" => factors.no_suffix = 1024.0 * 1024.0,
            "--pebi" => factors.no_suffix = 1024.0 * 1024.0 * 1024.0,
            "--kilo" => factors.no_suffix = 1000.0 / (1024.0 * 1024.0),
            "--mega" => factors.no_suffix = 1_000_000.0 / (1024.0 * 1024.0),
            "--giga" => factors.no_suffix = 1_000_000_000.0 / (1024.0 * 1024.0),
            "--tera" => factors.no_suffix = 1_000_000_000_000.0 / (1024.0 * 1024.0),
            "--peta" => factors.no_suffix = 1_000_000_000_000_000.0 / (1024.0 * 1024.0),
            "--si" => {
                factors.bare_suffix_mib = 1_000_000.0 / (1024.0 * 1024.0);
                factors.bare_suffix_step = 1000.0;
            }
            _ => {}
        }
        if let Some(short_options) = token.strip_prefix('-') {
            if short_options.starts_with('-') {
                continue;
            }
            if short_options.contains('b') {
                factors.no_suffix = 1.0 / (1024.0 * 1024.0);
            }
            if short_options.contains('k') {
                factors.no_suffix = 1.0 / 1024.0;
            }
            if short_options.contains('m') {
                factors.no_suffix = 1.0;
            }
            if short_options.contains('g') {
                factors.no_suffix = 1024.0;
            }
            if short_options.contains('t') {
                factors.no_suffix = 1024.0 * 1024.0;
            }
        }
    }

    factors
}

pub(super) fn command_from_tokens(
    tokens: &[&str],
    idx: usize,
    consume_tail: bool,
) -> Option<String> {
    if idx >= tokens.len() {
        return None;
    }
    if consume_tail {
        Some(tokens[idx..].join(" "))
    } else {
        Some(tokens[idx].to_string())
    }
}

pub(super) fn parse_float(value: &str) -> Option<f64> {
    value.trim_end_matches('%').parse::<f64>().ok()
}

pub(super) fn parse_u64(value: &str) -> Option<u64> {
    value.parse::<u64>().ok()
}

pub(super) fn parse_memory_to_mib(value: &str) -> Option<f64> {
    parse_memory_value_to_mib(
        value,
        MemoryUnitFactors {
            no_suffix: 1.0 / 1024.0,
            bare_suffix_mib: 1.0,
            bare_suffix_step: 1024.0,
        },
    )
}

pub(super) fn parse_free_memory_value_to_mib(
    value: &str,
    unit_factors: MemoryUnitFactors,
) -> Option<f64> {
    parse_memory_value_to_mib(value, unit_factors)
}

pub(super) fn free_output_uses_coarse_no_suffix_units(
    unit_factors: MemoryUnitFactors,
    values: &[&str],
) -> bool {
    unit_factors.no_suffix >= 1024.0 && values.iter().all(|value| is_bare_integer_value(value))
}

pub(super) fn is_bare_integer_value(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
}

pub(super) fn parse_memory_value_to_mib(
    value: &str,
    unit_factors: MemoryUnitFactors,
) -> Option<f64> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let suffix_start = value
        .char_indices()
        .find_map(|(idx, ch)| {
            if ch.is_ascii_digit() || ch == '.' {
                None
            } else {
                Some(idx)
            }
        })
        .unwrap_or(value.len());
    let number = &value[..suffix_start];
    let suffix = value[suffix_start..].trim().to_ascii_lowercase();
    let factor = match suffix.as_str() {
        "" => unit_factors.no_suffix,
        "b" => 1.0 / (1024.0 * 1024.0),
        "k" | "kb" => unit_factors.bare_suffix_mib / unit_factors.bare_suffix_step,
        "ki" | "kib" => 1.0 / 1024.0,
        "m" | "mb" => unit_factors.bare_suffix_mib,
        "mi" | "mib" => 1.0,
        "g" | "gb" => unit_factors.bare_suffix_mib * unit_factors.bare_suffix_step,
        "gi" | "gib" => 1024.0,
        "t" | "tb" => {
            unit_factors.bare_suffix_mib
                * unit_factors.bare_suffix_step
                * unit_factors.bare_suffix_step
        }
        "ti" | "tib" => 1024.0 * 1024.0,
        "p" | "pb" => {
            unit_factors.bare_suffix_mib
                * unit_factors.bare_suffix_step
                * unit_factors.bare_suffix_step
                * unit_factors.bare_suffix_step
        }
        "pi" | "pib" => 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    parse_float(number).map(|v| v * factor)
}

pub(super) fn is_top_mem_line(line: &str) -> bool {
    let line = line.to_ascii_lowercase();
    !line.contains("swap")
        && line.contains(" mem")
        && line.contains(" total")
        && line.contains(" free")
}

pub(super) fn is_top_swap_line(line: &str) -> bool {
    let line = line.to_ascii_lowercase();
    line.contains(" swap") && line.contains(" total")
}

pub(super) fn top_unit_factor(line: &str) -> Option<f64> {
    let line = line.to_ascii_lowercase();
    if line.starts_with("tib") {
        Some(1024.0 * 1024.0)
    } else if line.starts_with("gib") {
        Some(1024.0)
    } else if line.starts_with("mib") {
        Some(1.0)
    } else if line.starts_with("kib") {
        Some(1.0 / 1024.0)
    } else {
        None
    }
}

pub(super) fn top_metric_value(line: &str, label: &str) -> Option<f64> {
    let normalized = line.replace([',', ':'], " ");
    let tokens = split_tokens(&normalized);
    tokens
        .windows(2)
        .find(|window| window[1].trim_end_matches('.').eq_ignore_ascii_case(label))
        .and_then(|window| parse_float(window[0]))
}

pub(super) fn top_avail_mem_value(line: &str) -> Option<f64> {
    let normalized = line.replace([',', ':'], " ");
    let tokens = split_tokens(&normalized);
    tokens.windows(3).find_map(|window| {
        if window[1].eq_ignore_ascii_case("avail") && window[2].eq_ignore_ascii_case("Mem") {
            parse_float(window[0])
        } else {
            None
        }
    })
}

pub(super) fn display_command(command: &str) -> String {
    let mut tokens = command.split_whitespace().filter(|token| *token != "\\_");
    tokens.next().unwrap_or(command).trim().to_string()
}

pub(super) fn kib_to_mib(kib: u64) -> u64 {
    (kib + 512) / 1024
}

pub(super) fn round_mib(mib: f64) -> u64 {
    mib.round() as u64
}
