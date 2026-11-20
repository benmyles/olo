/* Runs NVIDIA's compiled EVO and GPFIFO objects on ordinary RAM and a virtual clock. */
#include "nvkms-dma.h"
#include "class/clc57d.h"
#include "nvidia-push-methods.h"
#include "nvidia-push-utils.h"

extern int puts(const char *);
extern void abort(void);
#define REQUIRE(x) do { if (!(x)) { puts("FAIL " #x); abort(); } } while (0)

static NVDevEvoRec dev;
static NVEvoChannel evo;
static NVDispEvoRec disp;
static NvU32 regs[2], ring[1024], note;
static NvU64 clockUs;
static NvBool advanceGet, unavailable;
static NvU32 logs, yields;

NvU64 nvkms_get_usec(void) { return clockUs; }
void nvkms_yield(void)
{
    REQUIRE(++yields < 32);
    clockUs += 1000000;
    if (advanceGet) regs[NVC57D_GET / 4] = 64;
}
NvBool nvkms_gpu_ref_unavailable(const struct nvkms_gpu_ref *ref) { return unavailable; }
void nvEvoLogDev(const NVDevEvoRec *d, NVEvoLogType t, const char *fmt, ...) { logs++; }
void nvEvoLogDisp(const NVDispEvoRec *d, NVEvoLogType t, const char *fmt, ...) { logs++; }
void nvEvoLog(NVEvoLogType t, const char *fmt, ...) { logs++; }

static void reset_evo(void)
{
    __builtin_memset(&dev, 0, sizeof(dev));
    __builtin_memset(&evo, 0, sizeof(evo));
    __builtin_memset(ring, 0, sizeof(ring));
    evo.pb.base = ring;
    evo.pb.buffer = ring + 1010;
    evo.pb.put_offset = 4040;
    evo.pb.offset_max = 4048;
    evo.pb.control = regs;
    evo.pb.pDevEvo = &dev;
    evo.notifierDma.cpuAddress = &note;
    dev.core = &evo;
    dev.simulationType = NV2080_CTRL_GPU_GET_SIMULATION_INFO_TYPE_NONE;
    disp.pDevEvo = &dev;
    regs[0] = 4040;
    regs[1] = 0;
    note = 0;
    clockUs = 1;
    logs = yields = 0;
    advanceGet = unavailable = FALSE;
}

static void check_evo(void)
{
    reset_evo();
    nvDmaSetStartEvoMethod(&evo, 0x100, 1);
    REQUIRE(dev.displayChannelFailed && logs == 1 && yields == 6);
    for (int i = 0; i < 10000; i++) {
        nvDmaSetStartEvoMethod(&evo, 0x100, 2);
        nvDmaSetEvoMethodDataU64(&evo, 0x12345678ULL);
        nvDmaKickoffEvo(&evo);
    }
    REQUIRE(evo.pb.fifo_free_count == 2 && evo.pb.buffer == ring + 1010);
    REQUIRE(regs[0] == 4040 && ring[1010] == 0 && logs == 1 && yields == 6);
    puts("EVO stuck GET: bounded, 10000 subsequent headers/data/kicks suppressed");
    reset_evo();
    advanceGet = TRUE;
    nvDmaSetStartEvoMethod(&evo, 0x100, 1);
    nvDmaSetEvoMethodData(&evo, 42);
    nvDmaKickoffEvo(&evo);
    REQUIRE(!dev.displayChannelFailed && ring[0] && ring[1] == 42 && regs[0] == 8);
    puts("EVO advancing GET: wrapped, wrote payload 42, published PUT 8");
    reset_evo();
    dev.gpuRefs[0] = (struct nvkms_gpu_ref *)&note;
    unavailable = TRUE;
    nvDmaSetStartEvoMethod(&evo, 0x100, 1);
    nvDmaSetEvoMethodData(&evo, 42);
    nvDmaKickoffEvo(&evo);
    REQUIRE(dev.displayChannelFailed && !yields && !ring[1010] && regs[0] == 4040);
    puts("cached RM failure: immediate refusal, no yield, no PUT");
    reset_evo();
    nvEvoWaitForCoreNotifier(&disp, 0, 0, 0, 1);
    REQUIRE(dev.displayChannelFailed && logs == 1 && yields == 4 && note == 0);
    REQUIRE(!nvEvoIsCoreNotifierComplete(&disp, 0, 0, 0, 1));
    reset_evo();
    note = 1;
    nvEvoWaitForCoreNotifier(&disp, 0, 0, 0, 1);
    REQUIRE(!dev.displayChannelFailed && !yields && !logs);
    puts("notifier: real completion succeeds; stalled notifier fails without writing DONE");
}

static NvPushDeviceRec host;
static NvPushChannelRec channel;
static NvPushChannelUnion segment[1024];
static NvU32 progress, gp[8], errors;
static NvU64 push_ms(NvPushDevicePtr p) { return clockUs / 1000; }
static void push_yield(NvPushDevicePtr p) { nvkms_yield(); }
static void push_error(NvPushChannelPtr p, NvU32 status) { errors++; }
static void wrapped(NvPushChannelPtr p) {}
static NvBool lost(NvPushDevicePtr p) { return unavailable; }
static const NvPushImports imports = {
    .getMilliSeconds = push_ms, .yield = push_yield,
    .channelErrorOccurred = push_error, .pushbufferWrapped = wrapped, .deviceUnavailable = lost,
};

static void reset_push(void)
{
    __builtin_memset(&channel, 0, sizeof(channel));
    __builtin_memset(segment, 0, sizeof(segment));
    __builtin_memset(gp, 0, sizeof(gp));
    clockUs = 1000; errors = yields = progress = 0; unavailable = FALSE;
    host.pImports = &imports; host.numSubDevices = 1;
    channel.pDevice = &host; channel.main.base = segment;
    channel.main.buffer = segment + 1023; channel.main.putOffset = 4092;
    channel.main.sizeInBytes = sizeof(segment); channel.progressSemaphore.ptr[0] = &progress;
    channel.ignoreChannelErrors = TRUE; channel.numGpFifoEntries = 4; channel.gpfifo = gp;
}

static void check_push(void)
{
    NvU32 data = 38;
    reset_push();
    __nvPushMakeRoom(&channel, 4);
    REQUIRE(channel.main.stopped && errors == 1 && yields == 10);
    for (int i = 0; i < 10000; i++) {
        nvPushMethod(&channel, 0, 0x100, 1);
        nvPushSetMethodData(&channel, 99);
        nvPushImmedVal(&channel, 0, 0x104, 2);
        nvPushInlineData(&channel, &data, 1);
        nvPushKickoff(&channel);
    }
    REQUIRE(channel.main.buffer == segment + 1023 && !segment[1023].u && errors == 1);
    puts("GPFIFO main ring: bounded, repeated immediate/scalar/bulk methods suppressed");
    reset_push();
    channel.main.putOffset = 4088; channel.gpPutOffset = 2;
    nvPushKickoff(&channel);
    REQUIRE(channel.main.stopped && errors == 1 && yields == 10 && channel.gpPutOffset == 2 && !gp[4]);
    puts("GPFIFO full entry queue: bounded, no entry or doorbell publication");
}
int main(void) { check_evo(); check_push(); return 0; }
