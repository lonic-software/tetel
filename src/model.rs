//! Data model for a `tetel` evidence row: the fields a `key: value` block
//! inside a ```tetel fence can carry, and the designator forms that
//! `domain`/`extent` are built from.

use std::fmt;

/// The field names a row may carry. Anything else is an "unknown field"
/// grammar refusal.
pub const ALLOWED_FIELDS: &[&str] = &[
    "id", "claim", "domain", "extent", "pin", "kind", "run", "value", "date", "status", "note",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Run,
    Reading,
    Observed,
    Attested,
}

impl Kind {
    pub fn parse(s: &str) -> Option<Kind> {
        match s {
            "RUN" => Some(Kind::Run),
            "READING" => Some(Kind::Reading),
            "OBSERVED" => Some(Kind::Observed),
            "ATTESTED" => Some(Kind::Attested),
            _ => None,
        }
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Kind::Run => "RUN",
            Kind::Reading => "READING",
            Kind::Observed => "OBSERVED",
            Kind::Attested => "ATTESTED",
        };
        write!(f, "{s}")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Verified,
    Owed,
    Refuted,
}

impl Status {
    pub fn parse(s: &str) -> Option<Status> {
        match s {
            "VERIFIED" => Some(Status::Verified),
            "OWED" => Some(Status::Owed),
            "REFUTED" => Some(Status::Refuted),
            _ => None,
        }
    }

    pub fn is_unsettled(&self) -> bool {
        matches!(self, Status::Owed | Status::Refuted)
    }
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Status::Verified => "VERIFIED",
            Status::Owed => "OWED",
            Status::Refuted => "REFUTED",
        };
        write!(f, "{s}")
    }
}

/// One entry inside a comma-separated `domain` or `extent` field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Designator {
    /// `path#symbol` — one symbol inside a file.
    Symbol { path: String, symbol: String },
    /// `path` — a whole file; covers `path#*` in the same file.
    Path(String),
    /// `proc: <command>` — the set is a command's output at the pin.
    Proc(String),
    /// `external: <free text>` — outside any checkable set.
    External(String),
}

impl Designator {
    /// Whether this designator is one of the two enumerated forms the
    /// subset check (D3) can reason about.
    pub fn is_enumerated(&self) -> bool {
        matches!(self, Designator::Symbol { .. } | Designator::Path(_))
    }

    fn parse_one(raw: &str) -> Result<Designator, String> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err("empty designator".to_string());
        }
        if let Some(rest) = trimmed.strip_prefix("proc:") {
            let rest = rest.trim();
            if rest.is_empty() {
                return Err("`proc:` designator has no command".to_string());
            }
            return Ok(Designator::Proc(rest.to_string()));
        }
        if let Some(rest) = trimmed.strip_prefix("external:") {
            let rest = rest.trim();
            if rest.is_empty() {
                return Err("`external:` designator has no text".to_string());
            }
            return Ok(Designator::External(rest.to_string()));
        }
        if let Some((path, symbol)) = trimmed.split_once('#') {
            if path.is_empty() || symbol.is_empty() {
                return Err(format!("malformed `path#symbol` designator: {trimmed:?}"));
            }
            return Ok(Designator::Symbol {
                path: path.to_string(),
                symbol: symbol.to_string(),
            });
        }
        Ok(Designator::Path(trimmed.to_string()))
    }

    /// Parse a comma-separated field into designators, collecting every
    /// malformed entry rather than stopping at the first.
    ///
    /// Known limitation, not solved in this slice: a `proc:`/`external:`
    /// designator's free text may itself contain a comma, which this
    /// splitter cannot distinguish from a second designator. See the
    /// report for detail.
    pub fn parse_list(raw: &str) -> Result<Vec<Designator>, Vec<String>> {
        let mut out = Vec::new();
        let mut errs = Vec::new();
        let mut any_part = false;
        for part in raw.split(',') {
            if part.trim().is_empty() {
                continue;
            }
            any_part = true;
            match Designator::parse_one(part) {
                Ok(d) => out.push(d),
                Err(e) => errs.push(e),
            }
        }
        if !any_part {
            errs.push("designator field is empty".to_string());
        }
        if errs.is_empty() {
            Ok(out)
        } else {
            Err(errs)
        }
    }
}

/// A parsed `tetel` evidence row plus the source line it started on, for
/// error messages.
#[derive(Debug, Clone)]
pub struct Row {
    /// 1-based line number of the row's first field line.
    pub line: usize,
    pub id: String,
    pub claim: String,
    pub domain: Vec<Designator>,
    pub extent: Vec<Designator>,
    #[allow(dead_code)] // opaque per the grammar; carried for future re-execution use
    pub pin: String,
    pub kind: Kind,
    #[allow(dead_code)] // not re-executed in this slice; kept for fidelity to the row
    pub run: Option<String>,
    pub value: Option<String>,
    #[allow(dead_code)] // not compared against anything in this slice
    pub date: Option<String>,
    pub status: Status,
    /// Read by the dependency-cascade check (row→row citation edges),
    /// alongside `claim`; otherwise free prose.
    pub note: Option<String>,
}

impl Row {
    /// True if every designator in both `domain` and `extent` is one of
    /// the two enumerated forms — the only rows the subset check (check 2)
    /// can reason about. Rows failing this are skipped for check 2 and
    /// listed in the human-owed partition as "coverage not machine-checked".
    pub fn fully_enumerated(&self) -> bool {
        self.domain.iter().all(Designator::is_enumerated)
            && self.extent.iter().all(Designator::is_enumerated)
    }
}
