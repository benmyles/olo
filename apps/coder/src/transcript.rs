//! A virtual, selectable rich document. Parsing and syntax captures arrive from
//! the presentation worker; the UI only lays out visible paragraphs.
use olo_ui::{
    editor::{self, Document, Editor, EditorHost},
    gpui::{AppContext as _, Context, Entity, IntoElement, Render, Window},
    rich_text,
};
use std::sync::Arc;
pub struct Transcript {
    editor: Editor,
    doc: Arc<rich_text::Text>,
    pub follow: bool,
    client: crate::client::Handle,
}
impl Transcript {
    pub fn new(cx: &mut olo_ui::gpui::App, client: crate::client::Handle) -> Entity<Self> {
        let editor = Editor::new(cx);
        cx.new(|_| Self { editor, doc: Arc::default(), follow: true, client })
    }
    pub fn set(&mut self, doc: Arc<rich_text::Text>, cx: &mut Context<Self>) {
        if Arc::ptr_eq(&self.doc, &doc) {
            return;
        }
        self.editor.changed(&self.doc.text, &doc.text);
        // Text can stay identical while a closing Markdown delimiter changes
        // its style. Only invalidate from the first changed semantic paragraph.
        let first = self.doc.paragraphs.iter().zip(&doc.paragraphs).find(|(a, b)| a.start != b.start || a.kind != b.kind).map(|(a, b)| a.start.min(b.start));
        let changed_run = self.doc.runs.iter().zip(&doc.runs).find(|(a, b)| a != b).map(|(a, b)| a.range.start.min(b.range.start));
        let first = first.into_iter().chain(changed_run).min();
        if self.doc.text == doc.text || first.is_some() {
            self.editor.restyled(first.unwrap_or(0)..doc.text.len());
        }
        self.doc = doc;
        if self.follow {
            let end = self.doc.text.len();
            self.editor.select(&self.doc.text, end..end);
        }
        cx.notify();
    }
}
impl EditorHost for Transcript {
    fn open_link(&mut self, _: usize, link: String, _: &mut Window, _: &mut Context<Self>) {
        if let Some(id) = link.strip_prefix("olo-coder:event/").and_then(|s| s.parse::<i64>().ok()) {
            self.client.submit(format!("/event {id}"));
        } else if link.starts_with("https://") || link.starts_with("http://") {
            std::thread::spawn(move || {
                let _ = olo_xdg::open::open(&olo_xdg::open::Target::parse(&link));
            });
        }
    }
    fn editor(&mut self, _: usize, cx: &mut Context<Self>, f: &mut dyn FnMut(&mut Editor, &mut dyn Document, &mut Context<Self>)) {
        let palette = olo_ui::theme::palette(cx);
        let mut doc = rich_text::Document { content: &self.doc, palette: &palette };
        f(&mut self.editor, &mut doc, cx);
    }
}
impl Render for Transcript {
    fn render(&mut self, w: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        editor::view(&cx.entity(), 0, &self.editor, editor::Options { editable: false, margin: (olo_ui::metrics::WINDOW_INSET, olo_ui::metrics::SPACING), ..Default::default() }, w)
    }
}
