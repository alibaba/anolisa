use clap::Parser;

#[derive(Parser)]
pub struct EnvArgs {
    /// Include all probe details
    #[arg(long)]
    pub verbose: bool,
}

pub fn handle(args: EnvArgs) -> anyhow::Result<()> {
    let facts = anolisa_env::EnvFacts::placeholder();
    if args.verbose {
        println!("{:#?}", facts);
    } else {
        println!("Platform:    {:?}", facts.platform);
        println!("Kernel:      {}", facts.kernel.version);
        println!(
            "Distro:      {} {}",
            facts.distro.name, facts.distro.version
        );
        println!("Arch:        {:?}", facts.arch);
        println!(
            "Filesystem:  btrfs={}, overlayfs={}",
            facts.filesystem.btrfs_available, facts.filesystem.overlayfs_available
        );
        println!("Frameworks:  {} detected", facts.frameworks.len());
    }
    Ok(())
}
