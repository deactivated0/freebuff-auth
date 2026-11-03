use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use freebuff_auth::{added_at_iso, added_at_local, json_accounts, sanitize, ui, Manager, VERSION};
use serde_json::json;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process;

#[derive(Parser)]
#[command(name = "freebuff-auth", version = VERSION, about = "Rust-only Freebuff multi-account manager")]
struct Cli {
    #[arg(long, global = true, help = "Structured JSON output when available")]
    json: bool,
    #[arg(long, global = true)]
    no_color: bool,
    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Show registered Freebuff accounts. Active is the profile used by `run`.
    #[command(alias = "ls")]
    List { #[arg(long)] active: bool },
    /// Register the account currently signed in to the original Freebuff CLI.
    Add { #[arg(long)] alias: Option<String> },
    /// Log in through the official Freebuff CLI in a fresh isolated profile.
    Login { #[arg(long)] alias: Option<String> },
    /// Select a profile by number, alias, email, key, or - (previous).
    #[command(alias = "use")]
    Switch { query: Option<String>, #[arg(long)] run: bool },
    /// Rotate through registered profiles, not based on a quota estimate.
    Rotate { #[arg(long)] run: bool },
    /// Forget an account registration, leaving its credentials untouched.
    #[command(alias = "rm")]
    Remove { query: String, #[arg(long)] yes: bool },
    /// Manage account nicknames.
    Alias { #[command(subcommand)] action: AliasCmd },
    /// Display selected account.
    Current,
    /// Display registry paths and selected profile health.
    Status,
    /// Display relevant configuration paths.
    #[command(alias = "paths")]
    Path,
    /// Register an existing Freebuff config directory without copying tokens.
    Import { dir: PathBuf, #[arg(long)] alias: Option<String> },
    /// Launch official Freebuff using the selected isolated account.
    Run { #[arg(last = true, allow_hyphen_values = true)] args: Vec<OsString> },
    /// Freebuff has no verified programmatic quota API in this implementation.
    Usage,
}

#[derive(Subcommand)]
enum AliasCmd {
    Set { query: String, alias: String },
    Clear { query: String },
    List,
}

fn main() {
    if let Err(err) = entry() {
        eprintln!("[ERROR] {err:#}");
        process::exit(1);
    }
}

fn run_and_exit(manager: &Manager, args: &[OsString]) -> Result<()> {
    let status = manager.run(args)?;
    process::exit(status.code().unwrap_or(1));
}

fn entry() -> Result<()> {
    let cli = Cli::parse();
    let manager = Manager::from_env()?;
    match cli.command.unwrap_or(Cmd::List { active: false }) {
        Cmd::List { active } => {
            let state = manager.list()?;
            if cli.json {
                let mut payload = json_accounts(&state);
                if active {
                    payload["data"]["accounts"].as_array_mut()
                        .context("Invalid account envelope")?
                        .retain(|a| a["active"] == true);
                }
                println!("{}", serde_json::to_string_pretty(&payload)?);
            } else {
                if active {
                    let mut filtered = state.clone();
                    filtered.accounts.retain(|a| state.active.as_deref() == Some(&a.key));
                    ui::print_accounts(&filtered, cli.no_color);
                } else {
                    ui::print_accounts(&state, cli.no_color);
                }
            }
        }
        Cmd::Add { alias } => {
            let account = manager.add_original(alias)?;
            println!("[OK] Registered {} (added {})",
                sanitize(&account.email), added_at_local(account.created_at_unix));
        }
        Cmd::Login { alias } => { manager.login(alias)?; }
        Cmd::Import { dir, alias } => {
            let account = manager.import_dir(dir, alias)?;
            println!("[OK] Registered {} (added {})",
                sanitize(&account.email), added_at_local(account.created_at_unix));
        }
        Cmd::Switch { query, run } => {
            let account = if let Some(query) = query {
                Some(manager.select(&query)?)
            } else {
                ui::switch_interactive(&manager, cli.no_color)?
            };
            if let Some(account) = account {
                println!("[OK] Selected {}", sanitize(&account.email));
                if run { run_and_exit(&manager, &[])?; }
                else { println!("     Launch with: freebuff-auth run"); }
            }
        }
        Cmd::Rotate { run } => {
            let account = manager.rotate()?;
            println!("[OK] Selected {}", sanitize(&account.email));
            println!("     Rotation is round-robin, not quota-aware.");
            if run { run_and_exit(&manager, &[])?; }
        }
        Cmd::Remove { query, yes } => {
            let state = manager.list()?;
            let account = &state.accounts[freebuff_auth::resolve_index(&state, &query)?];
            if !yes {
                bail!("Refusing to forget {} without --yes. Credentials are not deleted", sanitize(&account.email));
            }
            let account = manager.remove(&query)?;
            println!("[OK] Unregistered {}. Config directory left untouched: {}",
                sanitize(&account.email), account.config_dir.display());
        }
        Cmd::Alias { action } => match action {
            AliasCmd::Set { query, alias } => {
                let account = manager.set_alias(&query, Some(alias))?;
                println!("[OK] Alias updated for {}", sanitize(&account.email));
            }
            AliasCmd::Clear { query } => {
                let account = manager.set_alias(&query, None)?;
                println!("[OK] Alias cleared for {}", sanitize(&account.email));
            }
            AliasCmd::List => {
                let state = manager.list()?;
                for (i, a) in state.accounts.iter().enumerate() {
                    println!("{}  {}  {}", i + 1,
                        sanitize(a.alias.as_deref().unwrap_or("-")), sanitize(&a.email));
                }
            }
        },
        Cmd::Current => {
            let account = manager.selected()?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&json!({
                    "ok": true, "data": {"id": account.user_id, "email": account.email,
                    "name": account.name, "alias": account.alias,
                    "config_dir": account.config_dir,
                    "created_at_unix": account.created_at_unix,
                    "added_at": added_at_iso(account.created_at_unix)}
                }))?);
            } else {
                println!("{}  {}", sanitize(&account.email), account.config_dir.display());
                println!("Date added: {}", added_at_local(account.created_at_unix));
            }
        }
        Cmd::Status => {
            let state = manager.list()?;
            let selected = state.active.as_ref().and_then(|key| state.accounts.iter().find(|a| &a.key == key));
            let valid = selected.is_some_and(|a| manager.is_valid(a));
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&json!({
                    "ok": true, "data": {
                        "registry": manager.registry_path(), "original": manager.original_dir,
                        "selected": selected.map(|a| &a.email), "selected_valid": valid,
                        "count": state.accounts.len()
                    }
                }))?);
            } else {
                println!("Registry: {}", manager.registry_path().display());
                println!("Original Freebuff: {}", manager.original_dir.display());
                println!("Registered profiles: {}", state.accounts.len());
                println!("Selected: {}", selected.map(|a| sanitize(&a.email)).unwrap_or_else(|| "none".into()));
                println!("Selected credentials: {}", if valid { "present" } else { "missing, changed, or invalid" });
                println!("OS-keychain token validity: not inspected");
            }
        }
        Cmd::Path => {
            println!("Manager: {}", manager.root.display());
            println!("Registry: {}", manager.registry_path().display());
            println!("Original Freebuff: {}", manager.original_dir.display());
            println!("Isolated profiles: {}", manager.root.join("profiles").display());
        }
        Cmd::Run { args } => { run_and_exit(&manager, &args)?; }
        Cmd::Usage => {
            bail!("Usage stats are not implemented: Freebuff exposes no verified quota API here. Check /usage inside Freebuff");
        }
    }
    Ok(())
}
