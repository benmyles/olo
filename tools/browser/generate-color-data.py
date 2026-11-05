#!/usr/bin/python3
"""Extract normative color-table facts, never algorithms or test expectations.

Usage: generate-color-data.py SOURCE_HTML OUTPUT_DIRECTORY
Then run rustfmt on the two output files. The pinned HTML must be supplied
explicitly; updating the source is a reviewed standards-data change.
"""
import ast
import hashlib
from html.parser import HTMLParser
import json
from pathlib import Path
import re
import sys

PIN = "30fc610f95aaa508a4af345c736eed027da59c0125851fed95dd2c9684c43ffc"


class Text(HTMLParser):
    def __init__(self):
        super().__init__()
        self.parts = []

    def handle_data(self, data):
        self.parts.append(data)


def numeric(node):
    """Small numeric-data grammar, with no eval or JS execution."""
    if isinstance(node, ast.List):
        return [numeric(item) for item in node.elts]
    if isinstance(node, ast.Constant) and type(node.value) in (int, float):
        return float(node.value)
    if isinstance(node, ast.UnaryOp) and isinstance(node.op, ast.USub):
        return -numeric(node.operand)
    if isinstance(node, ast.BinOp) and isinstance(node.op, ast.Div):
        return numeric(node.left) / numeric(node.right)
    raise ValueError(f"not normative numeric table data: {ast.dump(node)}")


def main():
    source, destination = map(Path, sys.argv[1:])
    raw = source.read_bytes()
    assert hashlib.sha256(raw).hexdigest() == PIN, "source differs from pinned HTML"
    html = raw.decode("utf-8")
    names = re.findall(r'id="valdef-color-([a-z]+)">[a-z]+</dfn>\s*<td>#([0-9a-f]{6})', html)
    assert len(names) == len(dict(names)) == 148
    named = [
        "//! Normative named-color data, CSS Color 4 (retrieved 2026-11-05), section 6.1.",
        "//! https://drafts.csswg.org/css-color-4/#named-colors",
        "//! Facts extracted from the standard's named-color table, not test outputs.",
        f"//! Source HTML SHA-256: {PIN}",
        "",
        'pub(super) const NAMES: [(&str, u32); 148] = [',
    ]
    named.extend(f'    ({json.dumps(name)}, 0x{rgb}),' for name, rgb in sorted(names))
    named.append("];")
    parser = Text()
    parser.feed(html)
    text = "".join(parser.parts)
    tables = [
        ("SRGB_TO_XYZ", "lin_sRGB_to_XYZ", "M"),
        ("XYZ_TO_SRGB", "XYZ_to_lin_sRGB", "M"),
        ("P3_TO_XYZ", "lin_P3_to_XYZ", "M"),
        ("XYZ_TO_P3", "XYZ_to_lin_P3", "M"),
        ("PROPHOTO_TO_XYZ", "lin_ProPhoto_to_XYZ", "M"),
        ("XYZ_TO_PROPHOTO", "XYZ_to_lin_ProPhoto", "M"),
        ("A98_TO_XYZ", "lin_a98rgb_to_XYZ", "M"),
        ("XYZ_TO_A98", "XYZ_to_lin_a98rgb", "M"),
        ("REC2020_TO_XYZ", "lin_2020_to_XYZ", "M"),
        ("XYZ_TO_REC2020", "XYZ_to_lin_2020", "M"),
        ("D65_TO_D50", "D65_to_D50", "M"),
        ("D50_TO_D65", "D50_to_D65", "M"),
        ("XYZ_TO_LMS", "XYZ_to_OKLab", "XYZtoLMS"),
        ("LMS_TO_OKLAB", "XYZ_to_OKLab", "LMStoOKLab"),
        ("OKLAB_TO_LMS", "OKLab_to_XYZ", "OKLabtoLMS"),
        ("LMS_TO_XYZ", "OKLab_to_XYZ", "LMStoXYZ"),
    ]
    matrices = [
        "//! Normative matrix data only, CSS Color 4 section 19. Algorithms are ours.",
        "//! https://drafts.csswg.org/css-color-4/#color-conversion-code",
        f"//! Retrieved 2026-11-05; HTML SHA-256: {PIN}",
    ]
    for name, function, variable in tables:
        body = text.split(f"function {function}(", 1)[1].split("\nfunction ", 1)[0]
        body = re.sub(r"//[^\n]*", "", body)
        match = re.search(r"\bvar\s+" + variable + r"\s*=\s*(\[[\s\S]*?\]);", body)
        assert match, (function, variable)
        values = numeric(ast.parse(match[1], mode="eval").body)
        assert len(values) == 3 and all(len(row) == 3 for row in values)
        table = json.dumps(values)
        if name == "D65_TO_D50":
            table = table[:-1] + ",]"
        matrices.append(f"pub(super) const {name}: [[f64; 3]; 3] = {table};")
    destination.mkdir(parents=True, exist_ok=True)
    (destination / "named.rs").write_text("\n".join(named) + "\n")
    (destination / "matrices.rs").write_text("\n".join(matrices) + "\n")


if __name__ == "__main__":
    main()
