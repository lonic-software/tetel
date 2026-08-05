# Cascade — pure row-to-row edge fixture

Ordinary prose that never mentions either row directly.

```tetel
id: R-ROOT
claim: The root claim now known to be refuted.
domain: src/a.rs#x
extent: src/a.rs#x
pin: abc123
kind: READING
status: REFUTED
```

```tetel
id: R-DEP
claim: A verified-looking claim that quietly depends on the refuted one.
domain: src/b.rs#y
extent: src/b.rs#y
pin: abc123
kind: READING
status: VERIFIED
note: Built directly on top of [R-ROOT].
```
