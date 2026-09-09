//! Credential state: the master token (keychain), the cached bearer
//! (keychain, short-lived), and the Android id (config, not secret).
//!
//! Keychain layout under `piekstra.ghome`:
//!   `<email>`        → master token (`aas_et/…`)
//!   `<email>/bearer` → `{"token": "ya29…", "expires_at": <unix secs>}`

use pk_cli_core::CliError;
use pk_cli_secrets::{CredentialStore, Secret};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};

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
    /// Load the stored credential for `email`. `Auth` (exit 3) when nothing is
    /// stored, so drivers can branch straight to `auth login`.
    pub fn load(creds: &CredentialStore, email: &str, android_id: &str) -> Result<Self, CliError> {
        let master = creds.get(email)?.ok_or_else(|| {
            CliError::Auth(format!(
                "no Google credential stored for {email}; run `ghome auth login`"
            ))
        })?;
        let bearer = creds
            .get(&bearer_account(email))?
            .and_then(|s| serde_json::from_str::<CachedBearer>(s.expose()).ok());
        Ok(Session {
            email: email.to_string(),
            android_id: android_id.to_string(),
            master,
            bearer,
        })
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
        creds.set(&self.email, &self.master)
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
        let cached = CachedBearer {
            token: minted.auth.clone(),
            expires_at,
        };
        // A failed cache write is not worth failing the command over: the
        // bearer still works for this invocation.
        if let Ok(json) = serde_json::to_string(&cached) {
            let _ = creds.set(&bearer_account(&self.email), &Secret::new(json));
        }
        self.bearer = Some(cached);
        Ok(minted.auth)
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
        creds.delete(&bearer_account(email))?;
        if forget {
            creds.delete(email)?;
        }
        Ok(())
    }
}
