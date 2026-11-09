// WebGL for the page's scripts: WebGLRenderingContext (and the WebGL2
// surface) over the engine's GL (__olo.glOp), one framebuffer per canvas.
// Objects are small wrappers around an index the engine's tables hold.
(function (global) {
  'use strict';
  const N = global.__olo;
  // Olo's own functions (what the engine calls, and these files share):
  // on a holder page scripts can't reach (a worker's are its global's).
  const H = N.hidden || global;
  const C = {
    DEPTH_BUFFER_BIT: 256, STENCIL_BUFFER_BIT: 1024, COLOR_BUFFER_BIT: 16384, POINTS: 0, LINES: 1, LINE_LOOP: 2, LINE_STRIP: 3,
    TRIANGLES: 4, TRIANGLE_STRIP: 5, TRIANGLE_FAN: 6, ZERO: 0, ONE: 1, SRC_COLOR: 768, ONE_MINUS_SRC_COLOR: 769, SRC_ALPHA: 770,
    ONE_MINUS_SRC_ALPHA: 771, DST_ALPHA: 772, ONE_MINUS_DST_ALPHA: 773, DST_COLOR: 774, ONE_MINUS_DST_COLOR: 775, SRC_ALPHA_SATURATE: 776, FUNC_ADD: 32774, BLEND_EQUATION: 32777,
    BLEND_EQUATION_RGB: 32777, BLEND_EQUATION_ALPHA: 34877, FUNC_SUBTRACT: 32778, FUNC_REVERSE_SUBTRACT: 32779, BLEND_DST_RGB: 32968, BLEND_SRC_RGB: 32969, BLEND_DST_ALPHA: 32970, BLEND_SRC_ALPHA: 32971,
    CONSTANT_COLOR: 32769, ONE_MINUS_CONSTANT_COLOR: 32770, CONSTANT_ALPHA: 32771, ONE_MINUS_CONSTANT_ALPHA: 32772, BLEND_COLOR: 32773, ARRAY_BUFFER: 34962, ELEMENT_ARRAY_BUFFER: 34963, ARRAY_BUFFER_BINDING: 34964,
    ELEMENT_ARRAY_BUFFER_BINDING: 34965, STREAM_DRAW: 35040, STATIC_DRAW: 35044, DYNAMIC_DRAW: 35048, BUFFER_SIZE: 34660, BUFFER_USAGE: 34661, CURRENT_VERTEX_ATTRIB: 34342, FRONT: 1028,
    BACK: 1029, FRONT_AND_BACK: 1032, TEXTURE_2D: 3553, CULL_FACE: 2884, BLEND: 3042, DITHER: 3024, STENCIL_TEST: 2960, DEPTH_TEST: 2929,
    SCISSOR_TEST: 3089, POLYGON_OFFSET_FILL: 32823, SAMPLE_ALPHA_TO_COVERAGE: 32926, SAMPLE_COVERAGE: 32928, NO_ERROR: 0, INVALID_ENUM: 1280, INVALID_VALUE: 1281, INVALID_OPERATION: 1282,
    OUT_OF_MEMORY: 1285, CW: 2304, CCW: 2305, LINE_WIDTH: 2849, ALIASED_POINT_SIZE_RANGE: 33901, ALIASED_LINE_WIDTH_RANGE: 33902, CULL_FACE_MODE: 2885, FRONT_FACE: 2886,
    DEPTH_RANGE: 2928, DEPTH_WRITEMASK: 2930, DEPTH_CLEAR_VALUE: 2931, DEPTH_FUNC: 2932, STENCIL_CLEAR_VALUE: 2961, STENCIL_FUNC: 2962, STENCIL_FAIL: 2964, STENCIL_PASS_DEPTH_FAIL: 2965,
    STENCIL_PASS_DEPTH_PASS: 2966, STENCIL_REF: 2967, STENCIL_VALUE_MASK: 2963, STENCIL_WRITEMASK: 2968, STENCIL_BACK_FUNC: 34816, STENCIL_BACK_FAIL: 34817, STENCIL_BACK_PASS_DEPTH_FAIL: 34818, STENCIL_BACK_PASS_DEPTH_PASS: 34819,
    STENCIL_BACK_REF: 36003, STENCIL_BACK_VALUE_MASK: 36004, STENCIL_BACK_WRITEMASK: 36005, VIEWPORT: 2978, SCISSOR_BOX: 3088, COLOR_CLEAR_VALUE: 3106, COLOR_WRITEMASK: 3107, UNPACK_ALIGNMENT: 3317,
    PACK_ALIGNMENT: 3333, MAX_TEXTURE_SIZE: 3379, MAX_VIEWPORT_DIMS: 3386, SUBPIXEL_BITS: 3408, RED_BITS: 3410, GREEN_BITS: 3411, BLUE_BITS: 3412, ALPHA_BITS: 3413,
    DEPTH_BITS: 3414, STENCIL_BITS: 3415, POLYGON_OFFSET_UNITS: 10752, POLYGON_OFFSET_FACTOR: 32824, TEXTURE_BINDING_2D: 32873, SAMPLE_BUFFERS: 32936, SAMPLES: 32937, SAMPLE_COVERAGE_VALUE: 32938,
    SAMPLE_COVERAGE_INVERT: 32939, COMPRESSED_TEXTURE_FORMATS: 34467, DONT_CARE: 4352, FASTEST: 4353, NICEST: 4354, GENERATE_MIPMAP_HINT: 33170, BYTE: 5120, UNSIGNED_BYTE: 5121,
    SHORT: 5122, UNSIGNED_SHORT: 5123, INT: 5124, UNSIGNED_INT: 5125, FLOAT: 5126, DEPTH_COMPONENT: 6402, ALPHA: 6406, RGB: 6407,
    RGBA: 6408, LUMINANCE: 6409, LUMINANCE_ALPHA: 6410, UNSIGNED_SHORT_4_4_4_4: 32819, UNSIGNED_SHORT_5_5_5_1: 32820, UNSIGNED_SHORT_5_6_5: 33635, FRAGMENT_SHADER: 35632, VERTEX_SHADER: 35633,
    MAX_VERTEX_ATTRIBS: 34921, MAX_VERTEX_UNIFORM_VECTORS: 36347, MAX_VARYING_VECTORS: 36348, MAX_COMBINED_TEXTURE_IMAGE_UNITS: 35661, MAX_VERTEX_TEXTURE_IMAGE_UNITS: 35660, MAX_TEXTURE_IMAGE_UNITS: 34930, MAX_FRAGMENT_UNIFORM_VECTORS: 36349, SHADER_TYPE: 35663,
    DELETE_STATUS: 35712, LINK_STATUS: 35714, VALIDATE_STATUS: 35715, ATTACHED_SHADERS: 35717, ACTIVE_UNIFORMS: 35718, ACTIVE_ATTRIBUTES: 35721, SHADING_LANGUAGE_VERSION: 35724, CURRENT_PROGRAM: 35725,
    NEVER: 512, LESS: 513, EQUAL: 514, LEQUAL: 515, GREATER: 516, NOTEQUAL: 517, GEQUAL: 518, ALWAYS: 519,
    KEEP: 7680, REPLACE: 7681, INCR: 7682, DECR: 7683, INVERT: 5386, INCR_WRAP: 34055, DECR_WRAP: 34056, VENDOR: 7936,
    RENDERER: 7937, VERSION: 7938, NEAREST: 9728, LINEAR: 9729, NEAREST_MIPMAP_NEAREST: 9984, LINEAR_MIPMAP_NEAREST: 9985, NEAREST_MIPMAP_LINEAR: 9986, LINEAR_MIPMAP_LINEAR: 9987,
    TEXTURE_MAG_FILTER: 10240, TEXTURE_MIN_FILTER: 10241, TEXTURE_WRAP_S: 10242, TEXTURE_WRAP_T: 10243, TEXTURE: 5890, TEXTURE_CUBE_MAP: 34067, TEXTURE_BINDING_CUBE_MAP: 34068, TEXTURE_CUBE_MAP_POSITIVE_X: 34069,
    TEXTURE_CUBE_MAP_NEGATIVE_X: 34070, TEXTURE_CUBE_MAP_POSITIVE_Y: 34071, TEXTURE_CUBE_MAP_NEGATIVE_Y: 34072, TEXTURE_CUBE_MAP_POSITIVE_Z: 34073, TEXTURE_CUBE_MAP_NEGATIVE_Z: 34074, MAX_CUBE_MAP_TEXTURE_SIZE: 34076, TEXTURE0: 33984, TEXTURE1: 33985,
    TEXTURE2: 33986, TEXTURE3: 33987, TEXTURE4: 33988, TEXTURE5: 33989, TEXTURE6: 33990, TEXTURE7: 33991, TEXTURE8: 33992, TEXTURE9: 33993,
    TEXTURE10: 33994, TEXTURE11: 33995, TEXTURE12: 33996, TEXTURE13: 33997, TEXTURE14: 33998, TEXTURE15: 33999, TEXTURE16: 34000, TEXTURE17: 34001,
    TEXTURE18: 34002, TEXTURE19: 34003, TEXTURE20: 34004, TEXTURE21: 34005, TEXTURE22: 34006, TEXTURE23: 34007, TEXTURE24: 34008, TEXTURE25: 34009,
    TEXTURE26: 34010, TEXTURE27: 34011, TEXTURE28: 34012, TEXTURE29: 34013, TEXTURE30: 34014, TEXTURE31: 34015, ACTIVE_TEXTURE: 34016, REPEAT: 10497,
    CLAMP_TO_EDGE: 33071, MIRRORED_REPEAT: 33648, FLOAT_VEC2: 35664, FLOAT_VEC3: 35665, FLOAT_VEC4: 35666, INT_VEC2: 35667, INT_VEC3: 35668, INT_VEC4: 35669,
    BOOL: 35670, BOOL_VEC2: 35671, BOOL_VEC3: 35672, BOOL_VEC4: 35673, FLOAT_MAT2: 35674, FLOAT_MAT3: 35675, FLOAT_MAT4: 35676, SAMPLER_2D: 35678,
    SAMPLER_CUBE: 35680, VERTEX_ATTRIB_ARRAY_ENABLED: 34338, VERTEX_ATTRIB_ARRAY_SIZE: 34339, VERTEX_ATTRIB_ARRAY_STRIDE: 34340, VERTEX_ATTRIB_ARRAY_TYPE: 34341, VERTEX_ATTRIB_ARRAY_NORMALIZED: 34922, VERTEX_ATTRIB_ARRAY_POINTER: 34373, VERTEX_ATTRIB_ARRAY_BUFFER_BINDING: 34975,
    IMPLEMENTATION_COLOR_READ_TYPE: 35738, IMPLEMENTATION_COLOR_READ_FORMAT: 35739, COMPILE_STATUS: 35713, LOW_FLOAT: 36336, MEDIUM_FLOAT: 36337, HIGH_FLOAT: 36338, LOW_INT: 36339, MEDIUM_INT: 36340,
    HIGH_INT: 36341, FRAMEBUFFER: 36160, RENDERBUFFER: 36161, RGBA4: 32854, RGB5_A1: 32855, RGB565: 36194, DEPTH_COMPONENT16: 33189, STENCIL_INDEX8: 36168,
    DEPTH_STENCIL: 34041, RENDERBUFFER_WIDTH: 36162, RENDERBUFFER_HEIGHT: 36163, RENDERBUFFER_INTERNAL_FORMAT: 36164, RENDERBUFFER_RED_SIZE: 36176, RENDERBUFFER_GREEN_SIZE: 36177, RENDERBUFFER_BLUE_SIZE: 36178, RENDERBUFFER_ALPHA_SIZE: 36179,
    RENDERBUFFER_DEPTH_SIZE: 36180, RENDERBUFFER_STENCIL_SIZE: 36181, FRAMEBUFFER_ATTACHMENT_OBJECT_TYPE: 36048, FRAMEBUFFER_ATTACHMENT_OBJECT_NAME: 36049, FRAMEBUFFER_ATTACHMENT_TEXTURE_LEVEL: 36050, FRAMEBUFFER_ATTACHMENT_TEXTURE_CUBE_MAP_FACE: 36051, COLOR_ATTACHMENT0: 36064, DEPTH_ATTACHMENT: 36096,
    STENCIL_ATTACHMENT: 36128, DEPTH_STENCIL_ATTACHMENT: 33306, NONE: 0, FRAMEBUFFER_COMPLETE: 36053, FRAMEBUFFER_INCOMPLETE_ATTACHMENT: 36054, FRAMEBUFFER_INCOMPLETE_MISSING_ATTACHMENT: 36055, FRAMEBUFFER_INCOMPLETE_DIMENSIONS: 36057, FRAMEBUFFER_UNSUPPORTED: 36061,
    FRAMEBUFFER_BINDING: 36006, RENDERBUFFER_BINDING: 36007, MAX_RENDERBUFFER_SIZE: 34024, INVALID_FRAMEBUFFER_OPERATION: 1286, UNPACK_FLIP_Y_WEBGL: 37440, UNPACK_PREMULTIPLY_ALPHA_WEBGL: 37441, CONTEXT_LOST_WEBGL: 37442, UNPACK_COLORSPACE_CONVERSION_WEBGL: 37443,
    BROWSER_DEFAULT_WEBGL: 37444, READ_BUFFER: 3074, UNPACK_ROW_LENGTH: 3314, UNPACK_SKIP_ROWS: 3315, UNPACK_SKIP_PIXELS: 3316, PACK_ROW_LENGTH: 3330, PACK_SKIP_ROWS: 3331, PACK_SKIP_PIXELS: 3332,
    COLOR: 6144, DEPTH: 6145, STENCIL: 6146, RED: 6403, RGB8: 32849, RGBA8: 32856, RGB10_A2: 32857, TEXTURE_BINDING_3D: 32874,
    UNPACK_SKIP_IMAGES: 32877, UNPACK_IMAGE_HEIGHT: 32878, TEXTURE_3D: 32879, TEXTURE_WRAP_R: 32882, MAX_3D_TEXTURE_SIZE: 32883, UNSIGNED_INT_2_10_10_10_REV: 33640, MAX_ELEMENTS_VERTICES: 33000, MAX_ELEMENTS_INDICES: 33001,
    TEXTURE_MIN_LOD: 33082, TEXTURE_MAX_LOD: 33083, TEXTURE_BASE_LEVEL: 33084, TEXTURE_MAX_LEVEL: 33085, MIN: 32775, MAX: 32776, DEPTH_COMPONENT24: 33190, MAX_TEXTURE_LOD_BIAS: 34045,
    TEXTURE_COMPARE_MODE: 34892, TEXTURE_COMPARE_FUNC: 34893, CURRENT_QUERY: 34917, QUERY_RESULT: 34918, QUERY_RESULT_AVAILABLE: 34919, STREAM_READ: 35041, STREAM_COPY: 35042, STATIC_READ: 35045,
    STATIC_COPY: 35046, DYNAMIC_READ: 35049, DYNAMIC_COPY: 35050, MAX_DRAW_BUFFERS: 34852, DRAW_BUFFER0: 34853, DRAW_BUFFER1: 34854, DRAW_BUFFER2: 34855, DRAW_BUFFER3: 34856,
    DRAW_BUFFER4: 34857, DRAW_BUFFER5: 34858, DRAW_BUFFER6: 34859, DRAW_BUFFER7: 34860, DRAW_BUFFER8: 34861, DRAW_BUFFER9: 34862, DRAW_BUFFER10: 34863, DRAW_BUFFER11: 34864,
    DRAW_BUFFER12: 34865, DRAW_BUFFER13: 34866, DRAW_BUFFER14: 34867, DRAW_BUFFER15: 34868, MAX_FRAGMENT_UNIFORM_COMPONENTS: 35657, MAX_VERTEX_UNIFORM_COMPONENTS: 35658, SAMPLER_3D: 35679, SAMPLER_2D_SHADOW: 35682,
    FRAGMENT_SHADER_DERIVATIVE_HINT: 35723, PIXEL_PACK_BUFFER: 35051, PIXEL_UNPACK_BUFFER: 35052, PIXEL_PACK_BUFFER_BINDING: 35053, PIXEL_UNPACK_BUFFER_BINDING: 35055, FLOAT_MAT2x3: 35685, FLOAT_MAT2x4: 35686, FLOAT_MAT3x2: 35687,
    FLOAT_MAT3x4: 35688, FLOAT_MAT4x2: 35689, FLOAT_MAT4x3: 35690, SRGB: 35904, SRGB8: 35905, SRGB8_ALPHA8: 35907, COMPARE_REF_TO_TEXTURE: 34894, RGBA32F: 34836,
    RGB32F: 34837, RGBA16F: 34842, RGB16F: 34843, VERTEX_ATTRIB_ARRAY_INTEGER: 35069, MAX_ARRAY_TEXTURE_LAYERS: 35071, MIN_PROGRAM_TEXEL_OFFSET: 35076, MAX_PROGRAM_TEXEL_OFFSET: 35077, MAX_VARYING_COMPONENTS: 35659,
    TEXTURE_2D_ARRAY: 35866, TEXTURE_BINDING_2D_ARRAY: 35869, R11F_G11F_B10F: 35898, UNSIGNED_INT_10F_11F_11F_REV: 35899, RGB9_E5: 35901, UNSIGNED_INT_5_9_9_9_REV: 35902, TRANSFORM_FEEDBACK_BUFFER_MODE: 35967, MAX_TRANSFORM_FEEDBACK_SEPARATE_COMPONENTS: 35968,
    TRANSFORM_FEEDBACK_VARYINGS: 35971, TRANSFORM_FEEDBACK_BUFFER_START: 35972, TRANSFORM_FEEDBACK_BUFFER_SIZE: 35973, TRANSFORM_FEEDBACK_PRIMITIVES_WRITTEN: 35976, RASTERIZER_DISCARD: 35977, MAX_TRANSFORM_FEEDBACK_INTERLEAVED_COMPONENTS: 35978, MAX_TRANSFORM_FEEDBACK_SEPARATE_ATTRIBS: 35979, INTERLEAVED_ATTRIBS: 35980,
    SEPARATE_ATTRIBS: 35981, TRANSFORM_FEEDBACK_BUFFER: 35982, TRANSFORM_FEEDBACK_BUFFER_BINDING: 35983, RGBA32UI: 36208, RGB32UI: 36209, RGBA16UI: 36214, RGB16UI: 36215, RGBA8UI: 36220,
    RGB8UI: 36221, RGBA32I: 36226, RGB32I: 36227, RGBA16I: 36232, RGB16I: 36233, RGBA8I: 36238, RGB8I: 36239, RED_INTEGER: 36244,
    RGB_INTEGER: 36248, RGBA_INTEGER: 36249, SAMPLER_2D_ARRAY: 36289, SAMPLER_2D_ARRAY_SHADOW: 36292, SAMPLER_CUBE_SHADOW: 36293, UNSIGNED_INT_VEC2: 36294, UNSIGNED_INT_VEC3: 36295, UNSIGNED_INT_VEC4: 36296,
    INT_SAMPLER_2D: 36298, INT_SAMPLER_3D: 36299, INT_SAMPLER_CUBE: 36300, INT_SAMPLER_2D_ARRAY: 36303, UNSIGNED_INT_SAMPLER_2D: 36306, UNSIGNED_INT_SAMPLER_3D: 36307, UNSIGNED_INT_SAMPLER_CUBE: 36308, UNSIGNED_INT_SAMPLER_2D_ARRAY: 36311,
    DEPTH_COMPONENT32F: 36012, DEPTH32F_STENCIL8: 36013, FLOAT_32_UNSIGNED_INT_24_8_REV: 36269, FRAMEBUFFER_ATTACHMENT_COLOR_ENCODING: 33296, FRAMEBUFFER_ATTACHMENT_COMPONENT_TYPE: 33297, FRAMEBUFFER_ATTACHMENT_RED_SIZE: 33298, FRAMEBUFFER_ATTACHMENT_GREEN_SIZE: 33299, FRAMEBUFFER_ATTACHMENT_BLUE_SIZE: 33300,
    FRAMEBUFFER_ATTACHMENT_ALPHA_SIZE: 33301, FRAMEBUFFER_ATTACHMENT_DEPTH_SIZE: 33302, FRAMEBUFFER_ATTACHMENT_STENCIL_SIZE: 33303, FRAMEBUFFER_DEFAULT: 33304, UNSIGNED_INT_24_8: 34042, DEPTH24_STENCIL8: 35056, UNSIGNED_NORMALIZED: 35863, DRAW_FRAMEBUFFER_BINDING: 36006,
    READ_FRAMEBUFFER: 36008, DRAW_FRAMEBUFFER: 36009, READ_FRAMEBUFFER_BINDING: 36010, RENDERBUFFER_SAMPLES: 36011, FRAMEBUFFER_ATTACHMENT_TEXTURE_LAYER: 36052, MAX_COLOR_ATTACHMENTS: 36063, COLOR_ATTACHMENT1: 36065, COLOR_ATTACHMENT2: 36066,
    COLOR_ATTACHMENT3: 36067, COLOR_ATTACHMENT4: 36068, COLOR_ATTACHMENT5: 36069, COLOR_ATTACHMENT6: 36070, COLOR_ATTACHMENT7: 36071, COLOR_ATTACHMENT8: 36072, COLOR_ATTACHMENT9: 36073, COLOR_ATTACHMENT10: 36074,
    COLOR_ATTACHMENT11: 36075, COLOR_ATTACHMENT12: 36076, COLOR_ATTACHMENT13: 36077, COLOR_ATTACHMENT14: 36078, COLOR_ATTACHMENT15: 36079, FRAMEBUFFER_INCOMPLETE_MULTISAMPLE: 36182, MAX_SAMPLES: 36183, HALF_FLOAT: 5131,
    RG: 33319, RG_INTEGER: 33320, R8: 33321, RG8: 33323, R16F: 33325, R32F: 33326, RG16F: 33327, RG32F: 33328,
    R8I: 33329, R8UI: 33330, R16I: 33331, R16UI: 33332, R32I: 33333, R32UI: 33334, RG8I: 33335, RG8UI: 33336,
    RG16I: 33337, RG16UI: 33338, RG32I: 33339, RG32UI: 33340, VERTEX_ARRAY_BINDING: 34229, R8_SNORM: 36756, RG8_SNORM: 36757, RGB8_SNORM: 36758,
    RGBA8_SNORM: 36759, SIGNED_NORMALIZED: 36764, COPY_READ_BUFFER: 36662, COPY_WRITE_BUFFER: 36663, COPY_READ_BUFFER_BINDING: 36662, COPY_WRITE_BUFFER_BINDING: 36663, UNIFORM_BUFFER: 35345, UNIFORM_BUFFER_BINDING: 35368,
    UNIFORM_BUFFER_START: 35369, UNIFORM_BUFFER_SIZE: 35370, MAX_VERTEX_UNIFORM_BLOCKS: 35371, MAX_FRAGMENT_UNIFORM_BLOCKS: 35373, MAX_COMBINED_UNIFORM_BLOCKS: 35374, MAX_UNIFORM_BUFFER_BINDINGS: 35375, MAX_UNIFORM_BLOCK_SIZE: 35376, MAX_COMBINED_VERTEX_UNIFORM_COMPONENTS: 35377,
    MAX_COMBINED_FRAGMENT_UNIFORM_COMPONENTS: 35379, UNIFORM_BUFFER_OFFSET_ALIGNMENT: 35380, ACTIVE_UNIFORM_BLOCKS: 35382, UNIFORM_TYPE: 35383, UNIFORM_SIZE: 35384, UNIFORM_BLOCK_INDEX: 35386, UNIFORM_OFFSET: 35387, UNIFORM_ARRAY_STRIDE: 35388,
    UNIFORM_MATRIX_STRIDE: 35389, UNIFORM_IS_ROW_MAJOR: 35390, UNIFORM_BLOCK_BINDING: 35391, UNIFORM_BLOCK_DATA_SIZE: 35392, UNIFORM_BLOCK_ACTIVE_UNIFORMS: 35394, UNIFORM_BLOCK_ACTIVE_UNIFORM_INDICES: 35395, UNIFORM_BLOCK_REFERENCED_BY_VERTEX_SHADER: 35396, UNIFORM_BLOCK_REFERENCED_BY_FRAGMENT_SHADER: 35398,
    INVALID_INDEX: 4294967295, MAX_VERTEX_OUTPUT_COMPONENTS: 37154, MAX_FRAGMENT_INPUT_COMPONENTS: 37157, MAX_SERVER_WAIT_TIMEOUT: 37137, OBJECT_TYPE: 37138, SYNC_CONDITION: 37139, SYNC_STATUS: 37140, SYNC_FLAGS: 37141,
    SYNC_FENCE: 37142, SYNC_GPU_COMMANDS_COMPLETE: 37143, UNSIGNALED: 37144, SIGNALED: 37145, ALREADY_SIGNALED: 37146, TIMEOUT_EXPIRED: 37147, CONDITION_SATISFIED: 37148, WAIT_FAILED: 37149,
    SYNC_FLUSH_COMMANDS_BIT: 1, VERTEX_ATTRIB_ARRAY_DIVISOR: 35070, ANY_SAMPLES_PASSED: 35887, ANY_SAMPLES_PASSED_CONSERVATIVE: 36202, SAMPLER_BINDING: 35097, RGB10_A2UI: 36975, INT_2_10_10_10_REV: 36255, TRANSFORM_FEEDBACK: 36386,
    TRANSFORM_FEEDBACK_PAUSED: 36387, TRANSFORM_FEEDBACK_ACTIVE: 36388, TRANSFORM_FEEDBACK_BINDING: 36389, TEXTURE_IMMUTABLE_FORMAT: 37167, MAX_ELEMENT_INDEX: 36203, TEXTURE_IMMUTABLE_LEVELS: 33503, TIMEOUT_IGNORED: -1, MAX_CLIENT_WAIT_TIMEOUT_WEBGL: 37447,
  };
  const kinds = ['WebGLBuffer', 'WebGLTexture', 'WebGLProgram', 'WebGLShader', 'WebGLFramebuffer', 'WebGLRenderbuffer', 'WebGLVertexArrayObject', 'WebGLUniformLocation', 'WebGLActiveInfo', 'WebGLShaderPrecisionFormat', 'WebGLQuery', 'WebGLSampler', 'WebGLSync', 'WebGLTransformFeedback'];
  const classes = {};
  const EXTENSIONS1 = ['ANGLE_instanced_arrays', 'EXT_blend_minmax', 'EXT_color_buffer_half_float', 'EXT_float_blend', 'EXT_frag_depth', 'EXT_texture_filter_anisotropic', 'KHR_parallel_shader_compile', 'OES_element_index_uint', 'OES_fbo_render_mipmap', 'OES_standard_derivatives', 'OES_texture_float', 'OES_texture_float_linear', 'OES_texture_half_float', 'OES_texture_half_float_linear', 'OES_vertex_array_object', 'WEBGL_color_buffer_float', 'WEBGL_debug_renderer_info', 'WEBGL_debug_shaders', 'WEBGL_depth_texture', 'WEBGL_lose_context'];
  const EXTENSIONS2 = ['EXT_color_buffer_float', 'EXT_color_buffer_half_float', 'EXT_float_blend', 'EXT_texture_filter_anisotropic', 'KHR_parallel_shader_compile', 'OES_texture_float_linear', 'WEBGL_debug_renderer_info', 'WEBGL_debug_shaders', 'WEBGL_lose_context'];
  // The GPU as Chrome names it, through ANGLE: "Google Inc. (Intel)" and
  // "ANGLE (Intel, Mesa Intel(R) Graphics (PTL), OpenGL ES 3.2)", from the
  // driver's own strings; asked once.
  let gpu = null;
  const angle = (ctx) => {
    if (gpu) return gpu;
    const ask = (p) => { try { const v = ctx._op('getParameter', [p]); return typeof v === 'string' ? v : ''; } catch (e) { return ''; } };
    const vendor = ask(0x1F00), renderer = ask(0x1F01), version = ask(0x1F02);
    if (!renderer) return ['Google Inc. (Google)', 'ANGLE (Google, Vulkan 1.3.0 (SwiftShader Device (Subzero) (0x0000C0DE)), SwiftShader driver)'];
    const who = /intel/i.test(vendor + ' ' + renderer) ? 'Intel' : /nvidia/i.test(vendor) ? 'NVIDIA' : /amd|ati\b|radeon/i.test(vendor + ' ' + renderer) ? 'AMD' : /mesa|x\.org|vmware/i.test(vendor) ? 'Mesa' : vendor.split(/\s/)[0] || 'Google';
    const es = /^OpenGL ES (\d+\.\d+)/.exec(version), desktop = /^(\d+\.\d+)/.exec(version);
    const api = es ? 'OpenGL ES ' + es[1] : desktop ? 'OpenGL ' + desktop[1] : version;
    gpu = ['Google Inc. (' + who + ')', 'ANGLE (' + who + ', ' + renderer + ', ' + api + ')'];
    return gpu;
  };
  for (const k of kinds) { const cls = class { constructor(id) { this._id = id; } }; Object.defineProperty(cls, 'name', { value: k }); classes[k] = cls; global[k] = cls; }
  global.WebGLVertexArrayObjectOES = classes.WebGLVertexArrayObject;
  const id = (o) => (o && typeof o === 'object' && '_id' in o ? o._id : null);
  // Typed data is handed over as the array itself; the engine reads its bytes.
  const data = (v) => (v == null ? null : v);
  // The WebGL surface. Each method sends its name and arguments.
  class WebGLRenderingContext {
    constructor(canvas, attributes, version) {
      this.canvas = canvas;
      this._attributes = Object.assign({ alpha: true, depth: true, stencil: false, antialias: true, premultipliedAlpha: true, preserveDrawingBuffer: false, powerPreference: 'default', failIfMajorPerformanceCaveat: false, desynchronized: false }, attributes || {});
      this._version = version;
      this._lost = false;
      this._extensions = {};
      Object.assign(this, C);
    }
    _op(name, args) { this._ops = (this._ops || 0) + 1; return N.glOp(this.canvas, name, args); }
    get drawingBufferWidth() { return this.canvas.width; }
    get drawingBufferHeight() { return this.canvas.height; }
    get drawingBufferColorSpace() { return 'srgb'; }
    getContextAttributes() { return Object.assign({}, this._attributes); }
    isContextLost() { return this._lost; }
    // Chrome's, in its order, of those Olo has (compressed textures not
    // yet: a page told of them would upload pictures Olo can't draw).
    getSupportedExtensions() { return (this._version === 2 ? EXTENSIONS2 : EXTENSIONS1).slice(); }
    getExtension(name) {
      name = String(name);
      if (this._extensions[name] !== undefined) return this._extensions[name];
      const ctx = this;
      let ext = null;
      switch (name) {
        case 'OES_vertex_array_object': ext = { VERTEX_ARRAY_BINDING_OES: 0x85B5, createVertexArrayOES: () => ctx.createVertexArray(), deleteVertexArrayOES: (v) => ctx.deleteVertexArray(v), isVertexArrayOES: (v) => ctx.isVertexArray(v), bindVertexArrayOES: (v) => ctx.bindVertexArray(v) }; break;
        case 'ANGLE_instanced_arrays': ext = { VERTEX_ATTRIB_ARRAY_DIVISOR_ANGLE: 0x88FE, drawArraysInstancedANGLE: (m, f, c, n) => ctx.drawArraysInstanced(m, f, c, n), drawElementsInstancedANGLE: (m, c, t, o, n) => ctx.drawElementsInstanced(m, c, t, o, n), vertexAttribDivisorANGLE: (i, d) => ctx.vertexAttribDivisor(i, d) }; break;
        case 'OES_standard_derivatives': ext = { FRAGMENT_SHADER_DERIVATIVE_HINT_OES: 0x8B8B }; break;
        case 'OES_element_index_uint': case 'OES_texture_float': case 'OES_texture_float_linear': case 'OES_texture_half_float_linear': case 'EXT_color_buffer_float': case 'WEBGL_color_buffer_float': case 'EXT_frag_depth': case 'EXT_shader_texture_lod': ext = {}; break;
        case 'OES_texture_half_float': ext = { HALF_FLOAT_OES: 0x8D61 }; break;
        case 'EXT_color_buffer_half_float': ext = { RGBA16F_EXT: 0x881A, RGB16F_EXT: 0x881B, FRAMEBUFFER_ATTACHMENT_COMPONENT_TYPE_EXT: 0x8211, UNSIGNED_NORMALIZED_EXT: 0x8C17 }; break;
        case 'EXT_blend_minmax': ext = { MIN_EXT: 0x8007, MAX_EXT: 0x8008 }; break;
        case 'EXT_texture_filter_anisotropic': ext = { TEXTURE_MAX_ANISOTROPY_EXT: 0x84FE, MAX_TEXTURE_MAX_ANISOTROPY_EXT: 0x84FF }; break;
        case 'WEBGL_depth_texture': ext = { UNSIGNED_INT_24_8_WEBGL: 0x84FA }; break;
        case 'WEBGL_debug_renderer_info': ext = { UNMASKED_VENDOR_WEBGL: 0x9245, UNMASKED_RENDERER_WEBGL: 0x9246 }; break;
        case 'WEBGL_debug_shaders': ext = { getTranslatedShaderSource: () => '' }; break;
        case 'KHR_parallel_shader_compile': ext = { COMPLETION_STATUS_KHR: 0x91B1 }; break;
        case 'EXT_float_blend': case 'OES_fbo_render_mipmap': case 'WEBGL_multi_draw': ext = {}; break;
        case 'EXT_disjoint_timer_query': case 'EXT_disjoint_timer_query_webgl2': ext = null; break;
        case 'WEBGL_lose_context': ext = { loseContext: () => { ctx._lost = true; }, restoreContext: () => { ctx._lost = false; } }; break;
        default: ext = null;
      }
      this._extensions[name] = ext;
      return ext;
    }
    getParameter(p) {
      switch (p) {
        case 0x1F00: return 'WebKit';
        case 0x1F01: return 'WebKit WebGL';
        case 0x1F02: return this._version === 2 ? 'WebGL 2.0 (OpenGL ES 3.0 Chromium)' : 'WebGL 1.0 (OpenGL ES 2.0 Chromium)';
        case 0x8B8C: return this._version === 2 ? 'WebGL GLSL ES 3.00 (OpenGL ES GLSL ES 3.0 Chromium)' : 'WebGL GLSL ES 1.0 (OpenGL ES GLSL ES 1.0 Chromium)';
        case 0x9245: return angle(this)[0];
        case 0x9246: return angle(this)[1];
        case 0x8B8D: return this._program || null;
        case 0x8894: return this._arrayBuffer || null;
        case 0x8895: return this._elementBuffer || null;
        case 0x8CA6: return this._framebuffer || null;
        case 0x8CA7: return this._renderbuffer || null;
        case 0x8069: return this._texture2d || null;
        case 0x85B5: return this._vao || null;
        case 0x9240: return !!this._flipY;
        case 0x9241: return !!this._premultiply;
        case 0x9243: return 0x9244;
        default: {
          const v = this._op('getParameter', [p]);
          if (v === null || v === undefined) return null;
          return v;
        }
      }
    }
    getError() { return this._op('getError', []) || 0; }
    getShaderPrecisionFormat(s, p) { const f = this._op('getShaderPrecisionFormat', [s, p]) || [127, 127, 23]; return { rangeMin: f[0], rangeMax: f[1], precision: f[2] }; }
    createBuffer() { const i = this._op('createBuffer', []); return i == null ? null : new classes.WebGLBuffer(i); }
    createTexture() { const i = this._op('createTexture', []); return i == null ? null : new classes.WebGLTexture(i); }
    createProgram() { const i = this._op('createProgram', []); return i == null ? null : new classes.WebGLProgram(i); }
    createShader(t) { const i = this._op('createShader', [t]); return i == null ? null : new classes.WebGLShader(i); }
    createFramebuffer() { const i = this._op('createFramebuffer', []); return i == null ? null : new classes.WebGLFramebuffer(i); }
    createRenderbuffer() { const i = this._op('createRenderbuffer', []); return i == null ? null : new classes.WebGLRenderbuffer(i); }
    createVertexArray() { const i = this._op('createVertexArray', []); return i == null ? null : new classes.WebGLVertexArrayObject(i); }
    createQuery() { return new classes.WebGLQuery(0); } createSampler() { return new classes.WebGLSampler(0); } createTransformFeedback() { return new classes.WebGLTransformFeedback(0); } fenceSync() { return new classes.WebGLSync(0); }
    deleteBuffer(b) { this._op('deleteBuffer', [id(b)]); } deleteTexture(t) { this._op('deleteTexture', [id(t)]); } deleteProgram(p) { this._op('deleteProgram', [id(p)]); } deleteShader(s) { this._op('deleteShader', [id(s)]); } deleteFramebuffer(f) { this._op('deleteFramebuffer', [id(f)]); } deleteRenderbuffer(r) { this._op('deleteRenderbuffer', [id(r)]); } deleteVertexArray(v) { this._op('deleteVertexArray', [id(v)]); }
    deleteQuery() {} deleteSampler() {} deleteSync() {} deleteTransformFeedback() {}
    isBuffer(b) { return b instanceof classes.WebGLBuffer; } isTexture(t) { return t instanceof classes.WebGLTexture; } isProgram(p) { return p instanceof classes.WebGLProgram; } isShader(s) { return s instanceof classes.WebGLShader; } isFramebuffer(f) { return f instanceof classes.WebGLFramebuffer; } isRenderbuffer(r) { return r instanceof classes.WebGLRenderbuffer; } isVertexArray(v) { return v instanceof classes.WebGLVertexArrayObject; } isEnabled(c) { return !!this._op('isEnabled', [c]); }
    bindBuffer(t, b) { if (t === 0x8892) this._arrayBuffer = b; else if (t === 0x8893) this._elementBuffer = b; this._op('bindBuffer', [t, id(b)]); }
    bindTexture(t, x) { if (t === 0x0DE1) this._texture2d = x; this._op('bindTexture', [t, id(x)]); }
    bindFramebuffer(t, f) { this._framebuffer = f; this._op('bindFramebuffer', [t, id(f)]); }
    bindRenderbuffer(t, r) { this._renderbuffer = r; this._op('bindRenderbuffer', [t, id(r)]); }
    bindVertexArray(v) { this._vao = v; this._op('bindVertexArray', [id(v)]); }
    bindSampler() {} bindTransformFeedback() {}
    bufferData(target, dataOrSize, usage) { this._op('bufferData', [target, typeof dataOrSize === 'number' ? dataOrSize : data(dataOrSize), usage]); }
    bufferSubData(target, offset, d) { this._op('bufferSubData', [target, offset, data(d)]); }
    getBufferParameter() { return 0; }
    shaderSource(s, src) { this._op('shaderSource', [id(s), String(src)]); } compileShader(s) { this._op('compileShader', [id(s)]); }
    getShaderParameter(s, p) { const v = this._op('getShaderParameter', [id(s), p]); return v == null ? null : v; }
    getShaderInfoLog(s) { return this._op('getShaderInfoLog', [id(s)]) || ''; } getShaderSource() { return ''; }
    attachShader(p, s) { this._op('attachShader', [id(p), id(s)]); } detachShader(p, s) { this._op('detachShader', [id(p), id(s)]); } linkProgram(p) { this._op('linkProgram', [id(p)]); } useProgram(p) { this._program = p; this._op('useProgram', [id(p)]); } validateProgram(p) { this._op('validateProgram', [id(p)]); }
    getProgramParameter(p, pname) { if (pname === 0x91B1) return true; const v = this._op('getProgramParameter', [id(p), pname]); return v == null ? null : v; }
    getProgramInfoLog(p) { return this._op('getProgramInfoLog', [id(p)]) || ''; }
    getAttachedShaders() { return []; }
    getActiveAttrib(p, i) { const v = this._op('getActiveAttrib', [id(p), i]); if (!v) return null; const [name, size, type] = v.split('\u0001'); return { name, size: Number(size), type: Number(type) }; }
    getActiveUniform(p, i) { const v = this._op('getActiveUniform', [id(p), i]); if (!v) return null; const [name, size, type] = v.split('\u0001'); return { name, size: Number(size), type: Number(type) }; }
    getAttribLocation(p, name) { const v = this._op('getAttribLocation', [id(p), String(name)]); return v == null ? -1 : v; }
    bindAttribLocation(p, i, name) { this._op('bindAttribLocation', [id(p), i, String(name)]); }
    getUniformLocation(p, name) { const v = this._op('getUniformLocation', [id(p), String(name)]); return v == null ? null : new classes.WebGLUniformLocation(v); }
    getUniform() { return null; }
    getUniformBlockIndex(p, name) { const v = this._op('getUniformBlockIndex', [id(p), String(name)]); return v == null || v < 0 ? 0xFFFFFFFF : v; }
    uniformBlockBinding(p, i, b) { this._op('uniformBlockBinding', [id(p), i, b]); }
    getActiveUniformBlockParameter() { return 0; } getActiveUniformBlockName() { return ''; } getUniformIndices() { return []; } getActiveUniforms() { return []; }
    bindBufferBase(t, i, b) { this._op('bindBufferBase', [t, i, id(b)]); } bindBufferRange(t, i, b, o, s) { this._op('bindBufferRange', [t, i, id(b), o, s]); }
    enableVertexAttribArray(i) { this._op('enableVertexAttribArray', [i]); } disableVertexAttribArray(i) { this._op('disableVertexAttribArray', [i]); }
    vertexAttribPointer(i, size, type, normalized, stride, offset) { this._op('vertexAttribPointer', [i, size, type, normalized ? 1 : 0, stride, offset]); }
    vertexAttribIPointer(i, size, type, stride, offset) { this._op('vertexAttribIPointer', [i, size, type, stride, offset]); }
    vertexAttribDivisor(i, d) { this._op('vertexAttribDivisor', [i, d]); }
    vertexAttrib1f(i, x) { this._op('vertexAttrib1f', [i, x]); } vertexAttrib2f(i, x, y) { this._op('vertexAttrib2f', [i, x, y]); } vertexAttrib3f(i, x, y, z) { this._op('vertexAttrib3f', [i, x, y, z]); } vertexAttrib4f(i, x, y, z, w) { this._op('vertexAttrib4f', [i, x, y, z, w]); }
    vertexAttrib1fv(i, v) { this._op('vertexAttrib1fv', [i, Float32Array.from(v)]); } vertexAttrib2fv(i, v) { this._op('vertexAttrib2fv', [i, Float32Array.from(v)]); } vertexAttrib3fv(i, v) { this._op('vertexAttrib3fv', [i, Float32Array.from(v)]); } vertexAttrib4fv(i, v) { this._op('vertexAttrib4fv', [i, Float32Array.from(v)]); }
    getVertexAttrib() { return null; } getVertexAttribOffset() { return 0; }
    drawArrays(mode, first, count) { this._draws = (this._draws || 0) + 1; this._op('drawArrays', [mode, first, count]); } drawElements(mode, count, type, offset) { this._draws = (this._draws || 0) + 1; this._op('drawElements', [mode, count, type, offset]); }
    drawArraysInstanced(mode, first, count, n) { this._op('drawArraysInstanced', [mode, first, count, n]); } drawElementsInstanced(mode, count, type, offset, n) { this._op('drawElementsInstanced', [mode, count, type, offset, n]); }
    drawRangeElements(mode, start, end, count, type, offset) { this._op('drawElements', [mode, count, type, offset]); }
    clear(mask) { this._op('clear', [mask]); } clearColor(r, g, b, a) { this._op('clearColor', [r, g, b, a]); } clearDepth(d) { this._op('clearDepth', [d]); } clearStencil(s) { this._op('clearStencil', [s]); }
    clearBufferfv(buffer, index, values) { if (buffer === 0x1800) { this._op('clearColor', [values[0], values[1], values[2], values[3]]); this._op('clear', [0x4000]); } else if (buffer === 0x1801) { this._op('clearDepth', [values[0]]); this._op('clear', [0x100]); } }
    clearBufferiv() {} clearBufferuiv() {} clearBufferfi(b, i, d, s) { this._op('clearDepth', [d]); this._op('clearStencil', [s]); this._op('clear', [0x500]); }
    colorMask(r, g, b, a) { this._op('colorMask', [r ? 1 : 0, g ? 1 : 0, b ? 1 : 0, a ? 1 : 0]); } depthMask(m) { this._op('depthMask', [m ? 1 : 0]); } stencilMask(m) { this._op('stencilMask', [m]); } stencilMaskSeparate(f, m) { this._op('stencilMaskSeparate', [f, m]); }
    stencilFunc(f, r, m) { this._op('stencilFunc', [f, r, m]); } stencilFuncSeparate(face, f, r, m) { this._op('stencilFuncSeparate', [face, f, r, m]); } stencilOp(a, b, c) { this._op('stencilOp', [a, b, c]); } stencilOpSeparate(face, a, b, c) { this._op('stencilOpSeparate', [face, a, b, c]); }
    viewport(x, y, w, h) { this._viewport = [x, y, w, h]; this._op('viewport', [x, y, w, h]); } scissor(x, y, w, h) { this._op('scissor', [x, y, w, h]); }
    enable(c) { this._op('enable', [c]); } disable(c) { this._op('disable', [c]); }
    blendFunc(s, d) { this._op('blendFunc', [s, d]); } blendFuncSeparate(a, b, c, d) { this._op('blendFuncSeparate', [a, b, c, d]); } blendEquation(e) { this._op('blendEquation', [e]); } blendEquationSeparate(a, b) { this._op('blendEquationSeparate', [a, b]); } blendColor(r, g, b, a) { this._op('blendColor', [r, g, b, a]); }
    depthFunc(f) { this._op('depthFunc', [f]); } depthRange(n, f) { this._op('depthRange', [n, f]); } cullFace(m) { this._op('cullFace', [m]); } frontFace(m) { this._op('frontFace', [m]); } lineWidth(w) { this._op('lineWidth', [w]); } polygonOffset(f, u) { this._op('polygonOffset', [f, u]); } sampleCoverage(v, i) { this._op('sampleCoverage', [v, i ? 1 : 0]); } hint(t, m) { this._op('hint', [t, m]); }
    activeTexture(t) { this._op('activeTexture', [t]); } texParameteri(t, p, v) { this._op('texParameteri', [t, p, v]); } texParameterf(t, p, v) { this._op('texParameterf', [t, p, v]); } getTexParameter() { return null; }
    pixelStorei(p, v) { if (p === 0x9240) this._flipY = !!v; if (p === 0x9241) this._premultiply = !!v; this._op('pixelStorei', [p, typeof v === 'boolean' ? (v ? 1 : 0) : v]); }
    texImage2D(target, level, internalformat, a, b, c, d, e, f) {
      if (arguments.length <= 6) {
        // (target, level, internalformat, format, type, source)
        const source = arguments[5]; const [w, h, pixels] = pixelsOf(source);
        this._op('texImage2D', [target, level, internalformat, w, h, 0, a, b, pixels]);
      } else if (arguments.length >= 9 && isSource(f)) {
        const [w, h, pixels] = pixelsOf(f);
        this._op('texImage2D', [target, level, internalformat, w, h, 0, d, e, pixels]);
      } else {
        this._op('texImage2D', [target, level, internalformat, a, b, c, d, e, data(f)]);
      }
    }
    texSubImage2D(target, level, x, y, a, b, c, d, e) {
      if (arguments.length <= 7) { const source = arguments[6]; const [w, h, pixels] = pixelsOf(source); this._op('texSubImage2D', [target, level, x, y, w, h, a, b, pixels]); }
      else if (isSource(e)) { const [w, h, pixels] = pixelsOf(e); this._op('texSubImage2D', [target, level, x, y, Math.min(a, w), Math.min(b, h), c, d, pixels]); }
      else this._op('texSubImage2D', [target, level, x, y, a, b, c, d, data(e)]);
    }
    texStorage2D(target, levels, internalformat, w, h) { for (let l = 0; l < levels; l++) this._op('texImage2D', [target, l, storageFormat(internalformat), Math.max(1, w >> l), Math.max(1, h >> l), 0, baseFormat(internalformat), baseType(internalformat), null]); }
    texImage3D() {} texSubImage3D() {} texStorage3D() {} compressedTexImage2D() {} compressedTexSubImage2D() {}
    generateMipmap(t) { this._op('generateMipmap', [t]); } copyTexImage2D(...a) { this._op('copyTexImage2D', a); } copyTexSubImage2D(...a) { this._op('copyTexSubImage2D', a); }
    framebufferTexture2D(t, a, tt, tex, level) { this._op('framebufferTexture2D', [t, a, tt, id(tex), level]); } framebufferRenderbuffer(t, a, rt, rb) { this._op('framebufferRenderbuffer', [t, a, rt, id(rb)]); } framebufferTextureLayer() {}
    renderbufferStorage(t, f, w, h) { this._op('renderbufferStorage', [t, f, w, h]); } renderbufferStorageMultisample(t, s, f, w, h) { this._op('renderbufferStorageMultisample', [t, s, f, w, h]); }
    checkFramebufferStatus(t) { return this._op('checkFramebufferStatus', [t]) || 0x8CD5; } getFramebufferAttachmentParameter() { return null; } getRenderbufferParameter() { return 0; }
    drawBuffers(b) { this._op('drawBuffers', [Int32Array.from(b)]); } readBuffer(b) { this._op('readBuffer', [b]); } blitFramebuffer(...a) { this._op('blitFramebuffer', a); } invalidateFramebuffer() {} invalidateSubFramebuffer() {}
    readPixels(x, y, w, h, format, type, pixels) { const bytes = this._op('readPixels', [x, y, w, h, format, type]); if (bytes && pixels && pixels.set) { const view = new Uint8Array(bytes); const out = new Uint8Array(pixels.buffer, pixels.byteOffset, Math.min(pixels.byteLength, view.length)); out.set(view.subarray(0, out.length)); } }
    finish() { this._op('finish', []); } flush() { this._op('flush', []); }
    uniform1f(l, x) { if (l) this._op('uniform1f', [id(l), x]); } uniform2f(l, x, y) { if (l) this._op('uniform2f', [id(l), x, y]); } uniform3f(l, x, y, z) { if (l) this._op('uniform3f', [id(l), x, y, z]); } uniform4f(l, x, y, z, w) { if (l) this._op('uniform4f', [id(l), x, y, z, w]); }
    uniform1i(l, x) { if (l) this._op('uniform1i', [id(l), x]); } uniform2i(l, x, y) { if (l) this._op('uniform2i', [id(l), x, y]); } uniform3i(l, x, y, z) { if (l) this._op('uniform3i', [id(l), x, y, z]); } uniform4i(l, x, y, z, w) { if (l) this._op('uniform4i', [id(l), x, y, z, w]); }
    uniform1ui(l, x) { if (l) this._op('uniform1ui', [id(l), x]); } uniform2ui(l, x, y) { if (l) this._op('uniform2ui', [id(l), x, y]); } uniform3ui(l, x, y, z) { if (l) this._op('uniform3ui', [id(l), x, y, z]); } uniform4ui(l, x, y, z, w) { if (l) this._op('uniform4ui', [id(l), x, y, z, w]); }
    uniform1fv(l, v) { if (l) this._op('uniform1fv', [id(l), Float32Array.from(v)]); } uniform2fv(l, v) { if (l) this._op('uniform2fv', [id(l), Float32Array.from(v)]); } uniform3fv(l, v) { if (l) this._op('uniform3fv', [id(l), Float32Array.from(v)]); } uniform4fv(l, v) { if (l) this._op('uniform4fv', [id(l), Float32Array.from(v)]); }
    uniform1iv(l, v) { if (l) this._op('uniform1iv', [id(l), Int32Array.from(v)]); } uniform2iv(l, v) { if (l) this._op('uniform2iv', [id(l), Int32Array.from(v)]); } uniform3iv(l, v) { if (l) this._op('uniform3iv', [id(l), Int32Array.from(v)]); } uniform4iv(l, v) { if (l) this._op('uniform4iv', [id(l), Int32Array.from(v)]); }
    uniform1uiv(l, v) { if (l) this._op('uniform1uiv', [id(l), Int32Array.from(v)]); } uniform2uiv(l, v) { if (l) this._op('uniform2uiv', [id(l), Int32Array.from(v)]); } uniform3uiv(l, v) { if (l) this._op('uniform3uiv', [id(l), Int32Array.from(v)]); } uniform4uiv(l, v) { if (l) this._op('uniform4uiv', [id(l), Int32Array.from(v)]); }
    uniformMatrix2fv(l, t, v) { if (l) this._op('uniformMatrix2fv', [id(l), t ? 1 : 0, Float32Array.from(v)]); } uniformMatrix3fv(l, t, v) { if (l) this._op('uniformMatrix3fv', [id(l), t ? 1 : 0, Float32Array.from(v)]); } uniformMatrix4fv(l, t, v) { if (l) this._op('uniformMatrix4fv', [id(l), t ? 1 : 0, Float32Array.from(v)]); }
    uniformMatrix2x3fv() {} uniformMatrix3x2fv() {} uniformMatrix2x4fv() {} uniformMatrix4x2fv() {} uniformMatrix3x4fv() {} uniformMatrix4x3fv() {}
    beginQuery() {} endQuery() {} getQuery() { return null; } getQueryParameter() { return 0; } samplerParameteri() {} samplerParameterf() {} getSamplerParameter() { return 0; } clientWaitSync() { return 0x911A; } waitSync() {} getSyncParameter() { return 0x9119; } beginTransformFeedback() {} endTransformFeedback() {} transformFeedbackVaryings() {} getTransformFeedbackVarying() { return null; } pauseTransformFeedback() {} resumeTransformFeedback() {} copyBufferSubData() {} getBufferSubData() {} getIndexedParameter() { return null; } getInternalformatParameter() { return new Int32Array([4]); } getFragDataLocation() { return -1; }
    makeXRCompatible() { return Promise.resolve(); }
  }
  // Whether a texture call's last argument is a pixel source (an element,
  // ImageData or ImageBitmap) rather than bytes.
  const isSource = (v) => v != null && typeof v === 'object' && !ArrayBuffer.isView(v) && !(v instanceof ArrayBuffer);
  // Pixel sources for textures: a canvas (its raster's pixels), ImageData,
  // an image (not available yet: a transparent texel).
  const pixelsOf = (source) => {
    if (!source) return [0, 0, null];
    if (source instanceof ImageData) return [source.width, source.height, source.data];
    if (source instanceof ImageBitmap) source = source._source;
    if (source instanceof Node && source.nodeName === 'CANVAS') { const bytes = N.canvasPixels(source); const w = source.width, h = source.height; return [w, h, bytes ? new Uint8Array(bytes) : null]; }
    if (source instanceof Node && source.nodeName === 'IMG') { const z = H.__olo_imagePixelSize(source); const bytes = z && N.imagePixels(source); if (bytes) return [z[0], z[1], new Uint8Array(bytes)]; }
    return [1, 1, new Uint8Array(4)];
  };
  const storageFormat = (f) => f;
  const baseFormat = (f) => ({ 0x8058: 0x1908, 0x8051: 0x1907, 0x8229: 0x1903, 0x822B: 0x8227, 0x8814: 0x1908, 0x881A: 0x1908, 0x822E: 0x1903, 0x822D: 0x1903, 0x8CAC: 0x1902, 0x81A5: 0x1902, 0x88F0: 0x84F9 }[f] || 0x1908);
  const baseType = (f) => ({ 0x8814: 0x1406, 0x881A: 0x140B, 0x822E: 0x1406, 0x822D: 0x140B, 0x8CAC: 0x1406, 0x81A5: 0x1403, 0x88F0: 0x84FA }[f] || 0x1401);
  class WebGL2RenderingContext extends WebGLRenderingContext {}
  Object.assign(WebGLRenderingContext, C);
  Object.assign(WebGL2RenderingContext, C);
  global.WebGLRenderingContext = WebGLRenderingContext;
  global.WebGL2RenderingContext = WebGL2RenderingContext;
  global.WebGLContextEvent = class WebGLContextEvent extends Event { constructor(type, init) { super(type, init); this.statusMessage = (init && init.statusMessage) || ''; } };
  // getContext: WebGL kinds here; the 2D kind stays the prelude's.
  const getContext2D = HTMLCanvasElement.prototype.getContext;
  HTMLCanvasElement.prototype.getContext = { getContext(kind, attributes) {
    kind = String(kind);
    if (kind === 'webgl' || kind === 'experimental-webgl' || kind === 'webgl2') {
      if (this._gl) return this._gl._version === (kind === 'webgl2' ? 2 : 1) ? this._gl : null;
      if (this._ctx2d) return null;
      const version = kind === 'webgl2' ? 2 : 1;
      const a = attributes || {};
      const ok = N.glCreate(this, parseInt(N.attr(this, 'width'), 10) || 300, parseInt(N.attr(this, 'height'), 10) || 150, a.premultipliedAlpha !== false ? 1 : 0, a.preserveDrawingBuffer ? 1 : 0, version, a.alpha !== false ? 1 : 0);
      if (!ok) return null;
      Object.defineProperty(this, '_gl', { value: new (version === 2 ? WebGL2RenderingContext : WebGLRenderingContext)(this, a, version), configurable: true });
      Object.defineProperty(this, '_sized', { value: true, configurable: true });
      return this._gl;
    }
    return getContext2D.call(this, kind, attributes);
  } }.getContext;
})(globalThis);
