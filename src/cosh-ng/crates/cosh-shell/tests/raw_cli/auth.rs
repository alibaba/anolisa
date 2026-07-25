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

/// Fake cosh-core for the `/auth` management menu.
///
/// Serves the scripted `auth.state` / `auth.prepare` replies from the environment so a
/// single script covers ECS and non-ECS hosts, and appends every request to
/// `$AUTH_REGISTRY_LOG` so tests can count ECS probes.
const AUTH_MENU_CORE: &str = r#"#!/bin/sh
if [ "$1" = "--registry" ]; then
  read -r request
  printf '%s\n' "$request" >> "$AUTH_REGISTRY_LOG"
  case "$request" in
    *'"action":"state"'*)
      printf '%s\n' "$AUTH_STATE"
      ;;
    *'"action":"prepare"'*)
      printf '%s\n' "$AUTH_PREPARE"
      ;;
    *)
      printf '%s\n' '{"type":"registry_response","request_id":"reg","success":true,"data":{"authorized":true,"model":"main-model","configured":true}}'
      ;;
  esac
  exit 0
fi
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"auth-inline","model":"main-model","tools":[]}'
printf '%s\n' '{"type":"result","subtype":"success","session_id":"auth-inline","is_error":false,"result":"done"}'
"#;

/// The builtin template order cosh-core reports: DashScope, OpenAI Compatible, Aliyun.
const AUTH_TEMPLATES: &str = r#"[{"id":"dashscope","label":"DashScope (百炼)","fields":[{"name":"api_key","label":"API Key","hint":null,"secret":true,"required":true,"placeholder":null},{"name":"model","label":"Model","hint":null,"secret":false,"required":false,"placeholder":"qwen3.7-plus"}]},{"id":"openai_compat","label":"OpenAI Compatible","fields":[{"name":"base_url","label":"Base URL","hint":null,"secret":false,"required":true,"placeholder":null},{"name":"api_key","label":"API Key","hint":null,"secret":true,"required":true,"placeholder":null},{"name":"model","label":"Model","hint":null,"secret":false,"required":true,"placeholder":null}]},{"id":"aliyun","label":"Aliyun Authentication","fields":[{"name":"access_key_id","label":"Access Key ID","hint":null,"secret":true,"required":true,"placeholder":null},{"name":"access_key_secret","label":"Access Key Secret","hint":null,"secret":true,"required":true,"placeholder":null},{"name":"model","label":"Model","hint":null,"secret":false,"required":false,"placeholder":"qwen3.7-plus"}]}]"#;

const SAVED_NONE: &str = "[]";

const SAVED_DASHSCOPE: &str = r#"[{"provider_id":"qwen-prod","provider_type":"dashscope","source":"user","editable":true,"auth_source":null,"model":"qwen3.7-plus","base_url":null,"api_key_len":8,"active":true}]"#;

/// A saved SysOM setup is the `aliyun` provider with `auth_source=ecs_ram_role`.
const SAVED_DASHSCOPE_AND_ECS: &str = r#"[{"provider_id":"qwen-prod","provider_type":"dashscope","source":"user","editable":true,"auth_source":null,"model":"qwen3.7-plus","base_url":null,"api_key_len":8,"active":true},{"provider_id":"sysom-trial","provider_type":"aliyun","source":"user","editable":true,"auth_source":"ecs_ram_role","model":"qwen3.7-plus","base_url":null,"active":false}]"#;

const ECS_PREPARE: &str = r#"{"type":"registry_response","request_id":"reg","success":true,"data":{"mode":"ecs_ram_role","instance_id":"i-fake-ecs-1","console_url":"https://alinux.console.aliyun.com/cn-test/guide/cosh?instance=i-fake-ecs-1","values":{"auth_source":"ecs_ram_role"}}}"#;

const MANUAL_PREPARE: &str =
    r#"{"type":"registry_response","request_id":"reg","success":true,"data":{"mode":"manual"}}"#;

const FAILING_PREPARE: &str = r#"{"type":"registry_response","request_id":"reg","success":false,"error":"ecs metadata service unavailable"}"#;

const SYSOM_ROW: &str = "SysOM (free trial, uses this ECS instance's RAM role)";

fn auth_state_response(saved_providers: &str) -> String {
    format!(
        r#"{{"type":"registry_response","request_id":"reg","success":true,"data":{{"templates":{AUTH_TEMPLATES},"saved_providers":{saved_providers}}}}}"#
    )
}

/// Drives `/auth` against the scripted core and returns `(terminal output, registry log)`.
fn run_auth_menu_flow(
    name: &str,
    saved_providers: &str,
    prepare: &str,
    steps: &[(&str, &[u8])],
) -> (String, String) {
    let home = temp_shell_home(name);
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let cosh_core_path = bin_dir.join("cosh-core");
    write_executable(&cosh_core_path, AUTH_MENU_CORE);
    let registry_log = home.join("registry.log");

    let home_str = home.to_string_lossy().to_string();
    let core_str = cosh_core_path.to_string_lossy().to_string();
    let log_str = registry_log.to_string_lossy().to_string();
    let state = auth_state_response(saved_providers);
    let output = run_raw_cli_with_args_env_current_dir_and_marker_input(
        "cosh-core",
        &[],
        &[
            ("HOME", &home_str),
            ("COSH_CORE_PATH", &core_str),
            ("AUTH_REGISTRY_LOG", &log_str),
            ("AUTH_STATE", &state),
            ("AUTH_PREPARE", prepare),
        ],
        Path::new(env!("CARGO_MANIFEST_DIR")),
        steps,
    );
    let requests = fs::read_to_string(&registry_log).unwrap_or_default();
    let _ = fs::remove_dir_all(&home);
    (output, requests)
}

fn action_count(requests: &str, action: &str) -> usize {
    requests.matches(&format!(r#""action":"{action}""#)).count()
}

/// On ECS the free trial is the first thing `/auth` offers, even with nothing configured.
#[test]
fn raw_cli_auth_ecs_offers_sysom_first_without_saved_providers() {
    let (output, requests) = run_auth_menu_flow(
        "auth-ecs-no-saved",
        SAVED_NONE,
        ECS_PREPARE,
        &[
            ("cosh-osc$", b"/auth\n".as_slice()),
            ("+ Add new provider", b"".as_slice()),
        ],
    );

    let compact = compact_terminal_words(&output);
    assert!(compact.contains("Provider Management"), "{output}");
    // Selected by default: the panel marks the current row with '>'.
    assert!(compact.contains(&format!("> [1] {SYSOM_ROW}")), "{output}");
    assert!(compact.contains("[2] + Add new provider"), "{output}");
    // The template picker must not be what an ECS user sees first.
    assert!(!compact.contains("Authentication Required"), "{output}");
    assert_eq!(action_count(&requests, "prepare"), 1, "{requests}");
}

/// The SysOM row shifts the saved providers down without breaking their action menus.
#[test]
fn raw_cli_auth_ecs_keeps_saved_provider_reachable_below_sysom() {
    let (output, requests) = run_auth_menu_flow(
        "auth-ecs-saved-dashscope",
        SAVED_DASHSCOPE,
        ECS_PREPARE,
        &[
            ("cosh-osc$", b"/auth\n".as_slice()),
            ("+ Add new provider", b"\x1b[C\n".as_slice()),
            ("\"qwen-prod\":", b"".as_slice()),
        ],
    );

    let compact = compact_terminal_words(&output);
    assert!(compact.contains(&format!("> [1] {SYSOM_ROW}")), "{output}");
    assert!(compact.contains("[2] * [active] DashScope"), "{output}");
    assert!(compact.contains("[3] + Add new provider"), "{output}");
    // Row 2 opened the saved provider's action menu, not the SysOM shortcut.
    assert!(compact.contains("Edit configuration"), "{output}");
    assert!(compact.contains("Delete provider"), "{output}");
    assert!(!compact.contains("Enter Provider ID"), "{output}");
    assert_eq!(action_count(&requests, "prepare"), 1, "{requests}");
    assert_eq!(action_count(&requests, "configure"), 0, "{requests}");
}

/// An already-configured RAM-role provider is promoted instead of duplicated.
#[test]
fn raw_cli_auth_ecs_promotes_configured_ram_role_provider() {
    let (output, requests) = run_auth_menu_flow(
        "auth-ecs-saved-ram-role",
        SAVED_DASHSCOPE_AND_ECS,
        ECS_PREPARE,
        &[
            ("cosh-osc$", b"/auth\n".as_slice()),
            ("+ Add new provider", b"\n".as_slice()),
            ("\"sysom-trial\":", b"".as_slice()),
        ],
    );

    let compact = compact_terminal_words(&output);
    // Row 1 is the saved provider wearing the SysOM label, and there is no extra row for it.
    assert!(
        compact.contains(&format!("> [1] {SYSOM_ROW} - \"sysom-trial\"")),
        "{output}"
    );
    assert!(!compact.contains(&format!("[2] {SYSOM_ROW}")), "{output}");
    assert!(compact.contains("[2] * [active] DashScope"), "{output}");
    assert!(compact.contains("[3] + Add new provider"), "{output}");
    // Selecting it opens the ordinary action menu instead of re-running the setup flow.
    assert!(compact.contains("Set as active provider"), "{output}");
    assert!(compact.contains("Edit configuration"), "{output}");
    assert!(compact.contains("Delete provider"), "{output}");
    assert!(!compact.contains("Enter Provider ID"), "{output}");
    assert_eq!(action_count(&requests, "prepare"), 1, "{requests}");
    assert_eq!(action_count(&requests, "delete"), 0, "{requests}");
}

/// A non-ECS host keeps the template picker and its `dashscope, openai_compat, aliyun` order.
#[test]
fn raw_cli_auth_non_ecs_without_saved_providers_keeps_template_order() {
    let (output, requests) = run_auth_menu_flow(
        "auth-non-ecs-no-saved",
        SAVED_NONE,
        MANUAL_PREPARE,
        &[
            ("cosh-osc$", b"/auth\n".as_slice()),
            ("Authentication Required", b"".as_slice()),
        ],
    );

    let compact = compact_terminal_words(&output);
    assert!(compact.contains("> [1] DashScope"), "{output}");
    assert!(compact.contains("[2] OpenAI Compatible"), "{output}");
    assert!(compact.contains("[3] Aliyun Authentication"), "{output}");
    assert!(!compact.contains("SysOM"), "{output}");
    assert!(!compact.contains("Provider Management"), "{output}");
    assert_eq!(action_count(&requests, "prepare"), 1, "{requests}");
}

/// A non-ECS host keeps "saved providers + Add new provider", including Edit.
#[test]
fn raw_cli_auth_non_ecs_keeps_saved_providers_then_add_new() {
    let (output, requests) = run_auth_menu_flow(
        "auth-non-ecs-saved",
        SAVED_DASHSCOPE,
        MANUAL_PREPARE,
        &[
            ("cosh-osc$", b"/auth\n".as_slice()),
            ("+ Add new provider", b"\n".as_slice()),
            ("Edit configuration", b"\n".as_slice()),
            ("Edit API Key", b"".as_slice()),
        ],
    );

    let compact = compact_terminal_words(&output);
    assert!(compact.contains("> [1] * [active] DashScope"), "{output}");
    assert!(compact.contains("[2] + Add new provider"), "{output}");
    assert!(!compact.contains("[3] + Add new provider"), "{output}");
    assert!(!compact.contains("SysOM"), "{output}");
    // Edit still pre-fills the masked API key and offers to keep it.
    assert!(compact.contains("Edit API Key"), "{output}");
    assert!(compact.contains("Enter to keep current value"), "{output}");
    assert_eq!(action_count(&requests, "configure"), 0, "{requests}");
}

/// The shortcut is not a blank cheque: the Provider ID rule still applies to it.
#[test]
fn raw_cli_auth_sysom_shortcut_still_validates_provider_id() {
    let (output, requests) = run_auth_menu_flow(
        "auth-sysom-bad-id",
        SAVED_NONE,
        ECS_PREPARE,
        &[
            ("cosh-osc$", b"/auth\n".as_slice()),
            ("+ Add new provider", b"\n".as_slice()),
            ("Enter Provider ID", b"bad.provider\n".as_slice()),
            ("Provider ID allows letters", b"".as_slice()),
        ],
    );

    let compact = compact_terminal_words(&output);
    assert!(
        compact.contains("Provider ID allows letters, digits, '-' and '_' only (no '.')"),
        "{output}"
    );
    // A rejected id must not reach the challenge or the registry.
    assert!(!compact.contains("ECS Instance ID"), "{output}");
    assert_eq!(action_count(&requests, "configure"), 0, "{requests}");
    assert_eq!(action_count(&requests, "prepare"), 1, "{requests}");
}

/// The shortcut reuses the challenge `/auth` prefetched and configures aliyun + RAM role.
#[test]
fn raw_cli_auth_sysom_shortcut_reuses_prefetched_challenge() {
    let (output, requests) = run_auth_menu_flow(
        "auth-sysom-shortcut",
        SAVED_NONE,
        ECS_PREPARE,
        &[
            ("cosh-osc$", b"/auth\n".as_slice()),
            ("+ Add new provider", b"\n".as_slice()),
            ("Enter Provider ID", b"sysom-trial\n".as_slice()),
            ("ECS Instance ID", b"\n".as_slice()),
            ("Auth configured", b"".as_slice()),
        ],
    );

    let compact = compact_terminal_words(&output);
    assert!(compact.contains("i-fake-ecs-1"), "{output}");
    assert!(compact.contains("Auth configured"), "{output}");
    // The ECS metadata service is probed once, when `/auth` builds the menu.
    assert_eq!(action_count(&requests, "prepare"), 1, "{requests}");
    let configure = requests
        .lines()
        .find(|line| line.contains(r#""action":"configure""#))
        .unwrap_or_else(|| panic!("expected configure request: {requests}"));
    assert!(
        configure.contains(r#""provider_type":"aliyun""#),
        "{configure}"
    );
    assert!(
        configure.contains(r#""provider_id":"sysom-trial""#),
        "{configure}"
    );
    assert!(
        configure.contains(r#""auth_source":"ecs_ram_role""#),
        "{configure}"
    );
    // No manual credential ever entered the flow, so none may be persisted.
    assert!(!configure.contains("access_key_id"), "{configure}");
    assert!(!configure.contains("security_token"), "{configure}");
}

/// A failing `auth.prepare` is a missing recommendation, not a broken `/auth`.
#[test]
fn raw_cli_auth_prepare_failure_falls_back_to_the_existing_menu() {
    let (output, requests) = run_auth_menu_flow(
        "auth-prepare-failure",
        SAVED_DASHSCOPE,
        FAILING_PREPARE,
        &[
            ("cosh-osc$", b"/auth\n".as_slice()),
            ("+ Add new provider", b"".as_slice()),
        ],
    );

    let compact = compact_terminal_words(&output);
    assert!(compact.contains("Provider Management"), "{output}");
    assert!(compact.contains("> [1] * [active] DashScope"), "{output}");
    assert!(compact.contains("[2] + Add new provider"), "{output}");
    assert!(!compact.contains("SysOM"), "{output}");
    assert!(!compact.contains("Auth unavailable"), "{output}");
    assert_eq!(action_count(&requests, "prepare"), 1, "{requests}");
}

/// Manual Aliyun AK/SK stays available off ECS, with the secret fields still masked.
#[test]
fn raw_cli_auth_non_ecs_aliyun_falls_back_to_manual_keys() {
    let (output, requests) = run_auth_menu_flow(
        "auth-non-ecs-aliyun-manual",
        SAVED_NONE,
        MANUAL_PREPARE,
        &[
            ("cosh-osc$", b"/auth\n".as_slice()),
            ("Authentication Required", b"\x1b[C\x1b[C\n".as_slice()),
            ("Enter Provider ID", b"aliyun-manual\n".as_slice()),
            ("Enter Access Key ID", b"AK-TEST-VALUE\n".as_slice()),
            ("Enter Access Key Secret", b"".as_slice()),
        ],
    );

    let compact = compact_terminal_words(&output);
    assert!(compact.contains("Enter Access Key ID"), "{output}");
    assert!(compact.contains("Enter Access Key Secret"), "{output}");
    assert!(!compact.contains("ECS Instance ID"), "{output}");
    // Secret fields are echoed as bullets, never as the typed key.
    assert!(!compact.contains("AK-TEST-VALUE"), "{output}");
    assert!(compact.contains('\u{2022}'), "{output}");
    // The successful startup result is reused after the Provider ID is accepted.
    assert_eq!(action_count(&requests, "prepare"), 1, "{requests}");
}
