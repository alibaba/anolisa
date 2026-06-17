use super::command_risk::CommandShape;

#[derive(Debug, Clone)]
pub(super) struct ParsedCommand {
    pub(super) shape: CommandShape,
    pub(super) stages: Vec<Vec<String>>,
}

pub(super) fn parse_command(command: &str) -> ParsedCommand {
    if command.is_empty() {
        return ParsedCommand {
            shape: CommandShape::Empty,
            stages: Vec::new(),
        };
    }
    if command.contains('\0') {
        return ParsedCommand {
            shape: CommandShape::Unparseable,
            stages: Vec::new(),
        };
    }

    let mut tokens = Vec::new();
    let mut token = String::new();
    let mut stages: Vec<Vec<String>> = Vec::new();
    let mut shape = CommandShape::Simple;
    let mut quote: Option<char> = None;
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
            '\'' | '"' => quote = Some(ch),
            ' ' | '\t' => push_token(&mut tokens, &mut token),
            '\n' | ';' => {
                push_token(&mut tokens, &mut token);
                shape = max_shape(shape, CommandShape::Sequence);
            }
            '|' => {
                push_token(&mut tokens, &mut token);
                if chars.peek().is_some_and(|next| *next == '|') {
                    chars.next();
                    shape = max_shape(shape, CommandShape::AndOrList);
                } else {
                    stages.push(std::mem::take(&mut tokens));
                    shape = max_shape(shape, CommandShape::Pipeline);
                }
            }
            '&' => {
                push_token(&mut tokens, &mut token);
                if chars.peek().is_some_and(|next| *next == '&') {
                    chars.next();
                    shape = max_shape(shape, CommandShape::AndOrList);
                } else {
                    shape = max_shape(shape, CommandShape::Complex);
                }
            }
            '>' => {
                push_token(&mut tokens, &mut token);
                if chars.peek().is_some_and(|next| *next == '>') {
                    chars.next();
                }
                shape = max_shape(shape, CommandShape::RedirectionWrite);
            }
            '<' => {
                push_token(&mut tokens, &mut token);
                shape = max_shape(shape, CommandShape::RedirectionRead);
            }
            '`' => {
                push_token(&mut tokens, &mut token);
                shape = max_shape(shape, CommandShape::CommandSubstitution);
            }
            '$' if chars.peek().is_some_and(|next| *next == '(') => {
                push_token(&mut tokens, &mut token);
                chars.next();
                shape = max_shape(shape, CommandShape::CommandSubstitution);
            }
            '(' | ')' | '{' | '}' => {
                push_token(&mut tokens, &mut token);
                shape = max_shape(shape, CommandShape::Complex);
            }
            '\\' => {
                if let Some(next) = chars.next() {
                    token.push(next);
                }
            }
            _ => token.push(ch),
        }
    }

    if quote.is_some() {
        return ParsedCommand {
            shape: CommandShape::Unparseable,
            stages: Vec::new(),
        };
    }
    push_token(&mut tokens, &mut token);
    if !tokens.is_empty() {
        stages.push(tokens);
    }
    if matches!(shape, CommandShape::Simple) {
        if stages.first().is_some_and(|tokens| {
            tokens
                .iter()
                .take_while(|token| is_env_assignment(token))
                .count()
                > 0
        }) {
            shape = CommandShape::EnvSimple;
        }
    }

    ParsedCommand { shape, stages }
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

fn push_token(tokens: &mut Vec<String>, token: &mut String) {
    if !token.is_empty() {
        tokens.push(std::mem::take(token));
    }
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
