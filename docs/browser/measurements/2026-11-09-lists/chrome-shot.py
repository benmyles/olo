from pathlib import Path
import base64
p=Path('/var/tmp/codex-browser-lists/oracle.py').read_text();exec(p[:p.index('token=')])
ws.call('Emulation.setDeviceMetricsOverride',width=1000,height=578,deviceScaleFactor=1,mobile=False)
ws.call('Page.navigate',url=(root/'review.html').as_uri())
for _ in range(100):
 r=ws.call('Runtime.evaluate',expression="location.pathname.endsWith('/review.html') && document.readyState==='complete' && !!document.getElementById('y3')",returnByValue=True)
 if r['result'].get('value'):break
 time.sleep(.02)
ws.call('Runtime.evaluate',expression='document.fonts.ready',awaitPromise=True)
(root/'chrome-review.png').write_bytes(base64.b64decode(ws.call('Page.captureScreenshot',format='png')['data']))
ws.close()
