//! The model's arithmetic on the GPU (Vulkan through blade, as Olo's
//! windows draw): model.wgsl's `score` for a line's candidates, its
//! `grad` for a batch of examples, an example a workgroup. Buffers are
//! shared with the processor (written and read in place) and grow as a
//! batch needs; each call waits for its work, at most two seconds (a GPU
//! that doesn't answer is given up, and the processor does the rest).

use blade_graphics as gpu;
use olo_complete::{
    index::F,
    kernels::{Cpu, PARAMS, Weights},
    train::{Backend, CpuBackend, Example},
};

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Sizes {
    examples: u32,
    rows: u32,
    pad0: u32,
    pad1: u32,
}

#[derive(blade_macros::ShaderData)]
struct Data {
    sizes: Sizes,
    weights: gpu::BufferPiece,
    xs: gpu::BufferPiece,
    spans: gpu::BufferPiece,
    outs: gpu::BufferPiece,
}

/// A buffer shared with the processor, and how big it is.
struct Shared {
    raw: gpu::Buffer,
    size: u64,
    name: &'static str,
}

pub struct Gpu {
    ctx: gpu::Context,
    score: gpu::ComputePipeline,
    grad: gpu::ComputePipeline,
    encoder: gpu::CommandEncoder,
    weights: Shared,
    xs: Shared,
    spans: Shared,
    outs: Shared,
    device: String,
    /// Why it was given up, if it was: the processor does the work then.
    failed: Option<String>,
    cpu: CpuBackend,
}

/// How long a call waits for the GPU.
const WAIT_MS: u32 = 2000;

impl Gpu {
    /// The GPU, its kernels compiled (Err: why not: no Vulkan device, a
    /// shader the driver refused).
    pub fn new() -> Result<Gpu, String> {
        // SAFETY: a compute-only context (nothing presented), made once and
        // kept with its resources for as long as the Gpu lives.
        let ctx = unsafe { gpu::Context::init(gpu::ContextDesc::default()) }.map_err(|e| format!("no GPU to compute with ({e:?})"))?;
        let info = ctx.device_information();
        let device = if info.is_software_emulated { format!("{} (in software)", info.device_name) } else { info.device_name.clone() };
        let shader = ctx.try_create_shader(gpu::ShaderDesc { source: include_str!("model.wgsl") }).map_err(|e| format!("the GPU's kernels didn't compile ({e})"))?;
        shader.check_struct_size::<Sizes>();
        let layout = <Data as gpu::ShaderData>::layout();
        let score = ctx.create_compute_pipeline(gpu::ComputePipelineDesc { name: "olo-complete score", data_layouts: &[&layout], compute: shader.at("score") });
        let grad = ctx.create_compute_pipeline(gpu::ComputePipelineDesc { name: "olo-complete grad", data_layouts: &[&layout], compute: shader.at("grad") });
        let encoder = ctx.create_command_encoder(gpu::CommandEncoderDesc { name: "olo-complete", buffer_count: 1 });
        let new = |ctx: &gpu::Context, name: &'static str, size: u64| Shared { raw: ctx.create_buffer(gpu::BufferDesc { name, size, memory: gpu::Memory::Shared }), size, name };
        let weights = new(&ctx, "weights", (PARAMS * 4) as u64);
        let xs = new(&ctx, "xs", 64 * 1024);
        let spans = new(&ctx, "spans", 4 * 1024);
        let outs = new(&ctx, "outs", 64 * 1024);
        Ok(Gpu { ctx, score, grad, encoder, weights, xs, spans, outs, device, failed: None, cpu: CpuBackend(Cpu::Simd) })
    }

    /// `buf` at least `bytes` (made anew, bigger, if it isn't).
    fn fit(ctx: &gpu::Context, buf: &mut Shared, bytes: u64) {
        if buf.size >= bytes {
            return;
        }
        let size = bytes.next_power_of_two();
        ctx.destroy_buffer(buf.raw);
        buf.raw = ctx.create_buffer(gpu::BufferDesc { name: buf.name, size, memory: gpu::Memory::Shared });
        buf.size = size;
    }

    fn write<T: bytemuck::Pod>(buf: &Shared, data: &[T]) {
        let bytes: &[u8] = bytemuck::cast_slice(data);
        // SAFETY: a shared buffer at least this big (`fit`), not in use by
        // the GPU (each call waits for its work before the next writes).
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf.raw.data(), bytes.len()) };
    }

    fn read(buf: &Shared, n: usize) -> Vec<f32> {
        let mut out = vec![0.0f32; n];
        // SAFETY: as `write`: the GPU's work on it is done.
        unsafe { std::ptr::copy_nonoverlapping(buf.raw.data() as *const f32, out.as_mut_ptr(), n) };
        out
    }

    /// Run `pipeline` over `groups` workgroups and wait for it.
    fn run(&mut self, grad: bool, groups: u32, sizes: Sizes) -> Result<(), String> {
        let data = Data { sizes, weights: self.weights.raw.into(), xs: self.xs.raw.into(), spans: self.spans.raw.into(), outs: self.outs.raw.into() };
        self.encoder.start();
        {
            let mut pass = self.encoder.compute("olo-complete");
            let mut pc = pass.with(if grad { &self.grad } else { &self.score });
            pc.bind(0, &data);
            pc.dispatch([groups.max(1), 1, 1]);
        }
        let sp = self.ctx.submit(&mut self.encoder);
        if self.ctx.wait_for(&sp, WAIT_MS) { Ok(()) } else { Err(format!("the GPU didn't finish in {} s", WAIT_MS / 1000)) }
    }

    fn try_scores(&mut self, w: &Weights, x: &[f32]) -> Result<Vec<f32>, String> {
        let rows = x.len() / F;
        Gpu::fit(&self.ctx, &mut self.xs, (x.len() * 4) as u64);
        Gpu::fit(&self.ctx, &mut self.outs, (rows.max(1) * 4) as u64);
        Gpu::write(&self.weights, &w.flat());
        Gpu::write(&self.xs, x);
        self.run(false, rows.div_ceil(64) as u32, Sizes { examples: 0, rows: rows as u32, pad0: 0, pad1: 0 })?;
        Ok(Gpu::read(&self.outs, rows))
    }

    fn try_loss_and_grad(&mut self, w: &Weights, examples: &[&Example]) -> Result<(f32, Weights), String> {
        let mut x = Vec::new();
        let mut spans = Vec::with_capacity(examples.len() * 4);
        for ex in examples {
            spans.extend_from_slice(&[(x.len() / F) as u32, ex.rows() as u32, ex.target as u32, 0]);
            x.extend_from_slice(&ex.x);
        }
        let n = examples.len();
        Gpu::fit(&self.ctx, &mut self.xs, (x.len().max(F) * 4) as u64);
        Gpu::fit(&self.ctx, &mut self.spans, (spans.len().max(4) * 4) as u64);
        Gpu::fit(&self.ctx, &mut self.outs, (n.max(1) * (PARAMS + 1) * 4) as u64);
        Gpu::write(&self.weights, &w.flat());
        Gpu::write(&self.xs, &x);
        Gpu::write(&self.spans, &spans);
        self.run(true, n as u32, Sizes { examples: n as u32, rows: (x.len() / F) as u32, pad0: 0, pad1: 0 })?;
        let out = Gpu::read(&self.outs, n * (PARAMS + 1));
        let mut g = vec![0.0f32; PARAMS];
        let mut loss = 0.0;
        for e in 0..n {
            let row = &out[e * (PARAMS + 1)..(e + 1) * (PARAMS + 1)];
            for (a, b) in g.iter_mut().zip(row) {
                *a += b;
            }
            loss += row[PARAMS];
        }
        let k = 1.0 / n.max(1) as f32;
        g.iter_mut().for_each(|v| *v *= k);
        Ok((loss * k, Weights::from_flat(&g)))
    }
}

impl Backend for Gpu {
    fn name(&self) -> String {
        match &self.failed {
            None => format!("GPU ({})", self.device),
            Some(_) => self.cpu.name(),
        }
    }

    fn trouble(&self) -> Option<String> {
        self.failed.as_ref().map(|e| format!("the GPU stopped: {e}"))
    }

    fn scores(&mut self, w: &Weights, x: &[f32]) -> Vec<f32> {
        if self.failed.is_none() {
            match self.try_scores(w, x) {
                Ok(s) => return s,
                Err(e) => self.failed = Some(e),
            }
        }
        self.cpu.scores(w, x)
    }

    fn loss_and_grad(&mut self, w: &Weights, examples: &[&Example]) -> (f32, Weights) {
        if self.failed.is_none() {
            match self.try_loss_and_grad(w, examples) {
                Ok(r) => return r,
                Err(e) => self.failed = Some(e),
            }
        }
        self.cpu.loss_and_grad(w, examples)
    }
}

impl Drop for Gpu {
    fn drop(&mut self) {
        for b in [self.weights.raw, self.xs.raw, self.spans.raw, self.outs.raw] {
            self.ctx.destroy_buffer(b);
        }
        self.ctx.destroy_command_encoder(&mut self.encoder);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noise(seed: u32, n: usize) -> Vec<f32> {
        let mut s = seed;
        (0..n)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                (s as f32 / u32::MAX as f32) * 2.0 - 1.0
            })
            .collect()
    }

    /// The GPU's kernels do the processor's arithmetic: scores, and a
    /// batch's loss and gradient (examples of 1 to 32 candidates).
    #[test]
    fn the_gpu_does_the_processors_arithmetic() {
        let mut g = match Gpu::new() {
            Ok(g) => g,
            Err(why) => {
                eprintln!("skipped: {why}");
                return;
            }
        };
        let w = Weights::from_flat(&noise(3, PARAMS));
        let x = noise(5, 29 * F);
        let (a, b) = (CpuBackend(Cpu::Scalar).scores(&w, &x), g.scores(&w, &x));
        assert!(g.trouble().is_none(), "{:?}", g.trouble());
        assert!(a.iter().zip(&b).all(|(p, q)| (p - q).abs() < 1e-3), "{a:?}\n{b:?}");
        let examples: Vec<Example> = (0..40)
            .map(|i| {
                let rows = 1 + (i * 7) % 32;
                Example { x: noise(100 + i as u32, rows * F).iter().map(|v| v.abs() * 2.0).collect(), target: (i * 5) % rows }
            })
            .collect();
        let refs: Vec<&Example> = examples.iter().collect();
        let (l0, g0) = CpuBackend(Cpu::Scalar).loss_and_grad(&w, &refs);
        let (l1, g1) = g.loss_and_grad(&w, &refs);
        assert!(g.trouble().is_none(), "{:?}", g.trouble());
        assert!((l0 - l1).abs() < 1e-3, "{l0} vs {l1}");
        for (p, q) in g0.flat().iter().zip(g1.flat()) {
            assert!((p - q).abs() < 1e-3, "{p} vs {q}");
        }
        eprintln!("checked on {}", g.name());
    }
}
