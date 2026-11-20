#include <assert.h>
#include <stdio.h>
#include <string.h>
#include <stdint.h>
typedef int NvBool, NvU32, NV_STATUS, nv_pm_action_t, KernelGspUnloadMode;
typedef uint64_t NvU64;
enum { NV_OK=0, NV_ERR_NO_MEMORY=0x51, NV_ERR_TIMEOUT=0x65, NV_ERR_BUSY_RETRY=3,
       NV_ERR_GENERIC=0xffff, NV_ERR_NOT_SUPPORTED=0x56, NV_ERR_INVALID_ARGUMENT=0x1f };
enum { NV_PM_DEPTH_NONE,NV_PM_DEPTH_OS_LAYER,NV_PM_DEPTH_SR_META,NV_PM_DEPTH_STATE_LOAD };
enum { PDB_PROP_GPU_GCOFF_STATE_ENTERING, PDB_PROP_GPU_GCOFF_STATE_ENTERED,
       PDB_PROP_GPU_PREINITIALIZED_WPR_REGION, PDB_PROP_GPU_SC7_SUPPORTED,
       PDB_PROP_GPU_VGA_ENABLED, PDB_PROP_GPU_IN_PM_CODEPATH, PDB_PROP_GPU_IN_STANDBY,
       PDB_PROP_PFM_SUPPORTS_ACPI, PDB_PROP_GPU_IS_MOBILE, PDB_PROP_GPU_RTD3_GC6_SUPPORTED,
       PDB_PROP_GPU_RTD3_RG_SUPPORTED, PDB_PROP_GPU_RG_STATE_ENTERING, PDB_PROP_GPU_RG_STATE_ENTERED };
enum { NV_PM_ACTION_STANDBY,NV_PM_ACTION_HIBERNATE,NV_PM_ACTION_RESUME };
enum { KGSP_UNLOAD_MODE_SR_SUSPEND, KGSP_UNLOAD_MODE_GC6_ENTER,KGSP_UNLOAD_MODE_SR_WITH_WPR_IN_SYSMEM };
enum { NV2080_NOTIFIERS_POWER_SUSPEND, NV2080_NOTIFIERS_POWER_RESUME };
enum { NONE, RESERVE, SR, SPENT, PARAM, PRE, ENGINE, POST, HALT, RESUME };
static int inject, intr, meta, backing, unloads, sends, events[2], bc;
typedef struct G {struct {struct {int bSBIOSCaps;} jtMethodData;} acpiMethodData;
    NvBool (*getProperty)(struct G*,int); void (*setProperty)(struct G*,int,int);
    int props[32],powerManagementDepth,bInD3Cold;} OBJGPU;
typedef OBJGPU OBJPFM; typedef int OBJSYS, Intr;
typedef struct {int bTestDeferGcoff,bFatalError;} KernelGsp;
typedef struct {struct {void *sectionHandle;} pagedBufferInfo;} OBJFBSR;
typedef struct {int fixedFbsrModesMask,bUseVirtualCopyOnSuspend;OBJFBSR *pFbsr[2];} MemoryManager;
typedef struct {int bPreserveComptagBackingStoreOnSuspend;} KernelMemorySystem;
typedef struct {int rtd3_restart_required,flags,d0_state_in_suspend,preserve_vidmem_allocations;} nv_state_t;
typedef struct {int gc6_upstream_port_configured;struct {int InHibernate,IntrEn;} pm_state;} nv_priv_t;
typedef struct {int flavorId,stepMask;struct {int bIsRTD3Transition,bSkipPstateSanity;} params;} NV2080_CTRL_GC6_ENTRY_PARAMS;
typedef NV2080_CTRL_GC6_ENTRY_PARAMS NV2080_CTRL_GC6_EXIT_PARAMS;
static KernelGsp gsp; static MemoryManager mm; static KernelMemorySystem ms; static nv_state_t nvState; static nv_priv_t priv;
static int get(OBJGPU*g,int p){return g->props[p];}
static void set(OBJGPU*g,int p,int v){g->props[p]=v;}
#define NV_FALSE 0
#define NV_TRUE 1
#define SYS_GET_INSTANCE() NULL
#define SYS_GET_PFM(s) NULL
#define NV_GET_NV_STATE(g) (&nvState)
#define NV_GET_NV_PRIV(nv) (&priv)
#define GPU_GET_KERNEL_GSP(g) (&gsp)
#define GPU_GET_MEMORY_MANAGER(g) (&mm)
#define GPU_GET_KERNEL_MEMORY_SYSTEM(g) (&ms)
#define GPU_GET_INTR(g) NULL
#define IS_GSP_CLIENT(g) 1
#define IS_GPU_GC6_STATE_ENTERING(g) 0
#define NV_ASSERT_OK_OR_GOTO(st,step,label) do {if(((st)=(step))!=NV_OK) goto label;} while(0)
#define NV_PRINTF(...) ((void)0)
#define NV_DEV_PRINTF(...) ((void)0)
#define _gpuGspPrepareSuspendResumeDataFailureStore(...) ((void)0)
#define _gpuGspUnloadRmFailureStore(...) ((void)0)
#define kgspPrepareSuspendResumeData_HAL(...) (inject==SR?NV_ERR_NO_MEMORY:(meta=1,NV_OK))
#define kgspFreeSuspendResumeData_HAL(...) (meta=0)
#define rmapiInRtd3PmPath() 1
#define threadStateCheckTimeout(...) (inject==SPENT?NV_ERR_TIMEOUT:NV_OK)
#define gpuPowerManagementEnterPreUnloadPhysical(...) (inject==PRE?NV_ERR_TIMEOUT:NV_OK)
#define gpuStateUnload(...) (unloads++,inject==ENGINE?NV_ERR_TIMEOUT:NV_OK)
#define gpuPowerManagementEnterPostUnloadPhysical(...) (inject==POST?NV_ERR_TIMEOUT:NV_OK)
#define kgspUnloadRm(...) (sends++,inject==HALT?NV_ERR_TIMEOUT:NV_OK)
#define kpmuFreeLibosLoggingStructures(...) ((void)0)
#define memmgrFreeFbsrMemory(...) ((void)0)
#define gpuNotifySubDeviceEvent(g,e,...) (++events[e])
#define gpuEnterStandby gpuEnterStandby_IMPL
#define gpuEnterHibernate(...) (assert(0),NV_OK)
#define gpuResumeFromHibernate(...) assert(0)
#define gpuResumeFromStandby(...) (inject==RESUME?NV_ERR_TIMEOUT:NV_OK)
#define gpumgrGetBcEnabledStatus(g) bc
#define gpumgrSetBcEnabledStatus(g,v) (bc=(v))
#define intrGetIntrEn(i) intr
#define intrSetIntrEn(i,v) (intr=(v))
#define IsMobile(g) 0
#define INTERRUPT_TYPE_DISABLED 0
#define nv_acpi_get_powersource(...) 0
#define RmPowerSourceChangeEvent(...) ((void)0)
#define RmRequestDNotifierState(...) ((void)0)
#define RmUpdateFixedFbsrModes(g) NV_OK
#define osCloseFile(...) ((void)0)
#define nv_s2idle_pm_configured() 0
#define memmgrGetUsedRamSize(...) NV_OK
#define RmCheckForGcOffPM(...) 1
#define fbsrReserveSysMemoryForPowerMgmt(...) (inject==RESERVE?NV_ERR_NO_MEMORY:(backing=1,NV_OK))
#define fbsrFreeReservedSysMemoryForPowerMgmt(...) (backing=0)
#define nv_test_defer_gcoff(...) (inject==PARAM)
#define portMemSet memset
#define portAtomicSetU32(p,v) (*(p)=(v))
#define gpuGc6Entry(...) (assert(0),NV_OK)
#define gpuGc6Exit(...) (assert(0),NV_OK)
#define nv_idle_holdoff(...) ((void)0)
#define RmScheduleCallbackToRemoveIdleHoldoff(...) ((void)0)
#define NVBIT(i) (1u<<(i))
enum { GPU_STATE_FLAGS_PM_SUSPEND=1, GPU_STATE_FLAGS_PRESERVING=2, GPU_STATE_FLAGS_PM_TRANSITION=4,
       GPU_STATE_FLAGS_GC6_TRANSITION=8,NV2080_CTRL_GPU_SET_POWER_STATE_GPU_LEVEL_3=3,
       FBSR_TYPE_DMA=0,FBSR_TYPE_FILE=1,NV2080_CTRL_GC6_FLAVOR_ID_MSHYBRID=0,
       NV2080_CTRL_GC6_STEP_ID_GPU_OFF=0,NV_FLAG_PASSTHRU=4 };
static NV_STATUS gpuPowerManagementEnter(OBJGPU *pGpu, NvU32 newLevel, NvU32 flags)
{
    NV_STATUS  status = NV_OK;
    MemoryManager *pMemoryManager = GPU_GET_MEMORY_MANAGER(pGpu);

    pGpu->powerManagementDepth = NV_PM_DEPTH_OS_LAYER;

    KernelGsp *pKernelGsp = GPU_GET_KERNEL_GSP(pGpu);
    KernelGspUnloadMode unloadMode = KGSP_UNLOAD_MODE_SR_SUSPEND;

    if (IS_GSP_CLIENT(pGpu))
    {
        if (IS_GPU_GC6_STATE_ENTERING(pGpu))
        {
            unloadMode = KGSP_UNLOAD_MODE_GC6_ENTER;
        }
        else if ((pGpu->getProperty(pGpu, PDB_PROP_GPU_PREINITIALIZED_WPR_REGION)) &&
                 (flags & GPU_STATE_FLAGS_PM_SUSPEND) &&
                 (!pGpu->getProperty(pGpu, PDB_PROP_GPU_SC7_SUPPORTED)))
        {
            // Mode is specifc to S3, as WPR carveout remains preserved
            unloadMode = KGSP_UNLOAD_MODE_SR_WITH_WPR_IN_SYSMEM;
        }
        else
        {
            unloadMode = KGSP_UNLOAD_MODE_SR_SUSPEND;

            // Prepare SR metadata structure for suspend
            status = kgspPrepareSuspendResumeData_HAL(pGpu, pKernelGsp);
            if (status != NV_OK)
            {
                _gpuGspPrepareSuspendResumeDataFailureStore(
                    pGpu, status);
                return status;
            }
        }

        // The engines and firmware are still running. Do not start engine
        // unload with a budget already consumed by allocating the save area.
        if (rmapiInRtd3PmPath() &&
            pGpu->getProperty(pGpu, PDB_PROP_GPU_GCOFF_STATE_ENTERING) &&
            (pKernelGsp->bTestDeferGcoff ||
             threadStateCheckTimeout(pGpu, NULL) == NV_ERR_TIMEOUT))
        {
            if (unloadMode == KGSP_UNLOAD_MODE_SR_SUSPEND)
                kgspFreeSuspendResumeData_HAL(pGpu, pKernelGsp);
            return NV_ERR_BUSY_RETRY;
        }

        pGpu->powerManagementDepth = NV_PM_DEPTH_SR_META;
    }

    // This is a no-op in CPU-RM
    NV_ASSERT_OK_OR_GOTO(status, gpuPowerManagementEnterPreUnloadPhysical(pGpu), done);

    NV_ASSERT_OK_OR_GOTO(status, gpuStateUnload(pGpu,
        IS_GPU_GC6_STATE_ENTERING(pGpu) ?
        GPU_STATE_FLAGS_PRESERVING | GPU_STATE_FLAGS_PM_TRANSITION | GPU_STATE_FLAGS_GC6_TRANSITION :
        GPU_STATE_FLAGS_PRESERVING | GPU_STATE_FLAGS_PM_TRANSITION), done);

    pGpu->powerManagementDepth = NV_PM_DEPTH_STATE_LOAD;
    pGpu->setProperty(pGpu, PDB_PROP_GPU_VGA_ENABLED, NV_TRUE);

    // This is a no-op in CPU-RM
    NV_ASSERT_OK_OR_GOTO(status, gpuPowerManagementEnterPostUnloadPhysical(pGpu, newLevel), done);

    if (IS_GSP_CLIENT(pGpu))
    {
        status = kgspUnloadRm(pGpu, pKernelGsp, unloadMode, newLevel);
        if (status != NV_OK)
        {
            NV_PRINTF(LEVEL_ERROR, "GSP unload failed at suspend (bootMode 0x%x, newLevel 0x%x): 0x%x\n",
                      unloadMode, newLevel, status);
            _gpuGspUnloadRmFailureStore(
                pGpu, status);
            goto done;
        }

        if (!IS_GPU_GC6_STATE_ENTERING(pGpu))
        {
            kpmuFreeLibosLoggingStructures(pGpu, GPU_GET_KERNEL_PMU(pGpu));
        }
    }

done:
    if ((status != NV_OK) && !IS_GPU_GC6_STATE_ENTERING(pGpu))
    {
        memmgrFreeFbsrMemory(pGpu, pMemoryManager);
    }

    return status;
}
static NV_STATUS gpuEnterStandby_IMPL(OBJGPU *pGpu)
{
    OBJSYS       *pSys    = SYS_GET_INSTANCE();
    OBJPFM       *pPfm    = SYS_GET_PFM(pSys);
    NV_STATUS     suspendStatus;

    if ((pPfm != NULL) && pPfm->getProperty(pPfm, PDB_PROP_PFM_SUPPORTS_ACPI))
    {
        NV_PRINTF(LEVEL_INFO,
                  "gpuPowerState NV2080_CTRL_GPU_SET_POWER_STATE_GPU_LEVEL_3 Requested\n");
        NV_PRINTF(LEVEL_NOTICE, "Beginning transition from D0 to %s\n",
                  IS_GPU_GC6_STATE_ENTERING(pGpu) ? "GC6" : "D3");
    }
    else
    {
        NV_PRINTF(LEVEL_INFO,
                  "gpuPowerState NV2080_CTRL_GPU_SET_POWER_STATE_GPU_LEVEL_4 Requested\n");
        NV_PRINTF(LEVEL_NOTICE, "Beginning transition to %s\n",
                  IS_GPU_GC6_STATE_ENTERING(pGpu) ? "GC6" : "APM Suspend");
    }

    pGpu->setProperty(pGpu, PDB_PROP_GPU_IN_PM_CODEPATH, NV_TRUE);

    gpuNotifySubDeviceEvent(pGpu, NV2080_NOTIFIERS_POWER_SUSPEND, NULL, 0, 0, 0);

    suspendStatus = gpuPowerManagementEnter(pGpu, NV2080_CTRL_GPU_SET_POWER_STATE_GPU_LEVEL_3, GPU_STATE_FLAGS_PM_SUSPEND);

    if (suspendStatus != NV_OK && IS_GSP_CLIENT(pGpu) && rmapiInRtd3PmPath() &&
        pGpu->getProperty(pGpu, PDB_PROP_GPU_GCOFF_STATE_ENTERING) &&
        pGpu->powerManagementDepth == NV_PM_DEPTH_OS_LAYER)
    {
        // No engine unload, PM transition RPC or firmware save was started.
        pGpu->setProperty(pGpu, PDB_PROP_GPU_IN_PM_CODEPATH, NV_FALSE);
        pGpu->powerManagementDepth = NV_PM_DEPTH_NONE;
        return suspendStatus;
    }

    if (suspendStatus != NV_OK && rmapiInRtd3PmPath() && IS_GSP_CLIENT(pGpu))
        return suspendStatus;

    pGpu->setProperty(pGpu, PDB_PROP_GPU_IN_STANDBY, NV_TRUE);
    pGpu->bInD3Cold = NV_TRUE;

    if ((pPfm !=NULL) && pPfm->getProperty(pPfm, PDB_PROP_PFM_SUPPORTS_ACPI))
    {
        NV_PRINTF(LEVEL_NOTICE, "Ending transition from D0 to %s\n",
                  IS_GPU_GC6_STATE_ENTERING(pGpu) ? "GC6" : "D3");
    }
    else
    {
        NV_PRINTF(LEVEL_NOTICE, "Ending transition to %s\n",
                  IS_GPU_GC6_STATE_ENTERING(pGpu) ? "GC6" : "APM Suspend");
    }

    return suspendStatus;
}
static NV_STATUS RmPowerManagement(
    OBJGPU *pGpu,
    nv_pm_action_t pmAction
)
{
    // default to NV_OK. there may cases where resman is loaded, but
    // no devices are allocated (we're still at the console). in these
    // cases, it's fine to let the system do whatever it wants.
    NV_STATUS rmStatus = NV_OK;

    if (pGpu)
    {
        nv_state_t *nv = NV_GET_NV_STATE(pGpu);
        nv_priv_t  *nvp = NV_GET_NV_PRIV(nv);
        NvBool bcState = gpumgrGetBcEnabledStatus(pGpu);
        Intr *pIntr = GPU_GET_INTR(pGpu);
        MemoryManager *pMemoryManager = GPU_GET_MEMORY_MANAGER(pGpu);

        if ((pmAction == NV_PM_ACTION_HIBERNATE) || (pmAction == NV_PM_ACTION_STANDBY))
        {
            //
            // pFb object store the FBSR mode through which FB state unload has happened,
            // so os layer doesn't need to set FBSR mode on resume.
            //
            rmStatus = RmUpdateFixedFbsrModes(pGpu);
            if (rmStatus != NV_OK)
            {
                pMemoryManager->fixedFbsrModesMask = 0;
                return rmStatus;
            }
        }
        else
        {
            if (pGpu->powerManagementDepth == NV_PM_DEPTH_NONE)
            {
                return rmStatus;
            }
        }

        switch (pmAction)
        {
            case NV_PM_ACTION_HIBERNATE:
                nvp->pm_state.InHibernate = NV_TRUE;
                nvp->pm_state.IntrEn = intrGetIntrEn(pIntr);
                intrSetIntrEn(pIntr, INTERRUPT_TYPE_DISABLED);
                gpumgrSetBcEnabledStatus(pGpu, NV_FALSE);

                rmStatus = gpuEnterHibernate(pGpu);
                gpumgrSetBcEnabledStatus(pGpu, bcState);

                break;

            case NV_PM_ACTION_STANDBY:
                nvp->pm_state.IntrEn = intrGetIntrEn(pIntr);
                intrSetIntrEn(pIntr, INTERRUPT_TYPE_DISABLED);
                gpumgrSetBcEnabledStatus(pGpu, NV_FALSE);

                rmStatus = gpuEnterStandby(pGpu);
                gpumgrSetBcEnabledStatus(pGpu, bcState);
                if (rmStatus != NV_OK && IS_GSP_CLIENT(pGpu) && rmapiInRtd3PmPath() &&
                    pGpu->getProperty(pGpu, PDB_PROP_GPU_GCOFF_STATE_ENTERING) &&
                    pGpu->powerManagementDepth == NV_PM_DEPTH_NONE)
                {
                    intrSetIntrEn(pIntr, nvp->pm_state.IntrEn);
                    gpuNotifySubDeviceEvent(pGpu, NV2080_NOTIFIERS_POWER_RESUME,
                                            NULL, 0, 0, 0);
                }

                break;

            case NV_PM_ACTION_RESUME:
                gpumgrSetBcEnabledStatus(pGpu, NV_FALSE);

                if (nvp->pm_state.InHibernate)
                {
                    gpuResumeFromHibernate(pGpu);
                    nvp->pm_state.InHibernate = NV_FALSE;
                }
                else
                {
                    NV_STATUS resumeStatus = gpuResumeFromStandby(pGpu);
                    if (rmapiInRtd3PmPath() && IS_GSP_CLIENT(pGpu) && resumeStatus != NV_OK)
                    {
                        rmStatus = resumeStatus;
                        gpumgrSetBcEnabledStatus(pGpu, bcState);
                        break;
                    }
                }
                intrSetIntrEn(pIntr, nvp->pm_state.IntrEn);
                gpumgrSetBcEnabledStatus(pGpu, bcState);
                NvU32 ac_plugged = 0;

                if (IsMobile(pGpu))
                {
                    if (nv_acpi_get_powersource(&ac_plugged) == NV_OK)
                    {
                        //
                        // As we have already acquired API and GPU lock here, we are
                        // directly calling RmPowerSourceChangeEvent.
                        //
                        RmPowerSourceChangeEvent(nv, !ac_plugged);
                    }
                    RmRequestDNotifierState(nv);
                }
                break;

            default:
                rmStatus = NV_ERR_INVALID_ARGUMENT;
                break;
        }

        pMemoryManager->fixedFbsrModesMask = 0;

        //
        // The FBSR temp file handle may have remained open on a failed
        // suspend. Close it.
        //
        if (rmStatus != NV_OK)
        {
            OBJFBSR *pFbsr = pMemoryManager->pFbsr[FBSR_TYPE_FILE];
            if (pFbsr != NULL && pFbsr->pagedBufferInfo.sectionHandle != NULL)
            {
                osCloseFile(pFbsr->pagedBufferInfo.sectionHandle);
                pFbsr->pagedBufferInfo.sectionHandle = NULL;
            }
        }
    }

    return rmStatus;
}
static NV_STATUS RmGcxPowerManagement(
    OBJGPU *pGpu,
    NvBool  bEnter,
    NvBool  bIsDynamicPM,
    NvBool *bTryAgain
)
{
    MemoryManager *pMemoryManager           = GPU_GET_MEMORY_MANAGER(pGpu);
    KernelMemorySystem *pKernelMemorySystem = GPU_GET_KERNEL_MEMORY_SYSTEM(pGpu);
    nv_state_t         *nv                  = NV_GET_NV_STATE(pGpu);
    nv_priv_t          *nvp                 = NV_GET_NV_PRIV(nv);
    NV_STATUS           status              = NV_OK;

    if (pGpu->acpiMethodData.jtMethodData.bSBIOSCaps &&
        !pGpu->getProperty(pGpu, PDB_PROP_GPU_IS_MOBILE))
    {
        //
        // AML override is expected only on NB platforms to support GCOFF-1.0(
        // which is legacy GCOFF) and GC6-3.0
        // GC6 3.0 is possible without AML override but due to changes required
        // for GCOFF-1.0 in SBIOS & HW, AML override is needed for GC6-3.0 also.
        //
        NV_PRINTF(LEVEL_INFO,"AML overrides present in Desktop");
    }

    nv->d0_state_in_suspend = NV_FALSE;

    if (bEnter)
    {
        NvU64          usedFbSize = 0;
        NvBool         bCanUseGc6 = NV_FALSE;
        NV_STATUS      fbsrStatus = NV_ERR_GENERIC;

        //
        // If the GPU supports GC6, then check if GC6 can be used for
        // the current power management request.
        // 1. For dynamic PM, GC6 can be used if it is supported by the GPU.
        // 2. For system PM with s2idle, GC6 can be used if it is
        //    supported by the GPU.
        //
        if (pGpu->getProperty(pGpu, PDB_PROP_GPU_RTD3_GC6_SUPPORTED) &&
            nvp->gc6_upstream_port_configured)
        {
            bCanUseGc6 = bIsDynamicPM ? NV_TRUE : nv_s2idle_pm_configured();
        }

        if (pGpu->getProperty(pGpu, PDB_PROP_GPU_RTD3_RG_SUPPORTED))
        {
            pGpu->setProperty(pGpu, PDB_PROP_GPU_RG_STATE_ENTERING, NV_TRUE);
            status = RmPowerManagement(pGpu, NV_PM_ACTION_STANDBY);
            pGpu->setProperty(pGpu, PDB_PROP_GPU_RG_STATE_ENTERING, NV_FALSE);
            if (status == NV_OK)
            {
                pGpu->setProperty(pGpu, PDB_PROP_GPU_RG_STATE_ENTERED, NV_TRUE);
            }
        }
        //
        // If GC6 cannot be used, then no need to compare the used FB size with
        // threshold value and select GCOFF irrespective of FB size.
        //
        else if ((memmgrGetUsedRamSize(pGpu, pMemoryManager, &usedFbSize) == NV_OK) &&
            (!bCanUseGc6 || RmCheckForGcOffPM(pGpu, usedFbSize, bIsDynamicPM)) &&
            ((fbsrStatus = fbsrReserveSysMemoryForPowerMgmt(pGpu, pMemoryManager->pFbsr[FBSR_TYPE_DMA],
                                             usedFbSize)) == NV_OK))
        {
            pGpu->setProperty(pGpu, PDB_PROP_GPU_GCOFF_STATE_ENTERING, NV_TRUE);

            //
            // Set 'bPreserveComptagBackingStoreOnSuspend' so that comptag
            // related handling can be done during state unload/load.
            //
            if (!pMemoryManager->bUseVirtualCopyOnSuspend)
            {
                pKernelMemorySystem->bPreserveComptagBackingStoreOnSuspend = NV_TRUE;
            }

            KernelGsp *pTestGsp = (bIsDynamicPM && IS_GSP_CLIENT(pGpu)) ?
                GPU_GET_KERNEL_GSP(pGpu) : NULL;
            if (pTestGsp != NULL)
                pTestGsp->bTestDeferGcoff = nv_test_defer_gcoff(nv);
            status = RmPowerManagement(pGpu, NV_PM_ACTION_STANDBY);
            if (pTestGsp != NULL)
                pTestGsp->bTestDeferGcoff = NV_FALSE;
            pGpu->setProperty(pGpu, PDB_PROP_GPU_GCOFF_STATE_ENTERING, NV_FALSE);

            if (status == NV_OK)
            {
                pGpu->setProperty(pGpu, PDB_PROP_GPU_GCOFF_STATE_ENTERED, NV_TRUE);
            }
            else
            {
                /*
                 * Normally the NV_PM_ACTION_RESUME resume should free the
                 * the reserved memory while doing FB state load. But if the
                 * failure has happened at very early stage in FB state unload
                 * while doing NV_PM_ACTION_STANDBY, then reserved system memory
                 * might not be freed.
                 */
                if (bIsDynamicPM && IS_GSP_CLIENT(pGpu) &&
                    pGpu->powerManagementDepth == NV_PM_DEPTH_NONE)
                    *bTryAgain = NV_TRUE;

                fbsrFreeReservedSysMemoryForPowerMgmt(pMemoryManager->pFbsr[FBSR_TYPE_DMA]);

                if (!pMemoryManager->bUseVirtualCopyOnSuspend)
                {
                    pKernelMemorySystem->bPreserveComptagBackingStoreOnSuspend = nv->preserve_vidmem_allocations;
                }
            }
        }
        else if (bCanUseGc6)
        {
            NV2080_CTRL_GC6_ENTRY_PARAMS entryParams;
            portMemSet(&entryParams, 0, sizeof(entryParams));

            entryParams.flavorId = NV2080_CTRL_GC6_FLAVOR_ID_MSHYBRID;
            entryParams.stepMask = NVBIT(NV2080_CTRL_GC6_STEP_ID_GPU_OFF);
            entryParams.params.bIsRTD3Transition = NV_TRUE;

            //
            // Currently if the GPU is not in the Lowest P-State, then the
            // GC6 entry will fail in its internal sanity check. The system
            // suspend can be triggered in any P-State, so sanity check
            // for the lowest P-State needs to be skipped. In this case, during
            // Perf state unload time, the forceful P-State switch will
            // happen which will bring the GPU into lowest P-State.
            //
            entryParams.params.bSkipPstateSanity = !bIsDynamicPM;

            status = gpuGc6Entry(pGpu, &entryParams);

            //
            // The GC6 state requires SW involvement before each transition
            // from D0 to D3cold. In case of system suspend with pass-through
            // mode, the VM suspend should happen before host suspend. During
            // VM suspend, the NVIDIA driver running in the VM side will be
            // involved in D3cold transition, but in the host side, it will
            // go through D3cold->D0->D3cold transition and will make the
            // second D3cold transition without NVIDIA driver. To handle this
            // use-case, during VM suspend, use the D0 state and the host linux
            // kernel will put the device into D3cold state.
            //
            if ((status == NV_OK) && ((nv->flags & NV_FLAG_PASSTHRU) != 0))
            {
                nv->d0_state_in_suspend = NV_TRUE;
            }
        }
        //
        // The else condition below will hit in the following cases: 
        // Case 1. During system suspend transition: For systems that support s2idle but are configured
        // for deep sleep, this "else" condition will be hit when the system memory 
        // is not sufficient. In this case, we should unset bTryAgain to abort the current suspend entry. 
        // Case 2. During runtime suspend transition: For systems that do not support GC6 but support 
        // GCOFF, this "else" condition will be hit when system memory is not sufficent, In this case, we 
        // should set bTryagain so that the kernel can reschedule the callback later. 
        //
        else
        {
            if (bIsDynamicPM)
            {
                if (fbsrStatus == NV_ERR_NO_MEMORY)
                {
                    *bTryAgain = NV_TRUE;
                }
                status = fbsrStatus;
            }
            else
            {
                status = NV_ERR_NOT_SUPPORTED;
                NV_PRINTF(LEVEL_ERROR,
                          "System suspend failed with current system suspend configuration. "
                          "Please change the system suspend configuration to s2idle in /sys/power/mem_sleep.\n");
            }
        }
    }
    else
    {
        if (pGpu->getProperty(pGpu, PDB_PROP_GPU_GCOFF_STATE_ENTERED))
        {
            status = RmPowerManagement(pGpu, NV_PM_ACTION_RESUME);
            if (status == NV_OK || !bIsDynamicPM || !IS_GSP_CLIENT(pGpu))
                pGpu->setProperty(pGpu, PDB_PROP_GPU_GCOFF_STATE_ENTERED, NV_FALSE);
            if (!pMemoryManager->bUseVirtualCopyOnSuspend)
            {
                pKernelMemorySystem->bPreserveComptagBackingStoreOnSuspend = nv->preserve_vidmem_allocations;
            }
        }
        else if (pGpu->getProperty(pGpu, PDB_PROP_GPU_RG_STATE_ENTERED))
        {
            status = RmPowerManagement(pGpu, NV_PM_ACTION_RESUME);
            if (status == NV_OK)
            {
                pGpu->setProperty(pGpu, PDB_PROP_GPU_RG_STATE_ENTERED, NV_FALSE);
            }
        }
        else
        {
            NV2080_CTRL_GC6_EXIT_PARAMS exitParams;
            portMemSet(&exitParams, 0, sizeof(exitParams));

            exitParams.flavorId = NV2080_CTRL_GC6_FLAVOR_ID_MSHYBRID;
            exitParams.params.bIsRTD3Transition = NV_TRUE;

            status = gpuGc6Exit(pGpu, &exitParams);
        }
    }

    return status;
}
static NV_STATUS RmTransitionDynamicPower(
    OBJGPU *pGpu,
    NvBool  bEnter,
    NvBool *bTryAgain
)
{
    nv_state_t *nv   = NV_GET_NV_STATE(pGpu);
    NV_STATUS   status;

    status = RmGcxPowerManagement(pGpu, bEnter, NV_TRUE, bTryAgain);

    if (status != NV_OK && !*bTryAgain && IS_GSP_CLIENT(pGpu))
    {
        KernelGsp *pKernelGsp = GPU_GET_KERNEL_GSP(pGpu);

        // A partially transferred device cannot safely service new work.
        // The GSP RPC gate is checked before queuing, even for a control fd.
        pKernelGsp->bFatalError = NV_TRUE;
        portAtomicSetU32(&nv->rtd3_restart_required, 1);
        NV_DEV_PRINTF(NV_DBG_ERRORS, nv,
                      "RTD3 %s failed (status 0x%x, depth %u); GPU restart required\n",
                      bEnter ? "suspend" : "resume", status, pGpu->powerManagementDepth);
    }

    if (!bEnter && status == NV_OK)
    {
        nv_idle_holdoff(nv);
        RmScheduleCallbackToRemoveIdleHoldoff(pGpu);
    }

    return status;
}
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
