#!/usr/bin/python3
from pathlib import Path
import hashlib,re,subprocess
b=Path(__file__).resolve().parent.parent
def run(*args):return subprocess.check_output(args,text=True,stderr=subprocess.DEVNULL)
print('Source object, commit, tree')
for expr in ('refs/tags/615.71.09','615.71.09^{commit}','615.71.09^{tree}'):
 print(expr,run('git','--git-dir='+str(b/'upstream.git'),'rev-parse',expr).strip())
for name in ('baseline','verified','fedora','pristine-repeat','prevention'):
 if not (b/f'{name}.modules.tsv').is_file(): continue
 print('\n'+name,(b/f'{name}.time').read_text().strip())
 print((b/f'{name}.modules.tsv').read_text(),end='')
 for p in sorted((b/name/'kernel-open').glob('*.ko')):
  build_id=re.search(r'Build ID: (\w+)',run('readelf','-n',p)).group(1)
  print(p.name,'build_id='+build_id,'sha256='+hashlib.sha256(p.read_bytes()).hexdigest())
 for f in ('src/nvidia/_out/Linux_x86_64/nv-kernel.o','src/nvidia-modeset/_out/Linux_x86_64/nv-modeset-kernel.o'):
  if (b/name/f).is_file(): print(f,(b/name/f).stat().st_size)
 print('naked-return objtool warning lines',sum('warning: objtool:' in l and 'MITIGATION_RETHUNK' in l for l in (b/f'{name}.build.log').open()))
print('\nPackaged RPM on disk, extracted into scratch')
for p in sorted((b/'stock-rpm').rglob('*.ko.xz')):
 print(p.name,p.stat().st_size,repr(run('modinfo','-F','version',p).strip()),repr(run('modinfo','-F','vermagic',p).strip()))
print('\nTracing metadata, from objects on disk')
for f,func in [('nvidia/nv.o','nv_pmops_runtime_suspend'),('nvidia/nv-pci.o','nv_pci_remove'),('nvidia-drm/nvidia-drm-drv.o','nv_drm_dev_unload'),('nvidia-modeset/nvidia-modeset-linux.o','nvkms_yield')]:
 p=b/'baseline/kernel-open'/f;s=run('objdump','-drw',p)
 m=re.search('(?m)^([a-f0-9]+) <'+func+r'>:\n((?:[^\n]+\n){3})',s)
 print(f,func, next((v.strip() for v in m[2].splitlines() if '__fentry__' in v),'no entry in first three instructions') if m else 'missing symbol')
for sub,name in [('nvidia','nv-kernel.o_binary'),('nvidia-modeset','nv-modeset-kernel.o_binary')]:
 p=b.parent/'nvsrc/kernel-open'/sub/name
 ss=run('readelf','-SW',p)
 print('akmod prelink',name,'mcount_loc', '__mcount_loc' in ss, 'patchable_function_entries','__patchable_function_entries' in ss)
