struct Vertex {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) local: vec2<f32>,
    @location(3) @interpolate(flat) shape: vec4<f32>,
    @location(4) @interpolate(flat) stroke: f32,
    @location(5) clip_local: vec2<f32>,
    @location(6) @interpolate(flat) clip: vec3<f32>,
}

@group(0) @binding(0) var atlas: texture_2d<f32>;
@group(0) @binding(1) var atlas_sampler: sampler;

@vertex
fn vertex(@location(0) position: vec2<f32>, @location(1) uv: vec2<f32>,
          @location(2) color: vec4<f32>, @location(3) local: vec2<f32>,
          @location(4) shape: vec4<f32>, @location(5) stroke: f32,
          @location(6) clip_local: vec2<f32>, @location(7) clip: vec3<f32>) -> Vertex {
    return Vertex(vec4<f32>(position, 0.0, 1.0), uv, color, local, shape, stroke, clip_local, clip);
}

fn ellipse_arc(angle: f32, radius: vec2<f32>) -> f32 {
    if radius.x == radius.y {
        return angle * radius.x;
    }
    // Four-point Gauss-Legendre quadrature.
    let nodes = vec4<f32>(-0.86113631, -0.33998104, 0.33998104, 0.86113631);
    let weights = vec4<f32>(0.34785485, 0.65214515, 0.65214515, 0.34785485);
    let half = angle * 0.5;
    let scale = max(radius.x, radius.y);
    var sum = 0.0;
    for (var i = 0u; i < 4u; i += 1u) {
        let t = half * (nodes[i] + 1.0);
        sum += weights[i] * length((radius / scale) * vec2<f32>(sin(t), cos(t)));
    }
    return half * sum * scale;
}

@fragment
fn fragment(input: Vertex) -> @location(0) vec4<f32> {
    var color = textureSample(atlas, atlas_sampler, input.uv) * input.color;
    if input.shape.x > 0.0 && input.shape.y > 0.0 {
        let q = abs(input.local) - input.shape.xy + input.shape.zw;
        var distance: f32;
        if input.shape.z == input.shape.w {
            distance = length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - input.shape.z;
        } else if all(q > vec2<f32>(0.0)) {
            let radius = max(input.shape.zw, vec2<f32>(0.0001));
            let normalized = q / radius;
            let k0 = length(normalized);
            distance = k0 * (k0 - 1.0) / max(length(normalized / radius), 1e-20);
        } else {
            let edge = abs(input.local) - input.shape.xy;
            distance = max(edge.x, edge.y);
        }
        var coverage = clamp(0.5 - distance, 0.0, 1.0);
        let width = abs(input.stroke);
        if width > 0.0 {
            coverage *= clamp(distance + width + 0.5, 0.0, 1.0);
        }
        if input.stroke < 0.0 {
            var radius = max(input.shape.zw - width * 0.5, vec2<f32>(0.0));
            if any(radius == vec2<f32>(0.0)) {
                radius = vec2<f32>(0.0);
            }
            let straight = input.shape.xy - width * 0.5 - radius;
            let p = abs(input.local);
            let quarter = straight.x + straight.y + ellipse_arc(1.57079632679, radius);
            var along: f32;
            if p.y <= straight.y {
                along = p.y;
            } else if p.x <= straight.x || radius.x == 0.0 {
                along = quarter - p.x;
            } else {
                let angle = atan2((p.y - straight.y) / radius.y, (p.x - straight.x) / radius.x);
                along = straight.y + ellipse_arc(angle, radius);
            }
            if input.local.x < 0.0 {
                along = select(2.0 * quarter - along, 2.0 * quarter + along, input.local.y < 0.0);
            } else if input.local.y < 0.0 {
                along = 4.0 * quarter - along;
            }
            let phase = fract(along / (4.0 * width)) * (4.0 * width);
            coverage *= clamp(width + 0.5 - abs(phase - 2.0 * width), 0.0, 1.0);
        }
        color *= coverage;
    }
    if input.clip.x > 0.0 {
        let q = abs(input.clip_local) - input.clip.xy + input.clip.z;
        let distance = length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - input.clip.z;
        color *= clamp(0.5 - distance, 0.0, 1.0);
    }
    return color;
}
