# Querying the model

Everything on this page runs against
[`report.xsd`](examples/report.xsd).

=== "Python"

    ```python
    import xsdkit

    schemas = xsdkit.SchemaSet.from_file("report.xsd")
    report = schemas["{urn:example}report"]
    ```

=== "Rust"

    ```rust
    use xsdkit::{Diagnostics, SchemaSetBuilder};

    fn main() -> Result<(), Diagnostics> {
        let schemas = SchemaSetBuilder::new()
            .file("report.xsd")
            .compile()
            .into_result()?;
        let report = schemas.element(Some("urn:example"), "report").unwrap();
        Ok(())
    }
    ```

## Finding what a schema declares

=== "Python"

    A `SchemaSet` is a mapping over the globals your documents declare.

    ```python
    len(schemas)                        # 6
    "{urn:example}report" in schemas    # True
    list(schemas)
    # ['{urn:example}report', '{urn:example}Currency', '{urn:example}Item',
    #  '{urn:example}Money', '{urn:example}Report', '{urn:example}Sku']

    # A real mapping, not merely something shaped like one:
    schemas.keys()                      # the same names, as a list
    schemas.items()                     # (name, component) pairs
    dict(schemas)                       # so this works
    ```

    Elements come before types in iteration. For a list of one kind:

    ```python
    schemas.elements     # [<Element {urn:example}report>]
    schemas.types        # the six declared types, built-ins excluded
    schemas.documents    # what was read, with target namespaces
    ```

    The lookup methods return `None` when there is nothing, for when absence is
    an ordinary answer; subscripting raises `KeyError`, for when it is a
    mistake.

    ```python
    schemas.element("urn:example", "report")   # <Element …> or None
    schemas["{urn:example}nope"]               # KeyError
    ```

=== "Rust"

    Name lookups return a reference — a borrow of the schema plus an id, so
    following one allocates nothing — or `None` when the schema declares no
    such name.

    ```rust
    use xsdkit::Schemas;

    fn look_around(schemas: &Schemas) {
        let report = schemas.element(Some("urn:example"), "report");   // Option<ElementRef>
        let money = schemas.type_(Some("urn:example"), "Money");       // Option<TypeRef>

        for e in schemas.global_elements() {
            println!("{}", e.display_name());
        }
        for t in schemas.global_types() {
            // Unlike Python's `schemas.types`, this includes the 50 built-ins:
            // they are real components here, not a special case.
            println!("{}", t.display_name());
        }
    }
    ```

    When the id is what you mean to keep — as a map key, or to compare — the
    `_id` forms hand it over directly, and `Schemas::get` goes back the other
    way.

    ```rust
    use xsdkit::Schemas;

    fn ids(schemas: &Schemas) -> Option<()> {
        let id = schemas.element_id(Some("urn:example"), "report")?;
        let report = schemas.get(id);
        assert_eq!(report.id(), id);
        Some(())
    }
    ```

## Walking the tree

An element behaves as its children, so you navigate a schema without a
`.type` hop at every level.

=== "Python"

    Sized, iterable and subscriptable by name.

    ```python
    [child.local_name for child in report]
    # ['title', 'issued', 'item']

    report["item"]["price"].type.qname
    # '{urn:example}Money'

    len(report)          # 3
    ```

    A bare local name is enough, because a child is almost always in its
    parent's namespace. Clark notation and `(namespace, local)` pairs work too.

=== "Rust"

    ```rust
    use xsdkit::Schemas;

    fn walk(schemas: &Schemas) -> Option<()> {
        let report = schemas.element(Some("urn:example"), "report")?;

        let names: Vec<&str> = report.children().map(|c| c.local_name()).collect();
        // ["title", "issued", "item"]

        let price = report.child("item")?.child("price")?;
        println!("{}", price.type_of().display_name());
        // {urn:example}Money
        Some(())
    }
    ```

    `child` takes a local name for the same reason Python's subscript does.
    Turning a name into text never goes through the interner: `local_name`,
    `namespace` and `display_name` are on every reference, and
    `Schemas::local_of` and `namespace_of` do the same for a bare `QName`.

!!! tip "Read it once, whole"

    `element.tree()` prints the shape rather than making you walk it.

    ```python
    print(report.tree())
    ```

    ```text
    report: {urn:example}Report
      @id
      title: xs:string
      issued: xs:date
      item+: {urn:example}Item
        @sku
        @quantity?
        price: {urn:example}Money
          @currency
        note?: xs:string
    ```

    `?` optional, `+` one or more, `*` any number, nothing for exactly once;
    `@name` for attributes. Recursion stops where the shape starts repeating,
    so a self-referential schema prints rather than hangs. In a notebook the
    same call renders as a colour-coded tree — see [In a notebook](notebooks.md).

### Children come from everywhere

`children` is not "the elements written inside this type's `xs:sequence`". It
is every element that may actually appear there, with

- content inherited through `xs:extension` already included,
- `xs:group` references expanded,
- **substitution groups closed transitively**, abstract heads skipped.

That resolution is the whole point of working against components. Doing it
yourself from documents is where XSD tooling goes to die.

## Occurrence belongs to the pair

How often a child may appear is a fact about the *parent and child together*,
not about the declaration, because the same element can be referenced with
different occurrence constraints in different places. So subscripting or
iterating a parent gives a `Child`: the declaration, plus how it may appear
*here*. A `Child` answers everything an `Element` does, and `child.element`
gets the bare declaration back.

=== "Python"

    ```python
    item = report["item"]

    item.repeats                     # True  — maxOccurs="unbounded"
    item.optional                    # False — minOccurs defaults to 1

    item["note"].optional            # True  — minOccurs="0"

    for child in report:
        print(child.local_name, child.repeats, child.optional)
    ```

=== "Rust"

    ```rust
    use xsdkit::TypeRef;

    fn describe(ty: TypeRef<'_>) {
        for child in ty.children() {
            println!("{} {} {}", child.display_name(), child.repeats(), child.optional());
        }
    }
    ```

This is exactly the pair of questions a table-versus-column decision needs when
you are mapping a schema onto a relational or columnar shape.

!!! tip "Ask for all of them at once"

    Both facts come from walking the content model, and asking for the
    children walks it once for the whole type. In Rust the per-child
    predicates `Schemas::child_repeats` and `Schemas::child_is_optional` are
    still there for a single question about a single child — but called in a
    loop they re-walk the model per child, which on a type with hundreds of
    them (ordinary in GML, UBL and WITSML) measures about 40× slower.

## Attributes

=== "Python"

    ```python
    [(a.local_name, a.required, a.type.qname, a.default)
     for a in report["item"].attributes]
    # [('sku',      True,  '{urn:example}Sku', None),
    #  ('quantity', False, '{http://www.w3.org/2001/XMLSchema}positiveInteger', '1')]
    ```

=== "Rust"

    ```rust
    use xsdkit::Schemas;

    fn attributes(schemas: &Schemas) -> Option<()> {
        for a in schemas.element(Some("urn:example"), "report")?.child("item")?.attributes() {
            println!(
                "{} required={} type={} default={:?}",
                a.local_name(),
                a.is_required(),
                a.type_of().display_name(),
                a.default(),
            );
        }
        Some(())
    }
    ```

You get **attribute uses**, not bare declarations: the use carries `required`,
`default` and `fixed`, because those belong to the place the attribute is used
rather than to the attribute itself. Attribute groups are already flattened in,
transitively, and so are the attributes inherited from base types.

## Types

=== "Python"

    ```python
    money = schemas["{urn:example}Money"]

    money.is_complex        # True
    money.content           # 'simple'  — a simple value with attributes on it
    money.base.qname        # '{http://www.w3.org/2001/XMLSchema}decimal'
    money.derivation        # 'extension'
    money.derives_from(schemas.type("http://www.w3.org/2001/XMLSchema", "decimal"))
    # True

    [t.qname for t in money.base_chain]
    # ['{urn:example}Money', '…}decimal', '…}anyAtomicType', '…}anySimpleType', '…}anyType']
    ```

=== "Rust"

    ```rust
    use xsdkit::Schemas;

    fn types(schemas: &Schemas) -> Option<()> {
        let money = schemas.type_(Some("urn:example"), "Money")?;
        assert!(money.is_complex());

        // Walking up stops on its own: `xs:anyType` is its own base, so `base`
        // reports `None` there rather than looping.
        let mut t = money;
        while let Some(base) = t.base() {
            println!("{}", base.display_name());
            t = base;
        }
        Some(())
    }
    ```

`content` is one of `empty`, `simple`, `element-only`, `mixed`. For simple
types, `variety` is `atomic`, `list` or `union`, with `item_type` and
`member_types` for the latter two, and `primitive` naming what it ultimately
reduces to.

## Facets, composed

```python
currency = schemas["{urn:example}Currency"]
currency.facets.enumeration
# ['EUR', 'USD', 'GBP']

schemas["{urn:example}Sku"].facets.patterns
# [['[A-Z]{2}-[0-9]{4}']]
```

`facets` gives the constraints **in force**, composed down the whole
restriction chain — a type that declares only `maxLength` still reports its
base's `minLength`. `declared_facets` gives only what this restriction step
wrote. The first is what validation applies; the second is what the schema
author typed.

Two composition rules are easy to get wrong and are worth knowing:

- **Patterns OR within a step, AND across steps.** That is why `patterns` is a
  list of lists: the outer list is restriction steps, the inner one is the
  alternatives declared at that step.
- **The innermost enumeration wins.** A restriction may only narrow.

Bounds and enumerations are the **lexical forms the schema wrote**, not typed
values, because a facet constrains the lexical space as much as the value
space. Put one through `validate` for the value.

## Validating a single value

```python
currency.validate("EUR")     # 'EUR'
currency.is_valid("ZZZ")     # False
currency.validate("ZZZ")
# ValueError: enumeration: `ZZZ` is not one of the 3 permitted values

schemas.type("http://www.w3.org/2001/XMLSchema", "date").validate("2024-12-01")
# datetime.date(2024, 12, 1)
```

`validate` applies `whiteSpace` first, then parses, then checks the composed
facets, and returns the value as its closest native Python type. This is the
same machinery the document validator uses, so a value that passes here passes
there.

## Does this sequence fit?

```python
N = "{urn:example}"

report.type.accepts([N+"title", N+"issued", N+"item"])            # True
report.type.accepts([N+"title", N+"issued", N+"item", N+"item"])  # True
report.type.accepts([N+"issued", N+"title", N+"item"])            # False — order
report.type.accepts([N+"title", N+"issued"])                      # False — item required
```

Answered by running the compiled content automaton, not by pattern-matching
particles. Rust says the same thing the same way:

```rust
use xsdkit::Schemas;

fn title_alone_is_enough(schemas: &Schemas) -> Option<bool> {
    let report = schemas.element(Some("urn:example"), "report")?;
    let title = schemas.qname(Some("urn:example"), "title")?;
    Some(report.accepts([title]))
}
```

The matcher underneath is available too, for stepping through a document and
asking `accepts_end()` when you reach the end rather than judging a whole
sequence at once:

```rust
use xsdkit::{Schemas, TypeId};

fn step_through(schemas: &Schemas, ty: TypeId) -> Option<bool> {
    let mut m = schemas.match_content(ty)?;
    let title = schemas.qname(Some("urn:example"), "title")?;
    Some(m.step(title) && m.accepts_end())
}
```

Content models compile to **Glushkov position automata**. Unique Particle
Attribution checking falls out of the same structure rather than being a
separate pass, and `xs:all` gets per-member counters instead of `n!` regex
paths.

## Substitution groups

=== "Python"

    ```python
    head = schemas["{urn:example}report"]
    head.substitutes
    # every element that may appear where `head` may, transitively,
    # including `head` itself unless it is abstract
    ```

=== "Rust"

    ```rust
    use xsdkit::Schemas;

    fn members(schemas: &Schemas) -> Option<()> {
        let head = schemas.element(Some("urn:example"), "shape")?;
        for e in head.substitutes() {
            println!("{}", e.local_name());
        }
        Some(())
    }
    ```

Already closed for you, and already reflected in the children, so an element
with twelve substitutes shows all twelve as possible children of its parent.

!!! warning "Membership is not permission"

    `block` on a head bars substitution, or bars the derivation method a
    member's type used to reach the head's. `substitutes` applies it — so it
    answers *what a document may actually name here*, and agrees with both the
    content model and the validator.

    The other question, *who is in the group*, is
    `Schemas::substitution_group` in Rust. It ignores `block`, so it can
    report members that no document may use. Reach for it only when you mean
    the group itself; the two used to be one word apart and picking wrong was
    a silent wrong answer.

## Next

- [Validating documents](validation.md) — from a schema to a verdict and typed
  values.
- [Python API](reference/python.md) — every method, with types.
