//! The Print panel on olo-kit: where to (a printer CUPS has, or a PDF),
//! on what paper, how many copies, which pages; Cancel, and Print (Save…
//! for a PDF). A view in a window on a looper of its own; the printers
//! are asked for on a worker: until they come the choice is a PDF, then
//! the default printer unless the person has chosen.

use olo_kit::{
    Child, ClickEvent, Cx, Element, FontWeight, InteractiveElement as _, KeyDownEvent, ParentElement as _, SharedString, Styled as _, View, WindowHandle, WindowOptions, canvas, div, metrics as m, px,
    style::{self, Align},
    theme,
    widgets::{Button, MenuField, StringView, TextControl, TextField, button_row, text_field},
};

use super::{Choice, SIZE};
use crate::{PAPERS, ipp::Printer, local_paper};

type OnDone = Box<dyn FnOnce(Option<Choice>) + Send>;

/// The printers and the default one, as asked (or why not).
type Found = Result<(Vec<Printer>, Option<String>), String>;

/// The labels down the left, which the fields line up after.
const LABELS: [&str; 4] = ["Printer:", "Paper:", "Copies:", "Pages:"];
/// The page fields' width.
const PAGE_FIELD: f32 = 44.0;

/// The Print panel's view.
pub struct PrintView {
    found: Option<Found>,
    /// Where to: 0 is a PDF, then each printer.
    dest: usize,
    chosen_default: bool,
    paper: usize,
    /// 0: all pages, 1: a range.
    range: usize,
    copies: Child<TextField>,
    from: Child<TextField>,
    to: Child<TextField>,
    divider: f32,
    /// How many pages there are, if known.
    page_count: Option<u32>,
    on_done: Option<OnDone>,
}

/// What the panel is told: the printers CUPS has.
pub struct Printers(Found);

/// A Print panel's window, titled `title`, for [`show`] (over another
/// program's window: `.foreign_parent(handle)`).
pub fn window_options(title: impl Into<SharedString>) -> WindowOptions {
    WindowOptions::new(title, SIZE.0 as f32, SIZE.1 as f32).dialog()
}

/// Shows a Print panel in a window opened with `options`; `on_done` gets
/// the choice once Print (or Save…) is clicked, None for Cancel or the
/// window closed, on the panel's looper. `page_count`: how many pages
/// there are, if known (the range's hint).
pub fn show(options: WindowOptions, page_count: Option<u32>, on_done: impl FnOnce(Option<Choice>) + Send + 'static) -> WindowHandle<PrintView> {
    olo_kit::open_window(options, move |cx| {
        cx.activate_window();
        PrintView::new(page_count, Box::new(on_done), cx)
    })
}

impl PrintView {
    fn new(page_count: Option<u32>, on_done: OnDone, cx: &mut Cx<Self>) -> PrintView {
        let local = local_paper();
        let copies = text_field(cx, |f| f.name("Copies").max_len(3).with_text("1"));
        copies.update(cx, |f, cx| f.set_enabled(false, cx));
        let from = text_field(cx, |f| f.name("From page").max_len(5).with_text("1"));
        from.update(cx, |f, cx| f.set_enabled(false, cx));
        let hint = page_count.map(|n| n.to_string()).unwrap_or_default();
        let to = text_field(cx, |f| f.name("To page").max_len(5).hint(hint));
        to.update(cx, |f, cx| f.set_enabled(false, cx));
        let font = style::ui_font(FontWeight::NORMAL);
        let divider = LABELS.iter().map(|l| style::text_width_app(l, style::PLAIN_SIZE, &font)).fold(0.0, f32::max).ceil() + m::LABEL_GAP;
        // The printers, from CUPS on a worker.
        cx.work(|| crate::ipp::printers().map(|p| (p, crate::ipp::default_printer())), Printers);
        PrintView {
            found: None,
            dest: 0,
            chosen_default: false,
            paper: PAPERS.iter().position(|p| p.pwg == local.pwg).unwrap_or(0),
            range: 0,
            copies,
            from,
            to,
            divider,
            page_count,
            on_done: Some(on_done),
        }
    }

    fn printers(&self) -> &[Printer] {
        match &self.found {
            Some(Ok((p, _))) => p,
            _ => &[],
        }
    }

    /// The default printer, once they're known (unless the person chose).
    fn choose_default(&mut self, cx: &mut Cx<Self>) {
        if self.chosen_default {
            return;
        }
        self.chosen_default = true;
        if let Some(Ok((p, Some(default)))) = &self.found
            && let Some(i) = p.iter().position(|x| &x.name == default)
        {
            self.set_dest(i + 1, cx);
        }
    }

    fn set_dest(&mut self, dest: usize, cx: &mut Cx<Self>) {
        self.dest = dest;
        self.copies.update(cx, |f, cx| f.set_enabled(dest != 0, cx));
        cx.invalidate();
    }

    fn set_range(&mut self, range: usize, cx: &mut Cx<Self>) {
        self.range = range;
        for f in [&self.from, &self.to] {
            f.update(cx, |f, cx| f.set_enabled(range == 1, cx));
        }
        cx.invalidate();
    }

    /// What was chosen, as the panel stands.
    fn choice(&self, cx: &Cx<Self>) -> Choice {
        let n = |s: &str| s.trim().parse::<u32>().ok().filter(|v| *v > 0);
        let to_pdf = self.dest == 0;
        let page_count = self.page_count;
        let pages = (self.range == 1).then(|| {
            let a = n(self.from.read(cx).text()).unwrap_or(1);
            let b = n(self.to.read(cx).text()).or(page_count).unwrap_or(u32::MAX).max(a);
            (a, b)
        });
        Choice {
            printer: (!to_pdf).then(|| self.printers().get(self.dest - 1).map(|p| p.name.clone())).flatten(),
            copies: if to_pdf { 1 } else { n(self.copies.read(cx).text()).unwrap_or(1).min(999) },
            pages,
            paper: PAPERS[self.paper.min(PAPERS.len() - 1)],
        }
    }

    /// Answered (once): `choice`, or None for Cancel.
    fn finish(&mut self, choice: Option<Choice>) {
        if let Some(f) = self.on_done.take() {
            f(choice);
        }
    }

    fn done(&mut self, go: bool, cx: &mut Cx<Self>) {
        let choice = go.then(|| self.choice(cx));
        self.finish(choice);
        cx.close_window();
    }
}

impl View for PrintView {
    type Message = Printers;

    fn message(&mut self, Printers(found): Printers, cx: &mut Cx<Self>) {
        self.found = Some(found);
        self.choose_default(cx);
        cx.invalidate();
    }

    /// Closed from its tab: Cancel.
    fn window_closing(&mut self, _: &mut Cx<Self>) {
        self.finish(None);
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let divider = self.divider;
        let mut dests: Vec<SharedString> = vec!["PDF File".into()];
        dests.extend(self.printers().iter().map(|p| SharedString::from(if p.stopped() { format!("{} (stopped)", p.info) } else { p.info.clone() })));
        let papers: Vec<SharedString> = PAPERS.iter().map(|p| SharedString::from(p.label)).collect();
        let to_pdf = self.dest == 0;
        // (The pop-ups across the panel.)
        let full = SIZE.0 as f32 - 2.0 * m::WINDOW_INSET;
        let dest = MenuField::new("print-dest", dests, self.dest).label("Printer:").divider(divider).width(full).on_change(cx.listener(|v: &mut PrintView, i: &usize, cx| {
            v.chosen_default = true;
            v.set_dest(*i, cx);
        }));
        let paper = MenuField::new("print-paper", papers, self.paper).label("Paper:").divider(divider).width(full).on_change(cx.listener(|v: &mut PrintView, i: &usize, cx| {
            v.paper = *i;
            cx.invalidate();
        }));
        let copies = TextControl::new(cx, &self.copies).label("Copies:").divider(divider).width(divider + 60.0);
        let range = MenuField::new("print-range", vec!["All".into(), "From".into()], self.range)
            .label("Pages:")
            .divider(divider)
            .on_change(cx.listener(|v: &mut PrintView, i: &usize, cx| v.set_range(*i, cx)));
        // "to", its capitals centred as the fields' text is.
        let ranged = self.range == 1;
        let to_word = canvas(move |g, b| {
            let p = theme::palette();
            let ink = if ranged { style::c(p.panel_text) } else { style::disabled_text(&p) };
            style::label(g, b.left(), b.center().y, Align::Left, "to", style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), ink);
        })
        .paint_key(style::key(("to", ranged)))
        .w(px(style::text_width_app("to", style::PLAIN_SIZE, &style::ui_font(FontWeight::NORMAL)).ceil()))
        .h(px(m::CONTROL_HEIGHT));
        let pages = div()
            .flex()
            .items_center()
            .gap(px(m::SPACING))
            .h(px(m::CONTROL_HEIGHT))
            .child(range)
            .child(TextControl::new(cx, &self.from).width(PAGE_FIELD))
            .child(to_word)
            .child(TextControl::new(cx, &self.to).width(PAGE_FIELD));
        let error = match &self.found {
            Some(Err(e)) => Some(StringView::new(e.clone()).dim(true)),
            _ => None,
        };
        let cancel = Button::new("Cancel").on_click(cx.listener(|v: &mut PrintView, _: &ClickEvent, cx| v.done(false, cx)));
        let go = Button::new(if to_pdf { "Save…" } else { "Print" }).default(true).on_click(cx.listener(|v: &mut PrintView, _: &ClickEvent, cx| v.done(true, cx)));
        div()
            .size_full()
            .on_window_key_down(cx.listener(|v: &mut PrintView, e: &KeyDownEvent, cx| {
                if e.keystroke.key == "escape" {
                    v.done(false, cx);
                }
            }))
            .p(px(m::WINDOW_INSET))
            .flex()
            .flex_col()
            .gap(px(m::TIGHT))
            .child(dest)
            .child(paper)
            .child(copies)
            .child(pages)
            .children(error)
            .child(div().flex_1())
            .child(button_row().child(cancel).child(go))
    }
}
