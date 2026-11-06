# Initial HTML CSS control state

Original passive HTML, four document phases, 176 independent Chrome element
observations / 352 enabled/disabled booleans. `SOURCE.json` pins inputs, capture
code, transport and primary references. Chrome is launched through Olo on an
owned Agent Screen/profile; the capture proves ownership before creating a
local fixture tab and closes it afterward. No form is submitted.

```
# On an owned screen, with its environment, under /var/tmp:
olo-screen run NAME olo-chrome --user-data-dir=/var/tmp/OWNED/profile --remote-debugging-port=0 about:blank
/usr/bin/python3 crates/web/tests/control-fixtures/capture.py --screen NAME --profile /var/tmp/OWNED/profile --out /var/tmp/OWNED/control.json
```

CSS option/optgroup selector state includes the owning select in this pinned
Chrome build (`OptionDisablednessCheckAncestors`), while intrinsic option
activation/submission disabledness does not. Fieldset selector state also uses
actual inherited disabledness. These distinctions are pinned in the primary
implementation references; this API supplies CSS state only.

The native index covers connected inert HTML controls and rejects stale or
unindexed control queries. It is built lazily by the page worker, bounded and
cancelled; no DOM copy or repeated ancestor scan. Template/disconnected/shadow
styling, custom form-associated upgrades, other states, form widget interaction,
Web IDL and live invalidation remain required. These fixtures accept no sites.
