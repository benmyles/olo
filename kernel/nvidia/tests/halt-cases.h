static void scenario(const char *name, int rt, int mode, NvU64 delay, int expected, int checks)
{
    OBJGPU gpu = {0};
    KernelGsp gsp;
    td.pGpu=&gpu; td.defaultFlags=GPU_TIMEOUT_FLAGS_OSTIMER; td.defaultus=4000000;
    now=0x1011593fc4e3c0ull; parentDeadline=0x1011588a08d780ull;
    haltAt=now+delay; reads=parentChecks=0; runtime=rt;
    NvU64 start=now;
    int rc=kgspTeardown_GH100(&gpu,&gsp,mode);
    printf("%s status=0x%x parent_checks=%u register_reads=%u elapsed_virtual_ms=%llu\n",
           name,rc,parentChecks,reads,(unsigned long long)((now-start)/1000000ull));
    assert(rc == expected && !!parentChecks == checks && (now-start)<=4001000000ull);
}
int main(int argc,char **argv) {
    int patched=strcmp(argv[1],"patch")==0;
    scenario(patched?"patched late halt":"tag late halt",1,1,10000000,patched?0:0x65,!patched);
    scenario("persistent busy",1,1,8000000000ull,0x65,!patched);
    scenario("system sleep",0,1,10000000,0x65,1);
    scenario("different unload mode",1,0,10000000,0x65,1);
    scenario("already halted",1,1,0,0,0);
    return 0;
}
