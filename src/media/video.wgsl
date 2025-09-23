struct VideoUniforms {
    rect: vec4<f32>, // x, y, w, h (归一化屏幕坐标)
};

@group(0) @binding(0) var video_tex: texture_2d<f32>;
@group(0) @binding(1) var video_sampler: sampler;
@group(0) @binding(2) var<uniform> uniforms: VideoUniforms;

// 定义一个结构体用于在顶点和片元着色器之间传递数据
struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>, // 使用 @location(0) 作为插值器
};

@vertex
fn vs_main(@builtin(vertex_index) idx: u32) -> VertexOutput {
    // 矩形两个三角形的标准化坐标 (同时也是 UV 坐标)
    var quad = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), // top-left
        vec2<f32>(1.0, 0.0), // top-right
        vec2<f32>(0.0, 1.0), // bottom-left
        vec2<f32>(0.0, 1.0), // bottom-left
        vec2<f32>(1.0, 0.0), // top-right
        vec2<f32>(1.0, 1.0)  // bottom-right
    );

    // 这个 uv 就是我们要传递给片元着色器的纹理坐标
    let uv = quad[idx];

    // 计算顶点在归一化屏幕坐标系 [0, 1] 中的位置
    let screen_pos = vec2<f32>(
        uniforms.rect.x + uv.x * uniforms.rect.z,
        uniforms.rect.y + uv.y * uniforms.rect.w
    );

    // 转换到 NDC 坐标系 [-1, 1]
    let ndc = vec2<f32>(
        screen_pos.x * 2.0 - 1.0,
        1.0 - screen_pos.y * 2.0 // Y 轴翻转
    );

    var out: VertexOutput;
    out.clip_position = vec4<f32>(ndc, 0.0, 1.0);
    out.uv = uv; // 将 UV 坐标放入输出结构体

    return out;
}

@fragment
// 输入参数改为我们定义的 VertexOutput 结构体
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // 直接使用从顶点着色器传递并插值好的 UV 坐标
    return textureSample(video_tex, video_sampler, in.uv);
}
