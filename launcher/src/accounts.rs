use crate::paths::Paths;
use crate::util::{atomic_write, now_epoch_secs};
use anyhow::{Context, Result};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use keyring::{Entry, Error as KeyringError};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::OnceLock;

const KEYRING_SERVICE: &str = "velgrinor";
const LEGACY_KEYRING_SERVICE: &str = "shard";
const KEYRING_CHUNK_MAX_LEN: usize = 1000;

/// Global flag to track if keyring is available (checked once per process)
static KEYRING_AVAILABLE: OnceLock<bool> = OnceLock::new();

/// Check if the system keyring is available
fn is_keyring_available() -> bool {
    *KEYRING_AVAILABLE.get_or_init(|| {
        // Try to create a test entry to see if keyring works
        match Entry::new(KEYRING_SERVICE, "test-availability") {
            Ok(entry) => {
                // Try to get (will fail with NoEntry, but that's fine - it means keyring works)
                match entry.get_password() {
                    Ok(_) => true,
                    Err(KeyringError::NoEntry) => true,
                    Err(_) => false,
                }
            }
            Err(_) => false,
        }
    })
}

/// File-based token storage (fallback when keyring is unavailable)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct FileTokenStore {
    #[serde(default)]
    tokens: HashMap<String, StoredTokens>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Accounts {
    #[serde(default)]
    pub active: Option<String>,
    #[serde(default)]
    pub accounts: Vec<Account>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub uuid: String,
    pub username: String,
    #[serde(default)]
    pub kind: AccountKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub xuid: Option<String>,
    #[serde(skip_serializing)]
    pub msa: MsaTokens,
    #[serde(skip_serializing)]
    pub minecraft: MinecraftTokens,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AccountKind {
    #[default]
    Microsoft,
    Offline,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MsaTokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MinecraftTokens {
    pub access_token: String,
    pub expires_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredTokens {
    pub msa: MsaTokens,
    pub minecraft: MinecraftTokens,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct StoredAccounts {
    #[serde(default)]
    pub active: Option<String>,
    #[serde(default)]
    pub accounts: Vec<StoredAccount>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredAccount {
    pub uuid: String,
    pub username: String,
    #[serde(default)]
    pub kind: AccountKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub xuid: Option<String>,
}

impl MsaTokens {
    pub fn is_expired(&self) -> bool {
        now_epoch_secs() + 60 >= self.expires_at
    }
}

impl MinecraftTokens {
    pub fn is_expired(&self) -> bool {
        now_epoch_secs() + 60 >= self.expires_at
    }
}

impl Account {
    pub fn is_offline(&self) -> bool {
        self.kind == AccountKind::Offline
    }
}

pub fn create_offline_account(username: &str) -> Result<Account> {
    let username = username.trim();
    if !(3..=16).contains(&username.len())
        || !username
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        anyhow::bail!("offline username must be 3-16 characters using only letters, numbers, or _");
    }

    let mut bytes = md5::compute(format!("OfflinePlayer:{username}")).0;
    bytes[6] = (bytes[6] & 0x0f) | 0x30;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let uuid = format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    );

    Ok(Account {
        uuid,
        username: username.to_string(),
        kind: AccountKind::Offline,
        auth_client_id: None,
        xuid: None,
        msa: MsaTokens {
            access_token: String::new(),
            refresh_token: String::new(),
            expires_at: u64::MAX,
        },
        minecraft: MinecraftTokens {
            access_token: String::new(),
            expires_at: u64::MAX,
        },
    })
}

fn account_key(uuid: &str) -> String {
    format!("account:{uuid}")
}

fn account_chunk_meta_key(uuid: &str) -> String {
    format!("account:{uuid}:chunks")
}

fn account_chunk_key(uuid: &str, index: usize) -> String {
    format!("account:{uuid}:chunk:{index}")
}

fn keyring_entry(service: &str, name: &str) -> Result<Entry> {
    Entry::new(service, name).with_context(|| format!("failed to open keyring entry: {name}"))
}

// ============================================================================
// File-based token storage (fallback)
// ============================================================================

fn load_file_token_store(paths: &Paths) -> Result<FileTokenStore> {
    if !paths.tokens.exists() {
        return Ok(FileTokenStore::default());
    }
    let data = fs::read_to_string(&paths.tokens)
        .with_context(|| format!("failed to read tokens file: {}", paths.tokens.display()))?;
    serde_json::from_str(&data)
        .with_context(|| format!("failed to parse tokens file: {}", paths.tokens.display()))
}

fn save_file_token_store(paths: &Paths, store: &FileTokenStore) -> Result<()> {
    if let Some(parent) = paths.tokens.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory: {}", parent.display()))?;
    }
    let data = serde_json::to_string_pretty(store).context("failed to serialize tokens")?;
    atomic_write(&paths.tokens, data)
        .with_context(|| format!("failed to write tokens file: {}", paths.tokens.display()))?;

    // Set restrictive permissions on Unix (tokens file contains secrets)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        let _ = fs::set_permissions(&paths.tokens, perms);
    }

    Ok(())
}

fn store_tokens_file(paths: &Paths, uuid: &str, tokens: &StoredTokens) -> Result<()> {
    let mut store = load_file_token_store(paths)?;
    store.tokens.insert(uuid.to_string(), tokens.clone());
    save_file_token_store(paths, &store)
}

fn load_tokens_file(paths: &Paths, uuid: &str) -> Result<StoredTokens> {
    let store = load_file_token_store(paths)?;
    store
        .tokens
        .get(uuid)
        .cloned()
        .with_context(|| format!("no tokens found for account {uuid}"))
}

fn delete_tokens_file(paths: &Paths, uuid: &str) -> Result<()> {
    let mut store = load_file_token_store(paths)?;
    store.tokens.remove(uuid);
    save_file_token_store(paths, &store)
}

// ============================================================================
// Keyring-based token storage (primary)
// ============================================================================

fn store_tokens_keyring(uuid: &str, tokens: &StoredTokens) -> Result<()> {
    delete_tokens_keyring(KEYRING_SERVICE, uuid)?;
    let data = serde_json::to_string(tokens).context("failed to serialize account tokens")?;
    if data.len() <= KEYRING_CHUNK_MAX_LEN {
        let entry = keyring_entry(KEYRING_SERVICE, &account_key(uuid))?;
        entry
            .set_password(&data)
            .with_context(|| format!("failed to store tokens in keyring for account {uuid}"))?;
        return Ok(());
    }

    let encoded = BASE64.encode(data.as_bytes());
    let chunks: Vec<&str> = encoded
        .as_bytes()
        .chunks(KEYRING_CHUNK_MAX_LEN)
        .map(|chunk| std::str::from_utf8(chunk).expect("base64 is always valid ASCII"))
        .collect();

    let chunk_count = chunks.len();
    for (index, chunk) in chunks.into_iter().enumerate() {
        let entry = keyring_entry(KEYRING_SERVICE, &account_chunk_key(uuid, index))?;
        entry
            .set_password(chunk)
            .with_context(|| format!("failed to store token chunk {index} for account {uuid}"))?;
    }

    let meta_entry = keyring_entry(KEYRING_SERVICE, &account_chunk_meta_key(uuid))?;
    meta_entry
        .set_password(&chunk_count.to_string())
        .with_context(|| format!("failed to store token chunk metadata for account {uuid}"))?;
    Ok(())
}

fn load_tokens_keyring(service: &str, uuid: &str) -> Result<Option<StoredTokens>> {
    let entry = keyring_entry(service, &account_key(uuid))?;
    match entry.get_password() {
        Ok(data) => {
            return serde_json::from_str(&data)
                .with_context(|| format!("failed to parse keyring tokens for account {uuid}"))
                .map(Some);
        }
        Err(KeyringError::NoEntry) => {}
        Err(err) => {
            return Err(err)
                .with_context(|| format!("failed to read keyring tokens for account {uuid}"));
        }
    }

    let meta_entry = keyring_entry(service, &account_chunk_meta_key(uuid))?;
    let count = match meta_entry.get_password() {
        Ok(value) => value,
        Err(KeyringError::NoEntry) => return Ok(None),
        Err(err) => {
            return Err(err).with_context(|| {
                format!("failed to read token chunk metadata for account {uuid}")
            });
        }
    };
    let count: usize = count
        .trim()
        .parse()
        .with_context(|| format!("invalid token chunk metadata for account {uuid}"))?;

    let mut encoded = String::new();
    for index in 0..count {
        let entry = keyring_entry(service, &account_chunk_key(uuid, index))?;
        let chunk = entry
            .get_password()
            .with_context(|| format!("missing token chunk {index} for account {uuid}"))?;
        encoded.push_str(&chunk);
    }

    // Decode base64 to get the original JSON
    let decoded_bytes = BASE64
        .decode(&encoded)
        .with_context(|| format!("failed to decode base64 tokens for account {uuid}"))?;
    let data = String::from_utf8(decoded_bytes)
        .with_context(|| format!("invalid UTF-8 in decoded tokens for account {uuid}"))?;

    serde_json::from_str(&data)
        .with_context(|| format!("failed to parse keyring tokens for account {uuid}"))
        .map(Some)
}

fn delete_tokens_keyring(service: &str, id: &str) -> Result<()> {
    let entry = keyring_entry(service, &account_key(id))?;
    match entry.delete_password() {
        Ok(()) => {}
        Err(KeyringError::NoEntry) => {}
        Err(err) => {
            return Err(err)
                .with_context(|| format!("failed to delete keyring tokens for account {id}"));
        }
    }

    let meta_entry = keyring_entry(service, &account_chunk_meta_key(id))?;
    let count = match meta_entry.get_password() {
        Ok(value) => value.trim().parse::<usize>().ok(),
        Err(KeyringError::NoEntry) => None,
        Err(err) => {
            return Err(err)
                .with_context(|| format!("failed to read token chunk metadata for account {id}"));
        }
    };

    if let Some(count) = count {
        for index in 0..count {
            let entry = keyring_entry(service, &account_chunk_key(id, index))?;
            match entry.delete_password() {
                Ok(()) => {}
                Err(KeyringError::NoEntry) => {}
                Err(err) => {
                    return Err(err).with_context(|| {
                        format!("failed to delete token chunk {index} for account {id}")
                    });
                }
            }
        }
        match meta_entry.delete_password() {
            Ok(()) => {}
            Err(KeyringError::NoEntry) => {}
            Err(err) => {
                return Err(err).with_context(|| {
                    format!("failed to delete token chunk metadata for account {id}")
                });
            }
        }
    }
    Ok(())
}

// ============================================================================
// Unified token storage API (uses keyring with file fallback)
// ============================================================================

fn store_tokens(paths: &Paths, uuid: &str, tokens: &StoredTokens) -> Result<()> {
    if is_keyring_available() {
        store_tokens_keyring(uuid, tokens)
    } else {
        store_tokens_file(paths, uuid, tokens)
    }
}

pub fn store_account_tokens(paths: &Paths, account: &Account) -> Result<()> {
    if account.is_offline() {
        return Ok(());
    }
    store_tokens(
        paths,
        &account.uuid,
        &StoredTokens {
            msa: account.msa.clone(),
            minecraft: account.minecraft.clone(),
        },
    )
}

fn load_tokens(paths: &Paths, uuid: &str) -> Result<StoredTokens> {
    if is_keyring_available() {
        load_tokens_keyring(KEYRING_SERVICE, uuid)?
            .or(load_tokens_keyring(LEGACY_KEYRING_SERVICE, uuid)?)
            .with_context(|| format!("no tokens found for account {uuid}"))
    } else {
        load_tokens_file(paths, uuid)
    }
}

pub fn delete_account_tokens(paths: &Paths, id: &str) -> Result<()> {
    if is_keyring_available() {
        delete_tokens_keyring(KEYRING_SERVICE, id)?;
        delete_tokens_keyring(LEGACY_KEYRING_SERVICE, id)
    } else {
        delete_tokens_file(paths, id)
    }
}

fn read_accounts_file(paths: &Paths) -> Result<String> {
    fs::read_to_string(&paths.accounts)
        .with_context(|| format!("failed to read accounts file: {}", paths.accounts.display()))
}

fn write_accounts_file(paths: &Paths, accounts: &StoredAccounts) -> Result<()> {
    if let Some(parent) = Path::new(&paths.accounts).parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory: {}", parent.display()))?;
    }
    let data = serde_json::to_string_pretty(accounts).context("failed to serialize accounts")?;
    atomic_write(&paths.accounts, data).with_context(|| {
        format!(
            "failed to write accounts file: {}",
            paths.accounts.display()
        )
    })?;
    Ok(())
}

fn to_stored_accounts(accounts: &Accounts) -> StoredAccounts {
    StoredAccounts {
        active: accounts.active.clone(),
        accounts: accounts
            .accounts
            .iter()
            .map(|account| StoredAccount {
                uuid: account.uuid.clone(),
                username: account.username.clone(),
                kind: account.kind,
                auth_client_id: account.auth_client_id.clone(),
                xuid: account.xuid.clone(),
            })
            .collect(),
    }
}

pub fn load_accounts(paths: &Paths) -> Result<Accounts> {
    if !paths.accounts.exists() {
        return Ok(Accounts::default());
    }
    let data = read_accounts_file(paths)?;
    let value: serde_json::Value = serde_json::from_str(&data).with_context(|| {
        format!(
            "failed to parse accounts JSON: {}",
            paths.accounts.display()
        )
    })?;

    let has_legacy_tokens = value
        .get("accounts")
        .and_then(|accounts| accounts.as_array())
        .map(|accounts| {
            accounts
                .iter()
                .any(|account| account.get("msa").is_some() || account.get("minecraft").is_some())
        })
        .unwrap_or(false);

    if has_legacy_tokens {
        let legacy: Accounts = serde_json::from_value(value).with_context(|| {
            format!(
                "failed to parse accounts JSON: {}",
                paths.accounts.display()
            )
        })?;

        for account in &legacy.accounts {
            let tokens = StoredTokens {
                msa: account.msa.clone(),
                minecraft: account.minecraft.clone(),
            };
            store_tokens(paths, &account.uuid, &tokens)?;
        }
        let stored = to_stored_accounts(&legacy);
        write_accounts_file(paths, &stored)?;
        return Ok(legacy);
    }

    let stored: StoredAccounts = serde_json::from_value(value).with_context(|| {
        format!(
            "failed to parse accounts JSON: {}",
            paths.accounts.display()
        )
    })?;
    let mut accounts = Accounts {
        active: stored.active,
        accounts: Vec::with_capacity(stored.accounts.len()),
    };
    for account in stored.accounts {
        let tokens = if account.kind == AccountKind::Offline {
            StoredTokens {
                msa: MsaTokens {
                    access_token: String::new(),
                    refresh_token: String::new(),
                    expires_at: u64::MAX,
                },
                minecraft: MinecraftTokens {
                    access_token: String::new(),
                    expires_at: u64::MAX,
                },
            }
        } else {
            load_tokens(paths, &account.uuid)?
        };
        accounts.accounts.push(Account {
            uuid: account.uuid,
            username: account.username,
            kind: account.kind,
            auth_client_id: account.auth_client_id,
            xuid: account.xuid,
            msa: tokens.msa,
            minecraft: tokens.minecraft,
        });
    }
    Ok(accounts)
}

pub fn save_accounts(paths: &Paths, accounts: &Accounts) -> Result<()> {
    for account in &accounts.accounts {
        store_account_tokens(paths, account)?;
    }
    let stored = to_stored_accounts(accounts);
    write_accounts_file(paths, &stored)
}

/// Check if account matches by UUID or username (case-insensitive)
fn matches_account(account: &Account, id: &str, id_lower: &str) -> bool {
    account.uuid == id || account.username.to_lowercase() == *id_lower
}

pub fn find_account_mut<'a>(accounts: &'a mut Accounts, id: &str) -> Option<&'a mut Account> {
    let id_lower = id.to_lowercase();
    accounts
        .accounts
        .iter_mut()
        .find(|account| matches_account(account, id, &id_lower))
}

pub fn upsert_account(accounts: &mut Accounts, account: Account) {
    if let Some(existing) = accounts
        .accounts
        .iter_mut()
        .find(|a| a.uuid == account.uuid)
    {
        *existing = account;
    } else {
        accounts.accounts.push(account);
    }
}

/// Removes accounts matching the given ID (UUID or username) and returns their UUIDs.
/// Returns an empty vector if no accounts were found.
pub fn remove_account(accounts: &mut Accounts, id: &str) -> Vec<String> {
    let id_lower = id.to_lowercase();
    let removed_uuids: Vec<String> = accounts
        .accounts
        .iter()
        .filter(|account| matches_account(account, id, &id_lower))
        .map(|account| account.uuid.clone())
        .collect();

    accounts
        .accounts
        .retain(|account| !removed_uuids.contains(&account.uuid));
    if let Some(active) = accounts.active.as_deref()
        && removed_uuids.iter().any(|uuid| uuid == active)
    {
        accounts.active = None;
    }
    removed_uuids
}

pub fn set_active(accounts: &mut Accounts, id: &str) -> bool {
    let id_lower = id.to_lowercase();
    if let Some(uuid) = accounts
        .accounts
        .iter()
        .find(|account| matches_account(account, id, &id_lower))
        .map(|account| account.uuid.clone())
    {
        accounts.active = Some(uuid);
        return true;
    }
    false
}

/// Portable account format for export/import (includes tokens in the JSON)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportedAccounts {
    #[serde(default)]
    pub active: Option<String>,
    #[serde(default)]
    pub accounts: Vec<ExportedAccount>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportedAccount {
    pub uuid: String,
    pub username: String,
    #[serde(default)]
    pub kind: AccountKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub xuid: Option<String>,
    pub msa: MsaTokens,
    pub minecraft: MinecraftTokens,
}

impl From<&Account> for ExportedAccount {
    fn from(account: &Account) -> Self {
        ExportedAccount {
            uuid: account.uuid.clone(),
            username: account.username.clone(),
            kind: account.kind,
            auth_client_id: account.auth_client_id.clone(),
            xuid: account.xuid.clone(),
            msa: account.msa.clone(),
            minecraft: account.minecraft.clone(),
        }
    }
}

impl From<ExportedAccount> for Account {
    fn from(exported: ExportedAccount) -> Self {
        Account {
            uuid: exported.uuid,
            username: exported.username,
            kind: exported.kind,
            auth_client_id: exported.auth_client_id,
            xuid: exported.xuid,
            msa: exported.msa,
            minecraft: exported.minecraft,
        }
    }
}

/// Export all accounts to a portable JSON format (includes tokens)
pub fn export_accounts(accounts: &Accounts) -> ExportedAccounts {
    ExportedAccounts {
        active: accounts.active.clone(),
        accounts: accounts
            .accounts
            .iter()
            .map(ExportedAccount::from)
            .collect(),
    }
}

/// Export accounts to a JSON file
pub fn export_accounts_to_file(accounts: &Accounts, path: &Path) -> Result<()> {
    let exported = export_accounts(accounts);
    let data = serde_json::to_string_pretty(&exported)
        .context("failed to serialize accounts for export")?;
    fs::write(path, data)
        .with_context(|| format!("failed to write export file: {}", path.display()))?;
    Ok(())
}

/// Import accounts from a portable JSON format
pub fn import_accounts(exported: ExportedAccounts) -> Accounts {
    Accounts {
        active: exported.active,
        accounts: exported.accounts.into_iter().map(Account::from).collect(),
    }
}

/// Import accounts from a JSON file
pub fn import_accounts_from_file(path: &Path) -> Result<Accounts> {
    let data = fs::read_to_string(path)
        .with_context(|| format!("failed to read import file: {}", path.display()))?;
    let exported: ExportedAccounts = serde_json::from_str(&data)
        .with_context(|| format!("failed to parse import file: {}", path.display()))?;
    Ok(import_accounts(exported))
}

/// Merge imported accounts into existing accounts, optionally replacing duplicates
pub fn merge_accounts(existing: &mut Accounts, imported: Accounts, replace: bool) -> usize {
    let mut count = 0;
    for account in imported.accounts {
        if let Some(existing_account) = existing
            .accounts
            .iter_mut()
            .find(|a| a.uuid == account.uuid)
        {
            if replace {
                *existing_account = account;
                count += 1;
            }
        } else {
            existing.accounts.push(account);
            count += 1;
        }
    }
    // Update active if not set and imported has one
    if existing.active.is_none() {
        existing.active = imported.active;
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_standard_offline_account() {
        let account = create_offline_account("Notch").unwrap();

        assert_eq!(account.uuid, "b50ad385-829d-3141-a216-7e7d7539ba7f");
        assert_eq!(account.kind, AccountKind::Offline);
        assert!(account.is_offline());
        assert!(account.minecraft.access_token.is_empty());
    }

    #[test]
    fn validates_offline_username() {
        assert!(create_offline_account("ab").is_err());
        assert!(create_offline_account("invalid name").is_err());
        assert!(create_offline_account("Valid_Name_123").is_ok());
    }

    #[test]
    fn defaults_missing_account_kind_to_microsoft() {
        let stored: StoredAccount =
            serde_json::from_str(r#"{"uuid":"id","username":"Player","xuid":null}"#).unwrap();

        assert_eq!(stored.kind, AccountKind::Microsoft);
    }
}
