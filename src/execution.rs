use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{Map, Value};

use crate::cancel::Cancel;
use crate::error::{Error, Result};
use crate::progress::Reporter;

/// Shared back-off: retries halve the parallelism of every running download.
#[derive(Debug)]
pub struct Adaptive {
    divisor: AtomicU32,
}

impl Default for Adaptive {
    fn default() -> Self {
        Adaptive { divisor: AtomicU32::new(1) }
    }
}

impl Adaptive {
    pub fn reduced(&self, limit: u32) -> u32 {
        (limit / self.divisor.load(Ordering::SeqCst).max(1)).max(1)
    }

    fn back_off(&self) {
        let current = self.divisor.load(Ordering::SeqCst).max(1);
        self.divisor.store((current * 2).min(16), Ordering::SeqCst);
    }
}

/// Run `work` over `items` with at most `limit()` workers. Workers finish their
/// active item before stopping, including after cancellation or failure.
pub fn run_pool<T: Sync>(items: &[T], limit: &(dyn Fn() -> u32 + Sync), cancel: &Cancel, work: &(dyn Fn(&T, usize) -> Result<()> + Sync)) -> Result<()> {
    let next = AtomicUsize::new(0);
    let failure: Mutex<Option<Error>> = Mutex::new(None);
    let workers = (limit() as usize).min(items.len());
    std::thread::scope(|scope| {
        for worker in 0..workers {
            let (next, failure) = (&next, &failure);
            scope.spawn(move || loop {
                if worker as u32 >= limit() {
                    return;
                }
                if let Err(error) = cancel.check() {
                    failure.lock().unwrap().get_or_insert(error);
                    return;
                }
                let index = next.fetch_add(1, Ordering::SeqCst);
                let Some(item) = items.get(index) else { return };
                if let Err(error) = work(item, index) {
                    failure.lock().unwrap().get_or_insert(error);
                    return;
                }
            });
        }
    });
    match failure.into_inner().unwrap() {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

// yt-dlp reports a failed concurrent fragment fetch as a local ENOENT on the
// fragment temp file (e.g. "...media.mp4.part-Frag29"). Those are worth
// retrying with reduced parallelism; permanent failures still fail fast.
pub fn retryable(error: &Error) -> bool {
    re!(r"(?i)HTTP(?: Error)?\s*(?:429|5[0-9][0-9])|too many requests|timed? ?out|ECONNRESET|ECONNREFUSED|ETIMEDOUT|temporary failure|connection (?:reset|aborted)|\.part-Frag[0-9]+").is_match(&error.message)
}

pub fn adaptive_run<T>(
    enabled: bool,
    state: &Adaptive,
    cancel: &Cancel,
    reporter: &dyn Reporter,
    timer: Option<&PhaseTimer>,
    wait: &dyn Fn(Duration, &Cancel) -> Result<()>,
    mut run: impl FnMut(u32) -> Result<T>,
) -> Result<T> {
    let mut attempt = 0u32;
    loop {
        cancel.check()?;
        match run(attempt) {
            Ok(value) => return Ok(value),
            Err(error) => {
                if !enabled || cancel.is_cancelled() || !retryable(&error) || attempt >= 2 {
                    return Err(error);
                }
                state.back_off();
                let ms = 2000u64 * 2u64.pow(attempt);
                reporter.status(&format!("Source busy or connection interrupted; reducing parallelism and retrying in {}s ({}/2).", ms / 1000, attempt + 1));
                let previous = timer.map(PhaseTimer::phase);
                if let Some(timer) = timer {
                    timer.switch("retryWait");
                }
                let waited = wait(Duration::from_millis(ms), cancel);
                if let (Some(timer), Some(previous)) = (timer, previous) {
                    timer.switch(&previous);
                }
                waited?;
                attempt += 1;
            }
        }
    }
}

pub fn real_wait(duration: Duration, cancel: &Cancel) -> Result<()> {
    cancel.sleep(duration)
}

/// Time spent per phase of one download.
pub struct PhaseTimer {
    inner: Mutex<(BTreeMap<String, f64>, Vec<String>, String, Instant)>,
}

impl Default for PhaseTimer {
    fn default() -> Self {
        PhaseTimer { inner: Mutex::new((BTreeMap::new(), Vec::new(), "setup".into(), Instant::now())) }
    }
}

impl PhaseTimer {
    pub fn phase(&self) -> String {
        self.inner.lock().unwrap().2.clone()
    }

    pub fn switch(&self, next: &str) {
        let mut inner = self.inner.lock().unwrap();
        let now = Instant::now();
        let elapsed = now.duration_since(inner.3).as_secs_f64() * 1000.0;
        let phase = inner.2.clone();
        if !inner.0.contains_key(&phase) {
            inner.1.push(phase.clone());
        }
        *inner.0.entry(phase).or_insert(0.0) += elapsed;
        inner.2 = next.to_string();
        inner.3 = now;
    }

    /// Milliseconds per phase in first-seen order.
    pub fn result(&self) -> Map<String, Value> {
        let phase = self.phase();
        self.switch(&phase);
        let inner = self.inner.lock().unwrap();
        inner.1.iter().map(|key| (key.clone(), Value::from(inner.0[key].round() as u64))).collect()
    }
}

pub fn format_timings(timings: &Map<String, Value>) -> String {
    let label = |phase: &str| match phase {
        "setup" => "setup".to_string(),
        "metadata" => "metadata".into(),
        "download" => "download/backend".into(),
        "processing" => "merge/processing".into(),
        "saving" => "saving".into(),
        "retryWait" => "retry wait".into(),
        other => other.into(),
    };
    let parts: Vec<String> =
        timings.iter().map(|(phase, ms)| format!("{} {}s", label(phase), crate::progress::to_fixed(ms.as_f64().unwrap_or(0.0) / 1000.0, 1))).collect();
    format!("Timing: {}", parts.join(" | "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::progress::NullReporter;

    #[test]
    fn retries_transient_errors_with_back_off() {
        let state = Adaptive::default();
        let mut calls = 0;
        let result = adaptive_run(true, &state, &Cancel::new(), &NullReporter, None, &|_, _| Ok(()), |attempt| {
            calls += 1;
            if attempt < 2 {
                Err(Error::msg("HTTP Error 503"))
            } else {
                Ok(attempt)
            }
        });
        assert_eq!(result.unwrap(), 2);
        assert_eq!(calls, 3);
        assert_eq!(state.reduced(8), 2);
    }

    #[test]
    fn permanent_errors_fail_fast() {
        let mut calls = 0;
        let result: Result<()> = adaptive_run(true, &Adaptive::default(), &Cancel::new(), &NullReporter, None, &|_, _| Ok(()), |_| {
            calls += 1;
            Err(Error::msg("Video unavailable"))
        });
        assert!(result.is_err());
        assert_eq!(calls, 1);
    }

    #[test]
    fn pool_runs_every_item() {
        let seen = Mutex::new(Vec::new());
        run_pool(&[1, 2, 3, 4, 5], &|| 2, &Cancel::new(), &|item, _| {
            seen.lock().unwrap().push(*item);
            Ok(())
        })
        .unwrap();
        let mut seen = seen.into_inner().unwrap();
        seen.sort();
        assert_eq!(seen, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn formats_timings() {
        let mut timings = Map::new();
        timings.insert("download".into(), 1250.into());
        timings.insert("saving".into(), 50.into());
        assert_eq!(format_timings(&timings), "Timing: download/backend 1.3s | saving 0.1s");
    }
}
