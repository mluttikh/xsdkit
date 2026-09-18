# Validating documents

Validation is a **streaming** pass over the document, driven by the compiled
content automata. It answers two different questions, and which one you want
decides which method to call:

- *Is this document valid?* → `validate`
- *What does this document say, as typed values?* → `iter_typed`

Both do the same work. The second just hands you the values on the way past.

## Is it valid?

```python
import xsdkit

schemas = xsdkit.SchemaSet.from_file("report.xsd")
xml = open("report.xml").read()
report = schemas.validate(xml)

report.is_valid          # True
bool(report)             # the same thing, for `if report:`
```

An invalid document is an **answer, not an exception**. `validate` never raises
for one — the only things that raise are a document that is not XML at all, or
a schema that would not build.

```python
broken_xml = """<report xmlns="urn:example" id="r1">
  <title>November orders</title>
  <issued>2024-13-45</issued>
  <item sku="nope"><price currency="CHF">1.00</price></item>
</report>"""

report = schemas.validate(broken_xml)
report.is_valid          # False

for d in report.errors:
    print(d)
```

```text
error[XSD2004]: `{urn:example}issued`: `2024-13-45` is not a valid xs:date: 13 is not a month
  --> <instance>:4

error[XSD2004]: attribute `sku`: pattern: `nope` does not match ^(?:(?:[A-Z]{2}\-[0-9]{4}))$
  --> <instance>:5

error[XSD2004]: attribute `currency`: enumeration: `CHF` is not one of the 3 permitted values
  --> <instance>:6
```

Every error, with a line number, not just the first. `report.diagnostics` has
the warnings and notes too; `report.errors` is the errors alone.

A document given as a path is named by that path. One given as text or bytes
is called `<instance>` unless you pass `uri="orders/report.xml"`.

=== "Rust"

    ```rust
    use xsdkit::Schemas;

    fn check(schemas: &Schemas, xml: &str) {
        let report = schemas.document_validator().validate(xml);
        if !report.is_valid() {
            for d in report.diagnostics.iter() {
                println!("{d}");
            }
        }
    }
    ```

## Reading it into typed values

The validator already knows the type of every element and attribute it walks —
that is what it is checking against. Asking for the values gives you that work
instead of discarding it, as a **PSVI**: a post-schema-validation infoset.

=== "Python"

    ```python
    here = None
    for ev in schemas.iter_typed(open("report.xml").read()):
        if ev.kind == "start":
            here = ev.local_name
        elif ev.kind == "text":
            print(f"{here:8} {type(ev.value).__name__:9} {ev.value!r}")
    ```

    ```text
    title    str       'November orders'
    issued   date      datetime.date(2024, 12, 1)
    price    Decimal   Decimal('19.95')
    note     str       'backordered'
    price    Decimal   Decimal('4.50')
    ```

=== "Rust"

    `validate_with` runs the same single pass and hands each event to a
    callback. A callback rather than an iterator because the events are
    produced inside the parse, and suspending that would mean either a thread
    or a self-referential struct.

    ```rust
    use xsdkit::{Schemas, instance::PsviEvent};

    fn values(schemas: &Schemas, xml: &str) {
        let report = schemas.document_validator().validate_with(xml, |ev| {
            if let PsviEvent::Text { value: Some(v), lexical, .. } = ev {
                println!("{v:?} from {lexical:?}");
            }
        });
        assert!(report.is_valid());
    }
    ```

`Decimal("19.95")`, not `"19.95"`. `date(2024, 12, 1)`, not `"2024-12-01"`.
Reparsing those strings yourself is not just repeated work — it is where the
subtle wrongness lives, because `float("19.95")` is not the value the schema
said and `datetime.strptime` does not implement `xs:date`.

### Which types you get

| XSD | Python |
|---|---|
| `xs:string` and its derivatives | `str` |
| `xs:boolean` | `bool` |
| `xs:int`, `xs:integer`, `xs:long`, … | `int` |
| `xs:decimal` | `decimal.Decimal`, in the scale it was written: `4.50` stays `4.50` |
| `xs:float`, `xs:double` | `float` |
| `xs:hexBinary`, `xs:base64Binary` | `bytes` |
| `xs:dateTime` | `datetime.datetime` |
| `xs:date` | `datetime.date` |
| `xs:time` | `datetime.time` |
| `xs:dayTimeDuration` | `datetime.timedelta` |
| `xs:duration`, `xs:gYear`, `xs:gMonthDay`, … | `str`, canonical form |
| a value `datetime` cannot hold exactly | `str`, canonical form |
| list types | `list` of the item type |

`xs:duration` stays a string on purpose: months and seconds are not
commensurable, so no `timedelta` can represent `P1M` faithfully. Guessing 30
days would be a silent, plausible, wrong answer. `xs:dayTimeDuration` has no
such problem, so it becomes a `timedelta`.

A value `datetime` cannot hold exactly keeps its canonical lexical form,
rather than failing to convert or being rounded into one it can hold:

- a year outside 1 to 9999 — XSD's year is unbounded in both directions, and
  XSD 1.1 has a year zero — or a `dayTimeDuration` of a billion days;
- an `xs:date` with a timezone, such as `2024-12-01Z`, because `datetime.date`
  has no room for one;
- a time or a duration with digits below the microsecond, such as
  `PT0.0000001S`.

An `xs:float` arrives as the shortest decimal that reads back as the same
32-bit value, so `0.1` is `0.1` rather than `0.10000000149011612`.

### The outcome comes at the end

`iter_typed` reads the document as it validates it, on a thread of its own,
so memory stays flat however large the document is. Whether a document is
valid is only known at its end, and that is where the report is:

```python
events = schemas.iter_typed(xml)
for ev in events:
    ...
events.report.is_valid       # once every event has been read
```

Reading `report` sooner raises `RuntimeError`; to know first, call `validate`.
An iterator dropped part way, after a `break` say, stops the reading as well,
rather than validating the rest of the document for nobody.

`iter_typed` composes the way an iterator should — with `enumerate`,
`itertools`, generator expressions. For everything at once, `read_typed`
returns `(events, report)`, with the events as a list.

### What is on an event

```python
ev.kind                 # 'start' | 'text' | 'end'
ev.name, ev.local_name  # ('urn:example', 'price'), 'price'; None on a text event
ev.declaration          # the Element declaration; None under skip, or lax with no match
ev.type                 # the type in force, after any xsi:type override
ev.type_from_instance   # True when xsi:type chose it
ev.nil                  # xsi:nil="true", where the declaration allows it
ev.value, ev.lexical    # typed value and the text it came from
ev.line                 # where in the document
ev.attributes           # AttributeValue, each with its own typed value
```

### Values the schema supplied

An attribute with a `default` or `fixed` value appears in the PSVI even when
the document never wrote it — and says so.

```python
for ev in schemas.iter_typed(xml):
    if ev.kind == "start" and ev.local_name == "item":
        print([(a.local_name, a.value, "from schema" if a.from_schema else "in document")
               for a in ev.attributes])

# [('sku', 'AB-1042', 'in document'), ('quantity', 3, 'in document')]
# [('sku', 'ZZ-0007', 'in document'), ('quantity', 1, 'from schema')]
```

The second item never wrote a `quantity`; the schema's `default="1"` supplied
it. `from_schema` is the flag that lets you tell "the document said 1" from
"the document said nothing" — which matters when you are round-tripping, or
when a default means something different from an explicit value.

## What is handled

- `xsi:type` overrides — the prefix resolved against the namespaces in
  scope, the derivation checked against the declared type, the `block` on
  both, and abstractness.
- `xsi:nil`, with `nillable` and `fixed` enforced, and a nil element held to
  having no content at all — no child elements, and no text, whitespace
  included.
- `xs:QName` and `xs:NOTATION` values, resolved against the namespaces in
  scope — the document's for a value, the schema's for an `xs:enumeration`
  literal, which are not the same bindings and need not agree on a prefix.
- Substitution groups, closed transitively, and `block` on the head — from
  the element declaration or from its type — barring a member from standing
  in for it. An `abstract` element cannot appear at all.
- Wildcards, with `strict`, `lax` and `skip` processing — on elements and
  on attributes. What a `lax` or `strict` wildcard admits is validated
  against its global declaration and reaches the PSVI typed; what a `lax` one
  admits with no declaration is assessed as `xs:anyType`, so its text,
  attributes and children are kept and any globally declared element inside
  it is still checked.
- Mixed content, `xs:all`, and repeated particles.
- Default and fixed values, for attributes and for elements with simple
  content — an empty element takes the value its declaration supplies, and
  `from_schema` on the event says the schema wrote it rather than the
  document. An element with *mixed* content does not take one yet.
- Character and entity references, resolved into the value: `caf&#233;` is
  `café`, not `caf`.
- Encoding detection — hand `validate` the raw `bytes` rather than decoding
  first, and the byte-order mark and XML declaration are read for you.

`xs:ID` uniqueness and `xs:IDREF` resolution are enforced: an ID binds to the
element carrying it, no two elements may claim one, and every reference must
match one somewhere in the document — including one that appears later.

`xs:ENTITY` and `xs:ENTITIES` are checked against the unparsed entities the
document's DTD declares — the one part of a DTD this reader looks at, because
it is the only part those datatypes need.

**Identity constraints** are enforced: `xs:key`, `xs:keyref` and `xs:unique`,
over the restricted XPath subset they take — an optional `.//`, child steps,
and an attribute as a field's last step. Keys compare in the value space, so
`07:00:00Z` and `02:00:00-05:00` are one key. Nodes under a `skip` wildcard,
which are never assessed, are not selected.

Not yet: XSD 1.1 **assertions** and conditional type assignment are stored and
not evaluated. A document that violates one of those is currently reported as
valid.

[Conformance](project/conformance.md) has the measured numbers: 99.0% of the
W3C suite's 21,671 documents are judged correctly when read as XSD 1.1, and
99.8% when read as XSD 1.0.

## Next

- [Diagnostics](diagnostics.md) — the shape of what comes back.
- [XSD 1.1](xsd11.md) — what turning it on changes.
