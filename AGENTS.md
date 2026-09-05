# AI Contributor Guide: xsdkit

> **This is the canonical AI/agent guide.** `CLAUDE.md` and `GEMINI.md` are
> thin pointers to this file — edit **this file only**, so all AI tools stay
> in sync.

## Project Overview

- **Crate:** `xsdkit` is a **generic XSD reader** — it parses W3C XML Schema
  into a queryable **schema component model**. Python bindings are the next
  phase and are first-class, not an afterthought.
- **Stack:** Rust (edition 2024), `roxmltree` (schema documents),
  `encoding_rs`, `fxhash`, `pyo3` (behind the `python` feature), and
  `quick-xml` later for instance reading.

**Two XML libraries, on purpose.** Schema loading needs random access —
lookahead into children before deciding, three passes over one subtree,
`appinfo` captured verbatim — and runs once per schema, so it uses
`roxmltree`. Instance validation (P4) is strictly forward over unbounded
input, so it will use `quick-xml`, as `xml2arrow` does. Same split
CodeSynthesis ships deliberately (C++/Tree vs C++/Parser). The cost is near
zero: `roxmltree` has no transitive dependencies. The risk to watch is that
the two libraries have subtly different XML-level semantics — entity handling,
whitespace, DTD acceptance — so where that matters it must be a documented
contract, not an accident.
- **Not in this crate:** the `xml2arrow` YAML generator is a **separate
  package** (`xsd2arrow`). Never add an `arrow` or `xml2arrow` dependency
  here — someone with an XSD problem and no interest in Arrow must still want
  this crate.
- **Units are out, extraction included.** No standard says where a unit lives
  in an XSD, so any built-in detection is a heuristic — right most of the
  time, silently wrong the rest, and a wrong unit is worse than no unit. The
  crate exposes attribute uses, `fixed`/`default` values, enumerations and
  `appinfo`; `examples/units.rs` shows the fifteen lines that turn those into
  one schema family's convention. Do not add a `units` module.
- **Design rationale:** `DESIGN.md`. Read Part I before touching the model —
  most XSD bugs come from not knowing the spec, not from bad Rust.

**Core philosophy:**
- **The component model is the product.** Validation, unit extraction and
  config generation are consumers of it. Nothing may shortcut back into the
  document syntax.
- **Compile once, query many.** `Schemas` is immutable and reusable.
- **Collect diagnostics, never bail on the first.** A schema author fixing a
  40-file import graph needs the whole list.
- **Correctness over cleverness:** silently wrong components are the worst
  failure mode. Prefer a structured diagnostic over a guess.

## Architecture & Data Flow

Four phases, in strict order:

1. **Load** (`load.rs`) — `roxmltree` reads each document into components.
   References are **not** resolved here: a `ref`/`base`/`type` becomes a
   placeholder id plus an entry in `Loader::fixups`. This linker split is the
   only thing that copes with circular import graphs.
2. **Compile** (`compile.rs`) — five ordered steps, each finishing before the
   next begins: `resolve_references` → `merge_attribute_groups` →
   `resolve_simple_content` → `check_cycles` → `build_substitution_closure`.
   Order is load-bearing; attribute groups cannot flatten before their
   references resolve.
3. **Content** (`content.rs`) — every complex type's particle tree compiles
   to a Glushkov position automaton. Runs against the *assembled* `Schemas`
   so it can expand substitution groups while building.
4. **Query** (`model.rs`, `content.rs`) — `Schemas`, immutable, `Send + Sync`.

`SchemaSetBuilder` and `Schemas` are separate types so an unresolved
`Schemas` is not representable — .NET needs an `IsCompiled` flag because C#
cannot express this.

## Critical Rules (MUST follow)

### 1. Arenas and ids
- Components live in `Arena<T>` and are named by `Copy` u32 ids. **Never**
  introduce `Rc`/`RefCell` — schemas are cyclic graphs and it would poison
  every signature with lifetimes and leak.
- `Id::PLACEHOLDER` is `u32::MAX` and exists **only** between load and
  resolve. `prune_placeholders` guarantees none reaches a `Schemas`; the
  `Index` impls `debug_assert!` on it. If you add a reference kind, add its
  `Fixup` variant *and* its pruning.

### 2. Diagnostics
- Use `Diagnostic` / `DiagCode` — never stringly-typed errors. **`DiagCode`
  string values are permanent**; adding a variant is fine, renumbering is
  not.
- Every diagnostic needs a `Span`. A diagnostic without a source location is
  a bug report the user cannot act on.
- Errors and warnings are both collected; `Conformance::Lax` downgrades
  violations that still permit building components.

### 3. Content models
- **UPA is automaton determinism.** A content model is 1-unambiguous exactly
  when no state has two out-transitions with overlapping labels. Do not write
  a separate UPA checker; extend the overlap test instead.
- **Occurrence ranges are unrolled, not counted.** `a{2,4}` becomes three
  positions. This keeps the automaton ordinary — no counter machinery — and
  it is what makes `a{2,2}, a` correctly *not* a UPA breach, where collapsing
  bounds to `+` would report a false positive.
- **Two budgets, for two different blowups.** `MAX_UNROLL` (64 copies) bounds
  the *quadratic* cost of unrolling; `MAX_POSITIONS` (4096) bounds total model
  size and is deliberately far looser, because a flat sequence of hundreds of
  distinct elements is ordinary. Past either, the range is widened to
  unbounded and `approximated` is set, which downgrades UPA findings on that
  model to warnings. Widening only ever *adds* reachable positions, so an
  approximated model accepts a superset — false positives, never false
  negatives.
- **Extension appends, restriction replaces — for *content*.** 
  `effective_particles` walks the base chain and stops at the first
  restriction step.
- **Attributes are different: *both* derivation methods inherit them.**
  Extension adds uses; restriction may only narrow one already present or
  remove it with `use="prohibited"`. `merge_inherited_attributes` folds the
  chain, and an own use replaces the inherited one for the same name. Getting
  this wrong is invisible on synthetic schemas and fatal on real ones: GML's
  whole measure family is *vacuous* extensions of `gml:MeasureType`, so every
  measure type reported no `uom` at all. Building from a
  type's own particle alone silently loses every inherited child; a real
  schema catches this immediately (`xs:keyref` adds only an attribute).
- **`xs:all` gets counters, not an automaton.** Interleaving `n` members is
  `n!` paths as a regex; per-member counts are smaller and are what the spec
  actually describes.
- The matcher simulates an NFA rather than determinising, so a model that
  breaches UPA still matches — which is what `Conformance::Lax` needs.
- **A failed step must not move the matcher.** Open content can accept a child
  the declared model rejects, so `step_automaton` leaves `active` alone when
  no position matches. Assigning an empty `active` and *then* trying open
  content loses the position the next child needs — the model accepted the
  wildcard and then rejected everything after it.
- **A wildcard must match names the schema never interned.** That is what
  wildcards are *for*, so matching cannot go through `QName` alone: an
  un-interned URI is not `None` (which means *no* namespace), it is simply
  not one of the enumerated ones. `NamespaceConstraint::admits_uri` and
  `ContentMatcher::step_foreign` are that path; the validator takes it
  whenever `Schemas::qname` returns `None`.
- **XSD 1.1 is opt-in** via `Version::Xsd11`, and it *relaxes* rules as well
  as adding syntax — an element competing with a wildcard is a UPA breach in
  1.0 and legal in 1.1. Default is 1.0: the stricter reading, and what most
  shipping schemas are.
- **Open content lives beside the model, not in the automaton.** Interleaved
  open content is the shuffle of the declared language with the wildcard's,
  which a position automaton cannot express — but the matcher decides it in
  one extra check.

### 4. Behavioral contracts (pinned by tests — do not "fix" these)
- **`whiteSpace` applies before lexical parsing.** It is the only difference
  between `xs:string` and `xs:token`, and why `<v> 42 </v>` parses as
  `xs:int`.
- **Patterns OR within a restriction step, AND across steps.**
  `FacetSet::patterns` is `Vec<Vec<String>>` for exactly this reason.
- **The innermost enumeration wins**; a restriction may only narrow.
- **Chameleon includes** key the document cache on `(uri, coerced_ns)`, never
  on `uri` alone. The same file yields different components per includer.
- **Inside `xs:redefine`, a reference to the name being redefined means the
  *original*.** `<complexType name="T"><extension base="T">` extends the
  included T, not the one being declared. `capture_originals` snapshots them
  and `pin_self_references` resolves those fixups immediately, before the new
  component takes the name. `xs:override` has no such rule — its references
  mean the new components — which is why the two share a reader but differ in
  one argument.
- **The `xml:` prefix is bound implicitly**, and `xml:lang`/`space`/`base`/`id`
  are predeclared. A schema must not need to fetch `xml.xsd`.
- **Redeclaring a built-in is not a duplicate-global error.** The
  schema-for-schemas declares all 50; ours win so `Schemas::builtin` stays a
  stable handle.
- **Local declarations are not globally addressable.** `{name, ns}` keys
  global components only; locals carry `Scope::Local(TypeId)`.
- **Seven symbol spaces.** `Foo` the type and `Foo` the element never collide.
- **Internal DTD subsets are accepted**; external entities are impossible
  because `roxmltree` performs no I/O. Do not "harden" this by rejecting
  DTDs — it would reject the W3C's own schema.

### 9. Security
- Network fetching is **opt-in**: `FileResolver` refuses `http(s)://`.
- Every graph walk needs a bound: `MAX_DEPTH` for includes, `nodes_limit` per
  document, cycle guards in `base_chain` and `check_cycles`.

## Public API & Compatibility

`src/lib.rs` re-exports are the public API. `SchemaSetBuilder`, `Schemas`,
`Diagnostics`, `Resolver` and the component types are the surface to keep
stable. Component structs have public fields on purpose — this is a model to
be read. Adding a field is breaking for struct-literal construction; call it
out in the PR.

## Conformance

`tests/w3c_suite.rs` runs the **W3C XML Schema Test Suite** — 5,737 schema
cases from NIST, Microsoft, IBM, Sun, Boeing and Saxonica. It is 231 MB and
not vendored; point `XSDTESTS` at a clone of
<https://github.com/w3c/xsdtests> and it runs, otherwise it skips.

```bash
git clone --depth 1 https://github.com/w3c/xsdtests /tmp/xsdtests
XSDTESTS=/tmp/xsdtests cargo test --test w3c_suite -- --nocapture
```

Two numbers, and the gap between them is the honest description of this
crate:

| | |
|---|---|
| valid schemas accepted | **99.7%** — it reads real schemas |
| invalid schemas rejected | **58.8%** — partial: see `src/restriction.rs` |

That asymmetry is by construction, not neglect: the Schema Component
Constraints and the Derivation Valid rules are largely unimplemented (see §7).
A schema this crate accepts is not thereby a *valid* schema. Implemented so
far: the facet rules (`src/facets.rs`), the declaration value-constraint
rules (`src/declarations.rs`), and the Schema Representation Constraints —
annotation placement, `block`/`final` keywords, `default` beside `fixed`, the
shape of a group definition. Those last live in `check_representation` in the
loader, as one sweep of the document tree: they are answerable from the XML
alone, with nothing resolved, and several concern elements the loader
otherwise never visits. `final` is enforced too (`src/derivation.rs`).

`examples/w3c_gap.rs` is how to pick what to implement next: it clusters the
invalid schemas we accept by test-group family, so a family with fifty misses
is fifty cases one rule buys.

The instance half — 21,671 documents — is `#[ignore]`d because it takes
minutes where the schema half takes seconds (4.5 minutes in `--release`; run
it that way). Run it with `-- --ignored`. It scores 21,565 of them, 95.3%
correct: **95.8%** of valid documents accepted, **94.6%** of invalid ones
rejected.

That first figure was 88.1% until a one-line bug turned up: an *enumeration on
a list type* compared its literals as strings against a list, so every such
enumeration rejected every value. Fixing it moved 900 documents, and the whole
run from 90.8% to 95.0%. Worth remembering when a conformance number looks
like a long tail of unrelated failures — 1,160 of the 1,412 false alarms sat
in NIST2004-01-14 alone, and they were all that one bug.

**A test may prescribe different results per version.** `<expected>` can carry
its own `version`, and the tokens on it are **and**ed — the result is
prescribed only for a processor supporting all of them. So the harness picks
the `<expected>` matching the version it is running the group as, falls back to
an unqualified one, and *skips the case entirely* when the only expectation
names the other version. Taking the first `<expected>` regardless, which it
used to do, scored 49 cases against the wrong expectation and inflated the
denominator by 10.

Some groups are unmarked but only make sense in one version, and no scoring
rule fixes that. `saxonData/Simple` is the clearest: `simple001` expects `+INF`
to be **valid** (a 1.1-only lexical form) while `simple004` expects
`final="extension"` on a simple type to be **invalid** (a 1.0-only
prohibition), and neither group declares a version. No single reading satisfies
both, so two cases there are permanently lost. **Do not "fix" that by relaxing
the 1.0 lexical rules** — the suite's own `XSD1_1TestCategories.xml` lists
`xsd1_1-Misc-LexicalRepForFloatAndDouble`, "lexical representation +INF for
float and double", as a 1.1 *feature*. The rule is right; the test data is
unmarked.

The harness carries a floor assertion on acceptance. **Raise it as the number
improves; never lower it silently** — a drop means a schema that used to load
no longer does.

It earns its keep. Its first run found a panic that 226 hand-written tests had
not: a type whose whole content was a dangling group reference. Dangling
particles were pruned from their containers, but a *content* particle hangs
off `ComplexType::content` with nothing to be pruned from.

## Testing Conventions

- `src/*/mod tests` — unit tests for pure logic (facets, interning, codes).
- `tests/integration_tests.rs` — a schema document in, components out. Uses
  the in-memory `MapResolver` so composition tests need no files.
- `tests/real_world.rs` — the W3C schema-for-schemas. **Every bug found on a
  real schema gets a repro here**, in addition to a unit test. Three of the
  first four bugs in this crate were found by loading it once.
- The full suite runs in under a second — run it often.
- `cargo llvm-cov --summary-only` for coverage. 87.4% of regions overall; see
  the road to 1.0 for where the floor is and why it is there.

CI (`.github/workflows/ci.yml`) gates on four things: `cargo fmt --check`,
`cargo clippy --all-targets -D warnings`, tests on Linux/macOS/Windows, and
`cargo doc` with `RUSTDOCFLAGS=-D warnings` — broken intra-doc links are only
warnings otherwise, and two had already crept in. A fourth job compiles on the
declared `rust-version`, because nothing enforces that claim at publish time.
Run all five locally before pushing; they take seconds.

**Check them by exit code, never by grepping output.** `cargo clippy` caches:
a second run on unchanged code prints `Finished` and re-emits nothing, so
`clippy | grep -c warning` reports zero whether or not the code is clean. That
mistake shipped two lint failures to CI. The same trap wearing a different
hat: `cargo check ... | tail -2 && echo OK` reports the exit status of `tail`,
so a chain of gates joined by `&&` prints OK while one of them failed. Pipe to
`/dev/null` and read `$?`. Use what CI uses:

```bash
cargo fmt --check \
  && cargo clippy --all-targets -- -D warnings \
  && cargo clippy --all-targets --features python -- -D warnings \
  && cargo test --all-targets && cargo test --doc \
  && RUSTDOCFLAGS="-D warnings" cargo doc --no-deps \
  && cargo +1.87 check --all-targets  # the rust-version in Cargo.toml
```

CI uses `dtolnay/rust-toolchain@stable`, which tracks the newest stable. A
local toolchain even one release behind will miss lints CI enforces, so
`rustup update stable` before trusting a green local run.

Python: `maturin develop` then `pytest python/tests -q`. On a machine with
conda active, maturin refuses to run while both `VIRTUAL_ENV` and
`CONDA_PREFIX` are set — `env -u CONDA_PREFIX` in front of the command.

Adding a schema feature? Add: a synthetic test for the feature alone, a
failure test for its malformed form, and a check that it survives the real
fixture.

**Raw strings and `##`.** Schema fixtures are written as raw strings, and XSD
is full of `##any`, `##other`, `##local`. An attribute value that opens with
them puts `"##` in the text, which closes an `r##"…"##` literal early — the
errors it produces name a *prefix* or a "reserved multi-hash token" and point
nowhere near the cause. Count the longest run of `#` after a `"` in the
content and use one more: `namespace="##any"` needs `r###"…"###`.

## Fuzzing

`fuzz/` is a `cargo-fuzz` workspace with four targets, run on nightly:

```bash
cargo +nightly fuzz run load_schema -- -max_total_time=300 -timeout=10 -rss_limit_mb=4096
```

- `load_schema` — arbitrary bytes through the loader in both versions under
  `Conformance::Lax`, then `walk_everything`: a traversal of *every* arena,
  calling every public accessor on every component. This is where the value is.
  A target that only checks "did the build panic" finds nothing, because the
  loader is written to answer rather than fail; the walk is what turns
  `Schemas`'s invariants into assertions. Both crashes it found were surviving
  placeholder ids, and neither was reachable from the schema roots.
- `xsd_regex` — arbitrary text as an XSD pattern, compiled and matched.
- `parse_value` — a leading byte selects the builtin, the rest is the lexical
  form; parse, display, `facet_length`, `partial_cmp_value`, canonical reparse.
  Comparison matters as much as parsing: the one finding here was a hang in an
  *ordering*, not in a parse.
- `validate_instance` — arbitrary bytes as an instance document against one
  fixed schema built once in a `OnceLock`, consuming the PSVI through
  `validate_with` and indexing every id it hands out back into the schema.
  Same idea as the walk: the document picks the path through the automaton, so
  the ids reaching a consumer are steered by the input in a way the loader's
  are not.

**Keep the walk honest.** `iter_*()` enumerates the arena, not the reachable
graph, so a component pruned from its container is still handed to callers. Any
new public accessor belongs in `walk_everything`, or the surface it opens is
unfuzzed.

**And index every id-bearing field there, directly.** Three separate bugs have
now been the same shape: `prune_placeholders` repairs the fields someone
remembered, and one nobody walked kept its placeholder into `Schemas` — an
attribute's type, a dangling particle's term, and the four places one type
points at another. The `debug_assert!` in the arena `Index` impls is what
catches it, so a field is only checked if something *indexes* it. Convenience
accessors do not count: `base_chain` guards against a placeholder and stops,
which is exactly why it never saw the third one. **Adding an id to a component
means adding a repair in `prune_placeholders` and an index in
`walk_everything`, in the same change.**

The corpus is not vendored — it is derived from the same W3C suite `XSDTESTS`
points at, and reseeded with:

```bash
find "$XSDTESTS" -name '*.xsd' -size -64k -exec cp {} fuzz/corpus/load_schema/ \;
find "$XSDTESTS" -name '*.xml' -size -64k -exec cp {} fuzz/corpus/validate_instance/ \;
```

`fuzz/.gitignore` keeps `corpus/`, `artifacts/`, `target/` and `coverage/` out
of the repository; only the targets and their manifest are committed. **A crash
gets a named regression test in `tests/` or a `mod tests`, not an artifact file
checked in** — the artifact is an opaque blob, the test says what was wrong.
Fuzz findings are marked `Found by fuzzing` at the fix site.

## The road to 1.0

Ordered by what gets more expensive the longer it waits, not by size.

The 58 valid schemas still rejected break down, by diagnostic code, as: **40**
`precisionDecimal` (27 unresolved `xs:precisionDecimal`, 13 `minScale`), **7**
the identity-constraint `ref` bug (now fixed), **4** UPA false positives (also
fixed), **5** or so
`vc:` conditional inclusion, and **2** that are correct behaviour — the
entity-reference-loop guard firing on `ElementDeclarations.xsd`, and
`FileResolver` refusing to fetch `xlink.xsd` over the network.

Regenerate that with `examples/w3c_why.rs` (why we reject valid schemas, by
code) and `examples/w3c_gap.rs` (which invalid schemas we accept, clustered by
test-group family, so a family with fifty misses is fifty cases one rule buys).
Both need `XSDTESTS`. Re-run them before trusting any number here.

### P0 — the irreversible one

~~**Get `oxsdatatypes` out of the public API.**~~ **Done, and then some** —
the dependency is gone entirely (`src/atomic.rs` implements all 14 datatypes).
The wrapping came first and made the removal possible: with `Value` no longer
naming the library's types, replacing them one at a time was an internal
change rather than a breaking one. What follows is why the wrapping mattered,
kept because the same trap will exist for whatever replaces it.

`Value` is public, so whatever it held was this crate's API, and it held the
library's types directly. Anyone matching on `Value::DateTime(dt)` had to add
`oxsdatatypes` to their own `Cargo.toml` and was pinned to our exact version,
so a patch bump on our side was a breaking change on theirs.

The 14 datatypes are now newtypes that forward to it. **Nothing outside
`src/atomic.rs` may name `oxsdatatypes`** — not even in a `From` impl, which
is why `TimezoneOffset::wrap` is an inherent function rather than the obvious
`From`. A public trait impl naming a foreign type is the same leak wearing a
different hat. `grep -rl oxsdatatypes src/` should list `atomic.rs` and
nothing else but prose.

What the wrappers expose is what a consumer actually does with a parsed value:
`Display` for the canonical form, ordering, and the components — enough to
build a `chrono::DateTime`, an Arrow column or a Python `datetime`. Add to
that list rather than handing out the inner value.

This was never about leaving the library; `DESIGN.md` §3.15.4 says why keeping
it is right. It is that after 1.0 the choice could not be revisited, and now
replacing any one type is an internal change.

### P1 — correctness bugs, all small, all with repros

1. ~~**Identity-constraint `ref`.**~~ **Done.** XSD 1.1 lets a constraint be
   referenced rather than defined (`<unique ref="a:u1"/>`); the loader read
   `name` unconditionally, so the `ref` form got the empty local name and the
   second one collided with the first. Recovered the 7 predicted rejections
   and cost 1 — a schema that is invalid for an unrelated reason was being
   caught by the bogus duplicate, which is a reminder that a rejection being
   *right* is not the same as a rejection being *correct*.
2. ~~**`values::parse` does not know the version.**~~ **Done.** `Schemas` now
   carries the XSD it was read as (`Schemas::xsd_version`), and
   `values::parse_in` takes it. The bare `values::parse` still reads the 1.1
   superset — with no schema in hand there is nothing to say which language
   applies.
3. ~~**Four UPA false positives.**~~ **Done**, and they were two unrelated
   gaps rather than one bug in the checker. `notNamespace` was never parsed,
   so such a wildcard silently became `##any` and two wildcards *partitioning*
   the namespaces looked identical. And `notQName`'s `##defined` /
   `##definedSibling` keywords were dropped, so a wildcard meant to stand
   aside for its named siblings competed with them.

   The second one is why `Content` carries a `siblings` set: "the content
   model this wildcard sits in" is only settled once group references are
   expanded and an extension's base is folded in, so it cannot be resolved at
   load time. `content::wildcard_admits` is the single place that answers
   whether a wildcard takes a name — **add new exclusions there**, not at the
   call sites, of which there are five.

### P2 — particle subsumption

*Derivation Valid (Restriction, Complex)* — partly done, in
`src/restriction.rs`. Read that module's header before touching it: **it is
the one place in this crate that errs towards accepting**, and the
[`Verdict`]-style three-way answer is what makes that safe. A pair it cannot
judge is not a match; treating it as one lets an ordered walk consume a base
particle it never checked, which is how the first version rejected the W3C
schema for schemas.

Implemented: *Occurrence Range OK*, *Elt:Elt NameAndTypeOK*, *Elt:Any
NSCompat*, *Any:Any NSSubset*, *RecurseAsIfGroup*, *Recurse* for
sequence-against-sequence and for anything against an `xs:all`, and
*RecurseLax* for choice-against-choice.

Still unjudged, and each accepted rather than guessed at: *MapAndSum* (a
restriction naming substitution-group members, whose bounds must be summed
across them), *NSRecurseCheckCardinality* (a group restricting a wildcard),
and the remaining cross-compositor pairs.

Two normalisations the rules need and the specification assumes: a base's
content model is its **whole derivation chain**, not the particle it declared
itself, and a same-compositor inline group occurring exactly once is spliced
into its parent. Without either, every type in a derivation chain looks like it
has the wrong number of particles.

### P3 — finishing the hardening pass

4. ~~**Coverage in the two thin modules.**~~ **Largely done.**
   `cargo llvm-cov --summary-only` now puts the crate at **88.9%** of regions,
   with `values.rs` up from 79.7% to **87.5%** — `tests/canonical.rs` covers
   the canonical forms and orderings, which is precisely the half fuzzing
   cannot judge. Ten million runs establish that nothing panics; none of them
   can say whether `--02-29` is a date.

   It found one, and it was in the backend rather than here: see
   `DESIGN.md` §3.15.4 and `src/atomic.rs`. **Write expectations from the
   specification, not from what the code prints** — a test that records
   current output asserts nothing.

   `content.rs` is at 84.0% and is the remaining floor along with
   `atomic.rs` (82.6%, mostly component accessors).
5. ~~**`vc:` conditional inclusion**~~ **Done.** The test is on `reads`, the
   filter every descend point in the loader now uses — *not* at the places
   components are created. An excluded element takes its subtree with it, and
   two alternatives for one name must never both be registered, which is what
   the failures looked like (`duplicate global element`). The conditions may
   also sit on `xs:schema` itself, excluding a whole document; that is checked
   where the root is validated.

   **Any new descend point must use `reads`, not `is_xs_element`** — the latter
   is what `reads` is built from and does not apply the conditions.

### P4 — optional, and genuinely optional

6. ~~Report the `oxsdatatypes` findings upstream.~~ **Dropped**, deliberately
   rather than forgotten: the crate no longer depends on it. The three
   findings stay written down in `DESIGN.md` §3.15.4, since they are the
   evidence for the decision recorded there.

7. ~~**Drop `oxsdatatypes` entirely.**~~ **Done.** All 14 datatypes are in
   `src/atomic.rs`, and the crate's remaining dependencies are `encoding_rs`,
   `fxhash`, `indexmap`, `quick-xml`, `regex`, `regex-syntax` and `roxmltree`.
   Conformance went *up*: instance cases correct 20,490 -> 20,492, invalid
   documents rejected 9,104 -> 9,106.

   Kept below is the reasoning that said not to, because it was right up until
   the evidence changed, and because the same judgement will be needed again.

   The evidence *had* said no (`DESIGN.md` §3.15.4): a direct probe found it
   correct on the parts that are genuinely hard, its only dependency was
   `thiserror`, and the problems this crate hit were one performance bug we
   routed around and two version differences that were ours. What changed the
   answer:

   - **`precisionDecimal`** (item 8) needs a decimal type it does not have.
     That is additive, and writing one turned out to be the natural first step.
   - A finding in the date-time family that is *wrong* rather than slow. **One
     has now fired**: `--02-29` was rejected as an `xs:gMonthDay`, and that
     type is implemented here as a result. Two more of the same kind would be
     a reason to reconsider the whole family; one is a reason to own one type.

   That is what happened, and the method is the part worth keeping: one type
   at a time, behind its existing wrapper, each landing with the suite and the
   fuzzer green. The fuzzer found four bugs in the new code — a char-boundary
   panic, an `unreachable!` on `P8TH`, an `i128` overflow comparing a
   thirteen-digit year, and a stack overflow on a list of itself — and the
   suite found a fifth that no amount of fuzzing would have: **equality on the
   temporal types is the order relation, not the fields**, because
   `13:00+01:00` and `12:00Z` are one instant written two ways.
8. ~~**`precisionDecimal`.**~~ **Done** — `src/atomic.rs`, and
   `tests/precision_decimal.rs` for the semantics, which came from the suite's
   own data rather than from a specification: the type never made it into a
   normative REC. It is optional in XSD 1.1 and this crate has it, so
   `vc:typeAvailable="xs:precisionDecimal"` answers yes.

   Three things it does that nothing else does. It **remembers its scale**, so
   `1.0` and `1.00` are the same number and different values, and `totalDigits`
   counts the digits as *written* — `1.000` has four where an `xs:decimal`
   would say one. Its `minScale` is **signed**, which is why it replaces
   `fractionDigits` rather than joining it. And it has infinities, a NaN and a
   signed zero, which makes it a **primitive in its own right** rather than a
   decimal.

   Was, before this: 40 of the 58 false rejections — 27
   unresolved `xs:precisionDecimal` plus 13 `minScale`/`maxScale` facets. It is
   the largest single cluster. It took the false rejections from 41 to **17**
   and the instance half from 20,492 correct to 20,541.

## Planned, not present

Deliberately out of scope until their phase (see `DESIGN.md` §3.14): XSD 1.1
assertions and conditional type assignment (P5, next). The `xsd2arrow`
package (P6) lives in its own repository. Units were a planned phase and were
**cut on evidence** — see `DESIGN.md` §3.7.

Three seams already exist and must not be removed:
- `Annotation::appinfo` keeps `appinfo` XML **verbatim** — the units layer
  cannot recover a unit from a summary.
- `Schemas::possible_children` / `child_repeats` / `child_is_optional` answer
  the config generator's table-versus-column and nullability questions from
  the automaton, not from a guess over the particle tree.
- `ContentMatcher` is the validator's core loop; P4 adds typed values and
  attributes around it rather than replacing it.

When touching the public API, remember P3 is imminent: every type here is
about to be wrapped in a `#[pyclass]` holding `(Arc<Schemas>, Id)`. Keep
accessors cheap and id-shaped, and keep `Schemas: Send + Sync` so the GIL can
be released around `build()`.

### 6. The Python bindings

The API is a Python API, not a transliterated Rust one. What that has meant in
practice, since each was a real complaint:

- **`SchemaSet` is a mapping** — `len`, `in`, `[]`, iteration — over the
  globals *the documents declare*. The XSD built-ins are filtered out of all
  of it, by namespace rather than by `Schemas::as_builtin`, which reads
  `SimpleType::builtin` and so cannot see the complex `xs:anyType`. `type()`
  still resolves them: the mapping is "what this schema says", the lookup
  methods are "resolve this name".
- **Every handle needs `__eq__` and `__hash__`.** Without them a `#[pyclass]`
  falls back to identity, so two lookups of one declaration are unequal and
  hash apart — a set of them silently holds duplicates and nothing raises.
  That is worse than being unhashable.
- **Anything document-shaped takes `str` or `bytes`**, and anything
  path-shaped goes through `os.fspath`. A caller has a `Path` and a file read
  in binary; refusing either is friction with no upside.
- **Iterators, not callbacks.** `iter_typed` composes with `enumerate`,
  `itertools` and generator expressions; `on_event=` composes with nothing.
  A return type that changes with an argument — `list | None` decided by a
  keyword — is worse still.
- **Every Rust knob needs a keyword.** `version=` was missing for a long time,
  which made the whole XSD 1.1 implementation unreachable from Python without
  anyone noticing.

- Every wrapper is `(Arc<Schemas>, Id)`. **Never copy components into Python
  objects** — handles must stay free so a schema with thousands of globals
  costs nothing to walk.
- `#[pyclass(frozen)]` everywhere: the model is immutable, and `frozen` gives
  `Sync` and skips runtime borrow checks.
- **Release the GIL** with `py.detach()` around every `build()`. That is why
  `Resolver: Send + Sync` — do not remove those bounds.
- Adding a `DiagCode` variant needs no binding change (codes are rendered as
  strings), but adding a **pyclass member** does: `python/xsdkit/_xsdkit.pyi`
  must gain it, and `test_stubs.py` fails if it does not.
- `SchemaError.diagnostics` has a class-level default so
  `except SchemaError as e: e.diagnostics` is always safe. The stub test found
  that; keep it.
- **Release the GIL only where no Python is called.** `validate()` detaches;
  `read_typed()` cannot, because every event becomes a Python object and
  `on_event` is Python code.
- **Values convert to native types, not strings.** That is most of what the
  binding is for. `xs:duration` and the gregorian fragments stay lexical
  because no lossless Python type exists — months and seconds are not
  commensurable — but everything else maps: `Decimal`, tz-aware `datetime`,
  `date`, `time`, `timedelta`, `bytes`, `list`.
- A pyclass holding `Py<PyAny>` cannot derive `Clone`; write it by hand with
  `Python::attach` and `clone_ref`.

### 7. Schema-supplied values, and checks we do not do

An absent attribute with `fixed` or `default` is **supplied** by the schema
into the PSVI, flagged `from_schema`. Without that, `<length>3.2</length>`
appears to have no unit even when the schema pins one — which is the whole
point of the `fixed` pattern.

Two derivation validity checks are **not** implemented, and both currently
pass silently:

- An `xs:extension` that redeclares an attribute the base already has is
  illegal (two attribute uses cannot share a name), but we accept it and let
  the own use win.
- An `xs:restriction` that widens — base `use="required"`, derived omitting
  `use` and so defaulting to `optional` — is illegal, and we accept it.

Both belong with *Derivation Valid (Extension)* and *(Restriction, Complex)*,
which are unimplemented. Do not add them piecemeal; do the set, or leave the
gap documented.

The sealing rule is the exception, and it is done (`src/derivation.rs`): a
type's `final` is complete on its own, needing only the base's `final` and the
method used. What remains is **particle subsumption** — whether a restriction's
content model actually accepts a subset of its base's — which is the single
biggest thing standing between the invalid-schema figure and 100%. It is worth
about 50 of the ~236 cases still accepted; `examples/w3c_gap.rs` names them
(the `all`, `simple`, `complex` and `over` families).

### 8. Input encoding

Decoding happens in `encoding.rs` and **only** there. `Resolver` returns
`Vec<u8>` precisely so no resolver reimplements BOM and XML-declaration
sniffing and gets it wrong differently. Do not decode inside a resolver.

Decoding is strict: bytes that are not valid in the encoding they claim are an
error, not a document full of U+FFFD, because a schema quietly full of
replacement characters produces components that are quietly wrong.

Note for tests: `encoding_rs` has **no UTF-16 encoder** — per WHATWG,
`encode()` falls back to UTF-8 — so UTF-16 test bytes must be built by hand.
The first version of that test passed while exercising nothing.

Code generation is **permanently out of scope**. `xsd-parser` covers it.
