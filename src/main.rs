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
    /// Assert a claim resting on one or more facts, or revise/withdraw
    /// an existing claim.
    Claim {
        /// The claim's proposition: literal text, `-` for stdin, or
        /// `@file`.
        #[arg(long, value_name = "TEXT|-|@FILE")]
        prop: Option<String>,
        /// Comma-separated fact ids the claim rests on.
        #[arg(long, value_name = "F1,F3")]
        from: Option<String>,
        /// Revise this claim instead of creating a new one.
        #[arg(long, value_name = "ID")]
        revise: Option<String>,
        /// Withdraw this claim instead of creating a new one.
        #[arg(long, value_name = "ID")]
        withdraw: Option<String>,
        /// Required with `--revise`/`--withdraw`: why. Literal text,
        /// `-` for stdin, or `@file`.
        #[arg(long, value_name = "TEXT|-|@FILE")]
        why: Option<String>,
    },
    /// Append a paragraph or heading block to the document's prose, or
    /// revise an existing block.
    ///
    /// The block's own text has no flag: give it with `--text`, or omit
    /// `--text` to read it from stdin (the default).
    Prose {
        /// The paragraph's text: literal text, `-` for stdin, or
        /// `@file`. Omit to read from stdin.
        #[arg(long, value_name = "TEXT|-|@FILE")]
        text: Option<String>,
        /// Mint a heading instead of a paragraph, at `--level`'s depth.
        /// Literal text, `-` for stdin, or `@file`.
        #[arg(long, value_name = "TEXT|-|@FILE")]
        heading: Option<String>,
        /// The heading's markdown depth, 1..=6. Required with `--heading`.
        #[arg(long)]
        level: Option<u8>,
        /// Comma-separated claim ids this paragraph cites.
        #[arg(long, value_name = "C1,C4")]
        cite: Option<String>,
        /// Revise this block's text instead of creating a new one.
        #[arg(long, value_name = "ID")]
        revise: Option<String>,
        /// Required with `--revise`: why. Literal text, `-` for stdin,
        /// or `@file`.
        #[arg(long, value_name = "TEXT|-|@FILE")]
        why: Option<String>,
    },
    /// Assemble the session's current prose into markdown on stdout.
    /// The only authoring command that produces the finished document.
    Render,
    /// Plain, greppable, read-only inspection of facts, claims, prose,
    /// and dependency links. Never refuses.
    Query {
        #[command(subcommand)]
        what: QueryCommand,
    },
}

#[derive(Subcommand)]
enum QueryCommand {
    /// List every fact.
    Facts,
    /// List every claim.
    Claims,
    /// List every prose block, in document order.
    Prose,
    /// What a fact or claim id rests on and is cited by.
    Deps { id: String },
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
        Command::Claim { prop, from, revise, withdraw, why } => {
            let session_dir = tetel::session::session_dir(&cli.session);
            if let Err(e) = tetel::session::ensure(&session_dir) {
                eprintln!("tetel: could not create session state: {e}");
                return ExitCode::from(1);
            }
            let resolve = |raw: &str| -> Result<String, ExitCode> {
                tetel::session::resolve_text_value(raw).map_err(|e| {
                    eprintln!("tetel: error reading text: {e}");
                    ExitCode::from(1)
                })
            };

            if let Some(id) = withdraw {
                let Some(why) = why else {
                    eprintln!("tetel: claim --withdraw requires --why");
                    return ExitCode::from(1);
                };
                let why = match resolve(&why) {
                    Ok(s) => s,
                    Err(code) => return code,
                };
                return match tetel::claims::withdraw(&session_dir, &id, &why) {
                    Ok(()) => {
                        println!("{id} withdrawn.");
                        ExitCode::from(0)
                    }
                    Err(e) => {
                        eprintln!("tetel: {e}");
                        ExitCode::from(1)
                    }
                };
            }

            if let Some(id) = revise {
                let Some(why) = why else {
                    eprintln!("tetel: claim --revise requires --why");
                    return ExitCode::from(1);
                };
                let why = match resolve(&why) {
                    Ok(s) => s,
                    Err(code) => return code,
                };
                let new_prop = match &prop {
                    Some(raw) => match resolve(raw) {
                        Ok(s) => Some(s),
                        Err(code) => return code,
                    },
                    None => None,
                };
                return match tetel::claims::revise(&session_dir, &id, new_prop.as_deref(), from.as_deref(), &why) {
                    Ok(()) => {
                        println!("{id} revised.");
                        ExitCode::from(0)
                    }
                    Err(e) => {
                        eprintln!("tetel: {e}");
                        ExitCode::from(1)
                    }
                };
            }

            let Some(prop) = prop else {
                eprintln!("tetel: claim requires --prop");
                return ExitCode::from(1);
            };
            let prop = match resolve(&prop) {
                Ok(s) => s,
                Err(code) => return code,
            };
            let Some(from) = from else {
                eprintln!("tetel: claim requires --from");
                return ExitCode::from(1);
            };
            match tetel::claims::create(&session_dir, &prop, &from) {
                Ok(outcome) => {
                    println!(
                        "OVERLAP REPORT (facts sharing a designator with {from}, excluding those cited):"
                    );
                    if outcome.overlap.is_empty() {
                        println!("  (none)");
                    } else {
                        for (id, note) in &outcome.overlap {
                            println!("  {id}: {note}");
                        }
                    }
                    println!("{} created (overlap report showed {} fact(s)).", outcome.claim.id, outcome.overlap.len());
                    ExitCode::from(0)
                }
                Err(e) => {
                    eprintln!("tetel: {e}");
                    ExitCode::from(1)
                }
            }
        }
        Command::Prose { text, heading, level, cite, revise, why } => {
            let session_dir = tetel::session::session_dir(&cli.session);
            if let Err(e) = tetel::session::ensure(&session_dir) {
                eprintln!("tetel: could not create session state: {e}");
                return ExitCode::from(1);
            }

            if let Some(id) = revise {
                let Some(why) = why else {
                    eprintln!("tetel: prose --revise requires --why");
                    return ExitCode::from(1);
                };
                let why = match tetel::session::resolve_text_value(&why) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("tetel: error reading --why: {e}");
                        return ExitCode::from(1);
                    }
                };
                let new_text = match tetel::session::resolve_text_or_stdin(text.as_deref()) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("tetel: error reading new text: {e}");
                        return ExitCode::from(1);
                    }
                };
                return match tetel::prose::revise(&session_dir, &id, &new_text, &why) {
                    Ok(()) => {
                        println!("{id} revised.");
                        ExitCode::from(0)
                    }
                    Err(e) => {
                        eprintln!("tetel: {e}");
                        ExitCode::from(1)
                    }
                };
            }

            if let Some(heading_raw) = heading {
                let Some(level) = level else {
                    eprintln!("tetel: prose --heading requires --level");
                    return ExitCode::from(1);
                };
                let heading_text = match tetel::session::resolve_text_value(&heading_raw) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("tetel: error reading --heading: {e}");
                        return ExitCode::from(1);
                    }
                };
                return match tetel::prose::create(&session_dir, &heading_text, None, Some(level)) {
                    Ok(block) => {
                        println!("{} appended.", block.id);
                        ExitCode::from(0)
                    }
                    Err(e) => {
                        eprintln!("tetel: {e}");
                        ExitCode::from(1)
                    }
                };
            }

            let body = match tetel::session::resolve_text_or_stdin(text.as_deref()) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("tetel: error reading prose text: {e}");
                    return ExitCode::from(1);
                }
            };
            match tetel::prose::create(&session_dir, &body, cite.as_deref(), None) {
                Ok(block) => {
                    println!("{} appended.", block.id);
                    ExitCode::from(0)
                }
                Err(e) => {
                    eprintln!("tetel: {e}");
                    ExitCode::from(1)
                }
            }
        }
        Command::Render => {
            let session_dir = tetel::session::session_dir(&cli.session);
            match tetel::compose::render(&session_dir) {
                Ok(out) => {
                    print!("{out}");
                    ExitCode::from(0)
                }
                Err(e) => {
                    eprintln!("tetel: error rendering: {e}");
                    ExitCode::from(1)
                }
            }
        }
        Command::Query { what } => {
            let session_dir = tetel::session::session_dir(&cli.session);
            let result = match what {
                QueryCommand::Facts => tetel::query::facts_text(&session_dir),
                QueryCommand::Claims => tetel::query::claims_text(&session_dir),
                QueryCommand::Prose => tetel::query::prose_text(&session_dir),
                QueryCommand::Deps { id } => tetel::query::deps_text(&session_dir, &id),
            };
            match result {
                Ok(out) => {
                    print!("{out}");
                    ExitCode::from(0)
                }
                Err(e) => {
                    eprintln!("tetel: error querying: {e}");
                    ExitCode::from(1)
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
