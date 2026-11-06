use super::*;
fn keyword(values: &[Entry], p: Property) -> Result<Keyword> {
    let Value::Keyword(k) = &get(values, p).value else { return Err(Error::Invalid) };
    Ok(*k)
}
fn get(values: &[Entry], property: Property) -> &Entry {
    &values[values.binary_search_by_key(&(property as u16), |e| e.property as u16).unwrap()]
}
fn set(values: &mut [Entry], property: Property, value: Value) {
    let i = values.binary_search_by_key(&(property as u16), |e| e.property as u16).unwrap();
    values[i].value = value;
}
fn blockify(display: Display) -> Display {
    match display {
        Display::Box { outer: Outer::Inline | Outer::RunIn, inner: Inner::FlowRoot, list_item } => Display::Box { outer: Outer::Block, inner: Inner::Flow, list_item },
        Display::Box { inner: Inner::Ruby, list_item, .. } => Display::Box { outer: Outer::Block, inner: Inner::Flow, list_item },
        Display::Box { inner, list_item, .. } => Display::Box { outer: Outer::Block, inner, list_item },
        Display::Internal(_) => Display::Box { outer: Outer::Block, inner: Inner::Flow, list_item: false },
        d => d,
    }
}
pub(super) fn apply(values: &mut [Entry], parent: Option<&Style>, axes: Axes, options: Options) -> Result<()> {
    use Property as P;
    let x = keyword(values, P::OverflowX)?;
    let y = keyword(values, P::OverflowY)?;
    if !matches!(x, Keyword::Visible | Keyword::Clip) || !matches!(y, Keyword::Visible | Keyword::Clip) {
        let transform = |k| match k {
            Keyword::Visible => Keyword::Auto,
            k => k,
        };
        set(values, P::OverflowX, Value::Keyword(transform(x)));
        set(values, P::OverflowY, Value::Keyword(transform(y)));
    }
    let position = keyword(values, P::Position)?;
    let float = keyword(values, P::Float)?;
    if matches!(position, Keyword::Absolute | Keyword::Fixed) {
        set(values, P::Float, Value::Keyword(Keyword::None));
    }
    let Value::Display(mut display) = get(values, P::Display).value else { return Err(Error::Invalid) };
    if options.root && display == Display::Contents {
        display = Display::Box { outer: Outer::Block, inner: Inner::Flow, list_item: false };
    }
    let item = matches!(options.layout_parent.map(|p| p.display), Some(Display::Box { inner: Inner::Flex | Inner::Grid, .. }));
    if options.root || item || matches!(position, Keyword::Absolute | Keyword::Fixed) || float != Keyword::None {
        display = blockify(display);
    }
    // Orthogonal inline flows establish an independent formatting context.
    if options.layout_parent.map(|p| p.axes).or_else(|| parent.map(|p| p.axes)).is_some_and(|p| p.writing_mode != axes.writing_mode)
        && let Display::Box { ref mut inner, .. } = display
        && *inner == Inner::Flow
    {
        *inner = Inner::FlowRoot;
    }
    set(values, P::Display, Value::Display(display));
    Ok(())
}
