//! The Password Generator window: a recipe, a preview with its strength,
//! Copy, and Use when it was opened from a password field.

use onepassword::{PasswordRecipe, SeparatorType, WordListType, generate};
use serde::{Deserialize, Serialize};
use tabula_kit::{
    egui::{self, Sense, Ui, vec2},
    metrics as m, style,
    widgets::{Button, CheckBox, MenuField, Slider, StatusBar, button_row},
};

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

pub fn meter_color(p: &tabula_theme::Palette, bits: f64) -> egui::Color32 {
    if bits < 36.0 {
        style::c(p.failure)
    } else if bits < 60.0 {
        style::c(tabula_theme::rgb(0xe8, 0xa8, 0x18))
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

pub enum Action {
    Copy(String),
    Use(String),
    Close,
}

impl Generator {
    pub fn new(recipe: Recipe, target: Option<(String, String)>) -> Generator {
        let password = recipe.generate();
        Generator { recipe, password, target }
    }

    pub fn ui(&mut self, ui: &mut Ui) -> Option<Action> {
        let p = style::palette(ui.ctx());
        let mut action = None;
        let before = self.recipe.clone();
        ui.spacing_mut().item_spacing.y = m::SPACING;

        // The password, large, in a well.
        let (well, _) = ui.allocate_exact_size(vec2(ui.available_width(), 64.0), Sense::hover());
        let well = style::snap(ui.ctx(), well);
        ui.painter().rect_filled(well, 0.0, style::c(p.document_background));
        style::sunken(ui.painter(), well, &p, false);
        let font = style::mono_font(if self.password.chars().count() > 34 { 13.0 } else { 16.0 });
        let galley = ui.painter().layout(self.password.clone(), font, style::c(p.document_text), well.width() - 2.0 * m::SPACING);
        let pos = well.center() - galley.size() / 2.0;
        ui.painter().galley(pos, galley, style::c(p.document_text));

        let bits = generate::entropy_bits(self.recipe.recipe());
        let (word, fraction) = verdict(bits);
        let label = format!("{word} ({bits:.0} bits)");
        ui.add(StatusBar::new(fraction).label(&label).color(meter_color(&p, bits)).bar_height(6.0));

        ui.add(MenuField::new("gen-kind", &mut self.recipe.kind, &KINDS).label("Type:").divider(80.0));
        match self.recipe.kind {
            1 => {
                let mut words = self.recipe.words as f32;
                let text = format!("{}", self.recipe.words);
                let range = *generate::WORD_COUNT.start() as f32..=*generate::WORD_COUNT.end() as f32;
                ui.add(Slider::new(&mut words, range).label("Words:").value_text(text).step(1.0));
                self.recipe.words = words.round() as u32;
                ui.add(MenuField::new("gen-sep", &mut self.recipe.separator, &SEPARATORS).label("Separator:").divider(80.0));
                ui.add(MenuField::new("gen-list", &mut self.recipe.word_list, &WORD_LISTS).label("Made of:").divider(80.0));
                ui.add(CheckBox::new(&mut self.recipe.capitalize, "Capitalise"));
            }
            2 => {
                let mut len = self.recipe.pin as f32;
                let text = format!("{}", self.recipe.pin);
                ui.add(Slider::new(&mut len, 4.0..=12.0).label("Digits:").value_text(text).step(1.0));
                self.recipe.pin = len.round() as u32;
            }
            _ => {
                let mut len = self.recipe.length as f32;
                let text = format!("{}", self.recipe.length);
                ui.add(Slider::new(&mut len, 8.0..=64.0).label("Characters:").value_text(text).step(1.0));
                self.recipe.length = len.round() as u32;
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = m::SPACING * 2.0;
                    ui.add(CheckBox::new(&mut self.recipe.digits, "Numbers"));
                    ui.add(CheckBox::new(&mut self.recipe.symbols, "Symbols"));
                });
            }
        }
        if self.recipe.kind != before.kind
            || self.recipe.length != before.length
            || self.recipe.digits != before.digits
            || self.recipe.symbols != before.symbols
            || self.recipe.words != before.words
            || self.recipe.capitalize != before.capitalize
            || self.recipe.separator != before.separator
            || self.recipe.word_list != before.word_list
            || self.recipe.pin != before.pin
        {
            self.password = self.recipe.generate();
        }

        // The buttons sit on the window's bottom edge.
        let row = m::CONTROL_HEIGHT + 4.0;
        ui.add_space((ui.available_height() - row - m::SPACING).max(0.0));
        ui.allocate_ui(vec2(ui.available_width(), row), |ui| {
            button_row(ui, |ui| {
                if self.target.is_some() {
                    if ui.add(Button::new("Use").default(true)).clicked() {
                        action = Some(Action::Use(self.password.clone()));
                    }
                    if ui.add(Button::new("Copy")).clicked() {
                        action = Some(Action::Copy(self.password.clone()));
                    }
                } else if ui.add(Button::new("Copy").default(true)).clicked() {
                    action = Some(Action::Copy(self.password.clone()));
                }
                if ui.add(Button::new("Regenerate")).clicked() {
                    self.password = self.recipe.generate();
                }
            });
        });
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            action = Some(Action::Close);
        }
        action
    }
}
