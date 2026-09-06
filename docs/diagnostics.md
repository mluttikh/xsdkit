# Diagnostics

## Every error, not the first

Building a schema returns *all* the diagnostics. Someone repairing a forty-file
import graph needs the list; giving them one error, then another after the next
five-second build, is a way of turning a ten-minute job into an afternoon.

```text
error[XSD1201]: no type named `{urn:example}Missing`
  --> schemas/report.xsd:14
  help: check the spelling, or add an xs:import for its namespace
```

Four parts, and each one is addressable:

| Part | Field | |
|---|---|---|
| `error` | `severity` | `error`, `warning` or `note` |
| `XSD1201` | `code` | stable, greppable, safe to match on |
| the message | `message` | what is wrong |
| `--> file:14` | `spans` | where; there may be more than one |
| `help:` | `help` | what to do about it |

=== "Python"

    ```python
    schemas, diagnostics = xsdkit.load("report.xsd", conformance="lax")

    for d in diagnostics:
        print(d.code, d.severity, d.message)
        for span in d.spans:
            print("   ", span.uri, span.line, span.label)
        if d.help:
            print("   help:", d.help)

    errors = [d for d in diagnostics if d.is_error]
    ```

=== "Rust"

    ```rust
    # use xsdkit::{Conformance, SchemaSetBuilder};
    let (schemas, diagnostics) = SchemaSetBuilder::new()
        .conformance(Conformance::Lax)
        .file("report.xsd")
        .build_with_warnings();

    for d in diagnostics.iter() {
        println!("{d}");
    }
    ```

Match on `code`, never on the message text. Codes are part of the compatibility
promise; wording is not.

## Getting them, or not

Two shapes, because two situations.

| | Schema is expected to be sound | Schema is expected to be imperfect |
|---|---|---|
| Python | `SchemaSet.from_file(...)` raises `SchemaError` | `xsdkit.load(...)` returns `(schemas, diagnostics)` |
| Rust | `build()` → `Result<Schemas, Diagnostics>` | `build_with_warnings()` → `(Schemas, Diagnostics)` |

`SchemaError` carries the whole list, so the raising form loses nothing:

```python
try:
    schemas = xsdkit.SchemaSet.from_file("report.xsd")
except xsdkit.SchemaError as e:
    for d in e.diagnostics:
        print(d)
```

Validation is different again: an invalid *document* is an answer, not an
error, so `validate` returns a report and never raises for one.

```python
report = schemas.validate(xml)
report.is_valid
report.errors        # errors only
report.diagnostics   # warnings and notes as well
```

## Strict and lax

`Conformance::Strict` — the default — refuses to hand back a schema that had
any error. `Conformance::Lax` downgrades the violations that still leave usable
components behind, and a dangling `xs:import` is the one you will meet.

Use `lax` when you are reading someone else's schema to find out what is in it,
and `strict` when you are checking your own before shipping it.

## The codes

Grouped by the phase that raises them, which is also the order they can occur
in.

### 10xx — reading the document

| Code | Meaning |
|---|---|
| `XSD1001` | Malformed XML |
| `XSD1002` | Root element is not `xs:schema` |
| `XSD1003` | Unknown element in the XSD namespace |
| `XSD1004` | A required attribute is missing |
| `XSD1005` | An attribute's value is not legal there |
| `XSD1006` | Unsupported character encoding |
| `XSD1007` | Bytes contradict the declared encoding |
| `XSD1008` | `xs:annotation` in a position the content model forbids |
| `XSD1009` | A required child element is absent |

### 11xx — composition

| Code | Meaning |
|---|---|
| `XSD1101` | `schemaLocation` could not be resolved |
| `XSD1102` | `xs:include` of a document with a different target namespace |
| `XSD1103` | `xs:import` whose namespace does not match the document's |
| `XSD1104` | A construct this version does not support |

### 12xx — resolution

| Code | Meaning |
|---|---|
| `XSD1201` | A reference names something that does not exist |
| `XSD1202` | Two global components with the same name in one symbol space |
| `XSD1203` | A circular definition |

### 13xx — component validity

| Code | Meaning |
|---|---|
| `XSD1301` | A simple type is `list` and `union` and `restriction` at once |
| `XSD1302` | `minOccurs` / `maxOccurs` are not a legal range |
| `XSD1303` | A type is defined two incompatible ways |
| `XSD1304` | The content model violates Unique Particle Attribution |
| `XSD1305` | A facet that does not apply to this type |
| `XSD1306` | A facet whose value is not legal |
| `XSD1307` | Facets that contradict each other |
| `XSD1308` | A `default` or `fixed` that its own type rejects |
| `XSD1309` | Derivation blocked by `final` or `block` |
| `XSD1310` | A restriction that does not restrict its base |

### 20xx — instance validation

| Code | Meaning |
|---|---|
| `XSD2001` | No declaration for this element |
| `XSD2002` | An element the content model does not allow here |
| `XSD2003` | The content ended before the model was satisfied |
| `XSD2004` | A value its type rejects |
| `XSD2005` | An attribute not allowed here |
| `XSD2006` | A required attribute is missing |
| `XSD2007` | Text in element-only content |
| `XSD2008` | `xsi:type` names something unusable here |
| `XSD2009` | `xsi:nil="true"` on an element that has content |
| `XSD2010` | The type in force is abstract, so nothing validates against it |

## Rendering

`str(diagnostic)` gives the compiler-style block shown above. In a notebook the
same object renders as HTML, colour-coded by severity, and a
`ValidationReport` renders as a summary line and a table — see
[In a notebook](notebooks.md).
