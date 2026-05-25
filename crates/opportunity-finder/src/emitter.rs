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
        let payload = json.to_string();
        let mut conn = self.conn.clone();
        let _: () = conn.publish(&self.channel, payload).await?;
        Ok(())
    }
}
