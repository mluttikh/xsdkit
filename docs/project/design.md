# Design

The full design document lives in the repository as
[`DESIGN.md`](https://github.com/mluttikh/xsdkit/blob/main/DESIGN.md). It
reviews the XSD format and **17 implementations across 8 languages**, and lays
out the staged plan this crate follows. What is below is the short version of
the decisions that shaped the API you see.

## Why the component model is the product

Covered in [The component model](../concepts.md): the specification defines
every validation rule against schema components, so a library that exposes only
`validate()` has built the interesting layer and then hidden it. The survey
found this repeatedly — `xsd-parser` builds a codegen-shaped intermediate that
discards validation semantics; `uppsala` builds a real model internally and
exposes only a verdict. Neither can answer *"what may go inside this element,
and can it repeat?"*

## Two types, not one

`SchemaSetBuilder` reads; `Schemas` is the result. They are separate types so
that a `Schemas` cannot exist in an unresolved state — "did you remember to
call `Compile()`?" is not a question you can be asked here, because it is not
representable.

## Ids, not references

The component graph is cyclic, so references would mean `Rc<RefCell<…>>`
throughout, which would cost `Send + Sync` and make every access a runtime
borrow check. Components live in arenas addressed by `Copy` u32 ids, and
`Index` is implemented for each so lookups read naturally.

The cost is that an unresolved reference is representable during loading. It is
paid for with a placeholder sentinel that every `Index` implementation asserts
against in debug builds, and a compile-time pass that must patch every
id-bearing field — a pass that has been the source of three bugs of the same
shape, each fixed by making the walk exhaustive rather than by remembering
harder.

## Every error, not the first

Discussed in [Diagnostics](../diagnostics.md). The decision costs a `Diagnostics`
collection threaded through the loader, and it is worth it.

## Glushkov automata for content models

Content models compile to position automata. Unique Particle Attribution
checking then falls out of the same structure instead of being a separate
analysis, and `possible_children` / `child_repeats` / `child_is_optional` are
answered from the automaton rather than by interpreting the particle tree at
every call. `xs:all` is handled with per-member counters rather than expanding
to `n!` interleavings.

## Datatypes implemented, not delegated

All 14 non-trivial XSD datatypes are implemented in `src/atomic.rs` — the
civil-calendar arithmetic, the ±14-hour timezone partial order, duration
comparison against the four reference dateTimes the specification names.

This was not the first plan. An existing crate was used, defended twice on
review, and then removed on evidence when the W3C suite found it rejecting
`--02-29` as an `xs:gMonthDay`. Conformance went *up* after the removal. The
decision record, including the two answers that were wrong, is kept in
`DESIGN.md` §3.12.4 rather than quietly rewritten.

## Facts, not interpretation

Schema families encode conventions the standard never defined — in `appinfo`,
in attribute names, in type-naming schemes. `xsdkit` exposes the facts and
stops: attribute uses folded down the derivation chain, `fixed` and `default`
values, enumeration facets, `appinfo` kept verbatim, and schema-supplied
values flagged in the PSVI.

Building the interpretation on top is a handful of lines for someone who knows
the convention. A built-in heuristic would be right most of the time and
silently wrong the rest — and a silently wrong answer is worse than none,
because nothing downstream can tell.

## What is deliberately out of scope

**Code generation.** That is [`xsd-parser`](https://crates.io/crates/xsd-parser)'s
job, and it does it well. Permanently out of scope.

**Config and binding generation for a particular downstream reader.** That
belongs in a library of its own, so reading a schema never pulls in
dependencies you did not ask for. Three seams in this crate exist so such a
consumer can be written against it — `Annotation::appinfo` verbatim, the
`possible_children` / `child_repeats` / `child_is_optional` trio, and
`ContentMatcher` — and are not to be removed.

## Roadmap

| | | |
|---|---|---|
| ✅ | Component model, loading, composition | done |
| ✅ | Content automata, UPA | done |
| ✅ | Python bindings, type stubs, encoding detection | done |
| ✅ | Instance validation, typed reading (PSVI) | done |
| ✅ | `redefine` / `override` | done |
| ✅ | XSD 1.1 open content, default attributes, relaxed UPA | done |
| → | **XSD 1.1 assertions and conditional type assignment** | next |
| | Identity constraint enforcement | |
