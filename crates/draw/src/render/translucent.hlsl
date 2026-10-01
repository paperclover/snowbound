// Shader model 4.0 twin of translucent.wgsl: the frame, blended in linear light, premultiplied
// again in sRGB for a window server that composites that way.
Texture2D frame : register(t0);

float4 vertex(uint index : SV_VertexID) : SV_Position {
    float2 corner = float2((index << 1) & 2, index & 2);
    return float4(corner * 2.0 - 1.0, 0.0, 1.0);
}

float3 encode(float3 linear_light) {
    return linear_light <= 0.0031308 ? linear_light * 12.92
                                     : 1.055 * pow(linear_light, 1.0 / 2.4) - 0.055;
}

float4 fragment(float4 at : SV_Position) : SV_Target {
    float4 pixel = frame.Load(int3(at.xy, 0));
    if (pixel.a == 0.0) {
        return 0.0;
    }
    return float4(encode(pixel.rgb / pixel.a) * pixel.a, pixel.a);
}
