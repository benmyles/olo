//! Making, renaming and describing a vault (Ben, 2026-11-14: "olo
//! passwords should support creating/managing new vaults"): File ▸ New
//! Vault… and Vault Info…, and a vault's right-click in the sources,
//! open this; Delete Vault… asks first (main.rs). Each is done here at
//! once and in 1Password at the next sync, which the person reviews
//! (vault.rs's `VaultChange`).

use olo_kit::{
    ClickEvent, Cx, Element, InteractiveElement as _, IntoElement as _, KeyDownEvent, ParentElement as _, Styled as _, View, WindowHandle, WindowOptions, div, metrics as m, px, style, theme,
    widgets::{Button, StringView, TextControl, TextField, TextFieldEvent, button_width, text_field},
};

use crate::{app, store::Job};

/// The dialog for a new vault (`id` None) or an existing one.
pub struct VaultInfo {
    id: Option<String>,
    name: olo_kit::Child<TextField>,
    description: olo_kit::Child<TextField>,
    /// Where it goes, or what's wrong.
    note: String,
    wrong: bool,
}

/// The labels' column.
const DIVIDER: f32 = 92.0;

impl VaultInfo {
    /// Opens it: for vault `id` (its name and description now), or a new
    /// one; `note` says where it goes.
    pub fn open(id: Option<String>, title: &str, description: &str, note: String) -> WindowHandle<VaultInfo> {
        let window = if id.is_some() { "Vault Info" } else { "New Vault" };
        let (title, description) = (title.to_owned(), description.to_owned());
        let options = WindowOptions { fixed: true, ..WindowOptions::new(window, 440.0, 160.0).dialog() };
        olo_kit::open_window(options, move |cx| {
            let name = text_field(cx, |f| f.name("Name").with_text(title));
            let description = text_field(cx, |f| f.name("Description").hint("What it’s for (optional)").with_text(description));
            for field in [&name, &description] {
                cx.subscribe(field, |v: &mut VaultInfo, e: &TextFieldEvent, cx| match e {
                    TextFieldEvent::Entered => v.done(true, cx),
                    TextFieldEvent::Changed if v.wrong => {
                        v.wrong = false;
                        v.note.clear();
                        cx.invalidate();
                    }
                    _ => {}
                });
            }
            name.focus(cx);
            VaultInfo { id, name, description, note, wrong: false }
        })
    }

    fn done(&mut self, ok: bool, cx: &mut Cx<Self>) {
        if ok {
            let title = self.name.read(cx).text().trim().to_owned();
            let description = self.description.read(cx).text().trim().to_owned();
            if title.is_empty() {
                self.note = "A vault needs a name.".into();
                self.wrong = true;
                return cx.invalidate();
            }
            let job = match self.id.clone() {
                Some(id) => Job::EditVault { id, title, description },
                None => Job::CreateVault { title, description },
            };
            app().store.send(job);
        }
        cx.close_window();
    }
}

impl View for VaultInfo {
    type Message = ();

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let p = theme::palette();
        let act = if self.id.is_some() { "Save" } else { "Create" };
        let bw = button_width(act, false).max(button_width("Cancel", false));
        let note = StringView::new(self.note.clone()).wrap(true).color(if self.wrong { style::c(p.failure) } else { style::disabled_text(&p) });
        div()
            .p(px(m::WINDOW_INSET))
            .flex()
            .flex_col()
            .gap(px(m::STACK))
            .on_key_down(cx.listener(|v: &mut VaultInfo, e: &KeyDownEvent, cx| {
                if e.keystroke.key == "escape" {
                    v.done(false, cx);
                }
            }))
            .child(TextControl::new(cx, &self.name).label("Name:").divider(DIVIDER))
            .child(TextControl::new(cx, &self.description).label("Description:").divider(DIVIDER))
            .child(div().pl(px(DIVIDER)).child(note))
            .child(
                div()
                    .mt(px(m::SPACING))
                    .flex()
                    .justify_end()
                    .gap(px(m::SPACING))
                    .child(Button::new("Cancel").min_width(bw).on_click(cx.listener(|v: &mut VaultInfo, _: &ClickEvent, cx| v.done(false, cx))))
                    .child(Button::new(act).default(true).min_width(bw).on_click(cx.listener(|v: &mut VaultInfo, _: &ClickEvent, cx| v.done(true, cx))))
                    .into_element(),
            )
    }
}
