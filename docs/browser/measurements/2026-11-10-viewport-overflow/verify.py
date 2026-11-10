"""Validate this checkpoint's recorded evidence; does not accept a website."""
import json
from pathlib import Path

root = Path(__file__).resolve().parent
def read(path):
    return json.loads((root / path).read_text())

assert len(read('chrome.json')) == 100
for variant in ['before', 'after', 'stripe']:
    rows = read(f'{variant}/results.json')
    assert len(rows) == 15
    assert {(r['mode'], r['scale']) for r in rows} == {
        (mode, scale) for mode in ['light', 'dark', 'night'] for scale in [1, 1.25, 1.5, 1.75, 2]
    }
    assert all(not w['findings'] for r in rows for w in r['review'])
for row in read('stripe/results.json'):
    assert row['data']['dpr'] == row['scale']
    assert row['data']['dark'] == (row['mode'] != 'light')
    assert row['data']['canvases'][0][0] == 1000  # gutter discrepancy remains
rows = read('costs.json')
assert len(rows) == 8
assert [r['variant'] for r in rows] == ['before', 'after', 'after', 'before'] * 2
for row in rows:
    assert row['idle']['total']['cpu'] == 0
    assert all(w['frames'] == 0 for w in row['idle']['windows'])
    assert all(s['moving_frames'] > 0 and s['hitches'] == 0 for s in row['scrolling'])
    if row['variant'] == 'after':
        assert len(row['bench_ms']['sizes']) == 7
        for sizes in row['bench_ms']['sizes']:
            assert len(sizes) == 20
            assert len(set(sizes)) == 2
            assert len(set(sizes[::2])) == len(set(sizes[1::2])) == 1
validation = read('validation.json')
assert (validation['passed'], validation['failed'], validation['ignored']) == (2197, 0, 136)
print('45 reviews, 15 Stripe environment checks and 16 moving scroll probes pass; no site accepted.')
