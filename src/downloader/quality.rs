//! Mapping a requested quality to a yt-dlp format selector.

use crate::metadata::Format;
use crate::utils::{available_heights, capped_height, closest_height};

pub fn format_selector(height: Option<u32>) -> String {
    let filter = height.map(|height| format!("[height={height}]")).unwrap_or_default();
    format!("bv{filter}+ba/b{filter}/bv{filter}")
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct QualityChoice {
    pub format: Option<String>,
    pub sort: Option<String>,
    pub label: Option<String>,
    pub error: Option<String>,
}

/// Turn a requested quality into backend arguments and a user-facing label.
/// Numeric qualities are an upper bound; `closest` picks the nearest height.
pub fn select_quality(quality: &str, audio: bool, closest: bool, formats: &[Format], playlist: bool) -> QualityChoice {
    if audio || quality == "best" {
        return QualityChoice::default();
    }
    let target = crate::utils::quality_target(quality);
    if playlist {
        // Entries differ in resolution, so cap them inside the backend instead.
        return QualityChoice {
            sort: Some(format!("res:{target}")),
            label: Some(if closest { format!("Quality: closest to {target}p per entry") } else { format!("Quality: up to {target}p per entry") }),
            ..QualityChoice::default()
        };
    }
    let heights = available_heights(formats);
    if heights.is_empty() {
        return QualityChoice { label: Some("Resolution unknown; using the best available stream.".into()), ..QualityChoice::default() };
    }
    if closest {
        let height = closest_height(formats, quality);
        let label = match height {
            Some(height) if height == target => format!("Quality: {height}p"),
            Some(height) => format!("Quality: {height}p (closest to {quality})"),
            None => "Quality: best".into(),
        };
        return QualityChoice { format: Some(format_selector(height)), label: Some(label), ..QualityChoice::default() };
    }
    let Some(height) = capped_height(formats, quality) else {
        let mut offered = heights.clone();
        offered.sort_unstable();
        let offered = offered.iter().map(|value| format!("{value}p")).collect::<Vec<_>>().join(", ");
        return QualityChoice {
            error: Some(format!("No stream at or below {target}p is available (offered: {offered}). Use a higher --quality or \"best\".")),
            ..QualityChoice::default()
        };
    };
    let label = if height == target { format!("Quality: {height}p") } else { format!("Quality: {height}p (highest at or below {quality})") };
    QualityChoice { format: Some(format_selector(Some(height))), label: Some(label), ..QualityChoice::default() }
}
