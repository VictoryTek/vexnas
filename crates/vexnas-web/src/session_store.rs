//! SQLite-backed `tower-sessions` store (ported from vexboard) so sessions
//! survive restarts.

use std::str::FromStr;

use async_trait::async_trait;
use sqlx::{Row, SqlitePool};
use time::OffsetDateTime;
use tower_sessions::{
    session::{Id, Record},
    session_store::{self, ExpiredDeletion},
    SessionStore,
};

#[derive(Clone, Debug)]
pub struct SqliteSessionStore {
    pool: SqlitePool,
}

impl SqliteSessionStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn migrate(&self) -> Result<(), sqlx::Error> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS tower_sessions (
                id          TEXT PRIMARY KEY NOT NULL,
                data        TEXT NOT NULL,
                expiry_date INTEGER NOT NULL
            )",
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

fn backend(e: sqlx::Error) -> session_store::Error {
    session_store::Error::Backend(e.to_string())
}

fn decode(e: impl ToString) -> session_store::Error {
    session_store::Error::Decode(e.to_string())
}

#[async_trait]
impl SessionStore for SqliteSessionStore {
    async fn save(&self, record: &Record) -> session_store::Result<()> {
        let data = serde_json::to_string(&record.data)
            .map_err(|e| session_store::Error::Encode(e.to_string()))?;
        sqlx::query(
            "INSERT OR REPLACE INTO tower_sessions (id, data, expiry_date) VALUES (?, ?, ?)",
        )
        .bind(record.id.to_string())
        .bind(data)
        .bind(record.expiry_date.unix_timestamp())
        .execute(&self.pool)
        .await
        .map_err(backend)?;
        Ok(())
    }

    async fn load(&self, session_id: &Id) -> session_store::Result<Option<Record>> {
        let row = sqlx::query("SELECT id, data, expiry_date FROM tower_sessions WHERE id = ?")
            .bind(session_id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(backend)?;
        let Some(row) = row else { return Ok(None) };

        let expiry_date =
            OffsetDateTime::from_unix_timestamp(row.try_get("expiry_date").map_err(decode)?)
                .map_err(decode)?;
        if expiry_date <= OffsetDateTime::now_utc() {
            return Ok(None);
        }
        let id = Id::from_str(&row.try_get::<String, _>("id").map_err(decode)?).map_err(decode)?;
        let data = serde_json::from_str(&row.try_get::<String, _>("data").map_err(decode)?)
            .map_err(decode)?;
        Ok(Some(Record {
            id,
            data,
            expiry_date,
        }))
    }

    async fn delete(&self, session_id: &Id) -> session_store::Result<()> {
        sqlx::query("DELETE FROM tower_sessions WHERE id = ?")
            .bind(session_id.to_string())
            .execute(&self.pool)
            .await
            .map_err(backend)?;
        Ok(())
    }
}

#[async_trait]
impl ExpiredDeletion for SqliteSessionStore {
    async fn delete_expired(&self) -> session_store::Result<()> {
        sqlx::query("DELETE FROM tower_sessions WHERE expiry_date <= ?")
            .bind(OffsetDateTime::now_utc().unix_timestamp())
            .execute(&self.pool)
            .await
            .map_err(backend)?;
        Ok(())
    }
}

/// `load()` filters expired rows but never removes them; do that periodically.
pub async fn cleanup_loop(store: SqliteSessionStore, interval: std::time::Duration) {
    loop {
        if let Err(e) = store.delete_expired().await {
            tracing::warn!("failed to delete expired sessions: {e}");
        }
        tokio::time::sleep(interval).await;
    }
}
