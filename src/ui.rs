//! ANSI terminal selector modeled after the provided AGY bash account switcher.
//! No shell, jq, or Python runtime dependencies.

use crate::{added_at_date, added_at_local, sanitize, Account, Manager, State};
use anyhow::Result;
use std::env;
use std::io::{self, IsTerminal, Write};
use unicode_width::UnicodeWidthChar;

pub struct Colors {
    red: &'static str,
    green: &'static str,
    dim: &'static str,
    bold: &'static str,
    reset: &'static str,
}

impl Colors {
    fn new(no_color: bool) -> Self {
        let colors = !no_color && env::var_os("NO_COLOR").is_none()
            && io::stdout().is_terminal()
            && env::var("TERM").unwrap_or_default() != "dumb";
        if colors {
            Self { red: "\x1b[1;31m", green: "\x1b[1;32m", dim: "\x1b[2m",
                bold: "\x1b[1m", reset: "\x1b[0m" }
        } else {
            Self { red: "", green: "", dim: "", bold: "", reset: "" }
        }
    }
}

fn width() -> usize {
    let user = env::var("COLUMNS").ok().and_then(|v| v.parse::<usize>().ok());
    let width = crossterm::terminal::size().ok().map(|(w, _)| usize::from(w));
    width.or(user).unwrap_or(88).clamp(40, 120)
}

fn fit(input: &str, width: usize) -> String {
    let mut result = String::new();
    let mut used = 0;
    for ch in sanitize(input).chars() {
        // Unicode-format controls (e.g. bidi overrides) are not printable UI data.
        if matches!(ch, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}') { continue; }
        let cell = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + cell > width { break; }
        used += cell;
        result.push(ch);
    }
    result.push_str(&" ".repeat(width.saturating_sub(used)));
    result
}

fn cell_name(a: &Account) -> String {
    if let Some(alias) = &a.alias {
        format!("{} ({})", alias, a.email)
    } else {
        a.email.clone()
    }
}

pub fn print_accounts(state: &State, no_color: bool) {
    let c = Colors::new(no_color);
    let cols = width();
    println!();
    if cols >= 76 {
        println!("{}{}{}", c.red, "  ______ ____  ______ ______ ____  __  ______ ______", c.reset);
        println!("{}{}{}", c.red, " / ____// __ \\/ ____// ____// __ )/ / / / ____// ____/", c.reset);
        println!("{}{}{}", c.red, "/ /_   / /_/ / __/  / __/  / __  / / / / /_   / /_", c.reset);
        println!("{}{}{}", c.red, "/ __/  / _, _/ /___ / /___ / /_/ / /_/ / __/  / __/", c.reset);
        println!("{}{}{}", c.red, "/_/    /_/ |_/_____//_____/_____ /____/_/    /_/", c.reset);
    } else {
        println!("{}  [ FREEBUFF // AUTH ]{}", c.red, c.reset);
    }
    println!("{}  ACCOUNT CONTROL CENTER{}", c.bold, c.reset);
    println!("{}  Secure selector  ::  LIVE = selected  ::  a = add  ::  d = delete  ::  i = import  ::  q = quit{}\n", c.dim, c.reset);

    let active_count = state.accounts.iter().filter(|a| state.active.as_deref() == Some(&a.key)).count();
    if cols < 72 {
        for (i, a) in state.accounts.iter().enumerate() {
            let active = state.active.as_deref() == Some(&a.key);
            let symbol = if active { "*" } else { " " };
            let status = if active { c.green } else { "" };
            println!("{}  [{}] {}  {}{}", status, i + 1, symbol,
                fit(&cell_name(a), cols.saturating_sub(14)), if active { c.reset } else { "" });
            println!("{}       PROFILE: {}  |  ADDED: {}{}", c.dim,
                if a.original { "DEFAULT" } else { "ISOLATED" },
                added_at_local(a.created_at_unix), c.reset);
        }
    } else {
        // Always reserve a visible ADDED column, including on 72-column terminals.
        // On very wide terminals, preserve the unavailable quota placeholders.
        let name_w = cols.saturating_sub(46).max(16);
        let head = format!(" | {:>4} {:<4} {} {:<9} {:<10} |",
            "NUM", "ST", fit("ACCOUNT / ALIAS", name_w), "PROFILE", "ADDED");
        let rule = "=".repeat(head.chars().count().saturating_sub(4));
        println!("{}  +{}+{}", c.dim, rule, c.reset);
        println!("{}{}{}", c.bold, head, c.reset);
        println!("{}  +{}+{}", c.dim, rule, c.reset);
        for (i, a) in state.accounts.iter().enumerate() {
            let active = state.active.as_deref() == Some(&a.key);
            let profile = if a.original { "DEFAULT" } else { "ISOLATED" };
            let date = added_at_date(a.created_at_unix);
            let line = format!(" | {:>4} {:<4} {} {:<9} {:<10} |",
                i + 1, if active { "LIVE" } else { "--" },
                fit(&cell_name(a), name_w), profile, date);
            if active { println!("{}{}{}", c.green, line, c.reset); }
            else { println!("{}", line); }
        }
        println!("{}  +{}+{}", c.dim, rule, c.reset);
    }
    println!("\n{}  [ACCOUNTS: {}] [ACTIVE: {}]{}", c.dim, state.accounts.len(), active_count, c.reset);
    println!("{}  Usage percentages: unavailable from a verified Freebuff account API.{}\n", c.dim, c.reset);
}

fn delete_prompt(manager: &Manager, query: &str, c: &Colors) -> Result<()> {
    let state = manager.list()?;
    let index = match crate::resolve_index(&state, query) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("{}[ERROR]{} {e:#}", c.red, c.reset);
            return Ok(());
        }
    };
    let account = &state.accounts[index];
    print!("  Are you sure you want to delete {} (account #{})? [y/N]: ", sanitize(&account.email), index + 1);
    io::stdout().flush()?;
    let mut confirm = String::new();
    if io::stdin().read_line(&mut confirm)? == 0 {
        println!("  Cancelled.");
        return Ok(());
    }
    let confirm = confirm.trim().to_lowercase();
    if confirm == "y" || confirm == "yes" {
        match manager.remove(query) {
            Ok(acc) => {
                println!("{}[OK]{} Removed account #{} ({})", c.green, c.reset, index + 1, sanitize(&acc.email));
            }
            Err(e) => eprintln!("{}[ERROR]{} {e:#}", c.red, c.reset),
        }
    } else {
        println!("  Cancelled.");
    }
    Ok(())
}

/// Returns Some(account) for a selection, or None when the user exits.
pub fn switch_interactive(manager: &Manager, no_color: bool) -> Result<Option<Account>> {
    loop {
        let state = manager.list()?;
        print_accounts(&state, no_color);
        let c = Colors::new(no_color);
        print!("{}  >>{} Enter account number [a=add, d=delete, i=import, r=reload, q=quit]: ", c.red, c.reset);
        io::stdout().flush()?;
        let mut choice = String::new();
        if io::stdin().read_line(&mut choice)? == 0 { println!("\n  Cancelled."); return Ok(None); }
        let choice = choice.trim();
        if let Some(target) = choice.strip_prefix("d ").or_else(|| choice.strip_prefix("D "))
            .or_else(|| choice.strip_prefix("del ")).or_else(|| choice.strip_prefix("rm ")) {
            delete_prompt(manager, target.trim(), &c)?;
            continue;
        }
        match choice {
            "" | "q" | "Q" => { println!("  Cancelled."); return Ok(None); }
            "r" | "R" => continue,
            "d" | "D" | "del" | "DEL" | "delete" | "DELETE" | "rm" | "RM" => {
                print!("  Enter account number to delete: ");
                io::stdout().flush()?;
                let mut target_input = String::new();
                if io::stdin().read_line(&mut target_input)? == 0 {
                    println!("\n  Cancelled.");
                    return Ok(None);
                }
                let target = target_input.trim();
                if !target.is_empty() {
                    delete_prompt(manager, target, &c)?;
                }
                continue;
            }
            "a" | "A" | "add" | "ADD" | "+" => {
                print!("  Account alias (optional; Enter to skip): ");
                io::stdout().flush()?;
                let mut alias_input = String::new();
                if io::stdin().read_line(&mut alias_input)? == 0 {
                    println!("\n  Cancelled.");
                    return Ok(None);
                }
                let alias = alias_input.trim();
                let alias_opt = if alias.is_empty() { None } else { Some(alias.to_string()) };
                match manager.login(alias_opt) {
                    Ok(a) => println!("{}[OK]{} Added and activated {} (added {})",
                        c.green, c.reset, sanitize(&a.email), added_at_local(a.created_at_unix)),
                    Err(e) => eprintln!("{}[ERROR]{} {e:#}", c.red, c.reset),
                }
                continue;
            }
            "i" | "I" | "import" | "IMPORT" => {
                print!("  Account alias (optional; Enter to skip): ");
                io::stdout().flush()?;
                let mut alias_input = String::new();
                if io::stdin().read_line(&mut alias_input)? == 0 {
                    println!("\n  Cancelled.");
                    return Ok(None);
                }
                let alias = alias_input.trim();
                let alias_opt = if alias.is_empty() { None } else { Some(alias.to_string()) };
                match manager.add_original(alias_opt) {
                    Ok(a) => println!("{}[OK]{} Imported and registered {} (added {})",
                        c.green, c.reset, sanitize(&a.email), added_at_local(a.created_at_unix)),
                    Err(e) => eprintln!("{}[ERROR]{} {e:#}", c.red, c.reset),
                }
                continue;
            }
            _ => match manager.select(choice) {
                Ok(account) => {
                    println!("\n{}[OK]{} Account #{} selected: {}", c.green, c.reset,
                        choice, sanitize(&account.email));
                    println!("     Active Freebuff account updated: {}", sanitize(&account.email));
                    return Ok(Some(account));
                }
                Err(err) => {
                    eprintln!("{}[ERROR]{} {err:#}", c.red, c.reset);
                }
            },
        }
    }
}
