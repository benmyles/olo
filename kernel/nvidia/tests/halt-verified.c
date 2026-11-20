#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
typedef uint64_t NvU64;
typedef uint32_t NvU32;
typedef int NV_STATUS;
typedef struct gpu {int (*getProperty)(struct gpu *, int);} OBJGPU;
typedef struct {} OBJTMR;
typedef struct {} KernelFalcon;
typedef KernelFalcon KernelGsp;
typedef int KernelGspUnloadMode;
typedef struct {NvU32 flags; NvU64 timeout; OBJGPU *pTmrGpu;} RMTIMEOUT;
typedef struct {OBJGPU *pGpu; NvU32 defaultFlags, defaultus;} TIMEOUT_DATA;
static TIMEOUT_DATA td;
static NvU64 now, parentDeadline, haltAt;
static unsigned reads, parentChecks;
static int runtime;
#define NVBIT(x) (1u << (x))
#define NV_OK 0
#define NV_ERR_TIMEOUT 0x65
#define NV_ERR_INVALID_STATE 1
#define NV_FALSE 0
#define NV_MIN(a,b) ((a)<(b)?(a):(b))
#define NV_ASSERT(x) assert(x)
#define NV_ASSERT_OR_RETURN_VOID(x) do {if(!(x)) return;} while(0)
#define NV_ASSERT_OR_RETURN(x,y) do {if(!(x)) return(y);} while(0)
#define IS_GSP_CLIENT(g) 1
#define IS_FMODEL(g) 0
#define IS_EMULATION(g) 0
#define API_GPU_IN_RESET_SANITY_CHECK(g) 0
#define API_GPU_ATTACHED_SANITY_CHECK(g) 1
#define PDB_PROP_GPU_IN_PM_RESUME_CODEPATH 1
#define PDB_PROP_GPU_IN_FULLCHIP_RESET 2
#define gpumgrGetParentGPU(g) (g)
#define gpumgrIsParentGPU(g) 1
#define gpuIsGpuFullPower(g) 1
#define GPU_GET_TIMER(g) ((OBJTMR*)1)
#define portMemSet memset
#define portTimeGetUptimeNanoseconds() now
#define portTimeGetUptimeResolution() 1
#define tmrGetCurrentTime(t,d) (*(d)=now)
#define tmrDelay(t,us) (now+=(us))
#define osDelayUs(us) (now+=(us)*1000ull)
#define NV_PRINTF(...) ((void)0)
#define DBG_BREAKPOINT() ((void)0)
#define NV_RETURN_ADDRESS() 0
#define threadPriorityThrottle() ((void)0)
#define threadStateLogTimeout(...) ((void)0)
#define threadStateYieldCpuIfNecessary(...) ((void)0)
#define threadStateCheckTimeout(g,p) (parentChecks++, now>=parentDeadline?NV_ERR_TIMEOUT:NV_OK)
#define osSpinLoop() (now+=1000000ull)
#define FLD_TEST_DRF_NUM(d,r,f,v,x) ((((x)>>4)&1)==(v))
#define kflcnRiscvRegRead_HAL(g,k,r) (reads++, now >= haltAt ? 16u : 0u)
#define KGSP_UNLOAD_MODE_SR_SUSPEND 1
#define staticCast(p,t) (p)
#define rmapiInRtd3PmPath() runtime
#define GPU_TIMEOUT_DEFAULT                 0
#define GPU_TIMEOUT_FLAGS_DEFAULT               NVBIT(0)  //!< default timeout mechanism as set by platform
#define GPU_TIMEOUT_FLAGS_USE_THREAD_STATE      NVBIT(1)  //!< default timeout time used - use the ThreadState
#define GPU_TIMEOUT_FLAGS_BYPASS_THREAD_STATE   NVBIT(2)  //!< even if default time was used - skip the ThreadState
#define GPU_TIMEOUT_FLAGS_OSTIMER               NVBIT(3)  //!< osGetSystemTime()
#define GPU_TIMEOUT_FLAGS_OSDELAY               NVBIT(4)  //!< osDelay()
#define GPU_TIMEOUT_FLAGS_TMR                   NVBIT(5)  //!< tmrGetCurrentTime()
#define GPU_TIMEOUT_FLAGS_BYPASS_JOURNAL_LOG    NVBIT(6)  //!< bypass timeout logging in the RM journal
#define GPU_TIMEOUT_FLAGS_TMRDELAY              NVBIT(7)  //!< tmrDelay()
#define GPU_TIMEOUT_FLAGS_BYPASS_CPU_YIELD      NVBIT(8)  //!< don't explicitly let other threads run first
#define GPU_TIMEOUT_FLAGS_STATUS_LOCAL_TIMEOUT          NVBIT(30)
#define GPU_TIMEOUT_FLAGS_STATUS_THREAD_STATE_TIMEOUT   NVBIT(31)
void timeoutSet
(
    TIMEOUT_DATA *pTD,
    RMTIMEOUT    *pTimeout,
    NvU32         timeoutUs,
    NvU32         flags
)
{
    OBJTMR  *pTmr;
    NvU64   timeInNs;
    NvU64   timeoutNs;
    OBJGPU  *pGpu = pTD->pGpu;

    portMemSet(pTimeout, 0, sizeof(*pTimeout));

    //
    // Note that if GPU_TIMEOUT_DEFAULT is used we will go through
    // threadStateCheckTimeout rather than timeoutCheck as we do
    // not want to have "stacked" gpuSetTimeouts.  The intent of
    // GPU_TIMEOUT_DEFAULT was to cover the *entire* RM API stack.
    // If GPU_TIMEOUT_DEFAULT was specified, this is essentially a
    // NULL operation other than setting the flags to route us to
    // threadStateCheckTimeout.  This can be overridden by
    // setting GPU_TIMEOUT_FLAGS_BYPASS_THREAD_STATE.
    //
    pTimeout->flags = flags;
    if ((flags == 0) || (flags & GPU_TIMEOUT_FLAGS_DEFAULT) ||
        !(flags & (GPU_TIMEOUT_FLAGS_OSTIMER | GPU_TIMEOUT_FLAGS_OSDELAY |
                   GPU_TIMEOUT_FLAGS_TMR | GPU_TIMEOUT_FLAGS_TMRDELAY)))
    {
        pTimeout->flags |= pTD->defaultFlags;
    }

    if (timeoutUs == GPU_TIMEOUT_DEFAULT)
    {
        timeoutUs = pTD->defaultus;

        //
        // Use the ThreadState by default if GPU_TIMEOUT_DEFAULT was specified
        // unless we were told explicitly not to.
        // ThreadState only supports OSTIMER and OSDELAY
        //
        if (!(pTimeout->flags & GPU_TIMEOUT_FLAGS_BYPASS_THREAD_STATE) &&
             (pTimeout->flags & (GPU_TIMEOUT_FLAGS_OSTIMER | GPU_TIMEOUT_FLAGS_OSDELAY)))
        {
            pTimeout->flags |= GPU_TIMEOUT_FLAGS_USE_THREAD_STATE;
        }
    }

    // Set end time for elapsed time methods
    timeoutNs = (NvU64)timeoutUs * 1000;

    if (pTimeout->flags & GPU_TIMEOUT_FLAGS_OSTIMER)
    {
        // WAR for FMODEL: Increase timeout by 10x to handle slow GSPRM on FMODEL on CPURM
        if (IS_GSP_CLIENT(pGpu) && IS_FMODEL(pGpu))
        {
            timeoutNs *= 10;
        }

        //
        // For small timeouts (timeout durations on the order of magnitude of
        // the OS tick resolution), starting the timeout near the end of a tick
        // could cause a premature timeout since the start time is determined
        // by the start of the tick. Mitigate this by always padding the
        // timeout using the OS tick resolution, to bump us to the next tick.
        //
        timeoutNs += portTimeGetUptimeResolution();

        timeInNs = portTimeGetUptimeNanoseconds();

        pTimeout->pTmrGpu = NULL;
        pTimeout->timeout = timeInNs + timeoutNs;
    }
    else if ((pTimeout->flags & GPU_TIMEOUT_FLAGS_TMR) ||
             (pTimeout->flags & GPU_TIMEOUT_FLAGS_TMRDELAY))
    {
        NV_ASSERT_OR_RETURN_VOID(pGpu != NULL);

        OBJGPU *pParentGpu = gpumgrGetParentGPU(pGpu);

        //
        // Set timer GPU to primary GPU for accurate timeout with SLI loop. But only
        // use the primary GPU if it is in full power mode or in the process of resuming.
        // Also don't use the primary if it is in full chip reset.
        //
        if (gpumgrIsParentGPU(pGpu) ||
            ((gpuIsGpuFullPower(pParentGpu) == NV_FALSE) &&
             !pParentGpu->getProperty(pParentGpu, PDB_PROP_GPU_IN_PM_RESUME_CODEPATH)) ||
            pParentGpu->getProperty(pParentGpu, PDB_PROP_GPU_IN_FULLCHIP_RESET))
        {
            pTimeout->pTmrGpu = pGpu;
        }
        else
        {
            pTimeout->pTmrGpu = pParentGpu;
        }

        pTmr = GPU_GET_TIMER(pTimeout->pTmrGpu);
        NV_ASSERT_OR_RETURN_VOID(pTmr != NULL);

        if (pTimeout->flags & GPU_TIMEOUT_FLAGS_TMR)
        {

            // nanoseconds
            tmrGetCurrentTime(pTmr, &pTimeout->timeout);
            pTimeout->timeout += timeoutNs;
        }
        else // GPU_TIMEOUT_FLAGS_TMRDELAY
        {
            pTimeout->timeout = timeoutUs;
        }
    }
    else
    {
        pTimeout->pTmrGpu = NULL;
        pTimeout->timeout = timeoutUs;
    }
}
static NV_STATUS _checkTimeout
(
    RMTIMEOUT  *pTimeout
)
{
    NV_STATUS status = NV_OK;
    OBJTMR   *pTmr;
    NvU64     current;
    NvU64     timeInNs;

    if (pTimeout->flags & GPU_TIMEOUT_FLAGS_OSTIMER)
    {
        timeInNs = portTimeGetUptimeNanoseconds();
        if (timeInNs >= pTimeout->timeout)
        {
            if (!(pTimeout->flags & GPU_TIMEOUT_FLAGS_BYPASS_JOURNAL_LOG))
            {
                NV_PRINTF(LEVEL_INFO, "OS elapsed %llx >= %llx\n",
                          timeInNs, pTimeout->timeout);
            }
            status = NV_ERR_TIMEOUT;
        }
    }
    else if (pTimeout->flags & GPU_TIMEOUT_FLAGS_OSDELAY)
    {
        osDelayUs(100);

        //
        // TODO: Bug: 3312158 - Isolate the fix timeout logic to emulation.
        // This is because of the numerous timeout issues exposed in DVS
        // Emulation requires this to make sure we are not wasting emulation resources
        // by waiting for timeouts too long.
        // Once DVS issues are fixed, this fix will be enabled for all platforms.
        //
        if ((pTimeout->pTmrGpu != NULL) && (IS_EMULATION(pTimeout->pTmrGpu)))
        {
            //
            // Adjust the remaining time. Note that the remaining time is in nanoseconds unit
            // for GPU_TIMEOUT_FLAGS_OSDELAY
            //
            pTimeout->timeout -= NV_MIN(100ULL * 1000ULL, pTimeout->timeout);
        }
        else
        {
            pTimeout->timeout -= NV_MIN(100ULL , pTimeout->timeout);
        }

        if (pTimeout->timeout == 0)
        {
            if (!(pTimeout->flags & GPU_TIMEOUT_FLAGS_BYPASS_JOURNAL_LOG))
            {
                NV_PRINTF(LEVEL_INFO, "OS timeout == 0\n");
            }
            status =  NV_ERR_TIMEOUT;
        }
    }
    else if (pTimeout->flags & GPU_TIMEOUT_FLAGS_TMR)
    {
        NV_ASSERT_OR_RETURN(pTimeout->pTmrGpu != NULL, NV_ERR_INVALID_STATE);
        if (!API_GPU_ATTACHED_SANITY_CHECK(pTimeout->pTmrGpu))
            return NV_ERR_TIMEOUT;

        pTmr = GPU_GET_TIMER(pTimeout->pTmrGpu);
        NV_ASSERT_OR_RETURN(pTmr != NULL, NV_ERR_INVALID_STATE);

        tmrDelay(pTmr, 5ULL * 1000ULL);
        tmrGetCurrentTime(pTmr, &current);

        if (current >= pTimeout->timeout)
        {
            if (!(pTimeout->flags & GPU_TIMEOUT_FLAGS_BYPASS_JOURNAL_LOG))
            {
                NV_PRINTF(LEVEL_ERROR, "ptmr elapsed %llx >= %llx\n",
                          current, pTimeout->timeout);
            }
            status =  NV_ERR_TIMEOUT;
        }
    }
    else if (pTimeout->flags & GPU_TIMEOUT_FLAGS_TMRDELAY)
    {
        NV_ASSERT_OR_RETURN(pTimeout->pTmrGpu != NULL, NV_ERR_INVALID_STATE);
        if (!API_GPU_ATTACHED_SANITY_CHECK(pTimeout->pTmrGpu))
            return NV_ERR_TIMEOUT;

        pTmr = GPU_GET_TIMER(pTimeout->pTmrGpu);
        NV_ASSERT_OR_RETURN(pTmr != NULL, NV_ERR_INVALID_STATE);

        tmrDelay(pTmr, 5ULL * 1000ULL);
        pTimeout->timeout -= NV_MIN(5, pTimeout->timeout);

        if (pTimeout->timeout == 0)
        {
            if (!(pTimeout->flags & GPU_TIMEOUT_FLAGS_BYPASS_JOURNAL_LOG))
            {
                NV_PRINTF(LEVEL_INFO, "ptmr timeout == 0\n");
            }
            status =  NV_ERR_TIMEOUT;
        }
    }
    else
    {
        NV_PRINTF(LEVEL_ERROR, "Invalid timeout flags 0x%08x\n",
                  pTimeout->flags);
        DBG_BREAKPOINT();
        status = NV_ERR_INVALID_STATE;
    }

    return status;
}
NV_STATUS timeoutCheck
(
    TIMEOUT_DATA *pTD,
    RMTIMEOUT    *pTimeout,
    NvU32         lineNum
)
{
    OBJGPU    *pGpu = pTD->pGpu;
    NV_STATUS  status = NV_OK;

    NV_ASSERT(pTimeout != NULL);

    if ((pGpu != NULL) && API_GPU_IN_RESET_SANITY_CHECK(pGpu))
        return NV_ERR_TIMEOUT;

    if (!(pTimeout->flags & GPU_TIMEOUT_FLAGS_BYPASS_CPU_YIELD))
    {
        threadStateYieldCpuIfNecessary(pGpu, !!(pTimeout->flags & GPU_TIMEOUT_FLAGS_BYPASS_JOURNAL_LOG));
    }

    //
    // Note that if GPU_TIMEOUT_DEFAULT is used we will go through
    // threadStateCheckTimeout rather than timeoutCheck as we do
    // not want to have "stacked" gpuSetTimeouts.  The intent of
    // GPU_TIMEOUT_DEFAULT is to cover the *entire* RM API stack.
    // If we are going through the case below, we should have just
    // called threadStateCheckTimeout directly rather than
    // timeoutCheck.
    //

    // If local timeout check was intended, check that first.
    if (!(pTimeout->flags & GPU_TIMEOUT_FLAGS_USE_THREAD_STATE))
    {
        status = _checkTimeout(pTimeout);
        if (status == NV_ERR_TIMEOUT)
        {
            // Mark that this Timeout is the result of a local timeout
            pTimeout->flags |= GPU_TIMEOUT_FLAGS_STATUS_LOCAL_TIMEOUT;
        }
    }

    //
    // Always check for the thread timeout in addition to any local timeout
    // unless we have EXPLICITLY been instructed not to by a timeout flag.
    //
    if ((status != NV_ERR_TIMEOUT) && !(pTimeout->flags & GPU_TIMEOUT_FLAGS_BYPASS_THREAD_STATE))
    {
        status = threadStateCheckTimeout(pGpu, NULL /*pElapsedTime*/);

        if (status == NV_ERR_TIMEOUT)
        {
            // Mark that this Timeout is the result of ThreadState
            pTimeout->flags |= GPU_TIMEOUT_FLAGS_STATUS_THREAD_STATE_TIMEOUT;
        }
        else if (status != NV_OK)
        {
            // Try the local timeout as fallback, unless it was already checked.
            if (pTimeout->flags & GPU_TIMEOUT_FLAGS_USE_THREAD_STATE)
            {
                status = _checkTimeout(pTimeout);
                if (status == NV_ERR_TIMEOUT)
                {
                    // Mark that this Timeout is the result of a local timeout
                    pTimeout->flags |= GPU_TIMEOUT_FLAGS_STATUS_LOCAL_TIMEOUT;
                }
            }
        }
    }

    // Throttle priority of boosted threads if necessary
    threadPriorityThrottle();

    // Log the Timeout in the RM Journal
    if ( (status == NV_ERR_TIMEOUT) &&
        !(pTimeout->flags & GPU_TIMEOUT_FLAGS_BYPASS_JOURNAL_LOG))
    {
        NvU64 funcAddr = (NvU64) (NV_RETURN_ADDRESS());
        threadStateLogTimeout(pGpu, funcAddr, lineNum);
    }

    return status;
}
#define gpuSetTimeout(g,u,t,f) timeoutSet(&td,t,u,f)
#define gpuCheckTimeout(g,t) timeoutCheck(&td,t,__LINE__)
NV_STATUS kflcnWaitForHaltRiscv_GA102
(
    OBJGPU *pGpu,
    KernelFalcon *pKernelFlcn,
    NvU32 timeoutUs,
    NvU32 flags
)
{
    NV_STATUS status = NV_OK;
    RMTIMEOUT timeout;

    gpuSetTimeout(pGpu, timeoutUs, &timeout, flags);

    while (!FLD_TEST_DRF_NUM(_PRISCV, _RISCV, _CPUCTL_HALTED, 0x1,
                             kflcnRiscvRegRead_HAL(pGpu, pKernelFlcn, NV_PRISCV_RISCV_CPUCTL)))
    {
        status = gpuCheckTimeout(pGpu, &timeout);
        if (status == NV_ERR_TIMEOUT)
        {
            NV_PRINTF(LEVEL_ERROR, "Timeout waiting for RISC-V to halt\n");
            DBG_BREAKPOINT();
            break;
        }
        osSpinLoop();
    }

    return status;
}
#define kflcnWaitForHaltRiscv_HAL kflcnWaitForHaltRiscv_GA102
NV_STATUS kgspTeardown_GH100
(
    OBJGPU *pGpu,
    KernelGsp *pKernelGsp,
    KernelGspUnloadMode unloadMode
)
{
    NvU32 flags = 0;

    // Runtime suspend may have spent its thread budget saving state. The
    // GSP has already been asked to suspend; allow it time to finish.
    if (unloadMode == KGSP_UNLOAD_MODE_SR_SUSPEND && rmapiInRtd3PmPath())
        flags = GPU_TIMEOUT_FLAGS_OSTIMER | GPU_TIMEOUT_FLAGS_BYPASS_THREAD_STATE;

    return kflcnWaitForHaltRiscv_HAL(pGpu, staticCast(pKernelGsp, KernelFalcon),
                                     GPU_TIMEOUT_DEFAULT, flags);
}
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
