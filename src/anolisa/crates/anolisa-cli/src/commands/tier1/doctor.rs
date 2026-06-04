use clap::Parser;

use crate::context::CliContext;
use crate::response::CliError;

#[derive(Parser)]
pub struct DoctorArgs {
    /// Diagnose a specific capability (default: all enabled)
    pub capability: Option<String>,
    /// Apply suggested fixes automatically.
    ///
    /// `doctor` with no `--fix` is read-only. `--fix` executes the fix
    /// plan inside a transaction. Combining `--dry-run --fix` is
    /// rejected as `INVALID_ARGUMENT`: `--dry-run` alone already shows
    /// the diagnostic plan; `--fix` is the explicit "execute" verb.
    #[arg(long)]
    pub fix: bool,
}

pub fn handle(args: DoctorArgs, ctx: &CliContext) -> Result<(), CliError> {
    let command = match &args.capability {
        Some(cap) => format!("doctor {cap}"),
        None => "doctor".to_string(),
    };

    if ctx.dry_run && args.fix {
        return Err(CliError::InvalidArgument {
            command,
            reason: "--dry-run --fix is invalid; --dry-run alone prints fix plan, --fix executes"
                .to_string(),
        });
    }

    Err(CliError::not_implemented(command))
}
