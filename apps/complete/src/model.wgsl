// olo-complete's model on the GPU: the arithmetic of olo_complete::kernels
// (16 features -> 16 hidden units, ReLU -> a score), in two kernels.
//
// `score`: one thread a candidate row.
// `grad`: one workgroup an example (its rows, at most 32): each row's
// hidden units and score, the softmax cross-entropy of its target, then
// each thread a share of the 289 numbers of its gradient, summed over the
// rows in workgroup memory. One example's gradient (and its loss, last)
// to a row of `outs`; the processor adds the rows of a batch.

struct Sizes {
    examples: u32,
    rows: u32,
    pad0: u32,
    pad1: u32,
}

var<uniform> sizes: Sizes;
var<storage, read> weights: array<f32>;
var<storage, read> xs: array<f32>;
// Per example: its first row, how many, which is the command, unused.
var<storage, read> spans: array<u32>;
var<storage, read_write> outs: array<f32>;

const F: u32 = 16u;
const H: u32 = 16u;
const PARAMS: u32 = 289u;
// Where each part of the weights starts (olo_complete::kernels::Weights::flat).
const W1: u32 = 0u;
const B1: u32 = 256u;
const W2: u32 = 272u;
const B2: u32 = 288u;
const ROWS: u32 = 32u;

fn hidden_of(row: u32, j: u32) -> f32 {
    var a = weights[B1 + j];
    for (var i = 0u; i < F; i++) {
        a += xs[row * F + i] * weights[W1 + i * H + j];
    }
    return max(a, 0.0);
}

@compute @workgroup_size(64)
fn score(@builtin(global_invocation_id) gid: vec3<u32>) {
    let r = gid.x;
    if (r >= sizes.rows) {
        return;
    }
    var s = weights[B2];
    for (var j = 0u; j < H; j++) {
        s += hidden_of(r, j) * weights[W2 + j];
    }
    outs[r] = s;
}

var<workgroup> wh: array<f32, 512>;
var<workgroup> wx: array<f32, 512>;
var<workgroup> wdh: array<f32, 512>;
var<workgroup> ws: array<f32, 32>;
var<workgroup> wds: array<f32, 32>;

@compute @workgroup_size(64)
fn grad(@builtin(workgroup_id) wid: vec3<u32>, @builtin(local_invocation_index) t: u32) {
    let e = wid.x;
    let first = spans[e * 4u];
    let rows = min(spans[e * 4u + 1u], ROWS);
    let goal = spans[e * 4u + 2u];
    if (t < rows) {
        let row = first + t;
        var s = weights[B2];
        for (var j = 0u; j < H; j++) {
            let h = hidden_of(row, j);
            wh[t * H + j] = h;
            s += h * weights[W2 + j];
        }
        for (var i = 0u; i < F; i++) {
            wx[t * F + i] = xs[row * F + i];
        }
        ws[t] = s;
    }
    workgroupBarrier();
    if (t == 0u) {
        var m = -3.0e38;
        for (var r = 0u; r < rows; r++) {
            m = max(m, ws[r]);
        }
        var sum = 0.0;
        for (var r = 0u; r < rows; r++) {
            let p = exp(ws[r] - m);
            wds[r] = p;
            sum += p;
        }
        for (var r = 0u; r < rows; r++) {
            wds[r] = wds[r] / sum;
        }
        outs[e * (PARAMS + 1u) + PARAMS] = -log(max(wds[goal], 1.0e-12));
        wds[goal] = wds[goal] - 1.0;
    }
    workgroupBarrier();
    if (t < rows) {
        for (var j = 0u; j < H; j++) {
            wdh[t * H + j] = select(0.0, wds[t] * weights[W2 + j], wh[t * H + j] > 0.0);
        }
    }
    workgroupBarrier();
    for (var p = t; p < PARAMS; p += 64u) {
        var g = 0.0;
        if (p < B1) {
            let i = p / H;
            let j = p % H;
            for (var r = 0u; r < rows; r++) {
                g += wx[r * F + i] * wdh[r * H + j];
            }
        } else if (p < W2) {
            for (var r = 0u; r < rows; r++) {
                g += wdh[r * H + (p - B1)];
            }
        } else if (p < B2) {
            for (var r = 0u; r < rows; r++) {
                g += wds[r] * wh[r * H + (p - W2)];
            }
        } else {
            for (var r = 0u; r < rows; r++) {
                g += wds[r];
            }
        }
        outs[e * (PARAMS + 1u) + p] = g;
    }
}
