//! WebGL: the page's `getContext('webgl')` canvases drawn by the GPU through
//! OpenGL ES (an EGL surfaceless context, loaded at first use), as Chrome
//! draws them through ANGLE. Each canvas is a framebuffer; the prelude's
//! WebGLRenderingContext forwards every call here by name, and the page
//! reads the pixels back as an image when the canvas is rendered.
//!
//! Objects (buffers, textures, programs, …) are kept in tables here and
//! handed to scripts as small wrapper objects holding an index.
use glow::HasContext;
use std::{cell::RefCell, rc::Rc, sync::Arc};

/// EGL_PLATFORM_SURFACELESS_MESA
const PLATFORM_SURFACELESS_MESA: khronos_egl::Enum = 0x31DD;
/// OLO_GL_TRACE: every call printed.
static TRACE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
/// EGL_CONTEXT_OPENGL_ROBUST_ACCESS_EXT
const CONTEXT_OPENGL_ROBUST_ACCESS_EXT: khronos_egl::Int = 0x30BF;
/// Vertex attributes a context tracks (WebGL's minimum is 16).
const ATTRIBS: usize = 32;
/// What WebGL validates before a draw: each enabled attribute array reads
/// a buffer, and indexed draws have an index buffer. GL itself would read
/// client memory (and fault).
#[derive(Clone, Copy, Default)]
struct Attrib {
    enabled: bool,
    buffer: Option<usize>,
}
#[derive(Clone)]
struct VertexState {
    attribs: [Attrib; ATTRIBS],
    element_buffer: Option<usize>,
}
impl Default for VertexState {
    fn default() -> Self {
        Self { attribs: [Attrib::default(); ATTRIBS], element_buffer: None }
    }
}

/// The GPU the window's renderer draws on (its UUID): WebGL draws on the
/// same one, so frames can be shared with no copy (and a page's canvas does
/// not wake a second GPU). Set before the first canvas; zero: any.
static PREFERRED_DEVICE: std::sync::Mutex<[u8; 16]> = std::sync::Mutex::new([0; 16]);
/// Draw WebGL on the GPU with this UUID (see `PREFERRED_DEVICE`).
pub fn prefer_device(uuid: [u8; 16]) {
    if let Ok(mut d) = PREFERRED_DEVICE.lock() {
        *d = uuid;
    }
}
/// EGL_PLATFORM_DEVICE_EXT
const PLATFORM_DEVICE_EXT: khronos_egl::Enum = 0x313F;
/// The thread's EGL: display and config, shared by every canvas's context.
struct Display {
    egl: khronos_egl::DynamicInstance<khronos_egl::EGL1_4>,
    display: khronos_egl::Display,
    config: khronos_egl::Config,
}
thread_local! {
    static DISPLAY: RefCell<Option<Result<Rc<Display>, String>>> = const { RefCell::new(None) };
    /// The EGL context current on this thread (a handle), to skip
    /// make_current when it already is.
    static CURRENT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
impl Display {
    fn shared() -> Result<Rc<Display>, String> {
        DISPLAY.with(|cell| {
            let mut slot = cell.borrow_mut();
            if slot.is_none() {
                *slot = Some(Self::new().map(Rc::new));
            }
            slot.as_ref().unwrap().clone()
        })
    }
    fn new() -> Result<Self, String> {
        // SAFETY: loading libEGL and calling it as the EGL specification
        // describes; the display and config are used on this thread only.
        unsafe {
            let egl = khronos_egl::DynamicInstance::<khronos_egl::EGL1_4>::load_required().map_err(|e| format!("EGL: {e}"))?;
            let platform = egl.upcast::<khronos_egl::EGL1_5>();
            // The window's GPU when it said which (and EGL can name devices).
            let preferred = PREFERRED_DEVICE.lock().map(|d| *d).unwrap_or([0; 16]);
            if preferred != [0; 16]
                && let Some(egl15) = platform
                && let Some(found) = Self::on_device(&egl, egl15, preferred)
            {
                return Ok(found);
            }
            let display = match platform {
                Some(egl15) => egl15
                    .get_platform_display(PLATFORM_SURFACELESS_MESA, khronos_egl::DEFAULT_DISPLAY, &[khronos_egl::ATTRIB_NONE])
                    .or_else(|_| egl.get_display(khronos_egl::DEFAULT_DISPLAY).ok_or(khronos_egl::Error::BadDisplay)),
                None => egl.get_display(khronos_egl::DEFAULT_DISPLAY).ok_or(khronos_egl::Error::BadDisplay),
            }
            .map_err(|e| format!("EGL display: {e}"))?;
            egl.initialize(display).map_err(|e| format!("EGL initialize: {e}"))?;
            egl.bind_api(khronos_egl::OPENGL_ES_API).map_err(|e| format!("EGL bind: {e}"))?;
            let attributes = [khronos_egl::RENDERABLE_TYPE, khronos_egl::OPENGL_ES3_BIT, khronos_egl::SURFACE_TYPE, 0, khronos_egl::NONE];
            let config = match egl.choose_first_config(display, &attributes) {
                Ok(Some(c)) => c,
                _ => {
                    let attributes = [khronos_egl::RENDERABLE_TYPE, khronos_egl::OPENGL_ES2_BIT, khronos_egl::NONE];
                    egl.choose_first_config(display, &attributes).ok().flatten().ok_or("EGL: no config")?
                }
            };
            Ok(Self { egl, display, config })
        }
    }
    /// The display of the EGL device whose GL is on GPU `uuid`: each device
    /// tried (EGL_EXT_device_enumeration), a context made on it and asked
    /// its GL_DEVICE_UUID_EXT.
    unsafe fn on_device(egl: &khronos_egl::DynamicInstance<khronos_egl::EGL1_4>, egl15: &khronos_egl::DynamicInstance<khronos_egl::EGL1_5>, uuid: [u8; 16]) -> Option<Self> {
        // SAFETY: EGL entry points called with their C signatures; displays
        // and contexts made here are made current only to be asked, then let go.
        unsafe {
            let query = egl.get_proc_address("eglQueryDevicesEXT")?;
            let query = std::mem::transmute::<extern "system" fn(), unsafe extern "system" fn(i32, *mut *mut std::ffi::c_void, *mut i32) -> u32>(query);
            let mut devices = [std::ptr::null_mut(); 16];
            let mut count = 0i32;
            if query(devices.len() as i32, devices.as_mut_ptr(), &mut count) == 0 {
                return None;
            }
            for &device in devices.iter().take(count.max(0) as usize) {
                let Ok(display) = egl15.get_platform_display(PLATFORM_DEVICE_EXT, device, &[khronos_egl::ATTRIB_NONE]) else { continue };
                if egl.initialize(display).is_err() {
                    continue;
                }
                if egl.bind_api(khronos_egl::OPENGL_ES_API).is_err() {
                    continue;
                }
                let attributes = [khronos_egl::RENDERABLE_TYPE, khronos_egl::OPENGL_ES3_BIT, khronos_egl::SURFACE_TYPE, 0, khronos_egl::NONE];
                let Ok(Some(config)) = egl.choose_first_config(display, &attributes) else { continue };
                let Ok(context) = egl.create_context(display, config, None, &[khronos_egl::CONTEXT_CLIENT_VERSION, 3, khronos_egl::NONE]) else { continue };
                if egl.make_current(display, None, None, Some(context)).is_err() {
                    let _ = egl.destroy_context(display, context);
                    continue;
                }
                let gl = glow::Context::from_loader_function(|s| egl.get_proc_address(s).map_or(std::ptr::null(), |p| p as *const std::ffi::c_void));
                let found = gl.supported_extensions().contains("GL_EXT_memory_object")
                    && egl.get_proc_address("glGetUnsignedBytevEXT").is_some_and(|f| {
                        let f = std::mem::transmute::<extern "system" fn(), unsafe extern "system" fn(u32, *mut u8)>(f);
                        let mut id = [0u8; 16];
                        f(DEVICE_UUID, id.as_mut_ptr());
                        id == uuid
                    });
                let _ = egl.make_current(display, None, None, None);
                let _ = egl.destroy_context(display, context);
                CURRENT.with(|c| c.set(0));
                if !found {
                    let _ = egl.terminate(display);
                }
                if found {
                    return Some(Self { egl: khronos_egl::DynamicInstance::<khronos_egl::EGL1_4>::load_required().ok()?, display, config });
                }
            }
            None
        }
    }
}
/// A canvas's own GL: its EGL context (WebGL contexts share nothing, and
/// GL state is per context), with glow over it.
pub struct Gl {
    display: Rc<Display>,
    context: khronos_egl::Context,
    pub gl: glow::Context,
    pub es3: bool,
    /// GL_EXT_memory_object_fd: memory another API exported, drawn into
    /// here (a canvas shown by the window's renderer with no copy).
    interop: Option<Interop>,
}
/// The entry points of GL_EXT_memory_object(_fd), and the GPU's UUID.
struct Interop {
    create_memory_objects: unsafe extern "system" fn(i32, *mut u32),
    delete_memory_objects: unsafe extern "system" fn(i32, *const u32),
    import_memory_fd: unsafe extern "system" fn(u32, u64, u32, i32),
    tex_storage_mem_2d: unsafe extern "system" fn(u32, i32, u32, i32, i32, u32, u64),
    device_uuid: [u8; 16],
}
/// GL_HANDLE_TYPE_OPAQUE_FD_EXT
const HANDLE_TYPE_OPAQUE_FD: u32 = 0x9586;
/// GL_DEVICE_UUID_EXT
const DEVICE_UUID: u32 = 0x9597;
/// A texture of the window's renderer imported into a canvas's context: its
/// memory object, the GL texture over it, and a framebuffer to blit into.
struct SharedTarget {
    key: u64,
    memory: u32,
    texture: glow::Texture,
    framebuffer: glow::Framebuffer,
}
impl Gl {
    /// A new context on the thread's display (made on first use; its
    /// error is remembered).
    pub fn make() -> Result<Rc<Gl>, String> {
        let started = std::time::Instant::now();
        let first = DISPLAY.with(|cell| cell.borrow().is_none());
        let made = Display::shared().and_then(Self::new).map(Rc::new);
        if first && std::env::var_os("OLO_WEB_TIMING").is_some() {
            match &made {
                Ok(gl) => eprintln!("webgl: {} (ES{}) in {:.1} ms", gl.renderer(), if gl.es3 { 3 } else { 2 }, started.elapsed().as_secs_f64() * 1e3),
                Err(e) => eprintln!("webgl: unavailable: {e}"),
            }
        }
        made
    }
    fn new(display: Rc<Display>) -> Result<Self, String> {
        // SAFETY: an EGL context on this thread's display, made current
        // here before GL is loaded through it.
        unsafe {
            let d = &display;
            // Robust buffer access first (reads past a buffer answer zero
            // instead of faulting, as WebGL requires), then without.
            let robust = [khronos_egl::CONTEXT_CLIENT_VERSION, 3, CONTEXT_OPENGL_ROBUST_ACCESS_EXT, 1, khronos_egl::NONE];
            let (context, es3) = match d
                .egl
                .create_context(d.display, d.config, None, &robust)
                .or_else(|_| d.egl.create_context(d.display, d.config, None, &[khronos_egl::CONTEXT_CLIENT_VERSION, 3, khronos_egl::NONE]))
            {
                Ok(c) => (c, true),
                Err(_) => (d.egl.create_context(d.display, d.config, None, &[khronos_egl::CONTEXT_CLIENT_VERSION, 2, khronos_egl::NONE]).map_err(|e| format!("EGL context: {e}"))?, false),
            };
            d.egl.make_current(d.display, None, None, Some(context)).map_err(|e| format!("EGL current: {e}"))?;
            CURRENT.with(|c| c.set(context.as_ptr() as usize));
            let gl = glow::Context::from_loader_function(|s| d.egl.get_proc_address(s).map_or(std::ptr::null(), |p| p as *const std::ffi::c_void));
            let interop = Interop::load(d, &gl);
            Ok(Self { display, context, gl, es3, interop })
        }
    }
    /// Make this context current (another canvas's could be).
    pub fn current(&self) {
        let handle = self.context.as_ptr() as usize;
        if CURRENT.with(|c| c.get()) != handle {
            let d = &self.display;
            let _ = d.egl.make_current(d.display, None, None, Some(self.context));
            CURRENT.with(|c| c.set(handle));
        }
    }
    /// The EGL context handle, for telling contexts apart.
    pub fn handle(&self) -> usize {
        self.context.as_ptr() as usize
    }
    pub fn renderer(&self) -> String {
        self.current();
        // SAFETY: GL strings from a current context.
        unsafe { self.gl.get_parameter_string(glow::RENDERER) }
    }
}
impl Interop {
    /// The extension's entry points and the device UUID, if the context has
    /// GL_EXT_memory_object_fd.
    unsafe fn load(d: &Display, gl: &glow::Context) -> Option<Self> {
        let extensions = gl.supported_extensions();
        if !extensions.contains("GL_EXT_memory_object") || !extensions.contains("GL_EXT_memory_object_fd") {
            return None;
        }
        let get = |name: &str| d.egl.get_proc_address(name).map(|p| p as *const std::ffi::c_void).filter(|p| !p.is_null());
        // SAFETY: each pointer is the named GL entry point, called with its
        // C signature.
        unsafe {
            let create_memory_objects = std::mem::transmute::<*const std::ffi::c_void, unsafe extern "system" fn(i32, *mut u32)>(get("glCreateMemoryObjectsEXT")?);
            let delete_memory_objects = std::mem::transmute::<*const std::ffi::c_void, unsafe extern "system" fn(i32, *const u32)>(get("glDeleteMemoryObjectsEXT")?);
            let import_memory_fd = std::mem::transmute::<*const std::ffi::c_void, unsafe extern "system" fn(u32, u64, u32, i32)>(get("glImportMemoryFdEXT")?);
            let tex_storage_mem_2d = std::mem::transmute::<*const std::ffi::c_void, unsafe extern "system" fn(u32, i32, u32, i32, i32, u32, u64)>(get("glTexStorageMem2DEXT")?);
            let get_unsigned_bytes = std::mem::transmute::<*const std::ffi::c_void, unsafe extern "system" fn(u32, *mut u8)>(get("glGetUnsignedBytevEXT")?);
            let mut device_uuid = [0u8; 16];
            get_unsigned_bytes(DEVICE_UUID, device_uuid.as_mut_ptr());
            Some(Self { create_memory_objects, delete_memory_objects, import_memory_fd, tex_storage_mem_2d, device_uuid })
        }
    }
}
impl Gl {
    /// The GPU this context draws on, if it can import shared memory.
    pub fn interop_uuid(&self) -> Option<[u8; 16]> {
        self.interop.as_ref().map(|i| i.device_uuid)
    }
}
impl Drop for Gl {
    fn drop(&mut self) {
        let d = &self.display;
        if CURRENT.with(|c| c.get()) == self.context.as_ptr() as usize {
            let _ = d.egl.make_current(d.display, None, None, None);
            CURRENT.with(|c| c.set(0));
        }
        let _ = d.egl.destroy_context(d.display, self.context);
    }
}
/// One argument from the script, as the natives pass it.
pub enum Arg {
    Null,
    Num(f64),
    Str(String),
    /// A typed array or buffer: bytes with the element kind they were given as.
    Bytes(Vec<u8>, Kind),
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    U8,
    I8,
    U16,
    I16,
    U32,
    I32,
    F32,
    Raw,
}
impl Arg {
    fn num(&self) -> f64 {
        if let Arg::Num(n) = self { *n } else { 0. }
    }
    fn i(&self) -> i32 {
        self.num() as i32
    }
    fn u(&self) -> u32 {
        self.num().max(0.) as u32
    }
    fn f(&self) -> f32 {
        self.num() as f32
    }
    fn bytes(&self) -> Option<&[u8]> {
        if let Arg::Bytes(b, _) = self { Some(b) } else { None }
    }
    fn floats(&self) -> Vec<f32> {
        match self {
            Arg::Bytes(b, Kind::F32) => b.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect(),
            Arg::Num(n) => vec![*n as f32],
            _ => Vec::new(),
        }
    }
    fn ints(&self) -> Vec<i32> {
        match self {
            Arg::Bytes(b, Kind::I32 | Kind::U32) => b.chunks_exact(4).map(|c| i32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect(),
            Arg::Bytes(b, Kind::F32) => b.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]) as i32).collect(),
            Arg::Num(n) => vec![*n as i32],
            _ => Vec::new(),
        }
    }
}
/// What an operation answers.
pub enum Out {
    None,
    Num(f64),
    Bool(bool),
    Str(String),
    Floats(Vec<f32>),
    Ints(Vec<i32>),
    Bytes(Vec<u8>),
}
/// A canvas's WebGL context: its framebuffer and object tables.
pub struct Context {
    gl: Rc<Gl>,
    width: u32,
    height: u32,
    framebuffer: glow::Framebuffer,
    color: glow::Texture,
    depth: glow::Renderbuffer,
    buffers: Vec<Option<glow::Buffer>>,
    textures: Vec<Option<glow::Texture>>,
    programs: Vec<Option<glow::Program>>,
    shaders: Vec<Option<glow::Shader>>,
    /// Each shader's type, by its index in `shaders`.
    shader_types: Vec<u32>,
    framebuffers: Vec<Option<glow::Framebuffer>>,
    renderbuffers: Vec<Option<glow::Renderbuffer>>,
    vaos: Vec<Option<glow::VertexArray>>,
    uniforms: Vec<glow::UniformLocation>,
    /// The framebuffer bound by the script (None: the canvas's own).
    bound_framebuffer: Option<usize>,
    /// The ARRAY_BUFFER bound, which vertexAttribPointer captures.
    array_buffer: Option<usize>,
    /// Vertex state of the default vertex array and of each VAO.
    default_vertex: VertexState,
    vao_vertex: Vec<VertexState>,
    bound_vao: Option<usize>,
    /// An error WebGL would raise that GL did not, for getError.
    error: u32,
    pub premultiplied_alpha: bool,
    pub preserve_drawing_buffer: bool,
    /// Whether the drawing buffer has an alpha channel the page sees
    /// (`alpha: false`: the canvas is opaque whatever was drawn).
    pub alpha: bool,
    /// Drawn since last read back.
    pub dirty: bool,
    pub touched: bool,
    /// The last read-back, good while nothing was drawn since.
    last: Option<Arc<olo_web::image::Image>>,
    /// Pixel buffers for reading back without waiting: this frame's
    /// readback goes to one while the other, the previous frame's, is
    /// mapped (one frame of latency, no stall).
    pbos: [Option<glow::Buffer>; 2],
    /// Which buffer holds a readback not yet taken, and its size.
    pending: Option<(usize, u32, u32)>,
    unpack_flip_y: bool,
    unpack_premultiply: bool,
    /// Textures of the window's renderer this canvas's frames are blitted
    /// into (a ring: the window shows one while the next is drawn), and the
    /// next to draw into; empty until given (`provide_shared`).
    shared: Vec<SharedTarget>,
    shared_next: usize,
    /// The size the ring was asked for, so it is asked once.
    shared_asked: Option<(u32, u32)>,
    /// Shared textures this canvas let go of (resized), for the window to
    /// let go of too.
    shared_released: Vec<u64>,
    /// The canvas shows where the window can't show a shared texture (under
    /// a transform, in a group): read back instead.
    shared_refused: bool,
}
impl Context {
    pub fn new(gl: Rc<Gl>, width: u32, height: u32, premultiplied_alpha: bool, preserve_drawing_buffer: bool, alpha: bool) -> Result<Self, String> {
        gl.current();
        let (width, height) = (width.clamp(1, 8192), height.clamp(1, 8192));
        // SAFETY: GL calls on the thread's current context, with objects made here.
        unsafe {
            let g = &gl.gl;
            let framebuffer = g.create_framebuffer()?;
            let color = g.create_texture()?;
            let depth = g.create_renderbuffer()?;
            let mut this = Self {
                gl: gl.clone(),
                width,
                height,
                framebuffer,
                color,
                depth,
                buffers: Vec::new(),
                textures: Vec::new(),
                programs: Vec::new(),
                shaders: Vec::new(),
                shader_types: Vec::new(),
                framebuffers: Vec::new(),
                renderbuffers: Vec::new(),
                vaos: Vec::new(),
                uniforms: Vec::new(),
                bound_framebuffer: None,
                array_buffer: None,
                default_vertex: VertexState::default(),
                vao_vertex: Vec::new(),
                bound_vao: None,
                error: 0,
                premultiplied_alpha,
                preserve_drawing_buffer,
                alpha,
                dirty: false,
                touched: false,
                last: None,
                pbos: [None, None],
                pending: None,
                unpack_flip_y: false,
                unpack_premultiply: false,
                shared: Vec::new(),
                shared_next: 0,
                shared_asked: None,
                shared_released: Vec::new(),
                shared_refused: false,
            };
            this.attach();
            g.bind_framebuffer(glow::FRAMEBUFFER, Some(this.framebuffer));
            g.viewport(0, 0, width as i32, height as i32);
            g.clear_color(0., 0., 0., 0.);
            g.clear(glow::COLOR_BUFFER_BIT | glow::DEPTH_BUFFER_BIT | glow::STENCIL_BUFFER_BIT);
            Ok(this)
        }
    }
    /// (Re)make the canvas's colour and depth/stencil storage at its size.
    unsafe fn attach(&mut self) {
        let g = &self.gl.gl;
        unsafe {
            g.bind_texture(glow::TEXTURE_2D, Some(self.color));
            g.tex_image_2d(glow::TEXTURE_2D, 0, glow::RGBA as i32, self.width as i32, self.height as i32, 0, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelUnpackData::Slice(None));
            g.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
            g.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
            g.bind_texture(glow::TEXTURE_2D, None);
            g.bind_renderbuffer(glow::RENDERBUFFER, Some(self.depth));
            g.renderbuffer_storage(glow::RENDERBUFFER, if self.gl.es3 { glow::DEPTH24_STENCIL8 } else { glow::DEPTH_COMPONENT16 }, self.width as i32, self.height as i32);
            g.bind_renderbuffer(glow::RENDERBUFFER, None);
            g.bind_framebuffer(glow::FRAMEBUFFER, Some(self.framebuffer));
            g.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(self.color), 0);
            if self.gl.es3 {
                g.framebuffer_renderbuffer(glow::FRAMEBUFFER, glow::DEPTH_STENCIL_ATTACHMENT, glow::RENDERBUFFER, Some(self.depth));
            } else {
                g.framebuffer_renderbuffer(glow::FRAMEBUFFER, glow::DEPTH_ATTACHMENT, glow::RENDERBUFFER, Some(self.depth));
            }
        }
    }
    pub fn resize(&mut self, width: u32, height: u32) {
        let (width, height) = (width.clamp(1, 8192), height.clamp(1, 8192));
        if (width, height) == (self.width, self.height) {
            return;
        }
        self.pending = None;
        self.width = width;
        self.height = height;
        self.release_shared();
        self.gl.current();
        // SAFETY: as in `new`.
        unsafe {
            self.attach();
            let g = &self.gl.gl;
            g.bind_framebuffer(glow::FRAMEBUFFER, Some(self.framebuffer));
            g.clear_color(0., 0., 0., 0.);
            g.clear(glow::COLOR_BUFFER_BIT | glow::DEPTH_BUFFER_BIT | glow::STENCIL_BUFFER_BIT);
            self.rebind_framebuffer();
        }
        self.dirty = true;
    }
    pub fn readback_pending(&self) -> bool {
        self.pending.is_some()
    }
    /// How many shared textures, at what size, this canvas would draw its
    /// frames into: an animating canvas (drawn again after it was shown)
    /// whose context can import memory, not yet given or asked for them.
    pub fn wants_shared(&mut self) -> Option<(u32, u32, [u8; 16])> {
        let uuid = self.gl.interop_uuid()?;
        if self.shared_refused || !self.alpha || self.last.is_none() || !self.shared.is_empty() || self.shared_asked == Some((self.width, self.height)) {
            return None;
        }
        // Small canvases cost little to read back.
        if u64::from(self.width) * u64::from(self.height) < 128 * 128 {
            return None;
        }
        self.shared_asked = Some((self.width, self.height));
        Some((self.width, self.height, uuid))
    }
    /// The window's textures for this canvas's frames: imported, each with a
    /// framebuffer. Textures of another size than the canvas's now (it was
    /// resized meanwhile) are let go of at once.
    pub fn provide_shared(&mut self, textures: Vec<(u64, std::os::fd::OwnedFd, u64, u32, u32)>) {
        let Some(interop) = self.gl.interop.as_ref() else {
            self.shared_released.extend(textures.into_iter().map(|t| t.0));
            return;
        };
        self.gl.current();
        let mut made = Vec::new();
        for (key, fd, size, w, h) in textures {
            if (w, h) != (self.width, self.height) || !self.shared.is_empty() {
                self.shared_released.push(key);
                continue;
            }
            // SAFETY: GL_EXT_memory_object_fd on the current context; the fd
            // is given to GL, which owns it from the import on.
            unsafe {
                let g = &self.gl.gl;
                let mut memory = 0u32;
                (interop.create_memory_objects)(1, &mut memory);
                (interop.import_memory_fd)(memory, size, HANDLE_TYPE_OPAQUE_FD, std::os::fd::IntoRawFd::into_raw_fd(fd));
                let Ok(texture) = g.create_texture() else {
                    (interop.delete_memory_objects)(1, &memory);
                    self.shared_released.push(key);
                    continue;
                };
                g.bind_texture(glow::TEXTURE_2D, Some(texture));
                (interop.tex_storage_mem_2d)(glow::TEXTURE_2D, 1, glow::RGBA8, w as i32, h as i32, memory, 0);
                g.bind_texture(glow::TEXTURE_2D, None);
                let Ok(framebuffer) = g.create_framebuffer() else {
                    g.delete_texture(texture);
                    (interop.delete_memory_objects)(1, &memory);
                    self.shared_released.push(key);
                    continue;
                };
                g.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
                g.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(texture), 0);
                let complete = g.check_framebuffer_status(glow::FRAMEBUFFER) == glow::FRAMEBUFFER_COMPLETE;
                self.rebind_framebuffer();
                if !complete || g.get_error() != glow::NO_ERROR {
                    g.delete_framebuffer(framebuffer);
                    g.delete_texture(texture);
                    (interop.delete_memory_objects)(1, &memory);
                    self.shared_released.push(key);
                    continue;
                }
                made.push(SharedTarget { key, memory, texture, framebuffer });
            }
        }
        if made.len() >= 2 {
            self.shared = made;
            self.shared_next = 0;
            self.dirty = true;
        } else {
            // Too few to draw one while another shows: let go of them.
            let keys: Vec<u64> = made.iter().map(|t| t.key).collect();
            self.shared = made;
            self.release_shared();
            let _ = keys;
        }
    }
    /// Lets go of the shared textures (resized, refused, dropped).
    fn release_shared(&mut self) {
        if self.shared.is_empty() {
            return;
        }
        self.gl.current();
        let targets = std::mem::take(&mut self.shared);
        for t in targets {
            // SAFETY: this context's own objects.
            unsafe {
                let g = &self.gl.gl;
                g.delete_framebuffer(t.framebuffer);
                g.delete_texture(t.texture);
                if let Some(i) = self.gl.interop.as_ref() {
                    (i.delete_memory_objects)(1, &t.memory);
                }
            }
            self.shared_released.push(t.key);
        }
    }
    /// The shared textures let go of since asked, for the window.
    pub fn take_released_shared(&mut self) -> Vec<u64> {
        std::mem::take(&mut self.shared_released)
    }
    /// The window can't show this canvas from a shared texture: frames are
    /// read back from now on.
    pub fn refuse_shared(&mut self) {
        self.shared_refused = true;
        self.release_shared();
        self.dirty = true;
    }
    /// The frame into the next shared texture, the right way up: blitted on
    /// the GPU, finished before it is shown. Its key, or None without a ring.
    fn draw_shared(&mut self) -> Option<u64> {
        if self.shared.len() < 2 {
            return None;
        }
        let t = &self.shared[self.shared_next];
        let (key, target) = (t.key, t.framebuffer);
        self.gl.current();
        // SAFETY: blitting between this context's framebuffers.
        unsafe {
            let g = &self.gl.gl;
            let scissor = g.is_enabled(glow::SCISSOR_TEST);
            if scissor {
                g.disable(glow::SCISSOR_TEST);
            }
            g.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(self.framebuffer));
            g.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(target));
            let (w, h) = (self.width as i32, self.height as i32);
            g.blit_framebuffer(0, 0, w, h, 0, h, w, 0, glow::COLOR_BUFFER_BIT, glow::NEAREST);
            if scissor {
                g.enable(glow::SCISSOR_TEST);
            }
            self.rebind_framebuffer();
            // Done before the window samples it (another API, no semaphore).
            g.finish();
        }
        self.shared_next = (self.shared_next + 1) % self.shared.len();
        Some(key)
    }
    pub fn width(&self) -> u32 {
        self.width
    }
    pub fn height(&self) -> u32 {
        self.height
    }
    unsafe fn rebind_framebuffer(&self) {
        let g = &self.gl.gl;
        let fb = self.bound_framebuffer.and_then(|i| self.framebuffers.get(i).copied().flatten()).unwrap_or(self.framebuffer);
        unsafe { g.bind_framebuffer(glow::FRAMEBUFFER, Some(fb)) };
    }
    /// The drawing buffer's pixels as the page's image: read back, flipped
    /// (GL's rows go up), premultiplied unless already so.
    /// The drawing buffer's pixels, straight RGBA, rows from the top (GL's
    /// go up).
    fn read_back(&mut self) -> Vec<u8> {
        self.gl.current();
        let (w, h) = (self.width as usize, self.height as usize);
        let mut rgba = vec![0u8; w * h * 4];
        // SAFETY: reading the canvas's own framebuffer at its size.
        unsafe {
            let g = &self.gl.gl;
            g.bind_framebuffer(glow::FRAMEBUFFER, Some(self.framebuffer));
            g.read_pixels(0, 0, w as i32, h as i32, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelPackData::Slice(Some(&mut rgba)));
            self.rebind_framebuffer();
        }
        let row = w * 4;
        for y in 0..h / 2 {
            let (a, b) = rgba.split_at_mut((h - 1 - y) * row);
            a[y * row..y * row + row].swap_with_slice(&mut b[..row]);
        }
        rgba
    }
    /// Start reading the drawing buffer back into a pixel buffer (the GPU
    /// copies when its drawing is done; nothing waits here), and take the
    /// previous readback, flipped to rows from the top. None until the
    /// first readback lands.
    fn read_back_async(&mut self, dirty: bool) -> Option<(u32, u32, Vec<u8>)> {
        // The first frame is read synchronously, so a canvas never shows
        // blank; later frames come back one render late, without a stall.
        if !self.gl.es3 || self.last.is_none() {
            let (w, h) = (self.width, self.height);
            return Some((w, h, self.read_back()));
        }
        self.gl.current();
        let (w, h) = (self.width, self.height);
        let size = (w as usize) * (h as usize) * 4;
        let taken = self.pending.take();
        let next = taken.map_or(0, |(i, _, _)| 1 - i);
        // SAFETY: GL calls on the current context; the mapped range is
        // read within its mapping.
        unsafe {
            let g = &self.gl.gl;
            if dirty {
                let pbo = match self.pbos[next] {
                    Some(b) => b,
                    None => {
                        let b = g.create_buffer().ok()?;
                        self.pbos[next] = Some(b);
                        b
                    }
                };
                g.bind_buffer(glow::PIXEL_PACK_BUFFER, Some(pbo));
                g.buffer_data_size(glow::PIXEL_PACK_BUFFER, size as i32, glow::STREAM_READ);
                g.bind_framebuffer(glow::FRAMEBUFFER, Some(self.framebuffer));
                g.read_pixels(0, 0, w as i32, h as i32, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelPackData::BufferOffset(0));
                self.rebind_framebuffer();
                g.bind_buffer(glow::PIXEL_PACK_BUFFER, None);
                self.pending = Some((next, w, h));
            }
            let (index, pw, ph) = taken?;
            let pbo = self.pbos[index]?;
            let bytes = (pw as usize) * (ph as usize) * 4;
            g.bind_buffer(glow::PIXEL_PACK_BUFFER, Some(pbo));
            let mapped = g.map_buffer_range(glow::PIXEL_PACK_BUFFER, 0, bytes as i32, glow::MAP_READ_BIT);
            if mapped.is_null() {
                g.bind_buffer(glow::PIXEL_PACK_BUFFER, None);
                return None;
            }
            let source = std::slice::from_raw_parts(mapped as *const u8, bytes);
            let row = pw as usize * 4;
            let mut rgba = Vec::with_capacity(bytes);
            for y in (0..ph as usize).rev() {
                rgba.extend_from_slice(&source[y * row..y * row + row]);
            }
            g.unmap_buffer(glow::PIXEL_PACK_BUFFER);
            g.bind_buffer(glow::PIXEL_PACK_BUFFER, None);
            Some((pw, ph, rgba))
        }
    }
    /// The drawing buffer as the page's image: read back when drawn since
    /// the last time, premultiplied unless already so.
    pub fn image(&mut self) -> Option<Arc<olo_web::image::Image>> {
        if !self.dirty && self.pending.is_none() && self.last.is_some() {
            return self.last.clone();
        }
        let was_dirty = self.dirty;
        // An animating canvas with a ring: the frame stays on the GPU, the
        // image only names the texture it is in.
        if was_dirty && !self.shared.is_empty() {
            self.dirty = false;
            if let Some(key) = self.draw_shared() {
                self.last = Some(olo_web::image::Image::shared(self.width, self.height, key, self.premultiplied_alpha));
                return self.last.clone();
            }
        }
        if !was_dirty && !self.shared.is_empty() && self.last.as_ref().is_some_and(|l| l.shared_texture().is_some()) {
            return self.last.clone();
        }
        self.dirty = false;
        let timing = std::env::var_os("OLO_CANVAS_TIMING").map(|_| std::time::Instant::now());
        let read = self.read_back_async(was_dirty);
        if let Some(t) = timing {
            eprintln!("  canvas readback: {:.3} ms", t.elapsed().as_secs_f64() * 1000.);
        }
        let Some((w, h, mut rgba)) = read else {
            // The frame's readback is in flight: nothing new to show yet
            // (the next render takes it); keep whatever was shown.
            return self.last.clone();
        };
        if !self.alpha {
            for px in rgba.chunks_exact_mut(4) {
                px[3] = 255;
            }
        }
        // OLO_GL_DUMP=DIR: each canvas's latest drawing buffer as a PNG
        // (named by its size and context), for looking at.
        if let Some(dir) = std::env::var_os("OLO_GL_DUMP") {
            let path = std::path::Path::new(&dir).join(format!("canvas-{}-{w}x{h}.png", self.gl.handle() % 100_000));
            if let Ok(file) = std::fs::File::create(&path) {
                let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), w, h);
                encoder.set_color(png::ColorType::Rgba);
                encoder.set_depth(png::BitDepth::Eight);
                if let Ok(mut writer) = encoder.write_header() {
                    let _ = writer.write_image_data(&rgba);
                }
            }
        }
        let made = std::time::Instant::now();
        let mut image = if self.premultiplied_alpha {
            olo_web::image::Image::from_premultiplied(w, h, rgba, 1., Default::default(), None).ok()
        } else {
            olo_web::image::Image::from_rgba(w, h, rgba, Default::default(), None).ok()
        };
        if timing.is_some() {
            eprintln!("  canvas image: {:.3} ms ({w}x{h}, premultiplied {})", made.elapsed().as_secs_f64() * 1000., self.premultiplied_alpha);
        }
        // Drawn on again after it was shown: an animating surface, shown
        // through a layer of its own from now on.
        if self.last.is_some()
            && let Some(image) = &mut image
        {
            olo_web::image::Image::set_live(image);
        }
        self.last = image;
        self.last.clone()
    }
    /// The drawing buffer's pixels for another canvas's texture: straight
    /// RGBA, rows from the top.
    pub fn pixels(&mut self) -> Option<Vec<u8>> {
        Some(self.read_back())
    }
    fn table<T: Copy>(table: &[Option<T>], arg: &Arg) -> Option<T> {
        if let Arg::Num(n) = arg { table.get(*n as usize).copied().flatten() } else { None }
    }
    fn push<T>(table: &mut Vec<Option<T>>, value: T) -> Out {
        table.push(Some(value));
        Out::Num((table.len() - 1) as f64)
    }
    /// One call. Unknown or malformed calls do nothing.
    #[allow(clippy::too_many_lines)]
    pub fn op(&mut self, name: &str, a: &[Arg]) -> Out {
        self.gl.current();
        if TRACE.get_or_init(|| std::env::var_os("OLO_GL_TRACE").is_some()).to_owned() {
            let args: Vec<String> = a
                .iter()
                .map(|x| match x {
                    Arg::Null => "null".into(),
                    Arg::Num(n) => format!("{n}"),
                    Arg::Str(t) => format!("{:?}", t.chars().take(40).collect::<String>()),
                    Arg::Bytes(b, k) => format!("{k:?}[{}]", b.len()),
                })
                .collect();
            eprintln!("gl#{} {name}({})", self.width ^ (self.height << 16), args.join(", "));
        }
        let arg = |i: usize| a.get(i).unwrap_or(&Arg::Null);
        // SAFETY: every call below is a GL call on the thread's current
        // context with values from the script, which GL validates (bad
        // enums and objects set the context's error, as WebGL specifies).
        let gl = self.gl.clone();
        unsafe {
            let g = &gl.gl;
            match name {
                "createBuffer" => return Self::push(&mut self.buffers, g.create_buffer().unwrap_or(glow::NativeBuffer(std::num::NonZeroU32::new(1).unwrap()))),
                "createTexture" => return g.create_texture().map_or(Out::None, |t| Self::push(&mut self.textures, t)),
                "createProgram" => return g.create_program().map_or(Out::None, |p| Self::push(&mut self.programs, p)),
                "createShader" => {
                    return g.create_shader(arg(0).u()).map_or(Out::None, |s| {
                        self.shader_types.push(arg(0).u());
                        Self::push(&mut self.shaders, s)
                    });
                }
                "createFramebuffer" => return g.create_framebuffer().map_or(Out::None, |f| Self::push(&mut self.framebuffers, f)),
                "createRenderbuffer" => return g.create_renderbuffer().map_or(Out::None, |r| Self::push(&mut self.renderbuffers, r)),
                "createVertexArray" => {
                    return g.create_vertex_array().map_or(Out::None, |v| {
                        self.vao_vertex.push(VertexState::default());
                        Self::push(&mut self.vaos, v)
                    });
                }
                "deleteBuffer" => {
                    if let Arg::Num(i) = arg(0)
                        && let Some(slot) = self.buffers.get_mut(*i as usize)
                        && let Some(b) = slot.take()
                    {
                        g.delete_buffer(b);
                        let i = *i as usize;
                        if self.array_buffer == Some(i) {
                            self.array_buffer = None;
                        }
                        for state in std::iter::once(&mut self.default_vertex).chain(self.vao_vertex.iter_mut()) {
                            if state.element_buffer == Some(i) {
                                state.element_buffer = None;
                            }
                            for a in &mut state.attribs {
                                if a.buffer == Some(i) {
                                    a.buffer = None;
                                }
                            }
                        }
                    }
                }
                "deleteTexture" => {
                    if let Arg::Num(i) = arg(0)
                        && let Some(slot) = self.textures.get_mut(*i as usize)
                        && let Some(t) = slot.take()
                    {
                        g.delete_texture(t);
                    }
                }
                "deleteProgram" => {
                    if let Arg::Num(i) = arg(0)
                        && let Some(slot) = self.programs.get_mut(*i as usize)
                        && let Some(p) = slot.take()
                    {
                        g.delete_program(p);
                    }
                }
                "deleteShader" => {
                    if let Arg::Num(i) = arg(0)
                        && let Some(slot) = self.shaders.get_mut(*i as usize)
                        && let Some(s) = slot.take()
                    {
                        g.delete_shader(s);
                    }
                }
                "deleteFramebuffer" => {
                    if let Arg::Num(i) = arg(0)
                        && let Some(slot) = self.framebuffers.get_mut(*i as usize)
                        && let Some(f) = slot.take()
                    {
                        g.delete_framebuffer(f);
                    }
                }
                "deleteRenderbuffer" => {
                    if let Arg::Num(i) = arg(0)
                        && let Some(slot) = self.renderbuffers.get_mut(*i as usize)
                        && let Some(r) = slot.take()
                    {
                        g.delete_renderbuffer(r);
                    }
                }
                "deleteVertexArray" => {
                    if let Arg::Num(i) = arg(0)
                        && let Some(slot) = self.vaos.get_mut(*i as usize)
                        && let Some(v) = slot.take()
                    {
                        g.delete_vertex_array(v);
                    }
                }
                "bindBuffer" => {
                    let index = if let Arg::Num(i) = arg(1) { Some(*i as usize).filter(|i| self.buffers.get(*i).is_some_and(Option::is_some)) } else { None };
                    match arg(0).u() {
                        glow::ARRAY_BUFFER => self.array_buffer = index,
                        glow::ELEMENT_ARRAY_BUFFER => self.vertex_state().element_buffer = index,
                        _ => {}
                    }
                    g.bind_buffer(arg(0).u(), Self::table(&self.buffers, arg(1)));
                }
                "bindTexture" => g.bind_texture(arg(0).u(), Self::table(&self.textures, arg(1))),
                "bindFramebuffer" => {
                    self.bound_framebuffer = if let Arg::Num(i) = arg(1) { Some(*i as usize) } else { None };
                    let fb = Self::table(&self.framebuffers, arg(1)).unwrap_or(self.framebuffer);
                    g.bind_framebuffer(arg(0).u(), Some(fb));
                }
                "bindRenderbuffer" => g.bind_renderbuffer(arg(0).u(), Self::table(&self.renderbuffers, arg(1))),
                "bindVertexArray" => {
                    self.bound_vao = if let Arg::Num(i) = arg(0) { Some(*i as usize).filter(|i| self.vaos.get(*i).is_some_and(Option::is_some)) } else { None };
                    g.bind_vertex_array(Self::table(&self.vaos, arg(0)));
                }
                "bufferData" => {
                    // (target, data or size, usage)
                    match arg(1) {
                        Arg::Bytes(bytes, _) => g.buffer_data_u8_slice(arg(0).u(), bytes, arg(2).u()),
                        Arg::Num(size) => g.buffer_data_size(arg(0).u(), *size as i32, arg(2).u()),
                        _ => {}
                    }
                }
                "bufferSubData" => {
                    if let Arg::Bytes(bytes, _) = arg(2) {
                        g.buffer_sub_data_u8_slice(arg(0).u(), arg(1).i(), bytes);
                    }
                }
                "shaderSource" => {
                    if let (Some(s), Arg::Str(src)) = (Self::table(&self.shaders, arg(0)), arg(1)) {
                        g.shader_source(s, src);
                    }
                }
                "compileShader" => {
                    if let Some(s) = Self::table(&self.shaders, arg(0)) {
                        g.compile_shader(s);
                    }
                }
                "getShaderParameter" => {
                    if let Some(s) = Self::table(&self.shaders, arg(0)) {
                        return match arg(1).u() {
                            glow::COMPILE_STATUS => Out::Bool(g.get_shader_compile_status(s)),
                            glow::DELETE_STATUS => Out::Bool(false),
                            glow::SHADER_TYPE => Out::Num(f64::from(self.shader_types.get(arg(0).u() as usize).copied().unwrap_or(0))),
                            _ => Out::None,
                        };
                    }
                }
                "getShaderInfoLog" => {
                    if let Some(s) = Self::table(&self.shaders, arg(0)) {
                        return Out::Str(g.get_shader_info_log(s));
                    }
                }
                "attachShader" => {
                    if let (Some(p), Some(s)) = (Self::table(&self.programs, arg(0)), Self::table(&self.shaders, arg(1))) {
                        g.attach_shader(p, s);
                    }
                }
                "detachShader" => {
                    if let (Some(p), Some(s)) = (Self::table(&self.programs, arg(0)), Self::table(&self.shaders, arg(1))) {
                        g.detach_shader(p, s);
                    }
                }
                "linkProgram" => {
                    if let Some(p) = Self::table(&self.programs, arg(0)) {
                        g.link_program(p);
                    }
                }
                "useProgram" => g.use_program(Self::table(&self.programs, arg(0))),
                "validateProgram" => {
                    if let Some(p) = Self::table(&self.programs, arg(0)) {
                        g.validate_program(p);
                    }
                }
                "getProgramParameter" => {
                    if let Some(p) = Self::table(&self.programs, arg(0)) {
                        return match arg(1).u() {
                            glow::LINK_STATUS => Out::Bool(g.get_program_link_status(p)),
                            glow::VALIDATE_STATUS => Out::Bool(true),
                            glow::DELETE_STATUS => Out::Bool(false),
                            glow::ACTIVE_ATTRIBUTES => Out::Num(f64::from(g.get_active_attributes(p))),
                            glow::ACTIVE_UNIFORMS => Out::Num(f64::from(g.get_active_uniforms(p))),
                            glow::ATTACHED_SHADERS => Out::Num(f64::from(g.get_program_parameter_i32(p, glow::ATTACHED_SHADERS))),
                            glow::ACTIVE_UNIFORM_BLOCKS => Out::Num(f64::from(g.get_program_parameter_i32(p, glow::ACTIVE_UNIFORM_BLOCKS))),
                            _ => Out::None,
                        };
                    }
                }
                "getProgramInfoLog" => {
                    if let Some(p) = Self::table(&self.programs, arg(0)) {
                        return Out::Str(g.get_program_info_log(p));
                    }
                }
                "getActiveAttrib" | "getActiveUniform" => {
                    if let Some(p) = Self::table(&self.programs, arg(0)) {
                        let info = if name == "getActiveAttrib" {
                            g.get_active_attribute(p, arg(1).u()).map(|i| (i.name, i.size, i.atype))
                        } else {
                            g.get_active_uniform(p, arg(1).u()).map(|i| (i.name, i.size, i.utype))
                        };
                        return match info {
                            Some((name, size, kind)) => Out::Str(format!("{name}\u{1}{size}\u{1}{kind}")),
                            None => Out::None,
                        };
                    }
                }
                "getAttribLocation" => {
                    if let (Some(p), Arg::Str(n)) = (Self::table(&self.programs, arg(0)), arg(1)) {
                        return Out::Num(g.get_attrib_location(p, n).map_or(-1., f64::from));
                    }
                    return Out::Num(-1.);
                }
                "bindAttribLocation" => {
                    if let (Some(p), Arg::Str(n)) = (Self::table(&self.programs, arg(0)), arg(2)) {
                        g.bind_attrib_location(p, arg(1).u(), n);
                    }
                }
                "getUniformLocation" => {
                    if let (Some(p), Arg::Str(n)) = (Self::table(&self.programs, arg(0)), arg(1))
                        && let Some(location) = g.get_uniform_location(p, n)
                    {
                        self.uniforms.push(location);
                        return Out::Num((self.uniforms.len() - 1) as f64);
                    }
                    return Out::None;
                }
                "getUniformBlockIndex" => {
                    if let (Some(p), Arg::Str(n)) = (Self::table(&self.programs, arg(0)), arg(1)) {
                        return Out::Num(g.get_uniform_block_index(p, n).map_or(-1., f64::from));
                    }
                }
                "uniformBlockBinding" => {
                    if let Some(p) = Self::table(&self.programs, arg(0)) {
                        g.uniform_block_binding(p, arg(1).u(), arg(2).u());
                    }
                }
                "bindBufferBase" => g.bind_buffer_base(arg(0).u(), arg(1).u(), Self::table(&self.buffers, arg(2))),
                "bindBufferRange" => g.bind_buffer_range(arg(0).u(), arg(1).u(), Self::table(&self.buffers, arg(2)), arg(3).i(), arg(4).i()),
                "uniform1f" | "uniform2f" | "uniform3f" | "uniform4f" | "uniform1i" | "uniform2i" | "uniform3i" | "uniform4i" | "uniform1fv" | "uniform2fv" | "uniform3fv" | "uniform4fv"
                | "uniform1iv" | "uniform2iv" | "uniform3iv" | "uniform4iv" | "uniform1ui" | "uniform2ui" | "uniform3ui" | "uniform4ui" | "uniform1uiv" | "uniform2uiv" | "uniform3uiv"
                | "uniform4uiv" => {
                    let Arg::Num(loc) = arg(0) else { return Out::None };
                    let Some(location) = self.uniforms.get(*loc as usize) else { return Out::None };
                    let count: usize = name[7..8].parse().unwrap_or(1);
                    let vector = name.ends_with('v');
                    let suffix = &name[8..];
                    if suffix.starts_with('u') {
                        let values: Vec<u32> = if vector { arg(1).ints().into_iter().map(|v| v as u32).collect() } else { (1..=count).map(|i| arg(i).u()).collect() };
                        match count {
                            1 => g.uniform_1_u32_slice(Some(location), &values),
                            2 => g.uniform_2_u32_slice(Some(location), &values),
                            3 => g.uniform_3_u32_slice(Some(location), &values),
                            _ => g.uniform_4_u32_slice(Some(location), &values),
                        }
                    } else if suffix.starts_with('i') {
                        let values: Vec<i32> = if vector { arg(1).ints() } else { (1..=count).map(|i| arg(i).i()).collect() };
                        match count {
                            1 => g.uniform_1_i32_slice(Some(location), &values),
                            2 => g.uniform_2_i32_slice(Some(location), &values),
                            3 => g.uniform_3_i32_slice(Some(location), &values),
                            _ => g.uniform_4_i32_slice(Some(location), &values),
                        }
                    } else {
                        let values: Vec<f32> = if vector { arg(1).floats() } else { (1..=count).map(|i| arg(i).f()).collect() };
                        match count {
                            1 => g.uniform_1_f32_slice(Some(location), &values),
                            2 => g.uniform_2_f32_slice(Some(location), &values),
                            3 => g.uniform_3_f32_slice(Some(location), &values),
                            _ => g.uniform_4_f32_slice(Some(location), &values),
                        }
                    }
                }
                "uniformMatrix2fv" | "uniformMatrix3fv" | "uniformMatrix4fv" => {
                    let Arg::Num(loc) = arg(0) else { return Out::None };
                    let Some(location) = self.uniforms.get(*loc as usize) else { return Out::None };
                    let transpose = arg(1).num() != 0.;
                    let values = arg(2).floats();
                    match name {
                        "uniformMatrix2fv" => g.uniform_matrix_2_f32_slice(Some(location), transpose, &values),
                        "uniformMatrix3fv" => g.uniform_matrix_3_f32_slice(Some(location), transpose, &values),
                        _ => g.uniform_matrix_4_f32_slice(Some(location), transpose, &values),
                    }
                }
                "enableVertexAttribArray" => {
                    let i = arg(0).u() as usize;
                    if i < ATTRIBS {
                        self.vertex_state().attribs[i].enabled = true;
                        g.enable_vertex_attrib_array(i as u32);
                    }
                }
                "disableVertexAttribArray" => {
                    let i = arg(0).u() as usize;
                    if i < ATTRIBS {
                        self.vertex_state().attribs[i].enabled = false;
                        g.disable_vertex_attrib_array(i as u32);
                    }
                }
                "vertexAttribPointer" | "vertexAttribIPointer" => {
                    let i = arg(0).u() as usize;
                    if i >= ATTRIBS {
                        return Out::None;
                    }
                    let Some(buffer) = self.array_buffer else {
                        // No buffer: WebGL refuses (a client pointer to GL).
                        self.error = glow::INVALID_OPERATION;
                        return Out::None;
                    };
                    self.vertex_state().attribs[i].buffer = Some(buffer);
                    if name == "vertexAttribPointer" {
                        g.vertex_attrib_pointer_f32(i as u32, arg(1).i(), arg(2).u(), arg(3).num() != 0., arg(4).i(), arg(5).i());
                    } else {
                        g.vertex_attrib_pointer_i32(i as u32, arg(1).i(), arg(2).u(), arg(3).i(), arg(4).i());
                    }
                }
                "vertexAttribDivisor" => g.vertex_attrib_divisor(arg(0).u(), arg(1).u()),
                "vertexAttrib1f" => g.vertex_attrib_1_f32(arg(0).u(), arg(1).f()),
                "vertexAttrib2f" => g.vertex_attrib_2_f32(arg(0).u(), arg(1).f(), arg(2).f()),
                "vertexAttrib3f" => g.vertex_attrib_3_f32(arg(0).u(), arg(1).f(), arg(2).f(), arg(3).f()),
                "vertexAttrib4f" => g.vertex_attrib_4_f32(arg(0).u(), arg(1).f(), arg(2).f(), arg(3).f(), arg(4).f()),
                "vertexAttrib1fv" | "vertexAttrib2fv" | "vertexAttrib3fv" | "vertexAttrib4fv" => {
                    let v = arg(1).floats();
                    let at = |i: usize| v.get(i).copied().unwrap_or(0.);
                    match name {
                        "vertexAttrib1fv" => g.vertex_attrib_1_f32(arg(0).u(), at(0)),
                        "vertexAttrib2fv" => g.vertex_attrib_2_f32(arg(0).u(), at(0), at(1)),
                        "vertexAttrib3fv" => g.vertex_attrib_3_f32(arg(0).u(), at(0), at(1), at(2)),
                        _ => g.vertex_attrib_4_f32(arg(0).u(), at(0), at(1), at(2), at(3)),
                    }
                }
                "drawArrays" => {
                    if self.drawable(false) {
                        g.draw_arrays(arg(0).u(), arg(1).i(), arg(2).i());
                        self.touch();
                    }
                }
                "drawElements" => {
                    if self.drawable(true) {
                        g.draw_elements(arg(0).u(), arg(1).i(), arg(2).u(), arg(3).i());
                        self.touch();
                    }
                }
                "drawArraysInstanced" => {
                    if self.drawable(false) {
                        g.draw_arrays_instanced(arg(0).u(), arg(1).i(), arg(2).i(), arg(3).i());
                        self.touch();
                    }
                }
                "drawElementsInstanced" => {
                    if self.drawable(true) {
                        g.draw_elements_instanced(arg(0).u(), arg(1).i(), arg(2).u(), arg(3).i(), arg(4).i());
                        self.touch();
                    }
                }
                "clear" => {
                    g.clear(arg(0).u());
                    self.touch();
                }
                "clearColor" => g.clear_color(arg(0).f(), arg(1).f(), arg(2).f(), arg(3).f()),
                "clearDepth" => g.clear_depth_f32(arg(0).f()),
                "clearStencil" => g.clear_stencil(arg(0).i()),
                "colorMask" => g.color_mask(arg(0).num() != 0., arg(1).num() != 0., arg(2).num() != 0., arg(3).num() != 0.),
                "depthMask" => g.depth_mask(arg(0).num() != 0.),
                "stencilMask" => g.stencil_mask(arg(0).u()),
                "stencilMaskSeparate" => g.stencil_mask_separate(arg(0).u(), arg(1).u()),
                "stencilFunc" => g.stencil_func(arg(0).u(), arg(1).i(), arg(2).u()),
                "stencilFuncSeparate" => g.stencil_func_separate(arg(0).u(), arg(1).u(), arg(2).i(), arg(3).u()),
                "stencilOp" => g.stencil_op(arg(0).u(), arg(1).u(), arg(2).u()),
                "stencilOpSeparate" => g.stencil_op_separate(arg(0).u(), arg(1).u(), arg(2).u(), arg(3).u()),
                "viewport" => g.viewport(arg(0).i(), arg(1).i(), arg(2).i(), arg(3).i()),
                "scissor" => g.scissor(arg(0).i(), arg(1).i(), arg(2).i(), arg(3).i()),
                "enable" => g.enable(arg(0).u()),
                "disable" => g.disable(arg(0).u()),
                "blendFunc" => g.blend_func(arg(0).u(), arg(1).u()),
                "blendFuncSeparate" => g.blend_func_separate(arg(0).u(), arg(1).u(), arg(2).u(), arg(3).u()),
                "blendEquation" => g.blend_equation(arg(0).u()),
                "blendEquationSeparate" => g.blend_equation_separate(arg(0).u(), arg(1).u()),
                "blendColor" => g.blend_color(arg(0).f(), arg(1).f(), arg(2).f(), arg(3).f()),
                "depthFunc" => g.depth_func(arg(0).u()),
                "depthRange" => g.depth_range_f32(arg(0).f(), arg(1).f()),
                "cullFace" => g.cull_face(arg(0).u()),
                "frontFace" => g.front_face(arg(0).u()),
                "lineWidth" => g.line_width(arg(0).f()),
                "polygonOffset" => g.polygon_offset(arg(0).f(), arg(1).f()),
                "sampleCoverage" => g.sample_coverage(arg(0).f(), arg(1).num() != 0.),
                "hint" => g.hint(arg(0).u(), arg(1).u()),
                "activeTexture" => g.active_texture(arg(0).u()),
                "texParameteri" => g.tex_parameter_i32(arg(0).u(), arg(1).u(), arg(2).i()),
                "texParameterf" => g.tex_parameter_f32(arg(0).u(), arg(1).u(), arg(2).f()),
                "pixelStorei" => match arg(0).u() {
                    0x9240 => self.unpack_flip_y = arg(1).num() != 0.,
                    0x9241 => self.unpack_premultiply = arg(1).num() != 0.,
                    0x9243 => {}
                    p => g.pixel_store_i32(p, arg(1).i()),
                },
                "texImage2D" => {
                    // (target, level, internalformat, width, height, border, format, type, pixels or null)
                    let (target, level, internal, width, height, format, kind) = (arg(0).u(), arg(1).i(), arg(2).i(), arg(3).i(), arg(4).i(), arg(6).u(), arg(7).u());
                    let pixels = arg(8).bytes().map(|b| self.unpack(b, width, height, format, kind));
                    g.tex_image_2d(target, level, internal, width, height, arg(5).i(), format, kind, glow::PixelUnpackData::Slice(pixels.as_deref()));
                }
                "texSubImage2D" => {
                    let (target, level, x, y, width, height, format, kind) = (arg(0).u(), arg(1).i(), arg(2).i(), arg(3).i(), arg(4).i(), arg(5).i(), arg(6).u(), arg(7).u());
                    if let Some(b) = arg(8).bytes() {
                        let pixels = self.unpack(b, width, height, format, kind);
                        g.tex_sub_image_2d(target, level, x, y, width, height, format, kind, glow::PixelUnpackData::Slice(Some(&pixels)));
                    }
                }
                "generateMipmap" => g.generate_mipmap(arg(0).u()),
                "copyTexImage2D" => g.copy_tex_image_2d(arg(0).u(), arg(1).i(), arg(2).u(), arg(3).i(), arg(4).i(), arg(5).i(), arg(6).i(), arg(7).i()),
                "copyTexSubImage2D" => g.copy_tex_sub_image_2d(arg(0).u(), arg(1).i(), arg(2).i(), arg(3).i(), arg(4).i(), arg(5).i(), arg(6).i(), arg(7).i()),
                "framebufferTexture2D" => g.framebuffer_texture_2d(arg(0).u(), arg(1).u(), arg(2).u(), Self::table(&self.textures, arg(3)), arg(4).i()),
                "framebufferRenderbuffer" => g.framebuffer_renderbuffer(arg(0).u(), arg(1).u(), arg(2).u(), Self::table(&self.renderbuffers, arg(3))),
                "renderbufferStorage" => g.renderbuffer_storage(arg(0).u(), arg(1).u(), arg(2).i(), arg(3).i()),
                "renderbufferStorageMultisample" => g.renderbuffer_storage_multisample(arg(0).u(), arg(1).i(), arg(2).u(), arg(3).i(), arg(4).i()),
                "checkFramebufferStatus" => return Out::Num(f64::from(g.check_framebuffer_status(arg(0).u()))),
                "drawBuffers" => {
                    let b: Vec<u32> = arg(0).ints().into_iter().map(|v| v as u32).collect();
                    g.draw_buffers(&b);
                }
                "readBuffer" => g.read_buffer(arg(0).u()),
                "blitFramebuffer" => g.blit_framebuffer(arg(0).i(), arg(1).i(), arg(2).i(), arg(3).i(), arg(4).i(), arg(5).i(), arg(6).i(), arg(7).i(), arg(8).u(), arg(9).u()),
                "readPixels" => {
                    let (x, y, w, h, format, kind) = (arg(0).i(), arg(1).i(), arg(2).i(), arg(3).i(), arg(4).u(), arg(5).u());
                    let size = (w.max(0) as usize) * (h.max(0) as usize) * 4;
                    let mut out = vec![0u8; size];
                    g.read_pixels(x, y, w, h, format, kind, glow::PixelPackData::Slice(Some(&mut out)));
                    return Out::Bytes(out);
                }
                "getError" => {
                    let gl_error = g.get_error();
                    let error = if self.error != 0 { std::mem::take(&mut self.error) } else { gl_error };
                    return Out::Num(f64::from(error));
                }
                "finish" => g.finish(),
                "flush" => g.flush(),
                "getParameter" => {
                    let p = arg(0).u();
                    return match p {
                        glow::VENDOR | glow::RENDERER | glow::VERSION | glow::SHADING_LANGUAGE_VERSION => Out::Str(g.get_parameter_string(p)),
                        glow::VIEWPORT | glow::SCISSOR_BOX | glow::MAX_VIEWPORT_DIMS => {
                            let mut v = [0i32; 4];
                            g.get_parameter_i32_slice(p, &mut v);
                            Out::Ints(v.to_vec())
                        }
                        glow::COLOR_CLEAR_VALUE | glow::BLEND_COLOR | glow::ALIASED_LINE_WIDTH_RANGE | glow::ALIASED_POINT_SIZE_RANGE | glow::DEPTH_RANGE => {
                            let mut v = [0f32; 4];
                            g.get_parameter_f32_slice(p, &mut v);
                            Out::Floats(v.to_vec())
                        }
                        glow::DEPTH_TEST
                        | glow::BLEND
                        | glow::CULL_FACE
                        | glow::SCISSOR_TEST
                        | glow::STENCIL_TEST
                        | glow::DITHER
                        | glow::POLYGON_OFFSET_FILL
                        | glow::SAMPLE_COVERAGE
                        | glow::DEPTH_WRITEMASK => Out::Bool(g.get_parameter_i32(p) != 0),
                        _ => Out::Num(f64::from(g.get_parameter_i32(p))),
                    };
                }
                "isEnabled" => return Out::Bool(g.is_enabled(arg(0).u())),
                "getShaderPrecisionFormat" => {
                    return match g.get_shader_precision_format(arg(0).u(), arg(1).u()) {
                        Some(f) => Out::Ints(vec![f.range_min, f.range_max, f.precision]),
                        None => Out::Ints(vec![127, 127, 23]),
                    };
                }
                _ => {}
            }
        }
        Out::None
    }
    /// Pixel data as the script gave it, flipped and premultiplied as
    /// UNPACK_FLIP_Y_WEBGL and UNPACK_PREMULTIPLY_ALPHA_WEBGL ask.
    fn unpack(&self, bytes: &[u8], width: i32, height: i32, format: u32, kind: u32) -> Vec<u8> {
        let mut out = bytes.to_vec();
        let (w, h) = (width.max(0) as usize, height.max(0) as usize);
        let bpp = match (format, kind) {
            (glow::RGBA, glow::UNSIGNED_BYTE) => 4,
            (glow::RGB, glow::UNSIGNED_BYTE) => 3,
            (glow::LUMINANCE_ALPHA, glow::UNSIGNED_BYTE) => 2,
            (glow::ALPHA | glow::LUMINANCE, glow::UNSIGNED_BYTE) => 1,
            (glow::RGBA, glow::FLOAT) => 16,
            (_, glow::UNSIGNED_SHORT_4_4_4_4 | glow::UNSIGNED_SHORT_5_5_5_1 | glow::UNSIGNED_SHORT_5_6_5) => 2,
            _ => 0,
        };
        if bpp == 0 || w == 0 || h == 0 || out.len() < w * h * bpp {
            return out;
        }
        if self.unpack_flip_y {
            let row = w * bpp;
            for y in 0..h / 2 {
                let (a, b) = out.split_at_mut((h - 1 - y) * row);
                a[y * row..y * row + row].swap_with_slice(&mut b[..row]);
            }
        }
        if self.unpack_premultiply && format == glow::RGBA && kind == glow::UNSIGNED_BYTE {
            for px in out.chunks_exact_mut(4) {
                let a = u32::from(px[3]);
                for c in &mut px[..3] {
                    *c = ((u32::from(*c) * a + 127) / 255) as u8;
                }
            }
        }
        out
    }
    /// The vertex state draws read: the bound VAO's, else the default's.
    fn vertex_state(&mut self) -> &mut VertexState {
        match self.bound_vao {
            Some(i) if i < self.vao_vertex.len() => &mut self.vao_vertex[i],
            _ => &mut self.default_vertex,
        }
    }
    /// Whether a draw may go to GL: every enabled attribute reads a live
    /// buffer, and an indexed draw has its index buffer. Otherwise WebGL
    /// raises INVALID_OPERATION and draws nothing.
    fn drawable(&mut self, indexed: bool) -> bool {
        let buffers = &self.buffers;
        let live = |b: Option<usize>| b.is_some_and(|i| buffers.get(i).is_some_and(Option::is_some));
        let state = match self.bound_vao {
            Some(i) if i < self.vao_vertex.len() => &self.vao_vertex[i],
            _ => &self.default_vertex,
        };
        let ok = state.attribs.iter().all(|a| !a.enabled || live(a.buffer)) && (!indexed || live(state.element_buffer));
        if !ok {
            self.error = glow::INVALID_OPERATION;
        }
        ok
    }
    fn touch(&mut self) {
        self.dirty = true;
        self.touched = true;
    }
}
impl Drop for Context {
    fn drop(&mut self) {
        self.release_shared();
        self.gl.current();
        // SAFETY: deleting this context's own objects on the current context.
        unsafe {
            let g = &self.gl.gl;
            for pbo in self.pbos.iter().flatten() {
                g.delete_buffer(*pbo);
            }
            g.delete_framebuffer(self.framebuffer);
            g.delete_texture(self.color);
            g.delete_renderbuffer(self.depth);
            for b in self.buffers.drain(..).flatten() {
                g.delete_buffer(b);
            }
            for t in self.textures.drain(..).flatten() {
                g.delete_texture(t);
            }
            for p in self.programs.drain(..).flatten() {
                g.delete_program(p);
            }
            for s in self.shaders.drain(..).flatten() {
                g.delete_shader(s);
            }
            for f in self.framebuffers.drain(..).flatten() {
                g.delete_framebuffer(f);
            }
            for r in self.renderbuffers.drain(..).flatten() {
                g.delete_renderbuffer(r);
            }
            for v in self.vaos.drain(..).flatten() {
                g.delete_vertex_array(v);
            }
        }
    }
}
