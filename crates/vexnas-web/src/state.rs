use std::sync::Arc;

use sqlx::SqlitePool;

use crate::config::Config;
use crate::helper::Helper;
use crate::rate_limit::LoginRateLimiter;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub db: SqlitePool,
    pub helper: Helper,
    pub login_limiter: Arc<LoginRateLimiter>,
    pub hostname: Arc<str>,
}
