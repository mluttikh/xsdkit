# Decoding documents

Validation answers *is this document allowed?* Decoding answers *what does it
say?* — and hands back data rather than an event stream.

=== "Python"

    ```python
    import xsdkit

    schemas = xsdkit.SchemaSet.from_file("report.xsd")
    data = schemas.decode(open("report.xml").read())

    data["title"]                    # 'November orders'
    data["issued"]                   # datetime.date(2024, 12, 1)
    data["item"][0]["@sku"]          # 'AB-1042'
    data["item"][0]["price"]["$"]    # Decimal('19.95')
    ```

=== "Rust"

    ```rust
    use xsdkit::Schemas;

    fn read(schemas: &Schemas, xml: &str) -> Option<()> {
        let doc = schemas.decode(xml).into_result().ok()?;
        for child in doc.children() {
            // `display_psvi_name` rather than `local_of`: a child a wildcard
            // admitted has no interned name to look up.
            println!("{}: {}", schemas.display_psvi_name(&child.name), child.text());
        }
        Some(())
    }
    ```

A document is XML text, bytes whose encoding is detected, or a `pathlib.Path`
to read it from. A `str` is always content — a path and a document cannot be
told apart once both are strings — so pass a `Path` for a file:

```python
from pathlib import Path

schemas.decode(Path("report.xml"))          # read the file
schemas.decode(Path("report.xml").read_bytes())
schemas.decode("report.xml")                # XsdError: this is not a document
```

Values arrive in their **value space**, not as strings to re-parse: a
`xs:decimal` is a `Decimal`, a `xs:date` is a `datetime.date`, a
`xs:positiveInteger` is an `int`. That falls out of decoding a typed PSVI
rather than a parse tree — the conversion already happened during validation.
A decimal keeps the scale it was written with: `4.50` decodes to
`Decimal('4.50')`, which equals `Decimal('4.5')` as the schema says it must,
and still prints — and multiplies — the way the document wrote it.

## The mapping

| XML | Python |
|---|---|
| element with children | `dict` |
| element with a simple value, no attributes | the value itself |
| element with a value *and* attributes | `dict` with the value under `"$"` |
| attribute | key prefixed with `"@"` |
| `xsi:nil="true"` | `None` |
| character data in mixed content | `"$"` |

## A list is a list

A child the schema allows more than once is **always** a list — holding two
entries, one, or none:

```python
schemas = xsdkit.SchemaSet.from_file("report.xsd")
data = schemas.decode(open("report.xml").read())

len(data["item"])                # 2
type(data["item"]).__name__      # 'list'
```

The shape comes from the schema, not from the document in front of you. That
removes the defect every schema-less XML-to-dict converter has: with
`xmltodict`, a `<report>` with one `<item>` decodes to a dict where a report
with two decodes to a list, so every consumer writes

```python,ignore
items = data["item"]
items = items if isinstance(items, list) else [items]   # not needed here
```

and the ones that forget break the first time a document has exactly one of
something. Here `data["item"]` is a list in all three cases.

`xmlschema` is schema-aware too, and its default converter gets the one-item
case right as well. Where the two differ is the empty case: an absent
repeating child is `[]` here, where `xmlschema` leaves the key out, so
`data["item"]` needs no `.get("item", [])` even when the document has none.

Keys keep document order, the way a reader of the document expects to see
them. A repeating child the document did not carry has no position in it, so
its empty list comes after the keys that were present.

A child that may appear **at most once** and does not appear has no key at
all, which is how `note` behaves above:

```python
[k for k in schemas.decode(open("report.xml").read())["item"][1]]
# ['@sku', '@quantity', 'price'] — no 'note', it was not there
```

## Names

Keys are local names. Clark notation appears only where two names under one
parent would otherwise collide — and whether they collide is decided by the
**schema**, so a key does not change shape because a particular document left
a sibling out.

Namespaces are the one thing the dictionary form gives up. Where that matters,
the Rust [`Decoded`](reference/rust.md) tree keeps every qualified name, the
type in force after any `xsi:type`, and which values the schema supplied
rather than the document.

## Invalid documents

`decode` raises, where `validate` does not:

```python
schemas = xsdkit.SchemaSet.from_file("report.xsd")
bad = '<report xmlns="urn:example" id="r1"><title>t</title></report>'

schemas.decode(bad)          # XsdError
```

The asymmetry is deliberate. `validate` is *asked* whether a document is
valid, so an invalid one is an answer. `decode` is asked for data, and handing
back data from a document that does not fit its schema is exactly the mistake
this is meant to remove.

Take it anyway when you mean to:

```python
data = schemas.decode(bad, lax=True)
data["title"]                # 't' — what there was of it
```

## Mixed content

Character data in a mixed type decodes under `"$"`, alongside the children.
The runs are concatenated: what was said, not where it sat between the
children. For the data-oriented schemas this library is built for that is a
non-question; for marked-up prose, decode is the wrong tool and the PSVI
event stream is the right one.
