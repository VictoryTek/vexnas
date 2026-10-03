//! SQLite: sessions (via `session_store`) and the audit log.

use std::net::IpAddr;
use std::path::Path;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::SqlitePool;

pub async fn connect(path: &Path) -> Result<SqlitePool, sqlx::Error> {
    let opts = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .foreign_keys(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(opts)
        .await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS audit (
            id     INTEGER PRIMARY KEY AUTOINCREMENT,
            ts     INTEGER NOT NULL,
            user   TEXT,
            ip     TEXT NOT NULL,
            action TEXT NOT NULL,
            detail TEXT
        )",
    )
    .execute(&pool)
    .await?;
    Ok(pool)
}

/// Append an audit record (and mirror it to the journal). Never records secrets.
/// A failed insert is logged, not propagated: auditing must not take the service down.
pub async fn audit(
    pool: &SqlitePool,
    user: Option<&str>,
    ip: IpAddr,
    action: &str,
    detail: Option<&str>,
) {
    tracing::info!(target: "audit", user = user.unwrap_or("-"), %ip, action, detail = detail.unwrap_or(""));
    let res =
        sqlx::query("INSERT INTO audit (ts, user, ip, action, detail) VALUES (?, ?, ?, ?, ?)")
            .bind(time::OffsetDateTime::now_utc().unix_timestamp())
            .bind(user)
            .bind(ip.to_string())
            .bind(action)
            .bind(detail)
            .execute(pool)
            .await;
    if let Err(e) = res {
        tracing::error!("audit insert failed: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn audit_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let pool = connect(&dir.path().join("t.db")).await.unwrap();
        audit(
            &pool,
            Some("alice"),
            "10.0.0.1".parse().unwrap(),
            "login_ok",
            None,
        )
        .await;
        let (user, action): (String, String) = sqlx::query_as("SELECT user, action FROM audit")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!((user.as_str(), action.as_str()), ("alice", "login_ok"));
    }
}
