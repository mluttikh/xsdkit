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
            println!("{}: {}", schemas.local_of(child.name), child.text());
        }
        Some(())
    }
    ```

Values arrive in their **value space**, not as strings to re-parse: a
`xs:decimal` is a `Decimal`, a `xs:date` is a `datetime.date`, a
`xs:positiveInteger` is an `int`. That falls out of decoding a typed PSVI
rather than a parse tree — the conversion already happened during validation.

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

The shape comes from the schema, not from the document in front of you. This
is the difference knowing the schema makes, and it removes the defect every
schema-less XML-to-dict converter has: with `xmltodict` or `xmlschema`'s
default, a `<report>` with one `<item>` decodes to a dict where a report with
two decodes to a list, so every consumer writes

```python,ignore
items = data["item"]
items = items if isinstance(items, list) else [items]   # not needed here
```

and the ones that forget break the first time a document has exactly one of
something. Here `data["item"]` is a list in all three cases, including when
the document has none — an absent repeating child is `[]`, not a missing key.

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
