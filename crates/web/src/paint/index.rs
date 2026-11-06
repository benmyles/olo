use super::*;
use smallvec::SmallVec;
#[derive(Clone, Copy)]
struct Node {
    bounds: layout::Rect,
    first: u32,
    count: u8,
    leaf: bool,
}
pub(super) struct Index {
    nodes: Box<[Node]>,
    ids: Box<[u32]>,
}
impl Index {
    pub fn new(commands: &[Command], cancel: Option<&AtomicBool>) -> Result<Self> {
        check(cancel)?;
        let mut ids: Vec<u32> = (0..commands.len() as u32).collect();
        ids.sort_unstable_by(|&a, &b| {
            let a = commands[a as usize].bounds;
            let b = commands[b as usize].bounds;
            (a.y + a.height / 2.).total_cmp(&(b.y + b.height / 2.))
        });
        let mut nodes = Vec::new();
        for (i, chunk) in ids.chunks(8).enumerate() {
            check(cancel)?;
            let bounds = chunk.iter().map(|&id| commands[id as usize].bounds).reduce(union).unwrap();
            nodes.push(Node { bounds, first: (i * 8) as u32, count: chunk.len() as u8, leaf: true });
        }
        let mut from = 0;
        let mut count = nodes.len();
        while count > 1 {
            let parent = nodes.len();
            for at in (from..from + count).step_by(8) {
                check(cancel)?;
                let end = (at + 8).min(from + count);
                let bounds = nodes[at..end].iter().map(|n| n.bounds).reduce(union).unwrap();
                nodes.push(Node { bounds, first: at as u32, count: (end - at) as u8, leaf: false });
            }
            from = parent;
            count = nodes.len() - parent;
        }
        Ok(Self { nodes: nodes.into_boxed_slice(), ids: ids.into_boxed_slice() })
    }
    pub fn query(&self, commands: &[Command], region: layout::Rect, out: &mut Vec<u32>, cancel: Option<&AtomicBool>) -> Result<()> {
        out.clear();
        if self.nodes.is_empty() {
            return Ok(());
        }
        let mut stack = SmallVec::<[u32; 32]>::new();
        stack.push(self.nodes.len() as u32 - 1);
        while let Some(id) = stack.pop() {
            check(cancel)?;
            let n = self.nodes[id as usize];
            if !intersects(n.bounds, region) {
                continue;
            }
            if n.leaf {
                for &id in &self.ids[n.first as usize..n.first as usize + n.count as usize] {
                    if intersects(commands[id as usize].bounds, region) {
                        out.push(id)
                    }
                }
            } else {
                stack.extend(n.first..n.first + u32::from(n.count));
            }
        }
        out.sort_unstable();
        Ok(())
    }
    pub fn payload_bytes(&self) -> usize {
        self.nodes.len() * size_of::<Node>() + self.ids.len() * size_of::<u32>()
    }
}
