#!/usr/bin/python3
"""Unicode Default Caseless Matching facts, C/F only, no normalization/tailoring.

Run from the repository root. No engine output or font oracle is inspected.
The pinned Unicode input and license live with the native font fixtures.
"""
import hashlib
from pathlib import Path

source = Path('crates/web/tests/font-native-fixtures/ucd/CaseFolding.txt')
data = source.read_bytes()
entries = {}
for line in data.decode().splitlines():
    record = line.split('#', 1)[0].strip()
    if not record:
        continue
    code, status, mapping, _ = record.split(';')
    if status.strip() in ['C', 'F']:
        entries[int(code, 16)] = ''.join(chr(int(x, 16)) for x in mapping.split())
text = '// Generated Unicode C/F case-fold facts; no normalization.\n'
text += '// ' + data.decode().splitlines()[0].removeprefix('# ') + '\n'
text += '// Source SHA-256: ' + hashlib.sha256(data).hexdigest() + '\n'
text += '// Unicode License V3, see tests/font-native-fixtures/ucd.\n'
text += 'pub(super) const FOLD: &[(char, &str)] = &[\n'
for code, mapping in sorted(entries.items()):
    escaped = ''.join('\\u{' + format(ord(c), 'x') + '}' for c in mapping)
    text += "    ('\\u{" + format(code, 'x') + "}', \"" + escaped + '\"),\n'
text += '];\n'
Path('crates/web/src/font/casefold_data.rs').write_text(text)
print('Generated', len(entries), 'C/F mappings')
