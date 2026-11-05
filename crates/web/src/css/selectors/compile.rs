use super::*;
use crate::css::{Component, Kind};
#[derive(Clone, Copy, Default)]
struct Context {
    depth: usize,
    real: bool,
    in_has: bool,
    relative: bool,
    ignore_subject_namespace: bool,
}
struct Compiler<'a> {
    out: Selectors,
    symbol_index: HashMap<Arc<str>, u32>,
    options: CompileOptions<'a>,
    limits: CompileLimits,
    cancelled: Option<&'a AtomicBool>,
    work: usize,
    test_count: usize,
    step_count: usize,
    selector_count: usize,
    default_ns: u32,
}
#[derive(Clone, Copy)]
struct Mark {
    lists: usize,
    links: usize,
    complexes: usize,
    steps: usize,
    tests: usize,
    nth: usize,
    failures: usize,
    languages: usize,
    symbols: usize,
}
pub(super) fn compile(values: Components<'_>, options: CompileOptions<'_>, limits: CompileLimits, cancelled: Option<&AtomicBool>) -> Result<Selectors> {
    let mut c = Compiler {
        out: Selectors {
            lists: Vec::new(),
            links: Vec::new(),
            complexes: Vec::new(),
            steps: Vec::new(),
            tests: Vec::new(),
            nth: Vec::new(),
            symbols: Vec::new(),
            symbol_bytes: 0,
            failures: Vec::new(),
            languages: Vec::new(),
            root: 0,
        },
        symbol_index: HashMap::new(),
        options,
        limits,
        cancelled,
        work: 0,
        test_count: 0,
        step_count: 0,
        selector_count: 0,
        default_ns: ANY,
    };
    check(cancelled)?;
    if let Some(ns) = options.default_namespace {
        c.default_ns = c.namespace(ns)?;
    }
    c.out.root = c.list(values, Context::default(), false)?;
    fn trim<T>(v: &mut Vec<T>) {
        if v.capacity() > v.len().saturating_mul(2).max(32) {
            v.shrink_to_fit();
        }
    }
    trim(&mut c.out.lists);
    trim(&mut c.out.links);
    trim(&mut c.out.complexes);
    trim(&mut c.out.steps);
    trim(&mut c.out.tests);
    trim(&mut c.out.nth);
    trim(&mut c.out.symbols);
    trim(&mut c.out.failures);
    trim(&mut c.out.languages);
    Ok(c.out)
}
impl Compiler<'_> {
    fn tick(&mut self, n: usize) -> Result<()> {
        check(self.cancelled)?;
        self.work = self.work.checked_add(n).ok_or(Error::CompileLimit)?;
        if self.work > self.limits.work { Err(Error::CompileLimit) } else { Ok(()) }
    }
    fn symbol(&mut self, s: &str) -> Result<u32> {
        self.tick(s.len().max(1))?;
        if let Some(&id) = self.symbol_index.get(s) {
            return Ok(id);
        }
        if s.len() > self.limits.symbol_bytes.saturating_sub(self.out.symbol_bytes) || self.out.symbols.len() >= NONE as usize {
            return Err(Error::CompileLimit);
        }
        let id = self.out.symbols.len() as u32;
        let symbol: Arc<str> = Arc::from(s);
        self.out.symbol_bytes += s.len();
        self.symbol_index.insert(symbol.clone(), id);
        self.out.symbols.push(symbol);
        Ok(id)
    }
    fn namespace(&mut self, ns: &str) -> Result<u32> {
        if ns.is_empty() { Ok(NONE) } else { self.symbol(ns) }
    }
    fn prefix(&mut self, prefix: &str) -> Result<u32> {
        for &(name, namespace) in self.options.namespaces.iter().rev() {
            self.tick(name.len().max(1))?;
            if name == prefix {
                return self.namespace(namespace);
            }
        }
        Err(Error::Invalid)
    }
    fn mark(&self) -> Mark {
        Mark {
            lists: self.out.lists.len(),
            links: self.out.links.len(),
            complexes: self.out.complexes.len(),
            steps: self.out.steps.len(),
            tests: self.out.tests.len(),
            nth: self.out.nth.len(),
            failures: self.out.failures.len(),
            languages: self.out.languages.len(),
            symbols: self.out.symbols.len(),
        }
    }
    fn rewind(&mut self, m: Mark) {
        self.out.lists.truncate(m.lists);
        self.out.links.truncate(m.links);
        self.out.complexes.truncate(m.complexes);
        self.out.steps.truncate(m.steps);
        self.out.tests.truncate(m.tests);
        self.out.nth.truncate(m.nth);
        self.out.failures.truncate(m.failures);
        self.out.languages.truncate(m.languages);
        for symbol in self.out.symbols.drain(m.symbols..) {
            self.out.symbol_bytes -= symbol.len();
            self.symbol_index.remove(&symbol);
        }
    }
    fn test(&mut self, values: &mut SmallVec<[Test; 8]>, test: Test) -> Result<()> {
        self.tick(1)?;
        self.test_count = self.test_count.checked_add(1).ok_or(Error::CompileLimit)?;
        if self.test_count > self.limits.tests || self.test_count >= NONE as usize {
            return Err(Error::CompileLimit);
        }
        values.push(test);
        Ok(())
    }
    fn bump<'a>(&mut self, input: &mut Components<'a>) -> Result<Component<'a>> {
        self.tick(1)?;
        input.next().ok_or(Error::Invalid)
    }
    fn list(&mut self, values: Components<'_>, ctx: Context, forgiving: bool) -> Result<u32> {
        self.tick(1)?;
        if ctx.depth > self.limits.depth.min(128) {
            return Err(Error::DepthLimit);
        }
        let mut rest = values.clone();
        let mut branch = values;
        let mut selected = SmallVec::<[u32; 4]>::new();
        let mut specificity = Specificity::default();
        loop {
            // Range endpoints are borrowed from the original arena; no
            // retokenization or per-branch component-vector allocation.
            let start = branch.clone().next().map(|v| v.at);
            let mut stop = None;
            for v in rest.by_ref() {
                self.tick(1)?;
                if v.kind() == Kind::Comma {
                    stop = Some(v.at);
                    break;
                }
            }
            let end = stop.or_else(|| branch.clone().last().map(Component::after));
            let input = match (start, end) {
                (Some(start), Some(end)) => branch.between(start, end),
                _ => branch.clone(),
            };
            let mark = self.mark();
            match self.complex(input, ctx) {
                Ok(id) => {
                    specificity = specificity.max(self.out.complexes[id as usize].specificity);
                    selected.push(id);
                }
                Err(Error::Invalid) if forgiving => self.rewind(mark),
                Err(e) => return Err(e),
            }
            if stop.is_none() {
                break;
            }
            branch = rest.clone();
        }
        if self.out.lists.len() >= NONE as usize || self.out.links.len() + selected.len() >= NONE as usize {
            return Err(Error::CompileLimit);
        }
        let featureless = selected.iter().any(|&id| self.out.complexes[id as usize].featureless);
        let start = self.out.links.len() as u32;
        self.out.links.extend(selected);
        let id = self.out.lists.len() as u32;
        self.out.lists.push(List { links: start..self.out.links.len() as u32, specificity, featureless });
        Ok(id)
    }
    fn complex(&mut self, mut input: Components<'_>, ctx: Context) -> Result<u32> {
        self.selector_count = self.selector_count.checked_add(1).ok_or(Error::CompileLimit)?;
        if self.selector_count > self.limits.selectors || self.selector_count >= NONE as usize {
            return Err(Error::CompileLimit);
        }
        self.skip(&mut input)?;
        let mut relation = Relation::None;
        let mut steps = SmallVec::<[Step; 4]>::new();
        let mut specificity = Specificity::default();
        let mut pseudo = false;
        let leading = if ctx.relative { self.combinator(&mut input)?.unwrap_or(Relation::Descendant) } else { Relation::None };
        if ctx.relative {
            let mut tests = SmallVec::new();
            self.test(&mut tests, Test::Structural(Structural::Anchor))?;
            steps.push(self.store_step(tests, Relation::None, ANY, false)?);
            relation = leading;
            self.skip(&mut input)?;
        }
        loop {
            let (step, spec, has_pseudo) = self.compound(&mut input, relation, ctx)?;
            specificity = specificity.add(spec);
            steps.push(step);
            pseudo |= has_pseudo;
            let space = self.skip(&mut input)?;
            if input.clone().next().is_none() {
                break;
            }
            if pseudo {
                return Err(Error::Invalid);
            }
            relation = match self.combinator(&mut input)? {
                Some(r) => r,
                None if space => Relation::Descendant,
                _ => return Err(Error::Invalid),
            };
            self.skip(&mut input)?;
        }
        if ctx.ignore_subject_namespace
            && let Some(last) = steps.last_mut()
            && last.implicit_ns
        {
            last.namespace = ANY;
        }
        if self.out.steps.len() + steps.len() >= NONE as usize {
            return Err(Error::CompileLimit);
        }
        let pseudo = steps.last().and_then(|step| self.out.tests[step.tests.start as usize..step.tests.end as usize].iter().find_map(|t| if let Test::Pseudo(p) = t { Some(*p) } else { None }));
        let featureless = steps.last().is_some_and(|s| s.featureless);
        let start = self.out.steps.len() as u32;
        self.out.steps.extend(steps);
        let id = self.out.complexes.len() as u32;
        self.out.complexes.push(Complex { steps: start..self.out.steps.len() as u32, specificity, relative: leading, pseudo, featureless });
        Ok(id)
    }
    fn store_step(&mut self, tests: SmallVec<[Test; 8]>, relation: Relation, namespace: u32, implicit_ns: bool) -> Result<Step> {
        self.step_count = self.step_count.checked_add(1).ok_or(Error::CompileLimit)?;
        if self.step_count > self.limits.steps || self.step_count >= NONE as usize {
            return Err(Error::CompileLimit);
        }
        let mut other = false;
        let featureless = tests.iter().all(|test| match *test {
            Test::Structural(Structural::Scope | Structural::Anchor) => {
                other = true;
                true
            }
            Test::Logical { mode: Logical::Is | Logical::Not, list } => {
                let allowed = self.out.lists[list as usize].featureless;
                other |= allowed;
                allowed
            }
            Test::Logical { mode: Logical::Has, .. } => true,
            _ => false,
        }) && other;
        let start = self.out.tests.len() as u32;
        self.out.tests.extend(tests);
        Ok(Step { tests: start..self.out.tests.len() as u32, relation, namespace, implicit_ns, featureless })
    }
    fn skip(&mut self, input: &mut Components<'_>) -> Result<bool> {
        let mut found = false;
        while input.clone().next().is_some_and(|v| v.kind() == Kind::Whitespace) {
            self.bump(input)?;
            found = true;
        }
        Ok(found)
    }
    fn combinator(&mut self, input: &mut Components<'_>) -> Result<Option<Relation>> {
        let relation = match input.clone().next().and_then(Component::character) {
            Some('>') => Relation::Child,
            Some('+') => Relation::Next,
            Some('~') => Relation::Later,
            _ => return Ok(None),
        };
        self.bump(input)?;
        Ok(Some(relation))
    }
    fn qualified(&mut self, input: &mut Components<'_>, attribute: bool) -> Result<(u32, u32)> {
        let first = self.bump(input)?;
        let mut ns = if attribute { NONE } else { self.default_ns };
        let mut name = first;
        if first.character() == Some('|') {
            ns = NONE;
            name = self.bump(input)?;
        } else if (first.kind() == Kind::Ident || first.character() == Some('*')) && input.clone().next().and_then(Component::character) == Some('|') {
            let mut look = input.clone();
            look.next();
            if !(attribute && look.next().and_then(Component::character) == Some('=')) {
                self.bump(input)?;
                ns = if first.character() == Some('*') { ANY } else { self.prefix(first.text().unwrap())? };
                name = self.bump(input)?;
            }
        }
        if name.kind() == Kind::Ident {
            Ok((ns, self.symbol(name.text().unwrap())?))
        } else if !attribute && name.character() == Some('*') {
            Ok((ns, ANY))
        } else {
            Err(Error::Invalid)
        }
    }
    fn compound(&mut self, input: &mut Components<'_>, relation: Relation, ctx: Context) -> Result<(Step, Specificity, bool)> {
        let mut tests = SmallVec::new();
        let mut specificity = Specificity::default();
        let mut pseudo = false;
        let mut explicit_type = false;
        if input.clone().next().is_some_and(|c| c.kind() == Kind::Ident || matches!(c.character(), Some('*' | '|'))) {
            let (ns, name) = self.qualified(input, false)?;
            self.test(&mut tests, Test::Type { ns, name })?;
            explicit_type = true;
            if name != ANY {
                specificity.types = 1;
            }
        }
        while let Some(c) = input.clone().next() {
            let spec = match c.kind() {
                Kind::IdHash if !pseudo => {
                    self.bump(input)?;
                    let id = self.symbol(c.text().unwrap())?;
                    self.test(&mut tests, Test::Id(id))?;
                    Specificity { ids: 1, ..Default::default() }
                }
                Kind::Delim if c.character() == Some('.') && !pseudo => {
                    self.bump(input)?;
                    let c = self.bump(input)?;
                    if c.kind() != Kind::Ident {
                        return Err(Error::Invalid);
                    }
                    let name = self.symbol(c.text().unwrap())?;
                    self.test(&mut tests, Test::Class(name))?;
                    Specificity { classes: 1, ..Default::default() }
                }
                Kind::Square if !pseudo => {
                    self.bump(input)?;
                    let t = self.attribute(c.children())?;
                    self.test(&mut tests, t)?;
                    Specificity { classes: 1, ..Default::default() }
                }
                Kind::Colon => {
                    self.bump(input)?;
                    let (t, s, is_pseudo) = self.pseudo(input, ctx, pseudo)?;
                    if pseudo && !matches!(t, Test::State(_)) {
                        return Err(Error::Unsupported("pseudo-element filters"));
                    }
                    pseudo |= is_pseudo;
                    self.test(&mut tests, t)?;
                    s
                }
                Kind::Delim if c.character() == Some('&') => return Err(Error::Unsupported("nesting selector")),
                _ => break,
            };
            specificity = specificity.add(spec);
        }
        if tests.is_empty() {
            return Err(Error::Invalid);
        }
        let implicit = !explicit_type && self.default_ns != ANY;
        let namespace = if implicit { self.default_ns } else { ANY };
        Ok((self.store_step(tests, relation, namespace, implicit)?, specificity, pseudo))
    }
    fn attribute(&mut self, mut input: Components<'_>) -> Result<Test> {
        self.skip(&mut input)?;
        let (ns, name) = self.qualified(&mut input, true)?;
        self.skip(&mut input)?;
        if input.clone().next().is_none() {
            return Ok(Test::Attr { ns, name, value: ANY, failure: ANY, op: AttrOp::Exists, case: Case::Default });
        }
        let first = self.bump(&mut input)?.character().ok_or(Error::Invalid)?;
        let op = match first {
            '=' => AttrOp::Equal,
            '~' => AttrOp::Includes,
            '|' => AttrOp::Dash,
            '^' => AttrOp::Prefix,
            '$' => AttrOp::Suffix,
            '*' => AttrOp::Contains,
            _ => return Err(Error::Invalid),
        };
        if first != '=' && self.bump(&mut input)?.character() != Some('=') {
            return Err(Error::Invalid);
        }
        self.skip(&mut input)?;
        let value = self.bump(&mut input)?;
        if !matches!(value.kind(), Kind::Ident | Kind::String | Kind::UnclosedString) {
            return Err(Error::Invalid);
        }
        let value = self.symbol(value.text().unwrap())?;
        self.skip(&mut input)?;
        let case = if let Some(c) = input.next() {
            self.tick(1)?;
            if c.kind() != Kind::Ident {
                return Err(Error::Invalid);
            }
            match c.text().unwrap() {
                s if s.eq_ignore_ascii_case("i") => Case::Ascii,
                s if s.eq_ignore_ascii_case("s") => Case::Sensitive,
                _ => return Err(Error::Invalid),
            }
        } else {
            Case::Default
        };
        self.skip(&mut input)?;
        if input.next().is_some() {
            return Err(Error::Invalid);
        }
        let failure = if matches!(op, AttrOp::Contains) && !matches!(case, Case::Sensitive) { self.failure(value)? } else { ANY };
        Ok(Test::Attr { ns, name, value, failure, op, case })
    }
    fn failure(&mut self, value: u32) -> Result<u32> {
        let text = self.out.symbols[value as usize].clone();
        let bytes = text.as_bytes();
        self.tick(bytes.len())?;
        if bytes.len() > self.limits.pattern_entries.saturating_sub(self.out.failures.len()) || self.out.failures.len() + bytes.len() >= NONE as usize {
            return Err(Error::CompileLimit);
        }
        let start = self.out.failures.len();
        let mut prefix = 0;
        for i in 0..bytes.len() {
            self.tick(1)?;
            if i > 0 {
                while prefix > 0 && !bytes[i].eq_ignore_ascii_case(&bytes[prefix]) {
                    prefix = self.out.failures[start + prefix - 1] as usize;
                    self.tick(1)?;
                }
                if bytes[i].eq_ignore_ascii_case(&bytes[prefix]) {
                    prefix += 1;
                }
            }
            self.out.failures.push(prefix as u32);
        }
        Ok(start as u32)
    }
    fn pseudo(&mut self, input: &mut Components<'_>, ctx: Context, already_pseudo: bool) -> Result<(Test, Specificity, bool)> {
        let colon = input.clone().next().is_some_and(|v| v.kind() == Kind::Colon);
        if colon {
            self.bump(input)?;
        }
        let c = self.bump(input)?;
        if !matches!(c.kind(), Kind::Ident | Kind::Function) {
            return Err(Error::Invalid);
        }
        let name = c.text().unwrap();
        self.tick(name.len())?;
        // Every recognized pseudo name is short. An unknown enormous
        // identifier must not allocate an equally enormous lowercase copy.
        if name.len() > 64 {
            return Err(Error::Invalid);
        }
        let legacy = c.kind() == Kind::Ident && ["before", "after", "first-line", "first-letter"].iter().any(|s| name.eq_ignore_ascii_case(s));
        if colon || legacy {
            if ctx.real {
                return Err(Error::Invalid);
            }
            if already_pseudo {
                return Err(Error::Unsupported("pseudo-element chains"));
            }
            if c.kind() == Kind::Function {
                return Err(
                    if ["part", "slotted", "cue", "highlight", "view-transition-group", "view-transition-image-pair", "view-transition-old", "view-transition-new"]
                        .iter()
                        .any(|s| name.eq_ignore_ascii_case(s))
                    {
                        Error::Unsupported("functional pseudo-elements")
                    } else {
                        Error::Invalid
                    },
                );
            }
            let p = match name.to_ascii_lowercase().as_str() {
                "before" => PseudoElement::Before,
                "after" => PseudoElement::After,
                "first-line" => PseudoElement::FirstLine,
                "first-letter" => PseudoElement::FirstLetter,
                "marker" => PseudoElement::Marker,
                "selection" => PseudoElement::Selection,
                "placeholder" => PseudoElement::Placeholder,
                "backdrop" => PseudoElement::Backdrop,
                "file-selector-button" => PseudoElement::FileSelectorButton,
                "spelling-error" => PseudoElement::SpellingError,
                "grammar-error" => PseudoElement::GrammarError,
                "target-text" => PseudoElement::TargetText,
                _ => return Err(Error::Invalid),
            };
            return Ok((Test::Pseudo(p), Specificity { types: 1, ..Default::default() }, true));
        }
        let base = Specificity { classes: 1, ..Default::default() };
        let lower = name.to_ascii_lowercase();
        if c.kind() == Kind::Ident {
            let structural = match lower.as_str() {
                "root" => Some(Structural::Root),
                "scope" => Some(Structural::Scope),
                "empty" => Some(Structural::Empty),
                "first-child" => Some(Structural::First),
                "last-child" => Some(Structural::Last),
                "only-child" => Some(Structural::Only),
                "first-of-type" => Some(Structural::FirstType),
                "last-of-type" => Some(Structural::LastType),
                "only-of-type" => Some(Structural::OnlyType),
                _ => None,
            };
            if let Some(s) = structural {
                return Ok((Test::Structural(s), base, false));
            }
            return state(&lower).map(|s| (Test::State(s), base, false)).ok_or(if matches!(lower.as_str(), "host" | "heading" | "blank" | "current" | "past" | "future") {
                Error::Unsupported("host or draft pseudo-class")
            } else {
                Error::Invalid
            });
        }
        let mut child = ctx;
        child.depth += 1;
        child.real = true;
        child.relative = false;
        match lower.as_str() {
            "is" | "where" | "not" | "has" => {
                let has = lower == "has";
                if has && ctx.in_has {
                    return Err(Error::Invalid);
                }
                child.in_has |= has;
                child.relative = has;
                child.ignore_subject_namespace = !has;
                let list = self.list(c.children(), child, matches!(lower.as_str(), "is" | "where"))?;
                let spec = if lower == "where" { Specificity::default() } else { self.out.lists[list as usize].specificity };
                let mode = if has {
                    Logical::Has
                } else if lower == "not" {
                    Logical::Not
                } else {
                    Logical::Is
                };
                Ok((Test::Logical { mode, list }, spec, false))
            }
            "nth-child" | "nth-last-child" | "nth-of-type" | "nth-last-of-type" => {
                let typed = lower.ends_with("of-type");
                let mut args = c.children();
                let start = args.clone().next().map(|v| v.at);
                let mut of = None;
                for v in args.by_ref() {
                    self.tick(1)?;
                    if v.kind() == Kind::Ident && v.text().unwrap().eq_ignore_ascii_case("of") {
                        of = Some(v.at);
                        break;
                    }
                }
                let formula = if let Some(of) = of {
                    if typed {
                        return Err(Error::Invalid);
                    }
                    AnB::parse(c.children().between(start.ok_or(Error::Invalid)?, of))?
                } else {
                    AnB::parse(c.children())?
                };
                let list = if of.is_some() {
                    child.ignore_subject_namespace = false;
                    self.list(args, child, false)?
                } else {
                    ANY
                };
                let spec = if list == ANY { base } else { base.add(self.out.lists[list as usize].specificity) };
                let id = self.out.nth.len() as u32;
                self.out.nth.push(Nth { formula, list, of_type: typed, last: lower.starts_with("nth-last") });
                Ok((Test::Nth(id), spec, false))
            }
            "lang" => {
                let mut args = c.children();
                let mut languages = SmallVec::<[u32; 4]>::new();
                loop {
                    self.skip(&mut args)?;
                    let value = self.bump(&mut args)?;
                    if !matches!(value.kind(), Kind::Ident | Kind::String | Kind::UnclosedString) {
                        return Err(Error::Invalid);
                    }
                    languages.push(self.symbol(value.text().unwrap())?);
                    self.skip(&mut args)?;
                    match args.next() {
                        None => break,
                        Some(v) if v.kind() == Kind::Comma => self.tick(1)?,
                        _ => return Err(Error::Invalid),
                    }
                }
                let start = self.out.languages.len() as u32;
                self.out.languages.extend(languages);
                Ok((Test::Lang { start, end: self.out.languages.len() as u32 }, base, false))
            }
            "dir" => {
                let mut args = c.children();
                self.skip(&mut args)?;
                let v = self.bump(&mut args)?;
                if v.kind() != Kind::Ident {
                    return Err(Error::Invalid);
                }
                let rtl = if v.text().unwrap().eq_ignore_ascii_case("rtl") {
                    true
                } else if v.text().unwrap().eq_ignore_ascii_case("ltr") {
                    false
                } else {
                    return Err(Error::Invalid);
                };
                self.skip(&mut args)?;
                if args.next().is_some() {
                    return Err(Error::Invalid);
                }
                Ok((Test::Dir(rtl), base, false))
            }
            "host" | "host-context" | "state" | "current" | "heading" => Err(Error::Unsupported("host or draft functional pseudo-class")),
            _ => Err(Error::Invalid),
        }
    }
}
fn state(name: &str) -> Option<State> {
    Some(match name {
        "any-link" => State::AnyLink,
        "link" => State::Link,
        "visited" => State::Visited,
        "local-link" => State::LocalLink,
        "target" => State::Target,
        "hover" => State::Hover,
        "active" => State::Active,
        "focus" => State::Focus,
        "focus-within" => State::FocusWithin,
        "focus-visible" => State::FocusVisible,
        "enabled" => State::Enabled,
        "disabled" => State::Disabled,
        "checked" => State::Checked,
        "unchecked" => State::Unchecked,
        "indeterminate" => State::Indeterminate,
        "default" => State::Default,
        "defined" => State::Defined,
        "required" => State::Required,
        "optional" => State::Optional,
        "valid" => State::Valid,
        "invalid" => State::Invalid,
        "user-valid" => State::UserValid,
        "user-invalid" => State::UserInvalid,
        "in-range" => State::InRange,
        "out-of-range" => State::OutOfRange,
        "read-only" => State::ReadOnly,
        "read-write" => State::ReadWrite,
        "placeholder-shown" => State::PlaceholderShown,
        "autofill" => State::Autofill,
        "open" => State::Open,
        "popover-open" => State::PopoverOpen,
        "modal" => State::Modal,
        "fullscreen" => State::Fullscreen,
        "picture-in-picture" => State::PictureInPicture,
        "playing" => State::Playing,
        "paused" => State::Paused,
        "seeking" => State::Seeking,
        "buffering" => State::Buffering,
        "stalled" => State::Stalled,
        "muted" => State::Muted,
        "volume-locked" => State::VolumeLocked,
        _ => return None,
    })
}
