// --- Olo: LCD subpixel glyphs --- //
//
// Appended to shaders.wgsl (after `enable dual_source_blending;`) when
// the GPU can blend with a second source colour. Each of a glyph pixel's
// channels is the coverage of that colour stripe; blending gives each
// channel its own factor: text * coverage + under * (1 - coverage).

struct SubpixelOut {
    @location(0) @blend_src(0) color: vec4<f32>,
    @location(0) @blend_src(1) blend: vec4<f32>,
}

@fragment
fn fs_subpixel_sprite(input: MonoSpriteVarying) -> SubpixelOut {
    let sample = textureSample(t_sprite, s_sprite, input.tile_position).rgb;
    var out = SubpixelOut();
    if (any(input.clip_distances < vec4<f32>(0.0))) {
        out.color = vec4<f32>(0.0);
        out.blend = vec4<f32>(0.0);
        return out;
    }
    let rgb = input.color.rgb;
    let coverage = vec3<f32>(
        apply_contrast_and_gamma_correction(sample.r, rgb, grayscale_enhanced_contrast, gamma_ratios),
        apply_contrast_and_gamma_correction(sample.g, rgb, grayscale_enhanced_contrast, gamma_ratios),
        apply_contrast_and_gamma_correction(sample.b, rgb, grayscale_enhanced_contrast, gamma_ratios),
    ) * input.color.a;
    let alpha = max(coverage.r, max(coverage.g, coverage.b));
    out.color = vec4<f32>(rgb * coverage, alpha);
    out.blend = vec4<f32>(coverage, alpha);
    return out;
}
