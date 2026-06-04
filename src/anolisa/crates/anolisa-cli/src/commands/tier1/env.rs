use clap::Parser;

#[derive(Parser)]
pub struct EnvArgs {
    /// Include all probe details
    #[arg(long)]
    pub verbose: bool,
}

pub fn handle(args: EnvArgs) -> anyhow::Result<()> {
    let facts = anolisa_env::EnvService::detect();
    if args.verbose {
        println!("{:#?}", facts);
    } else {
        println!("OS:          {}", facts.os);
        println!("Kernel:      {}", display_opt(facts.kernel.as_deref()));
        println!("Pkg base:    {}", display_opt(facts.pkg_base.as_deref()));
        println!("Arch:        {}", facts.arch);
        println!("Libc:        {}", display_opt(facts.libc.as_deref()));
        println!("BTF:         {}", display_opt_bool(facts.btf));
        println!("CAP_BPF:     {}", display_opt_bool(facts.cap_bpf));
        println!("Container:   {}", display_opt(facts.container.as_deref()));
        println!("User:        {} ({})", facts.user, facts.uid);
        println!("Home:        {}", facts.home.display());
    }
    Ok(())
}

fn display_opt(v: Option<&str>) -> &str {
    v.unwrap_or("unknown")
}

fn display_opt_bool(v: Option<bool>) -> String {
    v.map(|b| b.to_string()).unwrap_or_else(|| "unknown".into())
}
