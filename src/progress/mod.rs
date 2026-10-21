//! Progress and status reporting for downloads and tool setup.

mod format;
mod render;
mod root;
mod terminal_reporter;

use std::sync::Arc;

pub use format::*;
use render::*;
pub use root::*;
pub use terminal_reporter::*;

/// Progress and status sink used by downloads and tool setup.
pub trait Reporter: Send + Sync {
    fn status(&self, _message: &str) {}
    fn finish_status(&self, _message: &str, _outcome: &str) {}
    fn progress(&self, _data: &Progress) {}
    fn processing(&self, _data: &Processing) {}
    fn name(&self, _title: &str) {}
    fn item(&self, _index: usize, _total: usize, _title: &str) {}
    fn scoped(&self, _index: usize, _total: usize, _title: &str) -> Option<Arc<dyn Reporter>> {
        None
    }
    fn fail_step(&self) {}
    fn finish(&self) {}
    /// The whole run succeeded (terminal title only).
    fn complete(&self) {}
    /// The whole run failed or was cancelled.
    fn fail(&self, _cancelled: bool) {}
}

/// Discards everything; used where nobody watches.
pub struct NullReporter;

impl Reporter for NullReporter {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::cells;

    fn progress(done: f64, total: Option<f64>, estimate: Option<f64>) -> Progress {
        Progress {
            status: Some("downloading".into()),
            downloaded_bytes: Some(done),
            total_bytes: total,
            total_bytes_estimate: estimate,
            speed: Some(1024.0 * 1024.0),
            eta: Some(65.0),
            stream: None,
        }
    }

    #[test]
    fn formats_known_totals() {
        let line = format_progress(&progress(512.0, Some(1024.0), None), &ProgressStyle { frame: 0, ..ProgressStyle::default() });
        assert_eq!(line, "[==========----------]  50%   1.0 MiB/s      512 B / 1.0 KiB  ETA 1:05");
    }

    #[test]
    fn marks_estimates_and_caps_them() {
        let line = format_progress(&progress(2048.0, None, Some(1024.0)), &ProgressStyle { frame: 0, ..ProgressStyle::default() });
        assert!(line.contains(" ~99%"), "{line}");
        assert!(line.contains("2.0 KiB / ~1.0 KiB"), "{line}");
    }

    #[test]
    fn finished_progress_reports_processing() {
        let mut data = progress(2048.0, Some(2048.0), None);
        data.status = Some("finished".into());
        assert_eq!(format_progress(&data, &ProgressStyle { prefix: "Video", ..ProgressStyle::default() }), "Video 2.0 KiB received; processing…");
    }

    #[test]
    fn unknown_total_animates_a_block() {
        let line = format_progress(&progress(10.0, None, None), &ProgressStyle { frame: 2, ..ProgressStyle::default() });
        assert!(line.starts_with("[--===---------------]   ?%"), "{line}");
    }

    #[test]
    fn narrow_terminals_wrap_statistics() {
        let line = format_progress(&progress(512.0, Some(1024.0), None), &ProgressStyle { columns: Some(30), frame: 0, ..ProgressStyle::default() });
        assert!(line.lines().all(|row| cells(row) <= 30), "{line}");
    }

    #[test]
    fn to_fixed_rounds_half_up() {
        assert_eq!(to_fixed(2.5, 0), "3");
        assert_eq!(to_fixed(0.05, 1), "0.1");
        assert_eq!(bytes(Some(1536.0)), "1.5 KiB");
        assert_eq!(bytes(None), "?");
    }
}
