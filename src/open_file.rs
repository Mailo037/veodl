use std::path::Path;
use std::process::{Command, Stdio};

use crate::error::Result;
use crate::paths;

/// Open a saved file with the default app. The absolute filename is passed as
/// one argument, never through a shell, and the opener runs detached.
pub fn open_file(file: &Path) -> Result<()> {
    let command = match paths::platform() {
        "win32" => "explorer.exe",
        "darwin" => "open",
        "android" => "termux-open",
        _ => "xdg-open",
    };
    let mut child = Command::new(command);
    child.arg(paths::resolve(file)).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        child.process_group(0);
    }
    child.spawn()?;
    Ok(())
}
