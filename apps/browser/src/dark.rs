//! Dark pages: View ▸ Dark Pages (Automatic follows the desktop, the
//! default; Always; Never). The darkening is done on the GPU, after Servo
//! has drawn a frame: [`Darkener`] copies the frame to a texture and draws
//! it back through a shader that turns how light each colour looks around
//! but keeps its hue (white pages go near-black, dark text near-white, a
//! blue link light blue), so everything is dark: blank pages, pages with
//! no styles, plain-text files, error pages.
//!
//! Pictures would come out as negatives, so a page script ([`script`])
//! turns them around first with CSS's `invert(1) hue-rotate(180deg)`,
//! which is the shader's own transform (it is its own inverse): photos,
//! video and canvases come out as they were, a little dimmer, short of
//! the most saturated colours (which the CSS filter clips). The
//! script also notices a page that is dark by itself (by the colours
//! actually behind its text) and reports it, and that tab is left alone.

use serde::{Deserialize, Serialize};

/// The preference.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum DarkPages {
    /// When the desktop is dark (or night).
    #[default]
    Auto,
    Always,
    Never,
}

impl DarkPages {
    pub fn on(self, desktop_dark: bool) -> bool {
        match self {
            DarkPages::Auto => desktop_dark,
            DarkPages::Always => true,
            DarkPages::Never => false,
        }
    }
}

const VERTEX: &str = r#"
out vec2 uv;
void main() {
    // One triangle covering the frame.
    vec2 p = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2));
    uv = p;
    gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0);
}
"#;

const FRAGMENT: &str = r#"
uniform sampler2D page;
in vec2 uv;
out vec4 color;
// CSS's luminance weights (hue-rotate's), so this is exactly what the
// pictures' filter does and the two cancel.
const vec3 W = vec3(0.213, 0.715, 0.072);
// The darkest the page gets (its background) and the lightest (its text).
const float LO = 0.105;
const float HI = 0.925;
void main() {
    vec4 c = texture(page, uv);
    // How light it looks turned around (luminance to 1 - luminance), hue
    // kept: invert(1) hue-rotate(180deg) in one.
    vec3 y = clamp(c.rgb + (1.0 - 2.0 * dot(c.rgb, W)), 0.0, 1.0);
    color = vec4(mix(vec3(LO), vec3(HI), y), c.a);
}
"#;

/// The GPU pass for one window's GL context.
pub struct Darkener {
    program: glow::Program,
    vao: glow::VertexArray,
    texture: glow::Texture,
    framebuffer: glow::Framebuffer,
    size: (i32, i32),
}

impl Darkener {
    /// Compile the shader on `gl` (current).
    pub fn new(gl: &glow::Context) -> Result<Darkener, String> {
        use glow::HasContext as _;
        let header = if gl.version().is_embedded { "#version 300 es\nprecision mediump float;\n" } else { "#version 330 core\n" };
        // SAFETY: plain GL object creation on the current context.
        unsafe {
            let program = gl.create_program()?;
            let mut shaders = Vec::new();
            for (kind, source) in [(glow::VERTEX_SHADER, VERTEX), (glow::FRAGMENT_SHADER, FRAGMENT)] {
                let shader = gl.create_shader(kind)?;
                gl.shader_source(shader, &format!("{header}{source}"));
                gl.compile_shader(shader);
                if !gl.get_shader_compile_status(shader) {
                    let log = gl.get_shader_info_log(shader);
                    gl.delete_shader(shader);
                    gl.delete_program(program);
                    return Err(format!("dark pages shader: {log}"));
                }
                gl.attach_shader(program, shader);
                shaders.push(shader);
            }
            gl.link_program(program);
            for s in shaders {
                gl.detach_shader(program, s);
                gl.delete_shader(s);
            }
            if !gl.get_program_link_status(program) {
                let log = gl.get_program_info_log(program);
                gl.delete_program(program);
                return Err(format!("dark pages shader: {log}"));
            }
            let vao = gl.create_vertex_array()?;
            let texture = gl.create_texture()?;
            let framebuffer = gl.create_framebuffer()?;
            Ok(Darkener { program, vao, texture, framebuffer, size: (0, 0) })
        }
    }

    /// Darken the frame in the framebuffer bound now (Servo's, just
    /// painted), `size` device pixels, leaving GL as it found it (Servo's
    /// renderer keeps its own idea of the bindings).
    pub fn apply(&mut self, gl: &glow::Context, size: (i32, i32)) {
        use glow::HasContext as _;
        let (w, h) = size;
        if w <= 0 || h <= 0 {
            return;
        }
        // SAFETY: GL calls on the current context with objects we made;
        // all state touched is saved first and put back after.
        unsafe {
            let target = gl.get_parameter_framebuffer(glow::FRAMEBUFFER_BINDING);
            let read = gl.get_parameter_framebuffer(glow::READ_FRAMEBUFFER_BINDING);
            let program = gl.get_parameter_program(glow::CURRENT_PROGRAM);
            let active = gl.get_parameter_i32(glow::ACTIVE_TEXTURE) as u32;
            gl.active_texture(glow::TEXTURE0);
            let texture = gl.get_parameter_texture(glow::TEXTURE_BINDING_2D);
            let vao = gl.get_parameter_vertex_array(glow::VERTEX_ARRAY_BINDING);
            let mut viewport = [0i32; 4];
            gl.get_parameter_i32_slice(glow::VIEWPORT, &mut viewport);
            let caps = [glow::SCISSOR_TEST, glow::BLEND, glow::DEPTH_TEST, glow::CULL_FACE, glow::STENCIL_TEST];
            let enabled: Vec<bool> = caps.iter().map(|&c| gl.is_enabled(c)).collect();
            let mask = gl.get_parameter_bool_array::<4>(glow::COLOR_WRITEMASK);

            gl.bind_texture(glow::TEXTURE_2D, Some(self.texture));
            if self.size != size {
                gl.tex_image_2d(glow::TEXTURE_2D, 0, glow::RGBA8 as i32, w, h, 0, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelUnpackData::Slice(None));
                for (p, v) in [
                    (glow::TEXTURE_MIN_FILTER, glow::NEAREST),
                    (glow::TEXTURE_MAG_FILTER, glow::NEAREST),
                    (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                    (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
                ] {
                    gl.tex_parameter_i32(glow::TEXTURE_2D, p, v as i32);
                }
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.framebuffer));
                gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(self.texture), 0);
                self.size = size;
            }
            for &c in &caps {
                gl.disable(c);
            }
            gl.color_mask(true, true, true, true);
            // The frame to our texture…
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, target);
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(self.framebuffer));
            gl.blit_framebuffer(0, 0, w, h, 0, 0, w, h, glow::COLOR_BUFFER_BIT, glow::NEAREST);
            // …and back through the shader.
            gl.bind_framebuffer(glow::FRAMEBUFFER, target);
            gl.viewport(0, 0, w, h);
            gl.use_program(Some(self.program));
            gl.uniform_1_i32(gl.get_uniform_location(self.program, "page").as_ref(), 0);
            gl.bind_vertex_array(Some(self.vao));
            gl.draw_arrays(glow::TRIANGLES, 0, 3);

            gl.bind_vertex_array(vao);
            gl.use_program(program);
            gl.bind_texture(glow::TEXTURE_2D, texture);
            gl.active_texture(active);
            gl.viewport(viewport[0], viewport[1], viewport[2], viewport[3]);
            for (&c, &on) in caps.iter().zip(&enabled) {
                if on {
                    gl.enable(c);
                }
            }
            gl.color_mask(mask[0], mask[1], mask[2], mask[3]);
            gl.bind_framebuffer(glow::FRAMEBUFFER, target);
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, read);
        }
    }
}

pub fn marker(token: &str) -> String {
    format!("\u{1}tabula-dark-{token}:")
}

/// The name of the page function NetPositive calls to say whether pages
/// are being darkened (`true`/`false`).
pub fn switch(token: &str) -> String {
    format!("__tabulaDark{token}")
}

/// The page script: pictures turned around while pages are darkened, and
/// whether the page is dark by itself, reported (`true`/`false`) when it
/// changes. `on`: whether pages are being darkened as it starts.
pub fn script(token: &str, on: bool) -> String {
    let marker = serde_json::to_string(&marker(token)).unwrap_or_default();
    let switch = serde_json::to_string(&switch(token)).unwrap_or_default();
    format!(
        r#"(function (marker, name, on) {{
  if (window[name]) return;
  var top = window === window.top;
  var dark = false, reported = null;
  var log = console.log.bind(console);
  var PICS = 'img,video,canvas,embed,object,[data-tabula-pic]';
  function css() {{
    var s = document.getElementById('tabula-dark');
    if (!s) {{
      s = document.createElement('style');
      s.id = 'tabula-dark';
      s.textContent = PICS.split(',').map(function (p) {{ return 'html[data-tabula-dark] ' + p; }}).join(',') +
        '{{filter:invert(1) hue-rotate(180deg) !important}}' +
        'html[data-tabula-dark] [data-tabula-pic] img, html[data-tabula-dark] picture [data-tabula-pic]{{filter:none !important}}';
      (document.head || document.documentElement).appendChild(s);
    }}
  }}
  function apply() {{
    var h = document.documentElement;
    if (!h) return;
    if (on && !dark) {{ css(); h.setAttribute('data-tabula-dark', ''); }}
    else h.removeAttribute('data-tabula-dark');
  }}
  function rgba(s) {{
    var m = /rgba?\(([^)]+)\)/.exec(s || '');
    if (!m) return null;
    var p = m[1].split(/[ ,\/]+/).filter(Boolean).map(parseFloat);
    return {{l: (0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]) / 255, a: p.length > 3 ? p[3] : 1}};
  }}
  // How light it is behind (x, y): the first solid background up from
  // what's there (pictures don't count), else the page's canvas.
  function behind(x, y) {{
    var e = document.elementFromPoint(x, y);
    for (; e && e.nodeType === 1; e = e.parentElement) {{
      if (/^(IMG|VIDEO|CANVAS|PICTURE|SVG)$/i.test(e.tagName)) continue;
      var c = rgba(getComputedStyle(e).backgroundColor);
      if (c && c.a > 0.5) return c.l;
    }}
    return canvas();
  }}
  function canvas() {{
    var els = [document.documentElement, document.body];
    for (var i = 0; i < els.length; i++) {{
      if (!els[i]) continue;
      var c = rgba(getComputedStyle(els[i]).backgroundColor);
      if (c && c.a > 0.5) return c.l;
    }}
    return 1;
  }}
  function measure() {{
    if (!top || !document.documentElement) return;
    var w = innerWidth, h = innerHeight, sum = 0, n = 0;
    for (var i = 1; i <= 5; i++)
      for (var j = 1; j <= 5; j++) {{ sum += behind(w * i / 6, h * j / 6); n++; }}
    var avg = n ? sum / n : 1;
    var text = document.body ? rgba(getComputedStyle(document.body).color) : null;
    // Light text is a dark page's tell; between, keep what it was.
    var was = dark;
    if (avg < 0.35 || (avg < 0.5 && text && text.l > 0.6)) dark = true;
    else if (avg > 0.5) dark = false;
    if (dark !== was) apply();
    if (dark !== reported) {{ reported = dark; log(marker + JSON.stringify(dark)); }}
  }}
  // Elements whose picture is a CSS background (and hold nothing else).
  function mark(root) {{
    if (!root || root.nodeType !== 1) return;
    var all = [root].concat(Array.prototype.slice.call(root.querySelectorAll('*')));
    for (var i = 0; i < all.length && i < 5000; i++) {{
      var e = all[i];
      if (e.children.length || e.textContent.trim()) continue;
      var b = getComputedStyle(e).backgroundImage;
      if (b && b.indexOf('url(') >= 0) e.setAttribute('data-tabula-pic', '');
    }}
  }}
  window[name] = function (v) {{ on = !!v; apply(); }};
  apply();
  var pending = [];
  var timer = null;
  function later() {{
    if (timer) return;
    timer = setTimeout(function () {{
      timer = null;
      var roots = pending; pending = [];
      roots.forEach(mark);
      measure();
    }}, 300);
  }}
  function start() {{
    // At once, so a dark page isn't shown turned around for a moment.
    mark(document.documentElement);
    measure();
    try {{
      new MutationObserver(function (list) {{
        list.forEach(function (m) {{ m.addedNodes.forEach(function (n) {{ if (n.nodeType === 1) pending.push(n); }}); }});
        later();
      }}).observe(document.documentElement, {{childList: true, subtree: true}});
    }} catch (x) {{}}
    addEventListener('load', later);
    addEventListener('resize', later);
    addEventListener('scroll', later, {{passive: true}});
  }}
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', start);
  else start();
}})({marker}, {switch}, {on})"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes() {
        assert!(DarkPages::Auto.on(true) && !DarkPages::Auto.on(false));
        assert!(DarkPages::Always.on(false) && !DarkPages::Never.on(true));
        assert_eq!(serde_json::to_string(&DarkPages::Auto).unwrap(), "\"auto\"");
        let s = script("abc", true);
        assert!(s.contains("tabula-dark-abc:") && s.contains("__tabulaDarkabc") && s.ends_with("true)"));
    }
}
