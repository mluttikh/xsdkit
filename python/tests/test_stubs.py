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
