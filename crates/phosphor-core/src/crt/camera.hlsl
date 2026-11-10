// The Screen Loop's camera on the GPU: a port of NTSCRT's Metal `howl_camera` and `howl_blur`.
// The CPU reference is `howlaround::camera_cpu`.

cbuffer Camera : register(b0)
{
    float4 axisX, axisY, normal, center, balance, counterRect;
    float4 params;   // aspect, zoom, brightness, contrast
    float4 params2;  // hue cos, hue sin, exposure, TV in view
    float4 keyInfo;  // key level, key invert, center dot normal, unused
};

cbuffer Blur : register(b1)
{
    int2 blurDir;
    float blurSigma;
    int blurRadius;
};

Texture2D<float4> scene : register(t0);
Texture2D<float4> tv : register(t1);
Texture2D<float2> counterMask : register(t2);
Texture2D<float4> blurSrc : register(t3);
SamplerState linearClamp : register(s0);
RWTexture2D<float4> cameraOut : register(u0);
RWTexture2D<float4> blurDst : register(u1);

[numthreads(16, 16, 1)]
void camera(uint3 id : SV_DispatchThreadID)
{
    uint W, H;
    cameraOut.GetDimensions(W, H);
    if (id.x >= W || id.y >= H) return;
    float3 sp = scene.Load(int3(id.xy, 0)).rgb;
    float3 c = sp;
    float aspect = params.x, zoom = params.y;
    float2 uv = (float2(id.xy) + 0.5) / float2(W, H);
    if (params2.w > 0.5) {
        float3 d = float3((uv.x - 0.5) * aspect, uv.y - 0.5, 1.0);
        float denom = dot(d, normal.xyz);
        if (abs(denom) > 1e-6) {
            float t = keyInfo.z / denom;
            if (t > 0.0) {
                float3 p = d * t - center.xyz;
                float sx = dot(p, axisX.xyz) / (zoom * aspect) + 0.5;
                float sy = dot(p, axisY.xyz) / zoom + 0.5;
                float ex = min(sx, 1.0 - sx) * zoom * W;
                float ey = min(sy, 1.0 - sy) * zoom * H;
                float cover = saturate(min(ex, ey) + 0.5);
                if (cover > 0.0) {
                    float3 s = tv.SampleLevel(linearClamp, float2(sx, sy), 0).rgb;
                    s = ((s * params.z - 0.5) * params.w + 0.5) * balance.rgb;
                    float y = dot(s, float3(0.299, 0.587, 0.114));
                    float i = dot(s, float3(0.596, -0.274, -0.322));
                    float q = dot(s, float3(0.211, -0.523, 0.312));
                    float i2 = i * params2.x - q * params2.y;
                    float q2 = i * params2.y + q * params2.x;
                    s = saturate(float3(y + 0.956 * i2 + 0.621 * q2,
                                        y - 0.272 * i2 - 0.647 * q2,
                                        y - 1.106 * i2 + 1.703 * q2));
                    c = lerp(c, s, cover);
                }
            }
        }
    }
    if (keyInfo.x > 0.0) {
        float y = dot(sp, float3(0.299, 0.587, 0.114));
        if (keyInfo.y > 0.5) y = 1.0 - y;
        float level = 1.0 - keyInfo.x;
        c = lerp(c, sp, smoothstep(level - 0.06, level + 0.06, y));
    }
    c *= params2.z;
    float4 r = counterRect;
    if (r.z > r.x && uv.x >= r.x && uv.x <= r.z && uv.y >= r.y && uv.y <= r.w) {
        uint cw, ch;
        counterMask.GetDimensions(cw, ch);
        uint fx = min((uint)((uv.x - r.x) / (r.z - r.x) * cw), cw - 1);
        uint fy = min((uint)((uv.y - r.y) / (r.w - r.y) * ch), ch - 1);
        float2 fe = counterMask.Load(int3(fx, fy, 0));
        float a = max(fe.x, fe.y);
        c = c * (1.0 - a) + fe.x;
    }
    cameraOut[id.xy] = float4(saturate(c), 1.0);
}

[numthreads(16, 16, 1)]
void blur(uint3 id : SV_DispatchThreadID)
{
    uint W, H;
    blurDst.GetDimensions(W, H);
    if (id.x >= W || id.y >= H) return;
    float4 acc = 0.0;
    float wsum = 0.0;
    for (int k = -blurRadius; k <= blurRadius; k++) {
        float w = exp(-0.5 * float(k * k) / (blurSigma * blurSigma));
        int2 p = clamp(int2(id.xy) + blurDir * k, int2(0, 0), int2(int(W) - 1, int(H) - 1));
        acc += blurSrc.Load(int3(p, 0)) * w;
        wsum += w;
    }
    blurDst[id.xy] = acc / wsum;
}
