//! Cooperative cancellation for export jobs.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
    /// `Err(Cancelled)` if cancelled; for use with `?` inside exporters.
    pub fn check(&self) -> Result<(), veskra_core::VeskraError> {
        if self.is_cancelled() {
            Err(veskra_core::VeskraError::new(veskra_core::ErrorCode::Cancelled, "export cancelled"))
        } else {
            Ok(())
        }
    }
}
