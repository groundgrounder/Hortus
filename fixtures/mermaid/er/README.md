# Entity-relationship diagram regression samples

All files in this directory are self-written public fixtures dedicated to CC0-1.0.

`order-schema.mmd` covers four entities, attribute types with length and array
syntax, `PK`/`FK`/`UK` markers and comma combinations, quoted comments, every
crow's-foot cardinality, solid identifying and dashed non-identifying relations,
and a duplicate entity pair. `inventory-lr.mmd` covers `direction LR`, aliased
entities, a title, multiple relations between one pair, and a self relation.
They are used by `crates/hortus-diagram/tests/class_er.rs` and
`tools/check-class-er-diagrams.py` at 700 px and 1100 px in both themes.
