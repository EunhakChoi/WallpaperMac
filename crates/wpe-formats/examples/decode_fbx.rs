//! Decode explicitly supplied files with the bounded independent FBX importer.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    for path in std::env::args_os().skip(1) {
        let path = std::path::PathBuf::from(path);
        let result = (|| -> Result<serde_json::Value, Box<dyn std::error::Error>> {
            if std::fs::metadata(&path)?.len() > 32 * 1024 * 1024 {
                return Err("FBX input exceeds32MiB".into());
            }
            let model = wpe_formats::fbx::decode_to_editable(&std::fs::read(&path)?)?;
            Ok(
                serde_json::json!({"passed":true,"path":path,"shapes":model["shapes"].as_array().unwrap().len(),
                "vertices":model["shapes"].as_array().unwrap().iter().map(|s|s["vertexBuffer"].as_array().unwrap().len()/12).sum::<usize>(),
                "triangles":model["shapes"].as_array().unwrap().iter().map(|s|s["indexBuffer"].as_array().unwrap().len()/3).sum::<usize>(),
                "bones":model["bones"].as_array().unwrap().len(),"clips":model["clips"].as_array().unwrap().len(),"metadata":model["nativeMetadata"]}),
            )
        })();
        println!(
            "{}",
            match result {
                Ok(value) => value,
                Err(error) =>
                    serde_json::json!({"passed":false,"path":path,"error":error.to_string()}),
            }
        );
    }
    Ok(())
}
