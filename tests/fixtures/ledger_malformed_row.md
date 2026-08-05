# A memo whose evidence ledger has one row missing a cell

Rows are against `src/foo.rs` at pin `abc1234` unless stated.

| ID | Proposition | Domain | Extent | Kind | Status |
|---|---|---|---|---|---|
| M-1 | `foo` always returns **4** | `foo`'s body | opened in full | READING |
| M-2 | `bar` never blocks the caller | `bar`'s body | opened in full | READING | **VERIFIED** |
