# Abutting-literal check — multi-token value pass fixture

The retry loop finished after 29s [M-1].

```tetel
id: M-1
claim: The retry loop's total wall-clock time is bounded.
domain: src/retry.rs#run
extent: src/retry.rs#run
pin: abc123
kind: OBSERVED
run: time ./retry-loop
value: elapsed 29s
status: VERIFIED
```
