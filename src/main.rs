use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

/// tetel — every factual claim carries executable evidence.
#[derive(Parser)]
#[command(name = "tetel", version, about)]
struct Cli {
    /// Which authoring session's state to use (see `tetel::session` for
    /// where that state lives). Irrelevant to `check`/`brief`/`record`,
    /// which read a memo already on disk instead.
    #[arg(long, global = true, default_value = "default")]
    session: String,

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

    // --- authoring commands -----------------------------------------
    /// Open a file (recording it into the pending observation buffer),
    /// or search a file/directory with `--grep`.
    Look {
        /// The file to open (plain form), or the file/directory to
        /// search when `--grep` is given.
        path: Option<String>,
        /// A 1-based inclusive line range, `A:B`. Only valid without
        /// `--grep`.
        #[arg(long, value_name = "A:B", conflicts_with = "grep")]
        lines: Option<String>,
        /// Search `path` for `pattern` instead of opening it.
        #[arg(long, value_name = "PATTERN")]
        grep: Option<String>,
    },
    /// Execute a command, printing and recording its combined output.
    Run {
        /// The command and its arguments.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    /// Mint a fact from the pending observation buffer, or revise an
    /// existing fact's note.
    ///
    /// There is no flag here for supplying a fact's extent or captured
    /// output directly — those come only from a preceding `tetel look`
    /// or `tetel run`.
    Fact {
        /// The fact's note: literal text, `-` for stdin, or `@file`.
        /// Required to mint a fact; also how a `--revise`'s new note is
        /// given.
        #[arg(long, value_name = "TEXT|-|@FILE")]
        note: Option<String>,
        /// Revise this fact's note instead of minting a new fact.
        #[arg(long, value_name = "ID")]
        revise: Option<String>,
        /// Required with `--revise`: why the note is changing. Literal
        /// text, `-` for stdin, or `@file`.
        #[arg(long, value_name = "TEXT|-|@FILE")]
        why: Option<String>,
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

        Command::Look { path, lines, grep } => {
            let session_dir = tetel::session::session_dir(&cli.session);
            if let Err(e) = tetel::session::ensure(&session_dir) {
                eprintln!("tetel: could not create session state: {e}");
                return ExitCode::from(1);
            }
            let result = if let Some(pattern) = grep {
                let Some(root) = path else {
                    eprintln!("tetel: `look --grep <pattern>` requires a path-or-dir");
                    return ExitCode::from(1);
                };
                tetel::observe::look_grep(&session_dir, &pattern, &root)
            } else {
                let Some(path) = path else {
                    eprintln!("tetel: usage: tetel look <path> [--lines A:B] | tetel look --grep <pattern> <path-or-dir>");
                    return ExitCode::from(1);
                };
                let parsed_lines = match lines {
                    Some(spec) => match parse_lines(&spec) {
                        Ok(v) => Some(v),
                        Err(e) => {
                            eprintln!("tetel: invalid --lines {spec:?}: {e}");
                            return ExitCode::from(1);
                        }
                    },
                    None => None,
                };
                tetel::observe::look_path(&session_dir, &path, parsed_lines)
            };
            match result {
                Ok(outcome) => {
                    print!("{}", outcome.printed);
                    ExitCode::from(0)
                }
                Err(e) => {
                    eprintln!("tetel: {e}");
                    ExitCode::from(1)
                }
            }
        }
        Command::Run { command } => {
            let session_dir = tetel::session::session_dir(&cli.session);
            if let Err(e) = tetel::session::ensure(&session_dir) {
                eprintln!("tetel: could not create session state: {e}");
                return ExitCode::from(1);
            }
            match tetel::observe::run_command(&session_dir, &command) {
                Ok(outcome) => {
                    print!("{}", outcome.printed);
                    ExitCode::from(outcome.exit_code.clamp(0, 255) as u8)
                }
                Err(e) => {
                    eprintln!("tetel: {e}");
                    ExitCode::from(1)
                }
            }
        }
        Command::Fact { note, revise, why } => {
            let session_dir = tetel::session::session_dir(&cli.session);
            if let Err(e) = tetel::session::ensure(&session_dir) {
                eprintln!("tetel: could not create session state: {e}");
                return ExitCode::from(1);
            }
            let resolved_note = match &note {
                Some(raw) => match tetel::session::resolve_text_value(raw) {
                    Ok(s) => Some(s),
                    Err(e) => {
                        eprintln!("tetel: error reading --note: {e}");
                        return ExitCode::from(1);
                    }
                },
                None => None,
            };
            if let Some(id) = revise {
                let Some(why) = why else {
                    eprintln!("tetel: fact --revise requires --why");
                    return ExitCode::from(1);
                };
                let resolved_why = match tetel::session::resolve_text_value(&why) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("tetel: error reading --why: {e}");
                        return ExitCode::from(1);
                    }
                };
                let Some(resolved_note) = resolved_note else {
                    eprintln!("tetel: fact --revise requires --note (the new note text)");
                    return ExitCode::from(1);
                };
                match tetel::facts::revise(&session_dir, &id, &resolved_note, &resolved_why) {
                    Ok(()) => {
                        println!("{id} revised.");
                        ExitCode::from(0)
                    }
                    Err(e) => {
                        eprintln!("tetel: {e}");
                        ExitCode::from(1)
                    }
                }
            } else {
                let Some(resolved_note) = resolved_note else {
                    eprintln!("tetel: fact requires --note");
                    return ExitCode::from(1);
                };
                match tetel::facts::mint(&session_dir, &resolved_note) {
                    Ok(fact) => {
                        println!("{} minted.", fact.id);
                        ExitCode::from(0)
                    }
                    Err(e) => {
                        eprintln!("tetel: {e}");
                        ExitCode::from(1)
                    }
                }
            }
        }
    }
}

/// Parse a `--lines A:B` value into a 1-based inclusive `(start, end)`.
fn parse_lines(spec: &str) -> Result<(usize, usize), String> {
    let (a, b) = spec.split_once(':').ok_or_else(|| "expected `A:B`".to_string())?;
    let a: usize = a.trim().parse().map_err(|_| format!("`{a}` is not a number"))?;
    let b: usize = b.trim().parse().map_err(|_| format!("`{b}` is not a number"))?;
    Ok((a, b))
}
