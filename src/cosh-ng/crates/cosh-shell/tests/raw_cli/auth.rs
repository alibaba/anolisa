use super::*;

/// Fake cosh-core exposing a single OpenAI Compatible template and logging registry traffic.
///
/// The template omits `provider_id` because slash auth injects it as the first field.
const AUTH_REGISTRY_CORE: &str = r#"#!/bin/sh
if [ "$1" = "--registry" ]; then
  read -r request
  printf '%s\n' "$request" >> "$AUTH_REGISTRY_LOG"
  case "$request" in
    *'"action":"state"'*)
      printf '%s\n' '{"type":"registry_response","request_id":"reg","success":true,"data":{"templates":[{"id":"openai_compat","label":"OpenAI Compatible","fields":[{"name":"base_url","label":"Base URL","hint":null,"secret":false,"required":true,"placeholder":null},{"name":"api_key","label":"API Key","hint":null,"secret":true,"required":true,"placeholder":null},{"name":"model","label":"Model","hint":null,"secret":false,"required":true,"placeholder":null}]}],"saved_providers":[]}}'
      ;;
    *)
      printf '%s\n' '{"type":"registry_response","request_id":"reg","success":true,"data":{"model":"main-model","configured":true}}'
      ;;
  esac
  exit 0
fi
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"auth-inline","model":"main-model","tools":[]}'
printf '%s\n' '{"type":"result","subtype":"success","session_id":"auth-inline","is_error":false,"result":"done"}'
"#;

/// A dotted Provider ID must be rejected on the spot instead of at the final `configure`.
#[test]
fn raw_cli_auth_dotted_provider_id_stays_on_provider_id_field() {
    let home = temp_shell_home("auth-dotted-provider-id");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let cosh_core_path = bin_dir.join("cosh-core");
    write_executable(&cosh_core_path, AUTH_REGISTRY_CORE);
    let registry_log = home.join("registry.log");

    let home_str = home.to_string_lossy().to_string();
    let core_str = cosh_core_path.to_string_lossy().to_string();
    let log_str = registry_log.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_env_current_dir_and_marker_input(
        "cosh-core",
        &[],
        &[
            ("HOME", &home_str),
            ("COSH_CORE_PATH", &core_str),
            ("AUTH_REGISTRY_LOG", &log_str),
        ],
        Path::new(env!("CARGO_MANIFEST_DIR")),
        &[
            ("cosh-osc$", b"/auth\n".as_slice()),
            ("Left/Right move | Enter send", b"\n".as_slice()),
            ("Type answer | Enter send", b"qwen3.7-max\n".as_slice()),
            ("Provider ID allows letters", b"".as_slice()),
        ],
    );
    let requests = fs::read_to_string(&registry_log).unwrap_or_default();
    let _ = fs::remove_dir_all(&home);

    let compact = compact_terminal_words(&output);
    // The rejected field keeps the focus: Base URL is never reached.
    assert!(
        compact.contains("Enter Provider ID"),
        "expected Provider ID prompt: {output}"
    );
    assert!(
        !compact.contains("Enter Base URL"),
        "flow advanced past Provider ID: {output}"
    );
    assert!(
        compact.contains("Provider ID allows letters, digits, '-' and '_' only (no '.')"),
        "expected inline Provider ID error: {output}"
    );
    // The hint states the rule before the user types anything.
    assert!(
        compact.contains("Config name (not model name; letters, digits, '-' and '_' only)"),
        "expected character rule in hint: {output}"
    );
    // Nothing reached cosh-core: no configure round-trip, no late failure panel.
    assert!(
        !requests.contains(r#""action":"configure""#),
        "auth configure must not be called: {requests}"
    );
    assert!(
        !output.contains("Credentials were not saved"),
        "late core rejection panel should not appear: {output}"
    );
    assert!(
        requests.contains(r#""action":"state""#),
        "expected auth state query: {requests}"
    );
}
