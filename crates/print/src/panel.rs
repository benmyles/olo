//! The Print panel, as an app shows it in a window of its own (as it
//! does a `FilePanel`): where to (a printer CUPS has, or a PDF), how many
//! copies, which pages, on what paper. The printers are asked for on a
//! worker; until they come the choice is a PDF.

use std::sync::{Arc, Mutex};

use olo_kit::{
    egui::{self, Key, Rect, Ui, pos2, vec2},
    metrics as m, style,
    widgets::{Button, MenuField, TextControl},
};

use crate::{PAPERS, Paper, ipp::Printer, local_paper};

/// The panel's size (points), for its window.
pub const SIZE: (u32, u32) = (380, 196);

/// What was chosen.
#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    /// The printer (CUPS's name), or None for a PDF.
    pub printer: Option<String>,
    pub copies: u32,
    /// Pages from 1, inclusive; None for all of them.
    pub pages: Option<(u32, u32)>,
    pub paper: Paper,
}

/// The printers and the default one, once the worker has them (or why
/// not).
type Found = Option<Result<(Vec<Printer>, Option<String>), String>>;

pub struct PrintPanel {
    found: Arc<Mutex<Found>>,
    /// The printers have been asked for (on the first frame).
    asked: bool,
    /// Where to: 0 is a PDF, then each printer.
    dest: usize,
    chosen_default: bool,
    paper: usize,
    copies: String,
    /// 0: all pages, 1: a range.
    range: usize,
    from: String,
    to: String,
    /// How many pages there are (once the app knows).
    pub page_count: Option<u32>,
}

impl Default for PrintPanel {
    fn default() -> Self {
        PrintPanel::new()
    }
}

impl PrintPanel {
    /// A panel; it asks CUPS for the printers when it first shows.
    pub fn new() -> PrintPanel {
        let local = local_paper();
        PrintPanel {
            found: Arc::new(Mutex::new(None)),
            asked: false,
            dest: 0,
            chosen_default: false,
            paper: PAPERS.iter().position(|p| p.pwg == local.pwg).unwrap_or(0),
            copies: "1".into(),
            range: 0,
            from: "1".into(),
            to: String::new(),
            page_count: None,
        }
    }

    /// Draw it; the choice once Print (or Save as PDF) is clicked, `Some(None)`
    /// for Cancel.
    pub fn ui(&mut self, ui: &mut Ui) -> Option<Option<Choice>> {
        if !self.asked {
            self.asked = true;
            let (f, ctx) = (self.found.clone(), ui.ctx().clone());
            let _ = std::thread::Builder::new().name("printers".into()).spawn(move || {
                let r = crate::ipp::printers().map(|p| (p, crate::ipp::default_printer()));
                *f.lock().unwrap_or_else(|e| e.into_inner()) = Some(r);
                ctx.request_repaint();
            });
        }
        let found = self.found.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let printers: Vec<Printer> = match &found {
            Some(Ok((p, _))) => p.clone(),
            _ => Vec::new(),
        };
        // The default printer, once they're known (until the person picks).
        if !self.chosen_default
            && let Some(Ok((p, Some(default)))) = &found
        {
            if let Some(i) = p.iter().position(|x| &x.name == default) {
                self.dest = i + 1;
            }
            self.chosen_default = true;
        }
        let mut dests: Vec<String> = vec!["PDF File".into()];
        dests.extend(printers.iter().map(|p| if p.stopped() { format!("{} (stopped)", p.info) } else { p.info.clone() }));
        let dest_refs: Vec<&str> = dests.iter().map(String::as_str).collect();
        let papers: Vec<&str> = PAPERS.iter().map(|p| p.label).collect();
        let ranges = ["All", "From"];
        let to_pdf = self.dest == 0;

        let full = ui.max_rect().shrink(m::WINDOW_INSET);
        let divider = olo_kit::widgets::divider_for(ui, &["Printer:", "Paper:", "Copies:", "Pages:"]);
        let field_w = full.width();
        ui.scope_builder(egui::UiBuilder::new().max_rect(full).layout(egui::Layout::top_down(egui::Align::Min)), |ui| {
            ui.spacing_mut().item_spacing.y = m::TIGHT;
            ui.add(MenuField::new("print-dest", &mut self.dest, &dest_refs).label("Printer:").divider(divider).width(field_w));
            ui.add(MenuField::new("print-paper", &mut self.paper, &papers).label("Paper:").divider(divider).width(field_w));
            ui.add_enabled_ui(!to_pdf, |ui| ui.add(TextControl::new(&mut self.copies).label("Copies:").divider(divider).width(divider + 60.0)));
            ui.horizontal(|ui| {
                ui.add(MenuField::new("print-range", &mut self.range, &ranges).label("Pages:").divider(divider));
                ui.add_enabled_ui(self.range == 1, |ui| {
                    ui.add(TextControl::new(&mut self.from).width(44.0));
                    // "to", its capitals centred as the fields' text is.
                    let w = style::plain_text_width("to").ceil();
                    let (r, _) = ui.allocate_exact_size(vec2(w, m::CONTROL_HEIGHT), egui::Sense::hover());
                    let p = style::palette(ui.ctx());
                    let color = if ui.is_enabled() { style::c(p.panel_text) } else { style::c(p.panel_text).gamma_multiply(0.45) };
                    style::label(ui.painter(), r.left(), r.center().y, egui::Align::Min, "to", style::plain_font(), color);
                    let hint = self.page_count.map(|n| n.to_string()).unwrap_or_default();
                    ui.add(TextControl::new(&mut self.to).width(44.0).hint(&hint));
                });
            });
            if let Some(Err(e)) = &found {
                ui.add_space(m::TIGHT);
                ui.label(egui::RichText::new(e).small());
            }
        });
        let bw = m::BUTTON_MIN_WIDTH;
        let y = full.bottom() - m::CONTROL_HEIGHT;
        let go_r = Rect::from_min_size(pos2(full.right() - bw, y), vec2(bw, m::CONTROL_HEIGHT));
        let cancel_r = go_r.translate(vec2(-(bw + m::SPACING + 2.0), 0.0));
        if ui.put(cancel_r, Button::new("Cancel")).clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
            return Some(None);
        }
        let label = if to_pdf { "Save…" } else { "Print" };
        if ui.put(go_r, Button::new(label).default(true)).clicked() || ui.input(|i| i.key_pressed(Key::Enter)) {
            let n = |s: &str| s.trim().parse::<u32>().ok().filter(|v| *v > 0);
            let pages = (self.range == 1).then(|| {
                let a = n(&self.from).unwrap_or(1);
                let b = n(&self.to).or(self.page_count).unwrap_or(u32::MAX).max(a);
                (a, b)
            });
            return Some(Some(Choice {
                printer: (!to_pdf).then(|| printers.get(self.dest - 1).map(|p| p.name.clone())).flatten(),
                copies: if to_pdf { 1 } else { n(&self.copies).unwrap_or(1).min(999) },
                pages,
                paper: PAPERS[self.paper.min(PAPERS.len() - 1)],
            }));
        }
        None
    }
}
