# AI Contributor Guide: xsdkit

> **This is the canonical AI/agent guide.** `CLAUDE.md` and `GEMINI.md` are
> thin pointers to this file — edit **this file only**, so all AI tools stay
> in sync.

## Project Overview

- **Crate:** `xsdkit` parses W3C XML Schema into a queryable **schema
  component model**. A Python extension and a CLI (`xsd2arrow`) come later.
- **Stack:** Rust (edition 2024), `roxmltree` (schema documents),
  `fxhash`, later `pyo3`.
- **Consumer:** the sibling crate
  [`xml2arrow`](https://github.com/mluttikh/xml2arrow); a config generator
  for it is a planned feature, not a current one.
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

Three phases, in strict order:

1. **Load** (`load.rs`) — `roxmltree` reads each document into components.
   References are **not** resolved here: a `ref`/`base`/`type` becomes a
   placeholder id plus an entry in `Loader::fixups`. This linker split is the
   only thing that copes with circular import graphs.
2. **Compile** (`compile.rs`) — five ordered steps, each finishing before the
   next begins: `resolve_references` → `merge_attribute_groups` →
   `resolve_simple_content` → `check_cycles` → `build_substitution_closure`.
   Order is load-bearing; attribute groups cannot flatten before their
   references resolve.
3. **Query** (`model.rs`) — `Schemas`, immutable, `Send + Sync`.

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

### 3. Behavioral contracts (pinned by tests — do not "fix" these)
- **`whiteSpace` applies before lexical parsing.** It is the only difference
  between `xs:string` and `xs:token`, and why `<v> 42 </v>` parses as
  `xs:int`.
- **Patterns OR within a restriction step, AND across steps.**
  `FacetSet::patterns` is `Vec<Vec<String>>` for exactly this reason.
- **The innermost enumeration wins**; a restriction may only narrow.
- **Chameleon includes** key the document cache on `(uri, coerced_ns)`, never
  on `uri` alone. The same file yields different components per includer.
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

### 4. Security
- Network fetching is **opt-in**: `FileResolver` refuses `http(s)://`.
- Every graph walk needs a bound: `MAX_DEPTH` for includes, `nodes_limit` per
  document, cycle guards in `base_chain` and `check_cycles`.

## Public API & Compatibility

`src/lib.rs` re-exports are the public API. `SchemaSetBuilder`, `Schemas`,
`Diagnostics`, `Resolver` and the component types are the surface to keep
stable. Component structs have public fields on purpose — this is a model to
be read. Adding a field is breaking for struct-literal construction; call it
out in the PR.

## Testing Conventions

- `src/*/mod tests` — unit tests for pure logic (facets, interning, codes).
- `tests/integration_tests.rs` — a schema document in, components out. Uses
  the in-memory `MapResolver` so composition tests need no files.
- `tests/real_world.rs` — the W3C schema-for-schemas. **Every bug found on a
  real schema gets a repro here**, in addition to a unit test. Three of the
  first four bugs in this crate were found by loading it once.
- The full suite runs in under a second — run it often.

Adding a schema feature? Add: a synthetic test for the feature alone, a
failure test for its malformed form, and a check that it survives the real
fixture.

## Planned, not present

Deliberately out of scope until their phase (see `DESIGN.md` §3.14):
content-model automata and UPA (P2), the `xml2arrow` config generator (P3),
the units layer (P4), instance validation and PSVI (P5), Python bindings
(P6), XSD 1.1 assertions and conditional type assignment (P7).

Two seams already exist and must not be removed:
- `Annotation::appinfo` keeps `appinfo` XML **verbatim** — the units layer
  cannot recover a unit from a summary.
- `Particle::is_repeating` / `is_optional` are the primitives the config
  generator's table/column and nullability decisions are built on.

Code generation is **permanently out of scope**. `xsd-parser` covers it.
