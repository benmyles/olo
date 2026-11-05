use super::*;
use crate::css::{Component, Kind};
use smallvec::SmallVec;
#[derive(Clone, Copy)]
enum Part {
    Atom(Atom),
    Node(u32),
}
struct Parser<'a> {
    nodes: Vec<Node>,
    limits: Limits,
    cancelled: Option<&'a AtomicBool>,
    work: usize,
    instructions: usize,
    calculations: SmallVec<[Option<math::Value>; 4]>,
    functions: SmallVec<[Functional; 1]>,
    dependencies: Dependencies,
}
pub(super) fn parse(mut values: Components<'_>, limits: Limits, cancelled: Option<&AtomicBool>) -> Result<Value> {
    let mut parser = Parser { nodes: Vec::new(), limits, cancelled, work: 0, instructions: 0, calculations: SmallVec::new(), functions: SmallVec::new(), dependencies: Dependencies::default() };
    parser.skip(&mut values)?;
    let color = parser.take(&mut values)?;
    let part = parser.color(color, 0)?;
    parser.end(values)?;
    Ok(match part {
        Part::Atom(Atom::Absolute(c)) => Value::Absolute(c),
        Part::Atom(Atom::Current) => Value::CurrentColor,
        Part::Atom(Atom::System(s)) => Value::System(s),
        Part::Atom(Atom::Functional(_)) => Value::Functional(Arc::new(parser.functions.pop().unwrap())),
        Part::Node(root) => Value::Expression(Arc::new(Expression {
            nodes: parser.nodes,
            root,
            functions: if parser.functions.is_empty() { Box::default() } else { parser.functions.into_vec().into_boxed_slice() },
            math: parser.dependencies.math,
            current_color: parser.dependencies.current_color,
            system_colors: parser.dependencies.system_colors,
            scheme: parser.dependencies.scheme,
        })),
    })
}
impl Parser<'_> {
    fn tick(&mut self, amount: usize) -> Result<()> {
        check(self.cancelled)?;
        self.work = self.work.checked_add(amount).ok_or(Error::Limit)?;
        if self.work > self.limits.work { Err(Error::Limit) } else { Ok(()) }
    }
    fn take<'a>(&mut self, input: &mut Components<'a>) -> Result<Component<'a>> {
        let value = input.next().ok_or(Error::Invalid)?;
        let span = value.span();
        self.tick((span.end - span.start) as usize + 1)?;
        Ok(value)
    }
    fn skip(&mut self, input: &mut Components<'_>) -> Result<()> {
        while input.clone().next().is_some_and(|c| c.kind() == Kind::Whitespace) {
            self.take(input)?;
        }
        Ok(())
    }
    fn end(&mut self, mut input: Components<'_>) -> Result<()> {
        self.skip(&mut input)?;
        if input.next().is_some() { Err(Error::Invalid) } else { Ok(()) }
    }
    fn delimiter(&mut self, input: &mut Components<'_>, wanted: char) -> Result<()> {
        self.skip(input)?;
        let c = self.take(input)?;
        if (wanted == ',' && c.kind() == Kind::Comma) || c.character() == Some(wanted) { Ok(()) } else { Err(Error::Invalid) }
    }
    fn node(&mut self, node: Node) -> Result<u32> {
        self.tick(1)?;
        if self.nodes.len().saturating_add(self.instructions) >= self.limits.nodes || self.nodes.len() >= u32::MAX as usize {
            return Err(Error::Limit);
        }
        let id = self.nodes.len() as u32;
        self.nodes.push(node);
        Ok(id)
    }
    fn index(&mut self, part: Part) -> Result<u32> {
        match part {
            Part::Node(i) => Ok(i),
            Part::Atom(a) => self.node(match a {
                Atom::Absolute(c) => Node::Absolute(c),
                Atom::Current => Node::Current,
                Atom::System(s) => Node::System(s),
                Atom::Functional(i) => Node::Functional(i),
            }),
        }
    }
    fn color(&mut self, c: Component<'_>, depth: usize) -> Result<Part> {
        self.tick(1)?;
        if depth > self.limits.depth.min(128) {
            return Err(Error::DepthLimit);
        }
        let atom = match c.kind() {
            Kind::Ident => self.keyword(c.text().unwrap())?,
            Kind::Hash | Kind::IdHash => Atom::Absolute(hex(c.text().unwrap()).ok_or(Error::Invalid)?),
            Kind::Function => {
                let name = c.text().unwrap();
                self.tick(name.len())?;
                let name = lower(name)?;
                let name = name.as_str();
                let mut args = c.children();
                self.skip(&mut args)?;
                if args.clone().next().is_some_and(|v| v.kind() == Kind::Ident && v.text().unwrap().eq_ignore_ascii_case("from"))
                    && matches!(name, "rgb" | "rgba" | "hsl" | "hsla" | "hwb" | "lab" | "lch" | "oklab" | "oklch" | "color")
                {
                    return Err(Error::Unsupported("relative colors"));
                }
                if name == "light-dark" {
                    self.dependencies.scheme = true;
                    let first = self.take(&mut args)?;
                    let light = self.color(first, depth + 1)?;
                    self.delimiter(&mut args, ',')?;
                    self.skip(&mut args)?;
                    let second = self.take(&mut args)?;
                    let dark = self.color(second, depth + 1)?;
                    self.end(args)?;
                    let light = self.index(light)?;
                    let dark = self.index(dark)?;
                    return Ok(Part::Node(self.node(Node::LightDark { light, dark })?));
                }
                match name {
                    "rgb" | "rgba" => self.functional(args, depth, Space::Srgb, true)?,
                    "hsl" | "hsla" => self.functional(args, depth, Space::Hsl, true)?,
                    "hwb" => self.functional(args, depth, Space::Hwb, false)?,
                    "lab" => self.functional(args, depth, Space::Lab, false)?,
                    "lch" => self.functional(args, depth, Space::Lch, false)?,
                    "oklab" => self.functional(args, depth, Space::Oklab, false)?,
                    "oklch" => self.functional(args, depth, Space::Oklch, false)?,
                    "color" => {
                        let prefix = self.take(&mut args)?;
                        if prefix.kind() != Kind::Ident {
                            return Err(Error::Invalid);
                        }
                        let label = prefix.text().unwrap();
                        self.tick(label.len())?;
                        if label.starts_with("--") {
                            return Err(Error::Unsupported("custom color profiles"));
                        }
                        let space = space(label).ok_or(Error::Invalid)?;
                        self.functional(args, depth, space, false)?
                    }
                    "var" | "env" | "attr" => return Err(Error::RequiresSubstitution),
                    "color-mix" => return Err(Error::Unsupported("color mixing")),
                    "device-cmyk" => return Err(Error::Unsupported("device colors")),
                    "contrast-color" | "color-contrast" => return Err(Error::Unsupported("contrast colors")),
                    "alpha" => return Err(Error::Unsupported("relative alpha colors")),
                    "color-layers" => return Err(Error::Unsupported("color layers")),
                    _ => return Err(Error::Invalid),
                }
            }
            _ => return Err(Error::Invalid),
        };
        Ok(Part::Atom(atom))
    }
    fn keyword(&mut self, name: &str) -> Result<Atom> {
        self.tick(name.len())?;
        let name = lower(name)?;
        let name = name.as_str();
        if name == "currentcolor" {
            self.dependencies.current_color = true;
            return Ok(Atom::Current);
        }
        if name == "transparent" {
            return Ok(Atom::Absolute(Color::new(Space::Srgb, [0.; 3], 0., 0)));
        }
        if let Some(s) = system(name) {
            self.dependencies.system_colors = true;
            self.dependencies.scheme = true;
            return Ok(Atom::System(s));
        }
        let at = named::NAMES.binary_search_by_key(&name, |(n, _)| *n).map_err(|_| Error::Invalid)?;
        let rgb = named::NAMES[at].1;
        Ok(Atom::Absolute(Color::new(Space::Srgb, [((rgb >> 16) & 255) as f64 / 255., ((rgb >> 8) & 255) as f64 / 255., (rgb & 255) as f64 / 255.], 1., 0)))
    }
    fn number(&mut self, input: &mut Components<'_>, depth: usize, angular: bool) -> Result<Numeric> {
        self.skip(input)?;
        let c = self.take(input)?;
        let (value, unit) = match c.kind() {
            Kind::Integer | Kind::Number => (c.number().unwrap().value, Unit::Number),
            Kind::IntegerPercentage | Kind::Percentage => (c.number().unwrap().value, Unit::Percent),
            Kind::IntegerDimension | Kind::Dimension => {
                let value = c.number().unwrap().value;
                let unit = c.unit().unwrap();
                self.tick(unit.len())?;
                let degrees = if unit.eq_ignore_ascii_case("deg") {
                    value
                } else if unit.eq_ignore_ascii_case("grad") {
                    value * 0.9
                } else if unit.eq_ignore_ascii_case("turn") {
                    value * 360.
                } else if unit.eq_ignore_ascii_case("rad") {
                    value.to_degrees()
                } else {
                    return Err(Error::Invalid);
                };
                (degrees, Unit::Angle)
            }
            Kind::Ident if c.text().unwrap().eq_ignore_ascii_case("none") => (0., Unit::None),
            Kind::Function => {
                let mut limits = functional::math_limits(self.limits);
                limits.work = limits.work.saturating_sub(self.work);
                limits.instructions = limits.instructions.saturating_sub(self.nodes.len().saturating_add(self.instructions));
                limits.depth = limits.depth.saturating_sub(depth);
                let (value, work) = math::Value::compile_accounted(c, math::Options { allow_percentage: !angular, percentage_hint: None }, limits, self.cancelled).map_err(functional::math_error)?;
                self.tick(work)?;
                let unit = match value.ty().kind() {
                    Some(math::Dimension::Number) => Unit::Number,
                    Some(math::Dimension::Percentage) => Unit::Percent,
                    Some(math::Dimension::Angle) if angular => Unit::Angle,
                    _ => return Err(Error::Invalid),
                };
                if let Some(q) = value.constant() {
                    return Ok(Numeric::literal(q.value, unit));
                }
                self.instructions = self.instructions.checked_add(value.memory().instructions).ok_or(Error::Limit)?;
                self.dependencies.math = self.dependencies.math.union(value.dependencies());
                let id = u32::try_from(self.calculations.len()).map_err(|_| Error::Limit)?;
                self.calculations.push(Some(value));
                return Ok(Numeric { value: 0., unit, calculation: id });
            }
            _ => return Err(Error::Invalid),
        };
        Ok(Numeric::literal(value, unit))
    }
    fn functional(&mut self, mut args: Components<'_>, depth: usize, space: Space, legacy_allowed: bool) -> Result<Atom> {
        let mut numbers = [Numeric::literal(0., Unit::Number); 4];
        numbers[3].value = 1.;
        numbers[0] = self.number(&mut args, depth, functional::angular(space, 0))?;
        self.skip(&mut args)?;
        let comma = args.clone().next().is_some_and(|c| c.kind() == Kind::Comma);
        if comma && !legacy_allowed {
            return Err(Error::Invalid);
        }
        for (i, number) in numbers.iter_mut().enumerate().take(3).skip(1) {
            if comma {
                self.delimiter(&mut args, ',')?;
            }
            *number = self.number(&mut args, depth, functional::angular(space, i))?;
        }
        self.skip(&mut args)?;
        if args.clone().next().is_some() {
            self.delimiter(&mut args, if comma { ',' } else { '/' })?;
            numbers[3] = self.number(&mut args, depth, false)?;
        }
        self.end(args)?;
        if self.calculations.is_empty() {
            return Ok(Atom::Absolute(functional::checked_materialize(&numbers, space, legacy_allowed, comma)?));
        }
        functional::validate(&numbers, space, comma)?;
        let channels = std::array::from_fn(|i| {
            let n = numbers[i];
            if n.calculation == u32::MAX { math::Value::Constant(n.quantity()) } else { self.calculations[n.calculation as usize].take().unwrap() }
        });
        self.calculations.clear();
        let id = u32::try_from(self.functions.len()).map_err(|_| Error::Limit)?;
        self.functions.push(Functional { channels, units: numbers.map(|n| n.unit), space, legacy: legacy_allowed });
        Ok(Atom::Functional(id))
    }
}
pub(super) fn space(name: &str) -> Option<Space> {
    Some(if name.eq_ignore_ascii_case("srgb") {
        Space::Srgb
    } else if name.eq_ignore_ascii_case("srgb-linear") {
        Space::SrgbLinear
    } else if name.eq_ignore_ascii_case("display-p3") {
        Space::DisplayP3
    } else if name.eq_ignore_ascii_case("display-p3-linear") {
        Space::DisplayP3Linear
    } else if name.eq_ignore_ascii_case("a98-rgb") {
        Space::A98Rgb
    } else if name.eq_ignore_ascii_case("prophoto-rgb") {
        Space::ProphotoRgb
    } else if name.eq_ignore_ascii_case("rec2020") {
        Space::Rec2020
    } else if name.eq_ignore_ascii_case("xyz-d50") {
        Space::XyzD50
    } else if name.eq_ignore_ascii_case("xyz") || name.eq_ignore_ascii_case("xyz-d65") {
        Space::XyzD65
    } else {
        return None;
    })
}
struct Lower {
    bytes: [u8; 32],
    len: usize,
}
impl Lower {
    fn as_str(&self) -> &str {
        std::str::from_utf8(&self.bytes[..self.len]).unwrap()
    }
}
fn lower(name: &str) -> Result<Lower> {
    if name.len() > 32 || !name.is_ascii() {
        return Err(Error::Invalid);
    }
    let mut result = Lower { bytes: [0; 32], len: name.len() };
    for (out, input) in result.bytes.iter_mut().zip(name.bytes()) {
        *out = input.to_ascii_lowercase();
    }
    Ok(result)
}
fn hex(value: &str) -> Option<Color> {
    let bytes = value.as_bytes();
    if !matches!(bytes.len(), 3 | 4 | 6 | 8) {
        return None;
    }
    let digit = |b: u8| match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    };
    let mut channels = [255u8; 4];
    if bytes.len() <= 4 {
        for (out, &b) in channels.iter_mut().zip(bytes) {
            *out = digit(b)? * 17;
        }
    } else {
        for (out, pair) in channels.iter_mut().zip(bytes.as_chunks::<2>().0) {
            *out = digit(pair[0])? * 16 + digit(pair[1])?;
        }
    }
    Some(Color::new(Space::Srgb, std::array::from_fn(|i| channels[i] as f64 / 255.), channels[3] as f64 / 255., 0))
}
fn system(name: &str) -> Option<SystemColor> {
    Some(match name {
        "accentcolor" => SystemColor::AccentColor,
        "accentcolortext" => SystemColor::AccentColorText,
        "activetext" => SystemColor::ActiveText,
        "buttonborder" => SystemColor::ButtonBorder,
        "buttonface" => SystemColor::ButtonFace,
        "buttontext" => SystemColor::ButtonText,
        "canvas" => SystemColor::Canvas,
        "canvastext" => SystemColor::CanvasText,
        "field" => SystemColor::Field,
        "fieldtext" => SystemColor::FieldText,
        "graytext" => SystemColor::GrayText,
        "highlight" => SystemColor::Highlight,
        "highlighttext" => SystemColor::HighlightText,
        "linktext" => SystemColor::LinkText,
        "mark" => SystemColor::Mark,
        "marktext" => SystemColor::MarkText,
        "selecteditem" => SystemColor::SelectedItem,
        "selecteditemtext" => SystemColor::SelectedItemText,
        "visitedtext" => SystemColor::VisitedText,
        "activeborder" => SystemColor::ActiveBorder,
        "activecaption" => SystemColor::ActiveCaption,
        "appworkspace" => SystemColor::AppWorkspace,
        "background" => SystemColor::Background,
        "buttonhighlight" => SystemColor::ButtonHighlight,
        "buttonshadow" => SystemColor::ButtonShadow,
        "captiontext" => SystemColor::CaptionText,
        "inactiveborder" => SystemColor::InactiveBorder,
        "inactivecaption" => SystemColor::InactiveCaption,
        "inactivecaptiontext" => SystemColor::InactiveCaptionText,
        "infobackground" => SystemColor::InfoBackground,
        "infotext" => SystemColor::InfoText,
        "menu" => SystemColor::Menu,
        "menutext" => SystemColor::MenuText,
        "scrollbar" => SystemColor::Scrollbar,
        "threeddarkshadow" => SystemColor::ThreeDDarkShadow,
        "threedface" => SystemColor::ThreeDFace,
        "threedhighlight" => SystemColor::ThreeDHighlight,
        "threedlightshadow" => SystemColor::ThreeDLightShadow,
        "threedshadow" => SystemColor::ThreeDShadow,
        "window" => SystemColor::Window,
        "windowframe" => SystemColor::WindowFrame,
        "windowtext" => SystemColor::WindowText,
        _ => return None,
    })
}
