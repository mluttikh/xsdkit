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
| valid schemas accepted | **98.9%** — it reads real schemas |
| invalid schemas rejected | **50.4%** — particle subsumption is the big rule still missing |

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
it that way). Run it with `-- --ignored`. It scores 21,575 of them, 90.8%
correct: **88.1%** of valid documents accepted, **94.1%** of invalid ones
rejected. The shape is the mirror of the schema half — validation is
implemented, so it catches things (the schema half's 21.4% at the time). 1,160 of the 1,412 false alarms sit in
NIST2004-01-14 alone, which is where to look first.

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
