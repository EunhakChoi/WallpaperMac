//! End-to-end owned fixtures: script state must alter submitted geometry/pixels.
use serde_json::{Value, json};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
use wpe_core::{PlayerInput, ScenePlayer, load_project};
use wpe_render::{RgbaImage, cpu::Renderer};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new(objects: Value) -> Self {
        let root = std::env::temp_dir().join(format!(
            "wpe-player-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let f = Self(root);
        f.json("project.json",json!({"type":"scene","file":"scene.json","general":{"properties":{"enabled":{"value":true},"factor":{"value":0.5}}}}));
        f.json("scene.json",json!({"general":{"orthogonalprojection":{"width":16,"height":16},"clearcolor":"0 0 0"},"objects":objects}));
        f.json(
            "models/image.json",
            json!({"width":8,"height":8,"nopadding":true,"material":"materials/image.json"}),
        );
        f.json("materials/image.json",json!({"passes":[{"shader":"genericimage2","textures":["white"],"blending":"translucent"}]}));
        let image = RgbaImage::new(1, 1, vec![255; 4]).unwrap();
        f.write(
            "materials/white.tex",
            wpe_formats::tex::encode_rgba8(&image).unwrap(),
        );
        f
    }
    fn write(&self, name: &str, bytes: impl AsRef<[u8]>) {
        let p = self.0.join(name);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, bytes).unwrap();
    }
    fn json(&self, name: &str, value: Value) {
        self.write(name, serde_json::to_vec(&value).unwrap());
    }
    fn player(&self) -> ScenePlayer {
        ScenePlayer::new(load_project(&self.0, None).unwrap()).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn image(id: u32) -> Value {
    json!({"id":id,"image":"models/image.json","origin":"8 8 0","size":"8 8"})
}
fn pixel(image: &RgbaImage, x: u32, y: u32) -> [u8; 4] {
    image.rgba[((y * image.width + x) * 4) as usize..((y * image.width + x) * 4 + 4) as usize]
        .try_into()
        .unwrap()
}
#[test]
fn unsaved_editor_resources_and_scene_override_reach_the_live_player() {
    use std::{collections::BTreeMap, sync::Arc};
    let f = Fixture::new(json!([image(1)]));
    let mut project = load_project(&f.0, None).unwrap();
    let green = RgbaImage::new(1, 1, vec![0, 255, 0, 255]).unwrap();
    project.vfs.mount_memory_front(BTreeMap::from([
        ("materials/white.tex".into(), Some(Arc::new(wpe_formats::tex::encode_rgba8(&green).unwrap()))),
        ("scene.json".into(), Some(Arc::new(serde_json::to_vec(&json!({
            "general":{"orthogonalprojection":{"width":16,"height":16}},
            "objects":[{"id":1,"image":"models/image.json","size":"4 4","origin":"4 8 0"}]
        })).unwrap()))),
    ])).unwrap();
    let mut player = ScenePlayer::new(project).unwrap();
    let frame = player.advance(0., &PlayerInput::default()).unwrap();
    let output = Renderer::new().render(&frame).unwrap();
    assert_eq!(pixel(&output, 4, 8), [0, 255, 0, 255]);
    assert_eq!(pixel(&output, 10, 8), [0, 0, 0, 255]);
    assert_eq!(
        fs::read(f.0.join("materials/white.tex")).unwrap(),
        wpe_formats::tex::encode_rgba8(&RgbaImage::new(1, 1, vec![255; 4]).unwrap()).unwrap()
    );
}

#[test]
fn shader_environment_uses_previous_pointer_ndc_effect_geometry_and_live_mips() {
    use wpe_render::{TextureData, shader::UniformValue};
    let mut node = image(1);
    node["effects"] = json!([{"file":"effects/owned/effect.json"}]);
    let f = Fixture::new(json!([node]));
    f.json(
        "effects/owned/effect.json",
        json!({"passes":[{"material":"materials/fx.json"}]}),
    );
    f.json(
        "effects/owned/materials/fx.json",
        json!({"passes":[{
            "shader":"effects/owned", "textures":[null,null,null,"_rt_MipMappedFrameBuffer"]
        }]}),
    );
    f.write("effects/owned/shaders/effects/owned.vert", r#"
        uniform mat4 g_ModelViewProjectionMatrix;
        uniform mat4 g_EffectTextureProjectionMatrixInverse;
        uniform vec2 g_PointerPosition;
        attribute vec3 a_Position; attribute vec2 a_TexCoord;
        varying vec2 v_Uv;
        void main(){gl_Position=g_ModelViewProjectionMatrix*vec4(a_Position,1.0);
        v_Uv=a_TexCoord+0.001*(g_EffectTextureProjectionMatrixInverse*vec4(g_PointerPosition,0.,1.)).xy;}
    "#);
    f.write(
        "effects/owned/shaders/effects/owned.frag",
        r#"
        uniform sampler2D g_Texture0; uniform sampler2D g_Texture3;
        uniform float g_Frametime; uniform vec2 g_PointerPositionLast;
        uniform float g_AudioSpectrum16Left[16]; varying vec2 v_Uv;
        void main(){gl_FragColor=texture(g_Texture0,v_Uv)+0.001*texture(g_Texture3,v_Uv)
        +vec4(g_Frametime+g_PointerPositionLast.x+g_AudioSpectrum16Left[0]);}
    "#,
    );
    let mut player = f.player();
    let mut input = PlayerInput {
        cursor: [0.25, 0.75],
        audio_left: vec![0.25; 64],
        ..Default::default()
    };
    player.advance(0.01, &input).unwrap();
    input.cursor = [0.5, 0.6];
    input.buttons = 1;
    let frame = player.advance(0.02, &input).unwrap();
    assert!(frame.issues.is_empty(), "{:?}", frame.issues);
    let fx = frame
        .draws
        .iter()
        .find(|d| d.name.contains(":effect-"))
        .unwrap();
    assert!(
        fx.vertices
            .iter()
            .all(|v| v.position[0].abs() == 1. && v.position[1].abs() == 1.)
    );
    let values = &fx.material.uniforms.values;
    assert!(matches!(&values["g_PointerPosition"],UniformValue::Vector(v) if v==&vec![0.5,0.6]));
    assert!(
        matches!(&values["g_PointerPositionLast"],UniformValue::Vector(v) if v==&vec![0.25,0.75])
    );
    assert!(matches!(&values["g_PointerState"],UniformValue::Vector(v) if v[2]==1.));
    assert!(matches!(&values["g_Frametime"],UniformValue::Float(v) if *v==0.02));
    assert!(
        matches!(&values["g_AudioSpectrum16Left"],UniformValue::Array(v) if v.len()==16 && matches!(v[0],UniformValue::Float(0.25)))
    );
    assert!(
        matches!(&values["g_ModelViewProjectionMatrix"],UniformValue::Matrix{values,..} if values==&wpe_sim::math::Mat4::IDENTITY.0)
    );
    assert!(
        matches!(&values["g_EffectTextureProjectionMatrixInverse"],UniformValue::Matrix{values,..} if values[0]==2. && values[5]==2.)
    );
    let background = fx.material.textures.iter().find(|b| b.slot == 3).unwrap();
    let texture = &frame.textures[&background.texture];
    assert_eq!(texture.mips.len(), 4);
    assert!(matches!(
        texture.mips.last(),
        Some(TextureData::RenderTarget {
            width: 1,
            height: 1
        })
    ));
    for stage in [&fx.material.vertex, &fx.material.fragment]
        .into_iter()
        .flatten()
    {
        assert_eq!(
            stage.pack_uniforms(values).unwrap().len(),
            stage.uniform_size as usize
        );
    }
}

#[test]
fn legacy_float_tex_remains_signed_hdr_data_in_the_submitted_frame() {
    use wpe_render::{TextureData, TextureFormat};
    let f = Fixture::new(json!([image(1)]));
    let bytes = include_bytes!(
        "../../wpe-formats/validation/texture-extension-fixtures/legacy-v4-format-10.tex"
    );
    f.write("materials/velocity.tex", bytes);
    f.json(
        "materials/image.json",
        json!({"passes":[{"shader":"owned.data","textures":["velocity"]}]}),
    );
    f.write("shaders/owned.data.vert", "uniform mat4 g_ModelViewProjectionMatrix; attribute vec3 a_Position;attribute vec2 a_TexCoord;varying vec2 v_Uv;void main(){gl_Position=g_ModelViewProjectionMatrix*vec4(a_Position,1.);v_Uv=a_TexCoord;}");
    f.write("shaders/owned.data.frag", "uniform sampler2D g_Texture0;varying vec2 v_Uv;void main(){gl_FragColor=texture(g_Texture0,v_Uv);}");
    let frame = f.player().advance(0., &PlayerInput::default()).unwrap();
    assert!(frame.issues.is_empty(), "{:?}", frame.issues);
    let binding = &frame.draws[0].material.textures[0];
    let texture = &frame.textures[&binding.texture];
    assert_eq!(texture.format, TextureFormat::Rg16Float);
    assert!(
        matches!(&texture.data, TextureData::Float2D{rgba,..} if rgba.as_slice()==[2.,-1.,0.,1.])
    );
}

#[test]
fn native_logical_jpeg_name_still_resolves_its_compiled_tex_before_direct_image() {
    let f = Fixture::new(json!([image(1)]));
    f.json(
        "materials/image.json",
        json!({"passes":[{"shader":"wpe:textured","textures":["workshop/2328851328/雪花.jpg"]}]}),
    );
    f.write(
        "materials/workshop/2328851328/雪花.jpg.tex",
        wpe_formats::tex::encode_rgba8(&RgbaImage::new(1, 1, vec![0, 255, 0, 255]).unwrap())
            .unwrap(),
    );
    f.write(
        "workshop/2328851328/雪花.jpg",
        b"invalid direct image must not take precedence",
    );
    let frame = f.player().advance(0., &PlayerInput::default()).unwrap();
    assert!(frame.issues.is_empty(), "{:?}", frame.issues);
    assert_eq!(
        pixel(&Renderer::new().render(&frame).unwrap(), 8, 8),
        [0, 255, 0, 255]
    );
}

#[test]
fn hidden_dependency_renders_a_texture_without_painting_it_on_the_scene() {
    let mut hidden = image(1);
    hidden["visible"] = json!(false);
    hidden["origin"] = json!("4 8 0");
    hidden["color"] = json!("0 1 0");
    let mut visible = image(2);
    visible["origin"] = json!("12 8 0");
    visible["image"] = json!("models/dependency.json");
    visible["dependencies"] = json!([1]);
    let f = Fixture::new(json!([hidden, visible]));
    f.json(
        "models/dependency.json",
        json!({"width":8,"height":8,"material":"materials/dependency.json"}),
    );
    f.json(
        "materials/dependency.json",
        json!({"passes":[{"shader":"wpe:textured","textures":["_rt_imageLayerComposite_1_a"]}]}),
    );
    let frame = f.player().advance(0., &PlayerInput::default()).unwrap();
    assert!(frame.issues.is_empty(), "{:?}", frame.issues);
    let output = Renderer::new().render(&frame).unwrap();
    assert_eq!(pixel(&output, 4, 8), [0, 0, 0, 255]);
    assert_eq!(pixel(&output, 12, 8), [0, 255, 0, 255]);
}

#[test]
fn fullscreen_cursor_hooks_and_transient_preview_camera_use_the_visible_geometry() {
    let mut node = image(1);
    node["color"] = json!({"value":"1 0 0","script":"export function cursorClick(){thisLayer.color=new Vec3(0,1,0);}export function update(v){return v;}"});
    let f = Fixture::new(json!([node]));
    f.json(
        "models/image.json",
        json!({"fullscreen":true,"material":"materials/image.json"}),
    );
    let mut player = f.player();
    let mut input = PlayerInput {
        cursor: [0.9, 0.9],
        ..Default::default()
    };
    player.advance(0., &input).unwrap();
    input.buttons = 1;
    player.advance(0., &input).unwrap();
    input.buttons = 0;
    let frame = player.advance(0., &input).unwrap();
    assert_eq!(
        pixel(&Renderer::new().render(&frame).unwrap(), 14, 14),
        [0, 255, 0, 255]
    );
    assert!(player.set_preview_camera(0., [0.; 2]).is_err());
    let scene = player.scene().clone();
    player.set_preview_camera(2., [2., 0.]).unwrap();
    assert_eq!(scene, *player.scene());
}

#[test]
fn procedural_triangle_hit_test_rejects_empty_corners_and_reports_actual_local_point() {
    let f = Fixture::new(json!([{"id":1,"origin":"8 8 1","modeldata":{"shapes":[{
        "vertexFormat":["position","color"],"vertexBuffer":[-5,-5,0,1,0,0,1,5,-5,0,1,0,0,1,0,5,0,1,0,0,1],"indexBuffer":[0,1,2]}]},
        "alpha":{"value":1,"script":"export function cursorClick(e){localStorage.set('clicked',e.localPosition);localStorage.set('world',e.worldPosition);}export function update(v){return v;}"}}]));
    let mut player = f.player();
    let mut input = PlayerInput {
        cursor: [0.75, 0.25],
        ..Default::default()
    };
    player.advance(0., &input).unwrap();
    input.buttons = 1;
    player.advance(0., &input).unwrap();
    input.buttons = 0;
    player.advance(0., &input).unwrap();
    assert!(!player.storage().to_string().contains("clicked"));
    input.cursor = [0.5; 2];
    input.buttons = 1;
    player.advance(0., &input).unwrap();
    input.buttons = 0;
    player.advance(0., &input).unwrap();
    let stored = player.storage().to_string();
    assert!(stored.contains("clicked"), "{stored}");
    assert!(stored.contains("world"));
}

#[test]
fn paused_dirty_render_does_not_run_vm_timers_or_particle_steps() {
    let mut node = image(1);
    node["alpha"] = json!({"value":1,"script":"export function init(){localStorage.set('ticks',0);thisScene.getLayerByID(2).emitParticles(1);return 1;}export function update(v){localStorage.set('ticks',localStorage.get('ticks')+1);return v;}"});
    let f =
        Fixture::new(json!([node,{"id":2,"particle":"particles/moving.json","origin":"8 8 0"}]));
    f.json("particles/moving.json",json!({"maxcount":8,"emitter":[{"name":"boxrandom","rate":0}],"initializer":[{"name":"lifetimerandom","min":5,"max":5},{"name":"velocityrandom","min":"1 2 0","max":"1 2 0"}],"operator":[{"name":"movement"}]}));
    let mut player = f.player();
    let input = PlayerInput::default();
    let before = player.advance(0.1, &input).unwrap();
    let time = player.time();
    let storage = player.storage().clone();
    let scene = player.scene().clone();
    player.set_paused(true);
    let paused = player.advance(0.2, &input).unwrap();
    let current = player.render_current(&input).unwrap();
    assert_eq!(player.time(), time);
    assert_eq!(*player.storage(), storage);
    assert_eq!(*player.scene(), scene);
    assert_eq!(
        before.draws.last().unwrap().vertices,
        paused.draws.last().unwrap().vertices
    );
    assert_eq!(
        before.draws.last().unwrap().vertices,
        current.draws.last().unwrap().vertices
    );
    player.set_preview_camera(2., [0.; 2]).unwrap();
    let zoomed = player.render_current(&input).unwrap();
    assert_ne!(zoomed.draws[0].vertices, paused.draws[0].vertices);
    assert_eq!(*player.storage(), storage);
}

#[test]
fn editable_model_geometry_reaches_pixels_once_and_preserves_original_binary() {
    let bytes = include_bytes!("../../wpe-editor/tests/fixtures/editable-triangle.mdl");
    let mut editable = wpe_editor::model_tools::decode_model_to_editable(bytes).unwrap();
    editable["sourceModel"] = json!("models/triangle.mdl");
    for row in editable["shapes"][0]["vertexBuffer"]
        .as_array_mut()
        .unwrap()
        .chunks_mut(5)
    {
        row[0] = json!(row[0].as_f64().unwrap() + 4.);
    }
    let f = Fixture::new(json!([{"id":1,"modeldata":editable,"origin":"8 8 0"}]));
    f.write("models/triangle.mdl", bytes);
    let frame = f.player().advance(0., &PlayerInput::default()).unwrap();
    assert!(frame.issues.is_empty(), "{:?}", frame.issues);
    assert_eq!(frame.draws.len(), 1);
    let output = Renderer::new().render(&frame).unwrap();
    assert_eq!(pixel(&output, 8, 8), [0, 0, 0, 255]);
    assert_eq!(pixel(&output, 12, 8), [255; 4]);
    assert_eq!(fs::read(f.0.join("models/triangle.mdl")).unwrap(), bytes);
}

#[test]
fn workshop_script_created_descriptor_layer_reaches_the_real_renderer() {
    let mut owner = image(1);
    owner["visible"] = json!(false);
    owner["origin"] = json!({"value":"0 0 0","script":
        "export const __workshopId='42'; export function init(){let layer=thisScene.createLayer('models/bar.json');layer.origin=new Vec3(8,8,0);layer.size=new Vec2(4,4);return new Vec3(0,0,0);}"});
    let f = Fixture::new(json!([owner]));
    f.json(
        "models/workshop/42/bar.json",
        json!({"width":8,"height":8,"nopadding":true,"material":"materials/image.json"}),
    );
    let frame = f.player().advance(0., &PlayerInput::default()).unwrap();
    assert!(frame.issues.is_empty(), "{:?}", frame.issues);
    assert_eq!(frame.draws.len(), 1);
    let output = Renderer::new().render(&frame).unwrap();
    assert_eq!(pixel(&output, 8, 8), [255; 4]);
    assert_eq!(pixel(&output, 4, 8), [0, 0, 0, 255]);
}

#[test]
fn translated_puppet_shader_preserves_skin_alpha_at_its_native_fragment_branch() {
    use wpe_render::{VertexStreamData, shader::UniformValue};
    let f = Fixture::new(json!([image(1)]));
    f.write(
        "models/skin.mdl",
        include_bytes!("../../wpe-editor/tests/fixtures/editable-skinned-triangle.mdl"),
    );
    f.json("models/image.json",json!({"width":16,"height":16,"nopadding":true,"puppet":"models/skin.mdl","material":"materials/image.json"}));
    f.json(
        "materials/image.json",
        json!({"passes":[{"shader":"genericimage2","textures":["white"],"combos":{"LIGHTING":1}}]}),
    );
    f.write("shaders/genericimage2.vert",r#"
        uniform mat4 g_ModelViewProjectionMatrix;
        attribute vec3 a_Position; attribute vec4 a_Color; varying float v_Alpha;
        #if SKINNING
        uniform mat4x3 g_Bones[BONECOUNT];
        attribute uvec4 a_BlendIndices; attribute vec4 a_BlendWeights;
        #endif
        #ifdef SKINNING_ALPHA
        uniform float g_BonesAlpha[BONECOUNT];
        #endif
        void main(){vec3 p=a_Position;v_Alpha=a_Color.a;
        #if SKINNING
        p=(g_Bones[a_BlendIndices.x]*a_BlendWeights.x+g_Bones[a_BlendIndices.y]*a_BlendWeights.y)*vec4(p,1.);
        #endif
        #if SKINNING_ALPHA
        v_Alpha*=g_BonesAlpha[a_BlendIndices.x]*a_BlendWeights.x+g_BonesAlpha[a_BlendIndices.y]*a_BlendWeights.y;
        #endif
        gl_Position=g_ModelViewProjectionMatrix*vec4(p,1.);}
    "#);
    f.write(
        "shaders/genericimage2.frag",
        r#"varying float v_Alpha;void main(){gl_FragColor=vec4(1.,1.,1.,v_Alpha);}"#,
    );
    let mut player = f.player();
    let frame = player.advance(0.25, &PlayerInput::default()).unwrap();
    assert!(frame.issues.is_empty(), "{:?}", frame.issues);
    let draw = &frame.draws[0];
    assert_eq!(draw.vertices[0].color[3], 1.);
    assert!(matches!(&draw.vertex_streams["a_BlendWeights"].data,
        VertexStreamData::Float32(values)if values[..4]==[0.25,0.75,0.,0.]));
    assert!(matches!(&draw.vertex_streams["a_BlendIndices"].data,
        VertexStreamData::Uint32(values)if values[..4]==[0,1,0,0]));
    assert!(matches!(&draw.material.uniforms.values["g_BonesAlpha"],
        UniformValue::Array(values)if matches!(values.as_slice(),[UniformValue::Float(0.),UniformValue::Float(1.)])));
    for stage in [&draw.material.vertex, &draw.material.fragment]
        .into_iter()
        .flatten()
    {
        stage.pack_uniforms(&draw.material.uniforms.values).unwrap();
    }
}

#[test]
fn puppet_channel_material_is_rendered_offscreen_and_replaces_only_main_texture_zero() {
    use wpe_render::{Target, VertexStreamData, shader::UniformValue};
    let f = Fixture::new(json!([image(1)]));
    f.write("models/channels.mdl", owned_channel_model());
    for (name, color) in [("white", [255; 4]), ("channel", [255, 0, 0, 255])] {
        let image = RgbaImage::new(16, 16, (0..256).flat_map(|_| color).collect()).unwrap();
        f.write(
            &format!("materials/{name}.tex"),
            wpe_formats::tex::encode_rgba8(&image).unwrap(),
        );
    }
    f.json("models/image.json",json!({"width":16,"height":16,"nopadding":true,"puppet":"models/channels.mdl","material":"materials/image.json"}));
    f.json("materials/image.json",json!({"passes":[{"shader":"genericimage2","textures":["white","white"],"blending":"translucent"}]}));
    f.json("materials/channels.json",json!({"passes":[{"shader":"owned_channels","textures":["channel","white"],"blending":"translucent"}]}));
    f.write("shaders/owned_channels.vert",r#"
        uniform mat4 g_ModelViewProjectionMatrix;uniform vec4 g_BlendMap[BLENDROWCOUNT];
        attribute vec3 a_Position;attribute vec4 a_TexCoordVec4;attribute uvec4 a_BlendIndices;
        varying vec2 v_Uv;varying float v_Alpha;
        void main(){gl_Position=g_ModelViewProjectionMatrix*vec4(a_Position,1.);
        v_Uv=a_TexCoordVec4.xy;v_Alpha=g_BlendMap[int(a_BlendIndices.x/4u)][int(a_BlendIndices.x%4u)];}
    "#);
    f.write(
        "shaders/owned_channels.frag",
        r#"
        uniform sampler2D g_Texture0;uniform sampler2D g_Texture1;
        varying vec2 v_Uv;varying float v_Alpha;
        void main(){gl_FragColor=texture(g_Texture0,v_Uv);gl_FragColor.a*=v_Alpha;
        gl_FragColor.rgb+=texture(g_Texture1,v_Uv).rgb*0.0001;}
    "#,
    );
    let frame = f.player().advance(0., &PlayerInput::default()).unwrap();
    assert!(frame.issues.is_empty(), "{:?}", frame.issues);
    assert_eq!(
        frame.draws.len(),
        3,
        "base copy, auxiliary channels, one primary mesh"
    );
    let producer = frame
        .passes
        .iter()
        .position(|p| p.name.ends_with(":texture-channels"))
        .unwrap();
    let main = frame
        .passes
        .iter()
        .position(|p| p.name.ends_with(":base") && !p.name.contains("channel"))
        .unwrap();
    assert!(producer < main);
    let Target::Texture(albedo) = frame.passes[producer].target else {
        panic!()
    };
    assert_eq!(
        [
            frame.textures[&albedo].data.dimensions()[0],
            frame.textures[&albedo].data.dimensions()[1]
        ],
        [16, 16]
    );
    let channel = &frame.draws[frame.passes[producer].draws[0]];
    assert!(matches!(&channel.material.uniforms.values["g_BlendMap"],
        UniformValue::Array(rows)if matches!(&rows[0],UniformValue::Vector(values)if values==&vec![0.75,0.,0.,0.])));
    assert!(
        matches!(&channel.vertex_streams["a_BlendIndices"].data,VertexStreamData::Uint32(values)if values.iter().all(|&v|v==0))
    );
    let main = &frame.draws[frame.passes[main].draws[0]];
    assert_eq!(main.indices.len(), 3);
    assert_eq!(
        main.material
            .textures
            .iter()
            .find(|b| b.slot == 0)
            .unwrap()
            .texture,
        albedo
    );
    assert_ne!(
        main.material
            .textures
            .iter()
            .find(|b| b.slot == 1)
            .unwrap()
            .texture,
        albedo
    );
    assert_ne!(
        channel
            .material
            .textures
            .iter()
            .find(|b| b.slot == 1)
            .unwrap()
            .texture,
        albedo
    );
    for draw in &frame.draws {
        for stage in [&draw.material.vertex, &draw.material.fragment]
            .into_iter()
            .flatten()
        {
            stage.pack_uniforms(&draw.material.uniforms.values).unwrap();
        }
    }
}

fn owned_channel_model() -> Vec<u8> {
    use wpe_formats::mdl::{Model, SectionData};
    let original = include_bytes!("../../wpe-editor/tests/fixtures/editable-skinned-triangle.mdl");
    let model = Model::parse(original).unwrap();
    let mut out = b"MDLV0023\0".to_vec();
    for word in [model.global_vertex_format, 1, 2] {
        out.extend(word.to_le_bytes());
    }
    out.push(0); // Main mesh material is the image descriptor's material.
    out.extend(&original[model.meshes[0].offset..model.meshes[0].end]);
    out.extend(b"materials/channels.json\0");
    for word in [2_u32, 1] {
        out.extend(word.to_le_bytes());
    }
    for _ in 0..6 {
        out.extend(0_f32.to_le_bytes());
    }
    out.extend(0x800021_u32.to_le_bytes());
    out.extend((4_u32 * 44).to_le_bytes());
    for (x, y, u, v) in [
        (4_f32, 12_f32, 0_f32, 0_f32),
        (12., 12., 1., 0.),
        (4., 4., 0., 1.),
        (12., 4., 1., 1.),
    ] {
        for value in [x, y, 0.] {
            out.extend(value.to_le_bytes());
        }
        for _ in 0..4 {
            out.extend(0_u32.to_le_bytes());
        }
        for value in [u, v, x / 16., 1. - y / 16.] {
            out.extend(value.to_le_bytes());
        }
    }
    out.extend(12_u32.to_le_bytes());
    for index in [0_u16, 2, 1, 1, 2, 3] {
        out.extend(index.to_le_bytes());
    }
    out.extend([0, 0]);
    out.extend(0_u32.to_le_bytes());
    for section in &model.sections {
        let mut bytes = original[section.offset..section.end].to_vec();
        if let SectionData::Animation(clips) = &section.data {
            let track = &clips[0].tracks[0];
            let at = track.offset + track.bytes - section.offset;
            assert_eq!(&bytes[at..at + 4], &[0; 4]);
            let mut scalar = 1_u32.to_le_bytes().to_vec();
            scalar.extend(0_u32.to_le_bytes());
            scalar.extend(12_u32.to_le_bytes());
            for _ in 0..3 {
                scalar.extend(0.75_f32.to_le_bytes());
            }
            bytes.splice(at..at + 4, scalar);
        }
        let end = (out.len() + bytes.len()) as u32;
        bytes[9..13].copy_from_slice(&end.to_le_bytes());
        out.extend(bytes);
    }
    out.push(0);
    Model::parse(&out).unwrap();
    out
}

#[test]
fn puppet_bone_alpha_changes_the_actual_image_pixels() {
    let f = Fixture::new(json!([image(1)]));
    f.write(
        "models/skin.mdl",
        include_bytes!("../../wpe-editor/tests/fixtures/editable-skinned-triangle.mdl"),
    );
    f.json(
        "models/image.json",
        json!({"width":16,"height":16,"nopadding":true,
        "puppet":"models/skin.mdl","material":"materials/image.json"}),
    );
    let mut player = f.player();
    let before = player.advance(0., &PlayerInput::default()).unwrap();
    assert!(before.issues.is_empty(), "{:?}", before.issues);
    assert_eq!(
        pixel(&Renderer::new().render(&before).unwrap(), 8, 8),
        [255; 4]
    );
    player.advance(0.25, &PlayerInput::default()).unwrap();
    let after = player.advance(0.25, &PlayerInput::default()).unwrap();
    assert!(after.issues.is_empty(), "{:?}", after.issues);
    let output = pixel(&Renderer::new().render(&after).unwrap(), 8, 8);
    assert!(
        output[..3]
            .iter()
            .all(|&value| (127..=128).contains(&value)),
        "{output:?}"
    );
    assert_eq!(output[3], 255);
}

#[test]
fn standalone_editable_bone_animation_reaches_pixels_without_an_mdl() {
    use wpe_sim::math::{Mat4, Quat, Vec3};
    let rest = Mat4::IDENTITY.0;
    let matrices = [
        rest,
        Mat4::trs(Vec3::new(4., 0., 0.), Quat::IDENTITY, Vec3::ONE).0,
        Mat4::trs(Vec3::new(8., 0., 0.), Quat::IDENTITY, Vec3::ONE).0,
    ];
    let editable = json!({
        "sourceModel":"models/owned.fbx",
        "bones":[{"name":"root","parent":-1,"transform":rest}],
        "shapes":[{"vertexFormat":["position"],"vertexBuffer":[-2,-2,0,2,-2,0,0,2,0],
            "indexBuffer":[0,1,2],"weights":[[1],[1],[1]]}],
        "clips":[{"name":"move","fps":1,"frames":2,"mode":"single","boneMatrices":[matrices]}]
    });
    let f =
        Fixture::new(json!([{"id":1,"origin":"4 8 0","rootmotion":false,"modeldata":editable}]));
    let mut player = f.player();
    let before = player.advance(0., &PlayerInput::default()).unwrap();
    assert!(before.issues.is_empty(), "{:?}", before.issues);
    assert_eq!(before.draws.len(), 1);
    let output = Renderer::new().render(&before).unwrap();
    assert_eq!(pixel(&output, 4, 8), [255; 4]);
    assert_eq!(pixel(&output, 8, 8), [0, 0, 0, 255]);
    let mut after = before;
    for _ in 0..4 {
        after = player.advance(0.25, &PlayerInput::default()).unwrap();
    }
    assert!(after.issues.is_empty(), "{:?}", after.issues);
    let output = Renderer::new().render(&after).unwrap();
    assert_eq!(pixel(&output, 4, 8), [0, 0, 0, 255]);
    assert_eq!(pixel(&output, 8, 8), [255; 4]);
    assert_eq!(
        player.scene()["objects"][0]["__animationClips"][0]["name"],
        "move"
    );
}

#[test]
fn camera_path_interpolates_observed_keys_and_preview_override_is_transient() {
    let f = Fixture::new(json!([image(1)]));
    f.json("scene.json",json!({"general":{"orthogonalprojection":{"width":16,"height":16}},
        "camera":{"eye":"8 8 1","center":"8 8 0","up":"0 1 0","paths":["paths/camera.json"]},"objects":[image(1)]}));
    f.json(
        "paths/camera.json",
        json!({"paths":[{"duration":1,"transforms":[
        {"timestamp":0,"eye":"8 8 1","center":"8 8 0","up":"0 1 0"},
        {"timestamp":1,"eye":"12 8 1","center":"12 8 0","up":"0 1 0"}]}]}),
    );
    let mut player = f.player();
    let frame = player.advance(0.25, &PlayerInput::default()).unwrap();
    use wpe_render::shader::UniformValue;
    assert!(
        matches!(&frame.draws[0].material.uniforms.values["g_EyePosition"],UniformValue::Vector(v) if v==&vec![9.,8.,1.])
    );
    let scene = player.scene().clone();
    player
        .set_preview_view(Some(json!({"eye":"20 8 1","center":"20 8 0"})))
        .unwrap();
    let frame = player.render_current(&PlayerInput::default()).unwrap();
    assert!(
        matches!(&frame.draws[0].material.uniforms.values["g_EyePosition"],UniformValue::Vector(v) if v==&vec![20.,8.,1.])
    );
    assert_eq!(*player.scene(), scene);
}

#[test]
fn legacy_reflection_alias_has_an_actual_mirrored_producer_before_main_draws() {
    let f = Fixture::new(json!([]));
    let bytes = include_bytes!("../../wpe-editor/tests/fixtures/editable-triangle.mdl");
    f.write("models/triangle.mdl", bytes);
    f.json(
        "models/reflection.json",
        json!({"fullscreen":true,"material":"materials/reflection.json"}),
    );
    f.json("materials/reflection.json",json!({"passes":[{"shader":"wpe:textured","blending":"normal","textures":["_rt_Reflection"]}]}));
    f.json("scene.json",json!({"general":{"orthogonalprojection":{"width":16,"height":16},"reflectionheight":8},
        "objects":[{"id":1,"model":"models/triangle.mdl","origin":"8 12 0"},{"id":2,"image":"models/reflection.json"}]}));
    let frame = f.player().advance(0., &PlayerInput::default()).unwrap();
    assert!(
        frame
            .issues
            .iter()
            .all(|i| i.code == "owned-reflection-profile"),
        "{:?}",
        frame.issues
    );
    let reflection = frame
        .passes
        .iter()
        .position(|p| p.name == "scene-planar-reflection")
        .unwrap();
    let main = frame
        .passes
        .iter()
        .position(|p| p.draws.iter().any(|&i| frame.draws[i].name == "1"))
        .unwrap();
    assert!(reflection < main);
    assert_eq!(frame.passes[reflection].draws.len(), 1);
    let output = Renderer::new().render(&frame).unwrap();
    assert_eq!(pixel(&output, 8, 12), [255; 4]);
    assert_eq!(pixel(&output, 8, 4), [0, 0, 0, 0]);
}
#[test]
fn hierarchy_user_property_changes_and_live_pixels() {
    let mut child = image(2);
    child["origin"] = json!("0 0 0");
    child["parent"] = json!(1);
    child["color"] = json!("1 0 0");
    child["visible"] = json!({"user":"enabled","value":false});
    let f = Fixture::new(json!([{"id":1,"origin":"8 8 0"},child]));
    let mut player = f.player();
    let frame = player.advance(0., &PlayerInput::default()).unwrap();
    let mut renderer = Renderer::new();
    let image = renderer.render(&frame).unwrap();
    assert_eq!(pixel(&image, 8, 8), [255, 0, 0, 255]);
    player
        .set_user_properties(&json!({"enabled":false}))
        .unwrap();
    let frame = player.advance(0.1, &PlayerInput::default()).unwrap();
    assert_eq!(
        pixel(&renderer.render(&frame).unwrap(), 8, 8),
        [0, 0, 0, 255]
    );
}
#[test]
fn module_globals_see_initial_inputs_and_init_commands_are_consumed() {
    let mut node = image(1);
    node["alpha"] = json!({"value":1,"script":"const f=engine.userProperties.factor; export function init(){localStorage.set('loaded',true);thisScene.getLayerByID(2).emitParticles(3);return f;}"});
    let f = Fixture::new(json!([node,{"id":2,"particle":"particles/p.json","origin":"8 8 0"}]));
    f.json("particles/p.json",json!({"maxcount":8,"emitter":[{"name":"boxrandom","rate":0,"distancemax":"0 0 0"}],"initializer":[{"name":"lifetimerandom","min":2,"max":2},{"name":"sizerandom","min":1,"max":1}]}));
    let mut player = f.player();
    let frame = player.advance(0., &PlayerInput::default()).unwrap();
    assert_eq!(player.scene()["objects"][0]["alpha"]["value"], 0.5);
    assert!(frame.draws.iter().any(|d| d.vertices.len() == 12));
    assert!(
        !player
            .issues()
            .iter()
            .any(|i| i.code == "script-host-command" || i.code == "script-error"),
        "{:?}",
        player.issues()
    );
    assert!(player.storage().to_string().contains("loaded"));
}
#[test]
fn procedural_script_geometry_reaches_the_triangle_rasterizer() {
    let mut node = image(1);
    node["visible"] = json!({"value":false,"script":"export function init(){const model=thisScene.createModelData({vertexFormat:[IModelData.POSITION,IModelData.COLOR],vertexBuffer:[-5,-5,0,1,0,0,1,5,-5,0,1,0,0,1,0,5,0,1,0,0,1],indexBuffer:[0,1,2]});let layer=thisScene.createLayer(model);layer.origin=new Vec3(8,8,0);return false;}"});
    let f = Fixture::new(json!([node]));
    let mut player = f.player();
    let frame = player.advance(0., &PlayerInput::default()).unwrap();
    assert!(frame.issues.is_empty(), "{:?}", frame.issues);
    assert_eq!(frame.draws.len(), 1);
    let rendered = Renderer::new().render(&frame).unwrap();
    assert_eq!(pixel(&rendered, 8, 8), [255, 0, 0, 255]);
}
#[test]
fn mouse_custom_hit_test_and_click_are_targeted_before_update() {
    let mut node = image(1);
    node["color"] = json!({"value":"1 0 0","script":"export function cursorHitTest(e){return e.localPosition.x>0;}export function cursorClick(e){thisLayer.color=new Vec3(0,1,0);}export function update(v){return v;}"});
    let f = Fixture::new(json!([node]));
    let mut player = f.player();
    let mut input = PlayerInput {
        cursor: [0.4, 0.5],
        ..Default::default()
    };
    player.advance(0., &input).unwrap();
    input.buttons = 1;
    player.advance(0., &input).unwrap();
    input.buttons = 0;
    let frame = player.advance(0., &input).unwrap();
    assert_eq!(
        pixel(&Renderer::new().render(&frame).unwrap(), 8, 8),
        [255, 0, 0, 255]
    );
    input.cursor = [0.6, 0.5];
    input.buttons = 1;
    player.advance(0., &input).unwrap();
    input.buttons = 0;
    let frame = player.advance(0., &input).unwrap();
    assert_eq!(
        pixel(&Renderer::new().render(&frame).unwrap(), 8, 8),
        [0, 255, 0, 255]
    );
}
#[test]
fn effect_graph_binds_previous_resamples_and_returns_to_scene() {
    let mut node = image(1);
    node["color"] = json!("0 0 1");
    node["effects"] = json!([{"file":"effects/identity.json","passes":[{},{}]}]);
    let f = Fixture::new(json!([node]));
    f.json("effects/identity.json",json!({"fbos":[{"name":"_rt_half","scale":2}],"passes":[{"material":"materials/effect.json","target":"_rt_half","bind":[{"name":"previous","index":0}]},{"material":"materials/effect.json","bind":[{"name":"_rt_half","index":0}]}]}));
    f.json("materials/effect.json",json!({"passes":[{"shader":"genericimage2","textures":["_rt_previous"],"blending":"normal"}]}));
    let mut player = f.player();
    let frame = player.advance(0., &PlayerInput::default()).unwrap();
    assert!(frame.issues.is_empty(), "{:?}", frame.issues);
    assert!(
        frame
            .textures
            .values()
            .any(|t| t.data.dimensions() == [4, 4, 1])
    );
    assert!(frame.passes.len() >= 6);
    let rendered = Renderer::new().render(&frame).unwrap();
    assert_eq!(pixel(&rendered, 8, 8), [0, 0, 255, 255]);
    assert_eq!(pixel(&rendered, 0, 0), [0, 0, 0, 255]);
}
#[test]
fn real_glsl_vertex_program_receives_local_positions_and_reflected_mvp() {
    let f = Fixture::new(json!([image(1)]));
    f.json(
        "materials/image.json",
        json!({"passes":[{"shader":"owned","textures":["white"]}]}),
    );
    f.write("shaders/owned.vert","uniform mat4 g_ModelViewProjectionMatrix;attribute vec3 a_Position;attribute vec2 a_TexCoord;varying vec2 v_TexCoord;void main(){gl_Position=g_ModelViewProjectionMatrix*vec4(a_Position,1.0);v_TexCoord=a_TexCoord;}");
    f.write("shaders/owned.frag","uniform sampler2D g_Texture0;varying vec2 v_TexCoord;void main(){gl_FragColor=texture2D(g_Texture0,v_TexCoord);}");
    let mut player = f.player();
    let frame = player.advance(0., &PlayerInput::default()).unwrap();
    assert!(frame.issues.is_empty(), "{:?}", frame.issues);
    let draw = &frame.draws[0];
    assert_eq!(draw.vertices[0].position, [-4., 4., 0., 1.]);
    let shader = draw.material.vertex.as_ref().unwrap();
    assert!(shader.msl.contains("vertex"));
    let bytes = shader
        .pack_uniforms(&draw.material.uniforms.values)
        .unwrap();
    assert!(bytes.iter().any(|b| *b != 0));
    assert!(Renderer::new().render(&frame).is_err());
}
#[test]
fn startsilent_sound_and_decoder_feedback_preserve_vm_and_clock() {
    let f = Fixture::new(
        json!([{"id":9,"sound":["sounds/owned.wav"],"playbackmode":"single","startsilent":true}]),
    );
    let mut player = f.player();
    let commands = player.drain_host_commands();
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].kind, "sound.configure");
    assert_eq!(commands[0].arguments["__playing"], false);
    player
        .apply_provider_state(&json!(9), &json!({"__playing":true,"__currentTime":1.25}))
        .unwrap();
    assert_eq!(player.scene()["objects"][0]["__currentTime"], 1.25);
    assert!(player.drain_host_commands().is_empty());
}

#[test]
fn startup_state_and_flags_are_available_to_module_globals() {
    let mut node = image(1);
    node["alpha"] = json!({"value":1,"script":"const saved=localStorage.get('opacity');const flag=engine.isScreensaver();const width=engine.screenResolution.x;export function init(){return saved+engine.userProperties.factor+(flag?0.1:0)+width/10000;}"});
    let f = Fixture::new(json!([node]));
    let mut player = ScenePlayer::new_with_context(
        load_project(&f.0, None).unwrap(),
        wpe_core::ScenePlayerOptions {
            is_wallpaper: false,
            ..Default::default()
        },
        wpe_core::ScenePlayerInitialState {
            storage: json!({"screen":{"opacity":0.2},"global":{}}),
            user_properties: json!({"factor":0.3}),
            input: PlayerInput {
                screen_resolution: [1000., 500.],
                ..Default::default()
            },
        },
    )
    .unwrap();
    player.advance(0., &PlayerInput::default()).unwrap();
    let value = player.scene()["objects"][0]["alpha"]["value"]
        .as_f64()
        .unwrap();
    assert!((value - 0.7).abs() < 1e-6, "{value}");
}

#[test]
fn native_text_raster_patches_current_frame_without_a_second_script_tick() {
    let f = Fixture::new(
        json!([{"id":1,"text":"한글 🐈","font":"Helvetica","origin":"8 8 0","alpha":{"value":1,"script":"export function update(v){shared.frames=(shared.frames||0)+1;localStorage.set('frames',shared.frames);return v;}"}}]),
    );
    let mut player = f.player();
    player.set_native_text_provider(true);
    let mut frame = player.advance(0.1, &PlayerInput::default()).unwrap();
    let raster = RgbaImage::new(6, 4, vec![255; 6 * 4 * 4]).unwrap();
    player.provide_text_raster(&json!(1), raster).unwrap();
    assert_eq!(player.apply_text_rasters(&mut frame).unwrap(), 1);
    assert_eq!(player.storage()["screen"]["frames"], 1);
    let rendered = Renderer::new().render(&frame).unwrap();
    assert_eq!(pixel(&rendered, 8, 8), [255; 4]);
    assert_eq!(pixel(&rendered, 0, 0), [0, 0, 0, 255]);
    player
        .apply_provider_state(&json!(1), &json!({"text":"Changed"}))
        .unwrap();
    assert_eq!(
        player.apply_text_rasters(&mut frame).unwrap(),
        0,
        "a stale glyph raster must not replace new text"
    );
}

#[test]
fn inactive_pointer_cancels_a_press_without_triggering_a_shortcut_click() {
    let mut node = image(1);
    node["color"] = json!({"value":"1 0 0","script":"export function cursorClick(){thisLayer.color=new Vec3(0,1,0);}"});
    let f = Fixture::new(json!([node]));
    let mut player = f.player();
    let mut input = PlayerInput::default();
    player.advance(0., &input).unwrap();
    input.buttons = 1;
    player.advance(0., &input).unwrap();
    input.pointer_active = false;
    input.buttons = 0;
    let frame = player.advance(0., &input).unwrap();
    assert_eq!(
        pixel(&Renderer::new().render(&frame).unwrap(), 8, 8),
        [255, 0, 0, 255]
    );
}

#[test]
fn scoped_effect_resources_persistent_fbos_and_real_clear_function() {
    let mut node = image(1);
    node["color"] = json!("0 0 1");
    node["effects"] =
        json!([{"file":"effects/owned/effect.json","passes":[{"combos":{"WRITE":1}},{},{}]}]);
    node["alpha"] = json!({"value":1,"script":"export function cursorClick(){thisLayer.getEffect(0).executeMaterialFunction('clearState');}"});
    let f = Fixture::new(json!([node]));
    f.json("effects/owned/effect.json",json!({"fbos":[{"name":"_rt_a","width":8,"height":8,"format":"rgba8888","clear":"0 0 0 0","unique":true},{"name":"_rt_b","width":8,"height":8,"format":"rgba8888","clear":"0 0 0 0","unique":true}],"passes":[{"material":"materials/identity.json","target":"_rt_a","conditions":[{"WRITE":1}],"bind":[{"name":"previous","index":0}]},{"command":"copy","source":"_rt_a","target":"_rt_b"},{"material":"materials/identity.json","bind":[{"name":"_rt_b","index":0}]}],"functions":{"clearState":{"action":"clear","fbos":["_rt_a"]}}}));
    f.json(
        "effects/owned/materials/identity.json",
        json!({"passes":[{"shader":"wpe:textured","blending":"normal","textures":[null]}]}),
    );
    let mut player = f.player();
    let mut renderer = Renderer::new();
    let frame = player.advance(0., &PlayerInput::default()).unwrap();
    assert!(frame.issues.is_empty(), "{:?}", frame.issues);
    assert_eq!(
        pixel(&renderer.render(&frame).unwrap(), 8, 8),
        [0, 0, 255, 255]
    );
    let mut effects = player.scene()["objects"][0]["effects"].clone();
    effects[0]["passes"][0]["combos"]["WRITE"] = json!(0);
    player
        .apply_provider_state(&json!(1), &json!({"effects":effects}))
        .unwrap();
    let frame = player.advance(0., &PlayerInput::default()).unwrap();
    assert_eq!(
        pixel(&renderer.render(&frame).unwrap(), 8, 8),
        [0, 0, 255, 255],
        "the previous FBO contents must persist"
    );
    player
        .dispatch(
            "cursorClick",
            json!({"worldPosition":[8,8,0],"localPosition":[0,0,0]}),
        )
        .unwrap();
    let frame = player.advance(0., &PlayerInput::default()).unwrap();
    assert!(
        !frame.issues.iter().any(|i| i.code == "material-action"),
        "{:?}",
        frame.issues
    );
    assert_eq!(
        pixel(&renderer.render(&frame).unwrap(), 8, 8),
        [0, 0, 0, 255],
        "the material clear action must reach actual target pixels"
    );
}
