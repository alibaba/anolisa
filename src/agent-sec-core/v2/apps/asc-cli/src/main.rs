use clap::Parser as _;

fn main() {
    let cli = asc_cli::Cli::parse();
    if let Err(problem) = asc_cli::execute(cli, &mut std::io::stdout().lock()) {
        eprintln!("{problem}");
        std::process::exit(1);
    }
}
