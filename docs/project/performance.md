# Performance

## Loading is linear

Building a `SchemaSet` is linear in the size of the schema documents.

| declarations | time |
|---|---|
| 400 | 1.9 ms |
| 800 | 3.9 ms |
| 1,600 | 8.0 ms |
| 3,000 | 15.5 ms |

The schema for schemas — 86 KB, 1,600 lines, and about as gnarly as real
schemas get — compiles in **3.0 ms**. The whole W3C schema suite — 10,511
runs, every dual-version group read as both languages — takes **2.4 s**.

Measured on an Apple M-series laptop with a release build. What matters is the
shape rather than the absolute numbers: doubling the input doubles the time.
`tests/performance.rs` holds it to that shape: four times the input may not cost
more than eight times the time.

## Build once, query many times

Compilation is the expensive half, and it is meant to be. Reference resolution,
attribute group flattening, substitution group closure and automaton
construction all happen once, so that afterwards `children`, `repeats` and
`accepts` are lookups rather than searches.

A `Schemas` is `Send + Sync` and immutable. One compiled schema can serve every
thread in a process, and the Python bindings release the GIL around `build()`,
so loading a large schema does not stall other threads.

The pattern that matters:

```python
import xsdkit

SCHEMAS = xsdkit.SchemaSet.from_file("report.xsd")   # once, at startup

def handle(document):                                 # many times
    return SCHEMAS.validate(document)
```

Rebuilding the schema per document is the one performance mistake that will
dominate everything else.

## Validation

Validation is a single streaming pass over `quick-xml`, driven by the compiled
automata. It does not build a DOM, so memory is proportional to nesting depth
rather than document size, and a large document does not have to fit in memory
twice.

In Rust, typed events cost little beyond validating: the validator already
computed the type and value of everything it checked, so `validate_with`'s
events are work you are being given rather than work being done again. Reading
a document for its values and validating it are the same pass.

From Python each event becomes an object, and that is not free. On a 5.9 MB
document `read_typed(on_event=...)` took 2.0× as long as `validate`, and
`iter_typed` 2.1×. `iter_typed` also builds every event before returning the
first, so it holds all of them at once: 648 MB for a 23.6 MB document, against
16 MB for `validate` or for the `on_event` callback. Use the callback for large
documents.

## Where the remaining time goes

For a large schema, roughly: XML parsing, then component construction, then
compilation. Every compile phase is linear and the whole of compilation is
under a millisecond for schemas of a few thousand declarations — the loader
dominates, and within it, parsing does.

There are no benchmarks in the repository yet. When there are, they will be
wired to CodSpeed in CI; the placeholder is noted in `.github/workflows/ci.yml`
so it does not get forgotten.

## Comparisons

None published. The obvious comparison for the Python side is `xmlschema`,
which is the only complete option in that ecosystem and which reports being
40–75× slower than lxml by its own benchmarks. A fair comparison needs equal
care on both sides — same schemas, same documents, same warm-up — and until
that has been done properly there is no number here worth quoting.
