//! Compile the actual generated buffer declarations as C++ and read real packed
//! bytes through them. This checks the host/Metal storage ABI independently of
//! Rust's reflection code. It does not execute or compile an Apple GPU shader.
use std::{collections::BTreeMap, fs, process::Command};
use wpe_render::{
    effect,
    shader::{CompiledShader, ShaderStage, UniformValue as U, compile_glsl},
    smoke,
};

fn verify(case: &str, shader: &CompiledShader, values: &BTreeMap<String, U>, reads: &str) {
    let directory = std::env::temp_dir().join(format!(
        "wpe-metal-uniform-layout-{}-{case}",
        std::process::id()
    ));
    fs::create_dir_all(&directory).unwrap();
    let (declarations, _) = shader
        .msl
        .split_once("\nmetal::float2x2 wpe_inverse(")
        .expect("owned GLSL fixture has the standard prelude after its type declarations");
    let mut cpp = String::from(
        "#include <cstddef>\n#include <cstring>\n#include <cmath>\n#include <cassert>\nnamespace metal { using uint = unsigned int; struct alignas(8) float2 { float x,y; }; struct alignas(16) float3 { float x,y,z,pad; }; struct alignas(16) float4 { float x,y,z,w; }; struct alignas(16) float4x3 { float values[16]; }; }\n",
    );
    for line in declarations.lines() {
        if line.starts_with("#include") {
            continue;
        }
        // Keep the thread overloads as ordinary C++ references. Other Metal
        // address spaces have identical storage, but aren't CPU C++ qualifiers.
        if line.contains("operator[]") {
            if ["constant", "device", "threadgroup"]
                .iter()
                .any(|space| line.contains(space))
            {
                continue;
            }
            cpp.push_str(&line.replace("thread ", ""));
        } else {
            cpp.push_str(line);
        }
        cpp.push('\n');
    }
    cpp.push_str(&format!(
        "static_assert(sizeof(WpeUniforms) == {}, \"uniform block size\");\n",
        shader.uniform_size
    ));
    for field in &shader.uniforms {
        cpp.push_str(&format!(
            "static_assert(offsetof(WpeUniforms, {}) == {}, \"uniform member offset\");\n",
            field.name, field.offset
        ));
    }
    let bytes = shader.pack_uniforms(values).unwrap();
    cpp.push_str("int main() { const unsigned char bytes[] = {");
    for byte in bytes {
        cpp.push_str(&format!("{byte},"));
    }
    cpp.push_str("}; WpeUniforms uniforms{}; std::memcpy(&uniforms, bytes, sizeof(uniforms));\n");
    cpp.push_str(reads);
    cpp.push_str("\n}\n");
    let source = directory.join("layout.cpp");
    let executable = directory.join("layout-check");
    fs::write(&source, cpp).unwrap();
    let result = Command::new(std::env::var_os("CXX").unwrap_or_else(|| "c++".into()))
        .args(["-std=c++14", "-Wall", "-Wextra", "-Werror"])
        .arg(&source)
        .arg("-o")
        .arg(&executable)
        .output()
        .expect("C++ compiler required for the generated Metal buffer ABI regression");
    assert!(
        result.status.success(),
        "{case}: generated Metal storage disagrees with host packing; {}\n{}",
        source.display(),
        String::from_utf8_lossy(&result.stderr)
    );
    let result = Command::new(&executable).output().unwrap();
    assert!(
        result.status.success(),
        "{case}: generated buffer declarations read different values; {}\n{}",
        source.display(),
        String::from_utf8_lossy(&result.stderr)
    );
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn generated_msl_storage_reads_real_audio_skin_bloom_and_nested_uniform_buffers() {
    let indexed = smoke::indexed_frame().unwrap();
    let audio = &indexed.draws.last().unwrap().material;
    verify(
        "audio",
        audio.fragment.as_ref().unwrap(),
        &audio.uniforms.values,
        "assert(uniforms.gain == 0.2f); assert(uniforms.ownedAudioLeft.inner[3] == 0.25f); assert(uniforms.ownedAudioRight.inner[7] == 0.75f);",
    );
    let skin = &indexed
        .draws
        .iter()
        .find(|d| d.vertex_streams.contains_key("a_BlendWeights"))
        .unwrap()
        .material;
    verify(
        "skin",
        skin.vertex.as_ref().unwrap(),
        &skin.uniforms.values,
        "assert(uniforms.g_BonesAlpha.inner[0] == 0.0f); assert(uniforms.g_BonesAlpha.inner[1] == 1.0f);",
    );
    let bloom = effect::lower_separable(&smoke::bloom_frame().unwrap()).unwrap();
    let horizontal =
        effect::draw_call_sized(bloom.passes[0].effect.as_ref().unwrap(), &bloom, 64, 64).unwrap();
    verify(
        "bloom",
        horizontal.material.fragment.as_ref().unwrap(),
        &horizontal.material.uniforms.values,
        "assert(uniforms.g_EffectRadius == 4); assert(uniforms.g_EffectWeights.inner[4] > 0.3f); assert(uniforms.g_EffectSize.x == 64.0f); assert(uniforms.g_EffectSize.y == 64.0f); assert(uniforms.g_EffectThresholdEnabled == 1); assert(uniforms.g_EffectThreshold == 0.2f); assert(uniforms.g_EffectHorizontal == 1);",
    );
    let nested = compile_glsl(
        "nested.frag",
        ShaderStage::Fragment,
        "struct Sample { float weights[3]; vec2 point; }; uniform Sample samples[2]; uniform vec2 points[2]; uniform float tail; void main(){gl_FragColor=vec4(samples[1].weights[2]+samples[1].point.x+points[1].y+tail);}",
        &BTreeMap::new(),
    )
    .unwrap();
    let sample = U::Struct(BTreeMap::from([
        (
            "weights".into(),
            U::Array(vec![U::Float(0.25), U::Float(0.5), U::Float(0.75)]),
        ),
        ("point".into(), U::Vector(vec![2., 3.])),
    ]));
    let values = BTreeMap::from([
        ("samples".into(), U::Array(vec![sample.clone(), sample])),
        (
            "points".into(),
            U::Array(vec![U::Vector(vec![4., 5.]), U::Vector(vec![6., 7.])]),
        ),
        ("tail".into(), U::Float(0.125)),
    ]);
    verify(
        "nested",
        &nested,
        &values,
        "assert(uniforms.samples.inner[1].weights.inner[2] == 0.75f); assert(uniforms.samples.inner[1].point.x == 2.0f); assert(uniforms.points.inner[1].y == 7.0f); assert(uniforms.tail == 0.125f);",
    );
}
