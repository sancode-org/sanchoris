//! The Plans of `infra/plans.json` (`README.md`「課金と Plan」).

use serde::Deserialize;

pub const PLANS_JSON: &str = include_str!("../../../infra/plans.json");

/// The limits of one thread of a Plan.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ThreadLimits {
    pub turns: u64,
    pub cost_usd: f64,
    pub timeout_seconds: u64,
    pub heartbeat_seconds: u64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub id: String,
    pub name: String,
    /// `None` for Admin, which is not sold.
    pub price_usd_cents: Option<u64>,
    /// The environment variable of the Stripe price ID.
    pub stripe_price_env: Option<String>,
    /// `None` has no limit.
    pub threads_per_period: Option<u64>,
    pub workspace_seconds_per_period: Option<u64>,
    pub max_concurrency: u64,
    pub workspace_vcpu: u64,
    pub workspace_memory_mib: u64,
    pub ephemeral_storage_gib: u64,
    pub inbound_events_per_minute: u64,
    pub thread_limits: ThreadLimits,
    pub retention_days: u64,
    pub audit_retention_days: u64,
}

impl Plan {
    pub fn is_admin(&self) -> bool {
        self.id == "admin"
    }
}

#[derive(Deserialize)]
struct Catalog {
    plans: Vec<Plan>,
}

/// Every Plan.
pub fn plans() -> Vec<Plan> {
    serde_json::from_str::<Catalog>(PLANS_JSON)
        .expect("infra/plans.json is a catalog of plans")
        .plans
}

/// The Plan with the ID.
pub fn plan(id: &str) -> Option<Plan> {
    plans().into_iter().find(|p| p.id == id)
}
