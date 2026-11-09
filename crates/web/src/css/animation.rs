//! CSS Animations 1 at run time: `@keyframes` rules as their frames (each
//! property's value as written), an element's animations sampled at a
//! moment into declarations of the animation origin.
//!
//! Values are interpolated from their text, token by token against the
//! next frame's: numbers and lengths of one unit directly, mixed units
//! through `calc()`, colours in sRGB, functions (transforms, shadows,
//! colours) argument by argument; values whose tokens do not pair (or
//! that hold `var()`) switch at the midpoint, as discrete values do.
use super::{
    Component, Components, Kind, StatementKind, Syntax,
    properties::{
        Keyword, Property,
        animation::{Easing, Item},
    },
};
use foldhash::{HashMap, HashMapExt};
use std::{sync::Arc, sync::atomic::AtomicBool};

/// One keyframe: its offset (0 to 1), its easing, and its declarations
/// (property name, value text).
#[derive(Clone, Debug)]
pub struct Frame {
    pub offset: f64,
    pub easing: Option<Easing>,
    pub values: Vec<(Arc<str>, Arc<str>)>,
}
/// A `@keyframes` rule's frames, by offset.
#[derive(Clone, Debug, Default)]
pub struct Keyframes {
    pub frames: Vec<Frame>,
}
fn text_of(syntax: &Syntax, values: Components<'_>) -> String {
    let mut parts = values.filter(|c| c.kind() != Kind::Whitespace);
    let (Some(first), last) = (parts.next(), parts.last()) else { return String::new() };
    let end = last.unwrap_or(first).span().end as usize;
    syntax.source()[first.span().start as usize..end].to_owned()
}
/// A `@keyframes` (or `-webkit-keyframes`) rule's name and frames.
pub fn parse_keyframes(syntax: &Syntax, rule: &super::Statement<'_>, cancelled: Option<&AtomicBool>) -> Option<(Arc<str>, Keyframes)> {
    let name = rule.value().find(|c| matches!(c.kind(), Kind::Ident | Kind::String))?.text()?;
    let mut frames = Vec::new();
    for block in rule.block_contents(cancelled).ok()? {
        if block.kind() != StatementKind::QualifiedRule {
            continue;
        }
        let mut offsets = Vec::new();
        for c in block.value().filter(|c| !matches!(c.kind(), Kind::Whitespace | Kind::Comma)) {
            match c.kind() {
                Kind::Ident if c.text()?.eq_ignore_ascii_case("from") => offsets.push(0.),
                Kind::Ident if c.text()?.eq_ignore_ascii_case("to") => offsets.push(1.),
                Kind::Percentage | Kind::IntegerPercentage => offsets.push(c.number()?.value / 100.),
                _ => {}
            }
        }
        let mut values = Vec::new();
        let mut easing = None;
        for d in block.block_contents(cancelled).ok()? {
            if d.kind() != StatementKind::Declaration {
                continue;
            }
            let Some(name) = d.name() else { continue };
            let name = name.to_ascii_lowercase();
            let value = text_of(syntax, d.value());
            if name == "animation-timing-function" {
                let parts: Vec<Component<'_>> = d.value().filter(|c| c.kind() != Kind::Whitespace).collect();
                if let Ok(super::properties::Value::Animation(list)) = super::properties::animation::parse_list(super::properties::animation::Field::Easing, &parts)
                    && let Some(Item::Easing(e)) = list.at(0)
                {
                    easing = Some(*e);
                }
                continue;
            }
            values.push((Arc::<str>::from(name), Arc::<str>::from(value)));
        }
        for offset in offsets.into_iter().filter(|o| (0. ..=1.).contains(o)) {
            frames.push(Frame { offset, easing, values: values.clone() });
        }
    }
    frames.sort_by(|a, b| a.offset.total_cmp(&b.offset));
    Some((Arc::from(name), Keyframes { frames }))
}
/// One animation of an element, as its longhands' i-th entries give it.
#[derive(Clone, Debug, PartialEq)]
pub struct Spec {
    pub name: Arc<str>,
    /// Seconds.
    pub duration: f64,
    pub delay: f64,
    /// Iterations; infinite as `f64::INFINITY`.
    pub count: f64,
    pub direction: Keyword,
    pub fill: Keyword,
    pub paused: bool,
    pub easing: Easing,
}
/// Where an animation is at `elapsed` seconds since it started: its
/// iteration progress (before easing) and whether it applies at all.
pub fn progress(spec: &Spec, elapsed: f64) -> Option<(f64, f64)> {
    let local = elapsed - spec.delay;
    let backwards = matches!(spec.fill, Keyword::Backwards | Keyword::Both);
    let forwards = matches!(spec.fill, Keyword::Forwards | Keyword::Both);
    let active = spec.duration * spec.count;
    let (iteration, p) = if local < 0. {
        if !backwards {
            return None;
        }
        (0., 0.)
    } else if spec.duration <= 0. || local >= active {
        if !forwards {
            return None;
        }
        // The end: the last iteration's end (a fractional count ends inside).
        let count = if spec.count.is_finite() { spec.count } else { 1. };
        let last = (count.ceil() - 1.).max(0.);
        let p = count - last;
        (last, if p == 0. { 1. } else { p })
    } else {
        let i = (local / spec.duration).floor();
        (i, local / spec.duration - i)
    };
    let reversed = match spec.direction {
        Keyword::Reverse => true,
        Keyword::Alternate => iteration % 2. == 1.,
        Keyword::AlternateReverse => iteration % 2. == 0.,
        _ => false,
    };
    Some((iteration, if reversed { 1. - p } else { p }))
}
/// Whether an animation still changes after `elapsed` seconds (else it holds).
pub fn running(spec: &Spec, elapsed: f64) -> bool {
    !spec.paused && spec.duration > 0. && (spec.count.is_infinite() || elapsed < spec.delay + spec.duration * spec.count)
}
/// The declarations (property, value text) an animation applies at
/// iteration progress `p`: each property between the frames around `p`
/// (an end with no frame for it holds the nearest one's value).
pub fn sample(keyframes: &Keyframes, p: f64, easing: Easing) -> Vec<(Arc<str>, String)> {
    sample_over(keyframes, p, easing, &|_| None)
}
/// `sample`, a property without a `from` or `to` frame taking the
/// element's own value there (CSS Animations 1 §3: the missing keyframe is
/// made from the underlying value) when `underlying` knows it: `to {
/// transform: rotate(360deg) }` turns from `none` (it held its one frame,
/// and every spinner stood still).
pub fn sample_over(keyframes: &Keyframes, p: f64, easing: Easing, underlying: &dyn Fn(&str) -> Option<Arc<str>>) -> Vec<(Arc<str>, String)> {
    let mut properties: Vec<&Arc<str>> = Vec::new();
    for f in &keyframes.frames {
        for (name, _) in &f.values {
            if !properties.contains(&name) {
                properties.push(name);
            }
        }
    }
    let mut out = Vec::new();
    for name in properties {
        let mut frames: Vec<(&Frame, &Arc<str>)> = keyframes.frames.iter().filter_map(|f| f.values.iter().find(|(n, _)| n == name).map(|(_, v)| (f, v))).collect();
        let Some(&(first, first_value)) = frames.first() else { continue };
        // The missing ends, from the element's own value.
        let own = underlying(name);
        let start = Frame { offset: 0., easing: None, values: Vec::new() };
        let end = Frame { offset: 1., easing: None, values: Vec::new() };
        if let Some(own) = &own {
            if first.offset > 0. {
                frames.insert(0, (&start, own));
            }
            if frames.last().is_some_and(|(f, _)| f.offset < 1.) {
                frames.push((&end, own));
            }
        }
        let before = frames.iter().rev().find(|(f, _)| f.offset <= p).copied();
        let after = frames.iter().find(|(f, _)| f.offset > p).copied();
        let value = match (before, after) {
            (Some((a, va)), Some((b, vb))) => {
                let span = b.offset - a.offset;
                let local = if span > 0. { (p - a.offset) / span } else { 0. };
                interpolate(va, vb, a.easing.unwrap_or(easing).at(local))
            }
            (Some((_, v)), None) => v.to_string(),
            (None, Some(_)) => first_value.to_string(),
            (None, None) => {
                let _ = first;
                continue;
            }
        };
        out.push((name.clone(), value));
    }
    out
}
fn lerp(a: f64, b: f64, p: f64) -> f64 {
    a + (b - a) * p
}
fn fmt(v: f64) -> String {
    let r = (v * 10000.).round() / 10000.;
    if r == r.trunc() { format!("{}", r as i64) } else { format!("{r}") }
}
const COLOR_FUNCTIONS: &[&str] = &["rgb", "rgba", "hsl", "hsla", "hwb", "lab", "lch", "oklab", "oklch", "color"];
struct NoContext;
impl super::color::Context for NoContext {
    fn scheme(&self) -> super::color::Scheme {
        super::color::Scheme::Light
    }
}
/// A colour token as sRGB 0-255 and alpha 0-1, when it is one.
fn color_of(syntax: &Syntax, c: Component<'_>) -> Option<[f64; 4]> {
    let plausible = match c.kind() {
        Kind::Hash | Kind::IdHash => true,
        Kind::Function => COLOR_FUNCTIONS.iter().any(|f| c.text().is_some_and(|t| t.eq_ignore_ascii_case(f))),
        Kind::Ident => c.text().is_some_and(|t| !t.eq_ignore_ascii_case("none") && !t.eq_ignore_ascii_case("auto")),
        _ => false,
    };
    if !plausible {
        return None;
    }
    let span = c.span();
    let text: Arc<str> = Arc::from(&syntax.source()[span.start as usize..span.end as usize]);
    let one = Syntax::parse(text, Default::default(), None).ok()?;
    let value = super::color::Value::parse(one.components(), Default::default(), None).ok()?;
    let color = value.resolve(&NoContext, Default::default(), None).ok()?;
    let [r, g, b, a] = color.srgb8_clipped();
    Some([f64::from(r), f64::from(g), f64::from(b), f64::from(a) / 255.])
}
fn token_text<'a>(syntax: &'a Syntax, c: Component<'_>) -> &'a str {
    let span = c.span();
    &syntax.source()[span.start as usize..span.end as usize]
}
/// Two component lists interpolated, or None when they do not pair.
fn pair(sa: &Syntax, a: Components<'_>, sb: &Syntax, b: Components<'_>, p: f64) -> Option<String> {
    let a: Vec<Component<'_>> = a.filter(|c| c.kind() != Kind::Whitespace).collect();
    let b: Vec<Component<'_>> = b.filter(|c| c.kind() != Kind::Whitespace).collect();
    if a.len() != b.len() {
        return None;
    }
    let mut out = String::new();
    for (x, y) in a.iter().zip(&b) {
        let piece = one(sa, *x, sb, *y, p)?;
        if !out.is_empty() && x.kind() != Kind::Comma && !out.ends_with('(') {
            out.push(' ');
        }
        out.push_str(&piece);
        if x.kind() == Kind::Comma {
            out.push(' ');
        }
    }
    Some(out.replace(" ,", ","))
}
fn one(sa: &Syntax, x: Component<'_>, sb: &Syntax, y: Component<'_>, p: f64) -> Option<String> {
    let num = |c: Component<'_>| c.number().map(|n| n.value);
    let is_num = |c: Component<'_>| matches!(c.kind(), Kind::Number | Kind::Integer);
    let is_dim = |c: Component<'_>| matches!(c.kind(), Kind::Dimension | Kind::IntegerDimension);
    let is_pct = |c: Component<'_>| matches!(c.kind(), Kind::Percentage | Kind::IntegerPercentage);
    if (x.kind() == Kind::Function && x.text().is_some_and(|t| t.eq_ignore_ascii_case("var"))) || (y.kind() == Kind::Function && y.text().is_some_and(|t| t.eq_ignore_ascii_case("var"))) {
        return None;
    }
    if is_num(x) && is_num(y) {
        return Some(fmt(lerp(num(x)?, num(y)?, p)));
    }
    if is_pct(x) && is_pct(y) {
        return Some(format!("{}%", fmt(lerp(num(x)?, num(y)?, p))));
    }
    let length = |c: Component<'_>| is_dim(c) || is_pct(c) || is_num(c) && num(c) == Some(0.);
    if length(x) && length(y) && (is_dim(x) || is_dim(y) || is_pct(x) || is_pct(y)) {
        if is_dim(x) && is_dim(y) && x.unit()?.eq_ignore_ascii_case(y.unit()?) {
            return Some(format!("{}{}", fmt(lerp(num(x)?, num(y)?, p)), x.unit()?));
        }
        let (tx, ty) = (token_text(sa, x), token_text(sb, y));
        let zero = |t: &str, other: Component<'_>| if t == "0" { other.unit().map_or("0%".to_owned(), |u| format!("0{u}")) } else { t.to_owned() };
        return Some(format!("calc({} * {} + {} * {})", zero(tx, y), fmt(1. - p), zero(ty, x), fmt(p)));
    }
    if let (Some(ca), Some(cb)) = (color_of(sa, x), color_of(sb, y)) {
        let c: Vec<f64> = (0..4).map(|i| lerp(ca[i], cb[i], p)).collect();
        return Some(format!("rgba({}, {}, {}, {})", fmt(c[0]), fmt(c[1]), fmt(c[2]), fmt(c[3])));
    }
    match (x.kind(), y.kind()) {
        (Kind::Function, Kind::Function) if x.text()?.eq_ignore_ascii_case(y.text()?) => Some(format!("{}({})", x.text()?, pair(sa, x.children(), sb, y.children(), p)?)),
        (Kind::Comma, Kind::Comma) => Some(",".to_owned()),
        _ if token_text(sa, x).eq_ignore_ascii_case(token_text(sb, y)) => Some(token_text(sa, x).to_owned()),
        _ => None,
    }
}
/// The identity of a transform list's functions, one for one (`none`
/// against a list interpolates as these: CSS Transforms 1 §11), or none
/// when one has no identity here (or it isn't a list of functions).
fn identity_transforms(list: &str) -> Option<String> {
    let syntax = Syntax::parse(Arc::from(list), Default::default(), None).ok()?;
    let mut out = Vec::new();
    for c in syntax.components().filter(|c| c.kind() != Kind::Whitespace) {
        if c.kind() != Kind::Function {
            return None;
        }
        let name = c.text()?.to_ascii_lowercase();
        let args = c.children().filter(|a| a.kind() == Kind::Comma).count() + 1;
        let repeat = |v: &str, n: usize| vec![v; n].join(", ");
        out.push(match name.as_str() {
            "translate" | "translatex" | "translatey" | "translatez" | "translate3d" => format!("{name}({})", repeat("0px", args)),
            "scale" | "scalex" | "scaley" | "scalez" | "scale3d" => format!("{name}({})", repeat("1", args)),
            "rotate" | "rotatex" | "rotatey" | "rotatez" | "skewx" | "skewy" => format!("{name}(0deg)"),
            "skew" => format!("skew({})", repeat("0deg", args)),
            "matrix" => "matrix(1, 0, 0, 1, 0, 0)".to_owned(),
            _ => return None,
        });
    }
    (!out.is_empty()).then(|| out.join(" "))
}
/// `a` and `b` (value texts) at `p` between them.
pub fn interpolate(a: &str, b: &str, p: f64) -> String {
    if p <= 0. {
        return a.to_owned();
    }
    if p >= 1. {
        return b.to_owned();
    }
    // `none` and a transform list: the list's identity against it.
    if a.trim().eq_ignore_ascii_case("none")
        && let Some(identity) = identity_transforms(b)
    {
        return interpolate(&identity, b, p);
    }
    if b.trim().eq_ignore_ascii_case("none")
        && let Some(identity) = identity_transforms(a)
    {
        return interpolate(a, &identity, p);
    }
    let (Ok(sa), Ok(sb)) = (Syntax::parse(Arc::from(a), Default::default(), None), Syntax::parse(Arc::from(b), Default::default(), None)) else {
        return if p < 0.5 { a } else { b }.to_owned();
    };
    pair(&sa, sa.components(), &sb, sb.components(), p).unwrap_or_else(|| if p < 0.5 { a } else { b }.to_owned())
}
/// An element's animations, from its computed `animation-*` lists.
pub fn specs(style: &super::style::Style) -> Vec<Spec> {
    use super::style::Value;
    let list = |p: Property| match style.value(p) {
        Value::Animation(a) => Some(a.clone()),
        _ => None,
    };
    let Some(names) = list(Property::AnimationName) else { return Vec::new() };
    let (durations, delays, counts, directions, fills, states, easings) = (
        list(Property::AnimationDuration),
        list(Property::AnimationDelay),
        list(Property::AnimationIterationCount),
        list(Property::AnimationDirection),
        list(Property::AnimationFillMode),
        list(Property::AnimationPlayState),
        list(Property::AnimationTimingFunction),
    );
    let mut out = Vec::new();
    for (i, name) in names.items().iter().enumerate() {
        let Item::Name(name) = name else { continue };
        let time = |l: &Option<super::properties::animation::Animations>| match l.as_ref().and_then(|l| l.at(i)) {
            Some(Item::Time(t)) => *t,
            _ => 0.,
        };
        let word = |l: &Option<super::properties::animation::Animations>, default: Keyword| match l.as_ref().and_then(|l| l.at(i)) {
            Some(Item::Keyword(k)) => *k,
            _ => default,
        };
        out.push(Spec {
            name: name.clone(),
            duration: time(&durations),
            delay: time(&delays),
            count: match counts.as_ref().and_then(|l| l.at(i)) {
                Some(Item::Count(n)) => *n,
                Some(Item::Infinite) => f64::INFINITY,
                _ => 1.,
            },
            direction: word(&directions, Keyword::Normal),
            fill: word(&fills, Keyword::None),
            paused: word(&states, Keyword::Running) == Keyword::Paused,
            easing: match easings.as_ref().and_then(|l| l.at(i)) {
                Some(Item::Easing(e)) => *e,
                _ => Easing::EASE,
            },
        });
    }
    out
}
/// The keyframes by name, the last rule of a name winning.
pub type Library = HashMap<Arc<str>, Arc<Keyframes>>;
/// What an element's `transition-*` lists say of one property: its
/// duration and delay (seconds) and easing, when it transitions at all.
pub fn transition_of(style: &super::style::Style, property: Property) -> Option<(f64, f64, Easing)> {
    use super::style::Value;
    let list = |p: Property| match style.value(p) {
        Value::Animation(a) => Some(a.clone()),
        _ => None,
    };
    let properties = list(Property::TransitionProperty)?;
    let durations = list(Property::TransitionDuration)?;
    // Nothing lasts: no transition (the common case, read first).
    if durations.items().iter().all(|d| matches!(d, Item::Time(t) if *t <= 0.)) {
        return None;
    }
    let (delays, easings) = (list(Property::TransitionDelay), list(Property::TransitionTimingFunction));
    let name = property.as_str();
    // The last entry naming it (or `all`) wins.
    let i = properties.items().iter().rposition(|p| matches!(p, Item::Name(n) if &**n == "all" || &**n == name))?;
    let duration = match durations.at(i) {
        Some(Item::Time(t)) => *t,
        _ => 0.,
    };
    let delay = match delays.as_ref().and_then(|l| l.at(i)) {
        Some(Item::Time(t)) => *t,
        _ => 0.,
    };
    let easing = match easings.as_ref().and_then(|l| l.at(i)) {
        Some(Item::Easing(e)) => *e,
        _ => Easing::EASE,
    };
    (duration > 0.).then_some((duration, delay, easing))
}
/// Whether two value texts interpolate (else a change is discrete and
/// doesn't transition).
pub fn interpolable(a: &str, b: &str) -> bool {
    let (Ok(sa), Ok(sb)) = (Syntax::parse(Arc::from(a), Default::default(), None), Syntax::parse(Arc::from(b), Default::default(), None)) else { return false };
    pair(&sa, sa.components(), &sb, sb.components(), 0.5).is_some_and(|mid| mid != a && mid != b)
}
/// One running transition (CSS Transitions 1 §3): a property going from
/// the value it showed when it changed to its new one.
#[derive(Clone, Debug)]
struct Running {
    from: Arc<str>,
    to: Arc<str>,
    start_ms: f64,
    duration: f64,
    delay: f64,
    easing: Easing,
    started: bool,
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn values_interpolate_by_their_tokens() {
        assert_eq!(interpolate("0", "1", 0.25), "0.25");
        assert_eq!(interpolate("10px", "20px", 0.5), "15px");
        assert_eq!(interpolate("translateY(24px) scale(1)", "translateY(0) scale(2)", 0.5), "translateY(calc(24px * 0.5 + 0px * 0.5)) scale(1.5)");
        assert_eq!(interpolate("rgb(0, 113, 227)", "#f5f5f7", 1. / 3.), "rgba(81.6667, 157, 233.6667, 1)");
        assert_eq!(interpolate("block", "none", 0.4), "block");
        assert_eq!(interpolate("var(--a)", "var(--b)", 0.6), "var(--b)");
        // `none` against a transform list: the list's identity.
        assert_eq!(interpolate("none", "rotate(360deg)", 0.25), "rotate(90deg)");
        assert_eq!(interpolate("translate(10px, 20px) scale(2)", "none", 0.5).split_whitespace().collect::<Vec<_>>().join(" "), "translate(5px, 10px) scale(1.5)");
        // A spinner's one frame (`to`) turns from the element's own value.
        let spin = Keyframes { frames: vec![Frame { offset: 1., easing: None, values: vec![(Arc::from("transform"), Arc::from("rotate(360deg)"))] }] };
        let own = |_: &str| Some(Arc::from("none"));
        assert_eq!(sample_over(&spin, 0.5, Easing::Linear, &own), vec![(Arc::from("transform"), "rotate(180deg)".to_owned())]);
        assert_eq!(sample(&spin, 0.5, Easing::Linear), vec![(Arc::from("transform"), "rotate(360deg)".to_owned())], "without its own value: held");
    }
    #[test]
    fn progress_follows_delay_count_direction_and_fill() {
        let spec = Spec { name: "x".into(), duration: 1., delay: 0.5, count: 2., direction: Keyword::Alternate, fill: Keyword::Forwards, paused: false, easing: Easing::Linear };
        assert_eq!(progress(&spec, 0.25), None);
        assert_eq!(progress(&spec, 0.75), Some((0., 0.25)));
        assert_eq!(progress(&spec, 1.75), Some((1., 0.75)));
        assert_eq!(progress(&spec, 9.), Some((1., 0.)));
        assert!(!running(&spec, 9.));
        let none = Spec { fill: Keyword::None, ..spec };
        assert_eq!(progress(&none, 9.), None);
    }
}
/// A page's CSS animations over time: when each element's animation
/// started (its first frame), the moment rendered, and what was applied.
#[derive(Clone, Debug, Default)]
pub struct Timeline {
    /// The page's clock, ms (the live page's frame time).
    pub now_ms: f64,
    starts: HashMap<(crate::dom::NodeId, Arc<str>, usize), f64>,
    /// The elements given animated values at the last sample.
    applied: Vec<crate::dom::NodeId>,
    /// Some animation still changes: the page wants its next frame.
    pub running: bool,
    /// Which events each animation has had: 1 started, 2 ended.
    phases: HashMap<(crate::dom::NodeId, Arc<str>, usize), u8>,
    /// Events since the last `take_events`: element, type, name, elapsed s.
    events: Vec<(crate::dom::NodeId, &'static str, Arc<str>, f64)>,
    /// The running transitions, by element and property.
    transitions: HashMap<(crate::dom::NodeId, Property), Running>,
    /// The elements given transitioned values at the last sample.
    transitioned: Vec<crate::dom::NodeId>,
    /// Scripts' animations compiled (keyframes and timing), by id, with the
    /// frames they were compiled from.
    scripted: HashMap<u64, (Arc<[crate::dom::ScriptFrame]>, Arc<Keyframes>, Spec)>,
    /// The elements scripts' animations gave values at the last sample.
    script_applied: Vec<crate::dom::NodeId>,
    /// Each animation's properties' values on its element before it ran
    /// (for the keyframes its rule leaves out), by animation.
    underlying: HashMap<(crate::dom::NodeId, Arc<str>, usize), Vec<(Arc<str>, Arc<str>)>>,
}
/// A timing function as CSS spells one (`ease-in-out`, `cubic-bezier(...)`,
/// `steps(4, end)`), or none when it isn't one.
fn easing_of(text: &str) -> Option<Easing> {
    let source: Arc<str> = Arc::from(format!("animation-timing-function:{text}"));
    let syntax = Syntax::parse(source, Default::default(), None).ok()?;
    let d = syntax.one_declaration(None).ok()?;
    let parts: Vec<Component<'_>> = d.value().filter(|c| c.kind() != Kind::Whitespace).collect();
    match super::properties::animation::parse_list(super::properties::animation::Field::Easing, &parts) {
        Ok(super::properties::Value::Animation(list)) => match list.at(0) {
            Some(Item::Easing(e)) => Some(*e),
            _ => None,
        },
        _ => None,
    }
}
/// A script's animation as keyframes and a timing the CSS machinery samples.
fn compile_script(a: &crate::dom::ScriptAnimation) -> (Arc<Keyframes>, Spec) {
    let frames = a.frames.iter().map(|f| Frame { offset: f.offset.clamp(0., 1.), easing: f.easing.as_deref().and_then(easing_of), values: f.values.clone() }).collect();
    let keyword = |text: &str, choices: &[(&str, Keyword)], default: Keyword| choices.iter().find(|(n, _)| *n == text).map_or(default, |(_, k)| *k);
    let spec = Spec {
        name: Arc::from(""),
        duration: (a.duration / 1000.).max(0.),
        delay: a.delay / 1000.,
        count: if a.iterations.is_nan() { 1. } else { a.iterations.max(0.) },
        direction: keyword(&a.direction, &[("reverse", Keyword::Reverse), ("alternate", Keyword::Alternate), ("alternate-reverse", Keyword::AlternateReverse)], Keyword::Normal),
        fill: keyword(&a.fill, &[("forwards", Keyword::Forwards), ("backwards", Keyword::Backwards), ("both", Keyword::Both)], Keyword::None),
        paused: a.held_ms.is_some(),
        easing: easing_of(&a.easing).unwrap_or(Easing::Linear),
    };
    (Arc::new(Keyframes { frames }), spec)
}
impl Timeline {
    /// The animation events since the last call (animationstart,
    /// animationend), for the page's scripts.
    pub fn take_events(&mut self) -> Vec<(crate::dom::NodeId, &'static str, Arc<str>, f64)> {
        std::mem::take(&mut self.events)
    }
}
impl Timeline {
    /// The property changed from `from` (the value it showed) to `to`: a
    /// transition starts, unless one already runs to `to`; one running to
    /// another value is replaced (cancelled), and a change back to where it
    /// shows needs none.
    #[allow(clippy::too_many_arguments)]
    pub fn transition(&mut self, node: crate::dom::NodeId, property: Property, from: String, to: String, duration: f64, delay: f64, easing: Easing) {
        let key = (node, property);
        if let Some(r) = self.transitions.get(&key) {
            if *r.to == *to {
                return;
            }
            self.events.push((node, "transitioncancel", Arc::from(property.as_str()), ((self.now_ms - r.start_ms) / 1000. - r.delay).max(0.)));
            self.transitions.remove(&key);
        }
        if from == to || !interpolable(&from, &to) {
            return;
        }
        let name: Arc<str> = Arc::from(property.as_str());
        if crate::env_on!("OLO_WEB_TIMING") {
            eprintln!("  transition {node:?} {name}: {from} -> {to} over {duration} s after {delay} s at {} ms", self.now_ms);
        }
        self.events.push((node, "transitionrun", name.clone(), 0.));
        if delay <= 0. {
            self.events.push((node, "transitionstart", name, 0.));
        }
        self.transitions.insert(key, Running { from: Arc::from(from), to: Arc::from(to), start_ms: self.now_ms, duration, delay, easing, started: delay <= 0. });
        self.running = true;
    }
    /// How many transitions run.
    pub fn transition_count(&self) -> usize {
        self.transitions.len()
    }
    /// Whether this property of this element is transitioning (its value
    /// now is the transition's, not its own).
    pub fn transitioning(&self, node: crate::dom::NodeId, property: Property) -> bool {
        self.transitions.contains_key(&(node, property))
    }
    /// Where a running transition goes.
    pub fn transition_end(&self, node: crate::dom::NodeId, property: Property) -> Option<&str> {
        self.transitions.get(&(node, property)).map(|r| &*r.to)
    }
    /// The transitioned declarations at `now_ms`, by element, and every
    /// element whose values must be computed again (transitioned now or at
    /// the last sample); a transition done is dropped (its element takes
    /// its own value again), with its `transitionend`.
    pub fn sample_transitions(&mut self, cancelled: Option<&AtomicBool>) -> (crate::dom::NodeMap<Vec<super::properties::Declaration>>, Vec<crate::dom::NodeId>) {
        let mut out: crate::dom::NodeMap<Vec<super::properties::Declaration>> = Default::default();
        let now = self.now_ms;
        let mut done = Vec::new();
        for (&(node, property), r) in self.transitions.iter_mut() {
            let elapsed = ((now - r.start_ms) / 1000.).max(0.);
            let local = elapsed - r.delay;
            if local >= r.duration {
                done.push((node, property, r.duration));
                continue;
            }
            if local >= 0. && !r.started {
                r.started = true;
                self.events.push((node, "transitionstart", Arc::from(property.as_str()), 0.));
            }
            let value = if local <= 0. { r.from.to_string() } else { interpolate(&r.from, &r.to, r.easing.at(local / r.duration)) };
            let text: Arc<str> = Arc::from(format!("{}:{value}", property.as_str()));
            let Ok(syntax) = Syntax::parse(text, Default::default(), cancelled) else { continue };
            let Ok(d) = syntax.one_declaration(cancelled) else { continue };
            if let Ok(declaration) = super::properties::Declaration::compile(&d, Default::default(), cancelled) {
                out.entry(node).or_default().push(declaration);
            }
        }
        for (node, property, duration) in done {
            self.transitions.remove(&(node, property));
            self.events.push((node, "transitionend", Arc::from(property.as_str()), duration));
        }
        self.running |= !self.transitions.is_empty();
        let mut touched: Vec<crate::dom::NodeId> = out.keys().copied().collect();
        for n in std::mem::take(&mut self.transitioned) {
            if !out.contains_key(&n) {
                touched.push(n);
            }
        }
        self.transitioned = out.keys().copied().collect();
        (out, touched)
    }
    /// The declarations scripts' animations (`element.animate()`) apply at
    /// `now_ms`, by element, sampled as CSS animations are, and every
    /// element whose values must be computed again (animated now or at the
    /// last sample). Each animation's time is its own: its start on the
    /// page's clock and its rate, or the time it is paused at.
    pub fn sample_script(&mut self, animations: &[crate::dom::ScriptAnimation], cancelled: Option<&AtomicBool>) -> (crate::dom::NodeMap<Vec<super::properties::Declaration>>, Vec<crate::dom::NodeId>) {
        let mut out: crate::dom::NodeMap<Vec<super::properties::Declaration>> = Default::default();
        self.scripted.retain(|id, _| animations.iter().any(|a| a.id == *id));
        for a in animations {
            let compiled = match self.scripted.get(&a.id) {
                Some((frames, keyframes, spec)) if Arc::ptr_eq(frames, &a.frames) => (keyframes.clone(), spec.clone()),
                _ => {
                    let (keyframes, spec) = compile_script(a);
                    self.scripted.insert(a.id, (a.frames.clone(), keyframes.clone(), spec.clone()));
                    (keyframes, spec)
                }
            };
            let (keyframes, mut spec) = compiled;
            // Timing that changed with the frames the same (paused, a new
            // rate) is read anew each sample.
            spec.paused = a.held_ms.is_some();
            let current_ms = a.held_ms.unwrap_or((self.now_ms - a.start_ms) * a.rate);
            let elapsed = current_ms / 1000.;
            if !spec.paused && spec.duration > 0. && (spec.count.is_infinite() || if a.rate >= 0. { elapsed < spec.delay + spec.duration * spec.count } else { elapsed > 0. }) {
                self.running = true;
            }
            let Some((_, p)) = progress(&spec, elapsed) else { continue };
            for (name, value) in sample(&keyframes, p, spec.easing) {
                let text: Arc<str> = Arc::from(format!("{name}:{value}"));
                let Ok(syntax) = Syntax::parse(text, Default::default(), cancelled) else { continue };
                let Ok(d) = syntax.one_declaration(cancelled) else { continue };
                if let Ok(declaration) = super::properties::Declaration::compile(&d, Default::default(), cancelled) {
                    out.entry(a.node).or_default().push(declaration);
                }
            }
        }
        let mut touched: Vec<crate::dom::NodeId> = out.keys().copied().collect();
        for n in std::mem::take(&mut self.script_applied) {
            if !out.contains_key(&n) {
                touched.push(n);
            }
        }
        self.script_applied = out.keys().copied().collect();
        (out, touched)
    }
    /// Whether scripts' animations applied values at the last sample.
    pub fn script_applied(&self) -> bool {
        !self.script_applied.is_empty()
    }
    /// The animated declarations for the elements `styled` lists (each with
    /// its style), at `now_ms`, and every element whose animated values
    /// must be computed again (those animated now or at the last sample).
    pub fn sample<'s>(
        &mut self,
        library: &Library,
        styled: impl Iterator<Item = (crate::dom::NodeId, &'s super::style::Style)>,
        cancelled: Option<&AtomicBool>,
    ) -> (crate::dom::NodeMap<Vec<super::properties::Declaration>>, Vec<crate::dom::NodeId>) {
        let mut out: crate::dom::NodeMap<Vec<super::properties::Declaration>> = Default::default();
        self.running = false;
        if library.is_empty() {
            let touched = std::mem::take(&mut self.applied);
            self.starts.clear();
            return (out, touched);
        }
        let mut seen = std::collections::HashSet::new();
        for (node, style) in styled {
            for (i, spec) in specs(style).into_iter().enumerate() {
                let Some(frames) = library.get(&spec.name) else { continue };
                let key = (node, spec.name.clone(), i);
                // Its element's values before it ran, for the frames its
                // rule leaves out: the initial value's where the element has
                // it (its style is not yet animated at its first frame).
                if !self.starts.contains_key(&key) {
                    let mut own = Vec::new();
                    for f in &frames.frames {
                        for (name, _) in &f.values {
                            if own.iter().any(|(n, _): &(Arc<str>, Arc<str>)| n == name) {
                                continue;
                            }
                            if let Some(super::properties::Name::Longhand(p)) = super::properties::Name::parse(name)
                                && style.is_initial(p)
                            {
                                own.push((name.clone(), Arc::from(p.initial_spelling())));
                            }
                        }
                    }
                    self.underlying.insert(key.clone(), own);
                }
                let start = *self.starts.entry(key.clone()).or_insert(self.now_ms);
                seen.insert(key.clone());
                let elapsed = (self.now_ms - start).max(0.) / 1000.;
                // One that shows nothing (its element hidden, unless it is
                // what it animates) asks for no frames: its clock goes on,
                // and it shows where it is once its element does (apple.com's
                // hidden bag spinner rendered the page 120 times a second).
                let shows = !matches!(style.value(Property::Visibility), super::style::Value::Keyword(Keyword::Hidden | Keyword::Collapse))
                    || frames.frames.iter().any(|f| f.values.iter().any(|(n, _)| &**n == "visibility"));
                self.running |= shows && running(&spec, elapsed);
                if shows && running(&spec, elapsed) && crate::env_on!("OLO_RENDER_TRACE") {
                    eprintln!("  animation runs: {node:?} {} ({} s x {})", spec.name, spec.duration, spec.count);
                }
                // Its events: start once the delay is over, end once it is done.
                let phase = self.phases.entry(key.clone()).or_insert(0);
                if *phase < 1 && elapsed >= spec.delay {
                    *phase = 1;
                    self.events.push((node, "animationstart", spec.name.clone(), (-spec.delay).max(0.)));
                }
                let active = spec.duration * spec.count;
                if *phase < 2 && active.is_finite() && elapsed >= spec.delay + active {
                    if *phase < 1 {
                        self.events.push((node, "animationstart", spec.name.clone(), 0.));
                    }
                    *phase = 2;
                    self.events.push((node, "animationend", spec.name.clone(), active));
                }
                let Some((_, p)) = progress(&spec, elapsed) else { continue };
                let own = self.underlying.get(&key);
                let underlying = |name: &str| own.and_then(|o| o.iter().find(|(n, _)| &**n == name)).map(|(_, v)| v.clone());
                for (name, value) in sample_over(frames, p, spec.easing, &underlying) {
                    let text: Arc<str> = Arc::from(format!("{name}:{value}"));
                    let Ok(syntax) = Syntax::parse(text, Default::default(), cancelled) else { continue };
                    let Ok(d) = syntax.one_declaration(cancelled) else { continue };
                    if let Ok(declaration) = super::properties::Declaration::compile(&d, Default::default(), cancelled) {
                        out.entry(node).or_default().push(declaration);
                    }
                }
            }
        }
        self.starts.retain(|k, _| seen.contains(k));
        self.underlying.retain(|k, _| seen.contains(k));
        self.phases.retain(|k, _| seen.contains(k));
        let mut touched: Vec<crate::dom::NodeId> = out.keys().copied().collect();
        for n in std::mem::take(&mut self.applied) {
            if !out.contains_key(&n) {
                touched.push(n);
            }
        }
        self.applied = out.keys().copied().collect();
        (out, touched)
    }
}
