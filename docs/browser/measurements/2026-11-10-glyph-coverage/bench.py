import hashlib,json,os,statistics,subprocess,time
from pathlib import Path
root=Path('/var/tmp/codex-browser-glyphs');rows=[]
def compilers():return subprocess.run(['ps','-C','rustc','-o','pid=,etimes=,pcpu='],capture_output=True,text=True).stdout
binary=Path('target/release/examples/paint_bench')
for i,mode in enumerate(['direct','cached','cached','direct']):
 before=compilers();load=os.getloadavg();start=time.monotonic()
 data=json.loads(subprocess.check_output([str(binary),'500',mode]))
 row=dict(mode=mode,binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(),compilers_before=before,compilers_after=compilers(),load=load,elapsed=time.monotonic()-start,result=data);rows.append(row)
 print(mode,[(c['paragraphs'],c['scale'],round(c['cold_raster_us']),round(c['tile_raster_us']),c['glyph_mask_bytes']) for c in data['cases']],flush=True)
 (root/'bench-final.json').write_text(json.dumps(rows,indent=2))
summary=[]
for n in [100,1000]:
 for s in [1,1.25,1.5,1.75,2]:
  row=dict(paragraphs=n,scale=s)
  for mode in ['direct','cached']:
   cases=[c for r in rows if r['mode']==mode for c in r['result']['cases'] if c['paragraphs']==n and c['scale']==s]
   row[mode]={k:statistics.median(c[k] for c in cases) for k in ['cold_raster_us','tile_raster_us','glyph_mask_bytes','glyph_masks','mask_hits','mask_misses','mask_resets']}
   row[mode]['sha256']=cases[0]['tile_sha256'];assert all(c['tile_sha256']==cases[0]['tile_sha256'] for c in cases)
  assert row['direct']['sha256']==row['cached']['sha256'];summary.append(row)
(root/'bench-summary.json').write_text(json.dumps(summary,indent=2))
