use std::io::Read;
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
    /// Ingest one grounding result and append it to
    /// `<memo>.evidence.jsonl`.
    ///
    /// The record is read as JSON from `--input <file>`, or from stdin if
    /// `--input` is omitted — never from a command-line argument, so a
    /// note containing backticks or newlines survives intact. Refuses an
    /// unknown claim id, a missing/invalid verdict, or malformed JSON,
    /// and never performs a partial write.
    Record {
        /// The memo the claim id must be defined in.
        memo: PathBuf,
        /// Read the record from this file instead of stdin.
        #[arg(long)]
        input: Option<PathBuf>,
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
        Command::Record { memo, input } => {
            let input_json = match &input {
                Some(path) => std::fs::read_to_string(path),
                None => {
                    let mut buf = String::new();
                    std::io::stdin().read_to_string(&mut buf).map(|_| buf)
                }
            };
            let input_json = match input_json {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("tetel: error reading input: {e}");
                    return ExitCode::from(1);
                }
            };
            match tetel::record_file(&memo, &input_json) {
                Ok(Ok(())) => ExitCode::from(0),
                Ok(Err(e)) => {
                    eprintln!("tetel: refused: {e}");
                    ExitCode::from(1)
                }
                Err(e) => {
                    eprintln!("tetel: error reading {}: {e}", memo.display());
                    ExitCode::from(1)
                }
            }
        }
    }
}
