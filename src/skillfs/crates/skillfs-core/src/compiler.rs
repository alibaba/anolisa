//! SKILL.md conditional compiler.
//!
//! Transforms generic SKILL.md content into environment-specific output via two strategies:
//!
//! 1. **Precise compilation** (when `<!-- @if ... -->` directives are present):
//!    Evaluates conditional blocks and emits only the content relevant to the
//!    current environment. Directive lines are stripped from output.
//!
//! 2. **Heuristic normalization** (no directives present):
//!    Applies built-in substitution rules (e.g. `pip install` → `uv pip install`
//!    when `uv` is available) to existing SKILL.md files without modification.
//!
//! # Directive Syntax
//!
//! ```markdown
//! <!-- @if has_command("uv") -->
//! Use uv: `uv pip install -r requirements.txt`
//! <!-- @else -->
//! Use pip: `pip install -r requirements.txt`
//! <!-- @endif -->
//!
//! <!-- @if os == darwin -->
//! macOS specific content
//! <!-- @endif -->
//! ```
//!
//! # Supported Expressions
//!
//! | Expression | Description |
//! |---|---|
//! | `os == darwin\|linux\|windows` | OS comparison |
//! | `os != darwin` | Negated OS comparison |
//! | `has_command("tool")` | Command available in PATH |
//! | `has_env("VAR")` | Environment variable is set |
//! | `expr && expr` | Logical AND |
//! | `expr \|\| expr` | Logical OR |

use crate::env::EnvironmentProfile;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Compile `content` for the given `env`.
///
/// Returns the environment-adapted content. Never fails; returns original
/// content on any unexpected state.
pub fn compile(content: &str, env: &EnvironmentProfile) -> String {
    if has_conditional_blocks(content) {
        compile_conditional(content, env)
    } else {
        apply_heuristic_normalization(content, env)
    }
}

// ---------------------------------------------------------------------------
// Conditional block compiler
// ---------------------------------------------------------------------------

/// Returns `true` if `content` contains at least one `<!-- @if` directive.
fn has_conditional_blocks(content: &str) -> bool {
    content.contains("<!-- @if ")
}

/// Compile content that contains `<!-- @if -->` / `<!-- @else -->` / `<!-- @endif -->` blocks.
///
/// Algorithm:
/// - Maintain a stack `emit_at_depth: Vec<bool>` starting with `[true]`.
/// - On `@if expr`: push `eval(expr)` when the parent scope is active, else push `false`.
/// - On `@else`: toggle the top entry **only** when all parent entries are `true`.
/// - On `@endif`: pop the top entry.
/// - Emit a line only when all stack entries are `true`.
fn compile_conditional(content: &str, env: &EnvironmentProfile) -> String {
    let mut output = String::with_capacity(content.len());
    // Depth 0 = root level, always emit.
    let mut emit_at_depth: Vec<bool> = vec![true];

    for line in content.lines() {
        let trimmed = line.trim();

        if let Some(expr) = parse_if_directive(trimmed) {
            // Push: active iff parent scope is active AND condition true.
            let parent_active = emit_at_depth.iter().all(|&e| e);
            let condition = parent_active && evaluate_expr(expr, env);
            emit_at_depth.push(condition);
            continue;
        }

        if is_else_directive(trimmed) {
            if emit_at_depth.len() > 1 {
                // Toggle only when all parent depths are true.
                let len = emit_at_depth.len();
                let parent_active = emit_at_depth[..len - 1].iter().all(|&e| e);
                if parent_active {
                    let last = emit_at_depth.last_mut().unwrap();
                    *last = !*last;
                }
            }
            continue;
        }

        if is_endif_directive(trimmed) {
            if emit_at_depth.len() > 1 {
                emit_at_depth.pop();
            }
            continue;
        }

        // Emit the line when all depth conditions are satisfied.
        if emit_at_depth.iter().all(|&e| e) {
            output.push_str(line);
            output.push('\n');
        }
    }

    // Match trailing newline behaviour of the original content.
    if !content.ends_with('\n') && output.ends_with('\n') {
        output.pop();
    }

    output
}

fn parse_if_directive(line: &str) -> Option<&str> {
    // Format: <!-- @if <expr> -->
    let inner = line.strip_prefix("<!-- @if ")?.strip_suffix(" -->")?;
    Some(inner.trim())
}

fn is_else_directive(line: &str) -> bool {
    line == "<!-- @else -->"
}

fn is_endif_directive(line: &str) -> bool {
    line == "<!-- @endif -->"
}

// ---------------------------------------------------------------------------
// Expression evaluator
// ---------------------------------------------------------------------------

/// Evaluate a boolean expression string against `env`.
///
/// Operator precedence: `||` is evaluated before `&&` (left-to-right scan).
/// Parentheses are not supported in Phase 1.
fn evaluate_expr(expr: &str, env: &EnvironmentProfile) -> bool {
    let expr = expr.trim();

    // Try || first (left-most top-level occurrence).
    if let Some(pos) = find_op(expr, "||") {
        return evaluate_expr(&expr[..pos], env) || evaluate_expr(&expr[pos + 2..], env);
    }

    // Then &&
    if let Some(pos) = find_op(expr, "&&") {
        return evaluate_expr(&expr[..pos], env) && evaluate_expr(&expr[pos + 2..], env);
    }

    evaluate_primitive(expr, env)
}

/// Find the position of `op` in `expr`, ignoring occurrences inside parentheses
/// or quoted strings.
fn find_op(expr: &str, op: &str) -> Option<usize> {
    let bytes = expr.as_bytes();
    let op_bytes = op.as_bytes();
    let mut depth: usize = 0;
    let mut in_quote = false;
    let mut quote_char = b'"';
    let mut i = 0;

    while i < bytes.len() {
        let b = bytes[i];
        if in_quote {
            if b == quote_char {
                in_quote = false;
            }
        } else {
            match b {
                b'"' | b'\'' => {
                    in_quote = true;
                    quote_char = b;
                }
                b'(' => depth += 1,
                b')' => depth = depth.saturating_sub(1),
                _ => {}
            }
            if depth == 0
                && i + op_bytes.len() <= bytes.len()
                && &bytes[i..i + op_bytes.len()] == op_bytes
            {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

/// Evaluate a single primitive expression (no boolean operators).
fn evaluate_primitive(expr: &str, env: &EnvironmentProfile) -> bool {
    let expr = expr.trim();

    // has_command("tool")
    if let Some(arg) = strip_func_arg(expr, "has_command") {
        return env.has_command(unquote(arg));
    }

    // has_env("VAR")
    if let Some(arg) = strip_func_arg(expr, "has_env") {
        return env.has_env(unquote(arg));
    }

    // os == value
    if let Some(pos) = expr.find("==") {
        let lhs = expr[..pos].trim();
        let rhs = unquote(expr[pos + 2..].trim());
        if lhs == "os" {
            return env.os.as_str() == rhs;
        }
    }

    // os != value
    if let Some(pos) = expr.find("!=") {
        let lhs = expr[..pos].trim();
        let rhs = unquote(expr[pos + 2..].trim());
        if lhs == "os" {
            return env.os.as_str() != rhs;
        }
    }

    // Unknown expression: safe default is false.
    false
}

/// Extract the argument from `func_name(...)`.
fn strip_func_arg<'a>(expr: &'a str, func_name: &str) -> Option<&'a str> {
    let prefix = format!("{}(", func_name);
    let inner = expr.strip_prefix(prefix.as_str())?.strip_suffix(')')?;
    Some(inner.trim())
}

/// Strip surrounding single or double quotes from a string.
fn unquote(s: &str) -> &str {
    let s = s.trim();
    if s.len() >= 2
        && ((s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')))
    {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

// ---------------------------------------------------------------------------
// Heuristic normalization
// ---------------------------------------------------------------------------

/// Apply heuristic command substitution rules to `content` without modifying
/// the overall structure of the file.
///
/// Returns a clone of the original content when no rules apply (idempotent).
fn apply_heuristic_normalization(content: &str, env: &EnvironmentProfile) -> String {
    let has_uv = env.has_command("uv");
    let node_pm = detect_best_node_pm(env);

    // Fast path: nothing to do.
    if !has_uv && node_pm.is_empty() {
        return content.to_string();
    }

    let mut output = String::with_capacity(content.len());

    for line in content.lines() {
        output.push_str(&normalize_line(line, has_uv, &node_pm));
        output.push('\n');
    }

    // Match trailing newline of original.
    if !content.ends_with('\n') && output.ends_with('\n') {
        output.pop();
    }

    output
}

/// Apply heuristic substitutions to a single line.
fn normalize_line(line: &str, has_uv: bool, node_pm: &str) -> String {
    let mut result = line.to_string();

    if has_uv {
        // pip install / pip3 install → uv pip install
        for pip_cmd in &["pip3 install", "pip install"] {
            if result.contains(pip_cmd) && !result.contains("uv pip install") {
                result = result.replace(pip_cmd, "uv pip install");
                break; // only one replacement per line
            }
        }

        // python -m venv / python3 -m venv → uv venv
        for venv_cmd in &["python3 -m venv", "python -m venv"] {
            if result.contains(venv_cmd) {
                result = result.replace(venv_cmd, "uv venv");
                break;
            }
        }

        // virtualenv <name> → uv venv <name>
        if result.contains("virtualenv ") && !result.contains("uv venv") {
            result = result.replace("virtualenv ", "uv venv ");
        }
    }

    // Node package manager normalization.
    if !node_pm.is_empty() && node_pm != "npm" {
        let npm_install = "npm install";
        let pm_install = format!("{} install", node_pm);
        if result.contains(npm_install) && !result.contains(&pm_install) {
            result = result.replace(npm_install, &pm_install);
        }

        let npm_run = "npm run ";
        let pm_run = format!("{} run ", node_pm);
        if result.contains(npm_run) {
            result = result.replace(npm_run, &pm_run);
        }

        let npm_test = "npm test";
        let pm_test = format!("{} test", node_pm);
        if result.contains(npm_test) && !result.contains(&pm_test) {
            result = result.replace(npm_test, &pm_test);
        }
    }

    result
}

/// Choose the best available Node package manager (pnpm > yarn > npm).
fn detect_best_node_pm(env: &EnvironmentProfile) -> String {
    if env.has_command("pnpm") {
        "pnpm".to_string()
    } else if env.has_command("yarn") {
        "yarn".to_string()
    } else if env.has_command("npm") {
        "npm".to_string()
    } else {
        String::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::{EnvironmentProfile, OsKind};
    use std::collections::{HashMap, HashSet};

    fn env_darwin_uv() -> EnvironmentProfile {
        let mut cmds = HashSet::new();
        cmds.insert("uv".to_string());
        cmds.insert("python3".to_string());
        EnvironmentProfile {
            os: OsKind::Darwin,
            available_commands: cmds,
            env_vars: HashMap::new(),
        }
    }

    fn env_linux_no_uv() -> EnvironmentProfile {
        let mut cmds = HashSet::new();
        cmds.insert("python3".to_string());
        cmds.insert("pip".to_string());
        EnvironmentProfile {
            os: OsKind::Linux,
            available_commands: cmds,
            env_vars: HashMap::new(),
        }
    }

    fn env_node_pnpm() -> EnvironmentProfile {
        let mut cmds = HashSet::new();
        cmds.insert("pnpm".to_string());
        cmds.insert("node".to_string());
        EnvironmentProfile {
            os: OsKind::Linux,
            available_commands: cmds,
            env_vars: HashMap::new(),
        }
    }

    // -----------------------------------------------------------------------
    // @if/@else/@endif tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_if_true_emits_if_block() {
        let env = env_darwin_uv();
        let content = "A\n<!-- @if os == darwin -->\nB\n<!-- @endif -->\nC\n";
        let result = compile(content, &env);
        assert!(result.contains('A'), "A should be emitted");
        assert!(result.contains('B'), "B (darwin block) should be emitted");
        assert!(result.contains('C'), "C should be emitted");
        assert!(!result.contains("@if"), "directives should be stripped");
    }

    #[test]
    fn test_if_false_skips_if_block() {
        let env = env_linux_no_uv();
        let content = "A\n<!-- @if os == darwin -->\nB\n<!-- @endif -->\nC\n";
        let result = compile(content, &env);
        assert!(result.contains('A'));
        assert!(!result.contains('B'), "B should be skipped on linux");
        assert!(result.contains('C'));
    }

    #[test]
    fn test_if_else_endif() {
        let env = env_darwin_uv();
        let content =
            "<!-- @if os == darwin -->\ndarwin-line\n<!-- @else -->\nlinux-line\n<!-- @endif -->\n";
        let result = compile(content, &env);
        assert!(
            result.contains("darwin-line"),
            "darwin block should be emitted"
        );
        assert!(
            !result.contains("linux-line"),
            "else block should be skipped"
        );
    }

    #[test]
    fn test_if_false_else_emitted() {
        let env = env_linux_no_uv();
        let content =
            "<!-- @if os == darwin -->\ndarwin-line\n<!-- @else -->\nlinux-line\n<!-- @endif -->\n";
        let result = compile(content, &env);
        assert!(!result.contains("darwin-line"));
        assert!(result.contains("linux-line"));
    }

    #[test]
    fn test_has_command_true() {
        let env = env_darwin_uv();
        let content = "<!-- @if has_command(\"uv\") -->\nuv-line\n<!-- @else -->\npip-line\n<!-- @endif -->\n";
        let result = compile(content, &env);
        assert!(result.contains("uv-line"));
        assert!(!result.contains("pip-line"));
    }

    #[test]
    fn test_has_command_false_uses_else() {
        let env = env_linux_no_uv();
        let content = "<!-- @if has_command(\"uv\") -->\nuv-line\n<!-- @else -->\npip-line\n<!-- @endif -->\n";
        let result = compile(content, &env);
        assert!(!result.contains("uv-line"));
        assert!(result.contains("pip-line"));
    }

    #[test]
    fn test_nested_if_parent_false_skips_child() {
        let env = env_linux_no_uv();
        // Parent @if false → both if and else blocks in child should be skipped
        let content = "<!-- @if os == darwin -->\n<!-- @if has_command(\"uv\") -->\nA\n<!-- @else -->\nB\n<!-- @endif -->\n<!-- @endif -->\nC\n";
        let result = compile(content, &env);
        assert!(!result.contains('A'), "A should be skipped: parent false");
        assert!(!result.contains('B'), "B should be skipped: parent false");
        assert!(result.contains('C'));
    }

    #[test]
    fn test_nested_if_both_true() {
        let env = env_darwin_uv();
        let content = "<!-- @if os == darwin -->\n<!-- @if has_command(\"uv\") -->\nA\n<!-- @endif -->\n<!-- @endif -->\n";
        let result = compile(content, &env);
        assert!(result.contains('A'));
    }

    #[test]
    fn test_no_directives_returns_original() {
        let env = env_linux_no_uv();
        let content = "Hello world\nno directives here\n";
        // env_linux_no_uv has no uv, no pnpm/yarn → nothing to normalize
        let result = compile(content, &env);
        assert_eq!(result, content);
    }

    #[test]
    fn test_directives_stripped_from_output() {
        let env = env_darwin_uv();
        let content = "A\n<!-- @if os == darwin -->\nB\n<!-- @endif -->\n";
        let result = compile(content, &env);
        assert!(!result.contains("<!-- @if"));
        assert!(!result.contains("<!-- @endif -->"));
    }

    // -----------------------------------------------------------------------
    // Heuristic normalization tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_heuristic_pip_to_uv_pip() {
        let env = env_darwin_uv();
        let content = "Run: pip install requests\n";
        let result = compile(content, &env);
        assert!(
            result.contains("uv pip install"),
            "should use uv pip install"
        );
        // The result should NOT contain a bare "pip install" (without the "uv " prefix).
        // We check by splitting on "uv pip install" and ensuring no fragment starts with "pip install".
        assert!(
            !result
                .replace("uv pip install", "__REPLACED__")
                .contains("pip install"),
            "bare pip install should be gone after substitution"
        );
    }

    #[test]
    fn test_heuristic_pip3_to_uv_pip() {
        let env = env_darwin_uv();
        let content = "pip3 install -r requirements.txt\n";
        let result = compile(content, &env);
        assert!(result.contains("uv pip install -r requirements.txt"));
    }

    #[test]
    fn test_heuristic_venv_to_uv_venv() {
        let env = env_darwin_uv();
        let content = "python -m venv .venv\n";
        let result = compile(content, &env);
        assert!(result.contains("uv venv .venv"));
    }

    #[test]
    fn test_heuristic_virtualenv_to_uv_venv() {
        let env = env_darwin_uv();
        let content = "virtualenv myenv\n";
        let result = compile(content, &env);
        assert!(result.contains("uv venv myenv"));
    }

    #[test]
    fn test_heuristic_no_double_replace() {
        let env = env_darwin_uv();
        let content = "uv pip install requests\n";
        let result = compile(content, &env);
        // Should not become "uv uv pip install"
        assert_eq!(result, content);
    }

    #[test]
    fn test_heuristic_npm_to_pnpm() {
        let env = env_node_pnpm();
        let content = "npm install\nnpm run build\nnpm test\n";
        let result = compile(content, &env);
        assert!(result.contains("pnpm install"));
        assert!(result.contains("pnpm run build"));
        assert!(result.contains("pnpm test"));
    }

    #[test]
    fn test_heuristic_no_uv_unchanged() {
        let env = env_linux_no_uv();
        let content = "pip install requests\n";
        // No uv available → no substitution
        let result = compile(content, &env);
        assert_eq!(result, content);
    }

    // -----------------------------------------------------------------------
    // Expression evaluator tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_expr_and_short_circuit() {
        let env = env_linux_no_uv();
        // "os == linux && has_command(uv)" → false (uv not present)
        assert!(!evaluate_expr("os == linux && has_command(\"uv\")", &env));
        // "os == linux && has_command(python3)" → true
        assert!(evaluate_expr(
            "os == linux && has_command(\"python3\")",
            &env
        ));
    }

    #[test]
    fn test_expr_or() {
        let env = env_linux_no_uv();
        assert!(evaluate_expr("os == darwin || os == linux", &env));
        assert!(!evaluate_expr("os == darwin || os == windows", &env));
    }

    #[test]
    fn test_expr_os_neq() {
        let env = env_darwin_uv();
        assert!(evaluate_expr("os != linux", &env));
        assert!(!evaluate_expr("os != darwin", &env));
    }

    #[test]
    fn test_unquote_single_char_no_panic() {
        assert_eq!(unquote("\""), "\"");
        assert_eq!(unquote("'"), "'");
        assert_eq!(unquote(""), "");
    }

    #[test]
    fn test_unquote_matched_pair() {
        assert_eq!(unquote("\"x\""), "x");
        assert_eq!(unquote("'x'"), "x");
        assert_eq!(unquote("\"\""), "");
        assert_eq!(unquote("''"), "");
    }

    #[test]
    fn test_unquote_mismatched_quotes() {
        assert_eq!(unquote("\"x'"), "\"x'");
        assert_eq!(unquote("'x\""), "'x\"");
    }

    #[test]
    fn test_compile_malformed_directives_never_panic() {
        let env = env_linux_no_uv();
        let cases = [
            "<!-- @if has_command(\") -->\nA\n<!-- @endif -->",
            "<!-- @if os == \" -->\nA\n<!-- @endif -->",
            "<!-- @if os == ' -->\nA\n<!-- @endif -->",
            "<!-- @if has_command(\")\") -->\nA\n<!-- @endif -->",
            "<!-- @if has_command(\"\") -->\nA\n<!-- @endif -->",
            "<!-- @if has_command('') -->\nA\n<!-- @endif -->",
            "<!-- @if has_command(') -->\nA\n<!-- @endif -->",
            "<!-- @if os == -->\nA\n<!-- @endif -->",
        ];
        for input in cases {
            let _ = compile(input, &env);
        }
    }

    #[test]
    fn test_trailing_newline_preserved() {
        let env = env_darwin_uv();
        let with_newline = "line\n";
        let without_newline = "line";
        let r1 = compile(with_newline, &env);
        let r2 = compile(without_newline, &env);
        assert!(r1.ends_with('\n'));
        assert!(!r2.ends_with('\n'));
    }
}
