# Grammar check — fail fixture

An unknown field (`bogus`) makes this row a grammar refusal.

```tetel
id: G-1
claim: The gateway validates every request header.
domain: src/gateway.rs#validate
extent: src/gateway.rs#validate
pin: abc123
kind: READING
status: VERIFIED
bogus: this field does not exist in the grammar
```
