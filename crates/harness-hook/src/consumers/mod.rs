//! The consumer registry (`core/router/consumers.py`), in the same order as the Python router.

pub mod context;
pub mod context_guard;
pub mod herdr_bridge;
pub mod notify;
pub mod obs;
pub mod orb;
pub mod services;
pub mod tasks;
pub mod usage_fallback;

use crate::consumer::Consumer;

pub fn all_consumers() -> Vec<Box<dyn Consumer>> {
    vec![
        Box::new(obs::ObsConsumer),
        Box::new(notify::NotifyConsumer),
        Box::new(tasks::TasksConsumer),
        Box::new(usage_fallback::UsageFallbackConsumer),
        Box::new(context_guard::ContextGuardConsumer),
        Box::new(context::ContextConsumer),
        Box::new(services::ServicesConsumer),
        Box::new(orb::OrbConsumer),
        Box::new(herdr_bridge::HerdrBridgeConsumer),
    ]
}
