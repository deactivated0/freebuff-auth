use anyhow::Result;
use clap::Parser;
use freebuff_auth::{ui, Manager};
use std::{process, thread, time::Duration};

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
    if let Some(_account) = ui::switch_interactive(&manager, options.no_color)? {
        if options.run {
            let status = manager.run(&[])?;
            process::exit(status.code().unwrap_or(1));
        } else {
            // Briefly pause so user can see selection feedback before terminal window closes
            thread::sleep(Duration::from_millis(1000));
        }
    }
    Ok(())
}

fn main() {
    if let Err(err) = entry() {
        eprintln!("[ERROR] {err:#}");
        eprintln!("\nPress Enter to exit...");
        let _ = std::io::stdin().read_line(&mut String::new());
        process::exit(1);
    }
}
