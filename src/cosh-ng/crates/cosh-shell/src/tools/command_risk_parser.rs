use super::command_risk::CommandShape;

/// Non-persistent output-suppression sink allowlist (issue #1667
/// implementation boundaries): a `[N]>` / `[N]>>` redirection is treated
/// as output suppression instead of a filesystem write only when the
/// target is an unquoted, non-expanded literal from this table. Every
/// other form (regular files, quoted or expanded targets, `2>&1`, `&>`)
/// keeps the fail-closed RedirectionWrite high-risk path. Extending this
/// table requires revisiting the issue #1667 boundaries and the
/// decision-matrix tests in `command_risk_tests.rs`.
const SAFE_OUTPUT_SINKS: &[&str] = &["/dev/null"];

#[derive(Debug, Clone)]
pub(super) struct ParsedCommand {
    pub(super) shape: CommandShape,
    pub(super) stages: Vec<Vec<String>>,
    pub(super) null_redirections: usize,
    /// Command segments split at `&&`, `||`, `;`, and newlines; each
    /// segment holds its own pipeline stages. Only populated when the
    /// command contains segment separators (used by the stripped-compound
    /// aggregation path).
    pub(super) segments: Vec<Vec<Vec<String>>>,
}

pub(super) fn parse_command(command: &str) -> ParsedCommand {
    if command.is_empty() {
        return ParsedCommand {
            shape: CommandShape::Empty,
            stages: Vec::new(),
            null_redirections: 0,
            segments: Vec::new(),
        };
    }
    if command.contains('\0') {
        return ParsedCommand {
            shape: CommandShape::Unparseable,
            stages: Vec::new(),
            null_redirections: 0,
            segments: Vec::new(),
        };
    }

    let mut tokens = Vec::new();
    let mut token = String::new();
    let mut stages: Vec<Vec<String>> = Vec::new();
    let mut shape = CommandShape::Simple;
    let mut quote: Option<char> = None;
    let mut null_redirections = 0usize;
    let mut amp_redirect_guard = false;
    // Segment breaks recorded as (stage index, token offset) at each
    // `&&`/`||`/`;`/newline, resolved into `segments` after parsing.
    let mut segment_marks: Vec<(usize, usize)> = Vec::new();
    // Tracks whether the current token buffer contains quoted or escaped
    // content; such tokens are ordinary arguments and never fd prefixes.
    let mut token_quoted = false;
    let mut chars = command.chars().peekable();

    while let Some(ch) = chars.next() {
        if let Some(quote_ch) = quote {
            if ch == quote_ch {
                quote = None;
            } else {
                token.push(ch);
            }
            continue;
        }
        match ch {
            '\'' | '"' => {
                quote = Some(ch);
                token_quoted = true;
            }
            ' ' | '\t' => push_token(&mut tokens, &mut token, &mut token_quoted),
            '\n' | ';' => {
                push_token(&mut tokens, &mut token, &mut token_quoted);
                segment_marks.push((stages.len(), tokens.len()));
                shape = max_shape(shape, CommandShape::Sequence);
            }
            '|' => {
                push_token(&mut tokens, &mut token, &mut token_quoted);
                if chars.peek().is_some_and(|next| *next == '|') {
                    chars.next();
                    segment_marks.push((stages.len(), tokens.len()));
                    shape = max_shape(shape, CommandShape::AndOrList);
                } else {
                    stages.push(std::mem::take(&mut tokens));
                    shape = max_shape(shape, CommandShape::Pipeline);
                }
            }
            '&' => {
                push_token(&mut tokens, &mut token, &mut token_quoted);
                if chars.peek().is_some_and(|next| *next == '&') {
                    chars.next();
                    segment_marks.push((stages.len(), tokens.len()));
                    shape = max_shape(shape, CommandShape::AndOrList);
                } else {
                    shape = max_shape(shape, CommandShape::Complex);
                    // `&>` redirects both stdout and stderr; it is not a
                    // `[N]>` form, keep the existing high-risk path.
                    amp_redirect_guard = chars.peek().is_some_and(|next| *next == '>');
                }
            }
            '>' => {
                let guarded = amp_redirect_guard;
                amp_redirect_guard = false;
                // Shell only treats an unquoted, unescaped whole-numeric
                // token adjacent to `>` as an IO_NUMBER fd prefix (the `2`
                // in `2>`). Any other pending token is an ordinary word
                // that belongs to the command, and the redirection itself
                // uses the default stdout fd (`ls>/dev/null`,
                // `echo "2">/dev/null`).
                let fd_candidate = !token.is_empty()
                    && !token_quoted
                    && token.bytes().all(|byte| byte.is_ascii_digit());
                if !fd_candidate {
                    push_token(&mut tokens, &mut token, &mut token_quoted);
                }
                // Non-consuming lookahead: on rejection fall back to a path
                // that is byte-for-byte identical to the pre-fix behavior.
                let mut lookahead = chars.clone();
                let mut consumed = 0usize;
                if lookahead.peek().is_some_and(|next| *next == '>') {
                    lookahead.next();
                    consumed += 1;
                }
                while lookahead
                    .peek()
                    .is_some_and(|next| *next == ' ' || *next == '\t')
                {
                    lookahead.next();
                    consumed += 1;
                }
                let mut target = String::new();
                let mut literal = true;
                while let Some(&next) = lookahead.peek() {
                    if matches!(
                        next,
                        ' ' | '\t' | '\n' | ';' | '|' | '&' | '<' | '>' | '(' | ')' | '{' | '}'
                    ) {
                        break;
                    }
                    if matches!(next, '\'' | '"' | '`' | '$' | '\\') {
                        literal = false;
                        break;
                    }
                    target.push(next);
                    lookahead.next();
                    consumed += 1;
                }
                if !guarded && literal && SAFE_OUTPUT_SINKS.contains(&target.as_str()) {
                    if fd_candidate {
                        token.clear();
                        token_quoted = false;
                    }
                    for _ in 0..consumed {
                        chars.next();
                    }
                    null_redirections += 1;
                } else {
                    if fd_candidate {
                        push_token(&mut tokens, &mut token, &mut token_quoted);
                    }
                    if chars.peek().is_some_and(|next| *next == '>') {
                        chars.next();
                    }
                    shape = max_shape(shape, CommandShape::RedirectionWrite);
                }
            }
            '<' => {
                push_token(&mut tokens, &mut token, &mut token_quoted);
                shape = max_shape(shape, CommandShape::RedirectionRead);
            }
            '`' => {
                push_token(&mut tokens, &mut token, &mut token_quoted);
                shape = max_shape(shape, CommandShape::CommandSubstitution);
            }
            '$' if chars.peek().is_some_and(|next| *next == '(') => {
                push_token(&mut tokens, &mut token, &mut token_quoted);
                chars.next();
                shape = max_shape(shape, CommandShape::CommandSubstitution);
            }
            '(' | ')' | '{' | '}' => {
                push_token(&mut tokens, &mut token, &mut token_quoted);
                shape = max_shape(shape, CommandShape::Complex);
            }
            '\\' => {
                if let Some(next) = chars.next() {
                    token.push(next);
                    token_quoted = true;
                }
            }
            _ => token.push(ch),
        }
    }

    if quote.is_some() {
        return ParsedCommand {
            shape: CommandShape::Unparseable,
            stages: Vec::new(),
            null_redirections: 0,
            segments: Vec::new(),
        };
    }
    push_token(&mut tokens, &mut token, &mut token_quoted);
    if !tokens.is_empty() {
        stages.push(tokens);
    }
    if matches!(shape, CommandShape::Simple)
        && stages.first().is_some_and(|tokens| {
            tokens
                .iter()
                .take_while(|token| is_env_assignment(token))
                .count()
                > 0
        })
    {
        shape = CommandShape::EnvSimple;
    }

    ParsedCommand {
        shape,
        segments: split_segments(&stages, &segment_marks),
        stages,
        null_redirections,
    }
}

/// Resolves the recorded segment marks into per-segment pipeline stages.
/// Consecutive stages between two marks belong to the same segment; a mark
/// splits the stage it points into at the recorded token offset.
fn split_segments(stages: &[Vec<String>], marks: &[(usize, usize)]) -> Vec<Vec<Vec<String>>> {
    if marks.is_empty() {
        return Vec::new();
    }
    let mut segments: Vec<Vec<Vec<String>>> = Vec::new();
    let mut current: Vec<Vec<String>> = Vec::new();
    let mut mark_iter = marks.iter().peekable();
    for (stage_index, stage) in stages.iter().enumerate() {
        let mut start = 0usize;
        while let Some(&&(mark_stage, mark_offset)) = mark_iter.peek() {
            if mark_stage != stage_index {
                break;
            }
            mark_iter.next();
            let part = &stage[start..mark_offset.min(stage.len())];
            if !part.is_empty() {
                current.push(part.to_vec());
            }
            if !current.is_empty() {
                segments.push(std::mem::take(&mut current));
            }
            start = mark_offset.min(stage.len());
        }
        let rest = &stage[start..];
        if !rest.is_empty() {
            current.push(rest.to_vec());
        }
    }
    if !current.is_empty() {
        segments.push(current);
    }
    segments
}

pub(super) fn is_env_assignment(token: &str) -> bool {
    let Some((name, _)) = token.split_once('=') else {
        return false;
    };
    !name.is_empty()
        && !name.bytes().next().unwrap_or_default().is_ascii_digit()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn push_token(tokens: &mut Vec<String>, token: &mut String, token_quoted: &mut bool) {
    if !token.is_empty() {
        tokens.push(std::mem::take(token));
    }
    *token_quoted = false;
}

fn max_shape(current: CommandShape, next: CommandShape) -> CommandShape {
    use CommandShape::*;
    let rank = |shape| match shape {
        Empty => 0,
        Simple | EnvSimple => 1,
        Pipeline => 2,
        AndOrList | Sequence | RedirectionRead => 3,
        Complex => 4,
        RedirectionWrite => 5,
        CommandSubstitution => 6,
        Unparseable => 7,
    };
    if rank(next) > rank(current) {
        next
    } else {
        current
    }
}
