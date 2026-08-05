# Demo design doc

A kitchen-sink fixture exercising every branch of `tetel check` at once —
not a realistic document, a demonstration of the output contract.

The gateway answers at 29s [S3-L17], which the retry budget assumes.

```tetel
id: S3-L17
claim: The gateway's health endpoint responds within its SLA.
domain: src/gateway.rs#health
extent: src/gateway.rs#health
pin: 71c0fd1
kind: OBSERVED
run: curl -o /dev/null -s -w '%{time_total}' https://example.invalid/health
value: 31s
status: VERIFIED
note: A timing spike; rerunnable but never verdict-bearing.
```

Every `UnboundedTicket::` occurrence in the ticket module honors the
configured timeout [S3-L8].

```tetel
id: S3-L8
claim: Every UnboundedTicket call in the ticket module honors the configured timeout.
domain: src/ticket.rs
extent: src/ticket.rs
pin: 71c0fd1
kind: READING
status: VERIFIED
```

The build passes on a clean checkout [T-1].

```tetel
id: T-1
claim: cargo test exits 0 on a clean checkout at the pinned commit.
domain: proc: cargo test
extent: proc: cargo test
pin: 71c0fd1
kind: RUN
run: cargo test
value: exit 0
status: VERIFIED
```

The corpus's RUN commands are believed deterministic at their pin, though
this has not been re-run on a second machine [T-8]. Reviewers should not
treat that as settled [!T-8] — the re-run is still owed.

```tetel
id: T-8
claim: The corpus's RUN commands are deterministic at their pin.
domain: external: developer machines used by contributors
extent: external: two independent machines running the suite twice each
pin: 71c0fd1
kind: ATTESTED
run: manual comparison of two independent re-runs
value: matched
date: 2026-08-04
status: OWED
```

This single symbol's inspection is claimed to cover the whole module.

```tetel
id: U-2
claim: The entire module is covered by this single symbol's inspection.
domain: src/ticket.rs
extent: src/ticket.rs#new
pin: 71c0fd1
kind: READING
status: VERIFIED
```

The module's public API is unchanged since the last release [T-9].
