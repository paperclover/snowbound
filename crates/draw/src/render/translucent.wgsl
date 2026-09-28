@group(0) @binding(0) var frame: texture_2d<f32>;

@vertex
fn vertex(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let corner = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    return vec4<f32>(corner * 2.0 - 1.0, 0.0, 1.0);
}

fn encode(linear: vec3<f32>) -> vec3<f32> {
    return select(1.055 * pow(linear, vec3<f32>(1.0 / 2.4)) - 0.055, linear * 12.92,
        linear <= vec3<f32>(0.0031308));
}

@fragment
fn fragment(@builtin(position) at: vec4<f32>) -> @location(0) vec4<f32> {
    let pixel = textureLoad(frame, vec2<i32>(at.xy), 0);
    if pixel.a == 0.0 {
        return vec4<f32>(0.0);
    }
    return vec4<f32>(encode(pixel.rgb / pixel.a) * pixel.a, pixel.a);
}
