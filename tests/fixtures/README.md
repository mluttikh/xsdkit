# Test fixtures

- `XMLSchema.xsd` — the W3C schema for schemas, from
  <https://www.w3.org/2001/XMLSchema.xsd>. Used by `real_world.rs`.

  It earns its place as the smallest schema that exercises the awkward parts
  at once: an internal DTD subset, an `xml:lang` attribute with no matching
  import, a `substitutionGroup` hierarchy, `xs:redefine`-free composition,
  and 50 global type declarations that collide by name with the built-ins
  this crate installs.
