# Cascade — self-citation fixture

An unsettled row whose own note names its own id must not be treated as a
dependency of itself — this row has no other citer anywhere.

```tetel
id: SC-1
claim: A row whose own note happens to mention its own id.
domain: src/a.rs#x
extent: src/a.rs#x
pin: abc123
kind: READING
status: OWED
note: This is the same row as [SC-1]; nothing else cites it.
```
