//! Python bindings.
//!
//! Every Python handle is a pair: an `Arc<Schemas>` and a `Copy` id. Handing
//! out ten thousand element wrappers costs ten thousand refcount bumps and
//! copies nothing, and no component leaves the model until Python asks for a
//! specific field. This is where the arena design pays off a second time —
//! and because `Schemas` is `Send + Sync`, the GIL is released around
//! compilation, which is the only slow part.

#![allow(clippy::needless_pass_by_value)]
// The constructors take one parameter per keyword argument, and Python
// keyword arguments do not have the readability cost that positional ones do —
// `from_string(xsd, version="1.1")` names what it passes. The lint counts them
// the same way regardless.
#![allow(clippy::too_many_arguments)]
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
use crate::{Conformance, FileResolver, SchemaSetBuilder, Version};
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyList, PyTuple, PyType};
use pyo3::{IntoPyObjectExt, create_exception};
use std::collections::BTreeMap;
use std::sync::Arc;

create_exception!(
    xsdkit,
    XsdError,
    pyo3::exceptions::PyException,
    "The base of every error this package raises, for `except xsdkit.XsdError`."
);
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

/// Bridges a Python callable into the [`Resolver`] trait.
///
/// The contract is deliberately small, because the caller already has a
/// language for this: a function of `(location, base)` that returns the
/// document, or raises. Returning `bytes` leaves the encoding to
/// `xsdkit` — byte-order mark, then the XML declaration, then UTF-8 — which is
/// the same treatment `from_bytes` gives, and the reason a resolver should not
/// decode for itself.
struct PyResolver {
    callable: Py<PyAny>,
}

impl crate::load::Resolver for PyResolver {
    fn resolve(&self, location: &str, base: Option<&str>) -> Result<(String, Vec<u8>), String> {
        // Reacquires the GIL: `build()` released it, and this is Python code.
        Python::attach(|py| {
            let out = self
                .callable
                .call1(py, (location, base))
                .map_err(|e| e.to_string())?;
            let out = out.bind(py);

            // `(uri, document)` when the resolver followed a redirect and
            // wants relative locations resolved against where it landed;
            // otherwise the location stands as the URI.
            if let Ok(t) = out.cast::<PyTuple>() {
                if t.len() != 2 {
                    return Err("a resolver tuple must be (uri, document)".into());
                }
                let uri: String = t
                    .get_item(0)
                    .and_then(|v| v.extract())
                    .map_err(|e| e.to_string())?;
                return Ok((
                    uri,
                    extract_document(&t.get_item(1).map_err(|e| e.to_string())?)?,
                ));
            }
            Ok((location.to_string(), extract_document(out)?))
        })
    }
}

/// An instance document given as `str` or as `bytes`.
///
/// Bytes are decoded the way a schema's are — byte-order mark, then the XML
/// declaration, then UTF-8 — so a document read with `open(path, "rb")` needs
/// no guess about its encoding, which is exactly the guess a caller is most
/// likely to get wrong.
fn instance_text(obj: &Bound<'_, PyAny>) -> PyResult<String> {
    if let Ok(s) = obj.extract::<String>() {
        return Ok(s);
    }
    let bytes: Vec<u8> = obj
        .extract()
        .map_err(|_| PyValueError::new_err("a document must be str or bytes"))?;
    crate::encoding::decode_document(&bytes, "<instance>")
        .map(|d| d.text)
        .map_err(|d| PyValueError::new_err(d.message))
}

/// A resolver's document, as `bytes` or as `str`.
fn extract_document(obj: &Bound<'_, PyAny>) -> Result<Vec<u8>, String> {
    if let Ok(b) = obj.extract::<Vec<u8>>() {
        return Ok(b);
    }
    obj.extract::<String>()
        .map(String::into_bytes)
        .map_err(|_| "a resolver must return bytes, str, or (uri, bytes)".to_string())
}

/// Accepts anything `os.fspath` understands, which in practice means a
/// `pathlib.Path` as readily as a `str`. Refusing one is friction with no
/// upside — every caller has a `Path`.
fn path_from(obj: &Bound<'_, PyAny>) -> PyResult<String> {
    let py = obj.py();
    let s = py
        .import("os")?
        .call_method1("fspath", (obj,))?
        .extract::<std::ffi::OsString>()?;
    s.into_string()
        .map_err(|_| PyValueError::new_err("the path is not valid Unicode"))
}

fn version_from(s: &str) -> PyResult<Version> {
    match s {
        "1.0" => Ok(Version::Xsd10),
        "1.1" => Ok(Version::Xsd11),
        other => Err(PyValueError::new_err(format!(
            "version must be '1.0' or '1.1', got {other:?}"
        ))),
    }
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

/// Walks the PSVI events of one document.
///
/// The events are produced eagerly and handed out one at a time. Validation is
/// a single pass that has to reach the end of the document to know whether the
/// content model was satisfied, so there is nothing to gain by deferring it.
/// What this buys is the *shape* Python expects — `for ev in ...` rather than a
/// callback — so `enumerate`, `zip`, `itertools` and generator expressions all
/// work on it.
#[pyclass(name = "PsviEvents", module = "xsdkit")]
pub struct PyPsviEvents {
    events: Vec<Py<PyPsviEvent>>,
    at: usize,
    valid: bool,
    diagnostics: Vec<PyDiagnostic>,
}

#[pymethods]
impl PyPsviEvents {
    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __next__(mut slf: PyRefMut<'_, Self>) -> Option<Py<PyPsviEvent>> {
        let at = slf.at;
        let out = Python::attach(|py| slf.events.get(at).map(|e| e.clone_ref(py)));
        slf.at += 1;
        out
    }

    /// How many events are left.
    fn __len__(&self) -> usize {
        self.events.len().saturating_sub(self.at)
    }

    /// The outcome, available before the events are consumed as well as after.
    ///
    /// A document can be read for its values and still be invalid, so this is
    /// not something to discover only once the loop has ended.
    #[getter]
    fn report(&self) -> PyValidationReport {
        PyValidationReport {
            valid: self.valid,
            diagnostics: self.diagnostics.clone(),
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "<PsviEvents {} remaining, {}>",
            self.__len__(),
            if self.valid { "valid" } else { "invalid" }
        )
    }
}

/// Walks the global names of a [`PySchemaSet`].
///
/// A snapshot rather than a live cursor: the model is immutable, so there is
/// nothing to invalidate, and holding the names costs one allocation against
/// the alternative of keeping an index into two maps in step.
#[pyclass(name = "NameIterator", module = "xsdkit")]
pub struct PyNameIter {
    names: Vec<String>,
    at: usize,
}

#[pymethods]
impl PyNameIter {
    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __next__(mut slf: PyRefMut<'_, Self>) -> Option<String> {
        let out = slf.names.get(slf.at).cloned();
        slf.at += 1;
        out
    }

    fn __len__(&self) -> usize {
        self.names.len().saturating_sub(self.at)
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
    version: &str,
    nodes_limit: Option<u32>,
    resolver: Option<Py<PyAny>>,
) -> PyResult<SchemaSetBuilder> {
    let mut b = SchemaSetBuilder::new()
        .conformance(conformance_from(conformance)?)
        .version(version_from(version)?);
    // A custom resolver replaces the filesystem entirely, so the two are
    // alternatives rather than layers — a caller serving documents from a zip
    // has no search path to add them to.
    match (resolver, search_paths) {
        (Some(callable), _) => b = b.resolver(PyResolver { callable }),
        (None, Some(paths)) => {
            let mut fr = FileResolver::new();
            fr.search_paths = paths.into_iter().map(Into::into).collect();
            b = b.resolver(fr);
        }
        (None, None) => {}
    }
    if let Some(limit) = nodes_limit {
        b = b.nodes_limit(limit);
    }
    Ok(b)
}

impl PySchemaSet {
    /// The global types this schema set's documents declare.
    ///
    /// The XSD built-ins live in the same table — they have to, so that
    /// `type="xs:int"` resolves like any other reference — but they are not
    /// part of what a schema *says*, and every Python-facing enumeration
    /// leaves them out.
    fn declared_types(&self) -> impl Iterator<Item = (&QName, &TypeId)> {
        // By namespace, not by `as_builtin`: that one answers from
        // `SimpleType::builtin` and so cannot see `xs:anyType`, which is
        // complex. Everything predeclared lives in the XSD namespace, and
        // nothing a document declares may.
        self.inner.globals().types.iter().filter(|(q, _)| {
            q.ns.is_none_or(|ns| self.inner.names().resolve_ns(ns) != crate::names::XS)
        })
    }
}

#[pymethods]
impl PySchemaSet {
    /// Loads a schema from a file, following its includes and imports.
    #[classmethod]
    #[pyo3(signature = (path, *, search_paths=None, conformance="strict", version="1.0", nodes_limit=None, resolver=None))]
    fn from_file(
        _cls: &Bound<'_, PyType>,
        py: Python<'_>,
        path: &Bound<'_, PyAny>,
        search_paths: Option<Vec<String>>,
        conformance: &str,
        version: &str,
        nodes_limit: Option<u32>,
        resolver: Option<Py<PyAny>>,
    ) -> PyResult<Self> {
        let b = builder(search_paths, conformance, version, nodes_limit, resolver)?
            .file(path_from(path)?);
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
    #[pyo3(signature = (xsd, *, uri="<string>", search_paths=None, conformance="strict", version="1.0", nodes_limit=None, resolver=None))]
    fn from_string(
        _cls: &Bound<'_, PyType>,
        py: Python<'_>,
        xsd: String,
        uri: &str,
        search_paths: Option<Vec<String>>,
        conformance: &str,
        version: &str,
        nodes_limit: Option<u32>,
        resolver: Option<Py<PyAny>>,
    ) -> PyResult<Self> {
        let b = builder(search_paths, conformance, version, nodes_limit, resolver)?.text(xsd, uri);
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
    #[pyo3(signature = (data, *, uri="<bytes>", search_paths=None, conformance="strict", version="1.0", nodes_limit=None, resolver=None))]
    fn from_bytes(
        _cls: &Bound<'_, PyType>,
        py: Python<'_>,
        data: Vec<u8>,
        uri: &str,
        search_paths: Option<Vec<String>>,
        conformance: &str,
        version: &str,
        nodes_limit: Option<u32>,
        resolver: Option<Py<PyAny>>,
    ) -> PyResult<Self> {
        let b =
            builder(search_paths, conformance, version, nodes_limit, resolver)?.bytes(data, uri);
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
                version: d.version.clone(),
            })
            .collect()
    }

    /// How many global elements and types *this schema* declares.
    ///
    /// `SchemaSet` behaves as a mapping from a global name to its declaration:
    /// `len`, `in`, `[]` and iteration all work, and iterating yields names, so
    /// `dict(s)` and `for name in s` read as they do for any other mapping.
    ///
    /// The XSD built-in types are excluded. They are present in every schema
    /// set, so counting them would drown a two-element schema in fifty
    /// entries — `len(s)` is meant to answer "how much is in this schema". They
    /// are still reachable by name through [`Self::type`], which resolves
    /// rather than enumerates. [`Self::counts`] is the component tally, and
    /// counts a great deal more than the globals.
    fn __len__(&self) -> usize {
        let g = self.inner.globals();
        g.elements.len() + self.declared_types().count()
    }

    fn __contains__(&self, name: &Bound<'_, PyAny>) -> PyResult<bool> {
        let Some(q) = parse_name(&self.inner, name)? else {
            return Ok(false);
        };
        Ok(self.inner.globals().elements.contains_key(&q)
            || self.inner.globals().types.contains_key(&q)
                && q.ns
                    .is_none_or(|ns| self.inner.names().resolve_ns(ns) != crate::names::XS))
    }

    /// The element or type of that name, raising `KeyError` when there is
    /// none.
    ///
    /// The lookup methods return `None` instead, for when absence is an
    /// ordinary answer; this is for when it is a mistake.
    fn __getitem__<'py>(
        &self,
        py: Python<'py>,
        name: &Bound<'_, PyAny>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let q = parse_name(&self.inner, name)?;
        if let Some(q) = q {
            if let Some(id) = self.inner.globals().elements.get(&q) {
                return PyElement {
                    s: self.inner.clone(),
                    id: *id,
                }
                .into_bound_py_any(py);
            }
            if let Some(id) = self.inner.globals().types.get(&q) {
                // Agrees with `in` and with iteration: a built-in is not one
                // of this schema's declarations, and `type()` is the way to
                // resolve one.
                if q.ns
                    .is_none_or(|ns| self.inner.names().resolve_ns(ns) != crate::names::XS)
                {
                    return PyType_ {
                        s: self.inner.clone(),
                        id: *id,
                    }
                    .into_bound_py_any(py);
                }
            }
        }
        Err(pyo3::exceptions::PyKeyError::new_err(
            name.repr()?.to_string(),
        ))
    }

    /// The global names, elements before types, each sorted.
    fn __iter__(&self) -> PyResult<Py<PyNameIter>> {
        let mut names: Vec<String> = self
            .elements()
            .into_iter()
            .map(|(n, _)| n)
            .chain(self.types().into_iter().map(|(n, _)| n))
            .collect();
        names.dedup();
        Python::attach(|py| Py::new(py, PyNameIter { names, at: 0 }))
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

    /// Every global type definition *this schema* declares, keyed by
    /// Clark-notation name.
    ///
    /// The XSD built-ins are excluded, for the same reason [`Self::__len__`]
    /// excludes them: they are in every schema set and would bury the ones the
    /// document actually wrote. `type("{...}string")` still resolves them.
    #[getter]
    fn types(&self) -> Vec<(String, PyType_)> {
        let mut v: Vec<_> = self
            .declared_types()
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
    fn validate(
        &self,
        py: Python<'_>,
        xml: &Bound<'_, PyAny>,
        uri: &str,
    ) -> PyResult<PyValidationReport> {
        let xml = instance_text(xml)?;
        let schemas = self.inner.clone();
        // No Python is called back into, so the GIL can go.
        let report = py.detach(|| {
            schemas
                .instance_validator()
                .validate_named(&xml, uri, |_| {})
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
    /// Reads a document into typed PSVI events, one at a time.
    ///
    /// The iterator form of [`Self::read_typed`], and the one to reach for:
    /// `for ev in schemas.iter_typed(xml)` composes with everything Python
    /// has for iterables, where a callback composes with nothing. The outcome
    /// is on the iterator's `report`, before or after the loop.
    #[pyo3(signature = (xml, *, uri="<instance>"))]
    fn iter_typed(
        &self,
        py: Python<'_>,
        xml: &Bound<'_, PyAny>,
        uri: &str,
    ) -> PyResult<PyPsviEvents> {
        let xml = instance_text(xml)?;
        let mut events: Vec<Py<PyPsviEvent>> = Vec::new();
        let mut failed: Option<PyErr> = None;
        let report = self
            .inner
            .instance_validator()
            .validate_named(&xml, uri, |ev| {
                if failed.is_some() {
                    return;
                }
                match self.psvi_to_py(py, ev) {
                    Ok(obj) => events.push(obj.unbind()),
                    Err(e) => failed = Some(e),
                }
            });
        if let Some(e) = failed {
            return Err(e);
        }
        Ok(PyPsviEvents {
            events,
            at: 0,
            valid: report.is_valid(),
            diagnostics: report.diagnostics.into_iter().map(PyDiagnostic).collect(),
        })
    }

    /// Reads a document into typed PSVI events, collected or streamed.
    ///
    /// Prefer `iter_typed`, which is the same thing in the shape Python
    /// expects. This form exists for feeding a callback that already exists,
    /// and returns the events as a list, or `None` in their place when
    /// `on_event` took them.
    #[pyo3(signature = (xml, *, on_event=None, uri="<instance>"))]
    fn read_typed(
        &self,
        py: Python<'_>,
        xml: &Bound<'_, PyAny>,
        on_event: Option<Bound<'_, PyAny>>,
        uri: &str,
    ) -> PyResult<(Option<Vec<Py<PyPsviEvent>>>, PyValidationReport)> {
        let xml = instance_text(xml)?;
        let mut collected: Vec<Py<PyPsviEvent>> = Vec::new();
        let mut callback_error: Option<PyErr> = None;

        // The GIL is held throughout: every event becomes a Python object,
        // and `on_event` is Python code.
        let report = self
            .inner
            .instance_validator()
            .validate_named(&xml, uri, |ev| {
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
    fn counts(&self) -> std::collections::BTreeMap<String, usize> {
        let c = self.inner.component_counts();
        BTreeMap::from([
            ("types".into(), c.types),
            ("elements".into(), c.elements),
            ("attributes".into(), c.attributes),
            ("particles".into(), c.particles),
            ("model_groups".into(), c.model_groups),
            ("attribute_groups".into(), c.attribute_groups),
            ("identity_constraints".into(), c.identity_constraints),
            ("annotations".into(), c.annotations),
        ])
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

/// An element declaration: a name, a type, and how it may appear.
///
/// A handle into the schema, not a copy — holding ten thousand of them costs
/// ten thousand refcounts. Two handles to the same declaration compare equal
/// and hash alike, so they work as dict keys and set members.
///
///     >>> report = schemas.element("urn:example", "report")
///     >>> report.type.children          # what may appear inside
///     >>> report.substitutes            # what may appear *instead*
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

    /// The local part of the name, without its namespace.
    #[getter]
    fn local_name(&self) -> String {
        self.s
            .names()
            .resolve(self.s[self.id].name.local)
            .to_string()
    }

    /// The namespace URI, or `None` when the name is unqualified.
    #[getter]
    fn namespace(&self) -> Option<String> {
        self.s[self.id]
            .name
            .ns
            .map(|n| self.s.names().resolve_ns(n).to_string())
    }

    /// The type in force for this element.
    #[getter]
    fn r#type(&self) -> PyType_ {
        PyType_ {
            s: self.s.clone(),
            id: self.s[self.id].type_id,
        }
    }

    /// Whether an instance may be empty by saying `xsi:nil="true"`.
    ///
    /// Nil is not the same as absent, and not the same as empty: it says the
    /// element is *present and has no value*.
    #[getter]
    fn nillable(&self) -> bool {
        self.s[self.id].nillable
    }

    /// Whether this element may not appear itself.
    ///
    /// An abstract head exists to be substituted for — see `substitutes`.
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

    /// The `default` value, supplied when the element is present but empty.
    #[getter]
    fn default(&self) -> Option<String> {
        match &self.s[self.id].value_constraint {
            Some(ValueConstraint::Default(v)) => Some(v.clone()),
            _ => None,
        }
    }

    /// The `fixed` value, which an instance may repeat but not contradict.
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

/// An attribute declaration.
///
/// The declaration itself, shared by every type that uses it. How a particular
/// type uses it — required, optional, prohibited, with what default — is on
/// `AttributeUse`, which is what `Type.attributes` returns.
#[pyclass(module = "xsdkit", name = "Attribute", frozen, skip_from_py_object)]
#[derive(Clone)]
pub struct PyAttribute {
    s: Arc<Schemas>,
    id: AttributeId,
}

#[pymethods]
impl PyAttribute {
    /// The name as a `(namespace, local)` pair.
    #[getter]
    fn name(&self) -> (Option<String>, String) {
        let q = self.s[self.id].name;
        (
            q.ns.map(|n| self.s.names().resolve_ns(n).to_string()),
            self.s.names().resolve(q.local).to_string(),
        )
    }

    /// The name in Clark notation, `{namespace}local`.
    #[getter]
    fn qname(&self) -> String {
        clark(&self.s, self.s[self.id].name)
    }

    /// The local part of the name, without its namespace.
    #[getter]
    fn local_name(&self) -> String {
        self.s
            .names()
            .resolve(self.s[self.id].name.local)
            .to_string()
    }

    /// The simple type of this attribute's value.
    #[getter]
    fn r#type(&self) -> PyType_ {
        PyType_ {
            s: self.s.clone(),
            id: self.s[self.id].type_id,
        }
    }

    /// The `default` value the schema supplies when the attribute is absent.
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
        format!("<Attribute {}>", self.qname())
    }

    /// Two handles to the same declaration are the same declaration.
    ///
    /// Without this the class falls back to identity, and looking the same
    /// attribute up twice yields two objects that are unequal and hash apart —
    /// so a set of them silently holds duplicates. That is worse than being
    /// unhashable, because nothing raises.
    fn __eq__(&self, other: &PyAttribute) -> bool {
        self.id == other.id && Arc::ptr_eq(&self.s, &other.s)
    }

    fn __hash__(&self) -> u64 {
        self.id.index() as u64
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
    /// The declaration this use refers to.
    ///
    /// Several types may use one declaration, each with its own `use` and
    /// value constraint.
    #[getter]
    fn attribute(&self) -> PyAttribute {
        PyAttribute {
            s: self.s.clone(),
            id: self.attribute,
        }
    }

    /// The name as a `(namespace, local)` pair.
    #[getter]
    fn name(&self) -> (Option<String>, String) {
        self.attribute().name()
    }

    /// The local part of the name, without its namespace.
    #[getter]
    fn local_name(&self) -> String {
        self.attribute().local_name()
    }

    /// The simple type of this attribute's value.
    #[getter]
    fn r#type(&self) -> PyType_ {
        self.attribute().r#type()
    }

    /// Whether an instance must carry this attribute.
    ///
    /// The same question as `use == "required"`, asked the way it is usually
    /// asked.
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

    /// The `default` for this use, which overrides the declaration's.
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

    fn __eq__(&self, other: &PyAttributeUse) -> bool {
        self.attribute == other.attribute
            && self.kind == other.kind
            && self.constraint == other.constraint
            && Arc::ptr_eq(&self.s, &other.s)
    }

    fn __hash__(&self) -> u64 {
        self.attribute.index() as u64
    }
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A type definition, simple or complex.
///
/// The centre of the model. A complex type answers what may appear inside it
/// (`children`, `attributes`, `accepts`); a simple type answers what its
/// values may be (`validate`, `facets`, `variety`). `is_complex` says which
/// you have.
///
///     >>> t = schemas.type("urn:example", "Measurement")
///     >>> t.validate("3.14")            # the typed value, or ValueError
///     >>> t.facets.max_inclusive        # the constraints in force
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

    /// The name in Clark notation, or `None` for an anonymous type.
    ///
    /// A type declared inline inside an element has no name to report.
    #[getter]
    fn qname(&self) -> Option<String> {
        self.s[self.id].name().map(|q| clark(&self.s, q))
    }

    /// Whether this type may have attributes and child elements.
    #[getter]
    fn is_complex(&self) -> bool {
        self.s[self.id].as_complex().is_some()
    }

    /// Whether this type has a value space — something `validate` can parse.
    #[getter]
    fn is_simple(&self) -> bool {
        self.s[self.id].is_simple()
    }

    /// Whether an instance may not use this type directly, only one derived from it.
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

    /// The facets *this type* declares, without its base's.
    ///
    /// What the restriction step wrote, which is what a tool rendering a
    /// schema back wants. [`Self::facets`] is what a validator applies.
    #[getter]
    fn declared_facets(&self) -> Option<PyFacets> {
        self.s[self.id]
            .as_simple()
            .map(|t| PyFacets(t.facets.clone()))
    }

    /// The facets in force, composed down the whole restriction chain.
    ///
    /// Not the ones this type declares — those are on
    /// [`Self::declared_facets`]. A restriction inherits everything its base
    /// constrained, so a type that says only `maxLength` still has its base's
    /// `minLength`, and reporting the declared set alone disagrees with what
    /// `validate` does.
    #[getter]
    fn facets(&self) -> Option<PyFacets> {
        self.s[self.id]
            .as_simple()
            .map(|_| PyFacets(crate::validate::effective_facets(&self.s, self.id)))
    }

    /// The `xs:documentation` text, entries joined.
    #[getter]
    fn doc(&self) -> Option<String> {
        let ann = match &self.s[self.id] {
            TypeDefinition::Simple(t) => t.annotation,
            TypeDefinition::Complex(t) => t.annotation,
        };
        annotation_doc(&self.s, ann)
    }

    /// The `xs:appinfo` blocks, with their XML kept verbatim.
    ///
    /// Kept as written, because a summary cannot be un-summarised: this is
    /// where a schema hides units, labels and anything else its authors agreed
    /// on.
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

/// A set of facets on a simple type.
///
/// The bounds and enumerations are kept as the lexical forms the schema wrote,
/// not as typed values: a facet constrains the lexical space as much as the
/// value space, and the string is what the document said. Pass one through
/// `Type.validate` to get the value.
#[pyclass(module = "xsdkit", name = "Facets", frozen, skip_from_py_object)]
#[derive(Clone)]
pub struct PyFacets(crate::datatypes::FacetSet);

#[pymethods]
impl PyFacets {
    /// Exact length. Characters, or *items* for a list type.
    #[getter]
    fn length(&self) -> Option<u64> {
        self.0.length
    }
    /// Least length, in characters or list items.
    #[getter]
    fn min_length(&self) -> Option<u64> {
        self.0.min_length
    }
    /// Greatest length, in characters or list items.
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

    /// The permitted values, as the lexical forms the schema wrote.
    ///
    /// Compared in the value space, so an enumeration listing `1.0` admits
    /// `1.00`.
    #[getter]
    fn enumeration(&self) -> Option<Vec<String>> {
        self.0.enumeration.clone()
    }

    /// `"preserve"`, `"replace"` or `"collapse"` when stated explicitly.
    #[getter]
    fn white_space(&self) -> Option<String> {
        self.0.white_space.map(|w| w.to_string())
    }

    /// Upper bound, inclusive.
    #[getter]
    fn max_inclusive(&self) -> Option<String> {
        self.0.max_inclusive.clone()
    }
    /// Upper bound, exclusive.
    #[getter]
    fn max_exclusive(&self) -> Option<String> {
        self.0.max_exclusive.clone()
    }
    /// Lower bound, inclusive.
    #[getter]
    fn min_inclusive(&self) -> Option<String> {
        self.0.min_inclusive.clone()
    }
    /// Lower bound, exclusive.
    #[getter]
    fn min_exclusive(&self) -> Option<String> {
        self.0.min_exclusive.clone()
    }
    /// Most significant digits a decimal may have.
    #[getter]
    fn total_digits(&self) -> Option<u32> {
        self.0.total_digits
    }
    /// Most digits a decimal may have after the point.
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

    /// A value, not a handle: two facet sets with the same constraints are the
    /// same set. Deliberately no `__hash__` — a `FacetSet` holds vectors, and
    /// nothing needs facets as a dict key.
    fn __eq__(&self, other: &PyFacets) -> bool {
        self.0 == other.0
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
    /// The `source` attribute — a URI naming what convention the payload
    /// follows, when the schema said.
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

/// One schema document that went into the set.
///
/// A schema is often many files — `xs:include` and `xs:import` pull in more —
/// and this is the record of each, including which namespace it ended up in.
#[pyclass(
    module = "xsdkit",
    name = "Document",
    frozen,
    get_all,
    skip_from_py_object
)]
#[derive(Clone)]
pub struct PyDocument {
    /// Where this document was read from — a path, a URL, or whatever a
    /// custom resolver called it.
    pub uri: String,
    /// The namespace its declarations landed in, `None` for a no-namespace
    /// schema.
    pub target_namespace: Option<String>,
    /// True when this document had no `targetNamespace` of its own and was
    /// absorbed into its includer's.
    pub chameleon: bool,
    /// The `xs:schema` `version` attribute, verbatim. The specification gives
    /// it no structure and no meaning, so it is reported, not interpreted.
    pub version: Option<String>,
}

#[pymethods]
impl PyDocument {
    fn __repr__(&self) -> String {
        format!("<Document {} ns={:?}>", self.uri, self.target_namespace)
    }

    /// A value, not a handle: two with the same fields are the same.
    fn __eq__(&self, other: &PyDocument) -> bool {
        self.uri == other.uri
            && self.target_namespace == other.target_namespace
            && self.chameleon == other.chameleon
            && self.version == other.version
    }

    fn __hash__(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (&self.uri, &self.target_namespace).hash(&mut h);
        h.finish()
    }
}

/// Where in a document a diagnostic points, and what that place is.
///
/// One diagnostic may carry several — an ambiguous content model names both
/// particles that could match, and the labels say which is which.
#[pyclass(module = "xsdkit", name = "Span", frozen, get_all, skip_from_py_object)]
#[derive(Clone)]
pub struct PySpan {
    /// The document this points into.
    pub uri: String,
    /// The line, counting from one. Zero when the position is not known.
    pub line: u32,
    /// What this place *is*, when a diagnostic names more than one — "one
    /// candidate" and "the other", say.
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

    /// A value, not a handle: two with the same fields are the same.
    fn __eq__(&self, other: &PySpan) -> bool {
        self.uri == other.uri && self.line == other.line && self.label == other.label
    }

    fn __hash__(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (&self.uri, self.line, &self.label).hash(&mut h);
        h.finish()
    }
}

/// Something the reader found wrong, or worth saying.
///
/// Carries a stable `code` to match on, a `message` for people, `spans` for
/// where, and often `help` for what to do. `str()` renders the lot the way a
/// compiler would.
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

    /// What is wrong, in a sentence.
    #[getter]
    fn message(&self) -> String {
        self.0.message.clone()
    }

    /// Where it is, sometimes in more than one place.
    ///
    /// An ambiguous content model names both particles that could match, and
    /// the labels say which is which.
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

    /// What to do about it, when there is something useful to say.
    #[getter]
    fn help(&self) -> Option<String> {
        self.0.help.clone()
    }

    /// Whether this stops the schema loading, as opposed to a warning or a note.
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
                tzinfo(py, dt.timezone_offset().map(|t| t.minutes()))?,
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
                tzinfo(py, t.timezone_offset().map(|t| t.minutes()))?,
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
    /// Whether the document satisfied the schema.
    ///
    /// The report is falsy when it did not, so `if not report:` reads.
    #[getter]
    fn is_valid(&self) -> bool {
        self.valid
    }

    /// Everything found, warnings and notes included.
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
    /// The name as a `(namespace, local)` pair.
    #[getter]
    fn name(&self) -> (Option<String>, String) {
        self.name.clone()
    }
    /// The local part of the name, without its namespace.
    #[getter]
    fn local_name(&self) -> String {
        self.name.1.clone()
    }
    /// The declaration this matched, absent under a `skip` wildcard.
    #[getter]
    fn declaration(&self) -> Option<PyAttribute> {
        self.declaration.clone()
    }
    /// The typed value, or `None` when it did not validate.
    #[getter]
    fn value(&self, py: Python<'_>) -> Option<Py<PyAny>> {
        self.value.as_ref().map(|v| v.clone_ref(py))
    }
    /// The attribute exactly as the document wrote it.
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
    /// The element's name as a `(namespace, local)` pair.
    #[getter]
    fn name(&self) -> (Option<String>, String) {
        self.name.clone()
    }
    /// The local part of the name, without its namespace.
    #[getter]
    fn local_name(&self) -> String {
        self.name.1.clone()
    }
    /// The declaration this element matched.
    ///
    /// Absent under a `skip` wildcard, or a `lax` one with nothing to match.
    #[getter]
    fn declaration(&self) -> Option<PyElement> {
        self.declaration.clone()
    }
    /// The type in force, after any `xsi:type` override.
    #[getter]
    fn r#type(&self) -> Option<PyType_> {
        self.type_.clone()
    }
    /// Whether `xsi:type` chose the type, rather than the declaration.
    #[getter]
    fn type_from_instance(&self) -> bool {
        self.type_from_instance
    }
    /// Whether the element said `xsi:nil="true"`.
    #[getter]
    fn nil(&self) -> bool {
        self.nil
    }
    /// The attributes, typed, including any the schema supplied.
    #[getter]
    fn attributes(&self) -> Vec<PyAttributeValue> {
        self.attributes.clone()
    }
    /// The typed value, on a `"text"` event.
    #[getter]
    fn value(&self, py: Python<'_>) -> Option<Py<PyAny>> {
        self.value.as_ref().map(|v| v.clone_ref(py))
    }
    /// The character content exactly as the document wrote it.
    #[getter]
    fn lexical(&self) -> Option<String> {
        self.lexical.clone()
    }
    /// The line the element started on, counting from one.
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
#[pyo3(signature = (path, *, search_paths=None, conformance="lax", version="1.0", nodes_limit=None, resolver=None))]
fn load(
    py: Python<'_>,
    path: &Bound<'_, PyAny>,
    search_paths: Option<Vec<String>>,
    conformance: &str,
    version: &str,
    nodes_limit: Option<u32>,
    resolver: Option<Py<PyAny>>,
) -> PyResult<(PySchemaSet, Vec<PyDiagnostic>)> {
    let b =
        builder(search_paths, conformance, version, nodes_limit, resolver)?.file(path_from(path)?);
    let (schemas, diags) = py.detach(|| b.build_with_warnings());
    Ok((
        PySchemaSet::wrap(schemas),
        diags.into_iter().map(PyDiagnostic).collect(),
    ))
}

/// The same, from a string.
#[pyfunction]
#[pyo3(signature = (xsd, *, uri="<string>", search_paths=None, conformance="lax", version="1.0", nodes_limit=None, resolver=None))]
fn load_string(
    py: Python<'_>,
    xsd: String,
    uri: &str,
    search_paths: Option<Vec<String>>,
    conformance: &str,
    version: &str,
    nodes_limit: Option<u32>,
    resolver: Option<Py<PyAny>>,
) -> PyResult<(PySchemaSet, Vec<PyDiagnostic>)> {
    let b = builder(search_paths, conformance, version, nodes_limit, resolver)?.text(xsd, uri);
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
    m.add_class::<PyNameIter>()?;
    m.add_class::<PyPsviEvents>()?;
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
