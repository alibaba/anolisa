use crate::runtime::prelude::*;

pub(crate) fn write_shell_prompt<W: Write>(
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    if std::env::var("COSH_SHELL_ISOLATED").is_ok() {
        writeln!(output)?;
        write!(output, "cosh-osc$ ")
    } else {
        state.trigger_pty_prompt = true;
        Ok(())
    }
}

pub(super) fn clear_shell_prompt_line<W: Write>(output: &mut W) -> std::io::Result<()> {
    write!(output, "\r\x1b[2K")
}
