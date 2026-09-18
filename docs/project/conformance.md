# Conformance

`xsdkit` is measured against the **W3C XML Schema Test Suite** — the same
corpus Xerces and Saxon are measured against, contributed by NIST, Microsoft,
IBM, Sun, Boeing and Saxonica. It runs on every change, and the numbers below
are regenerated rather than remembered.

## Schemas

10,511 runs: a schema, the language it is read as, and whether it should be
accepted. Every group the suite prescribes for **both** versions is run as
each, so most schemas are here twice; two more cases are recorded and not
scored, because the working group marked its own expectation for them
`queried`.

| | XSD 1.0 | XSD 1.1 |
|---|---|---|
| valid schemas accepted | **99.8%** (4,563 / 4,573) | **99.8%** (5,238 / 5,248) |
| invalid schemas rejected | **77.4%** (171 / 221) | **69.7%** (327 / 469) |
| overall correct | **98.7%** (4,734 / 4,794) | **97.3%** (5,565 / 5,717) |

The two columns are reported apart because a single percentage across them
would be an average of two different languages, true of neither.

The 1.1 column is lower on the second row because the 1.1-only test sets are
largely the features this version does not claim: assertions, conditional type
assignment and `openContent`.

**The gap between the first two rows is the honest description of this
library.**

Reading a schema and judging a schema are different jobs. `xsdkit` does the
first one very well: of the schemas a conforming processor must accept, it
accepts all but ten in each language. It does the second one partially: it
enforces some of the specification's *validity constraints* and not others, so
**a schema `xsdkit` accepts is not thereby a valid schema**.

If you need a conformance checker — you are validating schemas people submit
to you, or certifying a schema before publishing it — use Xerces or Saxon. If
you need to read a schema that already works, which is the overwhelmingly
common case, this is built for exactly that.

The invalid schemas still accepted are 50 as 1.0 and 142 as 1.1, and the
difference is entirely 1.1 features. As 1.0 they cluster as `Simple` (13),
`suntest` (7), `ElemDecl` (6), `Complex` (6), `MGroup` (4); as 1.1 the same
sets plus `All` (17), `Wild` (11), `CTA` (11), `Open` (8) and
`TypeAlternativeTests` (6). The `All` cluster is worth reading as separate
rules rather than one: of its 17, nine are particle subsumption and six are
*Derivation Valid (Extension)*, both P2-sized. Three areas cover most of it:
particle subsumption (the full *Derivation Valid (Restriction, Complex)* rule),
XSD 1.1 assertions, and conditional type assignment. The first is a matter of
finishing the remaining cases; the last two need an XPath 2.0 subset.

### The twenty false rejections

Ten in each language — schemas that should load and do not. By the diagnostic
that refuses them, six are a bare `XSD1201` unresolved reference in either
version; two more report `XSD1201` alongside `XSD1101` (a resolver declining
to fetch `xlink.xsd` over the network) or `XSD1001` (the entity-reference-loop
guard), and those two are correct behaviour rather than bugs. The remaining
two differ by version: as 1.0 they are `XSD1308` invalid value constraint,
`+INF` as a default, which is a 1.1-only lexical form; as 1.1 they are
`XSD1201`+`XSD1202`, a duplicate global reported alongside the unresolved
reference that caused it.

## Documents

41,994 runs over 21,671 documents: a schema, the language it is read as, a
document, and whether the document is valid. Sixty-four more are recorded and
not scored: a schema that does not compile says nothing about the document,
and two are `queried`.

| | XSD 1.0 | XSD 1.1 |
|---|---|---|
| valid documents accepted | **99.9%** (11,310 / 11,325) | **99.6%** (11,864 / 11,906) |
| invalid documents rejected | **99.8%** (9,065 / 9,083) | **98.4%** (9,525 / 9,680) |
| overall correct | **99.8%** (20,375 / 20,408) | **99.1%** (21,389 / 21,586) |

Here the rows are much closer, because validating a document against a model
you already built is the part that is finished.

The false alarms are 15 as 1.0 and 42 as 1.1, and they are not that many
separate bugs. Grouped by the diagnostics we wrongly emit — counted from
`tests/conformance/instance-cases.tsv`, which records the codes for every run,
so this table is read off the gate rather than assembled by hand:

| Diagnostics | as 1.0 | as 1.1 | Cause |
|---|---|---|---|
| `XSD2002`+`XSD2003` unexpected element, then incomplete content | 1 | 16 | As 1.1, features this version does not claim in full — `openContent` 6, `xs:all` 5, assertions and conditional type assignment 2 each — plus one `ElemDecl` case that fails in both versions |
| `XSD2008` bad `xsi:type` | 5 | 6 | Scattered; one reports `XSD2001` too |
| `XSD2001` element not declared | 5 | 5 | Scattered |
| `XSD2006` missing required attribute | — | 5 | Scattered |
| `XSD2005` attribute not allowed | 1 | 5 | Scattered |
| `XSD2004` invalid value | 3 | 2 | Scattered |
| `XSD2011` duplicate `xs:ID` | — | 3 | Two are the deliberate disagreement below |

As 1.1, the largest group is features this version does not claim in full. As
1.0, the 15 are scattered.

Two of the false *alarms* are a deliberate disagreement. `saxonData/Id`'s
`id003.v01` and `id004.v01` put the same `xs:ID` value on two sibling elements
and expect valid, while their own negative counterparts add a third element
carrying that value and expect invalid. No rule that counts the elements bound
to a value satisfies both, so this follows the specification and reports the
duplicate. Both are 1.1-only groups, which is why that row is empty in the 1.0
column.

The false *acceptances* — invalid documents we pass — are 18 as 1.0 and 155
as 1.1, and by test set they are mostly the two features this version stores
and never evaluates. XSD 1.1 **assertions** are 67 of them (`Assert` 43,
`assertion` 24) and **conditional type assignment** 24 (`CTA` 19,
`TypeAlternativeTests` 5), both waiting on an XPath 2.0 subset. Without
conditional type assignment, an element whose type comes only from
`xs:alternative` is an `xs:anyType`, which accepts any content. After those
come `vc:` conditional inclusion (13) and open content (11), which are
implemented and whose remaining misses have not been examined.

Identity constraints are enforced; three `xs:key`/`xs:unique` documents are
still accepted in each version, and six `xs:ID` cases in 1.1.

## Which rules, rather than how many cases

A percentage over a test suite is not the same as coverage of the
specification, and the suite cannot be made to give the second:
its schema half offers about 220 negative cases per version to share among 66
Schema Component Constraints. [Which rules of the specification are
enforced](spec-rules.md) answers that question directly — all 143 named rules,
generated from Appendix B of each Recommendation, with what `xsdkit` does about
each one.

For the XSD 1.1 features specifically, the suite ships its own taxonomy and
`cargo run --example w3c_features` scores against it.

## Running it yourself

The suite is 231 MB and is not vendored. Point `XSDTESTS` at a checkout of
the commit `tests/conformance/SUITE` pins and it runs; leave it unset and those
tests skip.

```bash
scripts/fetch-w3c-suite.sh /tmp/xsdtests
XSDTESTS=/tmp/xsdtests cargo test --test w3c_suite -- --nocapture
```

Both halves run, in about thirty-five seconds together. They print the tables
above plus a breakdown of the worst test sets, but the gate is
`tests/conformance/*.tsv`: one row per run, keyed by the case and the version
it was read as, holding what the suite expects, our verdict and the diagnostic
codes. A change that helps one area and hurts another shows up as two rows
rather than a percentage that did not move, and CI fails on any row that
differs. The figures above are those files' headers.

The instance cases run a second time through the Python package, against the
same baseline, in about half a minute:

```bash
pip install . && XSDTESTS=/tmp/xsdtests python3 scripts/check-w3c-python.py
```

Every verdict and every error code must match the Rust run. So must what
`iter_typed`, `read_typed` and `decode` make of each document, and nothing but
an `XsdError` may escape. That is where a binding differs from the crate it
wraps — a value it cannot convert, a report it assembles differently — and
the Rust harness cannot see it.

## Why report the failures

A conformance number with no denominator is marketing. The suite is the only
independent oracle that exists for XSD, it is unforgiving, and every
implementation that has been measured against it fails part of it. Publishing
which part is what lets you decide whether the gap matters for what you are
doing — which is a decision you can only make with the numbers in front of you.

## What else is tested

- **Unit and integration tests** across the loader, the component model,
  content automata, derivation, facets, restriction and instance validation.
- **Fuzzing** — four `cargo-fuzz` targets covering the loader, the pattern
  transpiler, value parsing and instance validation, seeded from the W3C
  corpus. Every finding has a named regression test.
- **One fixture pair per rule.** For each rule in [the spec-rule
  table](spec-rules.md) that has them, the smallest schema that breaks it and a
  near-miss that must still load — the half the suite cannot supply.
- **The schema for schemas.** XSD's own schema is a fixture, because it uses
  nearly every feature and no synthetic test exercises the combinations it
  does.
- **A performance guard** on the shape of the loader's scaling — see
  [Performance](performance.md).
