# Conformance

`xsdkit` is measured against the **W3C XML Schema Test Suite** — the same
corpus Xerces and Saxon are measured against, contributed by NIST, Microsoft,
IBM, Sun, Boeing and Saxonica. It runs on every change, and the numbers below
are regenerated rather than remembered.

## Schemas

5,727 scored cases: a schema, and whether it should be accepted.

| | |
|---|---|
| valid schemas accepted | **99.7%** (5,231 / 5,247) |
| invalid schemas rejected | **61.7%** (296 / 480) |
| overall correct | **96.5%** (5,527 / 5,727) |

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

The 198 invalid schemas still accepted are concentrated in three areas:
particle subsumption (the full *Derivation Valid (Restriction, Complex)* rule),
XSD 1.1 assertions, and conditional type assignment. The first is a matter of
finishing the remaining cases; the last two need an XPath 2.0 subset.

### The 16 false rejections

Sixteen schemas that should load and do not, across `Missing` (4), `VC` (3),
`Assert` (2), `Override` (2), `Simple` (2), and one each in `IRI`,
`introspection` and `suntest`. Each is a bug rather than a design limit, and
the list is short enough to be worked through.

## Documents

21,575 scored cases: a schema, a document, and whether the document is valid.

| | |
|---|---|
| valid documents accepted | **99.5%** (11,849 / 11,907) |
| invalid documents rejected | **95.1%** (9,197 / 9,668) |
| overall correct | **97.5%** (21,046 / 21,575) |

Here the two rows are much closer, because validating a document against a
model you already built is the part that is finished.

The 58 remaining false alarms are not 58 separate bugs. Grouped by the
diagnostic we wrongly emit:

| Diagnostic | Documents | Cause |
|---|---|---|
| `XSD2002` unexpected element | 34 | Conditional type assignment, `openContent` and `xs:all` — all three features this version does not claim |
| `XSD2001` element not declared | 6 | Scattered |
| `XSD2006` missing attribute, `XSD2008` bad `xsi:type` | 10 | Five each |
| `XSD2005` attribute not allowed | 4 | Scattered |
| `XSD2004` invalid value, `XSD2003` incomplete content | 4 | Scattered |

What is left is now mostly the declared gaps rather than defects: the largest
group is one this version says up front it does not implement.

This table used to be dominated by 112 `xlink:href` documents, written off as
a harness gap on the grounds that the suite's catalogue schema imports
`xlink.xsd` over the network and we decline to fetch it. That reading was
wrong. They were failing because an `xs:anyAttribute` written inside an
`xs:attributeGroup` never reached the types that referenced the group — a
plain bug, now fixed, and the documents pass without any schema being
supplied to the harness.

The false *acceptances* are dominated by the two unimplemented XSD 1.1
features — assertions and conditional type assignment — and by identity
constraints (`xs:key`, `xs:keyref`, `xs:unique`), which are read into the
model but not enforced.

## Running it yourself

The suite is 231 MB and is not vendored. Point `XSDTESTS` at a clone and it
runs; leave it unset and those tests skip.

```bash
git clone --depth 1 https://github.com/w3c/xsdtests /tmp/xsdtests
export XSDTESTS=/tmp/xsdtests

# schemas — about a second
cargo test --test w3c_suite -- --nocapture

# documents — about four minutes; ignored by default because of it
cargo test --release --test w3c_suite -- --ignored --nocapture
```

Both print the tables above plus a breakdown of the worst test sets, so a
change that helps one area and hurts another is visible immediately rather
than hidden behind a single percentage.

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
