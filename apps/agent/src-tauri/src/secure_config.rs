//! Encrypted local planner state. Linux stores an AES-256-GCM master key in
//! Secret Service; SQLite contains authenticated ciphertext, never the key.
//! If the desktop keyring is unavailable or locked, persistence fails closed.

use aes_gcm::{
    aead::{rand_core::RngCore, Aead, OsRng},
    Aes256Gcm, KeyInit, Nonce,
};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use rusqlite::{params, Connection, OptionalExtension};
use std::sync::Mutex;

const PREFIX: &str = "secret-service:aes256gcm:v1:";
static KEY_LOCK: Mutex<()> = Mutex::new(());

#[cfg(not(test))]
const SERVICE: &str = "FlowSight.Agent.Local";
#[cfg(test)]
const SERVICE: &str = "FlowSight.Agent.NativeTest";
const ACCOUNT: &str = "local-state-aes256-v1";

fn master_key(create: bool) -> Result<[u8; 32], String> {
    let _guard = KEY_LOCK
        .lock()
        .map_err(|_| "Desktop keyring lock failed.")?;
    let entry = keyring::Entry::new(SERVICE, ACCOUNT)
        .map_err(|_| "Could not open the desktop keyring. Unlock Secret Service and retry.")?;
    let encoded = match entry.get_password() {
        Ok(value) => value,
        Err(keyring::Error::NoEntry) if create => {
            let mut key = [0u8; 32];
            OsRng.fill_bytes(&mut key);
            let value = BASE64.encode(key);
            entry.set_password(&value).map_err(|_| {
                "Could not save the local encryption key. Unlock Secret Service and retry."
            })?;
            value
        }
        Err(_) => {
            return Err(
                "The local encryption key is unavailable. Unlock Secret Service and retry.".into(),
            )
        }
    };
    BASE64
        .decode(encoded)
        .map_err(|_| "The local encryption key is invalid.".to_string())?
        .try_into()
        .map_err(|_| "The local encryption key is invalid.".into())
}

fn encrypt(key: &[u8; 32], plain: &[u8]) -> Result<Vec<u8>, String> {
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| "Invalid encryption key.")?;
    let mut nonce = [0u8; 12];
    OsRng.fill_bytes(&mut nonce);
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce), plain)
        .map_err(|_| "Could not encrypt local state.".to_string())?;
    Ok([nonce.as_slice(), ciphertext.as_slice()].concat())
}

fn decrypt(key: &[u8; 32], stored: &[u8]) -> Result<Vec<u8>, String> {
    if stored.len() < 28 {
        return Err("Stored local state is corrupt.".into());
    }
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| "Invalid encryption key.")?;
    cipher
        .decrypt(Nonce::from_slice(&stored[..12]), &stored[12..])
        .map_err(|_| "Stored local state could not be authenticated.".into())
}

pub fn save_secret(conn: &Connection, key: &str, value: &str) -> Result<(), String> {
    let protected = encrypt(&master_key(true)?, value.as_bytes())?;
    conn.execute(
        "INSERT OR REPLACE INTO config (key, value) VALUES (?1, ?2)",
        params![key, format!("{PREFIX}{}", BASE64.encode(protected))],
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

pub fn load_secret(conn: &Connection, key: &str) -> Result<Option<String>, String> {
    let stored: Option<String> = conn
        .query_row(
            "SELECT value FROM config WHERE key = ?1",
            params![key],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let Some(stored) = stored else {
        return Ok(None);
    };
    if let Some(encoded) = stored.strip_prefix(PREFIX) {
        let ciphertext = BASE64
            .decode(encoded)
            .map_err(|_| "Stored local state is corrupt.".to_string())?;
        return String::from_utf8(decrypt(&master_key(false)?, &ciphertext)?)
            .map(Some)
            .map_err(|_| "Stored local state is not valid UTF-8.".into());
    }
    // Migrate only after authenticated persistence has succeeded.
    save_secret(conn, key, &stored)?;
    Ok(Some(stored))
}

/// Delete a credential without reading or replacing the desktop master key.
pub fn delete_secret(conn: &Connection, key: &str) -> Result<(), String> {
    conn.execute("DELETE FROM config WHERE key=?1", params![key])
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authenticated_state_roundtrip_uses_fresh_nonces() {
        let key = [17u8; 32];
        let first = encrypt(&key, b"private planner state").unwrap();
        let second = encrypt(&key, b"private planner state").unwrap();
        assert_ne!(first, second);
        assert_eq!(decrypt(&key, &first).unwrap(), b"private planner state");
        let mut corrupt = first;
        corrupt[13] ^= 1;
        assert!(decrypt(&key, &corrupt).is_err());
        assert!(decrypt(&key, &[0; 12]).is_err());
    }

    #[test]
    #[ignore = "requires an unlocked native Secret Service; run in Ubuntu desktop-keyring CI"]
    fn native_secret_service_roundtrip() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE config (key TEXT PRIMARY KEY, value TEXT)", [])
            .unwrap();
        save_secret(&conn, "session", "synthetic-private-state").unwrap();
        let raw: String = conn
            .query_row("SELECT value FROM config WHERE key='session'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert!(raw.starts_with(PREFIX));
        assert!(!raw.contains("synthetic-private-state"));
        assert_eq!(
            load_secret(&conn, "session").unwrap().as_deref(),
            Some("synthetic-private-state")
        );
        keyring::Entry::new(SERVICE, ACCOUNT)
            .unwrap()
            .delete_credential()
            .unwrap();
    }
}
