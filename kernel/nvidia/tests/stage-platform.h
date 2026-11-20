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
