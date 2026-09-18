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

Internal DTD subsets **are** accepted in schema documents, because real schemas
use them — the W3C's own schema for schemas among them — with
entity-reference-loop detection closing the billion-laughs vector. Instance
documents get no DTD processing at all: referencing an entity one declares is
`XSD1001`, rather than a value quietly missing its text.

## Bounded work

Every unbounded thing has a bound.

| Bound | Default | What it stops |
|---|---|---|
| `nodes_limit` | 10,000,000 | A single schema document exhausting memory |
| `max_depth` | 256 | A schema document nested deeply enough to overflow the stack |
| Instance nesting | 10,000, fixed | A document being validated nested past what the reader counts |
| Include nesting depth | 64, fixed | An include chain that never ends |
| Cycle guards | — | Circular includes, derivations, substitutions, structural cycles |

```python,ignore
schemas = xsdkit.SchemaSet.from_file("untrusted.xsd", nodes_limit=100_000, max_depth=64)
```

Nesting is bounded because parsing recurses. The XML parser descends one native
stack frame per level, and a stack overflow is not an error anyone can catch —
it aborts the process. So a schema document's nesting is measured *before* it
is parsed, counting any markup its internal DTD subset could insert through an
entity reference, and a document deeper than `max_depth` is refused with
`XSD1001`. 256 is libxml2's default for the same reason. It leaves room on a
1 MiB stack in a release build, where the deepest-recursing construct, nested
anonymous types, reached about 875 levels; an unoptimised build spends several
times as much stack per level.

Instance documents are validated with a stack of the validator's own and never
recurse, but the XML reader counts nesting in 16 bits and resolves namespaces
against the wrong scopes past 65,535 levels. A document nested deeper than
10,000 is refused rather than misread.

Cycles in a schema are legal and common — a type may contain an element of its
own type — so they are detected rather than forbidden. Every graph walk in the
library carries a guard.

## Patterns are transpiled, not passed through

XSD's pattern language is not PCRE. Patterns are transpiled to the `regex`
crate, which has no backtracking and therefore no catastrophic-backtracking
class of denial of service — a pattern is linear in the input, whatever it
looks like. A pattern is also one of the fuzz targets.

The `regex` crate does cap how large a compiled expression may grow, so a
valid pattern such as `(a{1000}){1000}` can be refused. That is reported as
`XSD1104` at the type that declared it — an error by default, a warning under
`lax` — rather than dropped, since a pattern that is not enforced is a type
that accepts more than it says.

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

## No `unsafe`

`#![forbid(unsafe_code)]` at the crate root. The Python bindings go through
PyO3, which contains the unsafety at a reviewed boundary rather than spreading
it through the library.

## Reporting

Security issues can be reported through
[GitHub's private vulnerability reporting](https://github.com/mluttikh/xsdkit/security/advisories/new)
on the repository.
