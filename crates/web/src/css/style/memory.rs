use super::*;
use std::collections::HashSet;
/// Referenced style payload, shared allocations counted once across the input
/// set. Includes Arc controls; excludes allocator overhead, DOM/stylesheets,
/// caller containers and the temporary accounting hash table. Not process RSS.
#[derive(Clone, Copy, Debug, Default)]
pub struct Memory {
    pub styles: usize,
    pub styles_bytes: usize,
    pub defaults_bytes: usize,
    pub overrides_bytes: usize,
    pub fonts_bytes: usize,
    pub families_bytes: usize,
    pub customs_bytes: usize,
    pub numeric_bytes: usize,
    pub accounted_reserved_bytes: usize,
}
impl Memory {
    pub fn for_styles<'a>(styles: impl IntoIterator<Item = &'a Style>) -> Self {
        let mut account = Account::default();
        for style in styles {
            account.add(style);
        }
        account.memory
    }
}
#[derive(Default)]
pub(super) struct Account {
    seen: HashSet<usize>,
    pub memory: Memory,
}
impl Account {
    pub fn scratch_bytes(&self) -> usize {
        self.seen.capacity() * size_of::<usize>()
    }
    fn font(&mut self, font: &Arc<Font>) {
        if self.seen.insert(Arc::as_ptr(font) as usize) {
            self.memory.fonts_bytes += size_of::<Font>() + 2 * size_of::<usize>();
            self.families(&font.request.families);
        }
    }
    fn families(&mut self, families: &Families) {
        if self.seen.insert(families.identity()) {
            self.memory.families_bytes += 2 * size_of::<usize>() + size_of_val(families.as_slice());
            for family in families.as_slice() {
                if let Family::Named(name) = family
                    && self.seen.insert(Arc::as_ptr(name) as *const u8 as usize)
                {
                    self.memory.families_bytes += name.len() + 2 * size_of::<usize>();
                }
            }
        }
    }
    fn numeric(&mut self, value: &Value) {
        if let Value::Families(f) = value {
            self.families(f);
        }
        if let Value::Numeric(n) | Value::FitContent(n) = value
            && let math::Value::Expression(program) = n.expression()
            && self.seen.insert(Arc::as_ptr(program) as usize)
        {
            self.memory.numeric_bytes += n.expression().memory().accounted_reserved_bytes;
        }
    }
    pub fn add(&mut self, style: &Style) {
        self.memory.styles += 1;
        self.memory.styles_bytes += size_of::<Style>();
        if self.seen.insert(Arc::as_ptr(&style.defaults) as usize) {
            self.memory.defaults_bytes += size_of::<Defaults>() + 2 * size_of::<usize>() + style.defaults.values.len() * size_of::<Entry>() + style.defaults.specified.len() * size_of::<p::Value>();
            for value in &style.defaults.specified {
                if let p::Value::Families(f) = value {
                    self.families(f);
                } else {
                    self.memory.defaults_bytes += value.reserved_bytes();
                }
            }
            for entry in &style.defaults.values {
                self.numeric(&entry.value);
            }
            self.font(&style.defaults.initial_font);
        }
        self.font(&style.font);
        if self.seen.insert(Arc::as_ptr(&style.values) as *const Entry as usize) {
            self.memory.overrides_bytes += style.values.len() * size_of::<Entry>() + 2 * size_of::<usize>();
            for entry in style.values.iter() {
                self.numeric(&entry.value);
            }
        }
        if self.seen.insert(Arc::as_ptr(&style.customs) as usize) {
            self.memory.customs_bytes += size_of::<Dictionary>() + 2 * size_of::<usize>() + style.customs.values.len() * size_of::<Custom>();
            for custom in &style.customs.values {
                if self.seen.insert(Arc::as_ptr(&custom.name) as *const u8 as usize) {
                    self.memory.customs_bytes += custom.name.len() + 2 * size_of::<usize>();
                }
                if let Some(tokens) = custom.value.value()
                    && self.seen.insert(tokens.arena_identity())
                {
                    self.memory.customs_bytes += tokens.arena_reserved_bytes();
                }
            }
        }
        self.memory.accounted_reserved_bytes = self.memory.styles_bytes
            + self.memory.defaults_bytes
            + self.memory.overrides_bytes
            + self.memory.fonts_bytes
            + self.memory.families_bytes
            + self.memory.customs_bytes
            + self.memory.numeric_bytes;
    }
}
