# A Rust XSD Toolkit — Review and Design Proposal

> Status: P0–P2 implemented (component model, loading, content automata, UPA).
> Plan revised 2026-09-04 to put Python bindings next.
>
> Goal: **`xsdkit`**, a generic XSD reader in Rust with first-class Python
> bindings — parse a schema, query it as a model, and validate and typed-read
> documents against it.

---

## Part 0 — Executive summary

**The finding.** Every serious XSD implementation in every language separates
three things: the *syntax* of schema documents, the *schema component model*
(the abstract graph the spec actually defines semantics over), and the
*consumers* of that model (validators, code generators, editors). The tools
that skip the middle layer — Go's `aqwari`/`xgen`, Rust's `xml-schema`, and to a
lesser degree Rust's `xsd-parser` — all hit the same wall on real-world schemas.
The tools that build it — Xerces `XSModel`, .NET's SOM, Saxon, Python's
`xmlschema` — are the ones people actually use against GML, UBL and HL7.

**The gap.** Rust has no schema component model. It has a good code generator
(`xsd-parser`), a validator with no introspection API (`uppsala`), and two
datatype libraries (`oxsdatatypes`, `xsd-types`). Nothing in either Rust or
Python lets you *ask a schema questions* at speed — `xmlschema` can, and is
40–75× slower than lxml doing it.

**The proposal.** Build **`xsdkit`**: a **generic XSD reader**. An
arena-backed schema component model with an explicit compile step, on top of
a datatype/facet layer, with a streaming typed reader (PSVI) over it — and
Python bindings, because that is where most schema processing actually
happens. Skip code generation entirely; `xsd-parser`
already does it and it is the single largest sink of effort in this space.


### What belongs in this package

`xsdkit` is a generic XSD reader — component model, validation, typed reading
— and keeps a dependency footprint of essentially nothing, so that anyone with
an XSD problem wants it regardless of what they intend to do afterwards.

Consumers built on top of it belong outside it. A downstream tool has a
different dependency set and a different release cadence from a schema reader,
and every dependency it needs would otherwise be paid for by people who never
asked for it.

### Name

**`xsdkit`** — crate, Python package and repository. Reserved-free on both
registries in every separator variant (`xsdkit` / `xsd-kit` / `xsd_kit`), no
existing project clash.

- `xsd` in the name keeps it findable by the search people actually run.
- `kit` covers reading, validating and introspecting honestly, where
  `-parser` and `-model` each name only one of them, and a name ending in a
  target format would box the scope in permanently.

Ruled out despite being available: **`xsom`** — `org.glassfish.jaxb:xsom` is a
shipping Java library doing exactly this job, so the name is already spoken for
in this problem domain.

---

## Part 1 — Review of the XSD format

### 1.1 XSD is three languages, not one

| Layer | What it is | Spec location |
|---|---|---|
| **Schema documents** | `<xs:schema>`, `<xs:element>`, `<xs:complexType>` … the XML you edit | Part 1 §3.x.2 ("XML Representation") |
| **Schema components** | Element Declarations, Complex Type Definitions, Particles, Model Groups, Wildcards, Attribute Uses … an in-memory graph | Part 1 §3.x.1 ("The … Schema Component") |
| **Validation semantics** | Rules over components producing the post-schema-validation infoset (PSVI) | Part 1 §3.x.4, Appendix C.2 |

The syntax→component mapping is many-to-one and lossy in both directions.
`<xs:element ref="x" minOccurs="0" maxOccurs="3"/>` is **not** an element
declaration; it is a *Particle* `{min: 0, max: 3, term: →ElementDecl(x)}` whose
term is a component shared with every other reference to `x`. Two syntactically
different documents can produce the identical component set, and one document
can produce different components depending on who includes it (see chameleon
includes, §1.5).

**Consequence for design:** the component model is the product. Validation,
codegen and every other consumer are built on it, and none of them should be
allowed to shortcut into the syntax.

### 1.2 The component inventory

*Primary* — Simple type definitions · Complex type definitions · Attribute
declarations · Element declarations

*Secondary* — Attribute group definitions · Identity-constraint definitions ·
Type alternatives (1.1) · Assertions (1.1) · Model group definitions · Notation
declarations

*Helper* — Annotations · Model groups (sequence/choice/all) · Particles ·
Wildcards · Attribute uses

Plus "the schema as a whole": a set of components indexed by namespace across
**seven independent symbol spaces** (types, elements, attributes, model groups,
attribute groups, notations, identity constraints). Names collide only within a
symbol space *and* a namespace — so `Foo` the type and `Foo` the element are
unrelated, and this must be modelled, not flattened into one map.

Critically, **local declarations are not globally addressable**. A local element
declaration is scoped to the complex type that contains it, so
`{name, targetNamespace}` is not a key for it. Any model that stores
declarations in one flat name-keyed dictionary is already wrong.

### 1.3 The datatype system (Part 2)

- **2 special types**: `anySimpleType`, `anyAtomicType`.
- **19 primitive types**: `string`, `boolean`, `decimal`, `float`, `double`,
  `duration`, `dateTime`, `time`, `date`, `gYearMonth`, `gYear`, `gMonthDay`,
  `gDay`, `gMonth`, `hexBinary`, `base64Binary`, `anyURI`, `QName`, `NOTATION`.
  (`precisionDecimal` was in drafts and did **not** make the final 1.1
  Recommendation; it is an optional implementation extension.)
- **~25 ordinary derived types**: the `string` chain
  (`normalizedString` → `token` → `language`/`Name`/`NMTOKEN`, `NCName` → `ID`/`IDREF`/`ENTITY`),
  the list types (`NMTOKENS`, `IDREFS`, `ENTITIES`), the `integer` chain
  (`integer` → `long` → `int` → `short` → `byte`, plus the `nonNegativeInteger`
  / `unsignedLong` … `unsignedByte` / `positiveInteger` /
  `nonPositiveInteger` / `negativeInteger` branches), and 1.1's
  `yearMonthDuration`, `dayTimeDuration`, `dateTimeStamp`.

**Three varieties of simple type**, and this is where naive implementations
break:

| Variety | Value | Gotcha |
|---|---|---|
| **atomic** | one value | the normal case |
| **list** | whitespace-separated sequence of an item type | `length`/`minLength` facets count *items*, not characters |
| **union** | value of one of N member types | the actual type is only known after trying members **in declaration order**; the first match wins |

**14 constraining facets** (1.1): `length`, `minLength`, `maxLength`,
`pattern`, `enumeration`, `whiteSpace`, `maxInclusive`, `maxExclusive`,
`minInclusive`, `minExclusive`, `totalDigits`, `fractionDigits`, `assertions`,
`explicitTimezone`. **4 fundamental facets** (derived, not settable):
`ordered`, `bounded`, `cardinality`, `numeric`.

Two facet rules that are routinely implemented wrong:

- **Multiple `pattern` facets in one restriction step are OR'd; patterns across
  restriction steps are AND'd.** So a type restricting a type restricting a
  base must satisfy all three levels' pattern disjunctions.
- **`whiteSpace` is applied before lexical parsing, not after.** It is the sole
  reason `xs:token` differs from `xs:string`, and it is fixed at `collapse` for
  every type not descended from `string`/`normalizedString`. Getting the order
  wrong makes `<v> 42 </v>` fail against `xs:int`.

**XSD regular expressions are not PCRE.** They are implicitly anchored, have no
backreferences or lookaround, and add `\i`/`\c` (XML name-start / name chars),
Unicode block escapes (`\p{IsBasicLatin}`), and **character class subtraction**
(`[a-z-[aeiou]]`). A conforming implementation cannot just hand the string to a
PCRE engine.

### 1.4 Content models

A Particle is `{minOccurs, maxOccurs, term}`; a term is an element declaration,
a wildcard, or a model group (`sequence` / `choice` / `all`).

- **Unique Particle Attribution (UPA)** — XSD 1.0 requires content models be
  deterministic with *zero* lookahead. `(a?, a)` is illegal. `(a?, xs:any)` is
  illegal. Real-world schemas violate this often enough that Symfony shipped
  one; a usable tool needs a lax mode. XSD 1.1 relaxes the wildcard case:
  element particles win over competing wildcards.
- **`xs:all`** in 1.0 was restricted to `maxOccurs = 1` and to being the whole
  content model. 1.1 permits element particles with `maxOccurs > 1` inside it,
  permits wildcards in it, and permits extending it.
- **`xs:openContent` / `defaultOpenContent`** (1.1) interleave or append
  wildcards across a whole schema document.
- Mixed content, plus the "effective total range" and "emptiable" computations
  needed for restriction checking.

### 1.5 Composition — where implementations actually break

| Mechanism | Semantics | The trap |
|---|---|---|
| `xs:include` | merge a document with the *same* target namespace | **Chameleon include**: an included document with *no* `targetNamespace` is absorbed into the includer's namespace. The same file therefore yields *different components* per includer — you can cache `(uri, coerced_ns) → components`, never `uri → components`. |
| `xs:import` | reference a *different* namespace | `schemaLocation` is a **hint**. Schemas legitimately import namespaces with no location and expect a catalog or a preloaded set. |
| `xs:redefine` (1.0, deprecated) | include + modify, where the redefinition may refer to the *old* definition by its own name | Requires shadowing during resolution. libxml2 never implemented it — `xmlschemas.c` still raises "Unimplemented block". |
| `xs:override` (1.1) | full component replacement, applied transitively through includes | Cleaner, but must be applied before resolution, not after. |

Beyond that: circular import/include graphs are normal (key on absolute URI);
multiple documents contribute to one namespace; and
`elementFormDefault`/`attributeFormDefault` decide whether *local* elements are
namespace-qualified in instances — the number-one cause of "why won't my XML
validate", and directly load-bearing for anything that matches paths against a
document.

### 1.6 Derivation and substitutability

- Complex types derive by **extension** (append particles and attribute uses)
  or **restriction** (narrow — and the derived content model must be a provable
  restriction of the base, a check most tools quietly skip).
- Simple types derive by restriction, list, or union.
- `xsi:type` in the *instance* overrides the declared type, to any type derived
  from it, subject to `block`.
- **Substitution groups**: any element whose `substitutionGroup` names a head
  may appear wherever the head is allowed, transitively, with a type derived
  from the head's. Heads may be `abstract`. This is pervasive in GML, WITSML
  and UBL, and it means *you cannot know which element names may appear at a
  position without computing the substitution closure*.
- `block` / `final` / `blockDefault` / `finalDefault` restrict which
  derivations and substitutions are permitted. .NET names the resolved forms
  `BlockResolved`/`FinalResolved` and only populates them after `Compile()` —
  a useful reminder that a schema has two distinct lifecycle states.
- `nillable` + `xsi:nil` let an element be present but valueless. This maps
  cleanly onto an Arrow null and is *not* the same as `minOccurs="0"`.

### 1.7 Identity constraints are free foreign keys

`xs:unique`, `xs:key`, `xs:keyref` use a deliberately tiny XPath subset
(child and attribute axes, `.//`, `|`). A `keyref` is literally a declared
foreign key between two element sets.

No tool in this survey exploits that. It is a direct source of the relational
structure of a document — which element sets reference which — and that is a
differentiator worth taking.

### 1.8 XSD 1.1, briefly

`xs:assert` on complex types and the `assertions` facet on simple types (XPath
2.0); `xs:alternative` for Conditional Type Assignment (element type chosen by
an XPath test over the instance's *attributes*); `xs:openContent`;
`xs:override`; `xs:defaultAttributes`; wildcard `notQName`/`notNamespace`;
relaxed UPA; the `vc:minVersion`/`vc:maxVersion`/`vc:typeAvailable` conditional
attributes that let one document carry both 1.0 and 1.1 variants.

### 1.9 Annotations — the standard's extension point

`xs:annotation` carries `xs:documentation` (human) and `xs:appinfo`
(machine-readable, arbitrary foreign-namespace XML). `appinfo` is the one
place the specification sets aside for conventions it does not itself define,
and schema families use it heavily: database mappings, UI hints, code-list
references, provenance.

Because the content is foreign XML with no schema-defined meaning, there is
nothing useful a reader can do with it except **keep it exactly as written**.
Anything else — normalising it, summarising it, parsing it against a guess at
the convention — destroys precisely the information the caller reached for it
to get.

**Design consequence:** `appinfo` is stored verbatim, with prefixes resolved
so no namespace binding is lost. Interpreting it is the caller's job, because
only the caller knows the convention.

### 1.10 Practical hazards worth designing against

- **Scale.** UBL, FpML, HL7, ISO 20022, XBRL, AUTOSAR. AUTOSAR's schema alone
  is ~100 MB; naive generators emit gigabytes.
- **Occurrence blowup.** `maxOccurs="5000"` is legal and appears in the wild.
  Unrolling it into automaton states is the classic memory bomb.
- **Recursive types.** `Node` containing `Node`. Must box in Rust, must not
  infinitely expand during config generation.
- **Invalid-but-shipping schemas.** UPA violations, dangling imports, wrong
  `schemaLocation`s. Strict mode must be optional.
- **Hostile input.** XXE and billion-laughs in the *schema itself*; catastrophic
  regex backtracking from a `pattern` facet; unbounded recursion.
- **`xs:anyType`** is the default type, so untyped content is everywhere.

---

## Part 2 — Survey of existing implementations

### 2.1 The comparison

| Project | Lang | Component model | Validates | Codegen | 1.1 | The one thing worth stealing |
|---|---|---|---|---|---|---|
| **Xerces2-J** `org.apache.xerces.xs` | Java | **Yes — `XSModel`**, the W3C XML Schema API reference impl | Full + PSVI via `PSVIProvider` | No | Yes | The API shape itself: immutable, read-only, namespace-spanning, with PSVI streamed alongside SAX events |
| **Saxon EE** | Java/.NET | Yes | Best-in-class 1.1 incl. assertions + CTA | No | Yes | Content models compiled to finite state machines |
| **.NET SOM** `System.Xml.Schema` | C# | Yes — *mutable*, then `XmlSchemaSet.Compile()` | Yes | `xsd.exe` | No | The explicit two-state lifecycle: `Block` vs `BlockResolved`, `IsCompiled` |
| **Apache XMLBeans** | Java | Yes — `SchemaTypeSystem` | Yes | Yes | No | The compiled type system is **serializable** — compile once, load fast |
| **Eclipse XSD (EMF)** | Java | Yes, with live syntax↔component bidirectional links | via Xerces | via EMF | No | Round-trippability; the right model for editors/LSPs |
| **`xmlschema`** (sissaschool) | Python | Yes — `XsdGlobals` mediator + `XsdElement`/`XsdComplexType`/… | **Full 1.0 and 1.1** | No | Yes | *Decoding* as a first-class feature: schema-driven XML → typed Python/JSON via pluggable converters. ~40–75× slower than lxml, and honest about it. |
| **libxml2 / lxml** | C | No public model | 1.0 only, **incomplete** | No | No | Speed. But `redefine` unimplemented, decimal capped at 24 digits |
| **`xsdata`** | Python | No — syntax → class model directly | No | Yes (dataclasses/attrs/pydantic) | Partial | The **ordered handler pipeline** (below) — the best-documented transformation architecture in the space |
| **`generateDS`, `PyXB`** | Python | Partial | Some | Yes | No | Cautionary tales; PyXB is abandoned |
| **CodeSynthesis XSD** | C++ | Uses Xerces-C's | via Xerces-C | Yes | No | Ships **two** mappings — C++/Tree (in-memory) and C++/Parser (event-driven). Precedent for offering both shapes |
| **JAXB / XJC** | Java | Yes | Yes | Yes | No | External `.xjb` binding-customization files: schema untouched, mapping overridden |
| **`aqwari.net/xml/xsd`** | Go | **No** — dereferences groups, flattens nested sequences, discards the info | No | Yes | No | Refreshingly explicit: "a subset targeted at client libraries, not validators" |
| **`xgen`** | Go | No | No | Yes, multi-language | No | — |
| **`xsd-parser`** (Bergmann89) | **Rust** | Partial — `Schemas` → `MetaTypes` → `DataTypes` → `Module` | **No** (listed as planned) | Yes | Partial | Pluggable **resolvers** for `schemaLocation`; five explicit pipeline stages each independently runnable |
| **`uppsala`** (kushaldas) | **Rust** | Internal only | **Claims XSD 1.1 structures + datatypes** | No | Claims yes | Zero-dependency; arena DOM; **its own NFA regex engine** for XSD patterns; explicit DoS budgets (depth 128, 1 MiB entity expansion) |
| **`xml-schema`** (media-io) | Rust | No | No | Yes (proc macro) | No | Stale since 2023 |
| **`oxsdatatypes`**, **`xsd-types`** | Rust | n/a | Datatype level | n/a | n/a | **Directly reusable** value-space implementations |

### 2.2 The Rust landscape, precisely

From crates.io (downloads = all-time, as of 2026-09-04):

```
oxsdatatypes    0.2.3   712k   2026-08-30   XSD datatypes for SPARQL (Oxigraph)
xsd-types       0.9.6   316k   2024-09-18   XSD data types
uppsala        0.10.1   168k   2026-09-02   XML parser + DOM + XPath + XSD validation
xsd-parser      1.5.2    89k   2026-03-25   Code generator for XML schema files
xml-schema      0.3.0    32k   2023-12-19   Struct generator from XSD (stale)
```

**There is no schema component model in Rust.** `xsd-parser` builds a
codegen-oriented intermediate (`MetaTypes`) that deliberately discards
validation semantics; `uppsala` builds one internally but exposes only
`validate()`. Neither lets you ask "what are the possible children of this
element, and can they repeat?" — which is the question every downstream
consumer of a schema is made of.

### 2.3 Five lessons to carry into the design

1. **Separate the component model from its consumers.** Xerces, .NET,
   `xmlschema` and XMLBeans do. `aqwari`, `xgen`, `xml-schema` and (partly)
   `xsd-parser` do not, and it caps what they can do.
2. **Make "compiled" a different type, not a flag.** .NET's `IsCompiled`
   boolean and its `Block` / `BlockResolved` pairs exist because the language
   couldn't express the two states. Rust can.
3. **Transformations belong in an ordered handler pipeline.** `xsdata`'s
   analyzer runs seven named steps (Ungroup → Flatten → Filter → Sanitize →
   Resolve → Vacuum → Finalize → Designate), each a list of small handlers
   (`FlattenClassExtensions`, `DetectCircularReferences`,
   `RenameDuplicateClasses`, …), with strict ordering. It is the only project
   in the survey whose flattening logic is legible.
4. **Schemas are cyclic graphs; use an arena, not `Rc<RefCell<…>>`.**
   Every GC language solves this with pointers. In Rust the answer is one arena
   per component kind plus typed `Copy` indices. Bonus: the compiled model
   becomes trivially serializable — XMLBeans' cache trick, which matters a lot
   for a Python extension that would otherwise recompile on every interpreter
   start.
5. **`schemaLocation` is a hint.** Pluggable resolver + OASIS XML Catalog +
   offline-by-default is table stakes, not a nice-to-have.

---

## Part 3 — Proposal

### 3.1 Scope, and one deliberate exclusion

**In scope for `xsdkit`**

1. A schema component model for XSD 1.0, shaped for 1.1 from day one.
2. **Python bindings** — first-class, not an afterthought. Most schema
   processing happens in Python, and `xmlschema` is the only complete option
   there.
3. Schema-driven typed reading of XML documents (streaming PSVI).
4. XSD 1.1.

**Out of scope — code generation.** `xsd-parser` covers it, it is the single
largest effort sink in this space, and it is orthogonal to everything above.
Saying no here is what makes the rest finishable.

**Out of scope — data-binding config generation.** Emitting a config for some
particular downstream reader belongs in a library of its own: it would drag
that reader's dependencies into every tree that only wanted to read a
schema (§3.0b).

### 3.2 Layout

Follow the `xml2arrow` precedent: one crate, strict internal module boundaries,
feature-gated consumers, Python bindings behind a `python` feature.

```
xsdkit/
├── Cargo.toml            # features: validate, python
├── src/
│   ├── lib.rs
│   ├── datatypes/        # value spaces, lexical mappings, facets, XSD regex
│   ├── model/            # arena, component types, IDs, queries      ← the product
│   ├── load/             # documents, resolvers, catalogs, include/import/override
│   ├── compile/          # resolution, derivation, subst groups, automata, UPA
│   ├── validate/         # streaming validator + PSVI            (feature)
│   ├── diagnostics.rs    # codes, spans, severities
│   └── python.rs         # pyo3                                   (feature)
└── python/               # maturin project + type stubs
```

Split into a workspace only once a boundary has proved stable. `datatypes` is
the likeliest first extraction — it is genuinely reusable and has no upward
dependencies.

There is no module here for any particular downstream format, and no
dependency on one (§3.0b).

The Python package lives in `python/` in the same repository and ships from
the same tag, so a binding can never lag the model it wraps.

### 3.3 The core: an arena-backed component model

```rust
/// Fully compiled, immutable, Send + Sync, cheap to clone, serializable.
pub struct Schemas {
    types:        Arena<TypeDefinition>,      // TypeId
    elements:     Arena<ElementDecl>,         // ElementId
    attributes:   Arena<AttributeDecl>,       // AttributeId
    particles:    Arena<Particle>,            // ParticleId
    model_groups: Arena<ModelGroupDef>,       // GroupId
    attr_groups:  Arena<AttributeGroupDef>,   // AttrGroupId
    constraints:  Arena<IdentityConstraint>,  // IdcId
    notations:    Arena<NotationDecl>,
    annotations:  Arena<Annotation>,

    names:      Interner,           // QName ↔ NameId; all strings interned
    globals:    SymbolTables,       // (Namespace, SymbolSpace) → Id — seven spaces
    automata:   Arena<ContentAutomaton>,
    subst:      SubstitutionClosure, // ElementId → &[ElementId], precomputed
    documents:  Vec<SourceDocument>, // provenance: URI + line/col spans
}

#[derive(Copy, Clone, PartialEq, Eq, Hash)] pub struct TypeId(u32);
#[derive(Copy, Clone, PartialEq, Eq, Hash)] pub struct ElementId(u32);
// …
```

Why arenas: schemas are cyclic (a type contains a particle referencing an
element declared with that type). `Rc<RefCell<…>>` would leak and poison every
signature with borrow lifetimes. `u32` indices are `Copy`, cache-friendly,
serializable, and make the whole `Schemas` a single `Send + Sync` value.

**Two lifecycle states as two types:**

```rust
pub struct SchemaSetBuilder { /* mutable, unresolved */ }

impl SchemaSetBuilder {
    pub fn new() -> Self;
    pub fn resolver(self, r: impl Resolver + 'static) -> Self;
    pub fn catalog(self, path: impl AsRef<Path>) -> Result<Self>;
    pub fn mode(self, m: Conformance) -> Self;   // Strict | Lax
    pub fn add_file(&mut self, p: impl AsRef<Path>) -> Result<()>;
    pub fn add_str(&mut self, xsd: &str, base: Option<&Url>) -> Result<()>;
    pub fn build(self) -> Result<Schemas, Diagnostics>;   // resolve + compile
}
```

`Schemas` never exists in an unresolved state, so "did you call `Compile()`?" —
the .NET footgun — is not representable. Every accessor
(`content_automaton()`, `substitution_members()`, `effective_type()`) lives on
`Schemas` only.

**Compile once, use many** — the same philosophy `xml2arrow`'s `Parser`
already has, and the reason `Schemas` should implement `serde::Serialize`:
compiling UBL or AUTOSAR is expensive, and a Python extension pays that cost on
every process start otherwise.

### 3.4 Datatypes and facets

```rust
pub enum Value {
    String(SmolStr), Boolean(bool), Decimal(Decimal),
    Float(f32), Double(f64),
    Duration(Duration), DateTime(DateTime), Date(Date), Time(Time), G(GregorianPart),
    HexBinary(Bytes), Base64Binary(Bytes),
    AnyUri(SmolStr), QName(QName), Notation(QName),
    List(Vec<Value>),          // list variety
}
```

- ~~**Reuse `oxsdatatypes`**~~ for `decimal` / `duration` / the date-time
  family. This was the plan, and it held for most of the project's life; the
  datatypes are now implemented in `src/atomic.rs` instead. See §3.12.4 for
  how that decision was made, defended twice, and finally reversed on
  evidence.
- **Transpile XSD regex to the `regex` crate** rather than writing an engine
  (`uppsala` wrote its own; `xmlschema` translates to Python `re`). Write a
  parser for the XSD regex grammar, then lower it: implicit anchoring →
  `\A…\z`; `\i`/`\c` → explicit classes; `\p{Is…}` blocks → Unicode ranges;
  **character class subtraction → set difference computed at transpile time**.
  Payoff: `regex`'s linear-time guarantee removes catastrophic backtracking as
  a DoS vector, for free.
- **Facet engine** must respect the two rules from §1.3: patterns OR within a
  step and AND across steps; `whiteSpace` applied *before* lexical parsing.
- Precompute per-type a `Lexer` closure so validation does no dispatch on the
  hot path.

### 3.5 Content model compilation

Compile every complex type's particle tree to a **Glushkov (position)
automaton**, with two extensions:

1. **Bounded unrolling instead of counting states.** *Revised during
   implementation.* The original plan was a counting automaton — one state
   plus a counter for `minOccurs=1 maxOccurs=5000`. Plain unrolling turned
   out to be the better trade: it keeps the automaton ordinary, with no
   counter machinery anywhere in the matcher, and it makes UPA *more*
   accurate — `a{2,2}, a` is correctly not a breach, where collapsing bounds
   to `+` reports a false positive.

   The cost is bounded by two separate caps, because there are two different
   blowups. `MAX_UNROLL` (64 copies) bounds the **quadratic** cost of
   unrolling: `a{1,n}` leaves every optional copy in the `last` set, so each
   further copy costs `O(n)` edges. `MAX_POSITIONS` (4096) bounds total model
   size and is deliberately much looser, since a flat sequence of hundreds of
   distinct elements is ordinary and must not be truncated. Past either cap
   the range is widened to unbounded and the model is marked `approximated`,
   which downgrades its UPA findings to warnings. Widening only ever adds
   reachable positions, so an approximated model accepts a superset — false
   positives, never false negatives.
2. **Predicate-labelled transitions.** A wildcard's label is a namespace-set
   predicate (with 1.1's `notQName`/`notNamespace`), so 1.1's "element particle
   beats wildcard" is a transition-priority rule rather than a special case.

`xs:all` gets a bitset matcher, not an automaton.

**UPA falls out for free:** the model violates UPA exactly when some state has
two outgoing transitions with overlapping labels. Report it as a diagnostic
with both particles' source spans; downgrade to a warning in `Lax` mode.

The automaton is also what answers the three questions the config generator
needs, and they are already exposed: `possible_children` (transition labels ∪
substitution closure), `child_repeats` (a cycle through the position, or
`maxOccurs > 1`), and `child_is_optional` (an accepting path that avoids the
position).

One rule discovered while implementing: **a type's content model is not its
own particle.** Extension appends to the base's, restriction replaces it. A
type that extends a base and adds only an attribute has an empty particle of
its own and every one of its base's children — `xs:keyref` is exactly that
shape, so a real schema catches the mistake immediately.

### 3.6 Streaming typed reading (PSVI)

No DOM. `quick-xml` events in, a validation stack, PSVI events out — the same
architecture as `xml2arrow`, and the differentiator against `xmlschema` and
lxml.

```rust
pub enum PsviEvent<'a> {
    StartElement { decl: ElementId, ty: TypeId, attrs: &'a [AttrPsvi], nil: bool },
    TypedText    { value: Value, ty: TypeId },
    EndElement   { decl: ElementId },
}

pub struct TypedReader<R> { /* … */ }
impl<R: BufRead> Iterator for TypedReader<R> { type Item = Result<PsviEvent<'_>>; }
```

Three honest limitations, stated up front rather than discovered later:

| Feature | Streaming? | Plan |
|---|---|---|
| `xsi:type` override | ✅ available at start-tag | supported |
| `xs:alternative` (CTA) | ✅ tests only the element's own attributes | supported (needs XPath 2.0 subset) |
| `xs:assert` | ❌ needs the element's whole subtree | requires a buffering mode; document it |
| `xs:key` / `keyref` | ❌ document-scope state | optional, with a memory cap |

### 3.7 Diagnostics

```rust
pub struct Diagnostic {
    pub code:     DiagCode,        // XSD1042 — stable, documented, greppable
    pub severity: Severity,        // Error | Warning | Note
    pub message:  String,
    pub spans:    Vec<Span>,       // document URI + line/col, multiple per diag
    pub help:     Option<String>,
}
```

Collect, never bail on the first — schema authors need the whole list. Render
through `miette` or `ariadne` for source-quoted output. Nothing in the Rust XSD
ecosystem does this, and for anyone debugging a 40-file import graph it is the
feature they will notice first.

Match the `xml2arrow` house rule: structured enums, never stringly-typed
errors, and `Display` output treated as a stability surface.

### 3.8 Security model

Mirroring `xml2arrow`'s trust-model section, since schemas arrive from
elsewhere just as often as documents do:

- **No DTD processing, no external entity resolution** in schema or instance.
- **Offline by default.** Network fetching of `schemaLocation` is opt-in;
  the default resolver is filesystem + XML Catalog only.
- **Linear-time regex** via the `regex` crate — no catastrophic backtracking.
- **Budgets**: nesting depth, total component count, automaton state count,
  occurrence-counter magnitude, identity-constraint table size.
- **Recursion guards** in every graph walk (derivation chains, substitution
  closure, include/import cycles).

### 3.9 Testing

| Layer | Approach |
|---|---|
| **Conformance** | The **W3C XML Schema Test Suite** (~26k cases across 1.0 and 1.1). Wire it up in Phase 1 and publish a per-feature-area pass rate. Highest-leverage single decision in the plan. |
| **Real-world corpus** | XHTML, SVG, GML, UBL, OGC, WITSML, ISO 20022, XBRL, AUTOSAR. Merely *loading* these is a strong smoke test. |
| **Differential** | Run the same corpus through Python `xmlschema` and diff verdicts. It is the most complete OSS 1.1 implementation and makes an excellent oracle. `uppsala` as a second. |
| **Property** | Facet composition, automaton equivalence under particle rewrites, regex transpilation (XSD pattern vs. transpiled `regex` on generated strings). |
| **Fuzzing** | `cargo-fuzz` on the schema loader and the regex transpiler. |
| **Bench** | `criterion` + CodSpeed, already in use. Track: compile time for UBL/AUTOSAR, validation throughput MB/s, `Schemas` deserialize time. |

### 3.10 Python bindings

The reason this moved to the front of the queue: most XSD processing happens
in Python, and `xmlschema` is the only complete option there — 40–75× slower
than lxml, by its own benchmarks. A fast, complete schema reader with a
Pythonic API is the single most useful thing `xsdkit` can be.

**The binding problem.** The Rust API is index-based: `schemas[element_id]`.
Python wants objects. Every Python handle is therefore a pair:

```rust
#[pyclass] pub struct SchemaSet(Arc<Schemas>);
#[pyclass] pub struct Element { schemas: Arc<Schemas>, id: ElementId }
```

`Schemas` goes behind an `Arc` **inside the binding**, so handing out ten
thousand element wrappers costs ten thousand refcount bumps and copies
nothing. Nothing leaves the model until Python asks for it. This is why the
arena design pays off twice: `Copy` ids make wrappers free, and the whole
model being one `Send + Sync` value means the GIL can be released around
compilation.

```python
import xsdkit

schemas = xsdkit.SchemaSet.from_file("report.xsd", search_paths=["schemas/"])

# Composition
[d.uri for d in schemas.documents]
[d.target_namespace for d in schemas.documents if d.chameleon]

# Globals, as mappings
schemas.elements["{urn:example}report"]
report = schemas.element("urn:example", "report")

# Declarations
report.name                    # ('urn:example', 'report')
report.nillable, report.abstract
report.substitutes             # every element that may appear in its place
report.doc                     # xs:documentation, joined
report.appinfo                 # [AppInfo(source=..., xml=...)] — verbatim

# Types and content models
t = report.type
t.attributes                   # [AttributeUse]
t.children                     # substitution groups expanded, inherited included
t.repeats(child)               # table or column
t.optional(child)              # nullable or not
t.accepts(["title", "count"])  # does this child sequence satisfy the model?

# Simple types
code = schemas.type("urn:example", "Code")
code.variety                   # 'atomic' | 'list' | 'union'
code.primitive                 # 'string'
code.facets.max_length         # 4
code.facets.patterns           # [['[A-Z]+', '[0-9]+']] — OR within, AND across
code.facets.enumeration

# Diagnostics — all of them, never just the first
schemas, diags = xsdkit.load("vendor/partial.xsd", conformance="lax")
for d in diags:
    print(d)                   # error[XSD1201]: ...  --> file.xsd:12
    d.code, d.severity, d.message, d.spans, d.help
```

#### Input encoding — a breaking change that belongs here

`roxmltree` parses `&str`, so bytes must be decoded before parsing. Today
`FileResolver` calls `read_to_string`, so a schema declaring
`encoding="ISO-8859-1"` — perfectly legal, and common in older European
industry schemas — simply fails:

```text
error[XSD1101]: latin1.xsd: stream did not contain valid UTF-8
  help: add a search path, or supply a custom Resolver
```

Two things are wrong there. The document does not load at all, and the
diagnostic blames the wrong thing: it is reported as an unresolved
`schemaLocation`, with help about search paths, when the file was found and
read perfectly well.

The fix is not "decode inside `FileResolver`". It is a change to the
**`Resolver` trait**:

```rust
// today — every resolver must decode, and each will get it wrong differently
fn resolve(&self, location: &str, base: Option<&str>) -> Result<(String, String), String>;

// after — resolvers fetch bytes; the loader decodes once, correctly
fn resolve(&self, location: &str, base: Option<&str>) -> Result<(String, Vec<u8>), String>;
```

Decoding then happens in one place, following XML Appendix F: BOM first
(UTF-8, UTF-16 LE/BE), then the `encoding=` pseudo-attribute of the XML
declaration, then UTF-8 as the default. `encoding_rs` does the transcoding —
a dependency `quick-xml` pulls in at P4 anyway for the instance side, so it is
shared rather than added.

It lands in P3 because `Resolver` is public API and the change is breaking.
Wrapping it in Python first and changing it after means doing the same work
twice, in two languages, with a released binding to migrate.

New diagnostic codes: `UnsupportedEncoding` for a declared encoding we cannot
decode, and `MalformedEncoding` for bytes that do not decode as the encoding
they claim. Neither should masquerade as a missing file.

Mechanics:

- `maturin`, `pyo3`, **abi3** — one wheel per platform across 3.9+.
- **Release the GIL** around `build()`. Compilation is the only slow part,
  and `Schemas` is `Send + Sync` precisely so this is legal.
- Typed exception hierarchy: `XsdError` base, `SchemaError` carrying
  `.diagnostics`. Adding a `DiagCode` variant **must** update the conversion,
  guarded by an exhaustiveness test — the `xml2arrow` discipline.
- `.pyi` stubs checked in and verified with mypy; `__repr__` on every wrapper
  that names its component; collections implement `__len__` / `__iter__` /
  `__getitem__` / `__contains__`.
- Same repository, `python/` directory, shipped from the same tag, so a
  binding can never lag the model it wraps.

Deferred to later phases, and named here so the API leaves room: typed
streaming reads (`schemas.read_typed("doc.xml")`, P4) and caching a compiled
`Schemas` to disk (§3.3) so repeated interpreter starts do not recompile UBL.

### 3.11 Staging

| Phase | Deliverable | Gate |
|---|---|---|
| ~~**P0**~~ | ~~`datatypes`: 19 primitives + derived, 14 facets~~ | **done** — facet composition, whiteSpace ordering |
| ~~**P1**~~ | ~~`load` + `model`: documents, resolver, include/import, arena, symbol tables~~ | **done** — W3C schema-for-schemas loads |
| ~~**P2**~~ | ~~`compile`: derivation, substitution closure, content automata, UPA~~ | **done** — 55 automata, 0 UPA findings on a valid schema |
| **P3** | **Python bindings** — the component model, Pythonic, on PyPI. Plus the `Resolver` encoding fix (below), which is breaking and must land before the API is wrapped. | wheels on 3.9–3.14; the fixture queried from Python; a Latin-1 schema loads |
| **P4** | `validate`: streaming validator + PSVI + `TypedReader`, exposed in Python as it lands | W3C instance tests; differential vs. `xmlschema` |
| **P5** | XSD 1.1: assertions, conditional type assignment, `openContent`, `override` | W3C 1.1 tests |

**Why Python at P3.** Binding early is cheaper than binding late: it forces
the Rust API to be ergonomic while it is still cheap to change, and each
later phase adds a thin layer rather than a retrofit. It also means every
phase from here ships something usable to a Python user, rather than three
phases of Rust-only work before anything is reachable.

### 3.12 Decisions worth making explicitly

1. **1.0 or 1.1?** Recommend: implement 1.0 semantics first, but shape the
   component model for 1.1 from day one — first-class (optional) slots for
   type alternatives, assertions and open content. Retrofitting 1.1 into a
   1.0-shaped model is what forced awkward corners on Xerces and left .NET
   stuck at 1.0 permanently.
2. **Build on `uppsala`?** No. It validates but exposes no component model, and
   its zero-dependency stance means it can't take ours. Use it as a second
   differential oracle instead.
3. **Build on `xsd-parser`?** No. `MetaTypes` is codegen-shaped and lossy about
   validation semantics. Do read its resolver design — that part is right.
4. **Reuse `oxsdatatypes`?** Originally yes; **now no — it is gone**, and all
   14 datatypes live in `src/atomic.rs`. The original reasoning and the two
   revisits are kept below, because the judgement was right each time on the
   evidence available, and the same call will come round again for whatever
   replaces it.

   *Originally:* yes. Decimal, duration and the date-time family
   are weeks of subtle work already done and battle-tested in Oxigraph.

   *Revisited after the W3C suite and the fuzzers were in place, which is the
   first point at which an implementation of our own could have been judged.*
   The answer is still yes. It gets the parts that are actually hard right —
   probed directly: `24:00` normalises to the next midnight, `2000-02-30` is
   rejected, leap seconds are rejected, `1E3` is not an `xs:decimal`, and the
   timezone partial order is correct (a value with no timezone against one
   with is *indeterminate* inside 14 hours and ordered outside it). That last
   is where date-time implementations traditionally get XSD wrong. Its only
   dependency is `thiserror`, so there is no weight argument either.

   One finding *is* a wrongness rather than a slowness, and it is the first:
   `--02-29` is rejected as an `xs:gMonthDay`. A gMonthDay names no year, so
   February has 29 days in it — the day exists, just not every year — and the
   W3C suite says so (`msData/datatypes/gMonthDay004` expects valid).
   `src/atomic.rs` now implements that one type outright, which is the
   incremental path the roadmap describes: one type at a time, behind the
   wrapper that already exists, each landing with the suite green.

   The other three are not reasons to leave:

   - `impl PartialOrd for Duration` normalises a date one month at a time, so
     a legal duration hangs rather than answers. Real, but a *performance*
     bug, and `src/values.rs` no longer calls that path.
   - It uses the reference dateTime 1969-09-01 where the specification says
     1696-09-01. Real, but in the same path we no longer call.
   - It accepts `+INF` and the year `0000`. **Not bugs** — both are legal XSD
     1.1 and illegal XSD 1.0. `oxsdatatypes` implements the 1.1 lexical
     spaces; we apply them in 1.0 mode because `values::parse` never receives
     the version. That one is ours (see AGENTS.md, road to 1.0, P1.2).

   What *did* change is the API judgement, and it is now acted on. `Value`
   exposed these types publicly, which pinned every downstream user to our
   exact version of the library. `src/atomic.rs` wraps all 14 — not to leave
   `oxsdatatypes`, but so that leaving it later is an internal change rather
   than a breaking one.

   *Revisited again, and reversed.* The wrapping is what made the reversal
   cheap, and once `xs:gMonthDay` had to be written here anyway, the rest
   turned out to be a few hundred lines sharing one timezone parser, one
   ±14-hour ordering rule, one civil-calendar day number and one seconds
   formatter. Conformance went up rather than down — instance cases correct
   20,490 to 20,492 — and the crate now has no datatype dependency at all.

   The lesson is not "write it yourself". It is that **the decision needed an
   oracle**, and the project did not have one when it was first made. With the
   W3C suite and four fuzz targets in place, an implementation of our own could
   be *judged*, and judging it took a day. Without them it would have been a
   guess, and the original answer was the right one.
5. **Codegen?** No — see §3.1. This is the load-bearing exclusion.
6. **How much belongs in one package?** Only the reader (§3.0b). `xsdkit`
   stays generic and almost dependency-free; consumers built on it live
   outside. The test is simple: would someone who has an XSD problem and no
   interest in your output format still want this dependency? For the reader,
   yes — which is the whole argument.
7. **Bind Python early or late?** Early, at P3. The API is small enough now
   that binding it is a day's work and a good forcing function; after
   validation lands it would be twice the surface and the ergonomic mistakes
   would already be baked in.
