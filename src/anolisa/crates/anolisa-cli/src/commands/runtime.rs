use clap::{Parser, Subcommand};

#[derive(Parser)]
pub struct RuntimeArgs {
    #[command(subcommand)]
    pub command: RuntimeCommands,
}

#[derive(Subcommand)]
pub enum RuntimeCommands {
    /// Install a runtime component
    Install {
        /// Component name or "all"
        component: String,
        /// Install from source (build locally)
        #[arg(long)]
        from_source: bool,
        /// Install from RPM/DEB repository
        #[arg(long)]
        from_rpm: bool,
        /// Specific version to install
        #[arg(long)]
        version: Option<String>,
    },
    /// Remove a runtime component
    Remove {
        component: String,
        /// Also remove configuration and data
        #[arg(long)]
        purge: bool,
    },
    /// Update a runtime component
    Update {
        /// Component name or "all"
        component: String,
    },
    /// Build a component from source
    Build {
        /// Component name or "all"
        component: String,
        /// Build in release mode (default)
        #[arg(long)]
        release: bool,
        /// Build in debug mode
        #[arg(long)]
        debug: bool,
        /// Build only, do not install
        #[arg(long)]
        no_install: bool,
    },
    /// List runtime components
    List {
        /// Show all available (not just installed)
        #[arg(long)]
        available: bool,
    },
    /// Show component status
    Status {
        /// Specific component (omit for all)
        component: Option<String>,
    },
}

pub fn handle(args: RuntimeArgs) -> anyhow::Result<()> {
    match args.command {
        RuntimeCommands::Install {
            component,
            from_source,
            from_rpm,
            version,
        } => {
            let source = if from_rpm {
                "rpm"
            } else if from_source {
                "source"
            } else {
                "auto"
            };
            println!(
                "runtime install {component} (source={source}, version={})",
                version.as_deref().unwrap_or("latest")
            );
            println!("  → not yet implemented");
        }
        RuntimeCommands::Remove { component, purge } => {
            println!("runtime remove {component} (purge={purge}): not yet implemented");
        }
        RuntimeCommands::Update { component } => {
            println!("runtime update {component}: not yet implemented");
        }
        RuntimeCommands::Build {
            component,
            debug,
            no_install,
            ..
        } => {
            let profile = if debug { "debug" } else { "release" };
            println!(
                "runtime build {component} (profile={profile}, install={})",
                !no_install
            );
            println!("  → not yet implemented");
        }
        RuntimeCommands::List { available } => {
            if available {
                println!("Available ANOLISA runtime components:");
            } else {
                println!("Installed ANOLISA runtime components:");
            }
            println!("  COMPONENT         LAYER      DOMAIN         STATUS");
            println!("  cosh              runtime    tools          -");
            println!("  tokenless         runtime    cost           -");
            println!("  ws-ckpt           runtime    state          -");
            println!("  agentsight        runtime    observability  -");
            println!("  agent-sec-core    runtime    security       -");
            println!("  os-skills         runtime    tools          -");
        }
        RuntimeCommands::Status { component } => {
            println!(
                "runtime status {}: not yet implemented",
                component.as_deref().unwrap_or("(all)")
            );
        }
    }
    Ok(())
}
