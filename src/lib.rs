//! Freebuff account management without token extraction, copying, or rewriting.
//! Each account is launched with a distinct FREEBUFF_CONFIG_DIR, which is also
//! the path Freebuff uses to namespace its OS-keychain token.

pub mod ui;

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Local, SecondsFormat, Utc};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::env;
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use std::time::{SystemTime, UNIX_EPOCH};
use tempfile::NamedTempFile;
use uuid::Uuid;

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const MAX_CREDENTIALS_BYTES: u64 = 2 * 1024 * 1024;

/// The registry stores an epoch timestamp, never a formatted local time.
/// This makes the date stable across timezone and locale changes.
fn registered_datetime(timestamp: u64) -> Option<DateTime<Utc>> {
    if timestamp == 0 { return None; }
    let seconds = i64::try_from(timestamp).ok()?;
    DateTime::<Utc>::from_timestamp(seconds, 0)
}

/// RFC 3339 timestamp for machine-readable JSON (or None for legacy records
/// whose registration timestamp was never stored).
pub fn added_at_iso(timestamp: u64) -> Option<String> {
    registered_datetime(timestamp)
        .map(|dt| dt.to_rfc3339_opts(SecondsFormat::Secs, true))
}

/// Short date for the terminal table, shown in the user's current local time.
pub fn added_at_date(timestamp: u64) -> String {
    registered_datetime(timestamp)
        .map(|dt| dt.with_timezone(&Local).format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "unknown".to_owned())
}

/// Full local date/time plus UTC offset for registration messages and details.
pub fn added_at_local(timestamp: u64) -> String {
    registered_datetime(timestamp)
        .map(|dt| dt.with_timezone(&Local).format("%Y-%m-%d %H:%M %:z").to_string())
        .unwrap_or_else(|| "unknown".to_owned())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Account {
    pub key: String,
    pub user_id: String,
    pub email: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    pub config_dir: PathBuf,
    #[serde(default)]
    pub original: bool,
    /// First registration time; kept unchanged when re-adding or switching.
    #[serde(default)]
    pub created_at_unix: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct State {
    pub version: u32,
    #[serde(default)]
    pub accounts: Vec<Account>,
    #[serde(default)]
    pub active: Option<String>,
    #[serde(default)]
    pub previous: Option<String>,
    #[serde(default)]
    pub original_discovered: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            version: 1,
            accounts: Vec::new(),
            active: None,
            previous: None,
            original_discovered: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Profile {
    pub id: String,
    pub email: String,
    pub name: String,
}

pub fn get_keychain_token(cred_path: &Path) -> Option<String> {
    let output = Command::new("secret-tool")
        .args(["lookup", "service", "freebuff-cli", "account"])
        .arg(cred_path.as_os_str())
        .output()
        .ok()?;
    if output.status.success() {
        let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !token.is_empty() {
            return Some(token);
        }
    }
    None
}

pub fn set_keychain_token(cred_path: &Path, token: &str) -> Result<()> {
    let mut child = Command::new("secret-tool")
        .args([
            "store",
            &format!("--label=freebuff-cli/{}", cred_path.display()),
            "service",
            "freebuff-cli",
            "account",
        ])
        .arg(cred_path.as_os_str())
        .args(["xdg:schema", "com.oven-sh.bun.Secret"])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .context("spawning secret-tool")?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(token.as_bytes());
    }
    let status = child.wait()?;
    if !status.success() {
        bail!("secret-tool exited with status {}", status);
    }
    Ok(())
}

pub fn get_profile_token(cred_path: &Path) -> Option<String> {
    if let Some(token) = get_keychain_token(cred_path) {
        return Some(token);
    }
    if let Ok(content) = fs::read_to_string(cred_path) {
        if let Ok(val) = serde_json::from_str::<Value>(&content) {
            if let Some(token) = val.get("default").and_then(|d| d.get("authToken")).and_then(Value::as_str) {
                if !token.is_empty() {
                    return Some(token.to_string());
                }
            }
        }
    }
    None
}

fn write_secure_file(path: &Path, content: &[u8]) -> Result<()> {
    let parent = path.parent().context("No parent directory")?;
    private_dir(parent)?;
    let mut file = NamedTempFile::new_in(parent)?;
    #[cfg(unix)]
    file.as_file().set_permissions(fs::Permissions::from_mode(0o600))?;
    file.write_all(content)?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|e| e.error)?;
    Ok(())
}

#[derive(Debug, Clone)]
pub struct Manager {
    pub root: PathBuf,
    pub original_dir: PathBuf,
}

pub fn home_dir() -> Result<PathBuf> {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .context("HOME (or USERPROFILE) must be set to an absolute directory")
}

pub fn default_original_dir() -> Result<PathBuf> {
    match env::var_os("FREEBUFF_CONFIG_DIR") {
        Some(path) if !path.is_empty() => {
            let path = PathBuf::from(path);
            if !path.is_absolute() {
                bail!("FREEBUFF_CONFIG_DIR must be an absolute path");
            }
            Ok(path)
        }
        _ => Ok(home_dir()?.join(".config").join("manicode")),
    }
}

impl Manager {
    pub fn from_env() -> Result<Self> {
        let root = match env::var_os("FREEBUFF_AUTH_HOME") {
            Some(path) if !path.is_empty() => PathBuf::from(path),
            _ => home_dir()?.join(".config").join("freebuff-auth"),
        };
        if !root.is_absolute() {
            bail!("FREEBUFF_AUTH_HOME must be an absolute path");
        }
        Ok(Self { root, original_dir: default_original_dir()? })
    }

    pub fn new(root: PathBuf, original_dir: PathBuf) -> Result<Self> {
        if !root.is_absolute() || !original_dir.is_absolute() {
            bail!("Account manager and Freebuff config paths must be absolute");
        }
        Ok(Self { root, original_dir })
    }

    pub fn registry_path(&self) -> PathBuf {
        self.root.join("accounts.json")
    }

    fn ensure_root(&self) -> Result<()> {
        private_dir(&self.root)?;
        private_dir(&self.root.join("profiles"))?;
        Ok(())
    }

    /// Hold a per-store exclusive lock for the complete read/modify/write cycle.
    /// No user login or Freebuff subprocess is run while holding the lock.
    fn edit<T>(&self, op: impl FnOnce(&mut State) -> Result<T>) -> Result<T> {
        self.ensure_root()?;
        let lock_path = self.root.join(".accounts.lock");
        let mut opts = OpenOptions::new();
        opts.read(true).write(true).create(true);
        #[cfg(unix)]
        opts.mode(0o600);
        let lock = opts.open(&lock_path).context("opening account registry lock")?;
        lock.lock_exclusive().context("locking account registry")?;
        let outcome = (|| {
            let mut state = read_state(&self.registry_path())?;
            if state.version != 1 {
                bail!("Unsupported account registry version {}", state.version);
            }
            for account in &mut state.accounts {
                if account.config_dir == self.original_dir {
                    let dir = self.root.join("profiles").join(&account.key);
                    let _ = private_dir(&dir);
                    let _ = fs::copy(self.original_dir.join("credentials.json"), dir.join("credentials.json"));
                    if self.original_dir.join("settings.json").exists() {
                        let _ = fs::copy(self.original_dir.join("settings.json"), dir.join("settings.json"));
                    }
                    if let Some(tok) = get_profile_token(&self.original_dir.join("credentials.json")) {
                        let _ = set_keychain_token(&dir.join("credentials.json"), &tok);
                    }
                    account.config_dir = dir;
                }
            }
            if !state.original_discovered {
                state.original_discovered = true;
                if let Some(profile) = read_profile(&self.original_dir)? {
                    let dir = self.root.join("profiles").join(Uuid::new_v4().to_string());
                    let _ = private_dir(&dir);
                    let _ = fs::copy(self.original_dir.join("credentials.json"), dir.join("credentials.json"));
                    if self.original_dir.join("settings.json").exists() {
                        let _ = fs::copy(self.original_dir.join("settings.json"), dir.join("settings.json"));
                    }
                    if let Some(tok) = get_profile_token(&self.original_dir.join("credentials.json")) {
                        let _ = set_keychain_token(&dir.join("credentials.json"), &tok);
                    }
                    register(&mut state, profile, dir, true, None)?;
                }
            }
            let value = op(&mut state)?;
            write_state(&self.registry_path(), &state)?;
            Ok(value)
        })();
        let _ = FileExt::unlock(&lock);
        outcome
    }

    pub fn sync_account_to_live(&self, account: &Account) -> Result<()> {
        if account.config_dir == self.original_dir {
            return Ok(());
        }
        private_dir(&self.original_dir)?;
        let src_cred = account.config_dir.join("credentials.json");
        let dst_cred = self.original_dir.join("credentials.json");
        if src_cred.exists() {
            let mut data = fs::read_to_string(&src_cred)?;
            let maybe_tok = get_profile_token(&src_cred);
            if let Some(ref tok) = maybe_tok {
                let _ = set_keychain_token(&dst_cred, tok);
                if let Ok(mut val) = serde_json::from_str::<Value>(&data) {
                    if let Some(obj) = val.get_mut("default").and_then(Value::as_object_mut) {
                        obj.insert("authToken".to_string(), Value::String(tok.clone()));
                        if let Ok(serialized) = serde_json::to_string_pretty(&val) {
                            data = serialized;
                        }
                    }
                }
            }
            write_secure_file(&dst_cred, data.as_bytes())?;
        }
        let src_settings = account.config_dir.join("settings.json");
        let dst_settings = self.original_dir.join("settings.json");
        if src_settings.exists() {
            let _ = fs::copy(&src_settings, &dst_settings);
        }
        let src_analytics = account.config_dir.join("analytics-id.json");
        let dst_analytics = self.original_dir.join("analytics-id.json");
        if src_analytics.exists() {
            let _ = fs::copy(&src_analytics, &dst_analytics);
        }
        Ok(())
    }

    pub fn save_live_to_account(&self, account: &Account) -> Result<()> {
        if account.config_dir == self.original_dir {
            return Ok(());
        }
        let live_cred = self.original_dir.join("credentials.json");
        if !live_cred.exists() {
            return Ok(());
        }
        if let Ok(Some(profile)) = read_profile(&self.original_dir) {
            if profile.id == account.user_id {
                let _ = private_dir(&account.config_dir);
                let dst_cred = account.config_dir.join("credentials.json");
                let _ = fs::copy(&live_cred, &dst_cred);
                if let Some(tok) = get_profile_token(&live_cred) {
                    let _ = set_keychain_token(&dst_cred, &tok);
                    if let Ok(content) = fs::read_to_string(&dst_cred) {
                        if let Ok(mut val) = serde_json::from_str::<Value>(&content) {
                            if let Some(obj) = val.get_mut("default").and_then(Value::as_object_mut) {
                                obj.insert("authToken".to_string(), Value::String(tok));
                                if let Ok(serialized) = serde_json::to_string_pretty(&val) {
                                    let _ = write_secure_file(&dst_cred, serialized.as_bytes());
                                }
                            }
                        }
                    }
                }
                let live_settings = self.original_dir.join("settings.json");
                if live_settings.exists() {
                    let _ = fs::copy(&live_settings, account.config_dir.join("settings.json"));
                }
            }
        }
        Ok(())
    }

    pub fn list(&self) -> Result<State> {
        self.edit(|s| Ok(s.clone()))
    }

    pub fn add_original(&self, alias: Option<String>) -> Result<Account> {
        let profile = read_profile(&self.original_dir)?
            .context("No signed-in Freebuff profile found. Run `freebuff login` first")?;
        self.ensure_root()?;
        let dir = self.root.join("profiles").join(Uuid::new_v4().to_string());
        private_dir(&dir)?;
        let dst_cred = dir.join("credentials.json");
        let _ = fs::copy(self.original_dir.join("credentials.json"), &dst_cred);
        if self.original_dir.join("settings.json").exists() {
            let _ = fs::copy(self.original_dir.join("settings.json"), dir.join("settings.json"));
        }
        if let Some(tok) = get_profile_token(&self.original_dir.join("credentials.json")) {
            let _ = set_keychain_token(&dst_cred, &tok);
            if let Ok(content) = fs::read_to_string(&dst_cred) {
                if let Ok(mut val) = serde_json::from_str::<Value>(&content) {
                    if let Some(obj) = val.get_mut("default").and_then(Value::as_object_mut) {
                        obj.insert("authToken".to_string(), Value::String(tok));
                        if let Ok(serialized) = serde_json::to_string_pretty(&val) {
                            let _ = write_secure_file(&dst_cred, serialized.as_bytes());
                        }
                    }
                }
            }
        }
        let account = self.edit(|state| {
            register(state, profile, dir, true, alias)
        })?;
        let _ = self.sync_account_to_live(&account);
        Ok(account)
    }

    pub fn import_dir(&self, dir: PathBuf, alias: Option<String>) -> Result<Account> {
        if !dir.is_absolute() {
            bail!("The import directory must be absolute");
        }
        let profile = read_profile(&dir)?
            .with_context(|| format!("No signed-in Freebuff profile in {}", dir.display()))?;
        let account = self.edit(|state| register(state, profile, dir, false, alias))?;
        let _ = self.sync_account_to_live(&account);
        Ok(account)
    }

    /// Perform the official browser login inside a new, isolated Freebuff
    /// configuration directory. Never ask for or read an auth token.
    pub fn login(&self, alias: Option<String>) -> Result<Account> {
        validate_alias(alias.as_deref())?;
        self.ensure_root()?;
        let dir = self.root.join("profiles").join(Uuid::new_v4().to_string());
        private_dir(&dir)?;
        println!("\n[LOGIN] Starting official Freebuff login in an isolated profile.");
        println!("        {}", dir.display());
        let status = run_freebuff(&dir, &[OsString::from("login")])?;
        if !status.success() {
            bail!("Freebuff login exited with {}. Profile directory preserved at {}", status, dir.display());
        }
        let profile = read_profile(&dir)?
            .context("Freebuff login succeeded, but no valid credentials.json profile appeared")?;
        let account = self.edit(|state| register(state, profile, dir, false, alias))?;
        let _ = self.sync_account_to_live(&account);
        println!("[OK] Registered and activated {} (added {})", sanitize(&account.email), added_at_local(account.created_at_unix));
        Ok(account)
    }

    pub fn select(&self, query: &str) -> Result<Account> {
        self.edit(|state| {
            let index = resolve_index(state, query)?;
            let account = state.accounts[index].clone();
            // Never activate an account whose backing directory was replaced.
            verify_profile(&account)?;
            if let Some(prev_key) = state.active.as_deref() {
                if prev_key != account.key {
                    if let Some(prev_acc) = state.accounts.iter().find(|a| a.key == prev_key) {
                        let _ = self.save_live_to_account(prev_acc);
                    }
                }
            }
            self.sync_account_to_live(&account)?;
            if state.active.as_deref() != Some(&account.key) {
                state.previous = state.active.clone();
                state.active = Some(account.key.clone());
            }
            Ok(account)
        })
    }

    pub fn rotate(&self) -> Result<Account> {
        self.edit(|state| {
            if state.accounts.is_empty() {
                bail!("No registered Freebuff accounts");
            }
            let start = state
                .active
                .as_ref()
                .and_then(|key| state.accounts.iter().position(|a| &a.key == key))
                .unwrap_or(state.accounts.len() - 1);
            for offset in 1..=state.accounts.len() {
                let next = (start + offset) % state.accounts.len();
                let account = state.accounts[next].clone();
                if verify_profile(&account).is_ok() {
                    if let Some(prev_key) = state.active.as_deref() {
                        if prev_key != account.key {
                            if let Some(prev_acc) = state.accounts.iter().find(|a| a.key == prev_key) {
                                let _ = self.save_live_to_account(prev_acc);
                            }
                        }
                    }
                    self.sync_account_to_live(&account)?;
                    if state.active.as_deref() != Some(&account.key) {
                        state.previous = state.active.clone();
                        state.active = Some(account.key.clone());
                    }
                    return Ok(account);
                }
            }
            bail!("No usable Freebuff account profiles")
        })
    }

    pub fn set_alias(&self, query: &str, alias: Option<String>) -> Result<Account> {
        validate_alias(alias.as_deref())?;
        self.edit(|state| {
            let index = resolve_index(state, query)?;
            if let Some(alias) = &alias {
                if state.accounts.iter().enumerate().any(|(i, a)| {
                    i != index && (a.alias.as_ref().is_some_and(|x| x.eq_ignore_ascii_case(alias)) ||
                        a.email.eq_ignore_ascii_case(alias) || a.user_id == *alias || a.key == *alias)
                }) {
                    bail!("Alias already assigned to another account");
                }
            }
            state.accounts[index].alias = alias;
            Ok(state.accounts[index].clone())
        })
    }

    /// Forget a profile registration, but NEVER delete its config directory.
    /// Deleting a directory may orphan OS-keychain entries or user data.
    pub fn remove(&self, query: &str) -> Result<Account> {
        self.edit(|state| {
            let index = resolve_index(state, query)?;
            let account = state.accounts.remove(index);
            if state.active.as_deref() == Some(&account.key) {
                state.active = state.accounts.first().map(|a| a.key.clone());
                if let Some(new_key) = state.active.as_deref() {
                    if let Some(new_acc) = state.accounts.iter().find(|a| a.key == new_key) {
                        let _ = self.sync_account_to_live(new_acc);
                    }
                }
            }
            if state.previous.as_deref() == Some(&account.key) {
                state.previous = None;
            }
            Ok(account)
        })
    }

    pub fn selected(&self) -> Result<Account> {
        let state = self.list()?;
        let key = state.active.context("No active account. Run `freebuff-auth login` or `add`")?;
        let account = state.accounts.into_iter().find(|a| a.key == key)
            .context("The selected profile was removed")?;
        verify_profile(&account)?;
        if self.original_dir.join("credentials.json").exists() {
            let live_profile = read_profile(&self.original_dir)?
                .context("Live Freebuff credentials missing or invalid")?;
            if live_profile.id != account.user_id {
                bail!("Backing Freebuff login changed for {}. Run `freebuff-auth add` or re-register that directory", sanitize(&account.email));
            }
        }
        Ok(account)
    }

    pub fn run(&self, args: &[OsString]) -> Result<ExitStatus> {
        let account = self.selected()?;
        run_freebuff(&account.config_dir, args)
    }

    pub fn is_valid(&self, account: &Account) -> bool {
        verify_profile(account).is_ok()
    }
}

fn register(
    state: &mut State,
    profile: Profile,
    dir: PathBuf,
    original: bool,
    alias: Option<String>,
) -> Result<Account> {
    validate_alias(alias.as_deref())?;
    // An existing path might have been re-logged-in to a different account.
    // Update its identity, rather than accidentally using the old account name.
    if let Some(index) = state.accounts.iter().position(|a| a.config_dir == dir || (original && a.original)) {
        if let Some(ref alias) = alias {
            if state.accounts.iter().enumerate().any(|(i, a)|
                i != index && (a.alias.as_ref().is_some_and(|v| v.eq_ignore_ascii_case(alias)) ||
                a.email.eq_ignore_ascii_case(alias) || a.user_id == *alias || a.key == *alias)) {
                bail!("Alias collides with another account identifier");
            }
        }
        let existing = &mut state.accounts[index];
        existing.user_id = profile.id;
        existing.email = profile.email;
        existing.name = profile.name;
        existing.config_dir = dir;
        if alias.is_some() { existing.alias = alias; }
        let account = existing.clone();
        let old = state.active.replace(account.key.clone());
        if old.as_deref() != Some(&account.key) { state.previous = old; }
        return Ok(account);
    }
    if let Some(alias) = &alias {
        if state.accounts.iter().any(|a|
            a.alias.as_ref().is_some_and(|s| s.eq_ignore_ascii_case(alias)) ||
            a.email.eq_ignore_ascii_case(alias) || a.user_id == *alias || a.key == *alias) {
            bail!("Alias collides with another account identifier");
        }
    }
    let account = Account {
        key: Uuid::new_v4().to_string(),
        user_id: profile.id,
        email: profile.email,
        name: profile.name,
        alias,
        config_dir: dir,
        original,
        created_at_unix: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
    };
    let old = state.active.replace(account.key.clone());
    if old.as_deref() != Some(&account.key) { state.previous = old; }
    state.accounts.push(account.clone());
    Ok(account)
}

pub fn resolve_index(state: &State, query: &str) -> Result<usize> {
    if query == "-" {
        let key = state.previous.as_ref().context("No previously active account")?;
        return state.accounts.iter().position(|a| &a.key == key)
            .context("Previously active account no longer exists");
    }
    if let Ok(number) = query.parse::<usize>() {
        if number > 0 && number <= state.accounts.len() { return Ok(number - 1); }
        bail!("Invalid account number: {}", number);
    }
    let matches: Vec<usize> = state.accounts.iter().enumerate().filter_map(|(i, a)| {
        let matched = a.key == query || a.user_id == query ||
            a.email.eq_ignore_ascii_case(query) ||
            a.alias.as_ref().is_some_and(|v| v.eq_ignore_ascii_case(query));
        matched.then_some(i)
    }).collect();
    match matches.as_slice() {
        [index] => Ok(*index),
        [] => bail!("No account matches: {}", sanitize(query)),
        _ => bail!("Ambiguous account query: {} (use a row number)", sanitize(query)),
    }
}

fn validate_alias(alias: Option<&str>) -> Result<()> {
    if let Some(alias) = alias {
        if alias.trim() != alias || alias.is_empty() || alias.len() > 64 ||
            alias.chars().any(|c| c.is_control()) || alias == "-" || alias.parse::<usize>().is_ok() {
            bail!("Alias must be 1-64 characters, no controls, no surrounding spaces or numeric-only aliases");
        }
    }
    Ok(())
}

fn read_state(path: &Path) -> Result<State> {
    if !path.exists() { return Ok(State::default()); }
    let data = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_slice::<State>(&data).context("Invalid account registry JSON; original file was not overwritten")
}

fn write_state(path: &Path, state: &State) -> Result<()> {
    let parent = path.parent().context("No account registry parent directory")?;
    let mut file = NamedTempFile::new_in(parent).context("creating secure temporary registry")?;
    #[cfg(unix)]
    file.as_file().set_permissions(fs::Permissions::from_mode(0o600))?;
    serde_json::to_writer_pretty(&mut file, state)?;
    file.write_all(b"\n")?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|e| e.error).context("atomically replacing account registry")?;
    #[cfg(unix)]
    if let Ok(dir) = File::open(parent) { let _ = dir.sync_all(); }
    Ok(())
}

pub fn read_profile(dir: &Path) -> Result<Option<Profile>> {
    let path = dir.join("credentials.json");
    let metadata = match fs::metadata(&path) {
        Ok(v) => v,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("opening {}", path.display())),
    };
    if metadata.len() > MAX_CREDENTIALS_BYTES { bail!("Credentials file exceeds 2 MiB"); }
    let mut data = String::new();
    File::open(&path)?.take(MAX_CREDENTIALS_BYTES + 1).read_to_string(&mut data)?;
    let root: Value = serde_json::from_str(&data)
        .with_context(|| format!("Invalid JSON in {}", path.display()))?;
    let Some(profile) = root.get("default") else { return Ok(None); };
    let id = profile.get("id").and_then(Value::as_str).unwrap_or("");
    let email = profile.get("email").and_then(Value::as_str).unwrap_or("");
    let fingerprints_present = ["fingerprintId", "fingerprintHash"].iter().all(|key|
        profile.get(*key).and_then(Value::as_str).is_some_and(|s| !s.is_empty()));
    let has_token = profile.get("authToken").and_then(Value::as_str).is_some_and(|s| !s.is_empty())
        || profile.get("tokenStore").and_then(Value::as_str) == Some("keychain");
    if id.is_empty() || email.is_empty() || !has_token || !fingerprints_present {
        bail!("Freebuff default profile lacks required login fields in {}", path.display());
    }
    if id.len() > 512 || email.len() > 512 { bail!("Invalid Freebuff profile identifier size"); }
    let name = profile.get("name").and_then(Value::as_str).unwrap_or("");
    Ok(Some(Profile { id: id.to_owned(), email: email.to_owned(), name: name.to_owned() }))
}

pub fn verify_profile(account: &Account) -> Result<()> {
    let profile = read_profile(&account.config_dir)
        .with_context(|| format!("Reading profile for {}", sanitize(&account.email)))?
        .context("Freebuff credentials missing from account directory")?;
    if profile.id != account.user_id || !profile.email.eq_ignore_ascii_case(&account.email) {
        bail!("Backing Freebuff login changed for {}. Run `freebuff-auth add` or re-register that directory", sanitize(&account.email));
    }
    Ok(())
}

fn private_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path).with_context(|| format!("creating {}", path.display()))?;
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

pub fn run_freebuff(config_dir: &Path, args: &[OsString]) -> Result<ExitStatus> {
    let binary = env::var_os("FREEBUFF_BIN").unwrap_or_else(|| OsString::from("freebuff"));
    let status = Command::new(&binary)
        .args(args)
        .env("FREEBUFF_CONFIG_DIR", config_dir)
        .status()
        .with_context(|| format!("Cannot launch official Freebuff binary {:?}; set FREEBUFF_BIN to its absolute path", binary))?;
    Ok(status)
}

/// Remove ANSI escapes and terminal controls from untrusted names.
pub fn sanitize(value: &str) -> String {
    value.chars().map(|c| if c.is_control() { ' ' } else { c }).collect()
}

pub fn json_accounts(state: &State) -> Value {
    serde_json::json!({
        "ok": true,
        "data": {
            "accounts": state.accounts.iter().enumerate().map(|(index, a)| {
                serde_json::json!({
                    "number": index + 1,
                    "id": a.user_id,
                    "key": a.key,
                    "email": a.email,
                    "name": a.name,
                    "alias": a.alias,
                    "active": state.active.as_deref() == Some(&a.key),
                    "config_dir": a.config_dir,
                    "created_at_unix": a.created_at_unix,
                    "added_at": added_at_iso(a.created_at_unix),
                    "usage": null,
                    "plan": null,
                })
            }).collect::<Vec<_>>(),
            "active_count": usize::from(state.active.is_some()),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn write_mock(dir: &Path, id: &str, email: &str) {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join("credentials.json"), json!({"default": {
            "id": id, "email": email, "name": "Demo", "tokenStore": "keychain",
            "fingerprintId": "fp", "fingerprintHash": "hash"
        }}).to_string()).unwrap();
    }

    #[test]
    fn initial_registration_and_switch() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = Manager::new(tmp.path().join("manager"), tmp.path().join("official")).unwrap();
        write_mock(&manager.original_dir, "u1", "a@example.com");
        let state = manager.list().unwrap();
        assert_eq!(state.accounts.len(), 1);
        assert_eq!(state.active.as_deref(), Some(state.accounts[0].key.as_str()));
        let second = tmp.path().join("extra");
        write_mock(&second, "u2", "b@example.com");
        manager.import_dir(second, Some("work".into())).unwrap();
        assert_eq!(manager.list().unwrap().accounts.len(), 2);
        assert_eq!(manager.selected().unwrap().email, "b@example.com");
        manager.select("-").unwrap();
        assert_eq!(manager.selected().unwrap().email, "a@example.com");
        manager.select("work").unwrap();
        manager.remove("work").unwrap();
        assert_eq!(manager.list().unwrap().accounts.len(), 1);
    }

    #[test]
    fn added_dates_persist_and_order_is_stable() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = Manager::new(tmp.path().join("manager"), tmp.path().join("official")).unwrap();
        write_mock(&manager.original_dir, "u1", "a@example.com");
        let original = manager.list().unwrap().accounts[0].clone();
        assert!(original.created_at_unix > 0);
        assert!(added_at_iso(original.created_at_unix).unwrap().ends_with('Z'));
        let added = tmp.path().join("second");
        write_mock(&added, "u2", "b@example.com");
        let second = manager.import_dir(added, None).unwrap();
        let before = manager.list().unwrap();
        let keys: Vec<_> = before.accounts.iter().map(|a| a.key.as_str()).collect();
        let json = json_accounts(&before);
        assert_eq!(json["data"]["accounts"][0]["created_at_unix"], original.created_at_unix);
        assert_eq!(json["data"]["accounts"][1]["added_at"],
            added_at_iso(second.created_at_unix).unwrap());
        manager.select("1").unwrap();
        manager.add_original(None).unwrap();
        let after = manager.list().unwrap();
        assert_eq!(after.accounts.iter().map(|a| a.key.as_str()).collect::<Vec<_>>(), keys);
        assert_eq!(after.accounts[0].created_at_unix, original.created_at_unix);
        assert_eq!(after.accounts[1].created_at_unix, second.created_at_unix);
        // A subsequent manager instance reads exactly the same stored timestamps.
        let manager2 = Manager::new(manager.root.clone(), manager.original_dir.clone()).unwrap();
        assert_eq!(manager2.list().unwrap().accounts[0].created_at_unix, original.created_at_unix);
    }

    #[test]
    fn unknown_legacy_date_is_not_fabricated() {
        assert_eq!(added_at_iso(0), None);
        assert_eq!(added_at_date(0), "unknown");
        assert_eq!(added_at_local(0), "unknown");
    }

    #[test]
    fn replaced_credentials_are_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = Manager::new(tmp.path().join("manager"), tmp.path().join("official")).unwrap();
        write_mock(&manager.original_dir, "u1", "a@example.com");
        manager.list().unwrap();
        write_mock(&manager.original_dir, "u2", "b@example.com");
        assert!(manager.selected().is_err());
        manager.add_original(None).unwrap();
        assert_eq!(manager.selected().unwrap().email, "b@example.com");
    }

    #[test]
    fn corrupt_credentials_do_not_reset_registry() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = Manager::new(tmp.path().join("manager"), tmp.path().join("official")).unwrap();
        write_mock(&manager.original_dir, "u1", "a@example.com");
        manager.list().unwrap();
        fs::write(manager.original_dir.join("credentials.json"), "{").unwrap();
        assert!(manager.selected().is_err());
        assert!(manager.registry_path().exists());
    }
}
