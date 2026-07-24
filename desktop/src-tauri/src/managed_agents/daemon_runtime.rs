use std::{collections::HashMap, sync::Mutex};

use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::DaemonSchedulerRuntime;

/// App-lifetime state for managed daemon execution and scheduling.
pub(crate) struct DaemonRuntimeState {
    pub cancellations: Mutex<HashMap<Uuid, CancellationToken>>,
    pub scheduler: Mutex<Option<DaemonSchedulerRuntime>>,
}

impl Default for DaemonRuntimeState {
    fn default() -> Self {
        Self {
            cancellations: Mutex::new(HashMap::new()),
            scheduler: Mutex::new(None),
        }
    }
}
