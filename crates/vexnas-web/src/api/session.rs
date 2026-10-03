//! The authenticated-session record and the role derived from group membership.

use serde::{Deserialize, Serialize};

use crate::config::AuthConfig;

pub const SESSION_KEY: &str = "auth";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Admin,
    Viewer,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthSession {
    pub username: String,
    pub role: Role,
    /// Per-session synchroniser token; required as `X-CSRF-Token` on writes.
    pub csrf: String,
    pub created_at: i64,
    pub groups_checked_at: i64,
}

/// Admin if in the admin group; viewer if a viewer group is configured and the
/// user is in it; otherwise no access at all (there is no implicit first-user admin).
pub fn role_for(groups: &[String], cfg: &AuthConfig) -> Option<Role> {
    if groups.contains(&cfg.admin_group) {
        Some(Role::Admin)
    } else if cfg
        .viewer_group
        .as_ref()
        .is_some_and(|v| groups.iter().any(|g| g == v))
    {
        Some(Role::Viewer)
    } else {
        None
    }
}

pub fn now_ts() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

pub fn new_csrf_token() -> String {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    let mut buf = [0u8; 32];
    getrandom::fill(&mut buf).expect("OS randomness available");
    URL_SAFE_NO_PAD.encode(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn groups(g: &[&str]) -> Vec<String> {
        g.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn role_mapping() {
        let mut cfg = AuthConfig::default();
        assert_eq!(
            role_for(&groups(&["users", "wheel"]), &cfg),
            Some(Role::Admin)
        );
        assert_eq!(role_for(&groups(&["users"]), &cfg), None);

        cfg.viewer_group = Some("nas-view".into());
        assert_eq!(role_for(&groups(&["nas-view"]), &cfg), Some(Role::Viewer));
        // admin wins when both
        assert_eq!(
            role_for(&groups(&["nas-view", "wheel"]), &cfg),
            Some(Role::Admin)
        );
    }

    #[test]
    fn csrf_tokens_are_unique_and_long() {
        let (a, b) = (new_csrf_token(), new_csrf_token());
        assert_ne!(a, b);
        assert!(a.len() >= 43);
    }
}
