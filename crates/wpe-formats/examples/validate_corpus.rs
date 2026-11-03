//! Read-only validation against an external frozen investigation directory.
//! No original content is linked into this crate or executed.
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    path::Path,
};
use wpe_formats::{
    ErrorKind, jsonc,
    mdl::{Model, SectionData},
    pkg::Package,
    tex::Texture,
};

type AnyResult<T> = Result<T, Box<dyn std::error::Error>>;
fn read_index(root: &Path, name: &str, key: &str) -> AnyResult<Vec<Value>> {
    let v: Value = serde_json::from_slice(&fs::read(root.join("resources").join(name))?)?;
    Ok(v[key].as_array().ok_or("missing index array")?.clone())
}
fn source(row: &Value, packages: &HashMap<String, String>) -> AnyResult<Vec<u8>> {
    let s = &row["source"];
    let path = s["path"]
        .as_str()
        .or_else(|| {
            s["package"]
                .as_str()
                .and_then(|name| packages.get(name).map(String::as_str))
        })
        .ok_or("source path/package identity")?;
    let mut f = File::open(path)?;
    let count = s["bytes"].as_u64().ok_or("source bytes")?;
    if count > 512 * 1024 * 1024 {
        return Err("source exceeds validation allocation policy".into());
    }
    f.seek(SeekFrom::Start(
        s["offset"].as_u64().ok_or("source offset")?,
    ))?;
    let mut data = vec![0; count as usize];
    f.read_exact(&mut data)?;
    Ok(data)
}
fn require(condition: bool, message: impl Into<String>) -> AnyResult<()> {
    if !condition {
        return Err(message.into().into());
    }
    Ok(())
}
fn main() -> AnyResult<()> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("usage: cargo run -p wpe-formats --example validate_corpus -- ANALYSIS_DIRECTORY")?;
    let root = Path::new(&path);
    let mut packages = 0;
    let mut members = 0;
    let mut textures = 0;
    let mut mips = 0;
    let mut decoded = 0;
    let mut unsupported = 0;
    let mut payload_bytes = 0u64;
    let mut rgba_bytes = 0u64;
    let mut models = 0;
    let mut meshes = 0;
    let mut clips = 0;
    let mut failures = Vec::new();
    let package_rows = read_index(root, "packages.json", "packages")?;
    let package_paths = package_rows
        .iter()
        .filter_map(|row| {
            Some((
                row["identity"].as_str()?.to_owned(),
                row["path"].as_str()?.to_owned(),
            ))
        })
        .collect::<HashMap<_, _>>();
    for row in package_rows {
        let id = row["identity"].as_str().unwrap_or("?");
        let check = (|| -> AnyResult<()> {
            let p = Package::parse(fs::read(row["path"].as_str().ok_or("package path")?)?)?;
            let expected = row["members"].as_array().ok_or("members")?;
            require(
                p.members().len() == expected.len(),
                format!("{id}: member count"),
            )?;
            require(
                format!("PKGV{:04}", p.version) == row["magic"],
                format!("{id}: version"),
            )?;
            for (i, (actual, gold)) in p.members().iter().zip(expected).enumerate() {
                require(
                    actual.name == gold["path"]
                        && actual.offset as u64 == gold["offset"]
                        && actual.size as u64 == gold["bytes"],
                    format!("{id}: member {i} index"),
                )?;
                require(
                    p.member_data(i).is_some_and(|b| b.len() == actual.size),
                    format!("{id}: member {i} bounds"),
                )?;
            }
            packages += 1;
            members += p.members().len();
            Ok(())
        })();
        if let Err(e) = check {
            failures.push(format!("{id}: {e}"));
        }
    }
    for row in read_index(root, "textures.json", "textures")? {
        let id = row["identity"].as_str().unwrap_or("?");
        let check = (|| -> AnyResult<()> {
            let t = Texture::parse(&source(&row, &package_paths)?)?;
            let gold = &row["result"];
            let h = &gold["header"];
            let actual = serde_json::to_value(&t.header)?;
            for key in [
                "format",
                "flags",
                "texture_width",
                "texture_height",
                "image_width",
                "image_height",
                "volume_depth",
            ] {
                require(actual[key] == h[key], format!("{id}: header {key}"))?;
            }
            require(
                t.header.average_color as u64 == h["unknown_u32"],
                format!("{id}: average color bits"),
            )?;
            require(
                t.images.len() as u64 == gold["image_count"]
                    && t.image_format as i64 == gold["image_format"]
                    && t.video as u64 == gold["video"],
                format!("{id}: container metadata"),
            )?;
            require(
                t.parsed_bytes as u64 == gold["parsed_bytes"]
                    && t.trailing_bytes.len() as u64 == gold["trailing_bytes"],
                format!("{id}: EOF/trailing"),
            )?;
            let expected = gold["mips"].as_array().ok_or("mip index")?;
            let mut ordinal = 0;
            for (i, image) in t.images.iter().enumerate() {
                for (j, mip) in image.mips.iter().enumerate() {
                    let g = &expected[ordinal];
                    ordinal += 1;
                    mips += 1;
                    require(
                        mip.width as u64 == g["width"]
                            && mip.height as u64 == g["height"]
                            && mip.depth as u64 == g["depth"]
                            && mip.offset as u64 == g["offset"],
                        format!("{id}: mip{i}/{j} fields"),
                    )?;
                    let data = mip.payload()?;
                    payload_bytes += data.len() as u64;
                    require(
                        data.len() as u64 == g["decoded_bytes"],
                        format!("{id}: mip{i}/{j} decompressed size"),
                    )?;
                    match t.decode_rgba8(i, j) {
                        Ok(rgba) => {
                            require(
                                rgba.width == mip.width
                                    && rgba.height == mip.height
                                    && rgba.rgba.len()
                                        == mip.width as usize * mip.height as usize * 4,
                                format!("{id}: decoded extent"),
                            )?;
                            decoded += 1;
                            rgba_bytes += rgba.rgba.len() as u64;
                        }
                        Err(e)
                            if e.kind == ErrorKind::Unsupported
                                && (t.video != 0
                                    || t.header.volume_depth != 1
                                    || mip.depth != 1) =>
                        {
                            unsupported += 1
                        }
                        Err(e) => return Err(e.into()),
                    }
                }
            }
            require(ordinal == expected.len(), format!("{id}: mip count"))?;
            match (&t.sprites, &gold["frames"]) {
                (Some(s), g) if !g.is_null() => require(
                    s.frames.len() as u64 == g["count"],
                    format!("{id}: sprites"),
                )?,
                (None, Value::Null) => {}
                _ => return Err(format!("{id}: sprite presence").into()),
            }
            textures += 1;
            Ok(())
        })();
        if let Err(e) = check {
            failures.push(format!("{id}: {e}"));
        }
    }
    for row in read_index(root, "model-meshes.json", "models")? {
        let id = row["identity"].as_str().unwrap_or("?");
        let check = (|| -> AnyResult<()> {
            let m = Model::parse(&source(&row, &package_paths)?)?;
            let gold = &row["result"];
            require(
                m.version as u64 == gold["version"]
                    && m.meshes.len() as u64 == gold["mesh_count"]
                    && m.parsed_bytes == m.file_bytes
                    && m.unsupported_sections.is_empty(),
                format!("{id}: model framing"),
            )?;
            for (actual, g) in m
                .meshes
                .iter()
                .zip(gold["meshes"].as_array().ok_or("mesh index")?)
            {
                require(
                    actual.vertex_count as u64 == g["vertex_count"]
                        && actual.index_count as u64 == g["u16_index_count"]
                        && actual.vertex_stride as u64 == g["vertex_stride"],
                    format!("{id}: mesh counts/stride"),
                )?;
                require(
                    actual.active_skin_indices_in_bounds != Some(false)
                        && actual.skin_weights_finite != Some(false),
                    format!("{id}: skin validation"),
                )?;
            }
            for section in &m.sections {
                if let SectionData::Animation(c) = &section.data {
                    clips += c.len();
                }
            }
            models += 1;
            meshes += m.meshes.len();
            Ok(())
        })();
        if let Err(e) = check {
            failures.push(format!("{id}: {e}"));
        }
    }
    let mut json_files = 0;
    for row in read_index(root, "json-observations.json", "observations")? {
        let id = row["identity"].as_str().unwrap_or("?");
        match fs::read(row["text_copy"].as_str().ok_or("text path")?)
            .map_err(|e| e.to_string())
            .and_then(|b| jsonc::parse(&b).map_err(|e| e.to_string()))
        {
            Ok(_) => json_files += 1,
            Err(e) => failures.push(format!("{id}: {e}")),
        }
    }
    let mut original_writer_controls = 0;
    for version in ["2.7.3", "2.8.42"] {
        let folder = root.join("resources/runtime-native-v4-files").join(version);
        for name in ["lossless", "solid-lz4"] {
            let bytes = fs::read(folder.join("materials").join(format!("{name}.tex")))?;
            let expected = fs::read(folder.join("materials").join(format!("{name}.rgba")))?;
            let actual = Texture::parse(&bytes)?.decode_rgba8(0, 0)?;
            require(
                actual.rgba == expected,
                format!("original writer {version}/{name}: exact RGBA mismatch"),
            )?;
            original_writer_controls += 1;
        }
        let m = Model::parse(&fs::read(folder.join("models/triangle.mdl"))?)?;
        require(
            m.meshes.len() == 1
                && m.meshes[0].vertex_count == 3
                && m.meshes[0].indices == [0, 1, 2],
            format!("original writer {version}: triangle"),
        )?;
        original_writer_controls += 1;
    }
    let report = json!({"passed":failures.is_empty(),"packages":packages,"members":members,"textures":textures,"mips":mips,
        "decoded_2d_mips":decoded,"explicitly_unsupported_volume_video_mips":unsupported,
        "decompressed_payload_bytes":payload_bytes,"decoded_rgba_bytes":rgba_bytes,
        "models":models,"meshes":meshes,"clips":clips,"json_files":json_files,
        "original_writer_controls":original_writer_controls,"failures":failures,
        "scope":"External corpus parse/decode, index metadata agreement and exact owned compiler RGBA controls. No original runtime/GPU execution, pixel equivalence or artwork redistribution."});
    println!("{}", serde_json::to_string_pretty(&report)?);
    require(report["passed"] == true, "corpus validation failed")
}
