#!/usr/bin/python3
"""Exercise production GCOFF flow with mock RM engines, interrupts, storage and clock."""
from pathlib import Path
import os,re,subprocess,sys
base=Path(__file__).resolve().parent.parent
tag=Path(sys.argv[1]).resolve()/'src/nvidia'
g=tag/'src/kernel/gpu/gpu_suspend.c'
d=tag/'arch/nvalloc/unix/src/dynamic-power.c'
def function(f,name):
    s=f.read_text()
    m=re.search(r'^(?:(?:static )?NV_STATUS )?('+name+r'\([^;{}]*?\)\s*\n\{)',s,re.M)
    assert m, name
    return 'static NV_STATUS '+s[m.start(1):s.index('\n}',m.end())+2]+'\n'

code=(base/'tests/stage-platform.h').read_text()
for f,fn in ((g,'gpuPowerManagementEnter'),(g,'gpuEnterStandby_IMPL'),(d,'RmPowerManagement'),(d,'RmGcxPowerManagement'),(d,'RmTransitionDynamicPower')):
    code+=function(f,fn)
code+=(base/'tests/stage-cases.h').read_text()
out=base/'tests/pm-stages.c';out.write_text(code)
exe=out.with_suffix('')
subprocess.run(['/usr/bin/gcc','-std=gnu11','-O2',str(out),'-o',str(exe)],env=dict(os.environ,TMPDIR=str(base/'prepare-tmp')),check=True)
subprocess.run([str(exe)],check=True,timeout=3)
