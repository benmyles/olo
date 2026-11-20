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
