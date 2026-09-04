# A Rust XSD Toolkit — Review and Design Proposal

> Status: proposal, 2026-09-04. Nothing implemented yet.
> Goal: a Rust crate (with a Python extension) that parses XSD, exposes the
> schema as a queryable model, drives typed XML → Arrow conversion, understands
> units of measure, and generates [`xml2arrow`](https://github.com/mluttikh/xml2arrow)
> YAML configs.

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
datatype libraries (`oxsdatatypes`, `xsd-types`). Nothing lets you *ask a
schema questions* — which is exactly what unit extraction and `xml2arrow`
config generation need.

**The proposal.** Build **`xsdkit`**: an arena-backed schema component model with an
explicit compile step, on top of a datatype/facet layer, with three consumers
layered on it — a streaming typed reader (PSVI), a units layer, and an
`xml2arrow` config generator. Skip code generation entirely; `xsd-parser`
already does it and it is the single largest sink of effort in this space.

**The happy accident.** `xml2arrow`'s per-field `scale` and `offset`
(`value = value * scale + offset`) are *exactly* an affine unit conversion.
The README's own example hand-writes `offset: 273.15` for °C→K and
`scale: 100.0` for hPa→Pa. A schema-driven generator can emit those
automatically for every schema-fixed unit. That is the shortest path from this
project to something useful.


### Name

**`xsdkit`** — crate, Python package and repository. Reserved-free on both
registries in every separator variant (`xsdkit` / `xsd-kit` / `xsd_kit`), no
existing project clash.

- `xsd` in the name keeps it findable by the search people actually run.
- `kit` covers all four capabilities honestly; `-parser`, `-model` and
  `2arrow` each name only one.
- The CLI binary is **`xsd2arrow`**, which gives the `xml2arrow` sibling
  naming without boxing the library's scope in.

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
codegen, unit extraction and config generation are all consumers of it, and
none of them should be allowed to shortcut into the syntax.

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
validate", and directly load-bearing for path matching in generated
`xml2arrow` configs.

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

No tool in this survey exploits that. For our purposes it is a direct source of
`links:` entries in an `xml2arrow` config and, more generally, of the
relational structure of the document. This is a differentiator worth taking.

### 1.8 XSD 1.1, briefly

`xs:assert` on complex types and the `assertions` facet on simple types (XPath
2.0); `xs:alternative` for Conditional Type Assignment (element type chosen by
an XPath test over the instance's *attributes*); `xs:openContent`;
`xs:override`; `xs:defaultAttributes`; wildcard `notQName`/`notNamespace`;
relaxed UPA; the `vc:minVersion`/`vc:maxVersion`/`vc:typeAvailable` conditional
attributes that let one document carry both 1.0 and 1.1 variants.

### 1.9 Annotations — the hook units hang on

`xs:annotation` carries `xs:documentation` (human) and `xs:appinfo`
(machine-readable, arbitrary foreign-namespace XML). Surveying how real
standards attach units of measure gives five distinct conventions:

| Convention | Example | Unit known at schema-compile time? |
|---|---|---|
| **Instance attribute** | GML/WITSML `<length uom="m">3.2</length>` | No — per value |
| **Schema-fixed attribute** | `<xs:attribute name="uom" type="xs:string" fixed="m"/>` | **Yes** |
| **`appinfo` annotation** | `<xs:appinfo><u:unit>Pa</u:unit></xs:appinfo>` | **Yes** |
| **Type naming** | `LengthMeasure`, `md:pressureUom` | Heuristically |
| **External dictionary** | WITSML `witsmlUnitDict.xml`, GML `gml:UnitDefinition` + `xlink:href="#m"` | Yes, with the dictionary |

The two "yes" rows are the ones that compile to `scale`/`offset` in an
`xml2arrow` config. The instance-attribute row cannot — it needs per-row
runtime conversion, which `xml2arrow` does not currently support (§3.7).

**Design consequence:** units must be a *pluggable annotation-extraction*
layer with built-in profiles, not a hardcoded feature.

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
element, and can they repeat?" — which is the question both the units layer and
the `xml2arrow` generator are made of.

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

**In scope**

1. A schema component model for XSD 1.0, shaped for 1.1 from day one.
2. Schema-driven typed reading of XML documents (streaming PSVI).
3. A units layer: extract unit bindings from schemas, convert values.
4. An `xml2arrow` config generator.
5. Python bindings.

**Out of scope — code generation.** `xsd-parser` covers it, it is the single
largest effort sink in this space, and it is orthogonal to everything above.
Saying no here is what makes the rest finishable.

**Out of scope for v1 — full XSD 1.1 validation.** Assertions and conditional
type assignment need an XPath 2.0 engine. Model the components; defer the
evaluator.

### 3.2 Layout

Follow the `xml2arrow` precedent: one crate, strict internal module boundaries,
feature-gated consumers, Python bindings behind a `python` feature.

```
xsdkit/
├── Cargo.toml            # features: validate, units, xml2arrow, python
├── src/
│   ├── lib.rs
│   ├── datatypes/        # value spaces, lexical mappings, facets, XSD regex
│   ├── model/            # arena, component types, IDs, queries      ← the product
│   ├── load/             # documents, resolvers, catalogs, include/import/override
│   ├── compile/          # resolution, derivation, subst groups, automata, UPA
│   ├── validate/         # streaming validator + PSVI            (feature)
│   ├── units/            # profiles, dictionaries, conversion     (feature)
│   ├── xml2arrow/        # config generator                       (feature)
│   ├── diagnostics.rs    # codes, spans, severities
│   └── python.rs         # pyo3                                   (feature)
└── python/               # maturin project + type stubs
```

Split into a workspace only once a boundary has proved stable. `datatypes` is
the likeliest first extraction — it is genuinely reusable and has no upward
dependencies.

*(Note: the project directory sits under `Projects/Python/`. If the intent is
Python-first, keep the maturin project at the repo root and the Rust crate in
`rust/` instead — but the Rust crate should still be independently useful.)*

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

- **Reuse `oxsdatatypes`** for `decimal` / `duration` / the date-time family.
  It is Oxigraph's, MIT/Apache-2.0, actively maintained, and already carries
  the ugly parts (arbitrary-precision decimal, timezone-aware comparison,
  the two duration subtypes).
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

1. **Counting states instead of unrolling.** `minOccurs=1 maxOccurs=5000`
   becomes one state plus a counter, not 5000 states. This is the difference
   between loading AUTOSAR and OOMing on it.
2. **Predicate-labelled transitions.** A wildcard's label is a namespace-set
   predicate (with 1.1's `notQName`/`notNamespace`), so 1.1's "element particle
   beats wildcard" is a transition-priority rule rather than a special case.

`xs:all` gets a bitset matcher, not an automaton.

**UPA falls out for free:** the model violates UPA exactly when some state has
two outgoing transitions with overlapping labels. Report it as a diagnostic
with both particles' source spans; downgrade to a warning in `Lax` mode.

The automaton is also what answers the two questions the config generator
needs: *which element names can appear here* (transition labels ∪ substitution
closure) and *can this element repeat* (is there a cycle through its state, or
`maxOccurs > 1`).

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

### 3.7 The units layer

```rust
pub struct UnitBinding {
    pub quantity: Option<QuantityKind>,   // "length", "pressure" — from schema or dict
    pub source:   UnitSource,
}

pub enum UnitSource {
    Fixed(Unit),                            // schema-fixed attribute, or appinfo
    Attribute { name: QName },              // uom="m" — per instance value
    Sibling   { path: RelPath },            // <value/><unit/> pairs
    Dictionary{ key: QName, dict: DictId }, // WITSML uomDict, gml:UnitDefinition
    None,
}
```

**Extraction is pluggable.** A `UnitProfile` trait, with built-ins matching the
conventions found in §1.9:

- `FixedAttributeProfile` — any `xs:attribute` with a `fixed` value whose name
  is in a configurable set (`uom`, `unit`, `units`, `unitCode`).
- `AppinfoProfile` — a user-supplied selector into `xs:appinfo`.
- `GmlProfile` — `uom` attribute, `gml:UnitDefinition` dictionaries,
  `xlink:href="#m"` resolution.
- `EnergisticsProfile` — WITSML/PRODML `uom` + `witsmlUnitDict.xml`, quantity
  class from the measure type.
- `TypeNameProfile` — regex over type names (`(?<q>.*)Measure`).
- `UnitsMlProfile`.

**Conversion is runtime, not compile-time.** `uom` (the crate) does
zero-cost dimensional analysis with types known at compile time; we don't know
the units until we read the schema. So:

```rust
pub struct Unit { dim: Dimension, factor: f64, offset: f64 }  // affine
pub struct Dimension([i8; 7]);   // SI base exponents
```

Conversion is legal iff dimensions match; the result is
`value * (from.factor / to.factor) + (from.offset − to.offset)/to.factor`.
Two rules that catch real bugs: **reject offset units inside products**
(`°C/m` is meaningless), and **reject non-linear units** (dB, pH) unless a
plugin supplies them. Ship a UCUM-backed default `UnitSystem` behind a feature
(`ucum` or `octofhir-ucum-core`), plus loaders for the GML and Energistics
dictionaries — but keep `UnitSystem` a trait so no one convention is baked in.

**The payoff.** `xml2arrow` already computes `value = value * scale + offset`
per field. That is precisely affine unit conversion:

| Conversion | `scale` | `offset` |
|---|---|---|
| °C → K | 1.0 | 273.15 |
| hPa → Pa | 100.0 | 0.0 |
| ft → m | 0.3048 | 0.0 |
| °F → K | 5/9 | 255.372… |

So for every `UnitSource::Fixed` binding, the generator can emit the conversion
into the YAML with no runtime support needed at all — automating exactly what
the `xml2arrow` README's own example does by hand.

`UnitSource::Attribute` (per-row units) is the one case that cannot be
expressed. It needs either a post-pass over the Arrow batch or a new
`xml2arrow` feature (a field whose `scale` is looked up from a sibling
attribute). Worth raising as an issue there.

### 3.8 The `xml2arrow` config generator

The algorithm, given a compiled `Schemas` and a root element:

1. **Walk** the element tree from the root, expanding types, substitution
   groups, and recursion up to a depth bound.
2. **Compute effective repeatability.** An element is *repeating* if its
   particle has `maxOccurs > 1` **or** it sits under a repeating ancestor
   without an intervening repeating boundary.
3. **Table rule:** a repeating element becomes a table's `row:` element. Its
   nearest repeating ancestor becomes the `links: - parent:` target. Its
   non-repeating simple-content descendants become that table's `fields:` with
   *relative* `path:`s (the modern `xml2arrow` spelling).
4. **Type map** XSD → `DType`:

   | XSD | `DType` | Note |
   |---|---|---|
   | `boolean` | `Boolean` | |
   | `byte`/`unsignedByte` … `long`/`unsignedLong` | `Int8`…`UInt64` | exact |
   | `integer`, `nonNegativeInteger`, `decimal` | `Int64` **if** `totalDigits`/`maxInclusive` prove it fits, else `Float64`, else `Utf8` | **facets used as evidence — a real advantage over hand-written configs** |
   | `float` / `double` | `Float32` / `Float64` | |
   | `string` and derived, `anyURI`, `QName` | `Utf8` | |
   | enumerated simple type | `Utf8` | ideally `Dictionary(Int32, Utf8)` — gap, §3.9 |
   | `date`, `dateTime`, `time`, `duration`, `g*` | `Utf8` | **gap**, §3.9 |
   | `hexBinary`, `base64Binary` | `Utf8` | gap |
   | list variety | `Utf8` (joined) | ideally `List<T>` — gap |
   | union | common supertype, else `Utf8` | |

5. **Nullability is derivable exactly**: `nullable: true` iff `minOccurs = 0`,
   or `nillable = true`, or the attribute `use` is `optional`, or the element
   lies inside a `choice` or optional branch. Humans get this wrong constantly;
   the schema states it precisely.
6. **Value policies from the schema**: `nillable` → `on_missing: null`;
   `xs:token`-derived types → `trim: true` (the `whiteSpace: collapse` facet
   *is* the trim instruction); a `default`/`fixed` value → note it in a comment.
7. **`links:` from `xs:keyref`** where the schema declares one, rather than
   inferring purely from nesting.
8. **Namespace decision.** Inspect `elementFormDefault`. Keep `strip_namespaces:
   true` by default, but **emit a warning when two namespaces in the schema set
   have colliding local names** — a correctness trap that is invisible in the
   XML but obvious in the schema.
9. **Pruning.** Real schemas explode (UBL would yield hundreds of tables).
   Needs: include/exclude path globs, a max depth, a
   "restrict to what appears in this sample document" mode (schema ∩ instance),
   and a flatten threshold for one-child-deep wrappers.
10. **Verify.** Generated config + sample XML → run `xml2arrow` with
    `error_on_unmatched_fields: true` → every configured field must match.
    Build this into the generator as a `--verify <sample.xml>` flag; it is a
    self-checking test loop in the style of `xml2arrow`'s own corpus tests.

### 3.9 Findings to feed back into `xml2arrow`

Working through the type map surfaced four concrete gaps:

1. **No temporal `DType`.** `xs:date`/`dateTime` are extremely common and
   currently land in `Utf8`. Proposal: `Date32`, `Time64`,
   `Timestamp(Microsecond, tz)`.
2. **No dictionary type.** XSD enumerations are a perfect
   `Dictionary(Int32, Utf8)` and often high-cardinality-savings.
3. **No list/binary types.** `NMTOKENS`, `IDREFS` and any list-variety simple
   type want Arrow `List<T>`; `hexBinary`/`base64Binary` want `Binary`.
4. **No per-row unit conversion.** `scale`/`offset` are per-field constants;
   `uom="ft"` varying per row cannot be expressed.

All four are `DType` additions, and `DType` is already `#[non_exhaustive]` — so
adding variants stays non-breaking. Worth filing before the generator ships.

### 3.10 Diagnostics

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

### 3.11 Security model

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

### 3.12 Testing

| Layer | Approach |
|---|---|
| **Conformance** | The **W3C XML Schema Test Suite** (~26k cases across 1.0 and 1.1). Wire it up in Phase 1 and publish a per-feature-area pass rate. Highest-leverage single decision in the plan. |
| **Real-world corpus** | XHTML, SVG, GML, UBL, OGC, WITSML, ISO 20022, XBRL, AUTOSAR. Merely *loading* these is a strong smoke test. |
| **Differential** | Run the same corpus through Python `xmlschema` and diff verdicts. It is the most complete OSS 1.1 implementation and makes an excellent oracle. `uppsala` as a second. |
| **Round-trip** | schema → `xml2arrow` config → parse sample XML → assert the Arrow schema matches independently-derived expectations. Frozen-corpus style, matching `tests/corpus/` in `xml2arrow`. |
| **Property** | Facet composition, automaton equivalence under particle rewrites, regex transpilation (XSD pattern vs. transpiled `regex` on generated strings). |
| **Fuzzing** | `cargo-fuzz` on the schema loader and the regex transpiler. |
| **Bench** | `criterion` + CodSpeed, already in use. Track: compile time for UBL/AUTOSAR, validation throughput MB/s, `Schemas` deserialize time. |

### 3.13 Python extension

Follow the `xml2arrow` pattern: a `python` feature on the Rust crate, `pyo3`,
`maturin`, abi3 wheels.

```python
import xsdkit

schemas = xsdkit.SchemaSet.from_file("witsml_v1.4.1.1.xsd", catalog="catalog.xml")

# introspection
well = schemas.elements["{witsml}well"]
well.type.attributes["uom"].fixed          # 'm'
[e.name for e in schemas.substitutes(well)]

# typed streaming read
for ev in schemas.read_typed("well.xml"):
    ...                                     # values already Decimal/datetime/bool

# units
schemas.units(profile="energistics").binding(well.type, "md")
# UnitBinding(quantity='length', source=Attribute('uom'))

# the payoff
cfg = schemas.to_xml2arrow_config(root="wells", units="SI", verify="sample.xml")
open("wells.yaml", "w").write(cfg)
```

- Value mapping: `xs:decimal` → `decimal.Decimal`, `dateTime` → tz-aware
  `datetime`, `duration` → a dedicated class, `QName` → `(ns, local)`.
- Release the GIL around parse/compile/validate (`py.allow_threads`).
- Cache the compiled `Schemas` to disk (§3.3) so repeated interpreter starts
  don't recompile UBL.
- Typed exception hierarchy, and the `xml2arrow` discipline: adding an error
  variant **must** update the `PyErr` conversion, guarded by an exhaustiveness
  test.

### 3.14 Staging

| Phase | Deliverable | Gate |
|---|---|---|
| **P0** | `datatypes`: 19 primitives + derived, 14 facets, XSD→`regex` transpiler | W3C datatype tests passing |
| **P1** | `load` + `model`: documents, resolver, catalog, include/import/redefine/override, arena, symbol tables | UBL, GML, XHTML load without error |
| **P2** | `compile`: derivation chains, substitution closure, content automata, UPA | W3C structures tests; differential vs. `xmlschema` |
| **P3** | **`xml2arrow` generator** — first user-visible payoff | round-trip on `xml2arrow`'s own corpus + a real WITSML file |
| **P4** | `units`: profiles, dictionaries, conversion; feeds `scale`/`offset` into P3 | °C→K and hPa→Pa emitted automatically for a real schema |
| **P5** | `validate`: streaming validator + PSVI + `TypedReader` | W3C instance tests |
| **P6** | Python bindings (thin from P3 onward, complete here) | wheels on 3.10–3.14 |
| **P7** | XSD 1.1: assertions, CTA, `openContent`, `override` | W3C 1.1 tests |

P3 before P5 is deliberate: the config generator needs the component model and
repeatability analysis, not validation. It gets something useful into your
hands roughly half-way through the plan instead of at the end.

### 3.15 Decisions worth making explicitly

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
4. **Reuse `oxsdatatypes`?** Yes. Decimal, duration and the date-time family
   are weeks of subtle work already done and battle-tested in Oxigraph.
5. **Codegen?** No — see §3.1. This is the load-bearing exclusion.
