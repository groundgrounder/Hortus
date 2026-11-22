# Class diagram regression samples

All files in this directory are self-written public fixtures dedicated to CC0-1.0.

`domain-model.mmd` covers implicit and explicit classes, block and inline members,
visibility prefixes, `*`/`$` modifiers, generics, stereotypes, all eight relation
kinds with cardinalities and a self relation, a class note, and `classDef`/`class`.
`namespaces-lr.mmd` covers `direction LR`, two `namespace` groups, cross-group
relations, a self relation, a note, and `classDef` on selected classes. They are
used by `crates/hortus-diagram/tests/class_er.rs` and
`tools/check-class-er-diagrams.py` at 700 px and 1100 px in both themes.
