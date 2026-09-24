//! Run the shipped verifier over a corpus of subjects: TET-98's harness.
//!
//! Each input line (from `scripts/verifier-eval/tet98_tasks.py`) names a
//! committed snapshot and a mint in it. The subject is built by tetel's own
//! `verify::fact_subject`, or `claims::overlap_for` + `verify::claim_subject`
//! for a claim at the wording and cites the line carries, and verified by
//! `verify::spawn` — the path a mint takes, with nothing re-implemented here.
//!
//! `spawn` appends to `<dir>/verify.log` from a detached thread, and an
//! append is not atomic across threads, so every (subject, draw) gets a
//! scratch dir of its own and the snapshot is never written to. Only the
//! main thread writes the output file.
//!
//! Resumable: a (memo, id, draw) whose last output line is `ok` or `gated`
//! is skipped; anything else (timeout, unavailable, a driver-side error) is
//! run again and its new line supersedes the old.
//!
//!     cargo run --release --example verify_corpus -- \
//!         --tasks claims.jsonl --out candidate.jsonl --scratch scratch/candidate \
//!         --draws 3 --jobs 8 --typed-model typesafe/jev-latest --literals
//!
//! `--dump` writes each task's subject (text and evidence, exactly as the
//! verifier is shown it) to `--out` and verifies nothing: what a hand
//! adjudication reads.
//!
//! `--model` names the check model; unset, it is `CHECK_MODEL`, the model
//! TET-98 measured. Every line records it as `arm_model`.

use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tetel::{claims, verify};

const CHECK_MODEL: &str = "openai/gpt-5.6-luna";
const TIMEOUT_MS: u64 = 300_000;
/// Past the verification's own budget, so a record the thread writes late
/// is not raced by the driver giving up on it.
const DRIVER_WAIT: Duration = Duration::from_secs(420);

struct Args {
    tasks: PathBuf,
    out: PathBuf,
    scratch: PathBuf,
    draws: u32,
    jobs: usize,
    model: String,
    typed_model: Option<String>,
    literals: bool,
    limit: Option<usize>,
    dump: bool,
}

fn args() -> Args {
    let mut a = Args {
        tasks: PathBuf::new(),
        out: PathBuf::new(),
        scratch: PathBuf::new(),
        draws: 1,
        jobs: 4,
        model: CHECK_MODEL.to_string(),
        typed_model: None,
        literals: false,
        limit: None,
        dump: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        let mut v = || it.next().unwrap_or_else(|| panic!("{k} needs a value"));
        match k.as_str() {
            "--tasks" => a.tasks = v().into(),
            "--out" => a.out = v().into(),
            "--scratch" => a.scratch = v().into(),
            "--draws" => a.draws = v().parse().expect("--draws"),
            "--jobs" => a.jobs = v().parse().expect("--jobs"),
            "--model" => a.model = v(),
            "--typed-model" => a.typed_model = Some(v()),
            "--limit" => a.limit = Some(v().parse().expect("--limit")),
            "--literals" => a.literals = true,
            "--dump" => a.dump = true,
            _ => panic!("unknown argument {k}"),
        }
    }
    assert!(!a.tasks.as_os_str().is_empty() && !a.out.as_os_str().is_empty(), "--tasks and --out are required");
    assert!(a.dump || !a.scratch.as_os_str().is_empty(), "--scratch is required");
    a
}

fn subject(task: &Value) -> Result<verify::Subject, String> {
    let s = |k: &str| task[k].as_str().ok_or_else(|| format!("task has no {k}"));
    let dir = Path::new(s("dir")?);
    let id = s("id")?;
    let subject = match s("verb")? {
        "fact" => verify::fact_subject(dir, id).map_err(|e| e.to_string())?,
        "claim" => {
            let cites: Vec<String> = task["cites"]
                .as_array()
                .ok_or("claim task has no cites")?
                .iter()
                .map(|c| c.as_str().unwrap_or_default().to_string())
                .collect();
            let overlap = claims::overlap_for(dir, &cites).map_err(|e| e.to_string())?;
            verify::claim_subject(dir, id, s("prop")?, &cites, &overlap, 0).map_err(|e| e.to_string())?
        }
        v => return Err(format!("unknown verb {v}")),
    };
    // `fact_subject` answers an unknown id with empty text rather than an
    // error; verifying "" would be scored as a clean pass.
    if subject.text.trim().is_empty() || subject.evidence.is_empty() {
        return Err(format!("empty subject for {id}"));
    }
    Ok(subject)
}

fn key(task: &Value, draw: u64) -> String {
    format!("{}\u{0}{}\u{0}{}", task["memo"].as_str().unwrap_or(""), task["id"].as_str().unwrap_or(""), draw)
}

fn main() {
    let a = args();
    let settings = verify::Settings {
        enabled: true,
        model: Some(a.model.clone()),
        approach: "split".to_string(),
        timeout_ms: TIMEOUT_MS,
        verbs: vec!["claim".to_string(), "fact".to_string()],
        literals: a.literals,
        // Off in every arm: the rows this run is compared against had none.
        refuter: None,
        typed_model: a.typed_model.clone(),
        typed_default_without_key: false,
        model_refusal: None,
        typed_model_refusal: None,
    };

    let tasks: Vec<Value> = BufReader::new(fs::File::open(&a.tasks).expect("open --tasks"))
        .lines()
        .map(|l| serde_json::from_str(&l.expect("read --tasks")).expect("task line is JSON"))
        .take(a.limit.unwrap_or(usize::MAX))
        .collect();

    if a.dump {
        let mut out = fs::File::create(&a.out).expect("create --out");
        for t in &tasks {
            let line = match subject(t) {
                Ok(s) => json!({"memo": t["memo"], "id": t["id"], "text": s.text, "evidence": s.evidence}),
                Err(e) => json!({"memo": t["memo"], "id": t["id"], "error": e}),
            };
            writeln!(out, "{line}").expect("write --out");
        }
        return;
    }

    let mut done: HashMap<String, String> = HashMap::new();
    if let Ok(f) = fs::File::open(&a.out) {
        for l in BufReader::new(f).lines().map_while(Result::ok) {
            let v: Value = serde_json::from_str(&l).expect("out line is JSON");
            let status = v["record"]["status"].as_str().unwrap_or("driver_error").to_string();
            done.insert(key(&v, v["draw"].as_u64().unwrap()), status);
        }
    }

    let mut queue: VecDeque<(usize, u64)> = VecDeque::new();
    for (i, t) in tasks.iter().enumerate() {
        for d in 0..a.draws as u64 {
            match done.get(&key(t, d)).map(String::as_str) {
                Some("ok") | Some("gated") => {}
                _ => queue.push_back((i, d)),
            }
        }
    }
    let total = queue.len();
    eprintln!("{} to run ({} tasks x {} draws, rest already done)", total, tasks.len(), a.draws);

    let mut out = OpenOptions::new().create(true).append(true).open(&a.out).expect("open --out");
    let mut emit = |task: &Value, draw: u64, record: Value, cost: &mut f64| {
        *cost += record["cost"].as_f64().unwrap_or(0.0);
        let mut line = task.clone();
        line["draw"] = json!(draw);
        line["arm_model"] = json!(a.model);
        line["arm_typed_model"] = json!(a.typed_model);
        line["arm_literals"] = json!(a.literals);
        line["record"] = record;
        writeln!(out, "{line}").expect("write --out");
        out.flush().expect("flush --out");
    };

    let mut in_flight: Vec<(usize, u64, PathBuf, Instant)> = Vec::new();
    let (mut finished, mut cost) = (0usize, 0.0f64);
    while !queue.is_empty() || !in_flight.is_empty() {
        while in_flight.len() < a.jobs {
            let Some((i, d)) = queue.pop_front() else { break };
            let t = &tasks[i];
            let dir = a
                .scratch
                .join(t["memo"].as_str().unwrap())
                .join(t["id"].as_str().unwrap())
                .join(d.to_string());
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).expect("scratch dir");
            let started = match subject(t) {
                Ok(s) => verify::spawn(&dir, &settings, s).then_some(()).ok_or("spawn returned false (no key or no model)".to_string()),
                Err(e) => Err(e),
            };
            match started {
                Ok(()) => in_flight.push((i, d, dir, Instant::now())),
                Err(e) => {
                    emit(t, d, json!({"status": "driver_error", "detail": e}), &mut cost);
                    finished += 1;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(500));
        let mut still = Vec::new();
        for (i, d, dir, at) in in_flight.drain(..) {
            let log = dir.join("verify.log");
            // A line without its newline may still be mid-write.
            let line = fs::read_to_string(&log).ok().and_then(|s| s.split_once('\n').map(|(l, _)| l.to_string()));
            match line {
                Some(l) => {
                    emit(&tasks[i], d, serde_json::from_str(&l).expect("record is JSON"), &mut cost);
                    finished += 1;
                    if finished % 25 == 0 || finished == total {
                        eprintln!("{finished}/{total}  ${cost:.3}");
                    }
                }
                None if at.elapsed() > DRIVER_WAIT => {
                    emit(&tasks[i], d, json!({"status": "driver_timeout"}), &mut cost);
                    finished += 1;
                }
                None => still.push((i, d, dir, at)),
            }
        }
        in_flight = still;
    }
    eprintln!("done: {finished} records, ${cost:.3} reported by this invocation");
}
