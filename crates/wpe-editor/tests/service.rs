use serde_json::{Value, json};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
use wpe_editor::Editor;
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    root: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "wpe-editor-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        Self { root }
    }
    fn editor(&self) -> Editor {
        Editor::new(
            self.root.clone(),
            json!({"title":"test","type":"scene","file":"scene.json"}),
            json!({"general":{},"objects":[{"id":1,"name":"one","origin":"0 0 0"}]}),
        )
        .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
#[test]
fn property_wrappers_history_transaction_and_autosave_roundtrip() {
    let f = Fixture::new();
    let mut e = f.editor();
    e.begin_transaction().unwrap();
    e.execute("sendAssetPropertyChange",json!([{"id":"1","tag":"main","properties":{"alpha":{"value":0.5,"script":"export function update(v){return v}"},"origin":"10 20 0"}}])).unwrap();
    e.execute("addTextLayer", json!([{"text":"actual text"}]))
        .unwrap();
    e.commit().unwrap();
    assert_eq!(e.snapshot().scene["objects"].as_array().unwrap().len(), 2);
    assert!(e.undo().unwrap());
    assert_eq!(e.snapshot().scene["objects"][0]["origin"], "0 0 0");
    assert_eq!(e.snapshot().scene["objects"].as_array().unwrap().len(), 1);
    assert!(e.redo().unwrap());
    assert!(!e.autosave(9.9).unwrap());
    assert!(e.autosave(0.2).unwrap());
    let scene: Value =
        serde_json::from_slice(&fs::read(f.root.join("scene.json")).unwrap()).unwrap();
    assert_eq!(scene["objects"][1]["text"], "actual text");
    assert_eq!(scene["objects"][0]["alpha"]["value"], 0.5);
    assert!(!e.snapshot().dirty);
}
#[test]
fn rollback_restores_dirty_and_timer() {
    let f = Fixture::new();
    let mut e = f.editor();
    e.begin_transaction().unwrap();
    e.execute("addCamera", json!([])).unwrap();
    e.rollback().unwrap();
    assert!(!e.snapshot().dirty);
    assert!(!e.autosave(100.0).unwrap());
    assert_eq!(e.snapshot().scene["objects"].as_array().unwrap().len(), 1);
}
#[test]
fn failure_is_atomic_and_redo_branch_is_discarded() {
    let f = Fixture::new();
    let mut e = f.editor();
    e.execute("addTextLayer", json!([{}])).unwrap();
    e.undo().unwrap();
    e.execute("addCamera", json!([])).unwrap();
    assert!(!e.redo().unwrap());
    let before = e.snapshot().scene;
    assert!(
        e.execute(
            "sendSceneOrderChange",
            json!([{"from":0,"to":99,"count":1}])
        )
        .is_err()
    );
    assert_eq!(before, e.snapshot().scene);
}
#[test]
fn binary_file_import_is_undoable_and_saved_bytes_match() {
    let f = Fixture::new();
    let mut e = f.editor();
    e.execute(
        "writeProjectFile",
        json!(["sounds/a.wav",{"bytes":[0,255,17,4]}]),
    )
    .unwrap();
    e.save().unwrap();
    assert_eq!(
        fs::read(f.root.join("sounds/a.wav")).unwrap(),
        [0, 255, 17, 4]
    );
    e.undo().unwrap();
    e.save().unwrap();
    assert!(!f.root.join("sounds/a.wav").exists());
    e.redo().unwrap();
    e.save().unwrap();
    assert_eq!(
        fs::read(f.root.join("sounds/a.wav")).unwrap(),
        [0, 255, 17, 4]
    );
}
#[test]
fn image_import_builds_native_descriptor_chain_and_png_export() {
    let f = Fixture::new();
    let mut e = f.editor();
    let pixels = image::RgbaImage::from_raw(2, 1, vec![255, 0, 0, 255, 0, 255, 0, 128]).unwrap();
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(pixels)
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    let result = e
        .execute(
            "importImage",
            json!([{"name":"two.colors.png","bytes":png.into_inner()}]),
        )
        .unwrap();
    assert_eq!(result.value["texture"], "materials/two.colors.tex");
    assert_eq!(
        e.snapshot().scene["objects"][1]["image"],
        "models/two.colors.json"
    );
    e.execute(
        "exportTextureFile",
        json!(["materials/two.colors.tex", "export/out.png"]),
    )
    .unwrap();
    e.save().unwrap();
    let png = image::open(f.root.join("export/out.png"))
        .unwrap()
        .to_rgba8();
    assert_eq!(png.into_raw(), vec![255, 0, 0, 255, 0, 255, 0, 128]);
    let descriptor: Value =
        serde_json::from_slice(&fs::read(f.root.join("models/two.colors.json")).unwrap()).unwrap();
    assert_eq!(descriptor["material"], "materials/two.colors.json");
}
#[test]
fn read_only_asset_mount_becomes_local_copy_and_undo_restores() {
    let f = Fixture::new();
    let asset = Fixture::new();
    fs::create_dir_all(asset.root.join("effects")).unwrap();
    fs::write(asset.root.join("effects/shared.json"), b"{\"passes\":[]}").unwrap();
    let mut e = f.editor();
    e.mount_asset_root(asset.root.clone()).unwrap();
    let read = e
        .execute("readJsonFile", json!(["effects/shared.json"]))
        .unwrap();
    assert_eq!(read.value["passes"], json!([]));
    assert!(!e.snapshot().dirty);
    e.execute(
        "writeDocumentProperty",
        json!(["effects/shared.json", "/name", "edited"]),
    )
    .unwrap();
    e.save().unwrap();
    assert_eq!(
        fs::read(asset.root.join("effects/shared.json")).unwrap(),
        b"{\"passes\":[]}"
    );
    assert!(f.root.join("effects/shared.json").exists());
    e.undo().unwrap();
    assert!(
        e.snapshot().resources["effects/shared.json"]
            .get("name")
            .is_none()
    );
}
#[test]
fn geometry_vertex_removal_remaps_faces_and_actual_weight_smoothing() {
    let f = Fixture::new();
    let mut e = f.editor();
    let data = json!({"shapes":[{"vertexFormat":["position"],"vertexBuffer":[0,0,0,1,0,0,1,1,0,0,1,0],"indexBuffer":[0,1,2,0,2,3],"weights":[[1,0],[0,1],[1,0],[0,1]]}]});
    e.execute(
        "writeDocumentProperty",
        json!(["scene", "/objects/0/modeldata", data]),
    )
    .unwrap();
    e.execute(
        "genericSceneAction",
        json!(["normalizePuppetWarpWeights", 1]),
    )
    .unwrap();
    e.execute("genericSceneAction", json!(["smoothPuppetWarpWeights", 1]))
        .unwrap();
    let w = &e.snapshot().scene["objects"][0]["modeldata"]["shapes"][0]["weights"][0];
    assert!((w[0].as_f64().unwrap() - 0.5).abs() < 1e-6);
    e.execute(
        "genericSceneAction",
        json!(["selectPuppetWarpTopologyVertices", 1, [1]]),
    )
    .unwrap();
    e.execute(
        "genericSceneAction",
        json!(["removePuppetWarpTopologySelectedVertices", 1]),
    )
    .unwrap();
    let scene = e.snapshot().scene;
    assert_eq!(
        scene["objects"][0]["modeldata"]["shapes"][0]["indexBuffer"],
        json!([0, 1, 2])
    );
    assert_eq!(
        scene["objects"][0]["modeldata"]["shapes"][0]["vertexBuffer"]
            .as_array()
            .unwrap()
            .len(),
        9
    );
}
#[test]
fn host_requests_are_pending_until_an_actual_provider_completes() {
    let f = Fixture::new();
    let mut e = f.editor();
    let reply = e
        .execute("queryWorkshop", json!([{"search":"abc"}]))
        .unwrap();
    assert_eq!(reply.value["pending"], true);
    let id = reply.value["request_id"].as_u64().unwrap();
    assert_eq!(reply.events[0].kind, "hostCommand");
    assert_eq!(e.pending_host_requests().len(), 1);
    let reply = e
        .complete_host_request(id, Err("Steam service not configured".into()))
        .unwrap();
    assert_eq!(reply.events[0].kind, "hostCompleted");
    assert_eq!(reply.value["ok"], false);
    assert!(e.complete_host_request(id, Ok(Value::Null)).is_err());
    assert!(!e.snapshot().dirty);
}
#[test]
fn effect_particle_and_clip_documents_change_and_undo() {
    let f = Fixture::new();
    let mut e = f.editor();
    e.execute(
        "addEffect",
        json!([1,{"file":"effects/test.json","name":"effect"}]),
    )
    .unwrap();
    e.execute("modelAction", json!(["addClip",1,{"name":"walk","fps":24}]))
        .unwrap();
    e.execute(
        "createAsset",
        json!(["particles/test.json",{"operator":[]}]),
    )
    .unwrap();
    e.execute(
        "addParticleElement",
        json!(["particles/test.json","operator",{"name":"movement","drag":0.1}]),
    )
    .unwrap();
    e.execute(
        "duplicateParticleElement",
        json!(["particles/test.json", "operator", 0]),
    )
    .unwrap();
    assert_eq!(
        e.snapshot().resources["particles/test.json"]["operator"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert!(e.undo().unwrap());
    assert_eq!(
        e.snapshot().resources["particles/test.json"]["operator"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(e.snapshot().scene["objects"][0]["clips"][0]["name"], "walk");
}
#[test]
fn obj_import_is_real_triangulation_with_negative_indices() {
    let f = Fixture::new();
    let mut e = f.editor();
    let obj = b"v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nf -4 -3 -2 -1\n";
    let reply = e
        .execute(
            "importModel",
            json!([{"name":"quad.obj","bytes":obj.to_vec()}]),
        )
        .unwrap();
    assert_eq!(
        reply.value["modeldata"]["shapes"][0]["indexBuffer"],
        json!([0, 1, 2, 0, 2, 3])
    );
}
#[test]
fn path_traversal_and_symlinks_cannot_write_outside_project() {
    let f = Fixture::new();
    let mut e = f.editor();
    for name in ["../escape", "/tmp/escape", "C:/escape", "a\\b"] {
        assert!(e.execute("writeProjectFile", json!([name, "bad"])).is_err());
    }
    #[cfg(unix)]
    {
        let outside = Fixture::new();
        std::os::unix::fs::symlink(&outside.root, f.root.join("escape")).unwrap();
        assert!(
            e.execute("writeProjectFile", json!(["escape/out", "bad"]))
                .is_err()
        );
    }
}
#[test]
fn all_original_methods_have_an_explicit_route() {
    let f = Fixture::new();
    for method in wpe_editor::surface::METHODS {
        let mut e = f.editor();
        if let Err(error) = e.execute(method, json!([])) {
            assert_ne!(error.code, "unknown-command", "missing method {method}");
        }
    }
}
#[test]
fn paint_changes_real_texture_pixels_and_whole_gesture_is_one_undo() {
    let f = Fixture::new();
    let mut e = f.editor();
    let image = wpe_formats::RgbaImage {
        width: 2,
        height: 2,
        rgba: vec![0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255],
    };
    let tex = wpe_formats::tex::encode_rgba8(&image).unwrap();
    e.execute(
        "writeProjectFile",
        json!(["materials/brush.tex",{"bytes":tex}]),
    )
    .unwrap();
    e.execute(
        "createAsset",
        json!(["materials/brush.json",{"passes":[{"textures":["brush"]}]}]),
    )
    .unwrap();
    e.execute("beginPaint", json!([1, "materials/brush.json", 0, -1, {}]))
        .unwrap();
    e.execute(
        "paintStroke",
        json!([{"x":0.5,"y":0.5,"radius":0.5,"color":[1,0,0,1]}]),
    )
    .unwrap();
    e.execute(
        "paintStroke",
        json!([{"x":1.5,"y":1.5,"radius":0.5,"color":[0,1,0,1]}]),
    )
    .unwrap();
    e.execute("endPaint", json!([])).unwrap();
    e.save().unwrap();
    let after =
        wpe_formats::tex::Texture::parse(&fs::read(f.root.join("materials/brush.tex")).unwrap())
            .unwrap()
            .decode_rgba8(0, 0)
            .unwrap();
    assert_eq!(&after.rgba[0..4], &[255, 0, 0, 255]);
    assert_eq!(&after.rgba[12..16], &[0, 255, 0, 255]);
    e.undo().unwrap();
    e.save().unwrap();
    let before =
        wpe_formats::tex::Texture::parse(&fs::read(f.root.join("materials/brush.tex")).unwrap())
            .unwrap()
            .decode_rgba8(0, 0)
            .unwrap();
    assert_eq!(before.rgba, image.rgba);
}
#[test]
fn particle_editor_resource_handles_apply_actual_document_edits() {
    let f = Fixture::new();
    let mut e = f.editor();
    e.execute("createAsset", json!(["particles/a.json",{"operator":[]}]))
        .unwrap();
    let loaded = e
        .execute("loadParticleInEditor", json!(["particles/a.json", {}]))
        .unwrap();
    let id = loaded.value["id"].clone();
    let added = e
        .execute(
            "addParticleElement",
            json!([id,"operator",{"name":"movement"}]),
        )
        .unwrap();
    e.execute(
        "removeParticleElement",
        json!([id, "operator", added.value["id"]]),
    )
    .unwrap();
    assert_eq!(
        e.snapshot().resources["particles/a.json"]["operator"],
        json!([])
    );
}
#[test]
fn undo_to_saved_content_clears_dirty_and_preview_seek_does_not_save_it() {
    let f = Fixture::new();
    let mut e = f.editor();
    e.execute("writeDocumentProperty",json!(["scene","/objects/0/alpha",{"value":1,"animation":{"c0":[{"frame":0,"value":0},{"frame":10,"value":1}],"options":{"fps":10,"length":10}}}])).unwrap();
    e.save().unwrap();
    e.execute(
        "sendAssetPropertyChange",
        json!([{"id":1,"properties":{"name":"changed"}}]),
    )
    .unwrap();
    assert!(e.snapshot().dirty);
    e.undo().unwrap();
    assert!(!e.snapshot().dirty);
    e.execute(
        "setAnimationPreviewFrame",
        json!([{"id":1,"key":"alpha","frame":5}]),
    )
    .unwrap();
    assert_eq!(
        e.preview_scene()["objects"][0]["alpha"]["animation"]["__previewFrame"],
        5.0
    );
    assert!(
        e.snapshot().scene["objects"][0]["alpha"]["animation"]
            .get("__previewFrame")
            .is_none()
    );
    assert!(!e.snapshot().dirty);
}
#[test]
fn failed_save_preparation_leaves_no_partial_output_or_temporary_files() {
    let f = Fixture::new();
    let mut e = f.editor();
    fs::create_dir_all(f.root.join("z/conflict")).unwrap();
    e.execute("writeProjectFile", json!(["a/file.txt", "first"]))
        .unwrap();
    e.execute("writeProjectFile", json!(["z/conflict", "invalid-file"]))
        .unwrap();
    assert!(e.save().is_err());
    assert!(!f.root.join("a/file.txt").exists());
    fn scan(path: &std::path::Path) {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            assert!(
                !path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .ends_with(".tmp")
            );
            if path.is_dir() {
                scan(&path)
            }
        }
    }
    scan(&f.root);
    assert!(e.snapshot().dirty);
}
#[test]
fn topology_insertion_and_decimation_produce_actual_valid_triangle_buffers() {
    let f = Fixture::new();
    let mut e = f.editor();
    let data = json!({"shapes":[{"vertexFormat":["position"],"vertexBuffer":[0,0,0,1,0,0,1,1,0,0,1,0],"indexBuffer":[0,1,2,0,2,3]}]});
    e.execute(
        "writeDocumentProperty",
        json!(["scene", "/objects/0/modeldata", data]),
    )
    .unwrap();
    e.execute(
        "genericSceneAction",
        json!(["addPuppetWarpTopologyVertices", 1, [0.5, 0.5, 0]]),
    )
    .unwrap();
    let scene = e.snapshot().scene;
    assert_eq!(
        scene["objects"][0]["modeldata"]["shapes"][0]["vertexBuffer"]
            .as_array()
            .unwrap()
            .len(),
        15
    );
    assert_eq!(
        scene["objects"][0]["modeldata"]["shapes"][0]["indexBuffer"]
            .as_array()
            .unwrap()
            .len(),
        12
    );
    e.execute(
        "genericSceneAction",
        json!(["reducePuppetWarpTopologyVertices",1,{"targetCount":3}]),
    )
    .unwrap();
    let scene = e.snapshot().scene;
    let shape = &scene["objects"][0]["modeldata"]["shapes"][0];
    assert_eq!(shape["vertexBuffer"].as_array().unwrap().len(), 9);
    assert!(
        shape["indexBuffer"]
            .as_array()
            .unwrap()
            .iter()
            .all(|i| i.as_u64().unwrap() < 3)
    );
    assert!(e.undo().unwrap());
    assert_eq!(
        e.snapshot().scene["objects"][0]["modeldata"]["shapes"][0]["vertexBuffer"]
            .as_array()
            .unwrap()
            .len(),
        15
    );
}
#[test]
fn opening_a_shared_edge_really_splits_vertex_islands() {
    let f = Fixture::new();
    let mut e = f.editor();
    e.execute("writeDocumentProperty",json!(["scene","/objects/0/modeldata",{"shapes":[{"vertexFormat":["position"],"vertexBuffer":[0,0,0,1,0,0,1,1,0,0,1,0],"indexBuffer":[0,1,2,0,2,3]}]}])).unwrap();
    e.execute(
        "genericSceneAction",
        json!(["markPuppetWarpTopologyEdgesToOpen", 1, [[0, 2]]]),
    )
    .unwrap();
    let selected = e
        .execute("genericSceneAction", json!(["selectVertexIsland", 1, 0]))
        .unwrap();
    assert_eq!(selected.value.as_array().unwrap().len(), 3);
    assert_eq!(
        e.snapshot().scene["objects"][0]["modeldata"]["shapes"][0]["vertexBuffer"]
            .as_array()
            .unwrap()
            .len(),
        18
    );
}
#[test]
fn depth_auto_and_reference_reset_then_interior_retriangulation() {
    let f = Fixture::new();
    let mut e = f.editor();
    let data = json!({"shapes":[{"vertexFormat":["position"],"vertexBuffer":[0,0,2,1,0,2,1,1,2,0,1,2,0.5,0.5,0],"indexBuffer":[0,1,4,1,2,4,2,3,4,3,0,4]}]});
    e.execute(
        "writeDocumentProperty",
        json!(["scene", "/objects/0/modeldata", data]),
    )
    .unwrap();
    e.execute(
        "genericSceneAction",
        json!(["selectPuppetWarpTopologyVertices", 1, [4]]),
    )
    .unwrap();
    e.execute(
        "genericSceneAction",
        json!(["puppetWarpSelectedVerticesDepthAuto", 1]),
    )
    .unwrap();
    assert_eq!(
        e.snapshot().scene["objects"][0]["modeldata"]["shapes"][0]["vertexBuffer"][14],
        2.0
    );
    e.execute(
        "genericSceneAction",
        json!(["resetPuppetWarpSelectedVertices", 1]),
    )
    .unwrap();
    assert_eq!(
        e.snapshot().scene["objects"][0]["modeldata"]["shapes"][0]["vertexBuffer"][14],
        0
    );
    e.execute(
        "genericSceneAction",
        json!(["removePuppetWarpTopologyInnerVertices", 1]),
    )
    .unwrap();
    let scene = e.snapshot().scene;
    let shape = &scene["objects"][0]["modeldata"]["shapes"][0];
    assert_eq!(shape["vertexBuffer"].as_array().unwrap().len(), 12);
    assert_eq!(shape["indexBuffer"].as_array().unwrap().len(), 6);
}
