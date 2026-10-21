use crate::error::{Error, Result};
use crate::options::Options;
use crate::terminal::{self, Std};

pub fn validate_fragments(value: f64) -> Result<()> {
    if value.fract() != 0.0 || !(1.0..=64.0).contains(&value) {
        return Err(Error::msg("--concurrent-fragments must be a whole number between 1 and 64."));
    }
    Ok(())
}

/// 17-64 fragments need an explicit opt-in, or a y/N answer in a terminal.
pub fn confirm_with(requests: &[Options], interactive: bool, confirm: &dyn Fn(&str) -> Result<String>) -> Result<()> {
    for request in requests {
        validate_fragments(f64::from(request.concurrent_fragments))?;
    }
    let pending: Vec<&Options> = requests
        .iter()
        .filter(|request| {
            request.concurrent_fragments > 16
                && !request.experimental_fragments
                && !request.dry_run
                && !request.list_qualities
                && !request.list_formats
                && !request.list_sources
        })
        .collect();
    if pending.is_empty() {
        return Ok(());
    }
    if !interactive || pending.iter().any(|request| request.json) {
        return Err(Error::msg("17-64 concurrent fragments are experimental. Explicitly accept with --experimental-fragments or experimentalFragments: true in your config/profile."));
    }
    let maximum = pending.iter().map(|request| request.concurrent_fragments).max().unwrap_or(0);
    let answer = confirm(&format!(
        "Experimental: up to {maximum} concurrent fragments. More connections may trigger server limits and may not improve speed. Continue? [y/N] "
    ))?;
    if !re!(r"(?i)^y(?:es)?$").is_match(answer.trim()) {
        return Err(Error::msg("Experimental fragment download cancelled."));
    }
    Ok(())
}

pub fn confirm_experimental_fragments(requests: &[Options]) -> Result<()> {
    confirm_with(requests, terminal::stdin_is_tty() && Std::Err.is_tty(), &|prompt| terminal::ask(prompt, Std::Err))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(fragments: u32, accepted: bool) -> Options {
        Options { concurrent_fragments: fragments, experimental_fragments: accepted, ..Options::default() }
    }

    #[test]
    fn requires_opt_in_above_sixteen() {
        assert!(confirm_with(&[request(16, false)], false, &|_| unreachable!()).is_ok());
        assert!(confirm_with(&[request(32, false)], false, &|_| unreachable!()).is_err());
        assert!(confirm_with(&[request(32, true)], false, &|_| unreachable!()).is_ok());
        assert!(confirm_with(&[request(32, false)], true, &|_| Ok("y".into())).is_ok());
        assert!(confirm_with(&[request(32, false)], true, &|_| Ok("".into())).is_err());
        assert!(validate_fragments(65.0).is_err());
    }
}
