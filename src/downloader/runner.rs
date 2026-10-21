//! Running yt-dlp and other backend tools with streamed output.

use std::path::Path;

use crate::cancel::Cancel;
use crate::error::{Error, Result};
use crate::process::{self, Command, Lines, Stream};

/// Runs a backend tool: `(executable, args, cancel, on_line)` → stdout.
pub type Runner<'a> = dyn Fn(&Path, &[String], &Cancel, Option<&mut dyn FnMut(&str) -> Result<()>>) -> Result<String> + Sync + 'a;

/// Run yt-dlp (or ffprobe). With `on_line`, stdout lines and veo progress lines
/// from stderr are streamed; otherwise stdout is returned (max 32 MiB).
pub fn run_backend(executable: &Path, args: &[String], cancel: &Cancel, mut on_line: Option<&mut dyn FnMut(&str) -> Result<()>>) -> Result<String> {
    let command = Command::new(executable).args(args.iter().cloned());
    let mut output: Vec<u8> = Vec::new();
    let mut errors = String::new();
    let (mut out_lines, mut err_lines) = (Lines::default(), Lines::default());
    let streaming = on_line.is_some();
    let code = process::run(&command, cancel, |stream, data| {
        match stream {
            Stream::Stdout => {
                if let Some(on_line) = on_line.as_mut() {
                    out_lines.push(data, |line| on_line(line))?;
                } else if output.len() + data.len() < 32 * 1024 * 1024 {
                    output.extend_from_slice(data);
                } else {
                    return Err(Error::msg("Video metadata exceeded the supported size."));
                }
            }
            Stream::Stderr => {
                errors.push_str(&String::from_utf8_lossy(data));
                if errors.len() > 16000 {
                    let cut = errors.len() - 16000;
                    let boundary = (cut..errors.len()).find(|index| errors.is_char_boundary(*index)).unwrap_or(errors.len());
                    errors.drain(..boundary);
                }
                if let Some(on_line) = on_line.as_mut() {
                    err_lines
                        .push(data, |line| if line.starts_with("veo-progress:") || line.starts_with("veo-postprocess:") { on_line(line) } else { Ok(()) })?;
                }
            }
        }
        Ok(())
    })?;
    if let Some(on_line) = on_line.as_mut() {
        out_lines.finish(|line| on_line(line))?;
    }
    if code != Some(0) {
        let diagnostics: Vec<&str> = errors.lines().filter(|line| !re!(r"^veo-(?:progress|postprocess):").is_match(line)).collect();
        let failure = diagnostics.iter().rev().find(|line| re!(r"(?i)^ERROR:").is_match(line));
        let message = match failure {
            Some(line) => line.to_string(),
            None if !diagnostics.join("\n").is_empty() => diagnostics.join("\n"),
            None => format!("Downloading backend exited with code {}.", code.map_or("null".into(), |code| code.to_string())),
        };
        return Err(Error::msg(message));
    }
    let _ = streaming;
    Ok(String::from_utf8_lossy(&output).into_owned())
}
