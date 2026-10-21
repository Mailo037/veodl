// Match misspelled flags without silently accepting them. Ignore punctuation
// differences first, then allow a small number of insertions or transpositions.
fn edit_distance(left: &[char], right: &[char]) -> usize {
    let mut rows = vec![vec![0usize; right.len() + 1]; left.len() + 1];
    for (i, row) in rows.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in rows[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=left.len() {
        for j in 1..=right.len() {
            let substitution = rows[i - 1][j - 1] + usize::from(left[i - 1] != right[j - 1]);
            rows[i][j] = (rows[i - 1][j] + 1).min(rows[i][j - 1] + 1).min(substitution);
            if i > 1 && j > 1 && left[i - 1] == right[j - 2] && left[i - 2] == right[j - 1] {
                rows[i][j] = rows[i][j].min(rows[i - 2][j - 2] + 1);
            }
        }
    }
    rows[left.len()][right.len()]
}

fn normalized(value: &str) -> Vec<char> {
    value.to_lowercase().chars().filter(|c| *c != '-').collect()
}

pub fn suggest(unknown: &str, options: &[String]) -> Option<String> {
    let target = normalized(unknown);
    let prefix = if unknown.starts_with("--") {
        Some("--")
    } else if unknown.starts_with('-') {
        Some("-")
    } else {
        None
    };
    let mut seen = std::collections::HashSet::new();
    let mut ranked: Vec<(usize, &String)> = options
        .iter()
        .filter(|option| seen.insert(option.as_str()))
        .filter(|option| match prefix {
            Some(prefix) => option.starts_with(prefix),
            None => !option.starts_with('-'),
        })
        .map(|option| (edit_distance(&target, &normalized(option)), option))
        .collect();
    ranked.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.chars().count().cmp(&b.1.chars().count())));
    let (distance, best) = ranked.first()?;
    let limit = (target.len() / 3).clamp(1, 3);
    (*distance <= limit).then(|| best.to_string())
}

#[cfg(test)]
fn suggest_str(unknown: &str, options: &[&str]) -> Option<String> {
    suggest(unknown, &options.iter().map(|option| option.to_string()).collect::<Vec<_>>())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggests_close_options() {
        assert_eq!(suggest_str("--qualty", &["--quality", "--output"]).as_deref(), Some("--quality"));
        assert_eq!(suggest_str("--dryrun", &["--dry-run"]).as_deref(), Some("--dry-run"));
        assert_eq!(suggest_str("histroy", &["history", "stats"]).as_deref(), Some("history"));
        assert_eq!(suggest_str("--zzz", &["--quality"]), None);
    }
}
