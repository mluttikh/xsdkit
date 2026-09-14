#!/usr/bin/env python3
"""The W3C XML Schema Test Suite, through the Python bindings.

`tests/w3c_suite.rs` scores the Rust crate against the suite, one case at a
time, against the committed baseline in `tests/conformance/`. It never goes
through `src/python.rs`, and that is where a date the bindings could not
convert made `decode` raise for nine documents `validate` accepted, with no
test anywhere to notice.

This runs the same instance cases through the installed `xsdkit` package and
fails on anything the bindings add or lose:

- a case list that differs from the baseline's, so the two harnesses cannot
  quietly select different cases;
- a `validate` verdict, or error codes, that differ from the baseline's;
- `iter_typed`, `read_typed`, or a schema set round-tripped through
  `serialize`, reaching a different report than `validate`;
- `decode` raising when the document is valid, or not raising
  `DocumentError` when it is invalid, or `decode(lax=True)` refusing a
  document that is XML;
- any exception that is not an `XsdError`.

    XSDTESTS=/tmp/xsdtests python3 scripts/check-w3c-python.py

Case selection follows `tests/suite/mod.rs` rule for rule; read the comments
there for why each rule is what it is.
"""

from __future__ import annotations

import os
import re
import sys
import time
import traceback
import xml.etree.ElementTree as ET
from collections import defaultdict
from pathlib import Path

import xsdkit

REPO = Path(__file__).resolve().parents[1]
BASELINE = REPO / "tests" / "conformance" / "instance-cases.tsv"
XLINK_HREF = "{http://www.w3.org/1999/xlink}href"
# Failures past this many are counted rather than printed.
SHOWN = 40


def local(tag: str) -> str:
    return tag.rsplit("}", 1)[-1]


def children(node: ET.Element, name: str) -> list[ET.Element]:
    return [c for c in node if local(c.tag) == name]


def expected_validity(test: ET.Element, version: str) -> str | None:
    expects = children(test, "expected")
    chosen = next((e for e in expects if version in (e.get("version") or "")), None)
    if chosen is None:
        chosen = next((e for e in expects if e.get("version") is None), None)
    return None if chosen is None else chosen.get("validity")


def versions_of(declared: str) -> list[str]:
    has10, has11 = "1.0" in declared, "1.1" in declared or "CTA" in declared
    if has10 and not has11:
        return ["1.0"]
    if has11 and not has10:
        return ["1.1"]
    return ["1.0", "1.1"]


def disputed(test: ET.Element) -> bool:
    current = next(iter(children(test, "current")), None)
    return current is not None and current.get("status") == "queried"


def instance_cases(root: Path):
    """Every instance case, as `tests/suite/mod.rs` selects them."""
    files = sorted(p for p in root.rglob("*.testSet") if ".git" not in p.parts)
    for f in files:
        try:
            doc = ET.parse(f).getroot()
        except ET.ParseError:
            continue
        directory = f.parent
        set_name = doc.get("name", "?")
        for group in (e for e in doc.iter() if local(e.tag) == "testGroup"):
            declared = group.get("version", "1.0 1.1")
            name = group.get("name", "?")
            for schema_test in children(group, "schemaTest"):
                documents = [
                    directory / d.get(XLINK_HREF)
                    for d in children(schema_test, "schemaDocument")
                    if d.get(XLINK_HREF)
                ]
                if not documents:
                    continue
                for version in versions_of(declared):
                    if expected_validity(schema_test, version) != "valid":
                        continue
                    for test in children(group, "instanceTest"):
                        href = next(
                            (d.get(XLINK_HREF) for d in children(test, "instanceDocument")),
                            None,
                        )
                        validity = expected_validity(test, version)
                        if href is None or validity not in ("valid", "invalid"):
                            continue
                        instance = directory / href
                        yield {
                            "key": f"{set_name}/{name}/{Path(href).name}@{version}",
                            "version": version,
                            "documents": documents,
                            "instance": instance,
                            "expected": validity,
                            "disputed": disputed(test),
                        }


def read_baseline() -> dict[str, list[dict]]:
    """The baseline's rows, by case key.

    Two keys repeat in the suite, and `check_baseline` suffixes the later of
    each pair with `#2` in *sorted* order, so the suffix says nothing about
    which walk it came from. The rows are paired with cases by expectation
    instead, which is what tells the `sg-abstract-upa2` pair apart.
    """
    rows: dict[str, list[dict]] = defaultdict(list)
    for line in BASELINE.read_text(encoding="utf-8").splitlines():
        if not line or line.startswith("#"):
            continue
        key, expected, verdict, codes = line.split("\t")
        rows[re.sub(r"#\d+$", "", key)].append(
            {"expected": expected, "verdict": verdict, "codes": codes, "used": False}
        )
    return rows


def codes_of(report: xsdkit.ValidationReport) -> str:
    codes = sorted({d.code for d in report.errors})
    return "+".join(codes) if codes else "-"


def verdict_of(report: xsdkit.ValidationReport) -> tuple[str, str]:
    return ("accept", "-") if report.is_valid else ("reject", codes_of(report))


def main() -> int:
    root = Path(os.environ.get("XSDTESTS", sys.argv[1] if len(sys.argv) > 1 else ""))
    if not (root / "suite.xml").is_file():
        print(f"no suite.xml under {root!s}: set XSDTESTS to a checkout of w3c/xsdtests")
        return 2

    started = time.perf_counter()
    baseline = read_baseline()
    failures: list[str] = []
    schemas_cache: dict[tuple, tuple | None] = {}
    tally = defaultdict(int)

    def fail(key: str, what: str) -> None:
        failures.append(f"{key}: {what}")

    for case in instance_cases(root):
        key = case["key"]
        row = next(
            (r for r in baseline.get(key, []) if not r["used"] and r["expected"] == case["expected"]),
            None,
        )
        if row is None:
            fail(key, f"no baseline row expecting {case['expected']}: the case lists have drifted apart")
            continue
        row["used"] = True
        verdict, codes = row["verdict"], row["codes"]

        tally["runs"] += 1
        if case["disputed"]:
            tally["skipped"] += 1
            if verdict != "skip":
                fail(key, f"disputed, but the baseline says {verdict}")
            continue

        # `read_to_string` in the Rust harness, so the same documents are
        # unreadable to both; anything else would compare different inputs.
        try:
            text = case["instance"].read_bytes().decode("utf-8")
        except (OSError, UnicodeDecodeError):
            tally["skipped"] += 1
            if (verdict, codes) != ("skip", "unreadable"):
                fail(key, f"unreadable here, but the baseline says {verdict} {codes}")
            continue

        cache_key = (case["version"], tuple(map(str, case["documents"])))
        if cache_key not in schemas_cache:
            try:
                schemas, diagnostics = xsdkit.load_files(
                    case["documents"],
                    search_paths=[case["documents"][0].parent],
                    version=case["version"],
                    conformance="lax",
                )
                compiled = None
                if not any(d.is_error for d in diagnostics):
                    copy = xsdkit.SchemaSet.deserialize(schemas.serialize())
                    compiled = (schemas, copy)
            except BaseException as e:  # a panic is a BaseException
                if isinstance(e, KeyboardInterrupt):
                    raise
                compiled = None
                fail(key, f"loading raised {type(e).__name__}: {e}")
            schemas_cache[cache_key] = compiled
        compiled = schemas_cache[cache_key]
        if compiled is None:
            tally["skipped"] += 1
            if (verdict, codes) != ("skip", "schema"):
                fail(key, f"schema did not compile here, but the baseline says {verdict} {codes}")
            continue
        schemas, copy = compiled

        try:
            report = schemas.validate(text)
            got = verdict_of(report)
            tally["accepted" if report.is_valid else "rejected"] += 1
            if got != (verdict, codes):
                fail(key, f"validate says {got[0]} {got[1]}, the baseline {verdict} {codes}")

            events = schemas.iter_typed(text)
            streamed = sum(1 for _ in events)
            if verdict_of(events.report) != got:
                fail(key, f"iter_typed reports {verdict_of(events.report)}, validate {got}")

            listed, listed_report = copy.read_typed(text)
            if verdict_of(listed_report) != got:
                fail(key, f"read_typed on a deserialized schema set reports "
                          f"{verdict_of(listed_report)}, validate {got}")
            if len(listed) != streamed:
                fail(key, f"read_typed gave {len(listed)} events, iter_typed {streamed}")

            try:
                schemas.decode(text)
                if not report.is_valid:
                    fail(key, "decode returned data for an invalid document")
            except xsdkit.DocumentError as e:
                if report.is_valid:
                    fail(key, f"decode refused a valid document: {e}")
                elif codes_of_error(e) != got[1]:
                    fail(key, f"decode refused with {codes_of_error(e)}, validate {got[1]}")

            try:
                schemas.decode(text, lax=True)
            except xsdkit.DocumentError as e:
                if not any(d.code in NOT_XML for d in e.diagnostics):
                    fail(key, f"decode(lax=True) refused XML: {codes_of_error(e)}")
        except BaseException as e:  # noqa: BLE001 - every escape is the finding
            if isinstance(e, KeyboardInterrupt):
                raise
            where = traceback.extract_tb(e.__traceback__)[-1]
            fail(key, f"{type(e).__name__}: {e} (line {where.lineno})")

    for key, rows in baseline.items():
        for row in rows:
            if not row["used"]:
                fail(key, f"in the baseline expecting {row['expected']}, but not selected here: "
                          "the case lists have drifted apart")

    elapsed = time.perf_counter() - started
    print("=== W3C XML Schema Test Suite — instance cases, through xsdkit for Python ===")
    print(f"xsdkit {xsdkit.__version__}, Python {sys.version.split()[0]}, {elapsed:.0f} s")
    print(f"runs {tally['runs']}: accepted {tally['accepted']}, rejected {tally['rejected']}, "
          f"not scored {tally['skipped']}")
    if failures:
        print(f"\n{len(failures)} disagreement(s) with the Rust baseline or between the calls:")
        for line in failures[:SHOWN]:
            print(f"  {line}")
        if len(failures) > SHOWN:
            print(f"  ... and {len(failures) - SHOWN} more")
        return 1
    print("every case agrees with tests/conformance/instance-cases.tsv, and every call with validate")
    return 0


# What says a document is not XML at all, which is the one thing
# `decode(lax=True)` still refuses.
NOT_XML = {"XSD1001", "XSD1006", "XSD1007", "XSD2017"}


def codes_of_error(e: xsdkit.DocumentError) -> str:
    codes = sorted({d.code for d in e.diagnostics if d.is_error})
    return "+".join(codes) if codes else "-"


if __name__ == "__main__":
    sys.exit(main())
