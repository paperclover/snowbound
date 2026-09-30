// GLSL 1.20 port of crates/draw/src/draw.wgsl. GLSL 1.20 has no flat interpolation, so
// the per-quad shape and stroke arrive interpolated between equal values: equality tests
// on them use a tolerance.
#ifdef VERTEX
attribute vec2 position;
attribute vec2 uv;
attribute vec4 color;
attribute vec2 local;
attribute vec4 shape;
attribute float stroke;
attribute vec2 clip_local;
attribute vec3 clip;
attribute float blur;
#endif
varying vec2 v_uv;
varying vec4 v_color;
varying vec2 v_local;
varying vec4 v_shape;
varying float v_stroke;
varying vec2 v_clip_local;
varying vec3 v_clip;
varying float v_blur;

#ifdef VERTEX
void main() {
    v_uv = uv;
    v_color = color;
    v_local = local;
    v_shape = shape;
    v_stroke = stroke;
    v_clip_local = clip_local;
    v_clip = clip;
    v_blur = blur;
    gl_Position = vec4(position, 0.0, 1.0);
}
#else
uniform sampler2D atlas;

bool same(float a, float b) {
    return abs(a - b) <= 1e-4 * max(1.0, max(abs(a), abs(b)));
}

float ellipse_arc(float angle, vec2 radius) {
    if (same(radius.x, radius.y)) {
        return angle * radius.x;
    }
    // Four-point Gauss-Legendre quadrature.
    vec4 nodes = vec4(-0.86113631, -0.33998104, 0.33998104, 0.86113631);
    vec4 weights = vec4(0.34785485, 0.65214515, 0.65214515, 0.34785485);
    float half_angle = angle * 0.5;
    float scale = max(radius.x, radius.y);
    float sum = 0.0;
    for (int i = 0; i < 4; i++) {
        float t = half_angle * (nodes[i] + 1.0);
        sum += weights[i] * length((radius / scale) * vec2(sin(t), cos(t)));
    }
    return half_angle * sum * scale;
}

vec2 erf(vec2 x) {
    vec2 s = sign(x);
    vec2 a = abs(x);
    vec2 y = 1.0 + (0.278393 + (0.230389 + 0.078108 * (a * a)) * a) * a;
    y *= y;
    return s - s / (y * y);
}

float shadow(vec2 p, vec2 half_size, float corner, float sigma) {
    float start = clamp(-3.0 * sigma, p.y - half_size.y, p.y + half_size.y);
    float end = clamp(3.0 * sigma, p.y - half_size.y, p.y + half_size.y);
    float stride = (end - start) / 4.0;
    float y = start + stride * 0.5;
    float value = 0.0;
    for (int i = 0; i < 4; i++) {
        float delta = min(half_size.y - corner - abs(p.y - y), 0.0);
        float curved = half_size.x - corner + sqrt(max(0.0, corner * corner - delta * delta));
        vec2 integral = 0.5 + 0.5 * erf((p.x + vec2(-curved, curved)) * (0.70710678 / sigma));
        float weight = exp(-y * y / (2.0 * sigma * sigma)) / (2.50662827 * sigma);
        value += (integral.y - integral.x) * weight * stride;
        y += stride;
    }
    return value;
}

// Distance from `p` to the tapered capsule from (-h, 0) to (h, 0) with round ends of radii
// `r0` and `r1` (Inigo Quilez's uneven capsule).
float taper(vec2 p, float h, float r0, float r1) {
    vec2 q = vec2(abs(p.y), p.x + h);
    float span = 2.0 * h;
    float b = (r0 - r1) / max(span, 1e-6);
    float far = length(q - vec2(0.0, span)) - r1;
    if (abs(b) >= 1.0) {
        return min(length(q) - r0, far);
    }
    float a = sqrt(1.0 - b * b);
    float k = dot(q, vec2(-b, a));
    if (k < 0.0) {
        return length(q) - r0;
    }
    if (k > a * span) {
        return far;
    }
    return dot(q, vec2(a, b)) - r0;
}

void main() {
    vec4 color = texture2D(atlas, v_uv) * v_color;
    vec4 shape = v_shape;
    if (v_blur > 1e-6) {
        color *= shadow(v_local, shape.xy, shape.z, v_blur);
    } else if (v_blur < -0.5) {
        color *= clamp(0.5 - taper(v_local, shape.x, shape.y, shape.z), 0.0, 1.0);
    } else if (shape.x > 0.0 && shape.y > 0.0) {
        vec2 q = abs(v_local) - shape.xy + shape.zw;
        float distance;
        if (same(shape.z, shape.w)) {
            distance = length(max(q, vec2(0.0))) + min(max(q.x, q.y), 0.0) - shape.z;
        } else if (all(greaterThan(q, vec2(0.0)))) {
            vec2 radius = max(shape.zw, vec2(0.0001));
            vec2 normalized = q / radius;
            float k0 = length(normalized);
            distance = k0 * (k0 - 1.0) / max(length(normalized / radius), 1e-20);
        } else {
            vec2 edge = abs(v_local) - shape.xy;
            distance = max(edge.x, edge.y);
        }
        float coverage = clamp(0.5 - distance, 0.0, 1.0);
        float width = abs(v_stroke);
        if (width > 1e-6) {
            coverage *= clamp(distance + width + 0.5, 0.0, 1.0);
        }
        if (v_stroke < -1e-6) {
            vec2 radius = max(shape.zw - width * 0.5, vec2(0.0));
            if (radius.x <= 1e-6 || radius.y <= 1e-6) {
                radius = vec2(0.0);
            }
            vec2 straight = shape.xy - width * 0.5 - radius;
            vec2 p = abs(v_local);
            float quarter = straight.x + straight.y + ellipse_arc(1.57079632679, radius);
            float along;
            if (p.y <= straight.y) {
                along = p.y;
            } else if (p.x <= straight.x || radius.x == 0.0) {
                along = quarter - p.x;
            } else {
                float angle = atan((p.y - straight.y) / radius.y, (p.x - straight.x) / radius.x);
                along = straight.y + ellipse_arc(angle, radius);
            }
            if (v_local.x < 0.0) {
                along = v_local.y < 0.0 ? 2.0 * quarter + along : 2.0 * quarter - along;
            } else if (v_local.y < 0.0) {
                along = 4.0 * quarter - along;
            }
            float phase = fract(along / (4.0 * width)) * (4.0 * width);
            coverage *= clamp(width + 0.5 - abs(phase - 2.0 * width), 0.0, 1.0);
        }
        color *= coverage;
    }
    if (v_clip.x > 0.0) {
        vec2 q = abs(v_clip_local) - v_clip.xy + v_clip.z;
        float distance = length(max(q, vec2(0.0))) + min(max(q.x, q.y), 0.0) - v_clip.z;
        color *= clamp(0.5 - distance, 0.0, 1.0);
    }
    gl_FragColor = color;
}
#endif
