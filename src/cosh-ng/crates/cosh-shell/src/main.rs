mod activity;
mod agent;
mod approval;
mod evidence;
mod hooks;
mod main_cli;
mod question;
mod question_choices;
mod recommendation;
mod runtime;
mod slash;

use main_cli::{
    adapter_name_from_args, bootstrap_process_path_from_shell, build_adapter,
    passthrough_non_interactive, print_usage_help, raw_shell_from_args_or_default, run_raw,
    should_start_default_raw, RawShellKind,
};

fn main() {
    let args = std::env::args().collect::<Vec<_>>();

    if args.get(1).map(String::as_str) == Some("--version") {
        println!("cosh-shell {}", env!("CARGO_PKG_VERSION"));
        std::process::exit(0);
    }
    if args.get(1).map(String::as_str) == Some("--help") {
        print_usage_help();
        std::process::exit(0);
    }

    runtime::terminal::install_terminal_recovery();

    let has_subcommand = matches!(
        args.get(1).map(String::as_str),
        Some("demo" | "host-demo" | "raw" | "interactive" | "interactive-demo" | "adapter-demo")
    );
    if !has_subcommand {
        if let Some(status) = passthrough_non_interactive(&args) {
            std::process::exit(status);
        }
        if should_start_default_raw(&args[1..]) {
            let config = cosh_shell::load_config();
            let status = run_raw(
                &config.adapter_default,
                raw_shell_from_args_or_default(&args[1..], &config.shell_default),
            );
            std::process::exit(status);
        }
    }

    let status = match args.get(1).map(String::as_str) {
        Some("demo") => runtime::controller::run_demo(),
        Some("host-demo") => runtime::controller::run_host_demo(),
        Some("raw") => {
            let config = cosh_shell::load_config();
            let adapter_name =
                adapter_name_from_args(&args[2..]).unwrap_or(&config.adapter_default);
            run_raw(
                adapter_name,
                raw_shell_from_args_or_default(&args[2..], &config.shell_default),
            )
        }
        Some("interactive") => {
            runtime::controller::run_interactive(args.get(2).map(String::as_str).unwrap_or("fake"))
        }
        Some("interactive-demo") => runtime::controller::run_interactive_demo(
            args.get(2).map(String::as_str).unwrap_or("fake"),
        ),
        Some("adapter-demo") => {
            runtime::controller::run_adapter_demo(args.get(2).map(String::as_str).unwrap_or("fake"))
        }
        _ => {
            eprintln!(
                "usage: cosh-shell <demo|host-demo|raw|interactive|interactive-demo|adapter-demo [fake|claude|co|qwen|cosh-tui] [--shell bash|zsh]>"
            );
            2
        }
    };
    std::process::exit(status);
}
