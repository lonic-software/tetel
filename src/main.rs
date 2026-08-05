use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

/// tetel — every factual claim carries executable evidence.
#[derive(Parser)]
#[command(name = "tetel", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Check a markdown file's `tetel` evidence rows.
    ///
    /// Never writes any file, never executes any command from the
    /// document, and makes no network calls.
    Check {
        /// The markdown file to check.
        file: PathBuf,
    },
    /// Emit the grounding brief for a memo's evidence ledger: every
    /// claim's id and proposition, byte-identical to the source, with
    /// domain/extent withheld so an independent pass can't see what the
    /// author declared the claim ranges over.
    ///
    /// Read-only: never writes to the memo.
    Brief {
        /// The memo to brief.
        memo: PathBuf,
        /// Emit machine-readable JSON instead of the human-readable form.
        #[arg(long)]
        json: bool,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Check { file } => match tetel::check_file(&file) {
            Ok((code, report)) => {
                print!("{report}");
                ExitCode::from(code as u8)
            }
            Err(e) => {
                eprintln!("tetel: error reading {}: {e}", file.display());
                ExitCode::from(1)
            }
        },
        Command::Brief { memo, json } => match tetel::brief_file(&memo, json) {
            Ok((code, out)) => {
                print!("{out}");
                ExitCode::from(code as u8)
            }
            Err(e) => {
                eprintln!("tetel: error reading {}: {e}", memo.display());
                ExitCode::from(1)
            }
        },
    }
}
