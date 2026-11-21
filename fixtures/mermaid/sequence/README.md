# Sequence diagram regression samples

All files in this directory are self-written public fixtures dedicated to CC0-1.0.
`agent-workflow.mmd` shows a four-participant agent workflow with solid messages,
dashed replies and a self-call. `control-flow.mmd` covers `autonumber`, `actor`, activation
shorthand and statements, all four note placements, `alt`/`else`, `loop`, `opt`,
`par`/`and`, `critical`/`option`, `break` and the `-x` arrow. `narrow-wrap.mmd`
exercises long mixed CJK/English labels at 700 px and 1100 px. `syntax-errors.mmd`
keeps one representative unclosed-`alt` error for the CLI failure path; further
error cases are covered by unit tests in `crates/hortus-diagram/src/sequence.rs`.
