use std::time::Duration;

use crate::cancel::Cancel;
use crate::error::{Error, Result};
use crate::paths;
use crate::process::{self, Command, Stream};

const MAX_TEXT: usize = 2 * 1024 * 1024;

/// Clipboard programs to try, the active display server's first.
pub fn clipboard_commands(copy: bool, platform: &str) -> Vec<(String, Vec<String>)> {
    let owned = |program: &str, args: &[&str]| (program.to_string(), args.iter().map(|arg| arg.to_string()).collect());
    match platform {
        "win32" => {
            let setup = "Add-Type -AssemblyName System.Windows.Forms; [Console]::InputEncoding = [System.Text.UTF8Encoding]::new($false); [Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false); ";
            let action = if copy {
                "[System.Windows.Forms.Clipboard]::SetText([Console]::In.ReadToEnd())"
            } else {
                "[Console]::Out.Write([System.Windows.Forms.Clipboard]::GetText())"
            };
            vec![owned("powershell.exe", &["-NoProfile", "-NonInteractive", "-Sta", "-Command", &format!("{setup}{action}")])]
        }
        "darwin" => vec![owned(if copy { "pbcopy" } else { "pbpaste" }, &[])],
        "android" => vec![owned(if copy { "termux-clipboard-set" } else { "termux-clipboard-get" }, &[])],
        _ => {
            let wayland = owned(if copy { "wl-copy" } else { "wl-paste" }, if copy { &[] } else { &["--no-newline"] });
            let xclip = owned("xclip", &["-selection", "clipboard", if copy { "-in" } else { "-out" }]);
            let xsel = owned("xsel", &["--clipboard", if copy { "--input" } else { "--output" }]);
            if std::env::var_os("WAYLAND_DISPLAY").is_some() {
                vec![wayland, xclip, xsel]
            } else if std::env::var_os("DISPLAY").is_some() {
                vec![xclip, xsel, wayland]
            } else {
                vec![wayland, xclip, xsel]
            }
        }
    }
}

fn run(program: &str, args: &[String], copy: bool, value: &str) -> Result<String> {
    let mut command = Command::new(program).args(args.iter().cloned()).timeout(Duration::from_secs(5), "Clipboard command failed.");
    // Clipboard owners such as xclip may keep running; never hold their output pipes.
    command.capture_stdout = !copy;
    command.capture_stderr = false;
    if copy {
        command.stdin = Some(value.as_bytes().to_vec());
    }
    let mut output = Vec::new();
    let code = process::run(&command, &Cancel::new(), |stream, data| {
        if stream == Stream::Stdout {
            if output.len() + data.len() > MAX_TEXT {
                return Err(Error::msg("Clipboard command failed."));
            }
            output.extend_from_slice(data);
        }
        Ok(())
    })?;
    if code != Some(0) {
        return Err(Error::msg("Clipboard command failed."));
    }
    Ok(String::from_utf8_lossy(&output).into_owned())
}

pub fn clipboard_text(copy: bool, value: &str) -> Result<String> {
    if value.len() > MAX_TEXT {
        return Err(Error::msg("Selection is too large for the editor clipboard."));
    }
    for (program, args) in clipboard_commands(copy, paths::platform()) {
        if let Ok(text) = run(&program, &args, copy, value) {
            return Ok(text);
        }
    }
    let hint = match paths::platform() {
        "linux" => " Install wl-clipboard, xclip or xsel and use a graphical session.",
        "android" => " Install the Termux:API app and pkg install termux-api.",
        _ => "",
    };
    Err(Error::msg(format!("System clipboard is unavailable.{hint}")))
}

pub fn copy(value: &str) -> Result<()> {
    clipboard_text(true, value).map(|_| ())
}

pub fn paste() -> Result<String> {
    clipboard_text(false, "")
}
