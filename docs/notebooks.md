# In a notebook

Every object worth looking at renders itself as HTML in Jupyter, JupyterLab,
VS Code notebooks, Marimo and anything else that speaks the IPython display
protocol. There is nothing to import and nothing to enable.

```python
import xsdkit

schemas = xsdkit.SchemaSet.from_file("report.xsd")
schemas                     # renders as a table, not as <SchemaSet object at 0x…>
```

| Put this in a cell | And you get |
|---|---|
| `schemas` | Its documents and globals at a glance |
| `element` | The element's shape, a couple of levels deep |
| `element.tree()` | The full tree, colour-coded, as deep as you asked |
| `type` | Name, variety or content kind, base |
| `type.facets` | The constraints in force, as a table |
| `schemas.validate(xml)` | A summary line and a table of what was found |
| `diagnostic` | The message, coloured by severity, with its span and help |

## `tree()` returns something you can look at

```python
import xsdkit

schemas = xsdkit.SchemaSet.from_file("report.xsd")
report = schemas["{urn:example}report"]

report.tree()
```

A plain `str` would have been the wrong type here, and the reason is worth
knowing because it bites everyone once. A notebook displays `repr()` of the
last expression, and `repr` of a string escapes every newline — so a method
that returns rendered text as a `str` shows you

```text
'report: {urn:example}Report\n  @id\n  title: xs:string\n  issued: xs:date\n…'
```

which is exactly the thing you were trying to read, made unreadable.

`tree()` returns a `Tree`: it renders as itself in a REPL, as HTML in a
notebook, and as plain text through `print`, while still behaving as the text
it is — `len`, `in`, `==`, `splitlines()` and `count()` all work.

```python
t = report.tree()
"price" in t             # True
len(t.splitlines())      # 10
print(t)                 # plain text, for a terminal or a log
```

Pass a depth when a schema is deep and you only want the top:

```python
report.tree(depth=2)
```

Recursion stops where the shape starts repeating, so a self-referential schema
prints instead of hanging.

## The renderings follow your theme

Colours come from JupyterLab's own CSS variables — `--jp-content-font-color1`,
`--jp-code-font-family`, `--jp-mirror-editor-def-color` and friends — with
literal fallbacks for hosts that do not define them. Switch JupyterLab to dark
mode and the trees, tables and diagnostics follow, because they are not
carrying a hardcoded palette that assumes a white page.

## A session that reads well

```python
import xsdkit

schemas = xsdkit.SchemaSet.from_file("report.xsd")
schemas
```

```python
report = schemas["{urn:example}report"]
report.tree()
```

```python
report["item"]["price"].type.facets
```

```python
schemas.validate(open("report.xml").read())
```

Each cell displays something readable, so exploring a schema you have never
seen is a matter of subscripting and looking, rather than printing dictionaries
of ids.

## Outside a notebook

The same objects print sensibly in a terminal. `str()` and `print()` give the
plain-text form of everything above, and diagnostics print in the
compiler-style block shown in [Diagnostics](diagnostics.md).

There is also an inspector for the command line:

```bash
cargo run --example inspect -- schemas/report.xsd --lax
```
