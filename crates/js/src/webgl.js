// WebGL for the page's scripts: WebGLRenderingContext (and the WebGL2
// surface) over the engine's GL (__olo.glOp), one framebuffer per canvas.
// Objects are small wrappers around an index the engine's tables hold.
(function (global) {
  'use strict';
  const N = global.__olo;
  const C = {
    DEPTH_BUFFER_BIT: 0x00000100, STENCIL_BUFFER_BIT: 0x00000400, COLOR_BUFFER_BIT: 0x00004000,
    POINTS: 0, LINES: 1, LINE_LOOP: 2, LINE_STRIP: 3, TRIANGLES: 4, TRIANGLE_STRIP: 5, TRIANGLE_FAN: 6,
    ZERO: 0, ONE: 1, SRC_COLOR: 0x0300, ONE_MINUS_SRC_COLOR: 0x0301, SRC_ALPHA: 0x0302, ONE_MINUS_SRC_ALPHA: 0x0303, DST_ALPHA: 0x0304, ONE_MINUS_DST_ALPHA: 0x0305, DST_COLOR: 0x0306, ONE_MINUS_DST_COLOR: 0x0307, SRC_ALPHA_SATURATE: 0x0308,
    FUNC_ADD: 0x8006, BLEND_EQUATION: 0x8009, BLEND_EQUATION_RGB: 0x8009, BLEND_EQUATION_ALPHA: 0x883D, FUNC_SUBTRACT: 0x800A, FUNC_REVERSE_SUBTRACT: 0x800B, MIN: 0x8007, MAX: 0x8008,
    BLEND_DST_RGB: 0x80C8, BLEND_SRC_RGB: 0x80C9, BLEND_DST_ALPHA: 0x80CA, BLEND_SRC_ALPHA: 0x80CB, CONSTANT_COLOR: 0x8001, ONE_MINUS_CONSTANT_COLOR: 0x8002, CONSTANT_ALPHA: 0x8003, ONE_MINUS_CONSTANT_ALPHA: 0x8004, BLEND_COLOR: 0x8005,
    ARRAY_BUFFER: 0x8892, ELEMENT_ARRAY_BUFFER: 0x8893, ARRAY_BUFFER_BINDING: 0x8894, ELEMENT_ARRAY_BUFFER_BINDING: 0x8895, STREAM_DRAW: 0x88E0, STATIC_DRAW: 0x88E4, DYNAMIC_DRAW: 0x88E8, STREAM_READ: 0x88E1, STREAM_COPY: 0x88E2, STATIC_READ: 0x88E5, STATIC_COPY: 0x88E6, DYNAMIC_READ: 0x88E9, DYNAMIC_COPY: 0x88EA, BUFFER_SIZE: 0x8764, BUFFER_USAGE: 0x8765, CURRENT_VERTEX_ATTRIB: 0x8626,
    FRONT: 0x0404, BACK: 0x0405, FRONT_AND_BACK: 0x0408, CULL_FACE: 0x0B44, BLEND: 0x0BE2, DITHER: 0x0BD0, STENCIL_TEST: 0x0B90, DEPTH_TEST: 0x0B71, SCISSOR_TEST: 0x0C11, POLYGON_OFFSET_FILL: 0x8037, SAMPLE_ALPHA_TO_COVERAGE: 0x809E, SAMPLE_COVERAGE: 0x80A0,
    NO_ERROR: 0, INVALID_ENUM: 0x0500, INVALID_VALUE: 0x0501, INVALID_OPERATION: 0x0502, OUT_OF_MEMORY: 0x0505, INVALID_FRAMEBUFFER_OPERATION: 0x0506, CONTEXT_LOST_WEBGL: 0x9242,
    CW: 0x0900, CCW: 0x0901, LINE_WIDTH: 0x0B21, ALIASED_POINT_SIZE_RANGE: 0x846D, ALIASED_LINE_WIDTH_RANGE: 0x846E, CULL_FACE_MODE: 0x0B45, FRONT_FACE: 0x0B46, DEPTH_RANGE: 0x0B70, DEPTH_WRITEMASK: 0x0B72, DEPTH_CLEAR_VALUE: 0x0B73, DEPTH_FUNC: 0x0B74, STENCIL_CLEAR_VALUE: 0x0B91, STENCIL_FUNC: 0x0B92, STENCIL_FAIL: 0x0B94, STENCIL_PASS_DEPTH_FAIL: 0x0B95, STENCIL_PASS_DEPTH_PASS: 0x0B96, STENCIL_REF: 0x0B97, STENCIL_VALUE_MASK: 0x0B93, STENCIL_WRITEMASK: 0x0B98, STENCIL_BACK_FUNC: 0x8800, STENCIL_BACK_FAIL: 0x8801, STENCIL_BACK_PASS_DEPTH_FAIL: 0x8802, STENCIL_BACK_PASS_DEPTH_PASS: 0x8803, STENCIL_BACK_REF: 0x8CA3, STENCIL_BACK_VALUE_MASK: 0x8CA4, STENCIL_BACK_WRITEMASK: 0x8CA5, VIEWPORT: 0x0BA2, SCISSOR_BOX: 0x0C10, COLOR_CLEAR_VALUE: 0x0C22, COLOR_WRITEMASK: 0x0C23, UNPACK_ALIGNMENT: 0x0CF5, PACK_ALIGNMENT: 0x0D05, MAX_TEXTURE_SIZE: 0x0D33, MAX_VIEWPORT_DIMS: 0x0D3A, SUBPIXEL_BITS: 0x0D50, RED_BITS: 0x0D52, GREEN_BITS: 0x0D53, BLUE_BITS: 0x0D54, ALPHA_BITS: 0x0D55, DEPTH_BITS: 0x0D56, STENCIL_BITS: 0x0D57, POLYGON_OFFSET_UNITS: 0x2A00, POLYGON_OFFSET_FACTOR: 0x8038, TEXTURE_BINDING_2D: 0x8069, SAMPLE_BUFFERS: 0x80A8, SAMPLES: 0x80A9, SAMPLE_COVERAGE_VALUE: 0x80AA, SAMPLE_COVERAGE_INVERT: 0x80AB, COMPRESSED_TEXTURE_FORMATS: 0x86A3,
    DONT_CARE: 0x1100, FASTEST: 0x1101, NICEST: 0x1102, GENERATE_MIPMAP_HINT: 0x8192,
    BYTE: 0x1400, UNSIGNED_BYTE: 0x1401, SHORT: 0x1402, UNSIGNED_SHORT: 0x1403, INT: 0x1404, UNSIGNED_INT: 0x1405, FLOAT: 0x1406, HALF_FLOAT: 0x140B,
    DEPTH_COMPONENT: 0x1902, ALPHA: 0x1906, RGB: 0x1907, RGBA: 0x1908, LUMINANCE: 0x1909, LUMINANCE_ALPHA: 0x190A, UNSIGNED_SHORT_4_4_4_4: 0x8033, UNSIGNED_SHORT_5_5_5_1: 0x8034, UNSIGNED_SHORT_5_6_5: 0x8363,
    FRAGMENT_SHADER: 0x8B30, VERTEX_SHADER: 0x8B31, MAX_VERTEX_ATTRIBS: 0x8869, MAX_VERTEX_UNIFORM_VECTORS: 0x8DFB, MAX_VARYING_VECTORS: 0x8DFC, MAX_COMBINED_TEXTURE_IMAGE_UNITS: 0x8B4D, MAX_VERTEX_TEXTURE_IMAGE_UNITS: 0x8B4C, MAX_TEXTURE_IMAGE_UNITS: 0x8872, MAX_FRAGMENT_UNIFORM_VECTORS: 0x8DFD, SHADER_TYPE: 0x8B4F, DELETE_STATUS: 0x8B80, LINK_STATUS: 0x8B82, VALIDATE_STATUS: 0x8B83, ATTACHED_SHADERS: 0x8B85, ACTIVE_UNIFORMS: 0x8B86, ACTIVE_ATTRIBUTES: 0x8B89, SHADING_LANGUAGE_VERSION: 0x8B8C, CURRENT_PROGRAM: 0x8B8D,
    NEVER: 0x0200, LESS: 0x0201, EQUAL: 0x0202, LEQUAL: 0x0203, GREATER: 0x0204, NOTEQUAL: 0x0205, GEQUAL: 0x0206, ALWAYS: 0x0207, KEEP: 0x1E00, REPLACE: 0x1E01, INCR: 0x1E02, DECR: 0x1E03, INVERT: 0x150A, INCR_WRAP: 0x8507, DECR_WRAP: 0x8508,
    VENDOR: 0x1F00, RENDERER: 0x1F01, VERSION: 0x1F02, NEAREST: 0x2600, LINEAR: 0x2601, NEAREST_MIPMAP_NEAREST: 0x2700, LINEAR_MIPMAP_NEAREST: 0x2701, NEAREST_MIPMAP_LINEAR: 0x2702, LINEAR_MIPMAP_LINEAR: 0x2703, TEXTURE_MAG_FILTER: 0x2800, TEXTURE_MIN_FILTER: 0x2801, TEXTURE_WRAP_S: 0x2802, TEXTURE_WRAP_T: 0x2803, TEXTURE_2D: 0x0DE1, TEXTURE: 0x1702, TEXTURE_CUBE_MAP: 0x8513, TEXTURE_BINDING_CUBE_MAP: 0x8514, TEXTURE_CUBE_MAP_POSITIVE_X: 0x8515, TEXTURE_CUBE_MAP_NEGATIVE_X: 0x8516, TEXTURE_CUBE_MAP_POSITIVE_Y: 0x8517, TEXTURE_CUBE_MAP_NEGATIVE_Y: 0x8518, TEXTURE_CUBE_MAP_POSITIVE_Z: 0x8519, TEXTURE_CUBE_MAP_NEGATIVE_Z: 0x851A, MAX_CUBE_MAP_TEXTURE_SIZE: 0x851C,
    TEXTURE0: 0x84C0, ACTIVE_TEXTURE: 0x84E0, REPEAT: 0x2901, CLAMP_TO_EDGE: 0x812F, MIRRORED_REPEAT: 0x8370,
    FLOAT_VEC2: 0x8B50, FLOAT_VEC3: 0x8B51, FLOAT_VEC4: 0x8B52, INT_VEC2: 0x8B53, INT_VEC3: 0x8B54, INT_VEC4: 0x8B55, BOOL: 0x8B56, BOOL_VEC2: 0x8B57, BOOL_VEC3: 0x8B58, BOOL_VEC4: 0x8B59, FLOAT_MAT2: 0x8B5A, FLOAT_MAT3: 0x8B5B, FLOAT_MAT4: 0x8B5C, SAMPLER_2D: 0x8B5E, SAMPLER_CUBE: 0x8B60, SAMPLER_3D: 0x8B5F, SAMPLER_2D_ARRAY: 0x8DC1,
    VERTEX_ATTRIB_ARRAY_ENABLED: 0x8622, VERTEX_ATTRIB_ARRAY_SIZE: 0x8623, VERTEX_ATTRIB_ARRAY_STRIDE: 0x8624, VERTEX_ATTRIB_ARRAY_TYPE: 0x8625, VERTEX_ATTRIB_ARRAY_NORMALIZED: 0x886A, VERTEX_ATTRIB_ARRAY_POINTER: 0x8645, VERTEX_ATTRIB_ARRAY_BUFFER_BINDING: 0x889F,
    IMPLEMENTATION_COLOR_READ_TYPE: 0x8B9A, IMPLEMENTATION_COLOR_READ_FORMAT: 0x8B9B, COMPILE_STATUS: 0x8B81, LOW_FLOAT: 0x8DF0, MEDIUM_FLOAT: 0x8DF1, HIGH_FLOAT: 0x8DF2, LOW_INT: 0x8DF3, MEDIUM_INT: 0x8DF4, HIGH_INT: 0x8DF5,
    FRAMEBUFFER: 0x8D40, RENDERBUFFER: 0x8D41, RGBA4: 0x8056, RGB5_A1: 0x8057, RGB565: 0x8D62, DEPTH_COMPONENT16: 0x81A5, STENCIL_INDEX8: 0x8D48, DEPTH_STENCIL: 0x84F9, RENDERBUFFER_WIDTH: 0x8D42, RENDERBUFFER_HEIGHT: 0x8D43, RENDERBUFFER_INTERNAL_FORMAT: 0x8D44, RENDERBUFFER_RED_SIZE: 0x8D50, RENDERBUFFER_GREEN_SIZE: 0x8D51, RENDERBUFFER_BLUE_SIZE: 0x8D52, RENDERBUFFER_ALPHA_SIZE: 0x8D53, RENDERBUFFER_DEPTH_SIZE: 0x8D54, RENDERBUFFER_STENCIL_SIZE: 0x8D55, FRAMEBUFFER_ATTACHMENT_OBJECT_TYPE: 0x8CD0, FRAMEBUFFER_ATTACHMENT_OBJECT_NAME: 0x8CD1, FRAMEBUFFER_ATTACHMENT_TEXTURE_LEVEL: 0x8CD2, FRAMEBUFFER_ATTACHMENT_TEXTURE_CUBE_MAP_FACE: 0x8CD3, COLOR_ATTACHMENT0: 0x8CE0, DEPTH_ATTACHMENT: 0x8D00, STENCIL_ATTACHMENT: 0x8D20, DEPTH_STENCIL_ATTACHMENT: 0x821A, NONE: 0, FRAMEBUFFER_COMPLETE: 0x8CD5, FRAMEBUFFER_INCOMPLETE_ATTACHMENT: 0x8CD6, FRAMEBUFFER_INCOMPLETE_MISSING_ATTACHMENT: 0x8CD7, FRAMEBUFFER_INCOMPLETE_DIMENSIONS: 0x8CD9, FRAMEBUFFER_UNSUPPORTED: 0x8CDD, FRAMEBUFFER_BINDING: 0x8CA6, RENDERBUFFER_BINDING: 0x8CA7, MAX_RENDERBUFFER_SIZE: 0x84E8, INVALID_FRAMEBUFFER_OPERATION_: 0x0506,
    UNPACK_FLIP_Y_WEBGL: 0x9240, UNPACK_PREMULTIPLY_ALPHA_WEBGL: 0x9241, UNPACK_COLORSPACE_CONVERSION_WEBGL: 0x9243, BROWSER_DEFAULT_WEBGL: 0x9244,
    // WebGL2
    READ_BUFFER: 0x0C02, UNPACK_ROW_LENGTH: 0x0CF2, UNPACK_SKIP_ROWS: 0x0CF3, UNPACK_SKIP_PIXELS: 0x0CF4, PACK_ROW_LENGTH: 0x0D02, PACK_SKIP_ROWS: 0x0D03, PACK_SKIP_PIXELS: 0x0D04, COLOR: 0x1800, DEPTH: 0x1801, STENCIL: 0x1802, RED: 0x1903, RGB8: 0x8051, RGBA8: 0x8058, RGB10_A2: 0x8059, TEXTURE_BINDING_3D: 0x806A, UNPACK_SKIP_IMAGES: 0x806D, UNPACK_IMAGE_HEIGHT: 0x806E, TEXTURE_3D: 0x806F, TEXTURE_WRAP_R: 0x8072, MAX_3D_TEXTURE_SIZE: 0x8073, UNSIGNED_INT_2_10_10_10_REV: 0x8368, MAX_ELEMENTS_VERTICES: 0x80E8, MAX_ELEMENTS_INDICES: 0x80E9, TEXTURE_MIN_LOD: 0x813A, TEXTURE_MAX_LOD: 0x813B, TEXTURE_BASE_LEVEL: 0x813C, TEXTURE_MAX_LEVEL: 0x813D, MAX_TEXTURE_LOD_BIAS: 0x84FD, TEXTURE_COMPARE_MODE: 0x884C, TEXTURE_COMPARE_FUNC: 0x884D, CURRENT_QUERY: 0x8865, QUERY_RESULT: 0x8866, QUERY_RESULT_AVAILABLE: 0x8867, STREAM_READ_: 0x88E1, MAX_DRAW_BUFFERS: 0x8824, DRAW_BUFFER0: 0x8825, DRAW_BUFFER1: 0x8826, DRAW_BUFFER2: 0x8827, DRAW_BUFFER3: 0x8828, DRAW_BUFFER4: 0x8829, DRAW_BUFFER5: 0x882A, DRAW_BUFFER6: 0x882B, DRAW_BUFFER7: 0x882C, MAX_FRAGMENT_UNIFORM_COMPONENTS: 0x8B49, MAX_VERTEX_UNIFORM_COMPONENTS: 0x8B4A, SAMPLER_2D_SHADOW: 0x8B62, FRAGMENT_SHADER_DERIVATIVE_HINT: 0x8B8B, PIXEL_PACK_BUFFER: 0x88EB, PIXEL_UNPACK_BUFFER: 0x88EC, PIXEL_PACK_BUFFER_BINDING: 0x88ED, PIXEL_UNPACK_BUFFER_BINDING: 0x88EF, FLOAT_MAT2x3: 0x8B65, FLOAT_MAT2x4: 0x8B66, FLOAT_MAT3x2: 0x8B67, FLOAT_MAT3x4: 0x8B68, FLOAT_MAT4x2: 0x8B69, FLOAT_MAT4x3: 0x8B6A, SRGB: 0x8C40, SRGB8: 0x8C41, SRGB8_ALPHA8: 0x8C43, COMPARE_REF_TO_TEXTURE: 0x884E, RGBA32F: 0x8814, RGB32F: 0x8815, RGBA16F: 0x881A, RGB16F: 0x881B, VERTEX_ATTRIB_ARRAY_INTEGER: 0x88FD, MAX_ARRAY_TEXTURE_LAYERS: 0x88FF, MIN_PROGRAM_TEXEL_OFFSET: 0x8904, MAX_PROGRAM_TEXEL_OFFSET: 0x8905, MAX_VARYING_COMPONENTS: 0x8B4B, TEXTURE_2D_ARRAY: 0x8C1A, TEXTURE_BINDING_2D_ARRAY: 0x8C1D, R11F_G11F_B10F: 0x8C3A, UNSIGNED_INT_10F_11F_11F_REV: 0x8C3B, RGB9_E5: 0x8C3D, UNSIGNED_INT_5_9_9_9_REV: 0x8C3E, TRANSFORM_FEEDBACK_BUFFER_MODE: 0x8C7F, MAX_TRANSFORM_FEEDBACK_SEPARATE_COMPONENTS: 0x8C80, TRANSFORM_FEEDBACK_VARYINGS: 0x8C83, TRANSFORM_FEEDBACK_BUFFER_START: 0x8C84, TRANSFORM_FEEDBACK_BUFFER_SIZE: 0x8C85, TRANSFORM_FEEDBACK_PRIMITIVES_WRITTEN: 0x8C88, RASTERIZER_DISCARD: 0x8C89, MAX_TRANSFORM_FEEDBACK_INTERLEAVED_COMPONENTS: 0x8C8A, MAX_TRANSFORM_FEEDBACK_SEPARATE_ATTRIBS: 0x8C8B, INTERLEAVED_ATTRIBS: 0x8C8C, SEPARATE_ATTRIBS: 0x8C8D, TRANSFORM_FEEDBACK_BUFFER: 0x8C8E, TRANSFORM_FEEDBACK_BUFFER_BINDING: 0x8C8F, RGBA32UI: 0x8D70, RGB32UI: 0x8D71, RGBA16UI: 0x8D76, RGB16UI: 0x8D77, RGBA8UI: 0x8D7C, RGB8UI: 0x8D7D, RGBA32I: 0x8D82, RGB32I: 0x8D83, RGBA16I: 0x8D88, RGB16I: 0x8D89, RGBA8I: 0x8D8E, RGB8I: 0x8D8F, RED_INTEGER: 0x8D94, RGB_INTEGER: 0x8D98, RGBA_INTEGER: 0x8D99, SAMPLER_2D_ARRAY_SHADOW: 0x8DC4, SAMPLER_CUBE_SHADOW: 0x8DC5, UNSIGNED_INT_VEC2: 0x8DC6, UNSIGNED_INT_VEC3: 0x8DC7, UNSIGNED_INT_VEC4: 0x8DC8, INT_SAMPLER_2D: 0x8DCA, INT_SAMPLER_3D: 0x8DCB, INT_SAMPLER_CUBE: 0x8DCC, INT_SAMPLER_2D_ARRAY: 0x8DCF, UNSIGNED_INT_SAMPLER_2D: 0x8DD2, UNSIGNED_INT_SAMPLER_3D: 0x8DD3, UNSIGNED_INT_SAMPLER_CUBE: 0x8DD4, UNSIGNED_INT_SAMPLER_2D_ARRAY: 0x8DD7, DEPTH_COMPONENT32F: 0x8CAC, DEPTH32F_STENCIL8: 0x8CAD, FLOAT_32_UNSIGNED_INT_24_8_REV: 0x8DAD, FRAMEBUFFER_ATTACHMENT_COLOR_ENCODING: 0x8210, FRAMEBUFFER_ATTACHMENT_COMPONENT_TYPE: 0x8211, FRAMEBUFFER_ATTACHMENT_RED_SIZE: 0x8212, FRAMEBUFFER_ATTACHMENT_GREEN_SIZE: 0x8213, FRAMEBUFFER_ATTACHMENT_BLUE_SIZE: 0x8214, FRAMEBUFFER_ATTACHMENT_ALPHA_SIZE: 0x8215, FRAMEBUFFER_ATTACHMENT_DEPTH_SIZE: 0x8216, FRAMEBUFFER_ATTACHMENT_STENCIL_SIZE: 0x8217, FRAMEBUFFER_DEFAULT: 0x8218, UNSIGNED_INT_24_8: 0x84FA, DEPTH24_STENCIL8: 0x88F0, UNSIGNED_NORMALIZED: 0x8C17, DRAW_FRAMEBUFFER_BINDING: 0x8CA6, READ_FRAMEBUFFER: 0x8CA8, DRAW_FRAMEBUFFER: 0x8CA9, READ_FRAMEBUFFER_BINDING: 0x8CAA, RENDERBUFFER_SAMPLES: 0x8CAB, FRAMEBUFFER_ATTACHMENT_TEXTURE_LAYER: 0x8CD4, MAX_COLOR_ATTACHMENTS: 0x8CDF, COLOR_ATTACHMENT1: 0x8CE1, COLOR_ATTACHMENT2: 0x8CE2, COLOR_ATTACHMENT3: 0x8CE3, COLOR_ATTACHMENT4: 0x8CE4, COLOR_ATTACHMENT5: 0x8CE5, COLOR_ATTACHMENT6: 0x8CE6, COLOR_ATTACHMENT7: 0x8CE7, FRAMEBUFFER_INCOMPLETE_MULTISAMPLE: 0x8D56, MAX_SAMPLES: 0x8D57, HALF_FLOAT_: 0x140B, RG: 0x8227, RG_INTEGER: 0x8228, R8: 0x8229, RG8: 0x822B, R16F: 0x822D, R32F: 0x822E, RG16F: 0x822F, RG32F: 0x8230, R8I: 0x8231, R8UI: 0x8232, R16I: 0x8233, R16UI: 0x8234, R32I: 0x8235, R32UI: 0x8236, RG8I: 0x8237, RG8UI: 0x8238, RG16I: 0x8239, RG16UI: 0x823A, RG32I: 0x823B, RG32UI: 0x823C, VERTEX_ARRAY_BINDING: 0x85B5, R8_SNORM: 0x8F94, RG8_SNORM: 0x8F95, RGB8_SNORM: 0x8F96, RGBA8_SNORM: 0x8F97, SIGNED_NORMALIZED: 0x8F9C, COPY_READ_BUFFER: 0x8F36, COPY_WRITE_BUFFER: 0x8F37, COPY_READ_BUFFER_BINDING: 0x8F36, COPY_WRITE_BUFFER_BINDING: 0x8F37, UNIFORM_BUFFER: 0x8A11, UNIFORM_BUFFER_BINDING: 0x8A28, UNIFORM_BUFFER_START: 0x8A29, UNIFORM_BUFFER_SIZE: 0x8A2A, MAX_VERTEX_UNIFORM_BLOCKS: 0x8A2B, MAX_FRAGMENT_UNIFORM_BLOCKS: 0x8A2D, MAX_COMBINED_UNIFORM_BLOCKS: 0x8A2E, MAX_UNIFORM_BUFFER_BINDINGS: 0x8A2F, MAX_UNIFORM_BLOCK_SIZE: 0x8A30, MAX_COMBINED_VERTEX_UNIFORM_COMPONENTS: 0x8A31, MAX_COMBINED_FRAGMENT_UNIFORM_COMPONENTS: 0x8A33, UNIFORM_BUFFER_OFFSET_ALIGNMENT: 0x8A34, ACTIVE_UNIFORM_BLOCKS: 0x8A36, UNIFORM_TYPE: 0x8A37, UNIFORM_SIZE: 0x8A38, UNIFORM_BLOCK_INDEX: 0x8A3A, UNIFORM_OFFSET: 0x8A3B, UNIFORM_ARRAY_STRIDE: 0x8A3C, UNIFORM_MATRIX_STRIDE: 0x8A3D, UNIFORM_IS_ROW_MAJOR: 0x8A3E, UNIFORM_BLOCK_BINDING: 0x8A3F, UNIFORM_BLOCK_DATA_SIZE: 0x8A40, UNIFORM_BLOCK_ACTIVE_UNIFORMS: 0x8A42, UNIFORM_BLOCK_ACTIVE_UNIFORM_INDICES: 0x8A43, UNIFORM_BLOCK_REFERENCED_BY_VERTEX_SHADER: 0x8A44, UNIFORM_BLOCK_REFERENCED_BY_FRAGMENT_SHADER: 0x8A46, INVALID_INDEX: 0xFFFFFFFF, MAX_VERTEX_OUTPUT_COMPONENTS: 0x9122, MAX_FRAGMENT_INPUT_COMPONENTS: 0x9125, MAX_SERVER_WAIT_TIMEOUT: 0x9111, OBJECT_TYPE: 0x9112, SYNC_CONDITION: 0x9113, SYNC_STATUS: 0x9114, SYNC_FLAGS: 0x9115, SYNC_FENCE: 0x9116, SYNC_GPU_COMMANDS_COMPLETE: 0x9117, UNSIGNALED: 0x9118, SIGNALED: 0x9119, ALREADY_SIGNALED: 0x911A, TIMEOUT_EXPIRED: 0x911B, CONDITION_SATISFIED: 0x911C, WAIT_FAILED: 0x911D, SYNC_FLUSH_COMMANDS_BIT: 0x00000001, VERTEX_ATTRIB_ARRAY_DIVISOR: 0x88FE, ANY_SAMPLES_PASSED: 0x8C2F, ANY_SAMPLES_PASSED_CONSERVATIVE: 0x8D6A, SAMPLER_BINDING: 0x8919, RGB10_A2UI: 0x906F, INT_2_10_10_10_REV: 0x8D9F, TRANSFORM_FEEDBACK: 0x8E22, TRANSFORM_FEEDBACK_PAUSED: 0x8E23, TRANSFORM_FEEDBACK_ACTIVE: 0x8E24, TRANSFORM_FEEDBACK_BINDING: 0x8E25, TEXTURE_IMMUTABLE_FORMAT: 0x912F, MAX_ELEMENT_INDEX: 0x8D6B, TEXTURE_IMMUTABLE_LEVELS: 0x82DF, TIMEOUT_IGNORED: -1, MAX_CLIENT_WAIT_TIMEOUT_WEBGL: 0x9247,
  };
  for (let i = 1; i < 32; i++) C['TEXTURE' + i] = 0x84C0 + i;
  const kinds = ['WebGLBuffer', 'WebGLTexture', 'WebGLProgram', 'WebGLShader', 'WebGLFramebuffer', 'WebGLRenderbuffer', 'WebGLVertexArrayObject', 'WebGLUniformLocation', 'WebGLActiveInfo', 'WebGLShaderPrecisionFormat', 'WebGLQuery', 'WebGLSampler', 'WebGLSync', 'WebGLTransformFeedback'];
  const classes = {};
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
    getSupportedExtensions() { return ['WEBGL_debug_shaders', 'KHR_parallel_shader_compile', 'EXT_float_blend', 'OES_fbo_render_mipmap', 'ANGLE_instanced_arrays', 'EXT_blend_minmax', 'EXT_color_buffer_float', 'EXT_color_buffer_half_float', 'EXT_frag_depth', 'EXT_shader_texture_lod', 'EXT_texture_filter_anisotropic', 'OES_element_index_uint', 'OES_standard_derivatives', 'OES_texture_float', 'OES_texture_float_linear', 'OES_texture_half_float', 'OES_texture_half_float_linear', 'OES_vertex_array_object', 'WEBGL_color_buffer_float', 'WEBGL_depth_texture', 'WEBGL_lose_context', 'WEBGL_debug_renderer_info']; }
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
        case 0x1F02: return this._version === 2 ? 'WebGL 2.0 (OpenGL ES 3.0 Olo)' : 'WebGL 1.0 (OpenGL ES 2.0 Olo)';
        case 0x8B8C: return this._version === 2 ? 'WebGL GLSL ES 3.00 (OpenGL ES GLSL ES 3.00 Olo)' : 'WebGL GLSL ES 1.0 (OpenGL ES GLSL ES 1.0 Olo)';
        case 0x9245: return 'Olo';
        case 0x9246: return 'Olo GPU (OpenGL ES)';
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
      } else if (arguments.length === 9 && f != null && typeof f === 'object' && !ArrayBuffer.isView(f) && !(f instanceof ArrayBuffer)) {
        const [w, h, pixels] = pixelsOf(f);
        this._op('texImage2D', [target, level, internalformat, w, h, 0, d, e, pixels]);
      } else {
        this._op('texImage2D', [target, level, internalformat, a, b, c, d, e, data(f)]);
      }
    }
    texSubImage2D(target, level, x, y, a, b, c, d, e) {
      if (arguments.length <= 7) { const source = arguments[6]; const [w, h, pixels] = pixelsOf(source); this._op('texSubImage2D', [target, level, x, y, w, h, a, b, pixels]); }
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
  // Pixel sources for textures: a canvas (its raster's pixels), ImageData,
  // an image (not available yet: a transparent texel).
  const pixelsOf = (source) => {
    if (!source) return [0, 0, null];
    if (source instanceof ImageData) return [source.width, source.height, source.data];
    if (source instanceof ImageBitmap) source = source._source;
    if (source instanceof Node && source.nodeName === 'CANVAS') { const bytes = N.canvasPixels(source); const w = source.width, h = source.height; return [w, h, bytes ? new Uint8Array(bytes) : null]; }
    if (source instanceof Node && source.nodeName === 'IMG') { const z = global.__olo_imagePixelSize(source); const bytes = z && N.imagePixels(source); if (bytes) return [z[0], z[1], new Uint8Array(bytes)]; }
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
  HTMLCanvasElement.prototype.getContext = function (kind, attributes) {
    kind = String(kind);
    if (kind === 'webgl' || kind === 'experimental-webgl' || kind === 'webgl2') {
      if (this._gl) return this._gl._version === (kind === 'webgl2' ? 2 : 1) ? this._gl : null;
      if (this._ctx2d) return null;
      const version = kind === 'webgl2' ? 2 : 1;
      const a = attributes || {};
      const ok = N.glCreate(this, parseInt(N.attr(this, 'width'), 10) || 300, parseInt(N.attr(this, 'height'), 10) || 150, a.premultipliedAlpha !== false ? 1 : 0, a.preserveDrawingBuffer ? 1 : 0, version);
      if (!ok) return null;
      Object.defineProperty(this, '_gl', { value: new (version === 2 ? WebGL2RenderingContext : WebGLRenderingContext)(this, a, version), configurable: true });
      Object.defineProperty(this, '_sized', { value: true, configurable: true });
      return this._gl;
    }
    return getContext2D.call(this, kind, attributes);
  };
})(globalThis);
