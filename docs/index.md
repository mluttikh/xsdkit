# xsdkit

A **generic XSD reader**: parse W3C XML Schema into a queryable schema
component model, in Rust and Python.

XSD is three languages at once — schema *documents*, schema *components*, and
validation *semantics* defined over those components. The specification defines
every rule it has against the middle layer. Most tools skip it: they read
documents and emit code, or they read documents and answer `valid`/`invalid`.
`xsdkit` builds the middle layer and hands it to you.

That is the difference between asking *"is this document valid?"* and asking
*"what may go inside a `report`, may it repeat, and what type is it?"* — the
second question is the one you have when you are generating a form, mapping a
schema onto a dataframe, or writing a converter.

---

## In thirty seconds

=== "Python"

    ```python
    import xsdkit

    schemas = xsdkit.SchemaSet.from_file("report.xsd")
    report = schemas["{urn:example}report"]

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

=== "Rust"

    ```rust
    use xsdkit::SchemaSetBuilder;

    let schemas = SchemaSetBuilder::new().file("report.xsd").build()?;
    let report = schemas.element(Some("urn:example"), "report").unwrap();
    let ty = schemas[report].type_id;

    for child in schemas.possible_children(ty) {
        println!(
            "{}  repeating={}  optional={}",
            schemas.display_name(schemas[child].name),
            schemas.child_repeats(ty, child),
            schemas.child_is_optional(ty, child),
        );
    }
    # Ok::<_, xsdkit::Diagnostics>(())
    ```

Every example on this site runs against
[`report.xsd`](examples/report.xsd), which is 60 lines and worth a glance.

---

## What it gives you

<div class="grid cards" markdown>

-   __The component model, not a parse tree__

    Types, elements, attributes, particles, model groups, wildcards, identity
    constraints, notations and annotations — with all seven symbol spaces kept
    separate, attribute groups flattened, and substitution groups closed.

    [:octicons-arrow-right-24: The component model](concepts.md)

-   __Answers about content, from an automaton__

    Content models compile to Glushkov position automata, so *"which children,
    can they repeat, can they be absent, does this sequence fit"* are lookups
    rather than a walk over particles you have to interpret yourself.

    [:octicons-arrow-right-24: Querying the model](querying.md)

-   __Validation with typed values__

    One streaming pass, and a PSVI where values arrive as `42` and
    `Decimal("19.95")` — not strings for you to parse a second time.

    [:octicons-arrow-right-24: Validating documents](validation.md)

-   __Diagnostics that name the problem__

    Stable codes, source spans, help text, and *every* error rather than the
    first — because someone fixing a 40-file import graph needs the list.

    [:octicons-arrow-right-24: Diagnostics](diagnostics.md)

</div>

---

## Status

The component model, loading, composition, content automata, instance
validation and the Python bindings work, and are measured against the W3C XML
Schema Test Suite on every change.

| | |
|---|---|
| valid schemas accepted | **99.7%** (5,231 / 5,247) |
| invalid schemas rejected | **66.7%** (320 / 480) |
| documents judged correctly | **98.6%** (21,281 / 21,575) |

The gap in the second row is the honest description of what this is.
`xsdkit` reads real schemas well; it does not yet enforce most of the
specification's *validity constraints*, so a schema it accepts is not thereby
a valid schema. If you need a conformance checker, reach for Xerces or Saxon.
If you need to read a schema that already works, this is built for that.

[:octicons-arrow-right-24: The full conformance picture](project/conformance.md)

Next up is XSD 1.1 assertions and conditional type assignment. Code generation
is permanently out of scope — that is
[`xsd-parser`](https://crates.io/crates/xsd-parser)'s job.

---

## Where to go next

- Never used it before → [Installation](install.md), then
  [Loading schemas](loading.md).
- Want to know what a "schema component" is and why it is the layer that
  matters → [The component model](concepts.md).
- Looking for a specific method → [Python API](reference/python.md) or
  [Rust API](reference/rust.md).
- Deciding whether to depend on it → [Conformance](project/conformance.md),
  [Security](project/security.md), [Performance](project/performance.md).
