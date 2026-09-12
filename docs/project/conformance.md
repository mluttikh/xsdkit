# Conformance

`xsdkit` is measured against the **W3C XML Schema Test Suite** — the same
corpus Xerces and Saxon are measured against, contributed by NIST, Microsoft,
IBM, Sun, Boeing and Saxonica. It runs on every change, and the numbers below
are regenerated rather than remembered.

## Schemas

5,725 scored cases: a schema, and whether it should be accepted. Two more are
recorded and not scored, because the working group marked its own expectation
for them `queried`.

| | |
|---|---|
| valid schemas accepted | **99.7%** (5,231 / 5,247) |
| invalid schemas rejected | **66.5%** (318 / 478) |
| overall correct | **96.9%** (5,549 / 5,725) |

**The gap between those two rows is the honest description of this library.**

Reading a schema and judging a schema are different jobs. `xsdkit` does the
first one very well: of 5,247 schemas that a conforming processor must accept,
it accepts all but 16. It does the second one partially: it enforces some of
the specification's *validity constraints* and not others, so **a schema
`xsdkit` accepts is not thereby a valid schema**.

If you need a conformance checker — you are validating schemas people submit
to you, or certifying a schema before publishing it — use Xerces or Saxon. If
you need to read a schema that already works, which is the overwhelmingly
common case, this is built for exactly that.

The 160 invalid schemas still accepted cluster by test set as `All` (25),
`Simple` (13), `Wild` (11), `CTA` (11), `Open` (8), `suntest` (7), and then a
tail of six or fewer. Three areas cover most of it: particle subsumption (the
full *Derivation Valid (Restriction, Complex)* rule), XSD 1.1 assertions, and
conditional type assignment. The first is a matter of finishing the remaining
cases; the last two need an XPath 2.0 subset.

### The 16 false rejections

Sixteen schemas that should load and do not, across `Missing` (4), `VC` (3),
`Assert` (2), `Override` (2), `Simple` (2), and one each in `IRI`,
`introspection` and `suntest`. By the diagnostic that refuses them: ten
`XSD1201` unresolved reference, two `XSD1308` invalid value constraint (`+INF`
as a default), and four that report a second code alongside `XSD1201`. Each is
a bug rather than a design limit, and the list is short enough to be worked
through.

## Documents

21,573 scored cases: a schema, a document, and whether the document is valid.
Fifty more are recorded and not scored: a schema that does not compile says
nothing about the document, and two are `queried`.

| | |
|---|---|
| valid documents accepted | **99.5%** (11,844 / 11,905) |
| invalid documents rejected | **98.4%** (9,517 / 9,668) |
| overall correct | **99.0%** (21,361 / 21,573) |

Here the two rows are much closer, because validating a document against a
model you already built is the part that is finished.

The 61 remaining false alarms are not 61 separate bugs. Grouped by the
diagnostics we wrongly emit — counted from `tests/conformance/instance-cases.tsv`,
which records the codes for every case, so this table is read off the gate
rather than assembled by hand:

| Diagnostics | Documents | Cause |
|---|---|---|
| `XSD2002`+`XSD2003` unexpected element, then incomplete content | 33 | Conditional type assignment, `openContent` and `xs:all` — all three features this version does not claim |
| `XSD2008` bad `xsi:type` | 6 | Scattered; one reports `XSD2001` too |
| `XSD2001` element not declared | 5 | Scattered |
| `XSD2006` missing required attribute | 5 | Scattered |
| `XSD2005` attribute not allowed | 5 | Scattered |
| `XSD2004` invalid value | 3 | Scattered |
| `XSD2011` duplicate `xs:ID` | 2 | The deliberate disagreement below |
| `XSD2003` incomplete content, `XSD2016` unresolved `keyref` | 2 | One each; the second is an identity constraint under a `lax` wildcard |

What is left is now mostly the declared gaps rather than defects: the largest
group is one this version says up front it does not implement.

This table used to be dominated by 112 `xlink:href` documents, written off as
a harness gap on the grounds that the suite's catalogue schema imports
`xlink.xsd` over the network and we decline to fetch it. That reading was
wrong. They were failing because an `xs:anyAttribute` written inside an
`xs:attributeGroup` never reached the types that referenced the group — a
plain bug, now fixed, and the documents pass without any schema being
supplied to the harness.

Two of the false *alarms* are a deliberate disagreement. `saxonData/Id`'s
`id003.v01` and `id004.v01` put the same `xs:ID` value on two sibling elements
and expect valid, while their own negative counterparts add a third element
carrying that value and expect invalid. No rule that counts the elements bound
to a value satisfies both, so this follows the specification and reports the
duplicate.

The false *acceptances* are what is left of the same measurement, taken by
test set rather than assumed. Identity constraints (`xs:key`, `xs:keyref`,
`xs:unique`, and `xs:ID` uniqueness) are the largest group, at about 80; they
are read into the model and not enforced. XSD 1.1 **assertions** account for
around 70 and **conditional type assignment** for 13 — both stored,
unevaluated, and both waiting on an XPath 2.0 subset.

Until recently the biggest group was none of those: 150 documents from the
NIST datatype sets, which wrap the element under test in
`<xs:any processContents="strict"/>`. Every wildcard behaved as `skip`, so
nothing inside one was checked and those tests passed vacuously on the valid
side while failing to catch anything on the invalid side. `processContents`
is now honoured, which is where the jump in this row comes from.

## Running it yourself

The suite is 231 MB and is not vendored. Point `XSDTESTS` at a clone and it
runs; leave it unset and those tests skip.

```bash
git clone --depth 1 https://github.com/w3c/xsdtests /tmp/xsdtests
XSDTESTS=/tmp/xsdtests cargo test --test w3c_suite -- --nocapture
```

Both halves run, in about twenty seconds together. They print the tables above
plus a breakdown of the worst test sets, but the gate is
`tests/conformance/*.tsv`: one row per case, holding the version it was run
as, what the suite expects, our verdict and the diagnostic codes. A change
that helps one area and hurts another shows up as two rows rather than a
percentage that did not move, and CI fails on any row that differs. The
figures above are that file's header.

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
- **The schema for schemas.** XSD's own schema is a fixture, because it uses
  nearly every feature and no synthetic test exercises the combinations it
  does.
- **A performance guard** on the shape of the loader's scaling — see
  [Performance](performance.md).
