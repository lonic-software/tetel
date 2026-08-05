# A memo carrying both a tetel row and an evidence ledger

A well-formed tetel row, exercised by check_cli.rs's grammar/subset/etc.
fixtures elsewhere; here only to prove the five existing checks and the
two ledger checks coexist in one report.

```tetel
id: TR-1
claim: A well-formed row, unrelated to the ledger below.
domain: a.rs#f
extent: a.rs#f
pin: p1
kind: READING
status: VERIFIED
```

Rows are against `src/foo.rs` at pin `abc1234` unless stated.

| ID | Proposition | Domain | Extent | Kind | Status |
|---|---|---|---|---|---|
| L-1 | `foo` always returns **4** | `foo`'s body | opened in full | READING | **VERIFIED** |
