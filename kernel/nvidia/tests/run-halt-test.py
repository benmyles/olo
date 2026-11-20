#!/usr/bin/python3
"""Compile the unmodified bodies from each tree. Replace physical dependencies with a clock and a CPUCTL register."""
from pathlib import Path
import os,re,subprocess,sys
base=Path(__file__).resolve().parent.parent
env=dict(os.environ,TMPDIR=str(base/'prepare-tmp'), PATH='/usr/bin:/bin')
def body(path,name):
    s=path.read_text()
    start=s.index('\n'+name+'\n(')+1
    end=s.index('\n}',start)+2
    return s[start:end]
for arg in sys.argv[1:]:
    root=Path(arg).resolve()/'src/nvidia'
    gpu=root/'src/kernel/gpu'
    td=gpu/'gpu_timeout.c'
    code=(base/'tests/halt-platform.h').read_text()
    for line in (root/'inc/kernel/gpu/gpu_timeout.h').read_text().splitlines():
        if re.match(r'#define GPU_TIMEOUT_(?:DEFAULT|FLAGS_\w+)\s',line): code+=line+'\n'
    code+='void '+body(td,'timeoutSet')+'\nstatic NV_STATUS '+body(td,'_checkTimeout')
    code+='\nNV_STATUS '+body(td,'timeoutCheck')+'\n'
    code+='#define gpuSetTimeout(g,u,t,f) timeoutSet(&td,t,u,f)\n'
    code+='#define gpuCheckTimeout(g,t) timeoutCheck(&td,t,__LINE__)\n'
    code+='NV_STATUS '+body(gpu/'falcon/arch/ampere/kernel_falcon_ga102.c','kflcnWaitForHaltRiscv_GA102')+'\n'
    code+='#define kflcnWaitForHaltRiscv_HAL kflcnWaitForHaltRiscv_GA102\n'
    code+='NV_STATUS '+body(gpu/'gsp/arch/hopper/kernel_gsp_gh100.c','kgspTeardown_GH100')+'\n'
    code+=(base/'tests/halt-cases.h').read_text()
    p=base/'tests'/('halt-'+root.parent.parent.name+'.c');p.write_text(code)
    exe=p.with_suffix('')
    subprocess.run(['/usr/bin/gcc','-std=gnu11','-O2',str(p),'-o',str(exe)],env=env,check=True)
    is_patched='GPU_TIMEOUT_FLAGS_BYPASS_THREAD_STATE' in body(gpu/'gsp/arch/hopper/kernel_gsp_gh100.c','kgspTeardown_GH100')
    subprocess.run([str(exe),'patch' if is_patched else 'base'],env=env,timeout=2,check=True)
