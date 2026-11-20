int main(void)
{
    const char *names[]={"success","reserve","SR metadata","spent budget","test parameter","pre-unload","engine unload","post-unload","GSP halt","resume"};
    for(inject=NONE;inject<=RESUME;inject++) {
        OBJGPU g={.getProperty=get,.setProperty=set};
        int transient=0;
        intr=7; bc=1; meta=backing=sends=unloads=events[0]=events[1]=0;
        memset(&gsp,0,sizeof(gsp)); memset(&nvState,0,sizeof(nvState));
        int rc=RmTransitionDynamicPower(&g,1,&transient);
        if(inject==RESUME) {
            assert(rc==NV_OK && g.props[PDB_PROP_GPU_GCOFF_STATE_ENTERED]);
            rc=RmTransitionDynamicPower(&g,0,&transient);
        }
        if(inject==NONE) {
            assert(rc==0 && g.props[PDB_PROP_GPU_GCOFF_STATE_ENTERED] && g.bInD3Cold && !transient && !nvState.rtd3_restart_required);
        } else if(inject<=PARAM) {
            assert(rc && transient && !g.bInD3Cold && !g.props[PDB_PROP_GPU_IN_STANDBY] && !g.props[PDB_PROP_GPU_IN_PM_CODEPATH] && !g.props[PDB_PROP_GPU_GCOFF_STATE_ENTERING]);
            assert(!g.powerManagementDepth && !nvState.rtd3_restart_required && !meta && !backing && !unloads && !sends && intr==7 && bc==1 && events[0]==events[1]);
        } else {
            assert(rc && !transient && nvState.rtd3_restart_required && gsp.bFatalError && intr==0);
            if(inject!=RESUME) assert(!g.props[PDB_PROP_GPU_GCOFF_STATE_ENTERED] && !g.bInD3Cold);
        }
        printf("%-14s status=0x%02x retry=%d restart=%d depth=%d irq=%d engine_calls=%d gsp_commands=%d\n",
               names[inject],rc,transient,nvState.rtd3_restart_required,g.powerManagementDepth,intr,unloads,sends);
    }
    return 0;
}
