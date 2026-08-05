# Cascade — two-hop fixture

The retry loop attempts three times before giving up [DIRECT-1].

The upstream client depends on that retry behavior [MID-1].

```tetel
id: DIRECT-1
claim: The retry loop attempts three times before giving up.
domain: src/gateway.rs#retry
extent: src/gateway.rs#retry
pin: abc123
kind: READING
status: OWED
```

```tetel
id: MID-1
claim: The upstream client depends on the retry loop's behavior.
domain: src/client.rs#call
extent: src/client.rs#call
pin: abc123
kind: READING
status: VERIFIED
note: Timing assumption traced back to [DIRECT-1].
```
