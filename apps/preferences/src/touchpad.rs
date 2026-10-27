//! Touchpad and mouse: tapping, scrolling direction, click method, speed.

use olo_kit::{
    egui::Ui,
    widgets::{BBox, CheckBox, Slider},
};
use olo_protocol::Settings;

pub fn ui(ui: &mut Ui, s: &mut Settings) {
    let t = &mut s.touchpad;
    BBox::new("Touchpad").show(ui, |ui| {
        let mut speed = t.speed as f32;
        ui.add(Slider::new(&mut speed, -1.0..=1.0).step(0.05).label("Tracking speed").limits("Slow", "Fast"));
        t.speed = speed as f64;
        ui.add(CheckBox::new(&mut t.tap_to_click, "Tap to click"));
        ui.add_enabled(t.tap_to_click, CheckBox::new(&mut t.tap_and_drag, "Tap and drag"));
        ui.add(CheckBox::new(&mut t.natural_scroll, "Natural scrolling (content follows your fingers)"));
        ui.add(CheckBox::new(&mut t.clickfinger, "Click with two fingers for a secondary click"));
        ui.add(CheckBox::new(&mut t.disable_while_typing, "Ignore the touchpad while typing"));
    });
    BBox::new("Mouse").show(ui, |ui| {
        let mut speed = s.mouse_speed as f32;
        ui.add(Slider::new(&mut speed, -1.0..=1.0).step(0.05).label("Mouse speed").limits("Slow", "Fast"));
        s.mouse_speed = speed as f64;
        ui.add(CheckBox::new(&mut s.focus_follows_mouse, "Focus follows the pointer"));
    });
}
