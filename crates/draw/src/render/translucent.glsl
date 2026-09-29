// GLSL 1.20 twin of translucent.wgsl: the frame, blended in linear light, premultiplied
// again in sRGB for a window server that composites that way.
#ifdef VERTEX
attribute vec2 position;

void main() {
    gl_Position = vec4(position, 0.0, 1.0);
}
#else
uniform sampler2D frame;
uniform vec2 size;

vec3 encode(vec3 linear) {
    return mix(1.055 * pow(linear, vec3(1.0 / 2.4)) - 0.055, linear * 12.92,
        vec3(lessThanEqual(linear, vec3(0.0031308))));
}

void main() {
    vec4 pixel = texture2D(frame, gl_FragCoord.xy / size);
    gl_FragColor = pixel.a == 0.0 ? vec4(0.0) : vec4(encode(pixel.rgb / pixel.a) * pixel.a, pixel.a);
}
#endif
