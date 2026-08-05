# Cascade — citation cycle fixture

Ordinary prose with no citations of either cyclic row.

```tetel
id: CYC-A
claim: Cycle participant A, refuted.
domain: src/a.rs#x
extent: src/a.rs#x
pin: abc123
kind: READING
status: REFUTED
note: Paired with [CYC-B] by construction.
```

```tetel
id: CYC-B
claim: Cycle participant B, verified but paired with a refuted row.
domain: src/b.rs#y
extent: src/b.rs#y
pin: abc123
kind: READING
status: VERIFIED
note: Paired with [CYC-A] by construction.
```
