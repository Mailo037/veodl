use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use crate::error::{Error, Result};

/// A cancellation token shared by threads. A child token stops with its parent
/// and may carry its own deadline; `timed_out` tells a deadline from the user.
#[derive(Clone, Debug, Default)]
pub struct Cancel {
    inner: Arc<Inner>,
}

#[derive(Debug, Default)]
struct Inner {
    flag: AtomicBool,
    deadline: Option<Instant>,
    parent: Option<Cancel>,
}

const SLICE: Duration = Duration::from_millis(25);

impl Cancel {
    pub fn new() -> Self {
        Cancel::default()
    }

    /// The process-wide token stopped by Ctrl+C, SIGTERM and `veo stop`.
    pub fn global() -> &'static Cancel {
        static GLOBAL: OnceLock<Cancel> = OnceLock::new();
        GLOBAL.get_or_init(Cancel::new)
    }

    pub fn child_with_timeout(&self, timeout: Duration) -> Cancel {
        Cancel { inner: Arc::new(Inner { flag: AtomicBool::new(false), deadline: Some(Instant::now() + timeout), parent: Some(self.clone()) }) }
    }

    pub fn cancel(&self) {
        self.inner.flag.store(true, Ordering::SeqCst);
    }

    pub fn timed_out(&self) -> bool {
        self.inner.deadline.is_some_and(|deadline| Instant::now() >= deadline) || self.inner.parent.as_ref().is_some_and(Cancel::timed_out)
    }

    /// Only an explicit stop, not a deadline.
    pub fn user_cancelled(&self) -> bool {
        self.inner.flag.load(Ordering::SeqCst) || self.inner.parent.as_ref().is_some_and(Cancel::user_cancelled)
    }

    pub fn is_cancelled(&self) -> bool {
        self.user_cancelled() || self.timed_out()
    }

    pub fn check(&self) -> Result<()> {
        if self.user_cancelled() {
            Err(Error::cancelled())
        } else if self.timed_out() {
            Err(Error::timeout("The operation timed out."))
        } else {
            Ok(())
        }
    }

    /// Sleep in short slices so a stop request is honoured promptly.
    pub fn sleep(&self, duration: Duration) -> Result<()> {
        let end = Instant::now() + duration;
        loop {
            self.check()?;
            let now = Instant::now();
            if now >= end {
                return Ok(());
            }
            std::thread::sleep(SLICE.min(end - now));
        }
    }
}
