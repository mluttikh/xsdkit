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
use crate::decode::{Decoded, DecodedContent};
use crate::diagnostics::{Diagnostic, Diagnostics, Severity, Span};
use crate::instance::PsviEvent as RustPsvi;
use crate::model::*;
use crate::names::QName;
use crate::refs::{AttributeRef, ElementRef, TypeRef};
use crate::values::Value;
use crate::{Compilation, Conformance, FileResolver, SchemaSetBuilder, Version};
use fxhash::{FxHashMap, FxHashSet};
use pyo3::exceptions::{
    PyException, PyIndexError, PyKeyError, PyOSError, PyTypeError, PyValueError,
};
use pyo3::prelude::*;
use pyo3::sync::PyOnceLock;
use pyo3::types::{PyByteArray, PyBytes, PyDict, PyIterator, PyList, PyString, PyTuple, PyType};
use pyo3::{IntoPyObjectExt, create_exception};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

create_exception!(
    xsdkit,
    XsdError,
    pyo3::exceptions::PyException,
    "The base of the errors about schemas, documents and values. A wrong argument type raises `TypeError`, and a path that cannot be read raises `OSError`."
);
create_exception!(
    xsdkit,
    SchemaError,
    XsdError,
    "Raised when a schema cannot be built. Carries every diagnostic on `.diagnostics`."
);
create_exception!(
    xsdkit,
    DocumentError,
    XsdError,
    "Raised by `decode` for a document that does not satisfy its schema. Carries every diagnostic on `.diagnostics`."
);

/// `InvalidValueError`, which is a `ValueError` as well as an `XsdError`.
///
/// Built at module initialisation with `type(...)`, because
/// `create_exception!` takes one base and `except ValueError` has to go on
/// catching what `Type.validate` raises.
static INVALID_VALUE_ERROR: PyOnceLock<Py<PyType>> = PyOnceLock::new();

/// An `InvalidValueError` carrying `message`.
fn invalid_value_error(py: Python<'_>, message: String) -> PyErr {
    match INVALID_VALUE_ERROR.get(py) {
        Some(ty) => PyErr::from_type(ty.bind(py).clone(), message),
        // Only before the module has initialised, which nothing can reach.
        None => PyValueError::new_err(message),
    }
}

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

/// Builds a `DocumentError` for a document, carrying the whole diagnostic list.
///
/// The same contract as [`schema_error`], for the same reason: a caller that
/// wants to show what was wrong with a document — filter by code, point at a
/// line — cannot do it with a formatted string.
fn document_error(py: Python<'_>, diags: &Diagnostics) -> PyErr {
    let err = DocumentError::new_err(format!("{diags}"));
    let wrapped: Vec<PyDiagnostic> = diags.iter().cloned().map(PyDiagnostic).collect();
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
///
/// The trait reports a failure as a string, which is right for a diagnostic
/// and wrong for control flow, so what the callable raised is kept as well.
/// Ctrl-C in a slow resolver used to become a diagnostic reading
/// `KeyboardInterrupt:` while the build carried on to the next import.
struct PyResolver {
    callable: Py<PyAny>,
    raised: Arc<Mutex<Raised>>,
}

/// What a Python resolver raised during one build.
#[derive(Default)]
struct Raised {
    /// The first ordinary exception, chained as the `SchemaError`'s
    /// `__cause__` so that its type and traceback survive.
    first: Option<PyErr>,
    /// A `KeyboardInterrupt`, `SystemExit` or anything else that is not an
    /// `Exception`. Once there is one, Python is not called again and the
    /// build ends by raising it.
    abort: Option<PyErr>,
}

fn lock(raised: &Mutex<Raised>) -> MutexGuard<'_, Raised> {
    raised.lock().unwrap_or_else(PoisonError::into_inner)
}

impl crate::load::Resolver for PyResolver {
    fn resolve(&self, location: &str, base: Option<&str>) -> Result<(String, Vec<u8>), String> {
        // Reacquires the GIL: `build()` released it, and this is Python code.
        Python::attach(|py| {
            if lock(&self.raised).abort.is_some() {
                return Err("the build was interrupted".to_string());
            }
            let out = match self.callable.call1(py, (location, base)) {
                Ok(out) => out,
                Err(e) => {
                    let message = e.to_string();
                    let mut raised = lock(&self.raised);
                    if e.is_instance_of::<PyException>(py) {
                        if raised.first.is_none() {
                            raised.first = Some(e);
                        }
                    } else {
                        raised.abort = Some(e);
                    }
                    return Err(message);
                }
            };
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

/// An instance document, read and decoded, with what diagnostics call it.
struct Instance {
    /// The text — or the diagnostic saying why the bytes could not be decoded,
    /// which makes an invalid document rather than a mistake by the caller.
    text: Result<String, Diagnostic>,
    /// The `uri` the caller gave, else the file it was read from, else a
    /// placeholder. The file, because a report on `orders/report.xml` that
    /// points at `<instance>:1` is least useful in exactly the case where the
    /// name is known.
    uri: String,
}

/// Reads an instance document given as `str`, as `bytes` or as a path.
///
/// Bytes are decoded the way a schema's are — byte-order mark, then the XML
/// declaration, then UTF-8 — so a document read with `open(path, "rb")` needs
/// no guess about its encoding, which is exactly the guess a caller is most
/// likely to get wrong.
fn read_instance(obj: &Bound<'_, PyAny>, uri: Option<&str>) -> PyResult<Instance> {
    let py = obj.py();
    let named = |fallback: &str| uri.unwrap_or(fallback).to_string();
    // `str` first, and always as content: a path is a `str` too, so the two
    // cannot be told apart here. What a bare name *does* produce is a clear
    // diagnostic — "document has no root element", with help saying so.
    if let Ok(text) = obj.extract::<String>() {
        return Ok(Instance {
            text: Ok(text),
            uri: named("<instance>"),
        });
    }
    if obj.is_instance_of::<PyBytes>() || obj.is_instance_of::<PyByteArray>() {
        let bytes: Vec<u8> = obj.extract()?;
        let uri = named("<instance>");
        let text = crate::encoding::decode_document(&bytes, &uri).map(|d| d.text);
        return Ok(Instance { text, uri });
    }
    // A `pathlib.Path`, on the other hand, is never ambiguous: nobody holds
    // one meaning "this is my XML". Read it, with the encoding detected from
    // the bytes exactly as for a document handed over directly.
    let path = match path_from(obj) {
        Ok(path) => path,
        Err(e) if e.is_instance_of::<PyTypeError>(py) => {
            return Err(PyTypeError::new_err(format!(
                "a document must be str, bytes or a path, not {}",
                obj.get_type().name()?
            )));
        }
        Err(e) => return Err(e),
    };
    let bytes = std::fs::read(&path).map_err(|e| os_error(py, &e, &path))?;
    let uri = named(&path);
    let text = crate::encoding::decode_document(&bytes, &uri).map(|d| d.text);
    Ok(Instance { text, uri })
}

/// The `OSError` Python raises for a file it cannot read — a
/// `FileNotFoundError` for a missing one — with the errno and the filename.
fn os_error(py: Python<'_>, e: &std::io::Error, path: &str) -> PyErr {
    let Some(errno) = e.raw_os_error() else {
        return PyOSError::new_err(format!("{path}: {e}"));
    };
    let message = py
        .import("os")
        .and_then(|os| os.call_method1("strerror", (errno,)))
        .and_then(|s| s.extract::<String>())
        .unwrap_or_else(|_| e.to_string());
    PyOSError::new_err((errno, message, path.to_string()))
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

/// How many events the worker behind `iter_typed` sends at a time.
///
/// One at a time, every event would cost a wake-up on each side and a trip
/// through the GIL; a batch keeps that off the per-event path, and still holds
/// only a few hundred events between the validator and the loop.
const EVENT_BATCH: usize = 256;

/// What the worker behind `iter_typed` sends: events, then the outcome.
enum Streamed {
    Events(Vec<RustPsvi>),
    Done(PyValidationReport),
}

/// Where an iteration stands.
enum Stream {
    /// Validation is running on its worker thread.
    Reading {
        batches: std::sync::mpsc::Receiver<Streamed>,
        worker: std::thread::JoinHandle<()>,
    },
    /// Another thread is waiting in `__next__` for the next batch.
    Busy,
    /// Every event has been handed out.
    Finished(PyValidationReport),
    /// The worker panicked, so there is no outcome to give.
    Failed(String),
}

struct Iteration {
    /// Events received and not yet handed out.
    pending: std::collections::VecDeque<RustPsvi>,
    stream: Stream,
}

/// An iterator over one document's typed events, read as it is validated.
///
/// Validation runs on a thread of its own, so memory stays flat however large
/// the document is, and an iterator dropped part way stops the reading. The
/// outcome is on `report` once every event has been read.
// The validator pushes events into a callback, and Python wants to pull them
// in a `for` loop, so validation runs on a worker thread and sends events over
// a bounded channel. Building every event first held 718 MB for a 26 MB
// document. When the iterator goes away the worker's next send fails and it
// stops reading, rather than validating the rest of the document for nobody.
#[pyclass(name = "PsviEvents", module = "xsdkit", frozen)]
pub struct PyPsviEvents {
    source: Arc<EventSource>,
    iteration: Mutex<Iteration>,
}

impl PyPsviEvents {
    /// An iteration with nothing to read, only an outcome.
    fn finished(source: Arc<EventSource>, report: PyValidationReport) -> Self {
        Self {
            source,
            iteration: Mutex::new(Iteration {
                pending: std::collections::VecDeque::new(),
                stream: Stream::Finished(report),
            }),
        }
    }

    fn state(&self) -> MutexGuard<'_, Iteration> {
        self.iteration
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// What a panicking worker said, as far as it can be read.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "validation panicked".to_string())
}

#[pymethods]
impl PyPsviEvents {
    fn __iter__(slf: Py<Self>) -> Py<Self> {
        slf
    }

    fn __next__<'py>(&self, py: Python<'py>) -> PyResult<Option<Bound<'py, PyPsviEvent>>> {
        loop {
            let (batches, worker) = {
                let mut state = self.state();
                if let Some(event) = state.pending.pop_front() {
                    drop(state);
                    return self.source.psvi_to_py(py, event).map(Some);
                }
                match std::mem::replace(&mut state.stream, Stream::Busy) {
                    Stream::Reading { batches, worker } => (batches, worker),
                    Stream::Busy => {
                        return Err(PyValueError::new_err(
                            "these events are already being read in another thread",
                        ));
                    }
                    done => {
                        state.stream = done;
                        return Ok(None);
                    }
                }
            };
            // The lock is not held while waiting: a second thread calling
            // `__next__` would block on it holding the GIL, which this thread
            // needs back to finish.
            let (batches, received) = py.detach(move || {
                let received = batches.recv();
                (batches, received)
            });
            let mut state = self.state();
            match received {
                Ok(Streamed::Events(events)) => {
                    state.pending.extend(events);
                    state.stream = Stream::Reading { batches, worker };
                }
                Ok(Streamed::Done(report)) => {
                    let _ = worker.join();
                    state.stream = Stream::Finished(report);
                    return Ok(None);
                }
                // The worker hung up without an outcome, which it only does by
                // panicking.
                Err(_) => {
                    let why = match worker.join() {
                        Err(payload) => panic_message(&*payload),
                        Ok(()) => "validation ended without an outcome".to_string(),
                    };
                    state.stream = Stream::Failed(why.clone());
                    return Err(pyo3::panic::PanicException::new_err(why));
                }
            }
        }
    }

    /// The outcome, once every event has been read.
    ///
    /// Whether a document is valid is only known at its end, which is where
    /// this is too. Raises `RuntimeError` before then; to know first, call
    /// `validate`.
    #[getter]
    fn report(&self) -> PyResult<PyValidationReport> {
        match &self.state().stream {
            Stream::Finished(report) => Ok(PyValidationReport {
                valid: report.valid,
                diagnostics: report.diagnostics.clone(),
            }),
            Stream::Failed(why) => Err(pyo3::exceptions::PyRuntimeError::new_err(format!(
                "validation did not finish: {why}"
            ))),
            _ => Err(pyo3::exceptions::PyRuntimeError::new_err(
                "the report is ready once every event has been read; to know whether a \
                 document is valid before reading it, call validate()",
            )),
        }
    }

    fn __repr__(&self) -> String {
        match &self.state().stream {
            Stream::Finished(r) if r.valid => "<PsviEvents finished, valid>".to_string(),
            Stream::Finished(_) => "<PsviEvents finished, invalid>".to_string(),
            Stream::Failed(_) => "<PsviEvents failed>".to_string(),
            _ => "<PsviEvents reading>".to_string(),
        }
    }
}

/// Walks the global names of a `SchemaSet`.
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

/// Which of a schema's symbol spaces a [`PyNamedComponents`] view holds.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ComponentKind {
    Element,
    Type,
    Attribute,
}

impl ComponentKind {
    const ALL: [ComponentKind; 3] = [Self::Element, Self::Type, Self::Attribute];

    fn article(self) -> &'static str {
        match self {
            Self::Element => "an element",
            Self::Type => "a type",
            Self::Attribute => "an attribute",
        }
    }

    /// Where a name of this kind is looked up, for an error that points there.
    fn lookup(self) -> &'static str {
        match self {
            Self::Element => "schemas[...]",
            Self::Type => "schemas.types[...]",
            Self::Attribute => "schemas.attributes[...]",
        }
    }
}

/// Whether a global name belongs to what every schema set carries — the XSD
/// built-in types, and the `xml:` and `xsi:` attributes — rather than to what
/// a document declared.
///
/// By namespace, not by `as_builtin`: that one answers from
/// `SimpleType::builtin` and so cannot see `xs:anyType`, which is complex.
fn predeclared(s: &Schemas, q: QName) -> bool {
    matches!(
        s.namespace_of(q),
        Some(crate::names::XS | crate::names::XSI | crate::names::XML)
    )
}

/// The global of `kind` that `name` names, when this schema declares one.
///
/// Anything that is not a name at all is simply not there, so that `42 in
/// schemas` is `False`, as it is for a `dict`.
fn lookup_global(s: &Schemas, kind: ComponentKind, name: &Bound<'_, PyAny>) -> Option<QName> {
    let q = parse_name(s, name).ok().flatten()?;
    let g = s.globals();
    let declared = match kind {
        ComponentKind::Element => g.elements.contains_key(&q),
        ComponentKind::Type => g.types.contains_key(&q) && !predeclared(s, q),
        ComponentKind::Attribute => g.attributes.contains_key(&q) && !predeclared(s, q),
    };
    declared.then_some(q)
}

/// A `KeyError` for a name `kind` does not have, which says so when another
/// kind does: an element and a type sharing a name is common, and the fix is
/// to look in the other place.
fn missing_global(s: &Schemas, kind: ComponentKind, name: &Bound<'_, PyAny>) -> PyErr {
    let shown = name
        .repr()
        .map(|r| r.to_string())
        .unwrap_or_else(|_| "that name".into());
    let other = ComponentKind::ALL
        .into_iter()
        .find(|k| *k != kind && lookup_global(s, *k, name).is_some());
    PyKeyError::new_err(match other {
        Some(k) => format!(
            "{shown} is not {} but {}; use {}",
            kind.article(),
            k.article(),
            k.lookup()
        ),
        None => format!("{shown} is not {} this schema declares", kind.article()),
    })
}

/// A schema's global components of one kind, in name order and by name.
///
/// Elements, types and attributes are separate symbol spaces, and an element
/// and a type sharing a name is one of the most common patterns in XSD, so
/// each kind has a view of its own rather than one mapping over all three. A
/// view iterates its components and indexes them by position the way a list
/// does, and looks them up by name the way a mapping does.
#[pyclass(module = "xsdkit", name = "NamedComponents", frozen)]
pub struct PyNamedComponents {
    s: Arc<Schemas>,
    kind: ComponentKind,
    /// Clark-notation names with the names they came from, sorted.
    entries: Vec<(String, QName)>,
}

impl PyNamedComponents {
    fn component<'py>(&self, py: Python<'py>, q: QName) -> PyResult<Option<Bound<'py, PyAny>>> {
        let g = self.s.globals();
        let s = self.s.clone();
        Ok(match self.kind {
            ComponentKind::Element => match g.elements.get(&q) {
                Some(&id) => Some(PyElement { s, id }.into_bound_py_any(py)?),
                None => None,
            },
            ComponentKind::Type => match g.types.get(&q) {
                Some(&id) => Some(PyType_ { s, id }.into_bound_py_any(py)?),
                None => None,
            },
            ComponentKind::Attribute => match g.attributes.get(&q) {
                Some(&id) => Some(PyAttribute { s, id }.into_bound_py_any(py)?),
                None => None,
            },
        })
    }
}

#[pymethods]
impl PyNamedComponents {
    fn __len__(&self) -> usize {
        self.entries.len()
    }

    /// The components, in name order.
    fn __iter__<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyIterator>> {
        self.values(py)?.try_iter()
    }

    /// By position, as in a list, or by name: `{ns}local` or `(ns, local)`.
    fn __getitem__<'py>(
        &self,
        py: Python<'py>,
        key: &Bound<'_, PyAny>,
    ) -> PyResult<Bound<'py, PyAny>> {
        if let Ok(i) = key.extract::<isize>() {
            let at = if i < 0 {
                i + self.entries.len() as isize
            } else {
                i
            };
            let found = usize::try_from(at)
                .ok()
                .and_then(|at| self.entries.get(at))
                .map(|(_, q)| *q);
            return match found {
                Some(q) => self
                    .component(py, q)?
                    .ok_or_else(|| PyIndexError::new_err("index out of range")),
                None => Err(PyIndexError::new_err(format!(
                    "{} index out of range",
                    self.kind.article()
                ))),
            };
        }
        let found = match lookup_global(&self.s, self.kind, key) {
            Some(q) => self.component(py, q)?,
            None => None,
        };
        found.ok_or_else(|| missing_global(&self.s, self.kind, key))
    }

    /// Whether a name, or a component of this kind, is in the view.
    fn __contains__(&self, item: &Bound<'_, PyAny>) -> bool {
        let g = self.s.globals();
        let ours = |s: &Arc<Schemas>| Arc::ptr_eq(s, &self.s);
        match self.kind {
            ComponentKind::Element => {
                if let Ok(e) = item.cast::<PyElement>() {
                    let e = e.get();
                    return ours(&e.s) && g.elements.get(&self.s[e.id].name) == Some(&e.id);
                }
            }
            ComponentKind::Type => {
                if let Ok(t) = item.cast::<PyType_>() {
                    let t = t.get();
                    return ours(&t.s)
                        && self.s[t.id].name().is_some_and(|n| {
                            g.types.get(&n) == Some(&t.id) && !predeclared(&self.s, n)
                        });
                }
            }
            ComponentKind::Attribute => {
                if let Ok(a) = item.cast::<PyAttribute>() {
                    let a = a.get();
                    let n = self.s[a.id].name;
                    return ours(&a.s)
                        && g.attributes.get(&n) == Some(&a.id)
                        && !predeclared(&self.s, n);
                }
            }
        }
        lookup_global(&self.s, self.kind, item).is_some()
    }

    /// The names, in Clark notation and in order.
    fn keys(&self) -> Vec<String> {
        self.entries.iter().map(|(n, _)| n.clone()).collect()
    }

    /// The components, in the same order as `keys`.
    fn values<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        let mut out = Vec::with_capacity(self.entries.len());
        for (_, q) in &self.entries {
            out.extend(self.component(py, *q)?);
        }
        PyList::new(py, out)
    }

    /// `(name, component)` pairs, in the same order as `keys`.
    fn items<'py>(&self, py: Python<'py>) -> PyResult<Vec<(String, Bound<'py, PyAny>)>> {
        let mut out = Vec::with_capacity(self.entries.len());
        for (n, q) in &self.entries {
            if let Some(c) = self.component(py, *q)? {
                out.push((n.clone(), c));
            }
        }
        Ok(out)
    }

    /// The component of that name, or `default` when there is none.
    #[pyo3(signature = (name, default=None))]
    fn get<'py>(
        &self,
        py: Python<'py>,
        name: &Bound<'_, PyAny>,
        default: Option<Bound<'py, PyAny>>,
    ) -> PyResult<Option<Bound<'py, PyAny>>> {
        match lookup_global(&self.s, self.kind, name) {
            Some(q) => self.component(py, q),
            None => Ok(default),
        }
    }

    /// The components as a list would show them.
    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        Ok(self.values(py)?.repr()?.to_string())
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
        let ty = obj
            .get_type()
            .name()
            .map(|n| n.to_string())
            .unwrap_or_default();
        PyTypeError::new_err(format!(
            "a name is '{{ns}}local', 'local' or (namespace, local), not {ty}"
        ))
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
#[pyclass(
    module = "xsdkit",
    name = "SchemaSet",
    frozen,
    skip_from_py_object,
    weakref
)]
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

/// What `SchemaSet.serialize` writes ahead of the xsdkit version and the
/// payload, so bytes from anywhere else are refused by name rather than
/// failing somewhere inside postcard.
const SERIAL_HEADER: &[u8] = b"xsdkit schema set\n";

/// What a `__reduce__` returns: what `pickle` calls to rebuild the object, and
/// the arguments it calls it with.
type Reduced<'py, Args> = PyResult<(Bound<'py, PyAny>, Args)>;

/// A `Diagnostic` as `pickle` carries it: code, severity, message, spans, help.
type DiagnosticFields = (
    &'static str,
    &'static str,
    String,
    Vec<PySpan>,
    Option<String>,
);

/// The prefix bindings a `namespaces=` argument gives: a mapping from prefix
/// to namespace URI, with `""` for the default namespace.
fn namespace_bindings(obj: Option<&Bound<'_, PyAny>>) -> PyResult<Vec<(Option<String>, String)>> {
    let Some(obj) = obj else {
        return Ok(Vec::new());
    };
    let items = obj.call_method0("items").map_err(|_| {
        let ty = obj
            .get_type()
            .name()
            .map(|n| n.to_string())
            .unwrap_or_default();
        PyTypeError::new_err(format!(
            "namespaces maps prefixes to namespace URIs, not {ty}"
        ))
    })?;
    let mut bindings = Vec::new();
    for item in items.try_iter()? {
        let (prefix, uri): (Option<String>, String) = item?.extract()?;
        bindings.push((prefix.filter(|p| !p.is_empty()), uri));
    }
    Ok(bindings)
}

/// The paths a list argument names, each a `str` or anything `os.fspath`
/// takes.
///
/// A single path is refused rather than iterated: a `str` is a sequence too,
/// and read as a list it would name each of its characters.
fn path_list(obj: &Bound<'_, PyAny>, argument: &str) -> PyResult<Vec<String>> {
    if obj.is_instance_of::<PyString>() || obj.is_instance_of::<PyBytes>() || path_from(obj).is_ok()
    {
        return Err(PyTypeError::new_err(format!(
            "{argument} takes a list of paths, not a single path"
        )));
    }
    obj.try_iter()?.map(|p| path_from(&p?)).collect()
}

/// The root documents `from_files` and `load_files` are given: at least one.
fn root_files(obj: &Bound<'_, PyAny>) -> PyResult<Vec<String>> {
    let files = path_list(obj, "paths")?;
    if files.is_empty() {
        return Err(PyValueError::new_err(
            "paths names no schema documents; give at least one",
        ));
    }
    Ok(files)
}

/// Assembles a builder from the keyword arguments the constructors share,
/// with the record of anything a Python resolver raises while it runs.
fn builder(
    search_paths: Option<Bound<'_, PyAny>>,
    conformance: &str,
    version: &str,
    nodes_limit: Option<u32>,
    max_depth: Option<u32>,
    resolver: Option<Py<PyAny>>,
) -> PyResult<(SchemaSetBuilder, Arc<Mutex<Raised>>)> {
    let raised = Arc::new(Mutex::new(Raised::default()));
    let mut b = SchemaSetBuilder::new()
        .conformance(conformance_from(conformance)?)
        .version(version_from(version)?);
    let search_paths = match search_paths {
        Some(paths) => Some(path_list(&paths, "search_paths")?),
        None => None,
    };
    // A custom resolver replaces the filesystem entirely, so the two are
    // alternatives rather than layers — a caller serving documents from a zip
    // has no search path to add them to — and passing both is refused rather
    // than quietly ignoring one of them.
    match (resolver, search_paths) {
        (Some(_), Some(_)) => {
            return Err(PyValueError::new_err(
                "pass a resolver or search_paths, not both: a resolver replaces the \
                 filesystem, so search_paths would never be read",
            ));
        }
        (Some(callable), None) => {
            b = b.resolver(PyResolver {
                callable,
                raised: raised.clone(),
            });
        }
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
    if let Some(limit) = max_depth {
        b = b.max_depth(limit);
    }
    Ok((b, raised))
}

/// Compiles with the GIL released, then raises whatever a resolver raised
/// that has to end the build.
///
/// Hands back the first ordinary exception a resolver raised, for the caller
/// to chain onto a `SchemaError`.
fn compile(
    py: Python<'_>,
    b: SchemaSetBuilder,
    raised: &Mutex<Raised>,
) -> PyResult<(Compilation, Option<PyErr>)> {
    // Compilation is the only slow part, and `Schemas` is Send + Sync
    // precisely so this is legal.
    let compilation = py.detach(|| b.compile());
    let raised = std::mem::take(&mut *lock(raised));
    match raised.abort {
        Some(abort) => Err(abort),
        None => Ok((compilation, raised.first)),
    }
}

/// The compiled set, or a `SchemaError` carrying every diagnostic — and, when
/// a resolver raised, that exception as its `__cause__`.
fn schema_set(py: Python<'_>, compiled: (Compilation, Option<PyErr>)) -> PyResult<PySchemaSet> {
    let (
        Compilation {
            schemas,
            diagnostics,
        },
        cause,
    ) = compiled;
    if diagnostics.has_errors() {
        let err = schema_error(py, diagnostics);
        err.set_cause(py, cause);
        return Err(err);
    }
    Ok(PySchemaSet::wrap(schemas))
}

impl PySchemaSet {
    /// This schema's global components of one kind, sorted by name.
    ///
    /// The XSD built-in types and the `xml:` and `xsi:` attributes live in the
    /// same tables — they have to, so that `type="xs:int"` resolves like any
    /// other reference — but they are not part of what a schema *says*, and
    /// every Python-facing enumeration leaves them out.
    fn declared(&self, kind: ComponentKind) -> Vec<(String, QName)> {
        let g = self.inner.globals();
        let names: Vec<QName> = match kind {
            ComponentKind::Element => g.elements.keys().copied().collect(),
            ComponentKind::Type => g.types.keys().copied().collect(),
            ComponentKind::Attribute => g.attributes.keys().copied().collect(),
        };
        let mut v: Vec<(String, QName)> = names
            .into_iter()
            .filter(|q| kind == ComponentKind::Element || !predeclared(&self.inner, *q))
            .map(|q| (clark(&self.inner, q), q))
            .collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    }

    fn view(&self, kind: ComponentKind) -> PyNamedComponents {
        PyNamedComponents {
            s: self.inner.clone(),
            kind,
            entries: self.declared(kind),
        }
    }
}

#[pymethods]
impl PySchemaSet {
    /// Loads a schema from a file, following its includes and imports.
    ///
    /// Raises `SchemaError`, carrying every diagnostic, when the schema has
    /// errors; `load` returns them instead. `version="1.1"` reads XSD 1.1: open
    /// content, conditional inclusion, assertions on wildcards,
    /// `xs:precisionDecimal` and the relaxed Unique Particle Attribution rule.
    /// `max_depth` caps how deeply elements nest in each schema document, 256 by
    /// default; a deeper document is refused with `XSD1001` rather than parsed.
    #[classmethod]
    #[pyo3(signature = (path, *, search_paths=None, conformance="strict", version="1.0", nodes_limit=None, max_depth=None, resolver=None))]
    fn from_file(
        _cls: &Bound<'_, PyType>,
        py: Python<'_>,
        path: &Bound<'_, PyAny>,
        search_paths: Option<Bound<'_, PyAny>>,
        conformance: &str,
        version: &str,
        nodes_limit: Option<u32>,
        max_depth: Option<u32>,
        resolver: Option<Py<PyAny>>,
    ) -> PyResult<Self> {
        let (b, raised) = builder(
            search_paths,
            conformance,
            version,
            nodes_limit,
            max_depth,
            resolver,
        )?;
        schema_set(py, compile(py, b.file(path_from(path)?), &raised)?)
    }

    /// Loads a schema from a string. The text must already be decoded.
    ///
    /// Relative `schemaLocation` hints resolve against `uri`; with the default
    /// `uri`, against the working directory and `search_paths`.
    #[classmethod]
    #[pyo3(signature = (xsd, *, uri="<string>", search_paths=None, conformance="strict", version="1.0", nodes_limit=None, max_depth=None, resolver=None))]
    fn from_string(
        _cls: &Bound<'_, PyType>,
        py: Python<'_>,
        xsd: String,
        uri: &str,
        search_paths: Option<Bound<'_, PyAny>>,
        conformance: &str,
        version: &str,
        nodes_limit: Option<u32>,
        max_depth: Option<u32>,
        resolver: Option<Py<PyAny>>,
    ) -> PyResult<Self> {
        let (b, raised) = builder(
            search_paths,
            conformance,
            version,
            nodes_limit,
            max_depth,
            resolver,
        )?;
        schema_set(py, compile(py, b.text(xsd, uri), &raised)?)
    }

    /// Loads a schema from raw bytes, detecting the encoding.
    ///
    /// Prefer this over `from_string` when the encoding is not known to be
    /// UTF-8: a byte-order mark or the XML declaration decides it.
    #[classmethod]
    #[pyo3(signature = (data, *, uri="<bytes>", search_paths=None, conformance="strict", version="1.0", nodes_limit=None, max_depth=None, resolver=None))]
    fn from_bytes(
        _cls: &Bound<'_, PyType>,
        py: Python<'_>,
        data: Vec<u8>,
        uri: &str,
        search_paths: Option<Bound<'_, PyAny>>,
        conformance: &str,
        version: &str,
        nodes_limit: Option<u32>,
        max_depth: Option<u32>,
        resolver: Option<Py<PyAny>>,
    ) -> PyResult<Self> {
        let (b, raised) = builder(
            search_paths,
            conformance,
            version,
            nodes_limit,
            max_depth,
            resolver,
        )?;
        schema_set(py, compile(py, b.bytes(data, uri), &raised)?)
    }

    /// Loads a schema from several files at once, following each one's
    /// includes and imports into one set.
    ///
    /// For a schema with no single root document: a vendor bundle, or a
    /// directory of XSDs that import one another. A single path, or an empty
    /// list, is refused.
    #[classmethod]
    #[pyo3(signature = (paths, *, search_paths=None, conformance="strict", version="1.0", nodes_limit=None, max_depth=None, resolver=None))]
    fn from_files(
        _cls: &Bound<'_, PyType>,
        py: Python<'_>,
        paths: &Bound<'_, PyAny>,
        search_paths: Option<Bound<'_, PyAny>>,
        conformance: &str,
        version: &str,
        nodes_limit: Option<u32>,
        max_depth: Option<u32>,
        resolver: Option<Py<PyAny>>,
    ) -> PyResult<Self> {
        let files = root_files(paths)?;
        let (mut b, raised) = builder(
            search_paths,
            conformance,
            version,
            nodes_limit,
            max_depth,
            resolver,
        )?;
        for file in files {
            b = b.file(file);
        }
        schema_set(py, compile(py, b, &raised)?)
    }

    /// The compiled schema set as bytes, for `deserialize` to read back
    /// without compiling the schema again.
    ///
    /// For a cache, and for handing a schema set to another process: pickling
    /// goes through this. Only the xsdkit version that wrote the bytes reads
    /// them back, so key a cache on `xsdkit.__version__`.
    fn serialize<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        let schemas = self.inner.clone();
        let payload = py
            .detach(move || postcard::to_allocvec(&*schemas))
            .map_err(|e| XsdError::new_err(format!("cannot serialize the schema set: {e}")))?;
        let version = env!("CARGO_PKG_VERSION").as_bytes();
        let mut out = Vec::with_capacity(SERIAL_HEADER.len() + version.len() + 1 + payload.len());
        out.extend_from_slice(SERIAL_HEADER);
        out.extend_from_slice(version);
        out.push(b'\n');
        out.extend_from_slice(&payload);
        Ok(PyBytes::new(py, &out))
    }

    /// Reads back what `serialize` wrote.
    ///
    /// Raises `ValueError` for bytes that are not a serialized schema set, or
    /// that another xsdkit version wrote: a name is an index into the
    /// interner, so a schema set from another build means nothing here. Read
    /// only bytes you trust, as with `pickle`.
    #[classmethod]
    fn deserialize(_cls: &Bound<'_, PyType>, py: Python<'_>, data: &[u8]) -> PyResult<Self> {
        let not_ours =
            || PyValueError::new_err("these bytes are not a schema set from SchemaSet.serialize()");
        let rest = data.strip_prefix(SERIAL_HEADER).ok_or_else(not_ours)?;
        let end = rest.iter().position(|&b| b == b'\n').ok_or_else(not_ours)?;
        let (version, payload) = (&rest[..end], &rest[end + 1..]);
        let ours = env!("CARGO_PKG_VERSION");
        if version != ours.as_bytes() {
            return Err(PyValueError::new_err(format!(
                "this schema set was serialized by xsdkit {}, and this is xsdkit {ours}: \
                 compile the schema again",
                String::from_utf8_lossy(version)
            )));
        }
        let schemas = py
            .detach(|| postcard::from_bytes::<Schemas>(payload))
            .map_err(|e| {
                PyValueError::new_err(format!("cannot read the serialized schema set: {e}"))
            })?;
        Ok(Self::wrap(schemas))
    }

    /// Pickles through `serialize`, so a schema set reaches a process pool's
    /// workers without being compiled again in each.
    fn __reduce__<'py>(slf: &Bound<'py, Self>) -> Reduced<'py, (Bound<'py, PyBytes>,)> {
        let bytes = slf.get().serialize(slf.py())?;
        Ok((slf.get_type().getattr("deserialize")?, (bytes,)))
    }

    /// A schema set cannot change, so a copy is the same object, the way it
    /// is for a `tuple`.
    fn __copy__(slf: Py<Self>) -> Py<Self> {
        slf
    }

    #[pyo3(signature = (_memo, /))]
    fn __deepcopy__(slf: Py<Self>, _memo: &Bound<'_, PyAny>) -> Py<Self> {
        slf
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
                    .map(|n| self.inner.namespace_uri(n).to_string()),
                chameleon: d.chameleon,
                version: d.version.clone(),
            })
            .collect()
    }

    /// How many global elements *this schema* declares.
    ///
    /// `SchemaSet` is a mapping from a global element's name to its
    /// declaration: `len`, `in`, `[]`, `get` and iteration all work, and
    /// iterating yields names, so `dict(s)` and `for name in s` read as they do
    /// for any other mapping. Types and attributes are separate symbol spaces
    /// — an element and a type often share a name — and have views of their
    /// own in `types` and `attributes`. `counts` is the component tally.
    fn __len__(&self) -> usize {
        self.inner.globals().elements.len()
    }

    fn __contains__(&self, name: &Bound<'_, PyAny>) -> bool {
        lookup_global(&self.inner, ComponentKind::Element, name).is_some()
    }

    /// The global element of that name, raising `KeyError` when there is none.
    ///
    /// The lookup methods return `None` instead, for when absence is an
    /// ordinary answer; this is for when it is a mistake. A name that belongs
    /// to a type or an attribute says so, and where to look instead.
    fn __getitem__(&self, name: &Bound<'_, PyAny>) -> PyResult<PyElement> {
        match lookup_global(&self.inner, ComponentKind::Element, name) {
            Some(q) => Ok(PyElement {
                s: self.inner.clone(),
                id: self.inner.globals().elements[&q],
            }),
            None => Err(missing_global(&self.inner, ComponentKind::Element, name)),
        }
    }

    /// The global element of that name, or `default` when there is none.
    #[pyo3(signature = (name, default=None))]
    fn get<'py>(
        &self,
        py: Python<'py>,
        name: &Bound<'_, PyAny>,
        default: Option<Bound<'py, PyAny>>,
    ) -> PyResult<Option<Bound<'py, PyAny>>> {
        self.view(ComponentKind::Element).get(py, name, default)
    }

    /// The global element names, sorted.
    ///
    /// Present so this really is a mapping: `dict(schemas)` needs `keys`
    /// alongside `__getitem__`.
    fn keys(&self) -> Vec<String> {
        self.view(ComponentKind::Element).keys()
    }

    /// The global elements, in the same order as `keys`.
    fn values<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        self.view(ComponentKind::Element).values(py)
    }

    /// `(name, element)` pairs, in the same order as `keys`.
    fn items<'py>(&self, py: Python<'py>) -> PyResult<Vec<(String, Bound<'py, PyAny>)>> {
        self.view(ComponentKind::Element).items(py)
    }

    /// The global element names, sorted.
    fn __iter__(&self) -> PyResult<Py<PyNameIter>> {
        let names = self.keys();
        Python::attach(|py| Py::new(py, PyNameIter { names, at: 0 }))
    }

    /// Every global element declaration, in name order and by name.
    ///
    /// A view: iterate it or index it by position, as a list, or look a
    /// declaration up by name, as a mapping — `schemas.elements["{ns}name"]`.
    #[getter]
    fn elements(&self) -> PyNamedComponents {
        self.view(ComponentKind::Element)
    }

    /// Every global type definition *this schema* declares, in name order and
    /// by name.
    ///
    /// An element and a type may share a name, which is why types have a view
    /// of their own. The XSD built-ins are excluded: they are in every schema
    /// set and would bury the ones the documents wrote. `type("{...}string")`
    /// still resolves them.
    #[getter]
    fn types(&self) -> PyNamedComponents {
        self.view(ComponentKind::Type)
    }

    /// Every global attribute declaration *this schema* declares, in name
    /// order and by name.
    ///
    /// The `xml:` and `xsi:` attributes every schema set carries are
    /// excluded; `attribute()` still resolves them.
    #[getter]
    fn attributes(&self) -> PyNamedComponents {
        self.view(ComponentKind::Attribute)
    }

    /// Looks up a global element. `None` if there is none.
    #[pyo3(signature = (namespace, local=None, /))]
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
    #[pyo3(name = "type", signature = (namespace, local=None, /))]
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
    #[pyo3(signature = (namespace, local=None, /))]
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
    ///
    /// Diagnostics name `uri`, or the file when the document was given as a
    /// path.
    #[pyo3(signature = (xml, *, uri=None))]
    fn validate(
        &self,
        py: Python<'_>,
        xml: &Bound<'_, PyAny>,
        uri: Option<&str>,
    ) -> PyResult<PyValidationReport> {
        let doc = read_instance(xml, uri)?;
        let text = match doc.text {
            Ok(text) => text,
            Err(d) => return Ok(PyValidationReport::undecodable(d)),
        };
        let schemas = self.inner.clone();
        // No Python is called back into, so the GIL can go.
        let report = py.detach(|| {
            schemas
                .document_validator()
                .validate_named(&text, &doc.uri, |_| {})
        });
        let valid = report.is_valid();
        Ok(PyValidationReport {
            valid,
            diagnostics: report.diagnostics.into_iter().map(PyDiagnostic).collect(),
        })
    }

    /// Decodes a document into Python data.
    ///
    /// Elements become dictionaries, values arrive in their value space —
    /// `Decimal`, `datetime`, `int` — and a child the schema allows more than
    /// once is always a list, whether the document carries two of them, one,
    /// or none. That shape comes from the schema, so it does not change under
    /// you when a document leaves something out.
    ///
    /// Keys are local names, spelled out in Clark notation only where two
    /// names under one parent would otherwise collide. Attributes are
    /// prefixed with `@`, and where an element has both a value and
    /// attributes the value sits under `$`. `xsi:nil` decodes to `None`, or
    /// to `None` under `$` when the element carries attributes too.
    ///
    /// Raises `DocumentError` if the document is invalid; pass `lax=True` to take
    /// the data anyway. Unlike `validate`, this one raises, because a caller
    /// asking for data has said what it wants and silently handing back data
    /// from a document that does not fit its schema is the trap this is meant
    /// to remove. Text that is not XML at all raises even with `lax=True`:
    /// there is nothing in it to take.
    ///
    /// The result is the root element's content. Pass `root=True` for
    /// `{root name: content}`, which says which global element the document
    /// was; the key follows the same rule as every other.
    #[pyo3(signature = (xml, *, uri=None, lax=false, root=false))]
    fn decode(
        &self,
        py: Python<'_>,
        xml: &Bound<'_, PyAny>,
        uri: Option<&str>,
        lax: bool,
        root: bool,
    ) -> PyResult<Py<PyAny>> {
        let doc = read_instance(xml, uri)?;
        let text = match doc.text {
            Ok(text) => text,
            // Bytes that cannot be decoded are not a document to take
            // anything from, lax or not.
            Err(d) => {
                let mut diagnostics = Diagnostics::new();
                diagnostics.push(d);
                return Err(document_error(py, &diagnostics));
            }
        };
        let schemas = self.inner.clone();
        // Nothing calls back into Python while the document is read.
        let mut decoding = py.detach(|| schemas.decode_named(&text, &doc.uri));
        let valid = decoding.is_valid();
        let tree = decoding.decoded.take();

        // `lax` takes what an invalid document says. Text that is not XML
        // says nothing, and `None` for it made a file name passed as a `str`
        // look like a document with no content.
        let malformed = decoding
            .diagnostics
            .iter()
            .any(|d| NOT_XML.contains(&d.code));
        let out = match &tree {
            Some(d) if valid || (lax && !malformed) => {
                decoded_to_py(py, &self.inner, d, &mut Shapes::default()).and_then(|value| {
                    if !root {
                        return Ok(value.unbind());
                    }
                    let keyed = PyDict::new(py);
                    keyed.set_item(root_key(&self.inner, d), value)?;
                    Ok(keyed.into_any().unbind())
                })
            }
            _ => Err(document_error(py, &decoding.diagnostics)),
        };
        // The tree's own drop glue recurses once per level, and overflows the
        // stack on a deep enough document whether or not it was converted.
        py.detach(|| dismantle(tree));
        out
    }

    /// Reads a document into typed PSVI events, as it is validated.
    ///
    /// `for ev in schemas.iter_typed(xml)` composes with everything Python has
    /// for iterables. Validation runs on a thread of its own and hands events
    /// over a batch at a time, so memory stays flat however large the
    /// document is, and an iterator dropped part way stops the reading.
    ///
    /// The outcome is on `report` once every event has been read. To know
    /// whether a document is valid before reading it, call `validate`.
    #[pyo3(signature = (xml, *, uri=None))]
    fn iter_typed(&self, xml: &Bound<'_, PyAny>, uri: Option<&str>) -> PyResult<PyPsviEvents> {
        let doc = read_instance(xml, uri)?;
        let source = Arc::new(EventSource {
            schemas: self.inner.clone(),
        });
        let text = match doc.text {
            Ok(text) => text,
            // Nothing to read: finished before it starts, with the diagnostic
            // saying why.
            Err(d) => {
                return Ok(PyPsviEvents::finished(
                    source,
                    PyValidationReport::undecodable(d),
                ));
            }
        };
        let inner = self.inner.clone();
        let uri = doc.uri;
        let (sender, batches) = std::sync::mpsc::sync_channel(1);
        let worker = std::thread::Builder::new()
            .name("xsdkit-iter-typed".to_string())
            .spawn(move || {
                let mut batch = Vec::with_capacity(EVENT_BATCH);
                let report = inner
                    .document_validator()
                    .validate_until(&text, &uri, |event| {
                        batch.push(event);
                        if batch.len() < EVENT_BATCH {
                            return std::ops::ControlFlow::Continue(());
                        }
                        let full = std::mem::replace(&mut batch, Vec::with_capacity(EVENT_BATCH));
                        // A send fails only once the iterator is gone, and
                        // then nobody is reading: stop.
                        match sender.send(Streamed::Events(full)) {
                            Ok(()) => std::ops::ControlFlow::Continue(()),
                            Err(_) => std::ops::ControlFlow::Break(()),
                        }
                    });
                if !batch.is_empty() && sender.send(Streamed::Events(batch)).is_err() {
                    return;
                }
                let valid = report.is_valid();
                let _ = sender.send(Streamed::Done(PyValidationReport {
                    valid,
                    diagnostics: report.diagnostics.into_iter().map(PyDiagnostic).collect(),
                }));
            })
            .map_err(|e| {
                PyOSError::new_err(format!("cannot start a thread to read the document: {e}"))
            })?;
        Ok(PyPsviEvents {
            source,
            iteration: Mutex::new(Iteration {
                pending: std::collections::VecDeque::new(),
                stream: Stream::Reading { batches, worker },
            }),
        })
    }

    /// Reads a document into typed PSVI events, all at once.
    ///
    /// Returns `(events, report)`: every event, as a list, and the outcome.
    /// Memory grows with the document; `iter_typed` reads one of any size.
    #[pyo3(signature = (xml, *, uri=None))]
    fn read_typed<'py>(
        &self,
        py: Python<'py>,
        xml: &Bound<'_, PyAny>,
        uri: Option<&str>,
    ) -> PyResult<(Vec<Bound<'py, PyPsviEvent>>, PyValidationReport)> {
        let doc = read_instance(xml, uri)?;
        let text = match doc.text {
            Ok(text) => text,
            Err(d) => return Ok((Vec::new(), PyValidationReport::undecodable(d))),
        };
        let source = Arc::new(EventSource {
            schemas: self.inner.clone(),
        });
        let mut events = Vec::new();
        let mut failed: Option<PyErr> = None;
        // The GIL is held throughout, because each event becomes a Python
        // object as it arrives: collecting the Rust events first would hold
        // the whole document's worth twice.
        let report = self
            .inner
            .document_validator()
            .validate_until(&text, &doc.uri, |event| {
                match source.psvi_to_py(py, event) {
                    Ok(obj) => {
                        events.push(obj);
                        std::ops::ControlFlow::Continue(())
                    }
                    Err(e) => {
                        failed = Some(e);
                        std::ops::ControlFlow::Break(())
                    }
                }
            });
        if let Some(e) = failed {
            return Err(e);
        }
        let valid = report.is_valid();
        Ok((
            events,
            PyValidationReport {
                valid,
                diagnostics: report.diagnostics.into_iter().map(PyDiagnostic).collect(),
            },
        ))
    }

    /// Component tallies — types, elements, particles and the rest — for
    /// diagnostics and smoke tests. Counts a great deal more than the globals
    /// `len()` reports.
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

    /// What is in this schema set, at a glance.
    ///
    /// The documents it was built from and the globals they declare, which is
    /// what you want to see first on opening a schema you did not write.
    #[allow(non_snake_case)]
    fn _repr_html_(&self) -> String {
        let elements = self.declared(ComponentKind::Element);
        let types = self.declared(ComponentKind::Type);
        let documents = self.documents();
        let plural = |n: usize| if n == 1 { "" } else { "s" };
        let mut out = format!(
            "<div style=\"{MONO}\"><b>SchemaSet</b> <span style=\"{FAINT}\">\
             {} document{}, {} element{}, {} type{}</span></div>",
            documents.len(),
            plural(documents.len()),
            elements.len(),
            plural(elements.len()),
            types.len(),
            plural(types.len()),
        );

        const LIMIT: usize = 25;
        let mut rows = String::new();
        for d in documents.iter().take(LIMIT) {
            rows.push_str(&format!(
                "<tr>{}{}</tr>",
                cell(MUTED, &esc(&d.uri)),
                cell(
                    NAME,
                    &esc(d.target_namespace.as_deref().unwrap_or("(no namespace)"))
                ),
            ));
        }
        out.push_str(&table(rows));

        // Names only. A schema with three hundred globals should not render
        // three hundred trees, and each one is a subscript away.
        if !elements.is_empty() {
            let shown: Vec<String> = elements
                .iter()
                .take(LIMIT)
                .map(|(_, q)| {
                    format!(
                        "<span style=\"{NAME}\">{}</span>",
                        esc(self.inner.local_of(*q))
                    )
                })
                .collect();
            let more = elements.len().saturating_sub(LIMIT);
            out.push_str(&format!(
                "<div style=\"{MONO};padding-top:.3em\"><span style=\"{FAINT}\">elements: \
                 </span>{}{}</div>",
                shown.join(&format!("<span style=\"{FAINT}\">, </span>")),
                if more > 0 {
                    format!("<span style=\"{FAINT}\"> … and {more} more</span>")
                } else {
                    String::new()
                }
            ));
        }
        out
    }

    /// What this schema declares, counted the way `len` and the views count
    /// it rather than with the built-ins every schema set carries.
    fn __repr__(&self) -> String {
        let n = |count: usize, noun: &str| {
            format!("{count} {noun}{}", if count == 1 { "" } else { "s" })
        };
        format!(
            "<SchemaSet {}, {}, {}>",
            n(self.inner.documents().len(), "document"),
            n(self.inner.globals().elements.len(), "element"),
            n(self.declared(ComponentKind::Type).len(), "type"),
        )
    }
}

/// The schema set behind one call's typed events.
///
/// An event holds this and ids rather than ready-made `Element` and `Type`
/// handles, and makes a handle when one is read. Every handle holds the set's
/// own `Arc`, so making them for every event wrote to that one count from every
/// thread reading a document: on free-threaded Python the count moved between
/// cores on each event, and eight threads read typed events no faster than
/// four. This `Arc` belongs to one call, and only the thread reading that
/// call's events writes to it.
struct EventSource {
    schemas: Arc<Schemas>,
}

impl EventSource {
    /// Turns one PSVI event into its Python wrapper.
    fn psvi_to_py<'py>(
        self: &Arc<Self>,
        py: Python<'py>,
        ev: RustPsvi,
    ) -> PyResult<Bound<'py, PyPsviEvent>> {
        let name_of = |q: crate::names::QName| {
            (
                self.schemas.namespace_of(q).map(str::to_string),
                self.schemas.local_of(q).to_string(),
            )
        };
        // A name the schema never interned has no symbols to look up, and
        // carries its own strings instead. Python sees the same pair either
        // way; before this it saw `(None, "")`.
        let psvi_name_of = |n: &crate::names::PsviName| match n {
            crate::names::PsviName::Known(q) => name_of(*q),
            crate::names::PsviName::Foreign { namespace, local } => {
                (namespace.clone(), local.clone())
            }
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
                        source: self.clone(),
                        name: psvi_name_of(&a.name),
                        declaration: a.declaration,
                        value,
                        lexical: a.lexical,
                        from_schema: a.from_schema,
                    });
                }
                PyPsviEvent {
                    source: self.clone(),
                    kind: "start",
                    name: Some(psvi_name_of(&name)),
                    declaration,
                    type_id: Some(type_id),
                    type_from_instance,
                    nil,
                    attributes: attrs,
                    value: None,
                    lexical: None,
                    from_schema: false,
                    line,
                }
            }
            RustPsvi::Text {
                value,
                type_id,
                lexical,
                from_schema,
                line,
            } => PyPsviEvent {
                source: self.clone(),
                kind: "text",
                name: None,
                declaration: None,
                type_id: Some(type_id),
                type_from_instance: false,
                nil: false,
                attributes: Vec::new(),
                value: match &value {
                    Some(v) => Some(value_to_py(py, v)?.unbind()),
                    None => None,
                },
                lexical: Some(lexical),
                from_schema,
                line,
            },
            RustPsvi::EndElement {
                name,
                declaration,
                line,
            } => PyPsviEvent {
                source: self.clone(),
                kind: "end",
                name: Some(psvi_name_of(&name)),
                declaration,
                type_id: None,
                type_from_instance: false,
                nil: false,
                attributes: Vec::new(),
                value: None,
                lexical: None,
                from_schema: false,
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
/// An element behaves as its children — iterable, sized, and subscriptable by
/// name — so a schema is walked without a `.type` hop at every level:
/// `report["item"]["price"]`, or `[child.local_name for child in report]`.
///
/// A handle into the schema, not a copy — holding ten thousand of them costs
/// ten thousand refcounts. Two handles to the same declaration compare equal
/// and hash alike, so they work as dict keys and set members.
///
///     >>> report = schemas.element("urn:example", "report")
///     >>> report.children               # what may appear inside
///     >>> report.substitutes            # what may appear *instead*
#[pyclass(module = "xsdkit", name = "Element", frozen, from_py_object)]
#[derive(Clone)]
pub struct PyElement {
    s: Arc<Schemas>,
    id: ElementId,
}

impl PyElement {
    /// The Rust view of this declaration.
    ///
    /// Python objects own their schema through an `Arc` — a PyO3 class
    /// cannot hold a borrow — so the reference is made per call rather than
    /// stored. It is two words and allocates nothing, and going through it
    /// is what keeps this binding a projection of the Rust API instead of a
    /// second implementation of it.
    fn r(&self) -> ElementRef<'_> {
        self.s.get(self.id)
    }
}

#[pymethods]
impl PyElement {
    /// `(namespace, local)`; the namespace is `None` when unqualified.
    #[getter]
    fn name(&self) -> (Option<String>, String) {
        let r = self.r();
        (
            r.namespace().map(str::to_string),
            r.local_name().to_string(),
        )
    }

    /// The name in Clark notation, `{ns}local`.
    #[getter]
    fn qname(&self) -> String {
        self.r().display_name()
    }

    /// The local part of the name, without its namespace.
    #[getter]
    fn local_name(&self) -> String {
        self.r().local_name().to_string()
    }

    /// The namespace URI, or `None` when the name is unqualified.
    #[getter]
    fn namespace(&self) -> Option<String> {
        self.r().namespace().map(str::to_string)
    }

    /// The type in force for this element.
    #[getter]
    fn r#type(&self) -> PyType_ {
        PyType_ {
            s: self.s.clone(),
            id: self.r().type_of().id(),
        }
    }

    /// Whether an instance may be empty by saying `xsi:nil="true"`.
    ///
    /// Nil is not the same as absent, and not the same as empty: it says the
    /// element is *present and has no value*.
    #[getter]
    fn nillable(&self) -> bool {
        self.r().is_nillable()
    }

    /// Whether this element may not appear itself.
    ///
    /// An abstract head exists to be substituted for — see `substitutes`.
    #[getter]
    fn r#abstract(&self) -> bool {
        self.r().is_abstract()
    }

    /// Whether this is a global declaration rather than one scoped to a type.
    #[getter]
    fn is_global(&self) -> bool {
        self.r().is_global()
    }

    /// Every element that may appear where this one is permitted, including
    /// itself when it is not abstract.
    ///
    /// Transitive, and with `block` applied — so this is what a document may
    /// actually name here, not merely who is in the substitution group. A
    /// head that blocks substitution has members that this does not list.
    #[getter]
    fn substitutes(&self) -> Vec<PyElement> {
        self.r()
            .substitutes()
            .map(|e| PyElement {
                s: self.s.clone(),
                id: e.id(),
            })
            .collect()
    }

    /// The `default` value, supplied when the element is present but empty.
    #[getter]
    fn default(&self) -> Option<String> {
        self.r().default().map(str::to_string)
    }

    /// The `fixed` value, which an instance may repeat but not contradict.
    #[getter]
    fn fixed(&self) -> Option<String> {
        self.r().fixed().map(str::to_string)
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

    /// Elements that may appear directly inside this one.
    ///
    /// The same as `element.type.children`, without the hop — an element's
    /// children are its type's, and browsing a schema should not have to say
    /// so at every level. Empty for a simple type.
    #[getter]
    fn children(&self) -> Vec<PyChild> {
        self.r#type().children()
    }

    /// The attributes this element may carry, with how it may carry them.
    ///
    /// The same as `element.type.attributes`, without the hop.
    #[getter]
    fn attributes(&self) -> Vec<PyAttributeUse> {
        self.r#type().attributes()
    }

    /// The child of that name, raising `KeyError` when there is none.
    ///
    /// Takes a local name as readily as a full one, because a child is almost
    /// always in its parent's namespace and `report["item"]["price"]` is what
    /// browsing a schema should look like. A Clark-notation or
    /// `(namespace, local)` name resolves exactly, for the case where it is
    /// not.
    fn __getitem__(&self, name: &Bound<'_, PyAny>) -> PyResult<PyChild> {
        pick_child(&self.s, self.children(), name, &self.qname())
    }

    /// Iterates the children, so `for child in element` reads.
    fn __iter__(&self) -> PyResult<Py<PyChildIter>> {
        Python::attach(|py| {
            Py::new(
                py,
                PyChildIter {
                    items: self.children(),
                    at: 0,
                },
            )
        })
    }

    /// How many children this element may have, by name.
    fn __len__(&self) -> usize {
        self.children().len()
    }

    /// Shows the tree in a notebook, where evaluating a value is how you look
    /// at it.
    ///
    /// Shallower than `tree()` on purpose: this fires on any cell that ends in
    /// an element, including by accident, so it shows the shape rather than
    /// the whole schema. Call `tree(depth=...)` for more.
    #[allow(non_snake_case)]
    fn _repr_html_(&self) -> String {
        self.tree(2)._repr_html_()
    }

    /// A readable tree of what may appear inside, for looking at a schema.
    ///
    /// Regular-expression markers for how often a child may appear — `?`
    /// optional, `+` one or more, `*` any number, nothing for exactly once —
    /// and `@name` for attributes, `?` when they are not required. Recursion
    /// stops where the shape repeats, so a section containing sections prints
    /// once rather than to the depth limit.
    ///
    ///     >>> print(schemas["{urn:example}report"].tree())
    ///     report
    ///       title: xs:string
    ///       item+
    ///         @sku
    ///         price: xs:decimal
    ///         note?: xs:string
    #[pyo3(signature = (depth=3))]
    fn tree(&self, depth: usize) -> PyTree {
        let (mut text, mut html) = (String::new(), String::new());
        let mut seen = Vec::new();
        self.write_tree(&mut text, &mut html, 0, depth, &mut seen, "");
        PyTree { text, html }
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

// ---------------------------------------------------------------------------
// Notebook rendering
// ---------------------------------------------------------------------------
//
// Everything below styles itself with JupyterLab's `--jp-*` CSS variables,
// each with a literal fallback. That is what makes a rendering readable in
// both themes without asking which one is on: `--jp-content-font-color1` is
// near-black in the light theme and white in the dark one. A hard-coded colour
// is unreadable in half of all notebooks, and the fallback covers the classic
// Notebook and VS Code, which define none of these.
//
// Styles are inline rather than in a `<style>` block: an output cell has no
// scope of its own, so a class name would leak into every other rendering on
// the page.

/// Escapes text for HTML. A namespace URI may hold an ampersand.
fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The monospaced frame every rendering sits in.
const MONO: &str = "font-family:var(--jp-code-font-family,ui-monospace,SFMono-Regular,Menlo,monospace);\
                    font-size:var(--jp-code-font-size,13px);line-height:1.5";
/// An element or attribute name — the thing being named.
const NAME: &str = "color:var(--jp-mirror-editor-def-color,#00f)";
/// An attribute name, distinguished from an element's.
const ATTR: &str = "color:var(--jp-mirror-editor-attribute-color,#00c)";
/// A type name: present, but not what the eye should land on.
const MUTED: &str = "color:var(--jp-content-font-color2,rgba(0,0,0,.54))";
/// Occurrence markers and other punctuation.
const FAINT: &str = "color:var(--jp-content-font-color3,rgba(0,0,0,.38))";

/// The colour a severity is shown in.
fn severity_color(severity: &str) -> &'static str {
    match severity {
        "error" => "var(--jp-error-color1,#d32f2f)",
        "warning" => "var(--jp-warn-color1,#f57c00)",
        _ => "var(--jp-info-color1,#1976d2)",
    }
}

/// A table, with the borders and padding every rendering here shares.
fn table(rows: String) -> String {
    format!("<table style=\"{MONO};border-collapse:collapse\"><tbody>{rows}</tbody></table>")
}

/// One cell of one.
fn cell(style: &str, content: &str) -> String {
    format!("<td style=\"padding:.1em .7em .1em 0;vertical-align:top;{style}\">{content}</td>")
}

/// Rendered text that knows how to show itself.
///
/// A plain `str` is the wrong return type for something meant to be *looked
/// at*: a notebook displays `repr()` of the last expression, and `repr` of a
/// string escapes every newline into `\n`. This renders as itself in a REPL,
/// in a notebook, and through `print`.
#[pyclass(module = "xsdkit", name = "Tree", frozen)]
pub struct PyTree {
    text: String,
    html: String,
}

#[pymethods]
impl PyTree {
    /// The text itself, so a notebook cell and a REPL both show the tree
    /// rather than an escaped one-liner.
    fn __repr__(&self) -> String {
        self.text.clone()
    }

    fn __str__(&self) -> String {
        self.text.clone()
    }

    /// Monospaced and with the whitespace kept, for Jupyter.
    ///
    /// The leading underscore is IPython's convention, not ours.
    #[allow(non_snake_case)]
    fn _repr_html_(&self) -> String {
        format!("<div style=\"{MONO}\">{}</div>", self.html)
    }

    /// The lines, so a tree can be sliced and searched like the text it is.
    fn splitlines(&self) -> Vec<String> {
        self.text.lines().map(str::to_string).collect()
    }

    fn __contains__(&self, needle: &str) -> bool {
        self.text.contains(needle)
    }

    /// How many times `needle` occurs, as `str.count` would say.
    #[pyo3(signature = (needle, /))]
    fn count(&self, needle: &str) -> usize {
        self.text.matches(needle).count()
    }

    /// The length in characters, matching what `str` would say.
    fn __len__(&self) -> usize {
        self.text.chars().count()
    }

    /// Equal to the string it renders as, so a test may compare against one.
    fn __eq__(&self, other: &Bound<'_, PyAny>) -> bool {
        match other.extract::<String>() {
            Ok(s) => s == self.text,
            Err(_) => false,
        }
    }

    /// Hashes as the string it equals, so the two are interchangeable as keys.
    fn __hash__(&self, py: Python<'_>) -> PyResult<isize> {
        PyString::new(py, &self.text).hash()
    }

    /// Joins as its text would, giving a plain `str`.
    fn __add__(&self, other: &str) -> String {
        format!("{}{other}", self.text)
    }

    fn __radd__(&self, other: &str) -> String {
        format!("{other}{}", self.text)
    }
}

/// `{ns}local` shortened to `local` for a built-in, left alone otherwise.
fn short(qname: &str) -> String {
    match qname.strip_prefix("{http://www.w3.org/2001/XMLSchema}") {
        Some(local) => format!("xs:{local}"),
        None => qname.to_string(),
    }
}

impl PyElement {
    /// One line per element, indented, stopping where the shape repeats.
    fn write_tree(
        &self,
        out: &mut String,
        html: &mut String,
        indent: usize,
        left: usize,
        seen: &mut Vec<ElementId>,
        // Supplied by the parent: how often a child may appear is a fact about
        // the pair, not about the declaration, which several parents may share
        // with different bounds.
        occurrence: &str,
    ) {
        use std::fmt::Write;
        let t = self.r#type();
        // Local names and occurrence, not qualified names and type names: at a
        // glance what matters is what may appear and how often, and almost
        // everything is in one namespace. A named type is worth saying; an
        // anonymous one has no name to say.
        let named = t
            .qname()
            .map(|n| format!(": {}", short(&n)))
            .unwrap_or_default();
        let _ = writeln!(
            out,
            "{}{}{}{}",
            "  ".repeat(indent),
            self.local_name(),
            occurrence,
            named
        );

        // The same line in HTML, coloured by role so that a name, its type and
        // how often it may appear are told apart at a glance.
        let label = format!(
            "<span style=\"{NAME}\">{}</span><span style=\"{FAINT}\">{}</span>\
             <span style=\"{MUTED}\">{}</span>",
            esc(&self.local_name()),
            esc(occurrence),
            esc(&named)
        );

        let attributes = t.attributes();
        let children = self.children();
        // A recursive schema — a section containing sections — would otherwise
        // print until the depth ran out, saying nothing new each time.
        let repeated = seen.contains(&self.id);
        let leaf = repeated || left == 0 || (attributes.is_empty() && children.is_empty());

        if leaf {
            // Padded to where a disclosure triangle puts its text, so leaves
            // and branches line up.
            let _ = write!(html, "<div style=\"padding-left:1.15em\">{label}");
            if repeated {
                let _ = write!(html, " <span style=\"{FAINT}\">…</span>");
            }
            let _ = writeln!(html, "</div>");
        } else {
            // Open near the root, closed deeper down: the top of a schema is
            // what you want to see, and a large one would otherwise arrive
            // fully expanded.
            let open = if indent < 2 { " open" } else { "" };
            let _ = writeln!(
                html,
                "<details{open}><summary style=\"cursor:pointer\">{label}</summary>\
                 <div style=\"padding-left:.6em;margin-left:.35em;\
                 border-left:1px solid var(--jp-border-color2,#e0e0e0)\">"
            );
        }

        if repeated {
            let _ = writeln!(out, "{}  ...", "  ".repeat(indent));
            return;
        }
        if leaf {
            return;
        }
        seen.push(self.id);
        for u in &attributes {
            let optional = if u.required() { "" } else { "?" };
            let _ = writeln!(
                out,
                "{}  @{}{}",
                "  ".repeat(indent),
                u.local_name(),
                optional
            );
            let _ = writeln!(
                html,
                "<div style=\"padding-left:1.15em\"><span style=\"{ATTR}\">@{}</span>\
                 <span style=\"{FAINT}\">{}</span></div>",
                esc(&u.local_name()),
                optional
            );
        }
        for c in &children {
            c.element
                .write_tree(out, html, indent + 1, left - 1, seen, occurrence_of(c));
        }
        seen.pop();
        let _ = writeln!(html, "</div></details>");
    }
}

/// The regex-style marker for how often a child may appear.
fn occurrence_of(c: &PyChild) -> &'static str {
    match (c.repeats, c.optional) {
        (true, true) => "*",
        (true, false) => "+",
        (false, true) => "?",
        (false, false) => "",
    }
}

/// Resolves a name against a type's children, accepting a local name as
/// readily as a full one.
///
/// Exact first: a local name that happens to look like a Clark-notation one
/// should not be second-guessed. When no child has the name, the parsed name
/// comes back anyway, for a wildcard to match.
fn child_qname(
    s: &Schemas,
    children: &[PyChild],
    name: &Bound<'_, PyAny>,
) -> PyResult<Option<QName>> {
    let exact = parse_name(s, name)?;
    if let Some(q) = exact {
        if children.iter().any(|c| s[c.element.id].name == q) {
            return Ok(Some(q));
        }
    }
    if let Ok(local) = name.extract::<String>() {
        if let Some(c) = children.iter().find(|c| c.element.local_name() == local) {
            return Ok(Some(s[c.element.id].name));
        }
    }
    Ok(exact)
}

/// Finds the named child, the way [`child_qname`] resolves the name.
fn pick_child(
    s: &Schemas,
    children: Vec<PyChild>,
    name: &Bound<'_, PyAny>,
    owner: &str,
) -> PyResult<PyChild> {
    if let Ok(Some(q)) = child_qname(s, &children, name) {
        if let Some(c) = children.iter().find(|c| s[c.element.id].name == q) {
            return Ok(c.clone());
        }
    }
    Err(PyKeyError::new_err(format!(
        "{owner} has no child {}",
        name.repr()?
    )))
}

/// An element as a child of one particular type.
///
/// Everything `Element` answers, this answers too, plus how often it may
/// appear *here*. That pairing is the point: `maxOccurs` and `minOccurs` are
/// written on the use, not on the declaration, so one global element may be
/// a repeating child of one type and a required single child of another.
///
/// Both flags come from the same pass over the content model that produced
/// the child list, so reading them costs nothing beyond the walk that was
/// already done.
#[pyclass(module = "xsdkit", name = "Child", frozen, from_py_object)]
#[derive(Clone)]
pub struct PyChild {
    element: PyElement,
    repeats: bool,
    optional: bool,
}

#[pymethods]
impl PyChild {
    /// Whether it may appear here more than once, which makes it a list when
    /// a document is decoded.
    #[getter]
    fn repeats(&self) -> bool {
        self.repeats
    }

    /// Whether some valid content leaves it out.
    #[getter]
    fn optional(&self) -> bool {
        self.optional
    }

    /// The declaration on its own, without this parent's occurrence.
    ///
    /// Rarely needed — a `Child` answers everything an `Element` does — but
    /// it is what to compare against `SchemaSet["{ns}name"]`, which has no
    /// parent to have occurrence in.
    #[getter]
    fn element(&self) -> PyElement {
        self.element.clone()
    }

    /// `(namespace, local)`; the namespace is `None` when unqualified.
    #[getter]
    fn name(&self) -> (Option<String>, String) {
        self.element.name()
    }

    /// The name in Clark notation, `{ns}local`.
    #[getter]
    fn qname(&self) -> String {
        self.element.qname()
    }

    /// The local part of the name, without its namespace.
    #[getter]
    fn local_name(&self) -> String {
        self.element.local_name()
    }

    /// The namespace URI, or `None` when the name is unqualified.
    #[getter]
    fn namespace(&self) -> Option<String> {
        self.element.namespace()
    }

    /// The type in force for this element.
    #[getter]
    fn r#type(&self) -> PyType_ {
        self.element.r#type()
    }

    /// Whether an instance may say `xsi:nil="true"` here.
    #[getter]
    fn nillable(&self) -> bool {
        self.element.nillable()
    }

    /// Whether this element may not appear itself, only a substitute.
    #[getter]
    fn r#abstract(&self) -> bool {
        self.element.r#abstract()
    }

    /// Whether the declaration is global rather than scoped to a type.
    #[getter]
    fn is_global(&self) -> bool {
        self.element.is_global()
    }

    /// Every element that may stand in for this one. Transitive.
    #[getter]
    fn substitutes(&self) -> Vec<PyElement> {
        self.element.substitutes()
    }

    /// The `default` value, supplied when the element is present but empty.
    #[getter]
    fn default(&self) -> Option<String> {
        self.element.default()
    }

    /// The `fixed` value, which an instance may repeat but not contradict.
    #[getter]
    fn fixed(&self) -> Option<String> {
        self.element.fixed()
    }

    /// The `xs:documentation` text, entries joined.
    #[getter]
    fn doc(&self) -> Option<String> {
        self.element.doc()
    }

    /// The `xs:appinfo` blocks, with their XML kept verbatim.
    #[getter]
    fn appinfo(&self) -> Vec<PyAppInfo> {
        self.element.appinfo()
    }

    /// The elements that may appear inside this child, in turn.
    #[getter]
    fn children(&self) -> Vec<PyChild> {
        self.element.children()
    }

    /// The attributes this child may carry, with how it may carry them.
    #[getter]
    fn attributes(&self) -> Vec<PyAttributeUse> {
        self.element.attributes()
    }

    fn __getitem__(&self, name: &Bound<'_, PyAny>) -> PyResult<PyChild> {
        self.element.__getitem__(name)
    }

    fn __iter__(&self) -> PyResult<Py<PyChildIter>> {
        self.element.__iter__()
    }

    fn __len__(&self) -> usize {
        self.element.__len__()
    }

    /// The shape below this child, `depth` levels deep.
    #[pyo3(signature = (depth = 3))]
    fn tree(&self, depth: usize) -> PyTree {
        self.element.tree(depth)
    }

    /// The element's tree, as a notebook shows it.
    #[allow(non_snake_case)]
    fn _repr_html_(&self) -> String {
        self.element._repr_html_()
    }

    fn __repr__(&self) -> String {
        format!(
            "<Child {}{}{}>",
            self.element.local_name(),
            if self.repeats { "+" } else { "" },
            if self.optional { "?" } else { "" },
        )
    }

    /// Equal when it is the same declaration used the same way.
    ///
    /// A `Child` is deliberately not equal to the bare `Element` it wraps:
    /// they answer different questions, and one of them knows where it is.
    fn __eq__(&self, other: &PyChild) -> bool {
        self.element.__eq__(&other.element)
            && self.repeats == other.repeats
            && self.optional == other.optional
    }

    fn __hash__(&self) -> u64 {
        self.element.__hash__()
    }
}

/// Walks a type's children.
#[pyclass(name = "ChildIterator", module = "xsdkit")]
pub struct PyChildIter {
    items: Vec<PyChild>,
    at: usize,
}

#[pymethods]
impl PyChildIter {
    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __next__(mut slf: PyRefMut<'_, Self>) -> Option<PyChild> {
        let item = slf.items.get(slf.at).cloned();
        slf.at += 1;
        item
    }

    /// How many children are left, as for the other iterators here.
    fn __len__(&self) -> usize {
        self.items.len().saturating_sub(self.at)
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

impl PyAttribute {
    /// The Rust view of this declaration. See [`PyElement::r`].
    fn r(&self) -> AttributeRef<'_> {
        self.s.get(self.id)
    }
}

#[pymethods]
impl PyAttribute {
    /// The name as a `(namespace, local)` pair.
    #[getter]
    fn name(&self) -> (Option<String>, String) {
        let r = self.r();
        (
            r.namespace().map(str::to_string),
            r.local_name().to_string(),
        )
    }

    /// The name in Clark notation, `{namespace}local`.
    #[getter]
    fn qname(&self) -> String {
        self.r().display_name()
    }

    /// The local part of the name, without its namespace.
    #[getter]
    fn local_name(&self) -> String {
        self.r().local_name().to_string()
    }

    /// The simple type of this attribute's value.
    #[getter]
    fn r#type(&self) -> PyType_ {
        PyType_ {
            s: self.s.clone(),
            id: self.r().type_of().id(),
        }
    }

    /// The `default` value the schema supplies when the attribute is absent.
    #[getter]
    fn default(&self) -> Option<String> {
        self.r().default().map(str::to_string)
    }

    /// A schema-declared constant value — the case that can be resolved
    /// without seeing an instance document.
    #[getter]
    fn fixed(&self) -> Option<String> {
        self.r().fixed().map(str::to_string)
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
///     >>> t = schemas.type("urn:example", "Sku")
///     >>> t.validate("AB-1042")         # the typed value, or ValueError
///     >>> t.facets.patterns             # the constraints in force
#[pyclass(module = "xsdkit", name = "Type", frozen, from_py_object)]
#[derive(Clone)]
pub struct PyType_ {
    s: Arc<Schemas>,
    id: TypeId,
}

impl PyType_ {
    /// The Rust view of this definition. See [`PyElement::r`].
    fn r(&self) -> TypeRef<'_> {
        self.s.get(self.id)
    }
}

impl PyType_ {
    /// The type a value of this one is checked against: itself, or for a
    /// complex type with simple content, the simple type of that content.
    ///
    /// Without this a price with a currency had no value space at all, and
    /// the type its value is checked against was reachable only through
    /// `.base`, and not even there once a restriction sat in between.
    fn value_type(&self) -> TypeId {
        match &self.s[self.id] {
            TypeDefinition::Complex(t) => match t.content {
                crate::model::ContentType::Simple(simple) if !simple.is_placeholder() => simple,
                _ => self.id,
            },
            TypeDefinition::Simple(_) => self.id,
        }
    }
}

#[pymethods]
impl PyType_ {
    /// `(namespace, local)`, or `None` for an anonymous inline type.
    #[getter]
    fn name(&self) -> Option<(Option<String>, String)> {
        let r = self.r();
        r.local_name()
            .map(|local| (r.namespace().map(str::to_string), local.to_string()))
    }

    /// The name in Clark notation, or `None` for an anonymous type.
    ///
    /// A type declared inline inside an element has no name to report.
    #[getter]
    fn qname(&self) -> Option<String> {
        self.r().name().map(|q| clark(&self.s, q))
    }

    /// Whether this type may have attributes and child elements.
    #[getter]
    fn is_complex(&self) -> bool {
        self.r().is_complex()
    }

    /// Whether this type has a value space — something `validate` can parse.
    #[getter]
    fn is_simple(&self) -> bool {
        self.r().is_simple()
    }

    /// Whether an instance may not use this type directly, only one derived from it.
    #[getter]
    fn r#abstract(&self) -> bool {
        self.s[self.id].as_complex().is_some_and(|c| c.is_abstract)
    }

    /// The type this one derives from, or `None` at `xs:anyType`.
    #[getter]
    fn base(&self) -> Option<PyType_> {
        self.r().base().map(|b| PyType_ {
            s: self.s.clone(),
            id: b.id(),
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
    ///
    /// Always `False` for a type from another `SchemaSet`: a type in one set says
    /// nothing about a type in another.
    // A handle is an index into one set's arenas, and the same index in another
    // set is an unrelated type; comparing them reported derivations that do not
    // exist.
    #[pyo3(signature = (other, /))]
    fn derives_from(&self, other: &PyType_) -> bool {
        Arc::ptr_eq(&self.s, &other.s) && self.s.derives_from(self.id, other.id)
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
    ///
    /// Each one is a `Child`: the declaration, plus whether it may repeat and
    /// whether it may be left out. Those two belong to the pair rather than
    /// to the declaration — one global element may be used by several types
    /// under different bounds — and they come from the same single pass over
    /// the content model that found the children.
    #[getter]
    fn children(&self) -> Vec<PyChild> {
        self.r()
            .children()
            .map(|c| PyChild {
                element: PyElement {
                    s: self.s.clone(),
                    id: c.id(),
                },
                repeats: c.repeats(),
                optional: c.optional(),
            })
            .collect()
    }

    /// The child of that name, raising `KeyError` when there is none.
    fn __getitem__(&self, name: &Bound<'_, PyAny>) -> PyResult<PyChild> {
        let owner = self.qname().unwrap_or_else(|| "(anonymous)".into());
        pick_child(&self.s, self.children(), name, &owner)
    }

    /// Iterates the children, so `for child in type` reads.
    fn __iter__(&self) -> PyResult<Py<PyChildIter>> {
        Python::attach(|py| {
            Py::new(
                py,
                PyChildIter {
                    items: self.children(),
                    at: 0,
                },
            )
        })
    }

    /// How many children this type may have, by name.
    fn __len__(&self) -> usize {
        self.children().len()
    }

    /// `"empty"`, `"simple"`, `"element-only"` or `"mixed"`; `None` for a
    /// simple type.
    #[getter]
    fn content(&self) -> Option<&'static str> {
        self.r().definition().as_complex().map(|c| match c.content {
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
    /// Names may be Clark notation, `(ns, local)` pairs, or bare local names,
    /// resolved against this type's children exactly as `type[name]` resolves
    /// them. A single `str` is refused rather than read one character at a
    /// time.
    #[pyo3(signature = (names, /))]
    fn accepts(&self, names: &Bound<'_, PyAny>) -> PyResult<bool> {
        if names.is_instance_of::<PyString>() || names.is_instance_of::<PyBytes>() {
            return Err(PyTypeError::new_err(
                "accepts() takes a sequence of names, not a single string",
            ));
        }
        let children = self.children();
        let mut wanted = Vec::new();
        for item in names.try_iter()? {
            let Some(q) = child_qname(&self.s, &children, &item?)? else {
                // A name no component in this schema carries cannot match.
                return Ok(false);
            };
            wanted.push(q);
        }
        Ok(self.r().accepts(wanted))
    }

    /// Validates a lexical form against this type, returning its typed value.
    ///
    /// `namespaces` maps prefixes to namespace URIs, `""` for the default
    /// namespace, for an `xs:QName`, whose value is whatever its prefix is
    /// bound to where it was written. A complex type with simple content
    /// validates against the simple type of that content.
    ///
    /// Raises `InvalidValueError`, which is also a `ValueError`, with the
    /// reason when the value is not valid.
    #[pyo3(signature = (lexical, /, *, namespaces=None))]
    fn validate(
        &self,
        py: Python<'_>,
        lexical: &str,
        namespaces: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Py<PyAny>> {
        let bindings = namespace_bindings(namespaces)?;
        let validator = self.s.value_validator();
        match validator.validate_with(self.value_type(), lexical, &bindings) {
            Ok(v) => Ok(value_to_py(py, &v)?.unbind()),
            Err(e) => Err(invalid_value_error(py, e.to_string())),
        }
    }

    /// Whether a lexical form is valid against this type.
    #[pyo3(signature = (lexical, /, *, namespaces=None))]
    fn is_valid(&self, lexical: &str, namespaces: Option<&Bound<'_, PyAny>>) -> PyResult<bool> {
        let bindings = namespace_bindings(namespaces)?;
        Ok(self
            .s
            .value_validator()
            .validate_with(self.value_type(), lexical, &bindings)
            .is_ok())
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
    /// schema back wants. `facets` is what a validator applies.
    #[getter]
    fn declared_facets(&self) -> Option<PyFacets> {
        self.s[self.id]
            .as_simple()
            .map(|t| PyFacets(t.facets.clone()))
    }

    /// The facets in force, composed down the whole restriction chain.
    ///
    /// Not the ones this type declares — those are on
    /// `declared_facets`. A restriction inherits everything its base
    /// constrained, so a type that says only `maxLength` still has its base's
    /// `minLength`, and reporting the declared set alone disagrees with what
    /// `validate` does. For a complex type with simple content, the facets of
    /// that content's simple type.
    #[getter]
    fn facets(&self) -> Option<PyFacets> {
        let ty = self.value_type();
        self.s[ty]
            .as_simple()
            .map(|_| PyFacets(crate::validate::effective_facets(&self.s, ty)))
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
    /// where a schema hides labels, mappings and anything else its authors agreed
    /// on.
    #[getter]
    fn appinfo(&self) -> Vec<PyAppInfo> {
        let ann = match &self.s[self.id] {
            TypeDefinition::Simple(t) => t.annotation,
            TypeDefinition::Complex(t) => t.annotation,
        };
        annotation_appinfo(&self.s, ann)
    }

    /// A readable tree of what may appear inside this type.
    ///
    /// The same rendering as `Element.tree`, rooted at the type rather than at
    /// a declaration — so the first line is the type's name and the rest is
    /// its content.
    #[pyo3(signature = (depth=3))]
    fn tree(&self, depth: usize) -> PyTree {
        use std::fmt::Write;
        let (mut text, mut html) = (String::new(), String::new());
        let name = self
            .qname()
            .map(|n| short(&n))
            .unwrap_or_else(|| "(anonymous)".into());
        let _ = writeln!(text, "{name}");
        let _ = writeln!(html, "<div style=\"{NAME}\">{}</div>", esc(&name));
        for u in self.attributes() {
            let optional = if u.required() { "" } else { "?" };
            let _ = writeln!(text, "  @{}{}", u.local_name(), optional);
            let _ = writeln!(
                html,
                "<div style=\"padding-left:1.15em\"><span style=\"{ATTR}\">@{}</span>\
                 <span style=\"{FAINT}\">{}</span></div>",
                esc(&u.local_name()),
                optional
            );
        }
        let mut seen = Vec::new();
        for c in self.children() {
            c.element
                .write_tree(&mut text, &mut html, 1, depth, &mut seen, occurrence_of(&c));
        }
        PyTree { text, html }
    }

    /// Shows the tree in a notebook. Shallower than `tree()`, as on `Element`.
    #[allow(non_snake_case)]
    fn _repr_html_(&self) -> String {
        self.tree(2)._repr_html_()
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

    /// The constraints in force, as a table of the ones that are set.
    #[allow(non_snake_case)]
    fn _repr_html_(&self) -> String {
        let mut rows = String::new();
        let mut row = |name: &str, value: String| {
            rows.push_str(&format!(
                "<tr>{}{}</tr>",
                cell(MUTED, &esc(name)),
                cell(NAME, &esc(&value))
            ));
        };
        if let Some(v) = self.length() {
            row("length", v.to_string());
        }
        if let Some(v) = self.min_length() {
            row("minLength", v.to_string());
        }
        if let Some(v) = self.max_length() {
            row("maxLength", v.to_string());
        }
        for (name, value) in [
            ("minInclusive", self.min_inclusive()),
            ("minExclusive", self.min_exclusive()),
            ("maxInclusive", self.max_inclusive()),
            ("maxExclusive", self.max_exclusive()),
            ("whiteSpace", self.white_space()),
        ] {
            if let Some(v) = value {
                row(name, v);
            }
        }
        if let Some(v) = self.total_digits() {
            row("totalDigits", v.to_string());
        }
        if let Some(v) = self.fraction_digits() {
            row("fractionDigits", v.to_string());
        }
        if let Some(values) = self.enumeration() {
            row("enumeration", values.join(" | "));
        }
        // Patterns are ANDed across steps and ORed within one, which a flat
        // list would misrepresent.
        for (i, step) in self.patterns().iter().enumerate() {
            row(
                &if i == 0 {
                    "pattern".to_string()
                } else {
                    format!("pattern ({})", i + 1)
                },
                step.join(" | "),
            );
        }
        if rows.is_empty() {
            return format!("<div style=\"{MONO};{FAINT}\">no facets</div>");
        }
        table(rows)
    }

    /// Every facet that is set, by its Python name.
    fn __repr__(&self) -> String {
        let f = &self.0;
        let mut parts: Vec<String> = [
            ("length", f.length.map(|v| v.to_string())),
            ("min_length", f.min_length.map(|v| v.to_string())),
            ("max_length", f.max_length.map(|v| v.to_string())),
            ("min_inclusive", f.min_inclusive.clone()),
            ("min_exclusive", f.min_exclusive.clone()),
            ("max_inclusive", f.max_inclusive.clone()),
            ("max_exclusive", f.max_exclusive.clone()),
            ("total_digits", f.total_digits.map(|v| v.to_string())),
            ("fraction_digits", f.fraction_digits.map(|v| v.to_string())),
            ("white_space", f.white_space.map(|w| w.to_string())),
        ]
        .into_iter()
        .filter_map(|(name, value)| value.map(|v| format!("{name}={v}")))
        .collect();
        if let Some(e) = &f.enumeration {
            parts.push(format!("enumeration={} value(s)", e.len()));
        }
        if !f.patterns.is_empty() {
            parts.push(format!("patterns={} step(s)", f.patterns.len()));
        }
        if parts.is_empty() {
            "<Facets (none)>".to_string()
        } else {
            format!("<Facets {}>", parts.join(" "))
        }
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

    /// Pickles as its fields.
    fn __reduce__<'py>(slf: &Bound<'py, Self>) -> Reduced<'py, (String, u32, Option<String>)> {
        let s = slf.get();
        Ok((
            slf.get_type().getattr("_rebuild")?,
            (s.uri.clone(), s.line, s.label.clone()),
        ))
    }

    /// What `__reduce__` hands `pickle` to call.
    #[classmethod]
    fn _rebuild(_cls: &Bound<'_, PyType>, uri: String, line: u32, label: Option<String>) -> Self {
        Self { uri, line, label }
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

    /// Pickles as its fields, so a `SchemaError` raised in a process pool's
    /// worker arrives whole instead of as a failure to pickle it.
    fn __reduce__<'py>(slf: &Bound<'py, Self>) -> Reduced<'py, DiagnosticFields> {
        let d = slf.get();
        Ok((
            slf.get_type().getattr("_rebuild")?,
            (d.code(), d.severity(), d.message(), d.spans(), d.help()),
        ))
    }

    /// What `__reduce__` hands `pickle` to call.
    #[classmethod]
    fn _rebuild(
        _cls: &Bound<'_, PyType>,
        code: &str,
        severity: &str,
        message: String,
        spans: Vec<Bound<'_, PyAny>>,
        help: Option<String>,
    ) -> PyResult<Self> {
        let code = crate::diagnostics::DiagCode::ALL
            .into_iter()
            .find(|c| c.as_str() == code)
            .ok_or_else(|| PyValueError::new_err(format!("no diagnostic has the code {code:?}")))?;
        let severity = match severity {
            "error" => Severity::Error,
            "warning" => Severity::Warning,
            "note" => Severity::Note,
            other => return Err(PyValueError::new_err(format!("no severity {other:?}"))),
        };
        let spans = spans
            .iter()
            .map(|s| {
                let s = s.cast::<PySpan>()?.get();
                Ok(Span {
                    uri: s.uri.clone(),
                    line: s.line,
                    label: s.label.clone(),
                })
            })
            .collect::<PyResult<Vec<Span>>>()?;
        Ok(Self(Diagnostic {
            code,
            severity,
            message,
            spans,
            help,
        }))
    }

    fn __str__(&self) -> String {
        self.0.to_string()
    }

    /// The diagnostic as a compiler would print it, coloured by severity.
    #[allow(non_snake_case)]
    fn _repr_html_(&self) -> String {
        let severity = self.severity();
        let mut out = format!(
            "<div style=\"{MONO}\"><span style=\"color:{};font-weight:600\">{}</span> \
             <span style=\"{FAINT}\">[{}]</span> {}",
            severity_color(severity),
            esc(severity),
            esc(self.code()),
            esc(&self.message())
        );
        for span in self.spans() {
            out.push_str(&format!(
                "<div style=\"padding-left:1.2em;{MUTED}\">→ {}{}</div>",
                esc(&span.__str__()),
                span.label
                    .as_deref()
                    .map(|l| format!("  <em>{}</em>", esc(l)))
                    .unwrap_or_default()
            ));
        }
        if let Some(help) = self.help() {
            out.push_str(&format!(
                "<div style=\"padding-left:1.2em;color:var(--jp-info-color1,#1976d2)\">\
                 help: {}</div>",
                esc(&help)
            ));
        }
        out.push_str("</div>");
        out
    }

    /// A value, not a handle: two diagnostics saying the same thing about the
    /// same place are equal, and hash alike.
    fn __eq__(&self, other: &PyDiagnostic) -> bool {
        self.0 == other.0
    }

    fn __hash__(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (self.0.code.as_str(), &self.0.message, &self.0.help).hash(&mut h);
        for s in &self.0.spans {
            (&s.uri, s.line, &s.label).hash(&mut h);
        }
        h.finish()
    }

    fn __repr__(&self) -> String {
        format!("<Diagnostic {} {}>", self.code(), self.0.message)
    }
}

// ---------------------------------------------------------------------------
// Values, as native Python objects
// ---------------------------------------------------------------------------

/// The diagnostics that say a document is not XML at all, rather than XML
/// that does not fit its schema.
const NOT_XML: [crate::diagnostics::DiagCode; 4] = [
    crate::diagnostics::DiagCode::MalformedXml,
    crate::diagnostics::DiagCode::UnsupportedEncoding,
    crate::diagnostics::DiagCode::MalformedEncoding,
    crate::diagnostics::DiagCode::UnknownEntity,
];

/// The key a document's root takes under `decode(..., root=True)`.
///
/// The rule every other key follows, with the set's global elements as the
/// siblings: the local name, unless another global element shares it. A root
/// that is not a global element keeps its full name, as anything a wildcard
/// admitted does.
fn root_key(schemas: &Schemas, root: &Decoded) -> String {
    let globals: Vec<QName> = schemas.global_elements().map(|e| e.name()).collect();
    match root.name.qname().filter(|q| globals.contains(q)) {
        Some(name) => decoded_key(
            schemas,
            name,
            &ambiguous_names(globals.into_iter(), schemas),
        ),
        None => schemas.display_psvi_name(&root.name),
    }
}

/// The key a name takes in a decoded dictionary.
///
/// Local name where that is unambiguous, Clark notation where it is not.
/// Which one a name gets is decided by the *schema* — by whether some sibling
/// under the same parent shares its local part — so it does not change
/// because a particular document happened to leave a sibling out.
fn decoded_key(schemas: &Schemas, name: QName, clark: &FxHashSet<QName>) -> String {
    if clark.contains(&name) {
        schemas.display_name(name)
    } else {
        schemas.local_of(name).to_string()
    }
}

/// The names under one parent type that have to be spelled out in full.
fn ambiguous_names(names: impl Iterator<Item = QName>, schemas: &Schemas) -> FxHashSet<QName> {
    // By identity first: the same name reaches this both as something the
    // schema declares and as something the document carries, and a name is
    // not ambiguous with itself.
    let unique: FxHashSet<QName> = names.collect();
    let mut by_local: FxHashMap<&str, Vec<QName>> = FxHashMap::default();
    for n in unique {
        by_local.entry(schemas.local_of(n)).or_default().push(n);
    }
    by_local
        .into_values()
        .filter(|group| group.len() > 1)
        .flatten()
        .collect()
}

/// What the schema says about one type, worked out once per decode instead of
/// once per element.
///
/// The old shape of this was computed inside the recursion, so a document
/// with twenty thousand `item`s walked `item`'s content model twenty thousand
/// times to be told the same thing.
struct TypeShape {
    /// Names that must be spelled out in Clark notation.
    clark: FxHashSet<QName>,
    /// Declared children, and whether each may appear more than once.
    repeats: FxHashMap<QName, bool>,
    /// The repeating ones, which start as empty lists whether or not the
    /// document carries any.
    repeating: Vec<QName>,
    /// The same question for attributes.
    attr_clark: FxHashSet<QName>,
    /// The dictionary key of each declared child, made once per decode.
    ///
    /// Made per occurrence, every key was a Rust `String` turned into a new
    /// Python `str` — a million of them for a document of 200,000 items, all
    /// with the GIL held.
    child_keys: FxHashMap<QName, Py<PyString>>,
    /// The same for declared attributes, `@` included.
    attr_keys: FxHashMap<QName, Py<PyString>>,
}

/// The shapes of every type reached so far in one decode.
type Shapes = FxHashMap<TypeId, std::rc::Rc<TypeShape>>;

fn shape_of(
    py: Python<'_>,
    schemas: &Schemas,
    ty: TypeId,
    shapes: &mut Shapes,
) -> std::rc::Rc<TypeShape> {
    if let Some(s) = shapes.get(&ty) {
        return s.clone();
    }
    let declared: Vec<QName> = schemas
        .children(ty)
        .iter()
        .map(|c| schemas[c.element].name)
        .collect();
    let repeats: FxHashMap<QName, bool> = schemas
        .children(ty)
        .iter()
        .map(|c| (schemas[c.element].name, c.repeats))
        .collect();
    let clark = ambiguous_names(declared.iter().copied(), schemas);
    let attributes: Vec<QName> = schemas
        .attribute_uses(ty)
        .iter()
        .map(|u| schemas[u.attribute].name)
        .collect();
    let attr_clark = ambiguous_names(attributes.iter().copied(), schemas);
    let child_keys = declared
        .iter()
        .map(|&n| {
            (
                n,
                PyString::new(py, &decoded_key(schemas, n, &clark)).unbind(),
            )
        })
        .collect();
    let attr_keys = attributes
        .iter()
        .map(|&n| {
            let key = format!("@{}", decoded_key(schemas, n, &attr_clark));
            (n, PyString::new(py, &key).unbind())
        })
        .collect();
    let shape = std::rc::Rc::new(TypeShape {
        repeating: declared
            .iter()
            .copied()
            .filter(|n| repeats.get(n).copied().unwrap_or(false))
            .collect(),
        clark,
        repeats,
        attr_clark,
        child_keys,
        attr_keys,
    });
    shapes.insert(ty, shape.clone());
    shape
}

/// Projects a decoded element onto Python data.
///
/// Lossy on purpose, and in exactly two ways: names lose their namespace
/// where nothing is ambiguous, and the type in force is not carried over.
/// `Decoded` on the Rust side keeps both for anyone who needs them.
///
/// A loop over a stack of open elements, not recursion. A recursive walk
/// spent a few hundred bytes of native stack per level, so a valid document
/// about twenty thousand elements deep overflowed it and killed the
/// interpreter, with no exception for anyone to catch.
fn decoded_to_py<'py>(
    py: Python<'py>,
    schemas: &Schemas,
    root: &Decoded,
    shapes: &mut Shapes,
) -> PyResult<Bound<'py, PyAny>> {
    let (value, open) = open_element(py, schemas, root, shapes)?;
    let mut stack: Vec<Open<'_, 'py>> = open.into_iter().collect();
    while let Some(parent) = stack.last_mut() {
        if let Some(child) = parent.children.next() {
            let (child_value, child_open) = open_element(py, schemas, child, shapes)?;
            insert_child(py, schemas, parent, child, child_value)?;
            stack.extend(child_open);
        } else if let Some(done) = stack.pop() {
            close_element(py, schemas, &done)?;
        }
    }
    Ok(value)
}

/// An element whose dictionary is already in its parent, waiting for the rest
/// of its children.
///
/// Putting a dictionary into its parent before filling it changes nothing a
/// caller can see: children are converted one at a time, so no sibling's key
/// can be inserted in between, and the keys keep document order.
struct Open<'a, 'py> {
    decoded: &'a Decoded,
    dict: Bound<'py, PyDict>,
    shape: std::rc::Rc<TypeShape>,
    children: std::slice::Iter<'a, Decoded>,
}

/// Starts converting one element: its value, and the open element that will
/// receive its children when it has any.
fn open_element<'a, 'py>(
    py: Python<'py>,
    schemas: &Schemas,
    d: &'a Decoded,
    shapes: &mut Shapes,
) -> PyResult<(Bound<'py, PyAny>, Option<Open<'a, 'py>>)> {
    let shape = shape_of(py, schemas, d.type_id, shapes);

    let mut attrs: Vec<(Bound<'py, PyString>, Bound<'py, PyAny>)> =
        Vec::with_capacity(d.attributes.len());
    for a in &d.attributes {
        let value = match &a.value {
            Some(v) => value_to_py(py, v)?,
            None => a.lexical.clone().into_bound_py_any(py)?,
        };
        // A name the schema never declared arrived through a wildcard, and
        // keeps its full name for the same reason a child element does.
        let key = match a.name.qname() {
            Some(q) => match shape.attr_keys.get(&q) {
                Some(key) => key.bind(py).clone(),
                None => PyString::new(
                    py,
                    &format!("@{}", decoded_key(schemas, q, &shape.attr_clark)),
                ),
            },
            None => PyString::new(py, &format!("@{}", schemas.display_psvi_name(&a.name))),
        };
        attrs.push((key, value));
    }

    // `xsi:nil` is the document saying there is no value, which is not the
    // same as an empty one. A nil element may still carry attributes — a nil
    // price can have a currency — and those are data like any other, so it
    // is plain `None` only when there is nothing else to say.
    if d.nil && attrs.is_empty() {
        return Ok((py.None().into_bound(py), None));
    }

    // Whether this element *has* a value, not whether this document happened
    // to give it children: an element-only type whose children are all absent
    // has no children either, and is still not a value.
    let simple = matches!(d.content, DecodedContent::Simple { .. });

    // A simple value with nothing else to say is the value itself, not a
    // dictionary wrapping one.
    if simple && attrs.is_empty() {
        return Ok((scalar(py, d)?, None));
    }

    let out = PyDict::new(py);
    for (k, v) in attrs {
        out.set_item(k, v)?;
    }

    if d.nil {
        out.set_item(pyo3::intern!(py, "$"), py.None())?;
        return Ok((out.into_any(), None));
    }
    if simple {
        // Simple content that also carries attributes: the value needs a key
        // of its own to sit beside them.
        out.set_item(pyo3::intern!(py, "$"), scalar(py, d)?)?;
        return Ok((out.into_any(), None));
    }

    let open = Open {
        decoded: d,
        dict: out.clone(),
        shape,
        children: d.children().iter(),
    };
    Ok((out.into_any(), Some(open)))
}

/// The value of simple content: typed where it validated, as written where it
/// did not.
fn scalar<'py>(py: Python<'py>, d: &Decoded) -> PyResult<Bound<'py, PyAny>> {
    match &d.content {
        DecodedContent::Simple { value: Some(v), .. } => value_to_py(py, v),
        DecodedContent::Simple { lexical, .. } => lexical.clone().into_bound_py_any(py),
        _ => Ok(py.None().into_bound(py)),
    }
}

/// Puts a child's value into its parent's dictionary, under the key the
/// schema gives it.
fn insert_child<'py>(
    py: Python<'py>,
    schemas: &Schemas,
    parent: &Open<'_, 'py>,
    child: &Decoded,
    value: Bound<'py, PyAny>,
) -> PyResult<()> {
    let (shape, out) = (&parent.shape, &parent.dict);
    // A name the schema did not declare here arrived through a wildcard.
    // Nothing about it is schema-determined, so it keeps its full name
    // rather than borrowing a short one that a declared sibling might
    // want, and its occurrence follows what the document shows.
    // Only an interned name can be looked up in the shape at all; a
    // foreign one is by definition not declared here.
    let qn = child.name.qname();
    let declared = qn.is_some_and(|q| shape.repeats.contains_key(&q));
    let key = match qn
        .filter(|_| declared)
        .and_then(|q| shape.child_keys.get(&q))
    {
        Some(key) => key.bind(py).clone(),
        None => PyString::new(py, &schemas.display_psvi_name(&child.name)),
    };
    let repeating = qn
        .and_then(|q| shape.repeats.get(&q).copied())
        .unwrap_or(false);
    match out.get_item(&key)? {
        Some(existing) if repeating => existing.cast::<PyList>()?.append(value)?,
        Some(existing) => match existing.cast::<PyList>() {
            // Already promoted by an earlier repeat: append in place.
            // Rebuilding it each time would make a wildcard carrying n
            // children of one name cost n² appends.
            Ok(list) => list.append(value)?,
            // Seen twice under a wildcard: promote to a list rather than
            // dropping the first one.
            Err(_) => {
                let list = PyList::empty(py);
                list.append(existing)?;
                list.append(value)?;
                out.set_item(key, list)?;
            }
        },
        None if repeating => {
            let list = PyList::empty(py);
            list.append(value)?;
            out.set_item(key, list)?;
        }
        None => out.set_item(key, value)?,
    }
    Ok(())
}

/// Finishes an element once all of its children are in.
fn close_element<'py>(py: Python<'py>, schemas: &Schemas, open: &Open<'_, 'py>) -> PyResult<()> {
    // Which children may repeat comes from the schema rather than from this
    // document, so a list is a list whether it holds two entries, one, or
    // none — code written against the shape never asks `isinstance(x, list)`.
    // The ones the document did not carry are added *after* the ones it did:
    // seeding them first used to put every repeating child ahead of its
    // siblings, which is not an order anyone wrote.
    for name in &open.shape.repeating {
        let key = match open.shape.child_keys.get(name) {
            Some(key) => key.bind(py).clone(),
            None => PyString::new(py, &decoded_key(schemas, *name, &open.shape.clark)),
        };
        if !open.dict.contains(&key)? {
            open.dict.set_item(key, PyList::empty(py))?;
        }
    }

    // Mixed content: the character data around the children.
    if let DecodedContent::Elements { text, .. } = &open.decoded.content {
        if !text.trim().is_empty() {
            open.dict.set_item(pyo3::intern!(py, "$"), text.clone())?;
        }
    }
    Ok(())
}

/// Drops a decoded tree without recursing.
///
/// `Decoded` owns its children, so its drop glue descends one native stack
/// frame per level: a valid document a few hundred thousand elements deep
/// overflowed the stack in a release build, and fifty thousand in a debug
/// one. Moving the children onto a heap stack first keeps every drop shallow.
fn dismantle(tree: Option<Decoded>) {
    let mut stack: Vec<Decoded> = tree.into_iter().collect();
    while let Some(mut d) = stack.pop() {
        if let DecodedContent::Elements { children, .. } = &mut d.content {
            stack.append(children);
        }
    }
}

/// The classes a value becomes, imported once rather than once per value.
///
/// Every `Decimal`, `date` and `datetime` used to begin with `py.import` and a
/// `getattr` — a module lookup and an attribute lookup for each of the hundreds
/// of thousands of values in a large document, all of it holding the GIL.
static DECIMAL: PyOnceLock<Py<PyType>> = PyOnceLock::new();
static DATE: PyOnceLock<Py<PyType>> = PyOnceLock::new();
static DATETIME: PyOnceLock<Py<PyType>> = PyOnceLock::new();
static TIME: PyOnceLock<Py<PyType>> = PyOnceLock::new();
static TIMEDELTA: PyOnceLock<Py<PyType>> = PyOnceLock::new();
static TIMEZONE: PyOnceLock<Py<PyType>> = PyOnceLock::new();
/// `datetime.timezone.utc`, which is what most timezoned values carry, and
/// what `timezone(timedelta(0))` returns anyway.
static UTC: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

/// The years Python's `datetime` can hold.
const PYTHON_YEARS: std::ops::RangeInclusive<i64> = 1..=9999;

/// `timedelta` holds 999,999,999 days either way and folds hours into days,
/// so the last whole day is left out.
const TIMEDELTA_DAYS: i64 = 999_999_999;

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
///
/// A date or a duration beyond what `datetime` and `timedelta` can hold keeps
/// its canonical lexical form the same way. XSD's year is unbounded in both
/// directions and XSD 1.1 has a year zero, and handing one of those to
/// `datetime.date` raised a bare `ValueError` out of `decode` for a document
/// `validate` had accepted.
///
/// So does anything else `datetime` would have to change to hold: an `xs:date`
/// with a timezone, which `datetime.date` has no room for, and a time or
/// duration with digits below the microsecond. Dropping the timezone made
/// `2024-12-01Z` and `2024-12-01+05:00` one value.
fn value_to_py<'py>(py: Python<'py>, v: &Value) -> PyResult<Bound<'py, PyAny>> {
    match v {
        Value::String(s) | Value::AnyUri(s) => s.into_bound_py_any(py),
        Value::Boolean(b) => b.into_bound_py_any(py),
        Value::Integer(n) => n.into_bound_py_any(py),
        // Through the shortest decimal that reads back as the same `f32`.
        // Widened digit for digit, `0.1` became `0.10000000149011612`: the
        // same 32-bit value, and not what anyone wrote.
        Value::Float(f) => widen_float(f32::from(*f)).into_bound_py_any(py),
        Value::Double(d) => f64::from(*d).into_bound_py_any(py),
        // The scale it was written with, not the canonical form. `4.50` and
        // `4.5` are one `xs:decimal` and compare equal as Python `Decimal`s
        // too, but a price written `4.50` is meant to be shown that way, and
        // arithmetic on it keeps the precision: `4.50 * 2` is `9.00`.
        Value::Decimal(d) => DECIMAL
            .import(py, "decimal", "Decimal")?
            .call1((d.as_written().to_string(),)),
        Value::HexBinary(b) | Value::Base64Binary(b) => PyBytes::new(py, b).into_bound_py_any(py),
        Value::DateTime(dt) if PYTHON_YEARS.contains(&dt.year()) => match whole_micros(dt.second())
        {
            Some((sec, micro)) => DATETIME.import(py, "datetime", "datetime")?.call1((
                dt.year(),
                dt.month(),
                dt.day(),
                dt.hour(),
                dt.minute(),
                sec,
                micro,
                tzinfo(py, dt.timezone_offset().map(|t| t.minutes()))?,
            )),
            None => v.to_string().into_bound_py_any(py),
        },
        Value::Date(d) if PYTHON_YEARS.contains(&d.year()) && d.timezone_offset().is_none() => DATE
            .import(py, "datetime", "date")?
            .call1((d.year(), d.month(), d.day())),
        Value::Time(t) => match whole_micros(t.second()) {
            Some((sec, micro)) => TIME.import(py, "datetime", "time")?.call1((
                t.hour(),
                t.minute(),
                sec,
                micro,
                tzinfo(py, t.timezone_offset().map(|t| t.minutes()))?,
            )),
            None => v.to_string().into_bound_py_any(py),
        },
        Value::DayTimeDuration(d) if d.days().abs() < TIMEDELTA_DAYS => {
            let seconds = d.seconds();
            let Some((whole, micro)) = whole_micros(seconds) else {
                return v.to_string().into_bound_py_any(py);
            };
            let sign = if seconds.is_negative() { -1 } else { 1 };
            // By name, not by position. `timedelta`'s positional order is
            // (days, seconds, microseconds, milliseconds, minutes, hours,
            // weeks) — seventh is *weeks*, and putting days there multiplied
            // every duration by seven.
            let kwargs = PyDict::new(py);
            kwargs.set_item("days", d.days())?;
            kwargs.set_item("hours", d.hours())?;
            kwargs.set_item("minutes", d.minutes())?;
            kwargs.set_item("seconds", sign * i64::from(whole))?;
            kwargs.set_item("microseconds", sign * i64::from(micro))?;
            TIMEDELTA
                .import(py, "datetime", "timedelta")?
                .call((), Some(&kwargs))
        }
        Value::List(items) => {
            let list = PyList::empty(py);
            for item in items {
                list.append(value_to_py(py, item)?)?;
            }
            list.into_bound_py_any(py)
        }
        // Clark notation, the same spelling names take everywhere else in this
        // API: `schemas["{urn:example}report"]`. The prefix is deliberately
        // gone — it is a document detail, not part of the value.
        Value::QName(q) => q.to_string().into_bound_py_any(py),
        // No lossless Python type — durations with months in them, the
        // gregorian fragments, and dates or durations out of Python's range —
        // so the canonical lexical form, which is exact.
        other => other.to_string().into_bound_py_any(py),
    }
}

/// Whole seconds and microseconds, when a count of seconds has no digit below
/// the microsecond that `datetime` and `timedelta` would have to drop.
///
/// Exact, from the decimal's digits rather than through a float, which
/// rounded `PT0.0000001S` to nothing without saying so.
fn whole_micros(seconds: crate::atomic::Decimal) -> Option<(u32, u32)> {
    let (digits, exponent) = (seconds.coefficient(), seconds.exponent());
    let scale = exponent.unsigned_abs();
    let micros = if exponent >= 0 {
        digits.checked_mul(10u128.checked_pow(scale + 6)?)?
    } else if scale <= 6 {
        digits.checked_mul(10u128.pow(6 - scale))?
    } else {
        let below = 10u128.checked_pow(scale - 6)?;
        if digits % below != 0 {
            return None;
        }
        digits / below
    };
    let whole = u32::try_from(micros / 1_000_000).ok()?;
    let micro = u32::try_from(micros % 1_000_000).ok()?;
    Some((whole, micro))
}

/// An `f32` as the Python float its shortest decimal reads as.
fn widen_float(f: f32) -> f64 {
    if f.is_finite() {
        f.to_string().parse().unwrap_or(f64::from(f))
    } else {
        f64::from(f)
    }
}

fn tzinfo<'py>(py: Python<'py>, minutes: Option<i16>) -> PyResult<Option<Bound<'py, PyAny>>> {
    let Some(m) = minutes else { return Ok(None) };
    if m == 0 {
        let utc = UTC.get_or_try_init(py, || {
            TIMEZONE
                .import(py, "datetime", "timezone")?
                .getattr("utc")
                .map(Bound::unbind)
        })?;
        return Ok(Some(utc.bind(py).clone()));
    }
    let delta = TIMEDELTA
        .import(py, "datetime", "timedelta")?
        .call1((0, 0, 0, 0, m))?;
    Ok(Some(
        TIMEZONE
            .import(py, "datetime", "timezone")?
            .call1((delta,))?,
    ))
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

impl PyValidationReport {
    /// The report for bytes that could not be decoded: an invalid document,
    /// with the one diagnostic saying why.
    fn undecodable(d: Diagnostic) -> Self {
        Self {
            valid: false,
            diagnostics: vec![PyDiagnostic(d)],
        }
    }
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

    /// Pickles as its fields, so a report crosses back from a process pool.
    fn __reduce__<'py>(slf: &Bound<'py, Self>) -> Reduced<'py, (bool, Vec<PyDiagnostic>)> {
        let r = slf.get();
        Ok((
            slf.get_type().getattr("_rebuild")?,
            (r.valid, r.diagnostics.clone()),
        ))
    }

    /// What `__reduce__` hands `pickle` to call.
    #[classmethod]
    fn _rebuild(
        _cls: &Bound<'_, PyType>,
        valid: bool,
        diagnostics: Vec<Bound<'_, PyAny>>,
    ) -> PyResult<Self> {
        let diagnostics = diagnostics
            .iter()
            .map(|d| Ok(d.cast::<PyDiagnostic>()?.get().clone()))
            .collect::<PyResult<Vec<PyDiagnostic>>>()?;
        Ok(Self { valid, diagnostics })
    }

    /// A summary line and a table of what was found.
    ///
    /// Long lists are cut off rather than filling the notebook: a document that
    /// is wrong in four hundred ways is not usefully read four hundred lines at a
    /// time.
    // The old rendering said only how many diagnostics there were, so seeing
    // any of them meant a loop.
    #[allow(non_snake_case)]
    fn _repr_html_(&self) -> String {
        let errors = self.errors().len();
        let others = self.diagnostics.len() - errors;
        let (colour, verdict) = if self.valid {
            ("var(--jp-success-color1,#388e3c)", "valid")
        } else {
            ("var(--jp-error-color1,#d32f2f)", "invalid")
        };
        let mut counts = Vec::new();
        if errors > 0 {
            counts.push(format!(
                "{errors} error{}",
                if errors == 1 { "" } else { "s" }
            ));
        }
        if others > 0 {
            counts.push(format!(
                "{others} other{}",
                if others == 1 { "" } else { "s" }
            ));
        }
        let summary = format!(
            "<div style=\"{MONO}\"><span style=\"color:{colour};font-weight:600\">{verdict}</span>\
             <span style=\"{FAINT}\">{}</span></div>",
            if counts.is_empty() {
                String::new()
            } else {
                format!(" — {}", counts.join(", "))
            }
        );

        const LIMIT: usize = 40;
        let mut rows = String::new();
        for d in self.diagnostics.iter().take(LIMIT) {
            let severity = d.severity();
            let location = d.spans().first().map(|s| s.__str__()).unwrap_or_default();
            rows.push_str(&format!(
                "<tr>{}{}{}{}</tr>",
                cell(
                    &format!("color:{};font-weight:600", severity_color(severity)),
                    &esc(severity)
                ),
                cell(FAINT, &esc(d.code())),
                cell("", &esc(&d.message())),
                cell(MUTED, &esc(&location)),
            ));
        }
        let more = self.diagnostics.len().saturating_sub(LIMIT);
        let tail = if more > 0 {
            format!("<div style=\"{MONO};{FAINT}\">… and {more} more</div>")
        } else {
            String::new()
        };
        format!("{summary}{}{tail}", table(rows))
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
    /// Where `declaration` is made from when it is read; see `EventSource`.
    source: Arc<EventSource>,
    name: (Option<String>, String),
    declaration: Option<AttributeId>,
    value: Option<Py<PyAny>>,
    lexical: String,
    from_schema: bool,
}

/// Hand-written because `Py<PyAny>` needs the GIL to clone.
impl Clone for PyAttributeValue {
    fn clone(&self) -> Self {
        Python::attach(|py| Self {
            source: self.source.clone(),
            name: self.name.clone(),
            declaration: self.declaration,
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
        self.declaration.map(|id| PyAttribute {
            s: self.source.schemas.clone(),
            id,
        })
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
    // Named `is_from_schema` in Rust so clippy does not read it as a
    // constructor; Python sees `from_schema`, which is the right name there.
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
    /// Where `declaration` and `type` are made from when they are read; see
    /// `EventSource`.
    source: Arc<EventSource>,
    kind: &'static str,
    /// Absent on a `"text"` event, which belongs to the element around it.
    name: Option<(Option<String>, String)>,
    declaration: Option<ElementId>,
    type_id: Option<TypeId>,
    type_from_instance: bool,
    nil: bool,
    attributes: Vec<PyAttributeValue>,
    value: Option<Py<PyAny>>,
    lexical: Option<String>,
    from_schema: bool,
    line: u32,
}

#[pymethods]
impl PyPsviEvent {
    /// `"start"`, `"text"` or `"end"`.
    #[getter]
    fn kind(&self) -> &'static str {
        self.kind
    }
    /// The element's name as a `(namespace, local)` pair; `None` on a
    /// `"text"` event, which belongs to the element around it.
    #[getter]
    fn name(&self) -> Option<(Option<String>, String)> {
        self.name.clone()
    }
    /// The local part of the name, without its namespace; `None` on a
    /// `"text"` event.
    #[getter]
    fn local_name(&self) -> Option<String> {
        self.name.as_ref().map(|n| n.1.clone())
    }
    /// The declaration this element matched.
    ///
    /// Absent under a `skip` wildcard, or a `lax` one with nothing to match.
    #[getter]
    fn declaration(&self) -> Option<PyElement> {
        self.declaration.map(|id| PyElement {
            s: self.source.schemas.clone(),
            id,
        })
    }
    /// The type in force, after any `xsi:type` override.
    #[getter]
    fn r#type(&self) -> Option<PyType_> {
        self.type_id.map(|id| PyType_ {
            s: self.source.schemas.clone(),
            id,
        })
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
    /// Whether the schema supplied this text, because the element was empty
    /// and its declaration had a `default` or `fixed` value.
    // Named `is_from_schema` in Rust so clippy does not read it as a
    // constructor; Python sees `from_schema`.
    #[getter(from_schema)]
    fn is_from_schema(&self) -> bool {
        self.from_schema
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
            self.kind,
            self.name.as_ref().map_or("", |n| n.1.as_str()),
            self.line
        )
    }
}

// ---------------------------------------------------------------------------
// Module-level functions
// ---------------------------------------------------------------------------

/// A compiled set with its diagnostics, for the functions that hand them back
/// rather than raise.
fn loaded(compilation: Compilation) -> (PySchemaSet, Vec<PyDiagnostic>) {
    let Compilation {
        schemas,
        diagnostics,
    } = compilation;
    (
        PySchemaSet::wrap(schemas),
        diagnostics.into_iter().map(PyDiagnostic).collect(),
    )
}

/// Loads a schema and returns it **with** its diagnostics, rather than
/// raising.
///
/// Use this when a schema is expected to be imperfect — a vendor schema with
/// dangling imports, say — and you want the components anyway.
#[pyfunction]
#[pyo3(signature = (path, *, search_paths=None, conformance="lax", version="1.0", nodes_limit=None, max_depth=None, resolver=None))]
fn load(
    py: Python<'_>,
    path: &Bound<'_, PyAny>,
    search_paths: Option<Bound<'_, PyAny>>,
    conformance: &str,
    version: &str,
    nodes_limit: Option<u32>,
    max_depth: Option<u32>,
    resolver: Option<Py<PyAny>>,
) -> PyResult<(PySchemaSet, Vec<PyDiagnostic>)> {
    let (b, raised) = builder(
        search_paths,
        conformance,
        version,
        nodes_limit,
        max_depth,
        resolver,
    )?;
    let (compilation, _) = compile(py, b.file(path_from(path)?), &raised)?;
    Ok(loaded(compilation))
}

/// The same, from a string.
#[pyfunction]
#[pyo3(signature = (xsd, *, uri="<string>", search_paths=None, conformance="lax", version="1.0", nodes_limit=None, max_depth=None, resolver=None))]
fn load_string(
    py: Python<'_>,
    xsd: String,
    uri: &str,
    search_paths: Option<Bound<'_, PyAny>>,
    conformance: &str,
    version: &str,
    nodes_limit: Option<u32>,
    max_depth: Option<u32>,
    resolver: Option<Py<PyAny>>,
) -> PyResult<(PySchemaSet, Vec<PyDiagnostic>)> {
    let (b, raised) = builder(
        search_paths,
        conformance,
        version,
        nodes_limit,
        max_depth,
        resolver,
    )?;
    let (compilation, _) = compile(py, b.text(xsd, uri), &raised)?;
    Ok(loaded(compilation))
}

/// The same, from several root documents at once.
#[pyfunction]
#[pyo3(signature = (paths, *, search_paths=None, conformance="lax", version="1.0", nodes_limit=None, max_depth=None, resolver=None))]
fn load_files(
    py: Python<'_>,
    paths: &Bound<'_, PyAny>,
    search_paths: Option<Bound<'_, PyAny>>,
    conformance: &str,
    version: &str,
    nodes_limit: Option<u32>,
    max_depth: Option<u32>,
    resolver: Option<Py<PyAny>>,
) -> PyResult<(PySchemaSet, Vec<PyDiagnostic>)> {
    let files = root_files(paths)?;
    let (mut b, raised) = builder(
        search_paths,
        conformance,
        version,
        nodes_limit,
        max_depth,
        resolver,
    )?;
    for file in files {
        b = b.file(file);
    }
    let (compilation, _) = compile(py, b, &raised)?;
    Ok(loaded(compilation))
}

/// The same, from bytes whose encoding is detected.
#[pyfunction]
#[pyo3(signature = (data, *, uri="<bytes>", search_paths=None, conformance="lax", version="1.0", nodes_limit=None, max_depth=None, resolver=None))]
fn load_bytes(
    py: Python<'_>,
    data: Vec<u8>,
    uri: &str,
    search_paths: Option<Bound<'_, PyAny>>,
    conformance: &str,
    version: &str,
    nodes_limit: Option<u32>,
    max_depth: Option<u32>,
    resolver: Option<Py<PyAny>>,
) -> PyResult<(PySchemaSet, Vec<PyDiagnostic>)> {
    let (b, raised) = builder(
        search_paths,
        conformance,
        version,
        nodes_limit,
        max_depth,
        resolver,
    )?;
    let (compilation, _) = compile(py, b.bytes(data, uri), &raised)?;
    Ok(loaded(compilation))
}

#[pymodule]
#[pyo3(name = "_xsdkit")]
fn xsdkit_module(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    let xsd_error = m.py().get_type::<XsdError>();
    // The same class-level default as `SchemaError` below, so
    // `except XsdError as e: e.diagnostics` is safe whichever of them was
    // raised, and on paths that attach none.
    //
    // A tuple, not a list. Every instance that sets no list of its own shares
    // this one object, and a list let one `e.diagnostics.append(...)` reach
    // every `XsdError` created after it.
    xsd_error.setattr("diagnostics", PyTuple::empty(m.py()))?;
    m.add("XsdError", xsd_error)?;
    let schema_error = m.py().get_type::<SchemaError>();
    // A class-level default so `except SchemaError as e: e.diagnostics` is
    // always safe, even for an error raised on a path that set none. A tuple,
    // for the same reason as above.
    schema_error.setattr("diagnostics", PyTuple::empty(m.py()))?;
    m.add("SchemaError", schema_error)?;
    m.add("DocumentError", m.py().get_type::<DocumentError>())?;

    // `create_exception!` takes one base, and `InvalidValueError` needs two:
    // it is an `XsdError`, and still a `ValueError`, so that `except
    // ValueError` goes on catching what `Type.validate` raises. So the class
    // is made the way Python makes any class.
    let py = m.py();
    let bases = PyTuple::new(
        py,
        [
            py.get_type::<XsdError>().into_any(),
            py.get_type::<PyValueError>().into_any(),
        ],
    )?;
    let namespace = PyDict::new(py);
    namespace.set_item("__module__", "xsdkit")?;
    namespace.set_item(
        "__doc__",
        "Raised by `Type.validate` for a lexical form its type does not admit. \
         Also a `ValueError`.",
    )?;
    let invalid_value = py
        .get_type::<PyType>()
        .call1(("InvalidValueError", bases, namespace))?
        .cast_into::<PyType>()?;
    m.add("InvalidValueError", &invalid_value)?;
    let _ = INVALID_VALUE_ERROR.set(py, invalid_value.unbind());
    m.add_class::<PySchemaSet>()?;
    m.add_class::<PyNamedComponents>()?;
    m.add_class::<PyNameIter>()?;
    m.add_class::<PyChild>()?;
    m.add_class::<PyChildIter>()?;
    m.add_class::<PyTree>()?;
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
    m.add_function(wrap_pyfunction!(load_files, m)?)?;
    m.add_function(wrap_pyfunction!(load_bytes, m)?)?;
    Ok(())
}
