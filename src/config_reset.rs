use std::fs;
use std::io::Write;
use std::path::Path;

use crate::config_template::CONFIG_TEMPLATE;
use crate::error::{Error, Result};
use crate::output::Out;
use crate::paths;
use crate::state;
use crate::terminal::{self, Std};

/// Replace the config with the default template after a backup and an explicit y/N.
pub fn reset_config_with(file: &Path, output: &Out, confirm: &dyn Fn(&str) -> Result<String>) -> Result<i32> {
    let file = paths::resolve(file);
    output.line(format!("Reset configuration and all profiles: {}", file.display()));
    output.line("The existing file will be backed up. Downloads, history and statistics are preserved.");
    let answer = confirm("Reset configuration? [y/N] ")?;
    if !re!(r"(?i)^y(?:es)?$").is_match(answer.trim()) {
        output.line("Config reset cancelled.");
        return Ok(0);
    }
    let original = crate::error::optional(fs::read(&file))?;
    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent)?;
    }
    if let Some(original) = original {
        let stamp = state::iso_now().replace([':', '.'], "-");
        let backup = file.with_file_name(format!("{}.{stamp}.{}.bak", paths::basename(&file), state::uuid()));
        state::create_private(&backup)?.write_all(&original)?;
        output.line(format!("Backup: {}", backup.display()));
    }
    state::write_atomic(&file, CONFIG_TEMPLATE.as_bytes())?;
    output.line(format!("Config reset: {}", file.display()));
    Ok(0)
}

pub fn reset_config(file: &Path, output: &Out) -> Result<i32> {
    reset_config_with(file, output, &|prompt| {
        if !terminal::stdin_is_tty() {
            return Err(Error::msg("Config reset requires an interactive terminal for confirmation."));
        }
        terminal::ask(prompt, Std::Out)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backs_up_before_resetting() {
        let root = crate::fsutil::temp_dir(&std::env::temp_dir(), "veo-reset-").unwrap();
        let file = root.join("config.json");
        fs::write(&file, "{\"quality\":\"720p\"}").unwrap();
        let out = Out::new(Std::Out, true);
        reset_config_with(&file, &out, &|_| Ok("n".into())).unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "{\"quality\":\"720p\"}");
        reset_config_with(&file, &out, &|_| Ok("yes".into())).unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), CONFIG_TEMPLATE);
        assert_eq!(fs::read_dir(&root).unwrap().filter(|entry| entry.as_ref().unwrap().file_name().to_string_lossy().ends_with(".bak")).count(), 1);
        let _ = fs::remove_dir_all(root);
    }
}
