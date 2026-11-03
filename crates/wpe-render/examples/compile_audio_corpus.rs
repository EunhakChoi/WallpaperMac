//! Read retained sources and exercise every inline stereo-audio function branch.
use std::{collections::BTreeMap, path::PathBuf};
use wpe_render::shader::{ShaderCompiler, ShaderStage, SourceLibrary, UniformValue};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .ok_or("usage: compile_audio_corpus SELECTED_TEXT_DIRECTORY OUTPUT_JSON")?,
    );
    let output = PathBuf::from(std::env::args_os().nth(2).ok_or("supply OUTPUT_JSON")?);
    let compiler = ShaderCompiler::new(SourceLibrary::from_directory(&root)?);
    let names = compiler
        .library
        .names()
        .filter(|n| {
            n.ends_with(".vert")
                && compiler
                    .library
                    .get(n)
                    .is_some_and(|s| s.contains("CreateAudioResponse"))
        })
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let mut records = Vec::new();
    for name in &names {
        let fragment = name.strip_suffix(".vert").unwrap().to_owned() + ".frag";
        for mode in 1..=3 {
            for mask in 0..=1 {
                for cursor in 0..=1 {
                    let defines = BTreeMap::from([
                        ("AUDIOPROCESSING".into(), mode),
                        ("MASK".into(), mask),
                        ("USECURSOR".into(), cursor),
                    ]);
                    let mut record =
                        serde_json::json!({"vertex":name,"fragment":fragment,"defines":defines});
                    let result = if compiler.library.get(&fragment).is_some() {
                        compiler
                            .compile_pair(name, &fragment, &defines)
                            .map(|(v, f)| vec![v, f])
                    } else {
                        compiler
                            .compile(name, ShaderStage::Vertex, &defines)
                            .map(|v| vec![v])
                    };
                    match result {
                        Ok(stages) => {
                            let values = BTreeMap::from([
                                (
                                    "g_AudioSpectrum16Left".into(),
                                    UniformValue::Vector((0..16).map(|n| n as f32 / 16.).collect()),
                                ),
                                (
                                    "g_AudioSpectrum16Right".into(),
                                    UniformValue::Vector(
                                        (0..16).map(|n| 1. - n as f32 / 16.).collect(),
                                    ),
                                ),
                            ]);
                            let packing = stages
                                .iter()
                                .map(|s| s.pack_uniforms(&values).map(|b| b.len()))
                                .collect::<wpe_render::Result<Vec<_>>>();
                            match packing {
                                Ok(sizes) => {
                                    record["passed"] = true.into();
                                    record["stages"]=serde_json::json!(stages.iter().zip(sizes).map(|(s,n)|serde_json::json!({"stage":s.stage,"msl_bytes":s.msl.len(),"packed_uniform_bytes":n,"natural_array_argument":s.preprocessed_glsl.contains("float [ 16 ] (")})).collect::<Vec<_>>());
                                }
                                Err(e) => {
                                    record["passed"] = false.into();
                                    record["error"] = e.to_string().into();
                                }
                            }
                        }
                        Err(e) => {
                            record["passed"] = false.into();
                            record["error"] = e.to_string().into();
                        }
                    }
                    records.push(record);
                }
            }
        }
    }
    let failures = records.iter().filter(|r| r["passed"] != true).count();
    let report = serde_json::json!({"passed":!names.is_empty()&&failures==0,"source_files":names,"variants":records.len(),"failures":failures,"records":records,"meaning":"Actual retained inline audio function branches through GLSL/Naga validation/MSL emission and real uniform packing; no original engine or Metal GPU execution."});
    std::fs::write(output, serde_json::to_vec_pretty(&report)?)?;
    println!(
        "{}",
        serde_json::json!({"passed":report["passed"],"sources":report["source_files"].as_array().unwrap().len(),"variants":report["variants"],"failures":failures})
    );
    if failures > 0 {
        return Err("audio corpus has shader failures; inspect report".into());
    }
    Ok(())
}
