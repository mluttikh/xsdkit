"""The stubs are only useful if they describe what actually exists."""

import ast
import pathlib

import xsdkit

STUB = pathlib.Path(xsdkit.__file__).with_name("_xsdkit.pyi")


def _stub_tree() -> ast.Module:
    return ast.parse(STUB.read_text())


def test_every_exported_name_exists():
    for name in xsdkit.__all__:
        assert hasattr(xsdkit, name), f"__all__ names {name}, which does not exist"


def test_every_stubbed_class_exists():
    for node in _stub_tree().body:
        if isinstance(node, ast.ClassDef):
            assert hasattr(xsdkit, node.name), f"stub declares {node.name}, runtime has no such name"


def test_every_stubbed_member_exists():
    """Catches a stub that has drifted from `src/python.rs`."""
    missing = []
    for node in _stub_tree().body:
        if not isinstance(node, ast.ClassDef):
            continue
        runtime = getattr(xsdkit, node.name, None)
        if runtime is None:
            continue
        for member in node.body:
            if isinstance(member, ast.FunctionDef):
                if not hasattr(runtime, member.name):
                    missing.append(f"{node.name}.{member.name}")
            elif isinstance(member, ast.AnnAssign) and isinstance(member.target, ast.Name):
                if not hasattr(runtime, member.target.id):
                    missing.append(f"{node.name}.{member.target.id}")
    assert not missing, f"stubbed but absent at runtime: {missing}"


def test_package_is_marked_typed():
    assert pathlib.Path(xsdkit.__file__).with_name("py.typed").exists()


def test_everything_public_is_documented():
    """`help()` is the documentation most users read, and it decays silently.

    A `#[getter]` with no `///` above it produces an empty docstring, not an
    error, so this is the only thing that notices.
    """
    import inspect

    undocumented = []
    for name, cls in vars(xsdkit).items():
        if not inspect.isclass(cls) or cls.__module__ != "xsdkit":
            continue
        if not (cls.__doc__ or "").strip():
            undocumented.append(f"class {name}")
        for member, obj in vars(cls).items():
            if member.startswith("_"):
                continue
            if not (getattr(obj, "__doc__", None) or "").strip():
                undocumented.append(f"{name}.{member}")

    for fn in ("load", "load_string"):
        if not (getattr(xsdkit, fn).__doc__ or "").strip():
            undocumented.append(fn)

    assert not undocumented, "undocumented public API: " + ", ".join(sorted(undocumented))


def test_constructors_show_their_signature():
    """`help(SchemaSet.from_file)` must name the keywords, not print `(...)`."""
    import inspect

    sig = str(inspect.signature(xsdkit.SchemaSet.from_file))
    for kw in ("search_paths", "conformance", "version", "nodes_limit", "resolver"):
        assert kw in sig, f"{kw} missing from the rendered signature"
