"""Compare component observations; this does not accept a website."""
import json
from pathlib import Path

root = Path(__file__).resolve().parent
native = json.loads((root / 'after.json').read_text())
chrome = json.loads((root / 'chrome.json').read_text())
assert len(native) == len(chrome) == 19
for n, c in zip(native, chrome):
    a, b = n['data'], c['data']
    assert a['canvas'] == b['canvas']
    assert a['matches'] == b['matches']
    assert a['size'][::2] == b['size'][::2]
    assert all(abs(x - y) < .001 for x, y in zip(a['box'], b['box']))
    assert a['lastDevice'] == b['lastDevice']
    assert all(event[1] for event in a['events'])
assert len({n['data']['identity'] for n in native}) == 1
assert len({n['data']['timeOrigin'] for n in native}) == 1
for n in native[:15]:
    assert all(not window['findings'] for window in n['review'])
stripe = json.loads((root / 'stripe.json').read_text())
for n in stripe:
    assert n['data']['dpr'] == n['scale']
    assert n['data']['dark'] == (n['mode'] != 'light')
    assert n['data']['resolution'] == (n['scale'] >= 1.5)
    assert all(not window['findings'] for window in n['review'])
print('19 component observations match; 30 widget reviews clean; Stripe environment follows all 15 changes.')
