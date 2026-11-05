//! Foreign-content tree dispatch. Spelling tables follow WHATWG HTML,
//! parsing.html#parsing-main-inforeign (consulted 2026-11-05).
//! SVG rendering, MathML layout and script execution are separate work.
use super::{Builder, Namespace, NodeId, Result, Tag, Token, html_special, whitespace};
use crate::dom::AttributeNamespace;
use std::borrow::Cow;

impl<'a> Builder<'a> {
    fn math_text_point(&self, node: NodeId) -> bool {
        self.document.namespace(node).unwrap() == Some(Namespace::MathMl) && matches!(self.name(node), "mi" | "mo" | "mn" | "ms" | "mtext")
    }
    fn html_point(&self, node: NodeId) -> bool {
        match self.document.namespace(node).unwrap() {
            Some(Namespace::Svg) => matches!(self.name(node), "foreignObject" | "desc" | "title"),
            Some(Namespace::MathMl) if self.name(node) == "annotation-xml" => {
                self.document.attribute(node, "encoding").unwrap().is_some_and(|v| v.eq_ignore_ascii_case("text/html") || v.eq_ignore_ascii_case("application/xhtml+xml"))
            }
            _ => false,
        }
    }
    #[inline]
    pub(super) fn special(&self, node: NodeId) -> bool {
        let (name, namespace) = self.document.element_name_and_namespace(node).unwrap();
        match namespace {
            Namespace::Html => html_special(name),
            _ => self.foreign_scope_boundary(node),
        }
    }
    pub(super) fn foreign_scope_boundary(&self, node: NodeId) -> bool {
        self.math_text_point(node)
            || match self.document.namespace(node).unwrap() {
                Some(Namespace::MathMl) => self.name(node) == "annotation-xml",
                Some(Namespace::Svg) => matches!(self.name(node), "foreignObject" | "desc" | "title"),
                _ => false,
            }
    }
    #[inline]
    pub(super) fn scope_boundary(&self, node: NodeId) -> bool {
        let (name, namespace) = self.document.element_name_and_namespace(node).unwrap();
        match namespace {
            Namespace::Html => matches!(name, "applet" | "caption" | "html" | "table" | "td" | "th" | "marquee" | "object" | "select" | "template"),
            _ => self.foreign_scope_boundary(node),
        }
    }

    pub(super) fn insert_foreign(&mut self, tag: &mut Tag<'a>, namespace: Namespace) -> Result<NodeId> {
        self.foreign_seen = true;
        if namespace == Namespace::Svg {
            tag.name = adjust_svg_tag(std::mem::replace(&mut tag.name, Cow::Borrowed("")));
        }
        let node = self.document.create_element(&tag.name, namespace)?;
        self.document.initial_attributes_ns(
            node,
            tag.attributes.drain(..).map(|a| {
                let name = if namespace == Namespace::Svg {
                    adjust_svg_attribute(a.name)
                } else if a.name == "definitionurl" {
                    Cow::Borrowed("definitionURL")
                } else {
                    a.name
                };
                let namespace = match name.as_ref() {
                    "xlink:actuate" | "xlink:arcrole" | "xlink:href" | "xlink:role" | "xlink:show" | "xlink:title" | "xlink:type" => Some(AttributeNamespace::Xlink),
                    "xml:lang" | "xml:space" => Some(AttributeNamespace::Xml),
                    "xmlns" | "xmlns:xlink" => Some(AttributeNamespace::Xmlns),
                    _ => None,
                };
                (name, a.value, namespace)
            }),
        )?;
        let (parent, before) = self.insertion_location(self.current())?;
        self.document.insert_before(parent, node, before)?;
        self.push(node)?;
        // SVG script elements are inert until the script runner exists.
        if tag.self_closing {
            self.stack.pop();
        }
        Ok(node)
    }

    #[inline]
    pub(super) fn dispatch_foreign(&mut self, token: &mut Option<Token<'a>>) -> Result<bool> {
        let Some(current) = self.adjusted_current() else { return Ok(false) };
        let namespace = self.document.namespace(current)?.unwrap();
        if namespace == Namespace::Html || token.is_none() {
            return Ok(false);
        }
        self.process_foreign(token, current, namespace)
    }
    // Keep the sizeable foreign rules out of the ordinary HTML token loop.
    #[inline(never)]
    fn process_foreign(&mut self, token: &mut Option<Token<'a>>, current: NodeId, namespace: Namespace) -> Result<bool> {
        if self.math_text_point(current) && matches!(token, Some(Token::Text(_))) {
            return Ok(false);
        }
        if let Some(Token::StartTag(tag)) = token {
            if self.math_text_point(current) && !matches!(tag.name.as_ref(), "mglyph" | "malignmark") {
                return Ok(false);
            }
            if namespace == Namespace::MathMl && self.name(current) == "annotation-xml" && tag.name == "svg" {
                return Ok(false);
            }
        }
        if self.html_point(current) && matches!(token, Some(Token::Text(_) | Token::StartTag(_))) {
            return Ok(false);
        }
        match token {
            Some(Token::Text(text)) => {
                let mut text = std::mem::replace(text, Cow::Borrowed(""));
                if text.bytes().any(|b| !whitespace(b) && b != 0) {
                    self.frameset_ok = false;
                }
                if text.contains('\0') {
                    text = Cow::Owned(text.replace('\0', "\u{fffd}"));
                }
                self.insert_text(text)?;
            }
            Some(Token::StartTag(tag)) => {
                let breakout = matches!(
                    tag.name.as_ref(),
                    "b" | "big"
                        | "blockquote"
                        | "body"
                        | "br"
                        | "center"
                        | "code"
                        | "dd"
                        | "div"
                        | "dl"
                        | "dt"
                        | "em"
                        | "embed"
                        | "h1"
                        | "h2"
                        | "h3"
                        | "h4"
                        | "h5"
                        | "h6"
                        | "head"
                        | "hr"
                        | "i"
                        | "img"
                        | "li"
                        | "listing"
                        | "menu"
                        | "meta"
                        | "nobr"
                        | "ol"
                        | "p"
                        | "pre"
                        | "ruby"
                        | "s"
                        | "small"
                        | "span"
                        | "strong"
                        | "strike"
                        | "sub"
                        | "sup"
                        | "table"
                        | "tt"
                        | "u"
                        | "ul"
                        | "var"
                ) || tag.name == "font" && tag.attributes.iter().any(|a| matches!(a.name.as_ref(), "color" | "face" | "size"));
                if breakout {
                    self.pop_foreign();
                    return Ok(false);
                }
                self.insert_foreign(tag, namespace)?;
            }
            Some(Token::EndTag(tag)) => {
                if matches!(tag.name.as_ref(), "br" | "p") {
                    self.pop_foreign();
                    return Ok(false);
                }
                for at in (1..self.stack.len()).rev() {
                    if self.name(self.stack[at]).eq_ignore_ascii_case(&tag.name) {
                        self.stack.truncate(at);
                        return Ok(true);
                    }
                    if self.document.namespace(self.stack[at - 1])? == Some(Namespace::Html) {
                        return Ok(false);
                    }
                }
            }
            Some(Token::Doctype(_)) => {}
            _ => {
                self.miscellaneous(token, current)?;
            }
        }
        Ok(true)
    }
    fn pop_foreign(&mut self) {
        while self.stack.len() > 1 {
            let current = self.current();
            if self.document.namespace(current).unwrap() == Some(Namespace::Html) || self.math_text_point(current) || self.html_point(current) {
                break;
            }
            self.stack.pop();
        }
    }
}

fn adjust_svg_tag(name: Cow<'_, str>) -> Cow<'_, str> {
    match name.as_ref() {
        "altglyph" => Cow::Borrowed("altGlyph"),
        "altglyphdef" => Cow::Borrowed("altGlyphDef"),
        "altglyphitem" => Cow::Borrowed("altGlyphItem"),
        "animatecolor" => Cow::Borrowed("animateColor"),
        "animatemotion" => Cow::Borrowed("animateMotion"),
        "animatetransform" => Cow::Borrowed("animateTransform"),
        "clippath" => Cow::Borrowed("clipPath"),
        "feblend" => Cow::Borrowed("feBlend"),
        "fecolormatrix" => Cow::Borrowed("feColorMatrix"),
        "fecomponenttransfer" => Cow::Borrowed("feComponentTransfer"),
        "fecomposite" => Cow::Borrowed("feComposite"),
        "feconvolvematrix" => Cow::Borrowed("feConvolveMatrix"),
        "fediffuselighting" => Cow::Borrowed("feDiffuseLighting"),
        "fedisplacementmap" => Cow::Borrowed("feDisplacementMap"),
        "fedistantlight" => Cow::Borrowed("feDistantLight"),
        "fedropshadow" => Cow::Borrowed("feDropShadow"),
        "feflood" => Cow::Borrowed("feFlood"),
        "fefunca" => Cow::Borrowed("feFuncA"),
        "fefuncb" => Cow::Borrowed("feFuncB"),
        "fefuncg" => Cow::Borrowed("feFuncG"),
        "fefuncr" => Cow::Borrowed("feFuncR"),
        "fegaussianblur" => Cow::Borrowed("feGaussianBlur"),
        "feimage" => Cow::Borrowed("feImage"),
        "femerge" => Cow::Borrowed("feMerge"),
        "femergenode" => Cow::Borrowed("feMergeNode"),
        "femorphology" => Cow::Borrowed("feMorphology"),
        "feoffset" => Cow::Borrowed("feOffset"),
        "fepointlight" => Cow::Borrowed("fePointLight"),
        "fespecularlighting" => Cow::Borrowed("feSpecularLighting"),
        "fespotlight" => Cow::Borrowed("feSpotLight"),
        "fetile" => Cow::Borrowed("feTile"),
        "feturbulence" => Cow::Borrowed("feTurbulence"),
        "foreignobject" => Cow::Borrowed("foreignObject"),
        "glyphref" => Cow::Borrowed("glyphRef"),
        "lineargradient" => Cow::Borrowed("linearGradient"),
        "radialgradient" => Cow::Borrowed("radialGradient"),
        "textpath" => Cow::Borrowed("textPath"),
        _ => name,
    }
}
fn adjust_svg_attribute(name: Cow<'_, str>) -> Cow<'_, str> {
    match name.as_ref() {
        "attributename" => Cow::Borrowed("attributeName"),
        "attributetype" => Cow::Borrowed("attributeType"),
        "basefrequency" => Cow::Borrowed("baseFrequency"),
        "baseprofile" => Cow::Borrowed("baseProfile"),
        "calcmode" => Cow::Borrowed("calcMode"),
        "clippathunits" => Cow::Borrowed("clipPathUnits"),
        "diffuseconstant" => Cow::Borrowed("diffuseConstant"),
        "edgemode" => Cow::Borrowed("edgeMode"),
        "filterunits" => Cow::Borrowed("filterUnits"),
        "glyphref" => Cow::Borrowed("glyphRef"),
        "gradienttransform" => Cow::Borrowed("gradientTransform"),
        "gradientunits" => Cow::Borrowed("gradientUnits"),
        "kernelmatrix" => Cow::Borrowed("kernelMatrix"),
        "kernelunitlength" => Cow::Borrowed("kernelUnitLength"),
        "keypoints" => Cow::Borrowed("keyPoints"),
        "keysplines" => Cow::Borrowed("keySplines"),
        "keytimes" => Cow::Borrowed("keyTimes"),
        "lengthadjust" => Cow::Borrowed("lengthAdjust"),
        "limitingconeangle" => Cow::Borrowed("limitingConeAngle"),
        "markerheight" => Cow::Borrowed("markerHeight"),
        "markerunits" => Cow::Borrowed("markerUnits"),
        "markerwidth" => Cow::Borrowed("markerWidth"),
        "maskcontentunits" => Cow::Borrowed("maskContentUnits"),
        "maskunits" => Cow::Borrowed("maskUnits"),
        "numoctaves" => Cow::Borrowed("numOctaves"),
        "pathlength" => Cow::Borrowed("pathLength"),
        "patterncontentunits" => Cow::Borrowed("patternContentUnits"),
        "patterntransform" => Cow::Borrowed("patternTransform"),
        "patternunits" => Cow::Borrowed("patternUnits"),
        "pointsatx" => Cow::Borrowed("pointsAtX"),
        "pointsaty" => Cow::Borrowed("pointsAtY"),
        "pointsatz" => Cow::Borrowed("pointsAtZ"),
        "preservealpha" => Cow::Borrowed("preserveAlpha"),
        "preserveaspectratio" => Cow::Borrowed("preserveAspectRatio"),
        "primitiveunits" => Cow::Borrowed("primitiveUnits"),
        "refx" => Cow::Borrowed("refX"),
        "refy" => Cow::Borrowed("refY"),
        "repeatcount" => Cow::Borrowed("repeatCount"),
        "repeatdur" => Cow::Borrowed("repeatDur"),
        "requiredextensions" => Cow::Borrowed("requiredExtensions"),
        "requiredfeatures" => Cow::Borrowed("requiredFeatures"),
        "specularconstant" => Cow::Borrowed("specularConstant"),
        "specularexponent" => Cow::Borrowed("specularExponent"),
        "spreadmethod" => Cow::Borrowed("spreadMethod"),
        "startoffset" => Cow::Borrowed("startOffset"),
        "stddeviation" => Cow::Borrowed("stdDeviation"),
        "stitchtiles" => Cow::Borrowed("stitchTiles"),
        "surfacescale" => Cow::Borrowed("surfaceScale"),
        "systemlanguage" => Cow::Borrowed("systemLanguage"),
        "tablevalues" => Cow::Borrowed("tableValues"),
        "targetx" => Cow::Borrowed("targetX"),
        "targety" => Cow::Borrowed("targetY"),
        "textlength" => Cow::Borrowed("textLength"),
        "viewbox" => Cow::Borrowed("viewBox"),
        "viewtarget" => Cow::Borrowed("viewTarget"),
        "xchannelselector" => Cow::Borrowed("xChannelSelector"),
        "ychannelselector" => Cow::Borrowed("yChannelSelector"),
        "zoomandpan" => Cow::Borrowed("zoomAndPan"),
        _ => name,
    }
}
