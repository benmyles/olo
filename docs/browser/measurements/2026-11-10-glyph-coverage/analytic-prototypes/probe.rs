use serde_json::Value;
use std::{collections::HashMap, fs};
use zeno::{Command, Mask};
struct Pen {
    commands: Vec<Command>,
    unit: f32,
    x: f32,
    y: f32,
    mode: u32,
}
impl Pen {
    fn p(&self, x: f32, y: f32) -> zeno::Point {
        let q = |v: f32| if self.mode == 2 || self.mode == 3 { (v * 64.).round() / 64. } else { v };
        let x = q(x * self.unit) + self.x;
        let y = self.y - q(y * self.unit);
        if self.mode >= 4 { [(x * 256.).trunc() / 256., (y * 4.).round() / 4.].into() } else { [x, y].into() }
    }
}
impl ttf_parser::OutlineBuilder for Pen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.commands.push(Command::MoveTo(self.p(x, y)));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.commands.push(Command::LineTo(self.p(x, y)));
    }
    fn quad_to(&mut self, x: f32, y: f32, a: f32, b: f32) {
        self.commands.push(Command::QuadTo(self.p(x, y), self.p(a, b)));
    }
    fn curve_to(&mut self, x: f32, y: f32, a: f32, b: f32, c: f32, d: f32) {
        self.commands.push(Command::CurveTo(self.p(x, y), self.p(a, b), self.p(c, d)));
    }
    fn close(&mut self) {
        self.commands.push(Command::Close);
    }
}
fn main() {
    let args: Vec<_> = std::env::args().collect();
    let scale: f64 = args[1].parse().unwrap();
    let mode: u32 = args[2].parse().unwrap();
    let root = "/var/tmp/codex-browser-glyphs";
    let data: Value = serde_json::from_slice(&fs::read(format!("{root}/input-{}.json", args[1])).unwrap()).unwrap();
    let mut fonts = HashMap::new();
    for (id, face) in data["faces"].as_object().unwrap() {
        let family = face["families"][0].as_str().unwrap();
        let short = family.strip_prefix("Liberation ").unwrap();
        fonts.insert(id.clone(), fs::read(format!("/usr/share/fonts/liberation-{}-fonts/Liberation{}-Regular.ttf", short.to_lowercase(), short)).unwrap());
    }
    let mut image = tiny_skia::Pixmap::new((1000. * scale) as u32, (1760. * scale) as u32).unwrap();
    image.fill(tiny_skia::Color::WHITE);
    for para in data["paragraphs"].as_array().unwrap() {
        let Some(id) = para["id"].as_str() else { continue };
        let i: u32 = id.strip_prefix("case-").unwrap().parse().unwrap();
        if i >= 116 {
            continue;
        }
        let baseline = ((para["bounds"][1].as_f64().unwrap() + para["lines"][0]["baseline"].as_f64().unwrap()) * scale).round();
        let x = para["bounds"][0].as_f64().unwrap() * scale;
        for run in para["runs"].as_array().unwrap() {
            let font = ttf_parser::Face::parse(&fonts[&run["face"].to_string()], 0).unwrap();
            let unit = run["size"].as_f64().unwrap() * scale / font.units_per_em() as f64;
            let mut advance = 0.;
            for g in run["glyphs"].as_array().unwrap() {
                let mut gx = x + advance + g["offset"][0].as_f64().unwrap() * unit;
                let gy = baseline - g["offset"][1].as_f64().unwrap() * unit;
                if mode >= 1 {
                    gx = (gx * 4.).round() / 4.;
                }
                let mut pen = Pen { commands: vec![], unit: unit as f32, x: gx as f32, y: gy as f32, mode };
                font.outline_glyph(ttf_parser::GlyphId(g["id"].as_u64().unwrap() as u16), &mut pen);

                if !pen.commands.is_empty() {
                    let (mask, pos) = Mask::new(&pen.commands).render();
                    for y in 0..pos.height {
                        for x in 0..pos.width {
                            let px = pos.left + x as i32;
                            let py = pos.top + y as i32;
                            if px < 0 || py < 0 || px >= image.width() as i32 || py >= image.height() as i32 {
                                continue;
                            }
                            let alpha = mask[(y * pos.width + x) as usize] as u32;
                            let offset = ((py as u32 * image.width() + px as u32) * 4) as usize;
                            for c in &mut image.data_mut()[offset..offset + 3] {
                                *c = ((17 * alpha + *c as u32 * (255 - alpha) + 127) / 255) as u8;
                            }
                        }
                    }
                }
                let adv = g["advance"][0].as_f64().unwrap() * unit;
                advance += if mode == 3 { (adv * 64.).round() / 64. } else { adv };
            }
        }
    }
    image.save_png(format!("{root}/zeno-{}-{mode}.png", args[1])).unwrap();
}
