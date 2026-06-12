pub mod core;
pub mod shell;

/// Slash command trait for cosh-tui.
pub trait SlashCommand {
    fn name(&self) -> &str;
    fn aliases(&self) -> &[&str] {
        &[]
    }
    fn description(&self) -> &str;
    fn execute(&self, args: &str, app: &mut crate::app::App) -> CommandResult;
}

/// Result of executing a slash command.
pub enum CommandResult {
    Output(String),
    Error(String),
    Clear,
    Quit,
    EnterShell,
}

/// Registry of all slash commands.
pub struct CommandRegistry {
    commands: Vec<Box<dyn SlashCommand>>,
}

impl CommandRegistry {
    pub fn new() -> Self {
        let mut reg = Self {
            commands: Vec::new(),
        };
        reg.register(Box::new(core::HelpCommand));
        reg.register(Box::new(core::AboutCommand));
        reg.register(Box::new(core::ClearCommand));
        reg.register(Box::new(core::QuitCommand));
        reg.register(Box::new(core::InitCommand));
        reg.register(Box::new(core::StatsCommand));
        reg.register(Box::new(core::ThemeCommand));
        reg.register(Box::new(core::ModelCommand));
        reg.register(Box::new(core::CompressCommand));
        reg.register(Box::new(core::MemoryCommand));
        reg.register(Box::new(core::ResumeCommand));
        reg.register(Box::new(core::ExportCommand));
        reg.register(Box::new(core::CopyCommand));
        reg.register(Box::new(core::RenameCommand));
        reg.register(Box::new(core::ApprovalModeCommand));
        reg.register(Box::new(shell::BashCommand));
        reg
    }

    pub fn register(&mut self, cmd: Box<dyn SlashCommand>) {
        self.commands.push(cmd);
    }

    pub fn find(&self, name: &str) -> Option<&dyn SlashCommand> {
        self.commands
            .iter()
            .find(|c| c.name() == name || c.aliases().contains(&name))
            .map(|c| c.as_ref())
    }

    /// Check if a slash command exists by name.
    #[cfg(test)]
    pub fn contains(&self, name: &str) -> bool {
        self.commands
            .iter()
            .any(|c| c.name() == name || c.aliases().contains(&name))
    }

    pub fn list(&self) -> Vec<(&str, &str)> {
        self.commands
            .iter()
            .map(|c| (c.name(), c.description()))
            .collect()
    }
}
