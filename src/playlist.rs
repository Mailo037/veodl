use serde::Serialize;

use crate::error::{Error, Result};
use crate::metadata::{positive, Metadata};

pub fn validate_items(value: &str) -> Result<&str> {
    if !re!(r"^[0-9]+(?:-[0-9]+)?(?:,[0-9]+(?:-[0-9]+)?)*$").is_match(value) {
        return Err(Error::msg("--playlist-items expects positive indices or ranges, e.g. 1,3-5."));
    }
    for (start, end) in ranges(value) {
        if start < 1 || end < start {
            return Err(Error::msg("--playlist-items requires ascending, positive ranges."));
        }
    }
    Ok(value)
}

fn ranges(value: &str) -> Vec<(u64, u64)> {
    value
        .split(',')
        .map(|part| {
            let mut bounds = part.split('-').map(|bound| bound.parse::<u64>().unwrap_or(u64::MAX));
            let start = bounds.next().unwrap_or(0);
            (start, bounds.next().unwrap_or(start))
        })
        .collect()
}

/// A playlist entry with its 1-based position in the collection.
pub struct Selected<'a> {
    pub entry: Option<&'a Metadata>,
    pub index: u64,
}

pub fn selected_entries<'a>(metadata: &'a Metadata, selection: Option<&str>) -> Result<Vec<Selected<'a>>> {
    let ranges = match selection {
        Some(selection) => Some(ranges(validate_items(selection)?)),
        None => None,
    };
    let selected: Vec<Selected> = metadata
        .entries
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .enumerate()
        .map(|(offset, entry)| {
            let index = entry.as_ref().and_then(|entry| positive(entry.playlist_index)).map_or(offset as u64 + 1, |index| index as u64);
            Selected { entry: entry.as_ref(), index }
        })
        .filter(|item| ranges.as_ref().is_none_or(|ranges| ranges.iter().any(|(start, end)| item.index >= *start && item.index <= *end)))
        .collect();
    if selected.is_empty() {
        return Err(Error::msg("No playlist entries match the selection."));
    }
    Ok(selected)
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Estimate {
    pub estimated_bytes: f64,
    pub known_sizes: usize,
    pub total_entries: usize,
}

pub fn size_estimate(entries: &[Selected]) -> Estimate {
    let known: Vec<f64> = entries.iter().filter_map(|item| item.entry.and_then(|entry| positive(entry.filesize).or(positive(entry.filesize_approx)))).collect();
    Estimate { estimated_bytes: known.iter().sum(), known_sizes: known.len(), total_entries: entries.len() }
}

pub fn describe_estimate(estimate: &Estimate) -> String {
    if estimate.known_sizes == 0 {
        return format!("{} entries; total size unknown", estimate.total_entries);
    }
    let remaining =
        if estimate.known_sizes < estimate.total_entries { format!(" for {} entries; remaining sizes unknown", estimate.known_sizes) } else { String::new() };
    format!("{} entries; approximately {} MiB{remaining}", estimate.total_entries, crate::progress::to_fixed(estimate.estimated_bytes / 1_048_576.0, 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn playlist(count: usize) -> Metadata {
        Metadata {
            entries: Some(
                (0..count)
                    .map(|index| Some(Metadata { title: Some(format!("e{index}")), filesize: (index % 2 == 0).then_some(1_048_576.0), ..Metadata::default() }))
                    .collect(),
            ),
            ..Metadata::default()
        }
    }

    #[test]
    fn validates_selections() {
        assert!(validate_items("1,3-5").is_ok());
        assert!(validate_items("0").is_err());
        assert!(validate_items("5-3").is_err());
        assert!(validate_items("a").is_err());
    }

    #[test]
    fn selects_and_estimates() {
        let metadata = playlist(5);
        let selected = selected_entries(&metadata, Some("2-4")).unwrap();
        assert_eq!(selected.iter().map(|item| item.index).collect::<Vec<_>>(), vec![2, 3, 4]);
        assert_eq!(describe_estimate(&size_estimate(&selected)), "3 entries; approximately 1.0 MiB for 1 entries; remaining sizes unknown");
        assert!(selected_entries(&metadata, Some("9")).is_err());
    }
}
