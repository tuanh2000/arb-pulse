use serde::Deserialize;

/// Opportunity as emitted by the opportunity-finder on the Redis channel.
/// Only the fields the broadcaster needs are decoded; the rest are ignored.
#[derive(Debug, Clone, Deserialize)]
pub struct Opportunity {
    pub token_in: String,
    pub amount_in: String,
    #[serde(default)]
    pub net_profit_token_in: f64,
    #[serde(default)]
    pub block: u64,
    pub hops: Vec<OppHop>,
    /// Database row id from the opportunities table, inserted by the finder.
    /// Present only when the finder has DB persistence enabled.
    #[serde(default)]
    pub db_id: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OppHop {
    pub pool: String,
    pub token_in: String,
    pub fee_bps: u32,
}
