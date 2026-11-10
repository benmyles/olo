import json,shutil,hashlib,re
from pathlib import Path
src=Path('/var/tmp/codex-browser-scroll');dest=Path('docs/browser/measurements/2026-11-10-scroll-into-view');dest.mkdir(exist_ok=True)
for p in src.iterdir():
 if p.is_file() and p.suffix in ['.py','.log','.json','.html'] and p.name not in ['merge-metadata.json']:shutil.copy2(p,dest/p.name)
for name in ['before','after']:
 if (src/name).is_dir():shutil.copytree(src/name,dest/name,dirs_exist_ok=True)
log=(dest/'test-workspace.log').read_text();rows=re.findall(r'test result: (\w+)\. (\d+) passed; (\d+) failed; (\d+) ignored',log)
summary={'suites':len(rows),'passed':sum(int(r[1])for r in rows),'failed':sum(int(r[2])for r in rows),'ignored':sum(int(r[3])for r in rows)}
(dest/'test-summary.json').write_text(json.dumps(summary,indent=2)+'\n');print(summary)
