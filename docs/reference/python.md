# Python API

Everything importable from `xsdkit`. Types come from the shipped stubs, so
`mypy` and `pyright` see exactly what is written here.

!!! tip "Start here"

    [`SchemaSet`](#xsdkit.SchemaSet) is the entry point — build one, then
    subscript it. [`Element`](#xsdkit.Element) and [`Type`](#xsdkit.Type) are
    where you will spend your time.

---

## Loading

::: xsdkit.SchemaSet

::: xsdkit.NamedComponents

::: xsdkit.load

::: xsdkit.load_files

::: xsdkit.load_string

::: xsdkit.load_bytes

::: xsdkit.Document

---

## Declarations

::: xsdkit.Element

::: xsdkit.Child

::: xsdkit.Type

::: xsdkit.Attribute

::: xsdkit.AttributeUse

::: xsdkit.Facets

::: xsdkit.AppInfo

---

## Validation

::: xsdkit.ValidationReport

::: xsdkit.PsviEvents

::: xsdkit.PsviEvent

::: xsdkit.AttributeValue

---

## Diagnostics

::: xsdkit.Diagnostic

::: xsdkit.Span

::: xsdkit.XsdError

::: xsdkit.SchemaError

::: xsdkit.DocumentError

::: xsdkit.InvalidValueError

---

## Rendering and iteration

::: xsdkit.Tree

::: xsdkit.ChildIterator

::: xsdkit.NameIterator

---

## Type aliases

The names the signatures above use, importable for annotating your own code:
`from xsdkit.typing import XsdValue`.

::: xsdkit.typing
    options:
      show_root_heading: false
      members_order: source
