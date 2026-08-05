# Cascade — dangling row-field citation fixture

A row's own note can cite an id that was never defined; that must be
reported informationally, the same way an undefined prose citation is,
and must never fail the run on its own.

```tetel
id: DG-1
claim: A verified row whose note cites a row that does not exist.
domain: src/a.rs#x
extent: src/a.rs#x
pin: abc123
kind: READING
status: VERIFIED
note: See also [GHOST-9], which was never defined.
```
