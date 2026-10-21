use std::cmp::Ordering;

use crate::error::{Error, Result};

struct Parsed {
    parts: Vec<u64>,
    pre: Vec<String>,
}

fn parse(value: &str) -> Result<Parsed> {
    let trimmed = value.trim();
    let trimmed = trimmed.strip_prefix('v').unwrap_or(trimmed);
    let (core, pre) = match trimmed.split_once('-') {
        Some((core, rest)) => (core, rest.split('-').next().unwrap_or("")),
        None => (trimmed, ""),
    };
    let valid = !core.is_empty() && core.split('.').all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()));
    if !valid {
        return Err(Error::msg(format!("Invalid version: {value}")));
    }
    Ok(Parsed {
        parts: core.split('.').map(|part| part.parse().unwrap_or(u64::MAX)).collect(),
        pre: if pre.is_empty() { Vec::new() } else { pre.split('.').map(str::to_string).collect() },
    })
}

/// Shared semver-ish comparison for veo releases and yt-dlp release dates.
pub fn compare(a: &str, b: &str) -> Result<Ordering> {
    let left = parse(a)?;
    let right = parse(b)?;
    for index in 0..left.parts.len().max(right.parts.len()) {
        let ordering = left.parts.get(index).unwrap_or(&0).cmp(right.parts.get(index).unwrap_or(&0));
        if ordering != Ordering::Equal {
            return Ok(ordering);
        }
    }
    // A release outranks any prerelease of the same core version.
    match (left.pre.is_empty(), right.pre.is_empty()) {
        (true, true) => return Ok(Ordering::Equal),
        (true, false) => return Ok(Ordering::Greater),
        (false, true) => return Ok(Ordering::Less),
        _ => {}
    }
    for index in 0..left.pre.len().max(right.pre.len()) {
        let (Some(l), Some(r)) = (left.pre.get(index), right.pre.get(index)) else {
            return Ok(if left.pre.len() < right.pre.len() { Ordering::Less } else { Ordering::Greater });
        };
        let numeric = |value: &str| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit());
        let ordering = match (numeric(l), numeric(r)) {
            (true, true) => l.parse::<u64>().unwrap_or(0).cmp(&r.parse::<u64>().unwrap_or(0)),
            // Numeric identifiers rank below alphanumerics.
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            (false, false) => l.cmp(r),
        };
        if ordering != Ordering::Equal {
            return Ok(ordering);
        }
    }
    Ok(Ordering::Equal)
}

/// Comparison for sorting known-valid versions; invalid ones sort as equal.
pub fn cmp(a: &str, b: &str) -> Ordering {
    compare(a, b).unwrap_or(Ordering::Equal)
}

pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_releases_and_prereleases() {
        assert_eq!(cmp("1.10.0", "1.9.9"), Ordering::Greater);
        assert_eq!(cmp("v1.2.3", "1.2.3"), Ordering::Equal);
        assert_eq!(cmp("1.2.3-beta.1", "1.2.3"), Ordering::Less);
        assert_eq!(cmp("1.2.3-beta.2", "1.2.3-beta.10"), Ordering::Less);
        assert_eq!(cmp("1.2.3-alpha", "1.2.3-1"), Ordering::Greater);
        assert_eq!(cmp("2026.08.19", "2026.08.19.1"), Ordering::Less);
        assert!(compare("x.1", "1").is_err());
    }
}
