//! Credential state: one keychain item per account holding the master
//! token and the cached short-lived bearer, plus the Android id (config,
//! not secret). One item, because every item a fresh binary reads is a
//! macOS permission prompt.
//!
//! Keychain layout under `piekstra.ghome`:
//!   `<email>` → `{"master": "aas_et/…", "bearer": "ya29…", "expires_at": <unix secs>}`
//! (older installs kept the bare master token there and the bearer under
//! `<email>/bearer`; both shapes are read and migrated on first use.)

use pk_cli_auth::refresh_lock::RefreshLock;
use pk_cli_core::CliError;
use pk_cli_secrets::{CredentialStore, Secret};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::gpsoauth;

/// Treat a bearer expiring within this window as already gone, so a command
/// doesn't start work that 401s partway through.
const SKEW_SECS: u64 = 60;
/// Google usually reports `Expiry`; when it doesn't, assume the documented
/// one-hour lifetime minus a margin.
const FALLBACK_TTL_SECS: u64 = 55 * 60;

#[derive(Debug, Serialize, Deserialize)]
struct CachedBearer {
    token: String,
    expires_at: u64,
}

/// The single keychain item.
#[derive(Debug, Serialize, Deserialize)]
struct Stored {
    master: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    bearer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expires_at: Option<u64>,
}

/// Whether a stored item, in either layout, is this master token's.
fn holds_master(raw: &str, master: &str) -> bool {
    match serde_json::from_str::<Stored>(raw) {
        Ok(stored) => stored.master == master,
        Err(_) => raw == master,
    }
}

pub struct Session {
    pub email: String,
    pub android_id: String,
    master: Secret,
    bearer: Option<CachedBearer>,
}

pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn bearer_account(email: &str) -> String {
    format!("{email}/bearer")
}

impl Session {
    /// The stored session of the configured account. `Auth` (exit 3), before the keychain, when
    /// no account is configured.
    pub fn for_config(creds: &CredentialStore, cfg: &Config) -> Result<Self, CliError> {
        let email = cfg.username.as_deref().ok_or_else(|| {
            CliError::Auth("no Google account configured; run `ghome auth login`".into())
        })?;
        let android_id = cfg.android_id.as_deref().ok_or_else(|| {
            CliError::Auth("no device identity stored; run `ghome auth login`".into())
        })?;
        Self::load(creds, email, android_id)
    }

    /// Load the stored credential for `email`. `Auth` (exit 3) when nothing is
    /// stored, so drivers can branch straight to `auth login`.
    pub fn load(creds: &CredentialStore, email: &str, android_id: &str) -> Result<Self, CliError> {
        let raw = creds.get(email)?.ok_or_else(|| {
            CliError::Auth(format!(
                "no Google credential stored for {email}; run `ghome auth login`"
            ))
        })?;
        let (master, bearer) = match serde_json::from_str::<Stored>(raw.expose()) {
            Ok(st) => (
                Secret::new(st.master),
                match (st.bearer, st.expires_at) {
                    (Some(token), Some(expires_at)) => Some(CachedBearer { token, expires_at }),
                    _ => None,
                },
            ),
            // Legacy layout: bare master token here, bearer in its own item.
            // Migrate to the single item and drop the second one. A server runs this too, so the
            // write takes the guard a mint does, and a logout during that read stays a logout.
            Err(_) => {
                let master = Secret::new(raw.expose().to_string());
                let bearer = creds
                    .get(&bearer_account(email))?
                    .and_then(|s| serde_json::from_str::<CachedBearer>(s.expose()).ok());
                let s = Session {
                    email: email.to_string(),
                    android_id: android_id.to_string(),
                    master,
                    bearer,
                };
                s.cache_bearer(creds)?;
                let _ = creds.delete(&bearer_account(email));
                return Ok(s);
            }
        };
        Ok(Session {
            email: email.to_string(),
            android_id: android_id.to_string(),
            master,
            bearer,
        })
    }

    fn persist(&self, creds: &CredentialStore) -> Result<(), CliError> {
        let st = Stored {
            master: self.master.expose().to_string(),
            bearer: self.bearer.as_ref().map(|b| b.token.clone()),
            expires_at: self.bearer.as_ref().map(|b| b.expires_at),
        };
        let blob = serde_json::to_string(&st)
            .map_err(|e| CliError::Other(format!("serializing credential: {e}")))?;
        creds.set(&self.email, &Secret::new(blob))
    }

    /// Build a session from a freshly exchanged master token (login path).
    pub fn fresh(email: &str, android_id: &str, master: Secret) -> Self {
        Session {
            email: email.to_string(),
            android_id: android_id.to_string(),
            master,
            bearer: None,
        }
    }

    pub fn persist_master(&self, creds: &CredentialStore) -> Result<(), CliError> {
        self.persist(creds)
    }

    /// When the cached bearer stops being valid, if one is cached.
    pub fn bearer_expires_at(&self) -> Option<u64> {
        self.bearer.as_ref().map(|b| b.expires_at)
    }

    pub fn bearer_is_valid(&self) -> bool {
        matches!(&self.bearer, Some(b) if now_unix() + SKEW_SECS < b.expires_at)
    }

    /// A usable bearer: the cached one while it lasts, else a freshly minted
    /// one, written back to the keychain so the next invocation skips the mint.
    pub fn bearer(&mut self, client: &Client, creds: &CredentialStore) -> Result<String, CliError> {
        if self.bearer_is_valid() {
            if let Some(b) = &self.bearer {
                return Ok(b.token.clone());
            }
        }
        self.mint(client, creds)
    }

    /// Drop the cached bearer (after a 401/403) and mint a new one.
    pub fn refresh_bearer(
        &mut self,
        client: &Client,
        creds: &CredentialStore,
    ) -> Result<String, CliError> {
        self.bearer = None;
        self.mint(client, creds)
    }

    fn mint(&mut self, client: &Client, creds: &CredentialStore) -> Result<String, CliError> {
        let now = now_unix();
        let minted = gpsoauth::get_auth_token(
            client,
            self.master.expose(),
            &self.android_id,
            gpsoauth::HOMEGRAPH_SCOPE,
            &self.email,
        )?;
        let cap = now + FALLBACK_TTL_SECS;
        let expires_at = match minted.expiry {
            Some(e) if e > now => e.min(cap),
            _ => cap,
        };
        self.bearer = Some(CachedBearer {
            token: minted.auth.clone(),
            expires_at,
        });
        // A failed cache write is not worth failing the command over: the
        // bearer still works for this invocation.
        let _ = self.cache_bearer(creds);
        Ok(minted.auth)
    }

    /// Write the new bearer back, if the item still holds this session's master token. The write
    /// carries that token, and a server can be minting while `auth logout --forget` or `auth login
    /// --overwrite` runs, so this holds the lock they take and reads first: a credential the user
    /// removed or replaced must not come back. A login has no item yet while it verifies, and
    /// stores one itself.
    fn cache_bearer(&self, creds: &CredentialStore) -> Result<(), CliError> {
        RefreshLock::for_binary(crate::BIN)?.renew(
            || creds.get(&self.email),
            |stored| {
                stored
                    .as_ref()
                    .is_some_and(|raw| holds_master(raw.expose(), self.master.expose()))
            },
            |stored| {
                self.persist(creds)?;
                Ok(stored)
            },
        )?;
        Ok(())
    }

    /// A session holding a bearer that is good for an hour, so a test reaches no keychain.
    #[cfg(test)]
    pub fn with_bearer(email: &str, android_id: &str, token: &str) -> Self {
        let mut session = Self::fresh(email, android_id, Secret::new("dummy-master"));
        session.bearer = Some(CachedBearer {
            token: token.to_string(),
            expires_at: now_unix() + 3600,
        });
        session
    }

    /// A Bearer for a scope other than the home-graph one (not cached; the
    /// mesh session wants `home.platform.selected.devices`).
    pub fn bearer_for_scope(&self, client: &Client, scope: &str) -> Result<String, CliError> {
        Ok(gpsoauth::get_auth_token(
            client,
            self.master.expose(),
            &self.android_id,
            scope,
            &self.email,
        )?
        .auth)
    }

    /// Remove the cached bearer; with `forget`, the master token too.
    pub fn logout(creds: &CredentialStore, email: &str, forget: bool) -> Result<(), CliError> {
        let _ = creds.delete(&bearer_account(email));
        if forget {
            creds.delete(email)?;
        } else if let Some(raw) = creds.get(email)? {
            if let Ok(mut st) = serde_json::from_str::<Stored>(raw.expose()) {
                st.bearer = None;
                st.expires_at = None;
                if let Ok(blob) = serde_json::to_string(&st) {
                    creds.set(email, &Secret::new(blob))?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bearer_is_cached_only_over_the_master_token_it_came_from() {
        let item = r#"{"master":"dummy-master","bearer":"dummy-bearer","expires_at":1}"#;
        assert!(holds_master(item, "dummy-master"));
        assert!(!holds_master(item, "dummy-replaced"));
        assert!(
            holds_master("dummy-master", "dummy-master"),
            "the legacy layout"
        );
        assert!(!holds_master("dummy-replaced", "dummy-master"));
    }
}
