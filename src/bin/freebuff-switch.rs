use anyhow::Result;
use clap::Parser;
use freebuff_auth::{ui, Manager};
use std::process;

#[derive(Parser)]
#[command(name = "freebuff-switch", version, about = "Rust Freebuff account selector")]
struct Opts {
    /// Launch official Freebuff immediately after selecting an account.
    #[arg(long)]
    run: bool,
    #[arg(long)]
    no_color: bool,
}

fn entry() -> Result<()> {
    let options = Opts::parse();
    let manager = Manager::from_env()?;
    if ui::switch_interactive(&manager, options.no_color)?.is_some() && options.run {
        let status = manager.run(&[])?;
        process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

fn main() {
    if let Err(err) = entry() {
        eprintln!("[ERROR] {err:#}");
        process::exit(1);
    }
}
