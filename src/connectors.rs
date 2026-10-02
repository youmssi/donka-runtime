//! Donka connector nodes (`donka.connector`): decisions call outside services
//! through `donka-connectors`, with secret values read from `DONKA_SECRET_*`.
//! One handler serves the whole process, so a service's circuit breaker
//! survives release reloads. Other custom node kinds keep upstream behaviour.

use crate::config::ConnectorsConfig;
use donka_connectors::{ConnectorAdapter, EnvSecrets, KIND, Limits};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use zen_engine::nodes::NodeResult;
use zen_engine::nodes::custom::{
    CustomNodeAdapter, CustomNodeRequest, DynamicCustomNode, NoopCustomNode,
};

static ADAPTER: OnceLock<DynamicCustomNode> = OnceLock::new();

#[derive(Debug)]
struct Nodes {
    connectors: ConnectorAdapter,
}

impl CustomNodeAdapter for Nodes {
    fn handle(&self, request: CustomNodeRequest) -> Pin<Box<dyn Future<Output = NodeResult> + '_>> {
        if request.node.kind.as_ref() == KIND {
            self.connectors.handle(request)
        } else {
            NoopCustomNode.handle(request)
        }
    }
}

/// Installs the process-wide handler. Runs at startup, before any release is loaded.
pub fn init(config: &ConnectorsConfig) {
    ADAPTER.get_or_init(|| handler(config));
}

/// The handler engines are built with.
pub fn adapter() -> DynamicCustomNode {
    ADAPTER
        .get_or_init(|| handler(&ConnectorsConfig::default()))
        .clone()
}

fn handler(config: &ConnectorsConfig) -> DynamicCustomNode {
    let limits = Limits {
        default_timeout: config.timeout,
        max_timeout: config.max_timeout,
        default_retries: config.retries,
        max_retries: config.max_retries,
        breaker_failures: config.breaker_failures,
        breaker_cooldown: config.breaker_cooldown,
    };
    Arc::new(Nodes {
        connectors: ConnectorAdapter::live(limits, Arc::new(EnvSecrets)),
    })
}
