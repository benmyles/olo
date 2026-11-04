#version 100

//_DEFINES_

#if defined(EXTERNAL)
#extension GL_OES_EGL_image_external : require
#endif

precision highp float;
#if defined(EXTERNAL)
uniform samplerExternalOES tex;
#else
uniform sampler2D tex;
#endif

uniform float alpha;
varying vec2 v_coords;

#if defined(COLOR)
// Colour management (gles/color.rs): the texture's encoding decoded to
// light (relative to SDR's white), converted between primaries and
// luminances, clipped to the target's range, encoded as it holds light.
// Transfer functions: 0 gamma 2.2, 1 PQ, 2 linear, 3 sRGB.
uniform mat3 color_matrix;
uniform float tf_in;
uniform float tf_out;
uniform float out_scale;
uniform float out_max;

const float PQ_M1 = 0.1593017578125;
const float PQ_M2 = 78.84375;
const float PQ_C1 = 0.8359375;
const float PQ_C2 = 18.8515625;
const float PQ_C3 = 18.6875;

vec3 decode(vec3 v, float tf) {
    if (tf < 0.5) {
        return pow(max(v, 0.0), vec3(2.2));
    } else if (tf < 1.5) {
        vec3 p = pow(max(v, 0.0), vec3(1.0 / PQ_M2));
        return pow(max(p - PQ_C1, 0.0) / (PQ_C2 - PQ_C3 * p), vec3(1.0 / PQ_M1));
    } else if (tf < 2.5) {
        return v;
    }
    vec3 lo = v / 12.92;
    vec3 hi = pow((max(v, 0.0) + 0.055) / 1.055, vec3(2.4));
    return mix(lo, hi, step(0.04045, v));
}

vec3 encode(vec3 l, float tf) {
    if (tf < 0.5) {
        return pow(l, vec3(1.0 / 2.2));
    } else if (tf < 1.5) {
        vec3 p = pow(l, vec3(PQ_M1));
        return pow((PQ_C1 + PQ_C2 * p) / (1.0 + PQ_C3 * p), vec3(PQ_M2));
    } else if (tf < 2.5) {
        return l;
    }
    vec3 lo = l * 12.92;
    vec3 hi = 1.055 * pow(l, vec3(1.0 / 2.4)) - 0.055;
    return mix(lo, hi, step(0.0031308, l));
}

vec4 convert(vec4 c) {
    if (c.a <= 0.0) {
        return c;
    }
    vec3 light = color_matrix * decode(c.rgb / c.a, tf_in);
    light = clamp(light, 0.0, out_max) * out_scale;
    return vec4(encode(light, tf_out) * c.a, c.a);
}
#endif

#if defined(DEBUG_FLAGS)
uniform float tint;
#endif

void main() {
    vec4 color = texture2D(tex, v_coords);

#if defined(NO_ALPHA)
    color = vec4(color.rgb, 1.0);
#endif
#if defined(COLOR)
    color = convert(color);
#endif
    color = color * alpha;

#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        color = vec4(0.0, 0.2, 0.0, 0.2) + color * 0.8;
#endif

    gl_FragColor = color;
}