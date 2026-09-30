struct Vertex {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) local: vec2<f32>,
    @location(3) @interpolate(flat) shape: vec4<f32>,
    @location(4) @interpolate(flat) stroke: f32,
    @location(5) clip_local: vec2<f32>,
    @location(6) @interpolate(flat) clip: vec3<f32>,
    @location(7) @interpolate(flat) blur: f32,
}

@group(0) @binding(0) var atlas: texture_2d<f32>;
@group(0) @binding(1) var atlas_sampler: sampler;

@vertex
fn vertex(@location(0) position: vec2<f32>, @location(1) uv: vec2<f32>,
          @location(2) color: vec4<f32>, @location(3) local: vec2<f32>,
          @location(4) shape: vec4<f32>, @location(5) stroke: f32,
          @location(6) clip_local: vec2<f32>, @location(7) clip: vec3<f32>,
          @location(8) blur: f32) -> Vertex {
    return Vertex(vec4<f32>(position, 0.0, 1.0), uv, color, local, shape, stroke, clip_local, clip, blur);
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

fn erf(x: vec2<f32>) -> vec2<f32> {
    let s = sign(x);
    let a = abs(x);
    var y = 1.0 + (0.278393 + (0.230389 + 0.078108 * (a * a)) * a) * a;
    y *= y;
    return s - s / (y * y);
}

// Coverage at `p` of a rectangle `half` about the origin with round corners `corner`,
// blurred by a Gaussian of deviation `sigma`: exact across, sampled four times along
// (Evan Wallace, "Fast Rounded Rectangle Shadows").
fn shadow(p: vec2<f32>, half: vec2<f32>, corner: f32, sigma: f32) -> f32 {
    let start = clamp(-3.0 * sigma, p.y - half.y, p.y + half.y);
    let end = clamp(3.0 * sigma, p.y - half.y, p.y + half.y);
    let stride = (end - start) / 4.0;
    var y = start + stride * 0.5;
    var value = 0.0;
    for (var i = 0; i < 4; i += 1) {
        let delta = min(half.y - corner - abs(p.y - y), 0.0);
        let curved = half.x - corner + sqrt(max(0.0, corner * corner - delta * delta));
        let integral = 0.5 + 0.5 * erf((p.x + vec2<f32>(-curved, curved)) * (0.70710678 / sigma));
        let weight = exp(-y * y / (2.0 * sigma * sigma)) / (2.50662827 * sigma);
        value += (integral.y - integral.x) * weight * stride;
        y += stride;
    }
    return value;
}

// Distance from `p` to the tapered capsule from (-h, 0) to (h, 0) with round ends of radii
// `r0` and `r1` (Inigo Quilez's uneven capsule).
fn taper(p: vec2<f32>, h: f32, r0: f32, r1: f32) -> f32 {
    let q = vec2<f32>(abs(p.y), p.x + h);
    let span = 2.0 * h;
    let b = (r0 - r1) / max(span, 1e-6);
    let far = length(q - vec2<f32>(0.0, span)) - r1;
    if abs(b) >= 1.0 {
        return min(length(q) - r0, far);
    }
    let a = sqrt(1.0 - b * b);
    let k = dot(q, vec2<f32>(-b, a));
    if k < 0.0 {
        return length(q) - r0;
    }
    if k > a * span {
        return far;
    }
    return dot(q, vec2<f32>(a, b)) - r0;
}

@fragment
fn fragment(input: Vertex) -> @location(0) vec4<f32> {
    var color = textureSample(atlas, atlas_sampler, input.uv) * input.color;
    if input.blur > 0.0 {
        color *= shadow(input.local, input.shape.xy, input.shape.z, input.blur);
    } else if input.blur < 0.0 {
        color *= clamp(0.5 - taper(input.local, input.shape.x, input.shape.y, input.shape.z), 0.0, 1.0);
    } else if input.shape.x > 0.0 && input.shape.y > 0.0 {
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
