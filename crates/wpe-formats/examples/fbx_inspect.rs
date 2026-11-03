//! Read-only typed DOM diagnostic for the explicitly supplied pinned FBX controls.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    for path in std::env::args_os().skip(1) {
        let data = std::fs::read(&path)?;
        if data.len() > 128 * 1024 * 1024 {
            return Err("FBX input exceeds128MiB".into());
        }
        let settings = fbx_dom::ImportSettings { strict: false };
        let doc = if data.starts_with(b"Kaydara FBX Binary") {
            fbx_dom::Document::from_binary_reader(std::io::Cursor::new(&data), settings)
        } else {
            fbx_dom::Document::from_parser(
                fbxscii::Parser::new(fbxscii::Tokenizer::new(std::io::Cursor::new(&data))),
                settings,
            )
        }
        .map_err(|e| format!("{e:?}"))?;
        let errors = doc
            .objects()
            .filter_map(|r| r.err())
            .map(|e| format!("{e:?}"))
            .collect::<Vec<_>>();
        let count = doc.objects().len();
        let owned: fbx_dom::OwnedDocument = doc.into();
        println!(
            "{}",
            serde_json::json!({"path":std::path::Path::new(&path),"version":owned.fbx_version,"objects":count,"object_errors":errors,"models":owned.models.len(),"meshes":owned.mesh_geometries.len(),"curves":owned.animation_curves.len(),"curve_nodes":owned.animation_curve_nodes.len(),"stacks":owned.animation_stacks.len(),"layers":owned.animation_layers.len(),"clusters":owned.clusters.len(),"skins":owned.skins.len(),"unknown_objects":owned.unknown_objects.iter().map(|o|(&o.type_name,&o.name)).collect::<Vec<_>>(),"unknown_geometry":owned.unknown_geometries.len(),"vertices":owned.mesh_geometries.iter().map(|g|g.vertices.len()).sum::<usize>(),"global":format!("{:?}",owned.global_settings),"node_transforms":owned.models.iter().map(|m|serde_json::json!({"name":m.inner().name,"kind":m.inner().class_name,"properties":format!("{:?}",m.properties())})).collect::<Vec<_>>()})
        );
    }
    Ok(())
}
