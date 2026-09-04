//! Python bindings.
//!
//! Every Python handle is a pair: an `Arc<Schemas>` and a `Copy` id. Handing
//! out ten thousand element wrappers costs ten thousand refcount bumps and
//! copies nothing, and no component leaves the model until Python asks for a
//! specific field. This is where the arena design pays off a second time —
//! and because `Schemas` is `Send + Sync`, the GIL is released around
//! compilation, which is the only slow part.

#![allow(clippy::needless_pass_by_value)]
// Every wrapper holds an `Arc<Schemas>`, so a derived `Debug` would print the
// entire schema once per handle. `__repr__` is the useful rendering, and it is
// defined on each type.
#![allow(missing_debug_implementations)]

use crate::content::ContentModel;
use crate::datatypes::Variety;
use crate::diagnostics::{Diagnostic, Diagnostics, Severity, Span};
use crate::instance::PsviEvent as RustPsvi;
use crate::model::*;
use crate::names::QName;
use crate::values::Value;
use crate::{Conformance, FileResolver, SchemaSetBuilder};
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyList, PyTuple, PyType};
use pyo3::{IntoPyObjectExt, create_exception};
use std::sync::Arc;

create_exception!(xsdkit, XsdError, pyo3::exceptions::PyException);
create_exception!(
    xsdkit,
    SchemaError,
    XsdError,
    "Raised when a schema cannot be built. Carries every diagnostic on `.diagnostics`."
);

/// Builds a `SchemaError` carrying the whole diagnostic list.
///
/// Every diagnostic, not the first: someone fixing a 40-file import graph
/// needs the list.
fn schema_error(py: Python<'_>, diags: Diagnostics) -> PyErr {
    let n = diags.errors().count();
    let err = SchemaError::new_err(format!("{n} error(s) building the schema:\n{diags}"));
    let wrapped: Vec<PyDiagnostic> = diags.into_iter().map(PyDiagnostic).collect();
    if let Ok(list) = wrapped.into_py_any(py) {
        let _ = err.value(py).setattr("diagnostics", list);
    }
    err
}

fn conformance_from(s: &str) -> PyResult<Conformance> {
    match s {
        "strict" => Ok(Conformance::Strict),
        "lax" => Ok(Conformance::Lax),
        other => Err(PyValueError::new_err(format!(
            "conformance must be 'strict' or 'lax', got {other:?}"
        ))),
    }
}

/// Parses a name written either in Clark notation (`{ns}local`), as a bare
/// local name, or as a `(namespace, local)` pair.
fn parse_name(schemas: &Schemas, obj: &Bound<'_, PyAny>) -> PyResult<Option<QName>> {
    if let Ok(t) = obj.cast::<PyTuple>() {
        if t.len() != 2 {
            return Err(PyValueError::new_err(
                "a name tuple must be (namespace, local)",
            ));
        }
        let ns: Option<String> = t.get_item(0)?.extract()?;
        let local: String = t.get_item(1)?.extract()?;
        return Ok(schemas.qname(ns.as_deref(), &local));
    }
    let s: String = obj.extract().map_err(|_| {
        PyValueError::new_err("expected '{ns}local', 'local', or (namespace, local)")
    })?;
    Ok(match s.strip_prefix('{') {
        Some(rest) => match rest.split_once('}') {
            Some((ns, local)) => schemas.qname(Some(ns), local),
            None => return Err(PyValueError::new_err("unterminated '{' in Clark notation")),
        },
        None => schemas.qname(None, &s),
    })
}

fn clark(schemas: &Schemas, q: QName) -> String {
    schemas.display_name(q)
}

// ---------------------------------------------------------------------------
// SchemaSet
// ---------------------------------------------------------------------------

/// A compiled set of schema components.
#[pyclass(module = "xsdkit", name = "SchemaSet", frozen, skip_from_py_object)]
pub struct PySchemaSet {
    inner: Arc<Schemas>,
}

impl PySchemaSet {
    fn wrap(schemas: Schemas) -> Self {
        Self {
            inner: Arc::new(schemas),
        }
    }
}

/// Assembles a builder from the keyword arguments the constructors share.
fn builder(
    search_paths: Option<Vec<String>>,
    conformance: &str,
    nodes_limit: Option<u32>,
) -> PyResult<SchemaSetBuilder> {
    let mut b = SchemaSetBuilder::new().conformance(conformance_from(conformance)?);
    if let Some(paths) = search_paths {
        let mut fr = FileResolver::new();
        fr.search_paths = paths.into_iter().map(Into::into).collect();
        b = b.resolver(fr);
    }
    if let Some(limit) = nodes_limit {
        b = b.nodes_limit(limit);
    }
    Ok(b)
}

#[pymethods]
impl PySchemaSet {
    /// Loads a schema from a file, following its includes and imports.
    #[classmethod]
    #[pyo3(signature = (path, *, search_paths=None, conformance="strict", nodes_limit=None))]
    fn from_file(
        _cls: &Bound<'_, PyType>,
        py: Python<'_>,
        path: String,
        search_paths: Option<Vec<String>>,
        conformance: &str,
        nodes_limit: Option<u32>,
    ) -> PyResult<Self> {
        let b = builder(search_paths, conformance, nodes_limit)?.file(path);
        // Compilation is the only slow part, and `Schemas` is Send + Sync
        // precisely so this is legal.
        let (schemas, diags) = py.detach(|| b.build_with_warnings());
        if diags.has_errors() {
            return Err(schema_error(py, diags));
        }
        Ok(Self::wrap(schemas))
    }

    /// Loads a schema from a string. The text must already be decoded.
    #[classmethod]
    #[pyo3(signature = (xsd, *, uri="<string>", search_paths=None, conformance="strict", nodes_limit=None))]
    fn from_string(
        _cls: &Bound<'_, PyType>,
        py: Python<'_>,
        xsd: String,
        uri: &str,
        search_paths: Option<Vec<String>>,
        conformance: &str,
        nodes_limit: Option<u32>,
    ) -> PyResult<Self> {
        let b = builder(search_paths, conformance, nodes_limit)?.text(xsd, uri);
        let (schemas, diags) = py.detach(|| b.build_with_warnings());
        if diags.has_errors() {
            return Err(schema_error(py, diags));
        }
        Ok(Self::wrap(schemas))
    }

    /// Loads a schema from raw bytes, detecting the encoding.
    ///
    /// Prefer this over `from_string` when the encoding is not known to be
    /// UTF-8: a byte-order mark or the XML declaration decides it.
    #[classmethod]
    #[pyo3(signature = (data, *, uri="<bytes>", search_paths=None, conformance="strict", nodes_limit=None))]
    fn from_bytes(
        _cls: &Bound<'_, PyType>,
        py: Python<'_>,
        data: Vec<u8>,
        uri: &str,
        search_paths: Option<Vec<String>>,
        conformance: &str,
        nodes_limit: Option<u32>,
    ) -> PyResult<Self> {
        let b = builder(search_paths, conformance, nodes_limit)?.bytes(data, uri);
        let (schemas, diags) = py.detach(|| b.build_with_warnings());
        if diags.has_errors() {
            return Err(schema_error(py, diags));
        }
        Ok(Self::wrap(schemas))
    }

    /// The documents this schema set was built from.
    #[getter]
    fn documents(&self) -> Vec<PyDocument> {
        self.inner
            .documents()
            .iter()
            .map(|d| PyDocument {
                uri: d.uri.clone(),
                target_namespace: d
                    .target_namespace
                    .map(|n| self.inner.names().resolve_ns(n).to_string()),
                chameleon: d.chameleon,
            })
            .collect()
    }

    /// Every global element declaration, keyed by Clark-notation name.
    #[getter]
    fn elements(&self) -> Vec<(String, PyElement)> {
        let mut v: Vec<_> = self
            .inner
            .globals()
            .elements
            .iter()
            .map(|(q, id)| {
                (
                    clark(&self.inner, *q),
                    PyElement {
                        s: self.inner.clone(),
                        id: *id,
                    },
                )
            })
            .collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    }

    /// Every global type definition, keyed by Clark-notation name.
    #[getter]
    fn types(&self) -> Vec<(String, PyType_)> {
        let mut v: Vec<_> = self
            .inner
            .globals()
            .types
            .iter()
            .map(|(q, id)| {
                (
                    clark(&self.inner, *q),
                    PyType_ {
                        s: self.inner.clone(),
                        id: *id,
                    },
                )
            })
            .collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    }

    /// Looks up a global element. `None` if there is none.
    #[pyo3(signature = (namespace, local=None))]
    fn element(
        &self,
        namespace: &Bound<'_, PyAny>,
        local: Option<&str>,
    ) -> PyResult<Option<PyElement>> {
        let q = match local {
            Some(l) => {
                let ns: Option<String> = namespace.extract()?;
                self.inner.qname(ns.as_deref(), l)
            }
            None => parse_name(&self.inner, namespace)?,
        };
        Ok(
            q.and_then(|q| self.inner.globals().elements.get(&q).copied())
                .map(|id| PyElement {
                    s: self.inner.clone(),
                    id,
                }),
        )
    }

    /// Looks up a global type. `None` if there is none.
    #[pyo3(name = "type", signature = (namespace, local=None))]
    fn type_(
        &self,
        namespace: &Bound<'_, PyAny>,
        local: Option<&str>,
    ) -> PyResult<Option<PyType_>> {
        let q = match local {
            Some(l) => {
                let ns: Option<String> = namespace.extract()?;
                self.inner.qname(ns.as_deref(), l)
            }
            None => parse_name(&self.inner, namespace)?,
        };
        Ok(q.and_then(|q| self.inner.globals().types.get(&q).copied())
            .map(|id| PyType_ {
                s: self.inner.clone(),
                id,
            }))
    }

    /// Looks up a global attribute. `None` if there is none.
    #[pyo3(signature = (namespace, local=None))]
    fn attribute(
        &self,
        namespace: &Bound<'_, PyAny>,
        local: Option<&str>,
    ) -> PyResult<Option<PyAttribute>> {
        let q = match local {
            Some(l) => {
                let ns: Option<String> = namespace.extract()?;
                self.inner.qname(ns.as_deref(), l)
            }
            None => parse_name(&self.inner, namespace)?,
        };
        Ok(
            q.and_then(|q| self.inner.globals().attributes.get(&q).copied())
                .map(|id| PyAttribute {
                    s: self.inner.clone(),
                    id,
                }),
        )
    }

    /// Validates an instance document against this schema.
    ///
    /// Never raises for an invalid document — an invalid document is an
    /// answer, not an error. Inspect `.is_valid` and `.diagnostics`.
    #[pyo3(signature = (xml, *, uri="<instance>"))]
    fn validate(&self, py: Python<'_>, xml: &str, uri: &str) -> PyResult<PyValidationReport> {
        let schemas = self.inner.clone();
        // No Python is called back into, so the GIL can go.
        let report = py.detach(|| {
            schemas
                .instance_validator()
                .validate_named(xml, uri, |_| {})
        });
        let valid = report.is_valid();
        Ok(PyValidationReport {
            valid,
            diagnostics: report.diagnostics.into_iter().map(PyDiagnostic).collect(),
        })
    }

    /// Reads a document into typed PSVI events.
    ///
    /// Returns the events as a list, or feeds them to `on_event` and returns
    /// `None`. Use the callback for documents large enough that holding every
    /// event defeats the point of streaming.
    ///
    /// Validation still runs; `report` carries the diagnostics either way.
    #[pyo3(signature = (xml, *, on_event=None, uri="<instance>"))]
    fn read_typed(
        &self,
        py: Python<'_>,
        xml: &str,
        on_event: Option<Bound<'_, PyAny>>,
        uri: &str,
    ) -> PyResult<(Option<Vec<Py<PyPsviEvent>>>, PyValidationReport)> {
        let mut collected: Vec<Py<PyPsviEvent>> = Vec::new();
        let mut callback_error: Option<PyErr> = None;

        // The GIL is held throughout: every event becomes a Python object,
        // and `on_event` is Python code.
        let report = self
            .inner
            .instance_validator()
            .validate_named(xml, uri, |ev| {
                if callback_error.is_some() {
                    return;
                }
                match self.psvi_to_py(py, ev) {
                    Ok(obj) => match &on_event {
                        Some(f) => {
                            if let Err(e) = f.call1((obj,)) {
                                callback_error = Some(e);
                            }
                        }
                        None => collected.push(obj.unbind()),
                    },
                    Err(e) => callback_error = Some(e),
                }
            });
        if let Some(e) = callback_error {
            return Err(e);
        }

        let valid = report.is_valid();
        let py_report = PyValidationReport {
            valid,
            diagnostics: report.diagnostics.into_iter().map(PyDiagnostic).collect(),
        };
        Ok((on_event.is_none().then_some(collected), py_report))
    }

    /// Component counts, for diagnostics and smoke tests.
    #[getter]
    fn counts(&self) -> Vec<(String, usize)> {
        let c = self.inner.component_counts();
        vec![
            ("types".into(), c.types),
            ("elements".into(), c.elements),
            ("attributes".into(), c.attributes),
            ("particles".into(), c.particles),
            ("model_groups".into(), c.model_groups),
            ("attribute_groups".into(), c.attribute_groups),
            ("identity_constraints".into(), c.identity_constraints),
            ("annotations".into(), c.annotations),
        ]
    }

    fn __repr__(&self) -> String {
        let c = self.inner.component_counts();
        format!(
            "<SchemaSet {} document(s), {} types, {} elements>",
            self.inner.documents().len(),
            c.types,
            c.elements
        )
    }
}

impl PySchemaSet {
    /// Turns one PSVI event into its Python wrapper.
    fn psvi_to_py<'py>(&self, py: Python<'py>, ev: RustPsvi) -> PyResult<Bound<'py, PyPsviEvent>> {
        let name_of = |q: crate::names::QName| {
            (
                q.ns.map(|n| self.inner.names().resolve_ns(n).to_string()),
                self.inner.names().resolve(q.local).to_string(),
            )
        };
        let wrapped = match ev {
            RustPsvi::StartElement {
                name,
                declaration,
                type_id,
                type_from_instance,
                nil,
                attributes,
                line,
            } => {
                let mut attrs = Vec::with_capacity(attributes.len());
                for a in attributes {
                    let value = match &a.value {
                        Some(v) => Some(value_to_py(py, v)?.unbind()),
                        None => None,
                    };
                    attrs.push(PyAttributeValue {
                        name: name_of(a.name),
                        declaration: a.declaration.map(|id| PyAttribute {
                            s: self.inner.clone(),
                            id,
                        }),
                        value,
                        lexical: a.lexical,
                        from_schema: a.from_schema,
                    });
                }
                PyPsviEvent {
                    kind: "start",
                    name: name_of(name),
                    declaration: declaration.map(|id| PyElement {
                        s: self.inner.clone(),
                        id,
                    }),
                    type_: Some(PyType_ {
                        s: self.inner.clone(),
                        id: type_id,
                    }),
                    type_from_instance,
                    nil,
                    attributes: attrs,
                    value: None,
                    lexical: None,
                    line,
                }
            }
            RustPsvi::Text {
                value,
                type_id,
                lexical,
                line,
            } => PyPsviEvent {
                kind: "text",
                name: (None, String::new()),
                declaration: None,
                type_: Some(PyType_ {
                    s: self.inner.clone(),
                    id: type_id,
                }),
                type_from_instance: false,
                nil: false,
                attributes: Vec::new(),
                value: match &value {
                    Some(v) => Some(value_to_py(py, v)?.unbind()),
                    None => None,
                },
                lexical: Some(lexical),
                line,
            },
            RustPsvi::EndElement {
                name,
                declaration,
                line,
            } => PyPsviEvent {
                kind: "end",
                name: name_of(name),
                declaration: declaration.map(|id| PyElement {
                    s: self.inner.clone(),
                    id,
                }),
                type_: None,
                type_from_instance: false,
                nil: false,
                attributes: Vec::new(),
                value: None,
                lexical: None,
                line,
            },
        };
        Bound::new(py, wrapped)
    }
}

// ---------------------------------------------------------------------------
// Declarations
// ---------------------------------------------------------------------------

#[pyclass(module = "xsdkit", name = "Element", frozen, from_py_object)]
#[derive(Clone)]
pub struct PyElement {
    s: Arc<Schemas>,
    id: ElementId,
}

#[pymethods]
impl PyElement {
    /// `(namespace, local)`; the namespace is `None` when unqualified.
    #[getter]
    fn name(&self) -> (Option<String>, String) {
        let q = self.s[self.id].name;
        (
            q.ns.map(|n| self.s.names().resolve_ns(n).to_string()),
            self.s.names().resolve(q.local).to_string(),
        )
    }

    /// The name in Clark notation, `{ns}local`.
    #[getter]
    fn qname(&self) -> String {
        clark(&self.s, self.s[self.id].name)
    }

    #[getter]
    fn local_name(&self) -> String {
        self.s
            .names()
            .resolve(self.s[self.id].name.local)
            .to_string()
    }

    #[getter]
    fn namespace(&self) -> Option<String> {
        self.s[self.id]
            .name
            .ns
            .map(|n| self.s.names().resolve_ns(n).to_string())
    }

    #[getter]
    fn r#type(&self) -> PyType_ {
        PyType_ {
            s: self.s.clone(),
            id: self.s[self.id].type_id,
        }
    }

    #[getter]
    fn nillable(&self) -> bool {
        self.s[self.id].nillable
    }

    #[getter]
    fn r#abstract(&self) -> bool {
        self.s[self.id].is_abstract
    }

    /// Whether this is a global declaration rather than one scoped to a type.
    #[getter]
    fn is_global(&self) -> bool {
        matches!(self.s[self.id].scope, Scope::Global)
    }

    /// Every element that may appear where this one is permitted, including
    /// itself when it is not abstract. Substitution is transitive.
    #[getter]
    fn substitutes(&self) -> Vec<PyElement> {
        self.s
            .substitution_closure(self.id)
            .into_iter()
            .map(|e| PyElement {
                s: self.s.clone(),
                id: e,
            })
            .collect()
    }

    #[getter]
    fn default(&self) -> Option<String> {
        match &self.s[self.id].value_constraint {
            Some(ValueConstraint::Default(v)) => Some(v.clone()),
            _ => None,
        }
    }

    #[getter]
    fn fixed(&self) -> Option<String> {
        match &self.s[self.id].value_constraint {
            Some(ValueConstraint::Fixed(v)) => Some(v.clone()),
            _ => None,
        }
    }

    /// The `xs:documentation` text, entries joined.
    #[getter]
    fn doc(&self) -> Option<String> {
        annotation_doc(&self.s, self.s[self.id].annotation)
    }

    /// The `xs:appinfo` blocks, with their XML kept verbatim.
    #[getter]
    fn appinfo(&self) -> Vec<PyAppInfo> {
        annotation_appinfo(&self.s, self.s[self.id].annotation)
    }

    fn __repr__(&self) -> String {
        format!("<Element {}>", self.qname())
    }

    fn __eq__(&self, other: &PyElement) -> bool {
        self.id == other.id && Arc::ptr_eq(&self.s, &other.s)
    }

    fn __hash__(&self) -> u64 {
        self.id.index() as u64
    }
}

#[pyclass(module = "xsdkit", name = "Attribute", frozen, skip_from_py_object)]
#[derive(Clone)]
pub struct PyAttribute {
    s: Arc<Schemas>,
    id: AttributeId,
}

#[pymethods]
impl PyAttribute {
    #[getter]
    fn name(&self) -> (Option<String>, String) {
        let q = self.s[self.id].name;
        (
            q.ns.map(|n| self.s.names().resolve_ns(n).to_string()),
            self.s.names().resolve(q.local).to_string(),
        )
    }

    #[getter]
    fn qname(&self) -> String {
        clark(&self.s, self.s[self.id].name)
    }

    #[getter]
    fn local_name(&self) -> String {
        self.s
            .names()
            .resolve(self.s[self.id].name.local)
            .to_string()
    }

    #[getter]
    fn r#type(&self) -> PyType_ {
        PyType_ {
            s: self.s.clone(),
            id: self.s[self.id].type_id,
        }
    }

    #[getter]
    fn default(&self) -> Option<String> {
        match &self.s[self.id].value_constraint {
            Some(ValueConstraint::Default(v)) => Some(v.clone()),
            _ => None,
        }
    }

    /// A schema-declared constant value — the case a units layer can resolve
    /// without seeing an instance document.
    #[getter]
    fn fixed(&self) -> Option<String> {
        match &self.s[self.id].value_constraint {
            Some(ValueConstraint::Fixed(v)) => Some(v.clone()),
            _ => None,
        }
    }

    #[getter]
    fn doc(&self) -> Option<String> {
        annotation_doc(&self.s, self.s[self.id].annotation)
    }

    #[getter]
    fn appinfo(&self) -> Vec<PyAppInfo> {
        annotation_appinfo(&self.s, self.s[self.id].annotation)
    }

    fn __repr__(&self) -> String {
        format!("<Attribute {}>", self.qname())
    }
}

/// An attribute declaration as used by one complex type.
#[pyclass(module = "xsdkit", name = "AttributeUse", frozen, skip_from_py_object)]
#[derive(Clone)]
pub struct PyAttributeUse {
    s: Arc<Schemas>,
    attribute: AttributeId,
    kind: AttributeUseKind,
    constraint: Option<ValueConstraint>,
}

#[pymethods]
impl PyAttributeUse {
    #[getter]
    fn attribute(&self) -> PyAttribute {
        PyAttribute {
            s: self.s.clone(),
            id: self.attribute,
        }
    }

    #[getter]
    fn name(&self) -> (Option<String>, String) {
        self.attribute().name()
    }

    #[getter]
    fn local_name(&self) -> String {
        self.attribute().local_name()
    }

    #[getter]
    fn r#type(&self) -> PyType_ {
        self.attribute().r#type()
    }

    #[getter]
    fn required(&self) -> bool {
        self.kind == AttributeUseKind::Required
    }

    /// `"required"`, `"optional"` or `"prohibited"`.
    #[getter]
    fn r#use(&self) -> &'static str {
        match self.kind {
            AttributeUseKind::Required => "required",
            AttributeUseKind::Optional => "optional",
            AttributeUseKind::Prohibited => "prohibited",
        }
    }

    /// The use's own fixed value, falling back to the declaration's.
    #[getter]
    fn fixed(&self) -> Option<String> {
        match &self.constraint {
            Some(ValueConstraint::Fixed(v)) => Some(v.clone()),
            Some(ValueConstraint::Default(_)) => None,
            None => self.attribute().fixed(),
        }
    }

    #[getter]
    fn default(&self) -> Option<String> {
        match &self.constraint {
            Some(ValueConstraint::Default(v)) => Some(v.clone()),
            Some(ValueConstraint::Fixed(_)) => None,
            None => self.attribute().default(),
        }
    }

    fn __repr__(&self) -> String {
        format!("<AttributeUse {} {}>", self.local_name(), self.r#use())
    }
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[pyclass(module = "xsdkit", name = "Type", frozen, from_py_object)]
#[derive(Clone)]
pub struct PyType_ {
    s: Arc<Schemas>,
    id: TypeId,
}

#[pymethods]
impl PyType_ {
    /// `(namespace, local)`, or `None` for an anonymous inline type.
    #[getter]
    fn name(&self) -> Option<(Option<String>, String)> {
        self.s[self.id].name().map(|q| {
            (
                q.ns.map(|n| self.s.names().resolve_ns(n).to_string()),
                self.s.names().resolve(q.local).to_string(),
            )
        })
    }

    #[getter]
    fn qname(&self) -> Option<String> {
        self.s[self.id].name().map(|q| clark(&self.s, q))
    }

    #[getter]
    fn is_complex(&self) -> bool {
        self.s[self.id].as_complex().is_some()
    }

    #[getter]
    fn is_simple(&self) -> bool {
        self.s[self.id].is_simple()
    }

    #[getter]
    fn r#abstract(&self) -> bool {
        self.s[self.id].as_complex().is_some_and(|c| c.is_abstract)
    }

    /// The type this one derives from, or `None` at `xs:anyType`.
    #[getter]
    fn base(&self) -> Option<PyType_> {
        let base = self.s[self.id].base();
        (base != self.id).then(|| PyType_ {
            s: self.s.clone(),
            id: base,
        })
    }

    /// `"extension"` or `"restriction"`; `None` for simple types.
    #[getter]
    fn derivation(&self) -> Option<&'static str> {
        self.s[self.id].as_complex().map(|c| match c.derivation {
            DerivationMethod::Extension => "extension",
            DerivationMethod::Restriction => "restriction",
        })
    }

    /// Whether this type is, or derives from, `other`.
    fn derives_from(&self, other: &PyType_) -> bool {
        self.s.derives_from(self.id, other.id)
    }

    /// The base chain, from this type up to `xs:anyType`.
    #[getter]
    fn base_chain(&self) -> Vec<PyType_> {
        self.s
            .base_chain(self.id)
            .into_iter()
            .map(|id| PyType_ {
                s: self.s.clone(),
                id,
            })
            .collect()
    }

    /// Attribute uses, with inherited attribute groups already flattened in.
    #[getter]
    fn attributes(&self) -> Vec<PyAttributeUse> {
        self.s
            .attribute_uses(self.id)
            .iter()
            .map(|u| PyAttributeUse {
                s: self.s.clone(),
                attribute: u.attribute,
                kind: u.kind,
                constraint: u.value_constraint.clone(),
            })
            .collect()
    }

    /// Every element that may appear directly inside this type, with
    /// substitution groups expanded and inherited content included.
    #[getter]
    fn children(&self) -> Vec<PyElement> {
        self.s
            .possible_children(self.id)
            .into_iter()
            .map(|id| PyElement {
                s: self.s.clone(),
                id,
            })
            .collect()
    }

    /// Whether `child` may appear more than once — the table-versus-column
    /// question.
    fn repeats(&self, child: &PyElement) -> bool {
        self.s.child_repeats(self.id, child.id)
    }

    /// Whether `child` may be absent, making a derived column nullable.
    fn optional(&self, child: &PyElement) -> bool {
        self.s.child_is_optional(self.id, child.id)
    }

    /// `"empty"`, `"simple"`, `"element-only"` or `"mixed"`; `None` for a
    /// simple type.
    #[getter]
    fn content(&self) -> Option<&'static str> {
        self.s[self.id].as_complex().map(|c| match c.content {
            ContentType::Empty => "empty",
            ContentType::Simple(_) => "simple",
            ContentType::ElementOnly(_) => "element-only",
            ContentType::Mixed(_) => "mixed",
        })
    }

    /// How the content model was compiled: `"empty"`, `"automaton"` or
    /// `"all"`.
    #[getter]
    fn content_model(&self) -> Option<&'static str> {
        self.s.content_model(self.id).map(|m| match m {
            ContentModel::Empty => "empty",
            ContentModel::Automaton(_) => "automaton",
            ContentModel::All(_) => "all",
        })
    }

    /// Whether a sequence of child names satisfies this type's content model.
    ///
    /// Names may be Clark notation, bare local names, or `(ns, local)` pairs.
    fn accepts(&self, names: &Bound<'_, PyAny>) -> PyResult<bool> {
        let Some(mut m) = self.s.match_content(self.id) else {
            return Ok(false);
        };
        for item in names.try_iter()? {
            let item = item?;
            let Some(q) = parse_name(&self.s, &item)? else {
                // A name no component in this schema carries cannot match.
                return Ok(false);
            };
            if !m.step(q) {
                return Ok(false);
            }
        }
        Ok(m.accepts_end())
    }

    /// Validates a lexical form against this type, returning its typed value.
    ///
    /// Raises `ValueError` with the reason when the value is not valid.
    fn validate(&self, py: Python<'_>, lexical: &str) -> PyResult<Py<PyAny>> {
        let validator = self.s.validator();
        match validator.validate(self.id, lexical) {
            Ok(v) => Ok(value_to_py(py, &v)?.unbind()),
            Err(e) => Err(PyValueError::new_err(e.to_string())),
        }
    }

    /// Whether a lexical form is valid against this type.
    fn is_valid(&self, lexical: &str) -> bool {
        self.s.validator().validate(self.id, lexical).is_ok()
    }

    // -- simple types ------------------------------------------------------

    /// `"atomic"`, `"list"` or `"union"`; `None` for a complex type.
    #[getter]
    fn variety(&self) -> Option<&'static str> {
        self.s[self.id].as_simple().map(|t| match t.variety {
            Variety::Atomic => "atomic",
            Variety::List => "list",
            Variety::Union => "union",
        })
    }

    /// The primitive this simple type reduces to, e.g. `"string"`.
    #[getter]
    fn primitive(&self) -> Option<String> {
        self.s[self.id]
            .as_simple()
            .and_then(|t| t.primitive)
            .map(|b| b.local_name().to_string())
    }

    /// The built-in this type *is*, if it is one.
    #[getter]
    fn builtin(&self) -> Option<String> {
        self.s
            .as_builtin(self.id)
            .map(|b| b.local_name().to_string())
    }

    /// A list type's item type.
    #[getter]
    fn item_type(&self) -> Option<PyType_> {
        self.s[self.id]
            .as_simple()
            .and_then(|t| t.item_type)
            .map(|id| PyType_ {
                s: self.s.clone(),
                id,
            })
    }

    /// A union's member types, in the order they are tried.
    #[getter]
    fn member_types(&self) -> Vec<PyType_> {
        self.s[self.id]
            .as_simple()
            .map(|t| {
                t.member_types
                    .iter()
                    .map(|id| PyType_ {
                        s: self.s.clone(),
                        id: *id,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    #[getter]
    fn facets(&self) -> Option<PyFacets> {
        self.s[self.id]
            .as_simple()
            .map(|t| PyFacets(t.facets.clone()))
    }

    #[getter]
    fn doc(&self) -> Option<String> {
        let ann = match &self.s[self.id] {
            TypeDefinition::Simple(t) => t.annotation,
            TypeDefinition::Complex(t) => t.annotation,
        };
        annotation_doc(&self.s, ann)
    }

    #[getter]
    fn appinfo(&self) -> Vec<PyAppInfo> {
        let ann = match &self.s[self.id] {
            TypeDefinition::Simple(t) => t.annotation,
            TypeDefinition::Complex(t) => t.annotation,
        };
        annotation_appinfo(&self.s, ann)
    }

    fn __repr__(&self) -> String {
        let kind = if self.is_complex() {
            "complex"
        } else {
            "simple"
        };
        match self.qname() {
            Some(n) => format!("<Type {kind} {n}>"),
            None => format!("<Type {kind} (anonymous)>"),
        }
    }

    fn __eq__(&self, other: &PyType_) -> bool {
        self.id == other.id && Arc::ptr_eq(&self.s, &other.s)
    }

    fn __hash__(&self) -> u64 {
        self.id.index() as u64
    }
}

/// The facets in force on a simple type, after composing its whole
/// restriction chain.
#[pyclass(module = "xsdkit", name = "Facets", frozen, skip_from_py_object)]
#[derive(Clone)]
pub struct PyFacets(crate::datatypes::FacetSet);

#[pymethods]
impl PyFacets {
    #[getter]
    fn length(&self) -> Option<u64> {
        self.0.length
    }
    #[getter]
    fn min_length(&self) -> Option<u64> {
        self.0.min_length
    }
    #[getter]
    fn max_length(&self) -> Option<u64> {
        self.0.max_length
    }

    /// Patterns as declared: the outer list is one entry per restriction
    /// step, **ANDed**; the inner alternatives at that step are **ORed**.
    #[getter]
    fn patterns(&self) -> Vec<Vec<String>> {
        self.0.patterns.clone()
    }

    #[getter]
    fn enumeration(&self) -> Option<Vec<String>> {
        self.0.enumeration.clone()
    }

    /// `"preserve"`, `"replace"` or `"collapse"` when stated explicitly.
    #[getter]
    fn white_space(&self) -> Option<String> {
        self.0.white_space.map(|w| w.to_string())
    }

    #[getter]
    fn max_inclusive(&self) -> Option<String> {
        self.0.max_inclusive.clone()
    }
    #[getter]
    fn max_exclusive(&self) -> Option<String> {
        self.0.max_exclusive.clone()
    }
    #[getter]
    fn min_inclusive(&self) -> Option<String> {
        self.0.min_inclusive.clone()
    }
    #[getter]
    fn min_exclusive(&self) -> Option<String> {
        self.0.min_exclusive.clone()
    }
    #[getter]
    fn total_digits(&self) -> Option<u32> {
        self.0.total_digits
    }
    #[getter]
    fn fraction_digits(&self) -> Option<u32> {
        self.0.fraction_digits
    }

    fn __repr__(&self) -> String {
        let mut parts = Vec::new();
        if let Some(v) = self.0.max_length {
            parts.push(format!("max_length={v}"));
        }
        if let Some(e) = &self.0.enumeration {
            parts.push(format!("enumeration={} value(s)", e.len()));
        }
        if !self.0.patterns.is_empty() {
            parts.push(format!("patterns={} step(s)", self.0.patterns.len()));
        }
        format!("<Facets {}>", parts.join(" "))
    }
}

// ---------------------------------------------------------------------------
// Annotations, documents, diagnostics
// ---------------------------------------------------------------------------

fn annotation_doc(s: &Schemas, id: Option<AnnotationId>) -> Option<String> {
    let a = s.get_annotation(id?)?;
    (!a.documentation.is_empty()).then(|| a.doc())
}

fn annotation_appinfo(s: &Schemas, id: Option<AnnotationId>) -> Vec<PyAppInfo> {
    id.and_then(|i| s.get_annotation(i))
        .map(|a| {
            a.appinfo
                .iter()
                .map(|ai| PyAppInfo {
                    source: ai.source.clone(),
                    xml: ai.xml.clone(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Machine-readable annotation content, kept verbatim.
#[pyclass(
    module = "xsdkit",
    name = "AppInfo",
    frozen,
    get_all,
    skip_from_py_object
)]
#[derive(Clone)]
pub struct PyAppInfo {
    pub source: Option<String>,
    /// The `appinfo` element's children, re-serialized. Element and attribute
    /// names are in Clark notation, so no prefix can be lost.
    pub xml: String,
}

#[pymethods]
impl PyAppInfo {
    fn __repr__(&self) -> String {
        format!(
            "<AppInfo source={:?} {} bytes>",
            self.source,
            self.xml.len()
        )
    }
}

#[pyclass(
    module = "xsdkit",
    name = "Document",
    frozen,
    get_all,
    skip_from_py_object
)]
#[derive(Clone)]
pub struct PyDocument {
    pub uri: String,
    pub target_namespace: Option<String>,
    /// True when this document had no `targetNamespace` of its own and was
    /// absorbed into its includer's.
    pub chameleon: bool,
}

#[pymethods]
impl PyDocument {
    fn __repr__(&self) -> String {
        format!("<Document {} ns={:?}>", self.uri, self.target_namespace)
    }
}

#[pyclass(module = "xsdkit", name = "Span", frozen, get_all, skip_from_py_object)]
#[derive(Clone)]
pub struct PySpan {
    pub uri: String,
    pub line: u32,
    pub label: Option<String>,
}

#[pymethods]
impl PySpan {
    fn __str__(&self) -> String {
        Span {
            uri: self.uri.clone(),
            line: self.line,
            label: self.label.clone(),
        }
        .to_string()
    }

    fn __repr__(&self) -> String {
        format!("<Span {}>", self.__str__())
    }
}

#[pyclass(module = "xsdkit", name = "Diagnostic", frozen, skip_from_py_object)]
#[derive(Clone)]
pub struct PyDiagnostic(Diagnostic);

#[pymethods]
impl PyDiagnostic {
    /// The stable code, e.g. `"XSD1201"`.
    #[getter]
    fn code(&self) -> &'static str {
        self.0.code.as_str()
    }

    /// `"error"`, `"warning"` or `"note"`.
    #[getter]
    fn severity(&self) -> &'static str {
        match self.0.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Note => "note",
        }
    }

    #[getter]
    fn message(&self) -> String {
        self.0.message.clone()
    }

    #[getter]
    fn spans(&self) -> Vec<PySpan> {
        self.0
            .spans
            .iter()
            .map(|s| PySpan {
                uri: s.uri.clone(),
                line: s.line,
                label: s.label.clone(),
            })
            .collect()
    }

    #[getter]
    fn help(&self) -> Option<String> {
        self.0.help.clone()
    }

    #[getter]
    fn is_error(&self) -> bool {
        self.0.is_error()
    }

    fn __str__(&self) -> String {
        self.0.to_string()
    }

    fn __repr__(&self) -> String {
        format!("<Diagnostic {} {}>", self.code(), self.0.message)
    }
}

// ---------------------------------------------------------------------------
// Values, as native Python objects
// ---------------------------------------------------------------------------

/// Converts an XSD value into the closest native Python type.
///
/// This is most of what a binding is *for*: `<count>42</count>` should reach
/// Python as `42`, and a `dateTime` as a timezone-aware `datetime`, not as a
/// string the caller has to re-parse.
///
/// Durations and the gregorian fragments stay as their canonical lexical
/// forms — `xs:duration` has no lossless Python counterpart, since months and
/// seconds are not commensurable. `xs:dayTimeDuration` alone becomes a
/// `timedelta`, because there it is.
fn value_to_py<'py>(py: Python<'py>, v: &Value) -> PyResult<Bound<'py, PyAny>> {
    match v {
        Value::String(s) | Value::AnyUri(s) => s.into_bound_py_any(py),
        Value::Boolean(b) => b.into_bound_py_any(py),
        Value::Integer(n) => n.into_bound_py_any(py),
        Value::Float(f) => f32::from(*f).into_bound_py_any(py),
        Value::Double(d) => f64::from(*d).into_bound_py_any(py),
        Value::Decimal(d) => py
            .import("decimal")?
            .getattr("Decimal")?
            .call1((d.to_string(),)),
        Value::HexBinary(b) | Value::Base64Binary(b) => PyBytes::new(py, b).into_bound_py_any(py),
        Value::DateTime(dt) => {
            let (sec, micro) = split_seconds(&dt.second().to_string());
            py.import("datetime")?.getattr("datetime")?.call1((
                dt.year(),
                dt.month(),
                dt.day(),
                dt.hour(),
                dt.minute(),
                sec,
                micro,
                tzinfo(py, dt.timezone_offset().map(tz_minutes))?,
            ))
        }
        Value::Date(d) => {
            py.import("datetime")?
                .getattr("date")?
                .call1((d.year(), d.month(), d.day()))
        }
        Value::Time(t) => {
            let (sec, micro) = split_seconds(&t.second().to_string());
            py.import("datetime")?.getattr("time")?.call1((
                t.hour(),
                t.minute(),
                sec,
                micro,
                tzinfo(py, t.timezone_offset().map(tz_minutes))?,
            ))
        }
        Value::DayTimeDuration(d) => py.import("datetime")?.getattr("timedelta")?.call1((
            0,
            d.seconds().to_string().parse::<f64>().unwrap_or(0.0),
            0,
            0,
            d.minutes(),
            d.hours(),
            d.days(),
        )),
        Value::List(items) => {
            let list = PyList::empty(py);
            for item in items {
                list.append(value_to_py(py, item)?)?;
            }
            list.into_bound_py_any(py)
        }
        // No lossless Python type; the canonical lexical form is exact.
        other => other.to_string().into_bound_py_any(py),
    }
}

/// Splits `"15.25"` into whole seconds and microseconds.
fn split_seconds(text: &str) -> (u32, u32) {
    let f: f64 = text.parse().unwrap_or(0.0);
    let sec = f.trunc().max(0.0) as u32;
    let micro = ((f - f.trunc()) * 1_000_000.0).round() as u32;
    (sec, micro.min(999_999))
}

fn tz_minutes(tz: oxsdatatypes::TimezoneOffset) -> i16 {
    i16::from_be_bytes(tz.to_be_bytes())
}

fn tzinfo<'py>(py: Python<'py>, minutes: Option<i16>) -> PyResult<Option<Bound<'py, PyAny>>> {
    let Some(m) = minutes else { return Ok(None) };
    let dt = py.import("datetime")?;
    let delta = dt.getattr("timedelta")?.call1((0, 0, 0, 0, m))?;
    Ok(Some(dt.getattr("timezone")?.call1((delta,))?))
}

// ---------------------------------------------------------------------------
// Instance validation
// ---------------------------------------------------------------------------

/// The outcome of validating a document.
#[pyclass(
    module = "xsdkit",
    name = "ValidationReport",
    frozen,
    skip_from_py_object
)]
pub struct PyValidationReport {
    valid: bool,
    diagnostics: Vec<PyDiagnostic>,
}

#[pymethods]
impl PyValidationReport {
    #[getter]
    fn is_valid(&self) -> bool {
        self.valid
    }

    #[getter]
    fn diagnostics(&self) -> Vec<PyDiagnostic> {
        self.diagnostics.clone()
    }

    /// Only the diagnostics that are errors.
    #[getter]
    fn errors(&self) -> Vec<PyDiagnostic> {
        self.diagnostics
            .iter()
            .filter(|d| d.0.is_error())
            .cloned()
            .collect()
    }

    fn __bool__(&self) -> bool {
        self.valid
    }

    fn __repr__(&self) -> String {
        format!(
            "<ValidationReport {} ({} diagnostic(s))>",
            if self.valid { "valid" } else { "invalid" },
            self.diagnostics.len()
        )
    }
}

/// An attribute after validation.
#[pyclass(
    module = "xsdkit",
    name = "AttributeValue",
    frozen,
    skip_from_py_object
)]
pub struct PyAttributeValue {
    name: (Option<String>, String),
    declaration: Option<PyAttribute>,
    value: Option<Py<PyAny>>,
    lexical: String,
    from_schema: bool,
}

/// Hand-written because `Py<PyAny>` needs the GIL to clone.
impl Clone for PyAttributeValue {
    fn clone(&self) -> Self {
        Python::attach(|py| Self {
            name: self.name.clone(),
            declaration: self.declaration.clone(),
            value: self.value.as_ref().map(|v| v.clone_ref(py)),
            lexical: self.lexical.clone(),
            from_schema: self.from_schema,
        })
    }
}

#[pymethods]
impl PyAttributeValue {
    #[getter]
    fn name(&self) -> (Option<String>, String) {
        self.name.clone()
    }
    #[getter]
    fn local_name(&self) -> String {
        self.name.1.clone()
    }
    #[getter]
    fn declaration(&self) -> Option<PyAttribute> {
        self.declaration.clone()
    }
    /// The typed value, or `None` when it did not validate.
    #[getter]
    fn value(&self, py: Python<'_>) -> Option<Py<PyAny>> {
        self.value.as_ref().map(|v| v.clone_ref(py))
    }
    #[getter]
    fn lexical(&self) -> String {
        self.lexical.clone()
    }
    /// True when the document did not spell this out and the schema supplied
    /// it from a `default` or `fixed` value.
    ///
    /// Named `is_from_schema` in Rust so clippy does not read it as a
    /// constructor; Python sees `from_schema`, which is the right name there.
    #[getter(from_schema)]
    fn is_from_schema(&self) -> bool {
        self.from_schema
    }
    fn __repr__(&self) -> String {
        let src = if self.from_schema {
            " (from schema)"
        } else {
            ""
        };
        format!("<AttributeValue {}={:?}{}>", self.name.1, self.lexical, src)
    }
}

/// One post-schema-validation event.
///
/// A single class with a `kind` discriminator rather than three, because the
/// consuming loop is invariably a dispatch on kind.
#[pyclass(module = "xsdkit", name = "PsviEvent", frozen, skip_from_py_object)]
pub struct PyPsviEvent {
    kind: &'static str,
    name: (Option<String>, String),
    declaration: Option<PyElement>,
    type_: Option<PyType_>,
    type_from_instance: bool,
    nil: bool,
    attributes: Vec<PyAttributeValue>,
    value: Option<Py<PyAny>>,
    lexical: Option<String>,
    line: u32,
}

#[pymethods]
impl PyPsviEvent {
    /// `"start"`, `"text"` or `"end"`.
    #[getter]
    fn kind(&self) -> &'static str {
        self.kind
    }
    #[getter]
    fn name(&self) -> (Option<String>, String) {
        self.name.clone()
    }
    #[getter]
    fn local_name(&self) -> String {
        self.name.1.clone()
    }
    #[getter]
    fn declaration(&self) -> Option<PyElement> {
        self.declaration.clone()
    }
    /// The type in force, after any `xsi:type` override.
    #[getter]
    fn r#type(&self) -> Option<PyType_> {
        self.type_.clone()
    }
    #[getter]
    fn type_from_instance(&self) -> bool {
        self.type_from_instance
    }
    #[getter]
    fn nil(&self) -> bool {
        self.nil
    }
    #[getter]
    fn attributes(&self) -> Vec<PyAttributeValue> {
        self.attributes.clone()
    }
    /// The typed value, on a `"text"` event.
    #[getter]
    fn value(&self, py: Python<'_>) -> Option<Py<PyAny>> {
        self.value.as_ref().map(|v| v.clone_ref(py))
    }
    #[getter]
    fn lexical(&self) -> Option<String> {
        self.lexical.clone()
    }
    #[getter]
    fn line(&self) -> u32 {
        self.line
    }
    fn __repr__(&self) -> String {
        format!(
            "<PsviEvent {} {} line {}>",
            self.kind, self.name.1, self.line
        )
    }
}

// ---------------------------------------------------------------------------
// Module-level functions
// ---------------------------------------------------------------------------

/// Loads a schema and returns it **with** its diagnostics, rather than
/// raising.
///
/// Use this when a schema is expected to be imperfect — a vendor schema with
/// dangling imports, say — and you want the components anyway.
#[pyfunction]
#[pyo3(signature = (path, *, search_paths=None, conformance="lax", nodes_limit=None))]
fn load(
    py: Python<'_>,
    path: String,
    search_paths: Option<Vec<String>>,
    conformance: &str,
    nodes_limit: Option<u32>,
) -> PyResult<(PySchemaSet, Vec<PyDiagnostic>)> {
    let b = builder(search_paths, conformance, nodes_limit)?.file(path);
    let (schemas, diags) = py.detach(|| b.build_with_warnings());
    Ok((
        PySchemaSet::wrap(schemas),
        diags.into_iter().map(PyDiagnostic).collect(),
    ))
}

/// The same, from a string.
#[pyfunction]
#[pyo3(signature = (xsd, *, uri="<string>", search_paths=None, conformance="lax", nodes_limit=None))]
fn load_string(
    py: Python<'_>,
    xsd: String,
    uri: &str,
    search_paths: Option<Vec<String>>,
    conformance: &str,
    nodes_limit: Option<u32>,
) -> PyResult<(PySchemaSet, Vec<PyDiagnostic>)> {
    let b = builder(search_paths, conformance, nodes_limit)?.text(xsd, uri);
    let (schemas, diags) = py.detach(|| b.build_with_warnings());
    Ok((
        PySchemaSet::wrap(schemas),
        diags.into_iter().map(PyDiagnostic).collect(),
    ))
}

#[pymodule]
#[pyo3(name = "_xsdkit")]
fn xsdkit_module(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    m.add("XsdError", m.py().get_type::<XsdError>())?;
    let schema_error = m.py().get_type::<SchemaError>();
    // A class-level default so `except SchemaError as e: e.diagnostics` is
    // always safe, even for an error raised on a path that set none.
    schema_error.setattr("diagnostics", PyList::empty(m.py()))?;
    m.add("SchemaError", schema_error)?;
    m.add_class::<PySchemaSet>()?;
    m.add_class::<PyElement>()?;
    m.add_class::<PyAttribute>()?;
    m.add_class::<PyAttributeUse>()?;
    m.add_class::<PyType_>()?;
    m.add_class::<PyFacets>()?;
    m.add_class::<PyAppInfo>()?;
    m.add_class::<PyDocument>()?;
    m.add_class::<PySpan>()?;
    m.add_class::<PyDiagnostic>()?;
    m.add_class::<PyValidationReport>()?;
    m.add_class::<PyAttributeValue>()?;
    m.add_class::<PyPsviEvent>()?;
    m.add_function(wrap_pyfunction!(load, m)?)?;
    m.add_function(wrap_pyfunction!(load_string, m)?)?;
    Ok(())
}
