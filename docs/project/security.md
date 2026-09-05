# Security

Schemas arrive from elsewhere as often as documents do. A schema is a document
that names other documents and describes patterns to be compiled — three
attack surfaces before anything is validated.

## No network, by default

`FileResolver` refuses `http://` and `https://` outright.

```text
error[XSD1101]: refusing to fetch `http://www.w3.org/2001/xml.xsd` over the network;
                supply a resolver or a local copy
```

`schemaLocation` is a *hint*; the specification is explicit that a processor
may ignore it. Following one over the network turns loading a schema into
fetching and trusting a remote document, makes builds depend on someone else's
uptime, and leaks the fact that you are processing that schema.

If you want network fetching, supply a [`Resolver`](../loading.md#a-resolver)
and own the decision — including the timeout, the allowlist and the cache.

## No external entities

This is not a setting to get wrong. `roxmltree` performs no I/O, so an external
entity cannot be fetched no matter what the document asks for.

Internal DTD subsets **are** accepted, because real schemas use them — the
W3C's own schema for schemas among them — with entity-reference-loop detection
closing the billion-laughs vector.

## Bounded work

Every unbounded thing has a bound.

| Bound | Default | What it stops |
|---|---|---|
| `nodes_limit` | 10,000,000 | A single document exhausting memory |
| Include nesting depth | fixed | An include chain that never ends |
| Cycle guards | — | Circular includes, derivations, substitutions, structural cycles |

```python
schemas = xsdkit.SchemaSet.from_file("untrusted.xsd", nodes_limit=100_000)
```

Cycles in a schema are legal and common — a type may contain an element of its
own type — so they are detected rather than forbidden. Every graph walk in the
library carries a guard; the one that did not, a self-referential `xs:list`
`itemType`, was a stack overflow and is now a checked error with a regression
test.

## Patterns are transpiled, not passed through

XSD's pattern language is not PCRE. Patterns are transpiled to the `regex`
crate, which has no backtracking and therefore no catastrophic-backtracking
class of denial of service — a pattern is linear in the input, whatever it
looks like. A pattern is also one of the fuzz targets.

## Fuzzed

Four `cargo-fuzz` targets, seeded from the W3C corpus:

| Target | Surface |
|---|---|
| `load_schema` | Arbitrary bytes into the loader |
| `xsd_regex` | Arbitrary patterns into the transpiler |
| `parse_value` | Arbitrary lexical forms into all 50 datatypes |
| `validate_instance` | Arbitrary XML into the validator, against a real schema |

CI builds all four on every commit and smoke-runs each for 30 seconds; longer
campaigns are run locally. Every finding has a named regression test rather
than only a corpus entry — a crash that is only remembered by a binary blob is
a crash that comes back.

Findings so far have included an `i128` overflow comparing distant dateTimes, a
character-boundary panic slicing a malformed `gMonthDay`, and an
`unreachable!()` reached by `P8TH`. All are fixed and pinned.

## No `unsafe`

`#![forbid(unsafe_code)]` at the crate root. The Python bindings go through
PyO3, which contains the unsafety at a reviewed boundary rather than spreading
it through the library.

## Reporting

Security issues can be reported through
[GitHub's private vulnerability reporting](https://github.com/mluttikh/xsdkit/security/advisories/new)
on the repository.
