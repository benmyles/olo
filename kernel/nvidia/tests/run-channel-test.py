#!/usr/bin/python3
"""Use the build's declarations and link its actual leaf objects; all device storage is RAM."""
import os, pathlib, shlex, subprocess, sys
base = pathlib.Path(__file__).resolve().parent.parent
tree = pathlib.Path(sys.argv[1]).resolve()
mode = tree/'src/nvidia-modeset'
out = base/'tests'; env = dict(os.environ, PATH='/usr/bin:/bin', TMPDIR=str(base/'prepare-tmp'))
recipe = subprocess.check_output(['make','-s','-n','-W','src/nvkms-dma.c','NV_VERBOSE=1','_out/Linux_x86_64/nvkms-dma.o'],cwd=mode,text=True,env=env)
cmd = shlex.split(next(line for line in recipe.splitlines() if ' -c src/nvkms-dma.c ' in line))
opts = cmd[1:cmd.index('-c')]
# The harness itself is user-space code. Supply the thunks referenced by the
# unchanged release objects. Direct jump/return is sufficient in this process.
asm = '.text\n.globl __x86_return_thunk\n__x86_return_thunk: ret\n'
for reg in ('rax','rcx','rdx','r11'):
    asm += f'.globl __x86_indirect_thunk_{reg}\n__x86_indirect_thunk_{reg}: jmp *%{reg}\n'
(out/'test-thunks.S').write_text(asm+'.section .note.GNU-stack,"",@progbits\n')
binary = out/'channel-test'
subprocess.run(['/usr/bin/gcc',*opts,'-Wno-missing-prototypes','-no-pie','-Wl,--gc-sections','-o',str(binary),str(out/'channel-test.c'),str(out/'test-thunks.S'),'_out/Linux_x86_64/nvkms-dma.o','_out/Linux_x86_64/nvidia-push.o'],cwd=mode,env=env,check=True)
subprocess.run([str(binary)],stdout=sys.stdout,timeout=5,check=True)
