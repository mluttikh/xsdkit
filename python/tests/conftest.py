import pathlib
import pytest
import xsdkit

REPO = pathlib.Path(__file__).resolve().parents[2]
FIXTURE = REPO / "tests" / "fixtures" / "XMLSchema.xsd"
XS = "http://www.w3.org/2001/XMLSchema"
NS = "urn:example"


@pytest.fixture(scope="session")
def schema_for_schemas():
    """The W3C schema for schemas — the same fixture the Rust tests use."""
    schemas, _ = xsdkit.load(str(FIXTURE), conformance="lax")
    return schemas


def build(body: str, **kwargs) -> xsdkit.SchemaSet:
    """Compiles a small schema whose body is dropped into `xs:schema`."""
    xsd = (
        '<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" '
        f'xmlns:tns="{NS}" targetNamespace="{NS}" '
        f'elementFormDefault="qualified">{body}</xs:schema>'
    )
    return xsdkit.SchemaSet.from_string(xsd, **kwargs)
