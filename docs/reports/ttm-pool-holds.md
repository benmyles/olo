# TTM's hard try for the beneficial order holds the threads that draw (a draft to send)

A draft of a report about what was measured on a ThinkPad P1 Gen 9 on
2026-11-21 (kernel/TUNING.md, "Graphics memory in 2 MB pieces"). Nothing
has been sent. It is for dri-devel@lists.freedesktop.org and
intel-xe@lists.freedesktop.org; the second part, shorter, is for
intel-xe alone.

## For dri-devel and intel-xe: drm/ttm: the beneficial-order allocation stalls compositors for hundreds of milliseconds

**What happened.** A laptop with Intel graphics on xe (Panther Lake,
8086:b0a0; 62 GB of memory; Fedora's 7.3.4-300.fc45) runs a Wayland
desktop while software is being built. Free memory is plentiful but in
small pieces: of 4.6 GB free, 132 MB is in blocks of order 9 and over,
and `compact_stall` is 281,399 four hours after boot, 68% of them
failures. On that computer the threads that draw are held in direct
compaction and direct reclaim *inside the GPU memory pool*, for memory
they did not ask to have in any particular size. Traced with bpftrace
(fentry/fexit on `try_to_compact_pages`, the
`mm_vmscan_direct_reclaim_begin/end` tracepoints, each attributed to
`ttm_pool_alloc_page` being on the thread's stack), ten minutes of
ordinary use with a build running:

| thread | held | in all | longest | of 4 ms and over |
|---|---|---|---|---|
| a browser's main threads (its GPU process) | 591 times | 4.2 s | 327 ms | 119 |
| one of the browser's pool threads | 144 times | 1.6 s | 502 ms | 31 |
| applications' render threads | 37 times | 1.0 s | 390 ms | 13 |
| an application's window thread | 64 times | 0.6 s | 458 ms | 5 |
| the Wayland compositor's main loop | 11 times | 13 ms | 7.3 ms | 1 |

Every one of them through `xe_ttm_tt_populate → ttm_pool_alloc →
__ttm_pool_alloc → ttm_pool_alloc_page → alloc_pages_node` at order 9.
A frame at 120 Hz is 8.3 ms; these are windows that stop drawing for a
third to half of a second while tens of gigabytes of page cache could
be had.

**Why.** Since "drm/ttm: Issue direct reclaim at beneficial_order"
(a56a73ec85c8) and "drm/ttm/pool: Try harder for beneficial orders"
(dd8dde5745ea), `ttm_pool_alloc_page()` removes `__GFP_NORETRY` and
adds `__GFP_RETRY_MAYFAIL` at the order the driver declares beneficial,
and xe declares `get_order(SZ_2M)`. Its caller passes `GFP_USER |
GFP_HIGHUSER`, so that one attempt may compact and reclaim, repeatedly,
on the calling thread, for every 2 MB of every buffer populated; on x86
ordinary cached pages are not pooled (`ttm_pool_select_type()` returns
NULL for `ttm_cached` unless the pool uses dma_alloc), so every
population goes to the page allocator. The second commit's message
says the trade it makes: "at the cost of longer allocation setup time.
That tradeoff is acceptable for beneficial-order allocations". On a
desktop whose memory is fragmented it isn't: the allocation setup time
is the frame.

**It reproduces without a GPU.** In a KVM guest running the same
kernel, a module makes a pool with xe's flags
(`TTM_ALLOCATION_POOL_BENEFICIAL_ORDER(9)`, no dma_alloc) and fills 64
buffers of 2 MB with `ttm_pool_alloc()` after another module has kept
one page of every free order-9 block. The filling thread entered direct
compaction 113 times and direct reclaim 56 times, and got 10 of its 64
order-9 attempts. With the order-9 attempt made without
`__GFP_DIRECT_RECLAIM` (and `__GFP_KSWAPD_RECLAIM` kept), it entered
neither, kcompactd was woken 18 times, it got the same 10 of 64, the
buffers came out of the same pieces (ten of 2 MB, the rest of 1 MB),
and every fill succeeded. (The modules and the script are in Olo's repository,
`kernel/gpu-pages/check` and `tools/dev/gpu-pages-check`: the link
goes here when this is sent.)

**A suggestion.** Let a driver, or the administrator, choose that the
beneficial-order attempt is made without waiting: it takes a block
that is free, wakes kswapd and kcompactd to make more, and otherwise
falls to the smaller orders exactly as it does today when the attempt
fails. A pool flag does it in three lines:

```diff
--- a/include/drm/ttm/ttm_allocation.h
+++ b/include/drm/ttm/ttm_allocation.h
@@
 #define TTM_ALLOCATION_PROPAGATE_ENOSPC		BIT(10) /* Do not convert ENOSPC from resource managers to ENOMEM. */
+/* Try the beneficial order without direct reclaim, then fall back. */
+#define TTM_ALLOCATION_POOL_DEFER_BENEFICIAL_RECLAIM	BIT(11)
--- a/drivers/gpu/drm/ttm/ttm_pool.c
+++ b/drivers/gpu/drm/ttm/ttm_pool.c
@@
 	if (beneficial_order && order == beneficial_order) {
 		gfp_flags &= ~__GFP_NORETRY;
-		gfp_flags |= __GFP_RETRY_MAYFAIL;
+		if (pool->alloc_flags & TTM_ALLOCATION_POOL_DEFER_BENEFICIAL_RECLAIM)
+			gfp_flags &= ~(__GFP_DIRECT_RECLAIM | __GFP_RETRY_MAYFAIL);
+		else
+			gfp_flags |= __GFP_RETRY_MAYFAIL;
 	}
```

with xe (or a module parameter of xe's, or of ttm's) selecting it. A
compositor cannot ask for this itself: the buffers are made by Mesa on
its behalf, and the display path's own.

**What we run meanwhile.** An out-of-tree module with a kprobe at
`ttm_pool_alloc_page()`'s entry that clears `__GFP_DIRECT_RECLAIM` from
the gfp argument for that one case (order 9, a pool whose beneficial
order is 9 and that doesn't use dma_alloc, owned by a device bound to
xe). It checks at load, through the `mm_page_alloc` tracepoint, that
the flags reaching the page allocator are the stock ones less that
flag, and refuses to load otherwise.

## For intel-xe: a display page table is created for every page flip

On the same computer the compositor's thread populates about 180 TTM
buffers a second, 33,612 of 33,781 in 188 s from

```
intel_prepare_plane_fb → intel_plane_pin_fb → xe_fb_pin_dpt_pin →
__xe_pin_fb_vma → __xe_pin_fb_vma_dpt → xe_bo_create_pin_map_at_novm →
__xe_bo_create_locked → … → xe_ttm_tt_populate → ttm_pool_alloc
```

that is, a new DPT buffer object for each flip between a swapchain's
buffers (`xe_fb_pin_reuse_vma()` reuses the vma only when the new plane
state has the old one's framebuffer and view). They are small (order 4
and under, which the pool doesn't wait for), write-combined (so mostly
from the pool's own lists) and cheap in the middle, 3 µs; the longest
in 45 s was 6.5 ms with no reclaim or compaction entered. Each also
writes a PTE for every page of the framebuffer (14,400 for a 5K one). i915 kept the DPT's
vma with the framebuffer for as long as it lived. Keeping it with the
framebuffer in xe would take the allocation, the PTE writes and the
GGTT insertion out of every flip.
