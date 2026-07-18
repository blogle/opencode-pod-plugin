pub mod api;
pub mod auth;
pub mod central;
pub mod checkpoint;
pub mod config;
pub mod k8s;
pub mod lifecycle;
pub mod preview;
pub mod state;

use std::sync::Arc;

use auth::Authenticator;
use central::CentralClient;
use checkpoint::CheckpointStorage;
use config::Config;
use k8s::Orchestrator;
use state::Store;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub store: Store,
    pub k8s: Arc<dyn Orchestrator>,
    pub operations: Arc<tokio::sync::Mutex<()>>,
    pub checkpoints: CheckpointStorage,
    pub central: CentralClient,
    pub auth: Authenticator,
}
