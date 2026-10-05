#!/usr/bin/env python3
"""Summarize an EARL report produced by TEAM Engine: counts and failed tests."""
import sys
import xml.etree.ElementTree as ET

EARL = "http://www.w3.org/ns/earl#"
RDF = "http://www.w3.org/1999/02/22-rdf-syntax-ns#"
DCT = "http://purl.org/dc/terms/"
CITE = "http://cite.opengeospatial.org/"

path = sys.argv[1]
try:
    root = ET.parse(path).getroot()
except ET.ParseError as e:
    print(f"Invalid EARL report {path}: {e}")
    print(open(path, encoding="utf-8", errors="replace").read()[:2000])
    sys.exit(2)

run = root.find(f".//{{{CITE}}}TestRun")
if run is not None:
    for tag in ["testsPassed", "testsFailed", "testsSkipped"]:
        el = run.find(f"{{{CITE}}}{tag}")
        print(f"{tag}: {el.text if el is not None else '?'}")

failed = []
for assertion in root.iter(f"{{{EARL}}}Assertion"):
    outcome = assertion.find(f".//{{{EARL}}}outcome")
    res = outcome.get(f"{{{RDF}}}resource", "") if outcome is not None else ""
    if res.endswith(("failed", "fail", "cantTell")):
        test = assertion.find(f"{{{EARL}}}test")
        name = ""
        if test is not None:
            name = test.get(f"{{{RDF}}}resource", "")
            title = test.find(f".//{{{DCT}}}title")
            if title is not None and title.text:
                name = title.text
        desc = assertion.find(f".//{{{DCT}}}description")
        failed.append((name, (desc.text or "").strip() if desc is not None else ""))

print(f"failed assertions: {len(failed)}")
for name, desc in failed:
    print(f"- {name}: {desc[:400]}")
sys.exit(1 if failed else 0)
