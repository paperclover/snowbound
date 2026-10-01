// Shader model 4.0 port of draw.wgsl, for Direct3D 11 from feature level 10_0. The render
// module embeds its bytecode, compiled by render/dxbc/compile.ps1.
struct Vertex {
    float4 position : SV_Position;
    float2 uv : TEXCOORD0;
    float4 color : TEXCOORD1;
    float2 local : TEXCOORD2;
    nointerpolation float4 shape : TEXCOORD3;
    nointerpolation float stroke : TEXCOORD4;
    float2 clip_local : TEXCOORD5;
    nointerpolation float3 clip : TEXCOORD6;
    nointerpolation float blur : TEXCOORD7;
};

Texture2D atlas : register(t0);
SamplerState atlas_sampler : register(s0);

Vertex vertex(float2 position : TEXCOORD0, float2 uv : TEXCOORD1, float4 color : TEXCOORD2,
              float2 local : TEXCOORD3, float4 shape : TEXCOORD4, float stroke : TEXCOORD5,
              float2 clip_local : TEXCOORD6, float3 clip : TEXCOORD7, float blur : TEXCOORD8) {
    Vertex output;
    output.position = float4(position, 0.0, 1.0);
    output.uv = uv;
    output.color = color;
    output.local = local;
    output.shape = shape;
    output.stroke = stroke;
    output.clip_local = clip_local;
    output.clip = clip;
    output.blur = blur;
    return output;
}

float ellipse_arc(float angle, float2 radius) {
    if (radius.x == radius.y) {
        return angle * radius.x;
    }
    // Four-point Gauss-Legendre quadrature.
    const float4 nodes = float4(-0.86113631, -0.33998104, 0.33998104, 0.86113631);
    const float4 weights = float4(0.34785485, 0.65214515, 0.65214515, 0.34785485);
    float half_angle = angle * 0.5;
    float scale = max(radius.x, radius.y);
    float sum = 0.0;
    [unroll] for (int i = 0; i < 4; i++) {
        float t = half_angle * (nodes[i] + 1.0);
        sum += weights[i] * length((radius / scale) * float2(sin(t), cos(t)));
    }
    return half_angle * sum * scale;
}

float2 erf(float2 x) {
    float2 s = sign(x);
    float2 a = abs(x);
    float2 y = 1.0 + (0.278393 + (0.230389 + 0.078108 * (a * a)) * a) * a;
    y *= y;
    return s - s / (y * y);
}

float shadow(float2 p, float2 half_size, float corner, float sigma) {
    float start = clamp(-3.0 * sigma, p.y - half_size.y, p.y + half_size.y);
    float end = clamp(3.0 * sigma, p.y - half_size.y, p.y + half_size.y);
    float stride = (end - start) / 4.0;
    float y = start + stride * 0.5;
    float value = 0.0;
    [unroll] for (int i = 0; i < 4; i++) {
        float delta = min(half_size.y - corner - abs(p.y - y), 0.0);
        float curved = half_size.x - corner + sqrt(max(0.0, corner * corner - delta * delta));
        float2 integral = 0.5 + 0.5 * erf((p.x + float2(-curved, curved)) * (0.70710678 / sigma));
        float weight = exp(-y * y / (2.0 * sigma * sigma)) / (2.50662827 * sigma);
        value += (integral.y - integral.x) * weight * stride;
        y += stride;
    }
    return value;
}

float taper(float2 p, float h, float r0, float r1) {
    float2 q = float2(abs(p.y), p.x + h);
    float span = 2.0 * h;
    float b = (r0 - r1) / max(span, 1e-6);
    float far = length(q - float2(0.0, span)) - r1;
    if (abs(b) >= 1.0) {
        return min(length(q) - r0, far);
    }
    float a = sqrt(1.0 - b * b);
    float k = dot(q, float2(-b, a));
    if (k < 0.0) {
        return length(q) - r0;
    }
    if (k > a * span) {
        return far;
    }
    return dot(q, float2(a, b)) - r0;
}

float4 fragment(Vertex input) : SV_Target {
    float4 color = atlas.Sample(atlas_sampler, input.uv) * input.color;
    if (input.blur > 0.0) {
        color *= shadow(input.local, input.shape.xy, input.shape.z, input.blur);
    } else if (input.blur < 0.0) {
        color *= saturate(0.5 - taper(input.local, input.shape.x, input.shape.y, input.shape.z));
    } else if (input.shape.x > 0.0 && input.shape.y > 0.0) {
        float2 q = abs(input.local) - input.shape.xy + input.shape.zw;
        float distance;
        if (input.shape.z == input.shape.w) {
            distance = length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - input.shape.z;
        } else if (all(q > 0.0)) {
            float2 radius = max(input.shape.zw, 0.0001);
            float2 normalized = q / radius;
            float k0 = length(normalized);
            distance = k0 * (k0 - 1.0) / max(length(normalized / radius), 1e-20);
        } else {
            float2 edge = abs(input.local) - input.shape.xy;
            distance = max(edge.x, edge.y);
        }
        float coverage = saturate(0.5 - distance);
        float width = abs(input.stroke);
        if (width > 0.0) {
            coverage *= saturate(distance + width + 0.5);
        }
        if (input.stroke < 0.0) {
            float2 radius = max(input.shape.zw - width * 0.5, 0.0);
            if (any(radius == 0.0)) {
                radius = 0.0;
            }
            float2 straight = input.shape.xy - width * 0.5 - radius;
            float2 p = abs(input.local);
            float quarter = straight.x + straight.y + ellipse_arc(1.57079632679, radius);
            float along;
            if (p.y <= straight.y) {
                along = p.y;
            } else if (p.x <= straight.x || radius.x == 0.0) {
                along = quarter - p.x;
            } else {
                float angle = atan2((p.y - straight.y) / radius.y, (p.x - straight.x) / radius.x);
                along = straight.y + ellipse_arc(angle, radius);
            }
            if (input.local.x < 0.0) {
                along = input.local.y < 0.0 ? 2.0 * quarter + along : 2.0 * quarter - along;
            } else if (input.local.y < 0.0) {
                along = 4.0 * quarter - along;
            }
            float phase = frac(along / (4.0 * width)) * (4.0 * width);
            coverage *= saturate(width + 0.5 - abs(phase - 2.0 * width));
        }
        color *= coverage;
    }
    if (input.clip.x > 0.0) {
        float2 q = abs(input.clip_local) - input.clip.xy + input.clip.z;
        float distance = length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - input.clip.z;
        color *= saturate(0.5 - distance);
    }
    return color;
}
