//! The maintenance gate (#196): at most one exclusive data operation runs at
//! a time — a dataset delete, a cache prune, or a database compaction — and
//! while one runs, nothing else starts writing.
//!
//! Imports and classifications are not tracked here. Their own state is the
//! truth: `datasets.import_state = 'importing'` (startup fails any orphaned
//! one, so in a running process it means a live worker) and
//! `datasets.classify_state = 'running'` with the
//! [`ClassifyRegistry`](crate::classify::ClassifyRegistry).
//!
//! Protocol, so an import or classification can't start in the gap before
//! maintenance does (or the reverse):
//! - Maintenance checks for live work and calls [`Activity::begin`] while
//!   holding the read-write connection.
//! - Writers (`import_csv`, `classify_dataset`) call
//!   [`Activity::ensure_idle`] twice: before taking the read-write
//!   connection, so they fail at once instead of queueing behind a long
//!   operation on the main thread, and again while holding it, which is the
//!   check that closes the race.

use std::sync::{Mutex, MutexGuard, PoisonError};

/// The exclusive operation in flight.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Maintenance {
    /// Deleting the dataset with this id.
    DeletingDataset(String),
    PruningCache,
    Compacting,
}

impl Maintenance {
    fn describe(&self) -> &'static str {
        match self {
            Self::DeletingDataset(_) => "deleting a dataset",
            Self::PruningCache => "removing cached classifications",
            Self::Compacting => "compacting the database",
        }
    }
}

/// Managed as Tauri state from startup, beside `ClassifyRegistry`.
#[derive(Default)]
pub(crate) struct Activity {
    slot: Mutex<Option<Maintenance>>,
}

impl Activity {
    /// Poisoning is benign: the slot is a single value that is always
    /// consistent, so recover it rather than wedge every write command.
    fn lock(&self) -> MutexGuard<'_, Option<Maintenance>> {
        self.slot.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// `Err` with a user-facing reason while maintenance is running.
    pub(crate) fn ensure_idle(&self) -> Result<(), String> {
        match &*self.lock() {
            Some(op) => Err(format!(
                "The app is busy {}. Try again when it finishes.",
                op.describe()
            )),
            None => Ok(()),
        }
    }

    /// Claim the slot for `op` until the guard drops. Call while holding the
    /// read-write connection (module docs).
    pub(crate) fn begin(&self, op: Maintenance) -> Result<MaintenanceGuard<'_>, String> {
        let mut slot = self.lock();
        if let Some(current) = &*slot {
            return Err(format!(
                "The app is busy {}. Try again when it finishes.",
                current.describe()
            ));
        }
        *slot = Some(op);
        Ok(MaintenanceGuard { activity: self })
    }

    /// Whether `dataset_id` is being deleted right now. A dataset marked
    /// `deleting` in the database for which this is false was left by a
    /// delete that didn't finish.
    pub(crate) fn is_deleting(&self, dataset_id: &str) -> bool {
        matches!(&*self.lock(), Some(Maintenance::DeletingDataset(id)) if id == dataset_id)
    }
}

/// Releases the maintenance slot on drop.
pub(crate) struct MaintenanceGuard<'a> {
    activity: &'a Activity,
}

impl MaintenanceGuard<'_> {
    /// Never release the slot: for compaction, where nothing may write
    /// between the copy and the relaunch that swaps it in.
    pub(crate) fn hold_until_exit(self) {
        std::mem::forget(self);
    }
}

impl Drop for MaintenanceGuard<'_> {
    fn drop(&mut self) {
        *self.activity.lock() = None;
    }
}

#[cfg(test)]
mod tests {
    use super::{Activity, Maintenance};

    /// One operation at a time; the slot frees when the guard drops; a
    /// dataset counts as "being deleted" only while its own delete holds
    /// the slot.
    #[test]
    fn slot_is_exclusive_and_released_on_drop() -> Result<(), String> {
        let activity = Activity::default();
        activity.ensure_idle()?;

        let guard = activity.begin(Maintenance::DeletingDataset("a".to_owned()))?;
        assert!(activity.is_deleting("a"));
        assert!(!activity.is_deleting("b"));
        assert!(activity.begin(Maintenance::Compacting).is_err());
        let busy = activity.ensure_idle().err().ok_or("idle while busy")?;
        assert!(busy.contains("deleting a dataset"), "{busy}");

        drop(guard);
        assert!(!activity.is_deleting("a"));
        activity.ensure_idle()?;
        drop(activity.begin(Maintenance::PruningCache)?);

        activity.begin(Maintenance::Compacting)?.hold_until_exit();
        assert!(activity.ensure_idle().is_err(), "a held slot was released");
        Ok(())
    }
}
