use crate::types::Opportunity;
use anyhow::Result;
use redis::AsyncCommands;

/// Publishes opportunities to a Redis channel for the Sender to consume.
#[derive(Clone)]
pub struct Emitter {
    conn: redis::aio::ConnectionManager,
    channel: String,
}

impl Emitter {
    pub async fn connect(url: &str, channel: String) -> Result<Self> {
        let client = redis::Client::open(url)?;
        let conn = redis::aio::ConnectionManager::new(client).await?;
        Ok(Self { conn, channel })
    }

    pub async fn emit(&self, opp: &Opportunity, db_id: Option<i64>) -> Result<()> {
        let mut json = opp.to_json();
        if let (Some(id), Some(obj)) = (db_id, json.as_object_mut()) {
            obj.insert("db_id".to_string(), serde_json::Value::Number(id.into()));
        }
        self.publish(&self.channel, json).await
    }

    /// Emit a speculative opportunity (Phase 2) to `channel`, tagged with the trigger
    /// tx and a `speculative: true` marker on top of the standard Opportunity JSON.
    pub async fn emit_speculative(
        &self,
        channel: &str,
        opp: &Opportunity,
        trigger_tx: &str,
    ) -> Result<()> {
        let mut json = opp.to_json();
        if let Some(obj) = json.as_object_mut() {
            obj.insert("speculative".to_string(), serde_json::Value::Bool(true));
            obj.insert(
                "trigger_tx".to_string(),
                serde_json::Value::String(trigger_tx.to_string()),
            );
        }
        self.publish(channel, json).await
    }

    async fn publish(&self, channel: &str, json: serde_json::Value) -> Result<()> {
        let payload = json.to_string();
        let mut conn = self.conn.clone();
        let _: () = conn.publish(channel, payload).await?;
        Ok(())
    }
}
