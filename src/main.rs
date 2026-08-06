use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

/// tetel — every factual claim carries executable evidence.
#[derive(Parser)]
#[command(name = "tetel", version, about)]
struct Cli {
    /// Which authoring workspace's state to use (see `tetel::workspace`
    /// for where that state lives). Irrelevant to `check`/`brief`/
    /// `record`, which read a memo already on disk instead.
    #[arg(long, global = true, default_value = "default")]
    workspace: String,

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
        /// The memo to brief. Omit when `--authoring` is given.
        memo: Option<PathBuf>,
        /// Emit machine-readable JSON instead of the human-readable form.
        #[arg(long)]
        json: bool,
        /// Emit the authoring rhythm brief instead of a grounding brief
        /// for a memo — the instructions handed to whoever is about to
        /// write a document with `tetel look`/`run`/`fact`/`claim`/
        /// `prose`/`render`. Takes no memo.
        #[arg(long)]
        authoring: bool,
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
        /// Backticks in the text must come via `-` or `@file`, never
        /// inline — the shell eats them first. Required to mint a fact;
        /// also how a `--revise`'s new note is given.
        #[arg(long, value_name = "TEXT|-|@FILE")]
        note: Option<String>,
        /// Revise this fact's note instead of minting a new fact.
        #[arg(long, value_name = "ID")]
        revise: Option<String>,
        /// Required with `--revise`: why the note is changing. Literal
        /// text, `-` for stdin, or `@file`. Backticks in the text must
        /// come via `-` or `@file`, never inline — the shell eats them
        /// first.
        #[arg(long, value_name = "TEXT|-|@FILE")]
        why: Option<String>,
    },
    /// Assert a claim resting on one or more facts, or revise/withdraw
    /// an existing claim.
    Claim {
        /// The claim's proposition: literal text, `-` for stdin, or
        /// `@file`. Backticks in the text must come via `-` or `@file`,
        /// never inline — the shell eats them first.
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
        /// `-` for stdin, or `@file`. Backticks in the text must come
        /// via `-` or `@file`, never inline — the shell eats them first.
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
        /// `@file`. Backticks in the text must come via `-` or `@file`,
        /// never inline — the shell eats them first. Omit to read from
        /// stdin.
        #[arg(long, value_name = "TEXT|-|@FILE")]
        text: Option<String>,
        /// Mint a heading instead of a paragraph, at `--level`'s depth.
        /// Literal text, `-` for stdin, or `@file`. Backticks in the
        /// text must come via `-` or `@file`, never inline — the shell
        /// eats them first.
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
        /// or `@file`. Backticks in the text must come via `-` or
        /// `@file`, never inline — the shell eats them first.
        #[arg(long, value_name = "TEXT|-|@FILE")]
        why: Option<String>,
    },
    /// Assemble the workspace's current prose into markdown on stdout.
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
        Command::Brief { memo, json, authoring } => {
            if authoring {
                print!("{}", tetel::brief::AUTHORING_BRIEF);
                return ExitCode::from(0);
            }
            let Some(memo) = memo else {
                eprintln!("tetel: `brief` requires a memo, or `--authoring`");
                return ExitCode::from(1);
            };
            match tetel::brief_file(&memo, json) {
                Ok((code, out)) => {
                    print!("{out}");
                    ExitCode::from(code as u8)
                }
                Err(e) => {
                    eprintln!("tetel: error reading {}: {e}", memo.display());
                    ExitCode::from(1)
                }
            }
        }
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
            let workspace_dir = match tetel::workspace::open(&cli.workspace) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("tetel: could not create workspace state: {e}");
                    return ExitCode::from(1);
                }
            };
            let req = if let Some(pattern) = grep {
                tetel::observe::LookRequest::Grep { pattern, root: path }
            } else {
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
                tetel::observe::LookRequest::Open { path, lines: parsed_lines }
            };
            match tetel::observe::dispatch(&workspace_dir, req) {
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
            let workspace_dir = match tetel::workspace::open(&cli.workspace) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("tetel: could not create workspace state: {e}");
                    return ExitCode::from(1);
                }
            };
            match tetel::observe::run_command(&workspace_dir, &command) {
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
            let workspace_dir = match tetel::workspace::open(&cli.workspace) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("tetel: could not create workspace state: {e}");
                    return ExitCode::from(1);
                }
            };
            let resolved_note = match &note {
                Some(raw) => match tetel::workspace::resolve_text_value(raw) {
                    Ok(s) => Some(s),
                    Err(e) => {
                        eprintln!("tetel: error reading --note: {e}");
                        return ExitCode::from(1);
                    }
                },
                None => None,
            };
            let req = if let Some(id) = revise {
                let resolved_why = match &why {
                    Some(raw) => match tetel::workspace::resolve_text_value(raw) {
                        Ok(s) => Some(s),
                        Err(e) => {
                            eprintln!("tetel: error reading --why: {e}");
                            return ExitCode::from(1);
                        }
                    },
                    None => None,
                };
                tetel::facts::FactRequest::Revise { id, note: resolved_note, why: resolved_why }
            } else {
                tetel::facts::FactRequest::Mint { note: resolved_note }
            };
            match tetel::facts::dispatch(&workspace_dir, req) {
                Ok(tetel::facts::FactOutcome::Minted(fact)) => {
                    println!("{} minted.", fact.id);
                    ExitCode::from(0)
                }
                Ok(tetel::facts::FactOutcome::Revised { id }) => {
                    println!("{id} revised.");
                    ExitCode::from(0)
                }
                Err(e) => {
                    eprintln!("tetel: {e}");
                    ExitCode::from(1)
                }
            }
        }
        Command::Claim { prop, from, revise, withdraw, why } => {
            let workspace_dir = match tetel::workspace::open(&cli.workspace) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("tetel: could not create workspace state: {e}");
                    return ExitCode::from(1);
                }
            };
            let resolve = |raw: &str| -> Result<String, ExitCode> {
                tetel::workspace::resolve_text_value(raw).map_err(|e| {
                    eprintln!("tetel: error reading text: {e}");
                    ExitCode::from(1)
                })
            };
            let resolve_opt = |raw: &Option<String>| -> Result<Option<String>, ExitCode> {
                match raw {
                    Some(s) => resolve(s).map(Some),
                    None => Ok(None),
                }
            };

            let req = if let Some(id) = withdraw {
                let why = match resolve_opt(&why) {
                    Ok(v) => v,
                    Err(code) => return code,
                };
                tetel::claims::ClaimRequest::Withdraw { id, why }
            } else if let Some(id) = revise {
                let why = match resolve_opt(&why) {
                    Ok(v) => v,
                    Err(code) => return code,
                };
                let prop = match resolve_opt(&prop) {
                    Ok(v) => v,
                    Err(code) => return code,
                };
                tetel::claims::ClaimRequest::Revise { id, prop, from, why }
            } else {
                let prop = match resolve_opt(&prop) {
                    Ok(v) => v,
                    Err(code) => return code,
                };
                tetel::claims::ClaimRequest::Create { prop, from }
            };

            match tetel::claims::dispatch(&workspace_dir, req) {
                Ok(tetel::claims::ClaimOutcome::Withdrawn { id }) => {
                    println!("{id} withdrawn.");
                    ExitCode::from(0)
                }
                Ok(tetel::claims::ClaimOutcome::Revised { id }) => {
                    println!("{id} revised.");
                    ExitCode::from(0)
                }
                Ok(tetel::claims::ClaimOutcome::Created(outcome)) => {
                    println!(
                        "OVERLAP REPORT (facts sharing a designator with {}, excluding those cited):",
                        outcome.claim.from.join(",")
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
            let workspace_dir = match tetel::workspace::open(&cli.workspace) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("tetel: could not create workspace state: {e}");
                    return ExitCode::from(1);
                }
            };

            let req = if let Some(id) = revise {
                let why = match &why {
                    Some(raw) => match tetel::workspace::resolve_text_value(raw) {
                        Ok(s) => Some(s),
                        Err(e) => {
                            eprintln!("tetel: error reading --why: {e}");
                            return ExitCode::from(1);
                        }
                    },
                    None => None,
                };
                let new_text = match tetel::workspace::resolve_text_or_stdin(text.as_deref()) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("tetel: error reading new text: {e}");
                        return ExitCode::from(1);
                    }
                };
                tetel::prose::ProseRequest::Revise { id, text: new_text, why }
            } else if let Some(heading_raw) = heading {
                let heading_text = match tetel::workspace::resolve_text_value(&heading_raw) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("tetel: error reading --heading: {e}");
                        return ExitCode::from(1);
                    }
                };
                tetel::prose::ProseRequest::Heading { text: heading_text, level }
            } else {
                let body = match tetel::workspace::resolve_text_or_stdin(text.as_deref()) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("tetel: error reading prose text: {e}");
                        return ExitCode::from(1);
                    }
                };
                tetel::prose::ProseRequest::Paragraph { text: body, cite }
            };

            match tetel::prose::dispatch(&workspace_dir, req) {
                Ok(tetel::prose::ProseOutcome::Revised { id }) => {
                    println!("{id} revised.");
                    ExitCode::from(0)
                }
                Ok(tetel::prose::ProseOutcome::Created(block)) => {
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
            let workspace_dir = tetel::workspace::workspace_dir(&cli.workspace);
            match tetel::compose::render(&workspace_dir) {
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
            let workspace_dir = tetel::workspace::workspace_dir(&cli.workspace);
            let result = match what {
                QueryCommand::Facts => tetel::query::facts_text(&workspace_dir),
                QueryCommand::Claims => tetel::query::claims_text(&workspace_dir),
                QueryCommand::Prose => tetel::query::prose_text(&workspace_dir),
                QueryCommand::Deps { id } => tetel::query::deps_text(&workspace_dir, &id),
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
