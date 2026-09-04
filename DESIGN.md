# A Rust XSD Toolkit — Review and Design Proposal

> Status: P0–P2 implemented (component model, loading, content automata, UPA).
> Plan revised 2026-09-04 to put Python bindings next and move the
> [`xml2arrow`](https://github.com/mluttikh/xml2arrow) YAML generator into a
> separate package.
>
> Goal: **`xsdkit`**, a generic XSD reader in Rust with first-class Python
> bindings — parse a schema, query it as a model, validate and typed-read
> documents against it, and report the units a schema declares.

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
a datatype/facet layer, with a streaming typed reader (PSVI) and a units
layer over it — and Python bindings, because that is where most schema
processing actually happens. Skip code generation entirely; `xsd-parser`
already does it and it is the single largest sink of effort in this space.

**The consumer.** `xml2arrow`'s per-field `scale` and `offset`
(`value = value * scale + offset`) are *exactly* an affine unit conversion.
The README's own example hand-writes `offset: 273.15` for °C→K and
`scale: 100.0` for hPa→Pa. A schema-driven generator can emit those
automatically for every schema-fixed unit — but it is a **separate library**
built on `xsdkit`, not part of it (§3.0b).


### The package split

Three packages, not one:

| Package | What it is | Depends on |
|---|---|---|
| **`xsdkit`** (Rust + PyPI) | A generic XSD reader: component model, validation, typed reading, unit *extraction* | nothing XSD-external |
| **`xsd2arrow`** (separate) | Generates `xml2arrow` YAML from a schema | `xsdkit`, `xml2arrow` |
| a unit-conversion crate | Dimensional analysis and UCUM; nothing XSD-specific | — |

The split earns itself three ways. `xsdkit` keeps a dependency footprint of
essentially nothing, so it is adoptable by anyone with an XSD problem and no
interest in Arrow. `xsd2arrow` can move at `xml2arrow`'s release cadence
rather than the schema reader's. And unit *conversion* — dimension vectors,
affine factors, UCUM parsing — is not an XSD concern at all and shouldn't
live in a schema library; only unit **extraction** (finding the binding a
schema declares) is introspection, and that stays in `xsdkit`.

`xsd2arrow` is free on both crates.io and PyPI, checked alongside `xsdkit`.

### Name

**`xsdkit`** — crate, Python package and repository. Reserved-free on both
registries in every separator variant (`xsdkit` / `xsd-kit` / `xsd_kit`), no
existing project clash.

- `xsd` in the name keeps it findable by the search people actually run.
- `kit` covers reading, validating and introspecting honestly; `-parser`,
  `-model` and `2arrow` each name only one of them.
- **`xsd2arrow`** names the downstream package (§3.0b), which gives the
  `xml2arrow` sibling naming without boxing this library's scope in.

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

**In scope for `xsdkit`**

1. A schema component model for XSD 1.0, shaped for 1.1 from day one.
2. **Python bindings** — first-class, not an afterthought. Most schema
   processing happens in Python, and `xmlschema` is the only complete option
   there.
3. Schema-driven typed reading of XML documents (streaming PSVI).
4. Unit *binding extraction*: what unit does the schema declare for this
   value, and where does it come from.
5. XSD 1.1.

**Out of scope — code generation.** `xsd-parser` covers it, it is the single
largest effort sink in this space, and it is orthogonal to everything above.
Saying no here is what makes the rest finishable.

**Out of scope — the `xml2arrow` config generator.** A separate library
(§3.0b). It would drag `arrow` and `xml2arrow` into every dependency tree
that only wanted to read a schema.

**Out of scope — unit conversion arithmetic.** Dimensional analysis is not an
XSD concern. `xsdkit` reports what the schema *says*; converting is somebody
else'"'"'s crate.

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
│   ├── units/            # binding extraction profiles           (feature)
│   ├── diagnostics.rs    # codes, spans, severities
│   └── python.rs         # pyo3                                   (feature)
└── python/               # maturin project + type stubs
```

Split into a workspace only once a boundary has proved stable. `datatypes` is
the likeliest first extraction — it is genuinely reusable and has no upward
dependencies.

There is no `xml2arrow` module and no `arrow` dependency: that generator is
its own package (§3.0b).

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

### 3.7 The units layer

#### There is no standard, and that is a finding, not a gap

The obvious first question — *which standard do we implement?* — has a
definite answer: **none, because XSD deliberately declined to have one.**

- **It was proposed and rejected.** Olken and McCarthy put measurement units
  to the XML Schema Working Group in 1999, as an optional *facet* on scalar
  datatypes. Their own framing was "the question is not whether the
  XML-Schema Working Group will include measurement units as part of its
  recommendation, but whether we do so in a systematic, extensible way or in
  a less consistent, ad-hoc way." XSD adopted `dateTime` and `duration` and
  left units out. Ad-hoc won.
- **UnitsML never landed.** The OASIS technical committee formed in July 2003;
  Committee Specification Draft 04 was approved in December 2011 and it has
  not reached OASIS Standard since. It is also a markup language for
  *describing units themselves*, not a convention for annotating a schema.

What **is** standardised is the *vocabulary* — the code you write in the slot
— not the slot:

| Registry | Who mandates it |
|---|---|
| **UN/CEFACT Rec. 20/21** | UBL, CII, Peppol, XRechnung, EN 16931 e-invoicing |
| **UCUM** | HL7 FHIR, DICOM, LOINC, ISO 11240 — and GML, whose `uom` is "expected to be a unit symbol appearing in UCUM" when it is not a URI |
| **QUDT** | an RDF ontology, not an XSD binding |
| **VOUnits** | astronomy only |

#### What real schemas actually do

Measured, not read: `xsdkit` was pointed at GML 3.2, UBL 2.1 and the W3C
schema-for-schemas. All loaded with zero error diagnostics.

| Schema | Slot | Type | Use |
|---|---|---|---|
| GML 3.2 `measures.xsd` | `@uom` | `gml:UomIdentifier` — a URI *or* a UCUM symbol | required |
| UBL 2.1 | `@unitCode` | `xs:normalizedString`, values from UN/CEFACT Rec. 20 | required |
| WITSML/Energistics | `@uom` | an enumeration per quantity class, plus a dictionary | required |

**The survey found a bug**, which is the real argument for doing it. GML's
measure family is built from *vacuous* extensions — `gml:LengthType` is
literally `<extension base="gml:MeasureType"/>` — and `attribute_uses` did not
inherit through derivation, so all twenty GML measure types reported no `uom`
at all. Fixed before this section was written.

#### Consequence for the design

Profiles were the right instinct, for a better reason than "several
conventions exist": with no standard to implement, a pluggable extraction
layer is not a convenience, it is the only correct architecture. But the
survey narrows it considerably.

**Detect structurally, not by vendor.** A measure type has a recognisable
*shape*: simple content over a numeric base, carrying an attribute. That one
rule finds GML, UBL, WITSML and in-house schemas alike, with no per-vendor
code. Names (`uom`, `unitCode`, `unit`, `units`) are then only a hint for
*which* attribute is the unit when a type has several — a tie-breaker, not
the detector.

**Three binding shapes, not six.** The five "conventions" collapse once the
question is "what does the reader have to do to learn the unit":

```rust
pub enum UnitSource {
    /// A schema-`fixed` attribute, or a constant in `appinfo`. Known without
    /// reading any document — the only shape that compiles to a constant.
    Fixed(UnitRef),
    /// A `uom`/`unitCode` attribute carrying the unit per value.
    Attribute { name: QName },
    /// A key or URI into an external dictionary (WITSML `uomDict`,
    /// `gml:UnitDefinition`).
    Dictionary { key: QName, dict: DictId },
}
```

**Two vocabularies are worth shipping**, since between them they cover the
schemas that exist: UCUM and UN/CEFACT Recommendation 20. Both are code lists,
not algorithms; the arithmetic still lives outside `xsdkit` (§3.0b).

**State the limit plainly.** `Attribute` bindings vary per value, so they
cannot compile to a constant `scale`/`offset`. Only `Fixed` can. That is a
property of the schemas, not of this implementation, and it is the same
finding already recorded for `xml2arrow` in §3.9.

#### How common is `fixed`, and how is it written?

Measured over nine shipping schemas — GML 3.2 (`measures`, `units`,
`basicTypes`), OGC O&M 2.0, WaterML 2.0, CityGML 2.0, SensorML, SWE Common,
and UBL 2.1 — there are five unit-bearing attribute declarations and
**none of them uses `fixed`**. All five are `use="required"` with a free
value.

That is not an oversight either. An interchange standard exists to carry data
from many producers; pinning the unit would force every one of them to convert
before serialising. `fixed` is a **closed-schema** decision — right when you
own both ends and the unit is a modelling choice rather than data.

It is also the *only* shape that compiles to a constant `scale`/`offset`, so
it stays the highest-value case to detect even though it is the rarer one.

Writing it has one trap. A base measure type plus per-quantity subtypes cannot
pin the unit by **extension**: extension merges the base's attribute uses with
its own, and two uses may not share a name. It must be a **restriction**, and
the restriction may not widen — if the base says `use="required"`, so must the
derived:

```xml
<xs:complexType name="LengthMetres">
  <xs:simpleContent>
    <xs:restriction base="tns:MeasureType">
      <xs:attribute name="uom" type="xs:string" use="required" fixed="m"/>
    </xs:restriction>
  </xs:simpleContent>
</xs:complexType>
```

Declaring it fresh needs no restriction, because `xs:double` has no `unit`
attribute to clash with:

```xml
<xs:complexType name="Metres">
  <xs:simpleContent>
    <xs:extension base="xs:double">
      <xs:attribute name="unit" type="xs:string" fixed="m"/>
    </xs:extension>
  </xs:simpleContent>
</xs:complexType>
```

`xsdkit` currently accepts both the illegal extension and the widening
restriction: the Derivation Valid constraints are not implemented. Recorded in
`AGENTS.md` rather than fixed piecemeal.

### 3.8 `xsd2arrow` — the downstream config generator

A **separate package** (§3.0b), built on `xsdkit` and scheduled last. It is
sketched here because working the algorithm through is what surfaced the
requirements `xsdkit` has to satisfy — `possible_children`, `child_repeats`,
`child_is_optional` and the unit bindings all exist because of it.

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

### 3.9 Findings for `xml2arrow` itself

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

### 3.13 Python bindings

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
streaming reads (`schemas.read_typed("doc.xml")`, P4), unit bindings
(`schemas.units(profile="energistics")`, P5), and caching a compiled
`Schemas` to disk (§3.3) so repeated interpreter starts do not recompile UBL.

### 3.14 Staging

| Phase | Deliverable | Gate |
|---|---|---|
| ~~**P0**~~ | ~~`datatypes`: 19 primitives + derived, 14 facets~~ | **done** — facet composition, whiteSpace ordering |
| ~~**P1**~~ | ~~`load` + `model`: documents, resolver, include/import, arena, symbol tables~~ | **done** — W3C schema-for-schemas loads |
| ~~**P2**~~ | ~~`compile`: derivation, substitution closure, content automata, UPA~~ | **done** — 55 automata, 0 UPA findings on a valid schema |
| **P3** | **Python bindings** — the component model, Pythonic, on PyPI. Plus the `Resolver` encoding fix (below), which is breaking and must land before the API is wrapped. | wheels on 3.9–3.14; the fixture queried from Python; a Latin-1 schema loads |
| **P4** | `validate`: streaming validator + PSVI + `TypedReader`, exposed in Python as it lands | W3C instance tests; differential vs. `xmlschema` |
| **P5** | `units`: extraction profiles and dictionaries (GML, Energistics, appinfo, fixed-attribute) | fixed *and* per-instance bindings recovered from a real WITSML schema |
| **P6** | XSD 1.1: assertions, conditional type assignment, `openContent`, `override` | W3C 1.1 tests |
| **P7** | **`xsd2arrow`** — separate package, generates `xml2arrow` YAML | round-trip on `xml2arrow`'s own corpus + a real WITSML file |

**Why Python at P3.** Binding early is cheaper than binding late: it forces
the Rust API to be ergonomic while it is still cheap to change, and each
later phase adds a thin layer rather than a retrofit. It also means every
phase from here ships something usable to a Python user, rather than three
phases of Rust-only work before anything is reachable.

**Why the generator is last.** It is a separate package (§3.0b) with a
different dependency set and a different release cadence, and it is the only
deliverable here that is not a *generic* XSD capability. Everything it needs
from `xsdkit` — `possible_children`, `child_repeats`, `child_is_optional`,
unit bindings — is built by P5, so it can start any time after that without
blocking anything.

**Why units before XSD 1.1.** Units were one of the two original motivating
features, and the profiles are a few hundred lines. XSD 1.1 needs an XPath
2.0 engine, is the largest remaining item by a wide margin, and matters only
for schemas that actually use 1.1 — which most shipping schemas do not.

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
6. **One package or three?** Three (§3.0b). `xsdkit` stays a generic XSD
   reader with almost no dependencies; `xsd2arrow` and the unit-conversion
   arithmetic live outside it. The test is simple: would someone who has an
   XSD problem and no interest in Arrow still want this dependency? For the
   reader, yes. For a YAML generator that pulls in `arrow`, no.
7. **Bind Python early or late?** Early, at P3. The API is small enough now
   that binding it is a day's work and a good forcing function; after
   validation and units land it would be three times the surface and the
   ergonomic mistakes would already be baked in.
