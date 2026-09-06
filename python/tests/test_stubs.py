"""The stubs are only useful if they describe what actually exists."""

import ast
import pathlib

import xsdkit

STUB = pathlib.Path(xsdkit.__file__).with_name("_xsdkit.pyi")


def _stub_tree() -> ast.Module:
    return ast.parse(STUB.read_text())


def _stub_classes() -> dict[str, ast.ClassDef]:
    return {n.name: n for n in _stub_tree().body if isinstance(n, ast.ClassDef)}


def _runtime_classes():
    """The classes this extension module defines, by name."""
    import inspect

    return {
        name: cls
        for name, cls in vars(xsdkit).items()
        if inspect.isclass(cls) and cls.__module__ == "xsdkit"
    }


def _declared(node: ast.ClassDef) -> dict[str, ast.stmt]:
    """What a stubbed class body declares, by member name."""
    out: dict[str, ast.stmt] = {}
    for member in node.body:
        if isinstance(member, ast.FunctionDef):
            out[member.name] = member
        elif isinstance(member, ast.AnnAssign) and isinstance(member.target, ast.Name):
            out[member.target.id] = member
    return out


def _is_dunder(name: str) -> bool:
    return name.startswith("__") and name.endswith("__")


def _is_property(node: ast.stmt) -> bool:
    return isinstance(node, ast.FunctionDef) and any(
        isinstance(d, ast.Name) and d.id == "property" for d in node.decorator_list
    )


def _stub_params(fn: ast.FunctionDef) -> tuple[list[str], list[str]]:
    """(positional names, keyword-only names), with `self` dropped."""
    args = fn.args
    positional = [a.arg for a in args.posonlyargs + args.args if a.arg != "self"]
    return positional, [a.arg for a in args.kwonlyargs]


def _runtime_params(obj) -> tuple[list[str], list[str]] | None:
    import inspect

    try:
        sig = inspect.signature(obj)
    except (ValueError, TypeError):
        return None
    params = [p for name, p in sig.parameters.items() if name != "self"]
    positional = [p.name for p in params if p.kind is not p.KEYWORD_ONLY]
    return positional, [p.name for p in params if p.kind is p.KEYWORD_ONLY]


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


def test_every_runtime_class_is_stubbed():
    """The direction that catches something *added* to `src/python.rs`.

    The stub is the only type surface Python users have, and nothing about
    adding a `#[pyclass]` obliges anyone to write it down.
    """
    stubbed = _stub_classes()
    missing = [name for name in _runtime_classes() if name not in stubbed]
    assert not missing, f"classes with no stub: {sorted(missing)}"


def test_every_runtime_member_is_stubbed():
    """The same, for every getter and method on those classes.

    Dunders are left out: the ones the stub declares are checked in the other
    direction, and Python synthesises the rest of the comparison protocol from
    a single `__eq__`.
    """
    stubbed = _stub_classes()
    missing = []
    for name, cls in _runtime_classes().items():
        node = stubbed.get(name)
        if node is None:
            continue
        declared = _declared(node)
        for member in vars(cls):
            if not _is_dunder(member) and member not in declared:
                missing.append(f"{name}.{member}")
    assert not missing, f"present at runtime, absent from the stub: {sorted(missing)}"


def test_every_exported_name_is_stubbed():
    tree = _stub_tree()
    names = {n.name for n in tree.body if isinstance(n, (ast.ClassDef, ast.FunctionDef))}
    names |= {
        n.target.id
        for n in tree.body
        if isinstance(n, ast.AnnAssign) and isinstance(n.target, ast.Name)
    }
    missing = [n for n in xsdkit.__all__ if n not in names]
    assert not missing, f"exported but not stubbed: {sorted(missing)}"


def test_the_stub_agrees_about_what_is_a_property():
    """`report.children` and `report.children()` are different code.

    A getter stubbed as a method — or the reverse — type-checks the calling
    code wrongly while the runtime keeps working, which is drift that no
    amount of running the tests would surface.
    """
    stubbed = _stub_classes()
    wrong = []
    for name, cls in _runtime_classes().items():
        node = stubbed.get(name)
        if node is None:
            continue
        declared = _declared(node)
        for member, obj in vars(cls).items():
            if _is_dunder(member) or member not in declared:
                continue
            # PyO3 renders a `#[getter]` as a get-set descriptor and a plain
            # method as a method descriptor.
            runtime_property = type(obj).__name__ == "getset_descriptor"
            if runtime_property != _is_property(declared[member]):
                wrong.append(
                    f"{name}.{member}: runtime property={runtime_property}, "
                    f"stub property={_is_property(declared[member])}"
                )
    assert not wrong, "property and method disagree: " + "; ".join(sorted(wrong))


def test_the_stub_agrees_about_parameters():
    """A renamed keyword is silent breakage for anyone type-checking.

    Names and keyword-only-ness are what a caller writes, so those are what is
    compared; annotations and defaults are the stub's own business.
    """
    stubbed = _stub_classes()
    wrong = []
    for name, cls in _runtime_classes().items():
        node = stubbed.get(name)
        if node is None:
            continue
        declared = _declared(node)
        for member, obj in vars(cls).items():
            fn = declared.get(member)
            if _is_dunder(member) or not isinstance(fn, ast.FunctionDef):
                continue
            if type(obj).__name__ == "getset_descriptor":
                continue
            runtime = _runtime_params(obj)
            if runtime is None:
                continue
            if runtime != _stub_params(fn):
                wrong.append(f"{name}.{member}: runtime {runtime} vs stub {_stub_params(fn)}")

    for fn_name in ("load", "load_string"):
        fn = next(
            (
                n
                for n in _stub_tree().body
                if isinstance(n, ast.FunctionDef) and n.name == fn_name
            ),
            None,
        )
        if fn is None:
            continue
        runtime = _runtime_params(getattr(xsdkit, fn_name))
        if runtime is not None and runtime != _stub_params(fn):
            wrong.append(f"{fn_name}: runtime {runtime} vs stub {_stub_params(fn)}")

    assert not wrong, "parameters disagree: " + "; ".join(sorted(wrong))
