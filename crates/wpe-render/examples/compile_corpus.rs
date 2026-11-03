//! Compile every retained stage and every material's finite declared combo/texture profile.
//! External assets are read only; no original shader code is embedded in this crate.
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};
use wpe_render::shader::{CompiledShader, ShaderCompiler, ShaderStage, SourceLibrary};

fn files(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            files(&entry.path(), out)?;
        } else if entry.file_type()?.is_file() {
            out.push(entry.path());
        }
    }
    Ok(())
}
fn resolve(library: &SourceLibrary, owner: &str, shader: &str, ext: &str) -> Option<String> {
    let scope = owner
        .split_once("/materials/")
        .map(|(p, _)| p)
        .unwrap_or_else(|| owner.rsplit_once('/').map_or("", |(p, _)| p));
    [
        format!("{scope}/shaders/{shader}.{ext}"),
        format!("assets/shaders/{shader}.{ext}"),
        format!("shaders/{shader}.{ext}"),
        format!("{shader}.{ext}"),
    ]
    .into_iter()
    .find(|p| library.get(p).is_some())
}
fn compact(shader: &CompiledShader) -> Value {
    let packing = shader.pack_uniforms(&BTreeMap::new());
    json!({"name":shader.name,"stage":shader.stage,"status":"compiled-to-msl","entry_point":shader.entry_point,"msl_bytes":shader.msl.len(),"uniform_size":shader.uniform_size,"default_packing":packing.as_ref().map(|p|p.len()).map_err(ToString::to_string),"bindings":shader.bindings,"attributes":shader.attributes,"source_names":shader.source_names,"profile_notes":shader.profile_notes})
}
fn runtime_profile() -> BTreeMap<String, i32> {
    let mut values = BTreeMap::from([
        ("INSTANCECOUNT".into(), 2),
        ("BLENDROWCOUNT".into(), 1),
        ("BONECOUNT".into(), 128),
        ("AUDIOPROCESSING".into(), 0),
    ]);
    for i in 0..32 {
        values.insert(format!("TEX{i}FORMAT"), 0);
    }
    values
}
fn pass_records<'a>(value: &'a Value, path: String, out: &mut Vec<(String, &'a Value)>) {
    match value {
        Value::Object(v) => {
            if v.get("shader").is_some_and(Value::is_string) {
                out.push((path.clone(), value));
            }
            for (k, v) in v {
                pass_records(v, format!("{path}/{k}"), out);
            }
        }
        Value::Array(v) => {
            for (i, v) in v.iter().enumerate() {
                pass_records(v, format!("{path}/{i}"), out);
            }
        }
        _ => {}
    }
}
fn error_result(error: impl ToString) -> Value {
    json!({"status":"error","diagnostic":error.to_string().chars().take(8000).collect::<String>()})
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root =
        PathBuf::from(std::env::args_os().nth(1).ok_or(
            "usage: compile_corpus SELECTED_TEXT_DIRECTORY [REPORT_JSON] [TEXTURES_JSON]",
        )?);
    let mut library = SourceLibrary::from_directory(&root)?;
    if let Some(path) = std::env::args_os().nth(4) {
        library.insert(
            "requirements/LightingV1.glsl",
            std::fs::read_to_string(path)?,
        );
    }
    let names = library
        .names()
        .filter(|name| {
            [".vert", ".frag", ".geom"]
                .iter()
                .any(|ext| name.ends_with(ext))
        })
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let compiler = ShaderCompiler::new(library);
    let mut results = Vec::new();
    let mut compiled = 0;
    let profile = runtime_profile();
    for name in &names {
        let stage = if name.ends_with(".vert") {
            ShaderStage::Vertex
        } else {
            ShaderStage::Fragment
        };
        if name.ends_with(".geom") {
            results.push(json!({"name":name,"status":"unsupported","stage":"geometry","diagnostic":"Naga has no geometry-stage frontend/backend. CPU billboard/strip expansion is a separate implementation, not geometry shader translation."}));
            continue;
        }
        let mut record = match compiler.compile(name, stage, &BTreeMap::new()) {
            Ok(shader) => {
                compiled += 1;
                compact(&shader)
            }
            Err(error) => {
                let mut record = error_result(error);
                record["name"] = json!(name);
                record["stage"] = json!(stage);
                record
            }
        };
        if record["status"] != "compiled-to-msl" {
            record["representative_runtime_profile"] = match compiler.compile(name, stage, &profile)
            {
                Ok(shader) => compact(&shader),
                Err(error) => error_result(error),
            };
        }
        results.push(record);
    }
    let mut formats = BTreeMap::<String, i32>::new();
    if let Some(path) = std::env::args_os().nth(3) {
        let value: Value = serde_json::from_slice(&std::fs::read(path)?)?;
        for row in value["textures"]
            .as_array()
            .ok_or("textures index missing")?
        {
            if let (Some(name), Some(format)) = (
                row["identity"].as_str(),
                row.pointer("/result/header/format").and_then(Value::as_i64),
            ) {
                formats.insert(name.into(), format as i32);
            }
        }
    }
    let mut paths = Vec::new();
    files(&root, &mut paths)?;
    paths.sort();
    let mut variants = BTreeMap::<String, Value>::new();
    let mut references = Vec::new();
    let mut json_errors = Vec::new();
    for path in paths {
        if !path
            .extension()
            .and_then(|x| x.to_str())
            .is_some_and(|ext| ext == "json" || ext == "jsonc")
        {
            continue;
        }
        let name = path
            .strip_prefix(&root)?
            .to_string_lossy()
            .replace('\\', "/");
        let value = match wpe_formats::jsonc::parse(&std::fs::read(&path)?) {
            Ok(value) => value,
            Err(error) => {
                json_errors.push(json!({"name":name,"diagnostic":error.to_string()}));
                continue;
            }
        };
        let mut passes = Vec::new();
        pass_records(&value, String::new(), &mut passes);
        for (pointer, pass) in passes {
            let shader = pass["shader"].as_str().unwrap();
            let mut defines = profile.clone();
            let scope = name.split_once("/materials/").map(|(p, _)| p).unwrap_or("");
            let mut texture_profile = Vec::new();
            if let Some(textures) = pass["textures"].as_array() {
                for (i, texture) in textures.iter().enumerate() {
                    if let Some(texture) = texture.as_str().or_else(|| texture["name"].as_str()) {
                        let candidates = [
                            format!("{scope}/materials/{texture}.tex"),
                            format!("assets/materials/{texture}.tex"),
                        ];
                        if let Some((source, format)) = candidates
                            .iter()
                            .find_map(|key| formats.get(key).map(|format| (key, *format)))
                        {
                            defines.insert(format!("TEX{i}FORMAT"), format);
                            texture_profile.push(json!({"slot":i,"texture":texture,"format":format,"source":source,"authority":"observed TEX header"}));
                        } else {
                            texture_profile.push(json!({"slot":i,"texture":texture,"format":0,"authority":"representative runtime format, not resolved source"}));
                        }
                    }
                }
            }
            if let Some(combos) = pass["combos"].as_object() {
                for (k, v) in combos {
                    if let Some(n) = v
                        .as_i64()
                        .or_else(|| v.as_bool().map(i64::from))
                        .or_else(|| v.as_f64().map(|v| v as i64))
                    {
                        defines.insert(k.clone(), n as i32);
                    }
                }
            }
            let vertex = resolve(&compiler.library, &name, shader, "vert");
            let fragment = resolve(&compiler.library, &name, shader, "frag");
            let key = serde_json::to_string(&json!([vertex, fragment, defines]))?;
            references.push(json!({"material":name,"pointer":pointer,"shader":shader,"variant":key,"texture_profile":texture_profile}));
            if variants.contains_key(&key) {
                continue;
            }
            let mut record = json!({"vertex":vertex,"fragment":fragment,"defines":defines,"scope":"declared material combos plus observed TEX header formats; remaining runtime counts/formats use explicit representative profile"});
            match (&vertex, &fragment) {
                (Some(vertex), Some(fragment)) => {
                    match compiler.compile_pair(vertex, fragment, &defines) {
                        Ok((v, f)) => {
                            record["status"] = json!("compiled-to-msl");
                            record["stages"] = json!([compact(&v), compact(&f)]);
                        }
                        Err(error) => {
                            record["status"] = json!("error");
                            record["diagnostic"] =
                                json!(error.to_string().chars().take(8000).collect::<String>());
                        }
                    }
                }
                _ => {
                    record["status"] = json!("missing-stage-source");
                }
            }
            variants.insert(key, record);
        }
    }
    let variants_compiled = variants
        .values()
        .filter(|v| v["status"] == "compiled-to-msl")
        .count();
    let report = json!({"scope":"Finite retained shader stages and material declarations. Actual Naga GLSL preprocessing/IR validation/MSL emission; no Apple Metal compiler or GPU execution. Runtime representative profiles are explicitly distinguished from observed material combos/TEX formats.","resources":names.len(),"compiled":compiled,"not_compiled":names.len()-compiled,"representative_runtime_defines":profile,"results":results,"material_references":references,"material_variants":variants,"material_variant_count":variants.len(),"material_variants_compiled":variants_compiled,"json_errors":json_errors});
    let text = serde_json::to_string_pretty(&report)?;
    if let Some(path) = std::env::args_os().nth(2) {
        std::fs::write(path, text)?;
        println!(
            "{}",
            json!({"resources":names.len(),"compiled":compiled,"not_compiled":names.len()-compiled,"material_variant_count":variants.len(),"material_variants_compiled":variants_compiled})
        );
    } else {
        println!("{text}");
    }
    Ok(())
}
