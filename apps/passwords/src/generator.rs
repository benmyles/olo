//! The Password Generator window: a recipe, a preview with its strength,
//! Copy, and Use when it was opened from a password field.

use olo_kit::{
    ClickEvent, Cx, Element, FocusHandle, FontWeight, Hsla, InteractiveElement as _, IntoElement as _, KeyDownEvent, ParentElement as _, SharedString, Styled as _, View, agent, canvas, div,
    metrics as m, px, style, theme,
    widgets::{Button, CheckBox, MenuField, Slider, StatusBar, button_row},
};
use onepassword::{PasswordRecipe, SeparatorType, WordListType, generate};
use serde::{Deserialize, Serialize};

use crate::app;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Recipe {
    /// 0 random, 1 memorable, 2 PIN.
    pub kind: usize,
    pub length: u32,
    pub digits: bool,
    pub symbols: bool,
    pub words: u32,
    pub capitalize: bool,
    pub separator: usize,
    pub word_list: usize,
    pub pin: u32,
}

impl Default for Recipe {
    fn default() -> Self {
        Recipe { kind: 0, length: 20, digits: true, symbols: true, words: 4, capitalize: true, separator: 1, word_list: 0, pin: 6 }
    }
}

const KINDS: [&str; 3] = ["Random Password", "Memorable Password", "PIN Code"];
const SEPARATORS: [&str; 7] = ["Digits", "Hyphens", "Spaces", "Periods", "Commas", "Underscores", "Digits and Symbols"];
const WORD_LISTS: [&str; 3] = ["Words", "Syllables", "Three Letters"];

impl Recipe {
    pub fn recipe(&self) -> PasswordRecipe {
        match self.kind {
            1 => PasswordRecipe::Memorable {
                separator: [
                    SeparatorType::Digits,
                    SeparatorType::Hyphens,
                    SeparatorType::Spaces,
                    SeparatorType::Periods,
                    SeparatorType::Commas,
                    SeparatorType::Underscores,
                    SeparatorType::DigitsAndSymbols,
                ][self.separator.min(6)],
                capitalize: self.capitalize,
                word_list: [WordListType::FullWords, WordListType::Syllables, WordListType::ThreeLetters][self.word_list.min(2)],
                word_count: self.words,
            },
            2 => PasswordRecipe::Pin { length: self.pin },
            _ => PasswordRecipe::Random { include_digits: self.digits, include_symbols: self.symbols, length: self.length },
        }
    }

    pub fn generate(&self) -> String {
        generate::password(self.recipe()).unwrap_or_default()
    }
}

/// How strong `bits` of entropy is, in words and as a meter colour.
pub fn verdict(bits: f64) -> (&'static str, f32) {
    let fraction = (bits / 128.0).clamp(0.04, 1.0) as f32;
    let word = match bits {
        b if b < 28.0 => "Terrible",
        b if b < 36.0 => "Weak",
        b if b < 60.0 => "Fair",
        b if b < 80.0 => "Good",
        b if b < 100.0 => "Very Good",
        _ => "Excellent",
    };
    (word, fraction)
}

pub fn meter_color(p: &olo_theme::Palette, bits: f64) -> Hsla {
    if bits < 36.0 {
        style::c(p.failure)
    } else if bits < 60.0 {
        style::c(p.warning)
    } else {
        style::c(p.success)
    }
}

pub struct Generator {
    pub recipe: Recipe,
    pub password: String,
    /// The editor field it was opened for (item id, field id).
    pub target: Option<(String, String)>,
}

impl Generator {
    pub fn new(recipe: Recipe, target: Option<(String, String)>) -> Generator {
        let password = recipe.generate();
        Generator { recipe, password, target }
    }
}

/// What the generator's window is told by the app.
pub enum GeneratorMsg {
    /// Opened again, for this field of the editor (or none).
    Target(Option<(String, String)>),
    Close,
}

/// The generator's window: the password, large, in a well, how strong it
/// is, the recipe, and Use (opened from a field), Copy and Regenerate.
pub struct GeneratorView {
    pub generator: Generator,
    focus: FocusHandle,
}

const DIVIDER: f32 = 80.0;

impl GeneratorView {
    pub fn new(generator: Generator, cx: &mut Cx<Self>) -> GeneratorView {
        let focus = FocusHandle::new();
        focus.focus(cx);
        GeneratorView { generator, focus }
    }

    /// The recipe changed: a new password, and the app keeps the recipe.
    fn change(&mut self, f: impl FnOnce(&mut Recipe), cx: &mut Cx<Self>) {
        f(&mut self.generator.recipe);
        self.generator.password = self.generator.recipe.generate();
        app().settings.generator = self.generator.recipe.clone();
        cx.invalidate();
    }

    fn copy(&self) {
        app().copy_secret("the password", &self.generator.password);
    }
}

impl View for GeneratorView {
    type Message = GeneratorMsg;

    fn message(&mut self, message: GeneratorMsg, cx: &mut Cx<Self>) {
        match message {
            GeneratorMsg::Target(target) => {
                self.generator.target = target;
                cx.activate_window();
                cx.invalidate();
            }
            GeneratorMsg::Close => cx.close_window(),
        }
    }

    fn window_closing(&mut self, _: &mut Cx<Self>) {
        app().generator_closed();
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let p = theme::palette();
        let r = &self.generator.recipe;
        let bits = generate::entropy_bits(r.recipe());
        let (word, fraction) = verdict(bits);
        let strength = StatusBar::new(fraction).label(format!("{word} ({bits:.0} bits)")).color(meter_color(&p, bits)).bar_height(6.0);

        // The password, large, in a well; a long one over lines.
        let password: SharedString = self.generator.password.clone().into();
        let well = canvas(move |g, b| {
            let p = theme::palette();
            let b = style::snap(g, b);
            style::fill_rect(g, b, style::c(p.document_background));
            style::sunken(g, b, &p, false);
            let size = if password.chars().count() > 34 { 13.0 } else { 16.0 };
            let font = style::mono_font(FontWeight::NORMAL);
            let advance = style::text_width("0", size, font.clone()).max(px(1.0));
            let per_line = (f32::from(b.size.width - px(2.0 * m::SPACING)) / f32::from(advance)).floor().max(1.0) as usize;
            let chars: Vec<char> = password.chars().collect();
            let lines: Vec<String> = chars.chunks(per_line).map(|c| c.iter().collect()).collect();
            let line_h = size * 1.3;
            let top = b.center().y - px(line_h * lines.len() as f32 / 2.0);
            for (i, line) in lines.into_iter().enumerate() {
                style::label(g, b.center().x, top + px(line_h * (i as f32 + 0.5)), style::Align::Center, line, size, font.clone(), style::c(p.document_text));
            }
            agent::record(g, agent::Node { role: "Label", name: Some("Password".into()), value: Some(password.clone()), bounds: b, enabled: true, ..Default::default() });
        })
        .w_full()
        .h(px(64.0))
        .flex_shrink_0();

        let kinds: Vec<SharedString> = KINDS.iter().map(|k| SharedString::from(*k)).collect();
        let mut form = div().flex().flex_col().gap(px(m::SPACING)).child(well).child(strength).child(MenuField::new("gen-kind", kinds, r.kind).label("Type:").divider(DIVIDER).on_change(cx.listener(
            |g: &mut GeneratorView, i: &usize, cx| {
                let i = *i;
                g.change(|r| r.kind = i, cx)
            },
        )));
        let slider = |id: &'static str, label: &'static str, value: u32, range: std::ops::RangeInclusive<f32>, set: fn(&mut Recipe, u32)| {
            Slider::new(id, value as f32, range).label(label).value_text(value.to_string()).step(1.0).on_change(cx.listener(move |g: &mut GeneratorView, v: &f32, cx| {
                let v = v.round() as u32;
                g.change(|r| set(r, v), cx)
            }))
        };
        let menu = |id: &'static str, label: &'static str, options: &[&'static str], chosen: usize, set: fn(&mut Recipe, usize)| {
            let options: Vec<SharedString> = options.iter().map(|o| SharedString::from(*o)).collect();
            MenuField::new(id, options, chosen).label(label).divider(DIVIDER).on_change(cx.listener(move |g: &mut GeneratorView, i: &usize, cx| {
                let i = *i;
                g.change(|r| set(r, i), cx)
            }))
        };
        let check = |id: &'static str, label: &'static str, on: bool, set: fn(&mut Recipe, bool)| {
            CheckBox::new(label, on).id(id).on_toggle(cx.listener(move |g: &mut GeneratorView, on: &bool, cx| {
                let on = *on;
                g.change(|r| set(r, on), cx)
            }))
        };
        let range = *generate::WORD_COUNT.start() as f32..=*generate::WORD_COUNT.end() as f32;
        form = match r.kind {
            1 => form
                .child(slider("gen-words", "Words:", r.words, range, |r, v| r.words = v))
                .child(menu("gen-sep", "Separator:", &SEPARATORS, r.separator, |r, i| r.separator = i))
                .child(menu("gen-list", "Made of:", &WORD_LISTS, r.word_list, |r, i| r.word_list = i))
                .child(check("gen-capitalize", "Capitalise", r.capitalize, |r, on| r.capitalize = on)),
            2 => form.child(slider("gen-pin", "Digits:", r.pin, 4.0..=12.0, |r, v| r.pin = v)),
            _ => form.child(slider("gen-length", "Characters:", r.length, 8.0..=64.0, |r, v| r.length = v)).child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(m::SPACING * 2.0))
                    .child(check("gen-digits", "Numbers", r.digits, |r, on| r.digits = on))
                    .child(check("gen-symbols", "Symbols", r.symbols, |r, on| r.symbols = on)),
            ),
        };

        // The buttons on the window's bottom edge.
        let mut row = button_row();
        row = row.child(Button::new("Regenerate").on_click(cx.listener(|g: &mut GeneratorView, _: &ClickEvent, cx| g.change(|_| {}, cx))));
        if self.generator.target.is_some() {
            row = row.child(Button::new("Copy").on_click(cx.listener(|g: &mut GeneratorView, _: &ClickEvent, _| g.copy()))).child(Button::new("Use").default(true).on_click(cx.listener(
                |g: &mut GeneratorView, _: &ClickEvent, cx| {
                    let (pw, target) = (g.generator.password.clone(), g.generator.target.clone());
                    app().use_generated(pw, target);
                    cx.close_window();
                },
            )));
        } else {
            row = row.child(Button::new("Copy").default(true).on_click(cx.listener(|g: &mut GeneratorView, _: &ClickEvent, _| g.copy())));
        }
        div()
            .size_full()
            .track_focus(&self.focus)
            .on_key_down(|e: &KeyDownEvent, cx| {
                if e.keystroke.key == "escape" {
                    cx.stop_propagation();
                    cx.close_window();
                }
            })
            .flex()
            .flex_col()
            .p(px(m::WINDOW_INSET))
            .child(form)
            .child(div().flex_1())
            .child(row)
            .into_any_element()
    }
}
