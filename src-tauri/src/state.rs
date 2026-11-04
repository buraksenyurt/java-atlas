use std::sync::{Arc, Mutex, MutexGuard};

use crate::project::OpenProject;
use crate::watch::{Batch, Watch};

/// The project currently on screen, and its live-update machinery.
///
/// The `Mutex`es only guard swapping small values; commands clone the `Arc`
/// and release the lock before doing any work, so a slow view never blocks
/// the next command.
pub struct AppState {
    project: Mutex<Option<Arc<OpenProject>>>,
    live: Mutex<Live>,
}

/// Phase 8: the folder watch and what to do with its batches.
pub struct Live {
    /// The user's switch; on by default.
    pub enabled: bool,
    pub watch: Option<Watch>,
    /// Why the last watch could not start.
    pub error: Option<String>,
    /// Changes seen while live updates were switched off, applied when
    /// they are switched on again.
    pub pending: Batch,
}

impl Default for AppState {
    fn default() -> Self {
        AppState {
            project: Mutex::new(None),
            live: Mutex::new(Live {
                enabled: true,
                watch: None,
                error: None,
                pending: Batch::default(),
            }),
        }
    }
}

impl AppState {
    pub fn set(&self, project: OpenProject) -> Arc<OpenProject> {
        let project = Arc::new(project);
        *self.project.lock().expect("state lock poisoned") = Some(Arc::clone(&project));
        project
    }

    /// Replaces the project only if it is still `expected`: a live update
    /// computed for a project the user has since closed must not win.
    pub fn replace(&self, expected: &Arc<OpenProject>, project: OpenProject) -> bool {
        let mut slot = self.project.lock().expect("state lock poisoned");
        match slot.as_ref() {
            Some(current) if Arc::ptr_eq(current, expected) => {
                *slot = Some(Arc::new(project));
                true
            }
            _ => false,
        }
    }

    pub fn get(&self) -> Option<Arc<OpenProject>> {
        self.project.lock().expect("state lock poisoned").clone()
    }

    pub fn live(&self) -> MutexGuard<'_, Live> {
        self.live.lock().expect("live lock poisoned")
    }
}
