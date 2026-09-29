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
#endif
varying vec2 v_uv;
varying vec4 v_color;
varying vec2 v_local;
varying vec4 v_shape;
varying float v_stroke;

#ifdef VERTEX
void main() {
    v_uv = uv;
    v_color = color;
    v_local = local;
    v_shape = shape;
    v_stroke = stroke;
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

void main() {
    vec4 color = texture2D(atlas, v_uv) * v_color;
    vec4 shape = v_shape;
    if (shape.x > 0.0 && shape.y > 0.0) {
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
    gl_FragColor = color;
}
#endif
