mod bridge;
mod client;
mod config;
mod executor;
pub mod model;
mod snapshot;

use std::fmt;

pub use bridge::RelayBridge;
pub use client::{HttpRelayClient, RelayTransport};
pub use config::{configure_registry, load_relay_config};
pub use executor::{DesktopRelayExecutor, RelayCommandExecutor, RelayExecutionResult};
pub use snapshot::{remote_snapshot, SnapshotSource};

#[derive(Debug)]
pub enum RelayError {
    Config(String),
    Http(String),
    Contract(String),
    Store(crate::orchestration::store::OrchestrationError),
}

impl fmt::Display for RelayError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(message) => write!(formatter, "relay configuration: {message}"),
            Self::Http(message) => write!(formatter, "relay transport: {message}"),
            Self::Contract(message) => write!(formatter, "relay command: {message}"),
            Self::Store(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for RelayError {}

impl From<crate::orchestration::store::OrchestrationError> for RelayError {
    fn from(value: crate::orchestration::store::OrchestrationError) -> Self {
        Self::Store(value)
    }
}
