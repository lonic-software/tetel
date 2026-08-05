# Cascade — nothing unsettled fixture

A row→row edge exists here, but both ends are settled, so no cascade
should ever be reported: propagation only starts from an unsettled root.

```tetel
id: OK-A
claim: A foundational claim, verified.
domain: src/a.rs#x
extent: src/a.rs#x
pin: abc123
kind: READING
status: VERIFIED
```

```tetel
id: OK-B
claim: A claim that depends on OK-A, also verified.
domain: src/b.rs#y
extent: src/b.rs#y
pin: abc123
kind: READING
status: VERIFIED
note: Builds on [OK-A].
```
