use serde_json::{Value, json};
use std::collections::BTreeMap;
use wpe_script::{FrameInput, RuntimeOptions, SceneRuntime, collect_bindings};

fn fixture(source: &str) -> Value {
    json!({"general":{"clearcolor":"0 0 0"},"objects":[{"id":1,"name":"owned","origin":"1 2 3","angles":"0 0 0","scale":"1 1 1","color":"1 1 1","alpha":{"value":1,"script":source}}]})
}
fn runtime(source: &str) -> SceneRuntime {
    let scene = fixture(source);
    let bindings = collect_bindings(&scene);
    let mut runtime = SceneRuntime::new(scene, BTreeMap::new(), RuntimeOptions::default()).unwrap();
    for binding in bindings {
        runtime.attach(binding).unwrap()
    }
    runtime
}
fn alpha(scene: &Value) -> f64 {
    scene["objects"][0]["alpha"]["value"].as_f64().unwrap()
}
fn near(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-5, "{a} != {b}")
}

#[test]
fn shared_values_keep_vector_matrix_identity_and_callable_host_effects() {
    let mut scene = fixture(
        "export function init(v){shared.vector=new Vec3(2,4,6);shared.matrix=Mat4.fromTranslation(new Vec3(3,0,0));shared.identity=shared.vector;shared.cycle={};shared.cycle.self=shared.cycle;shared.call=()=>{thisLayer.play();localStorage.set('owner',thisLayer.name);return shared.vector.mix(new Vec3(6,8,10),0.25)};return v}",
    );
    scene["objects"][0]["color"] = json!({"value":"1 1 1","script":"export function update(v){if(shared.vector!==shared.identity||shared.cycle.self!==shared.cycle)throw Error('identity lost');thisLayer.origin=shared.matrix.translation();return shared.call()}"});
    let bindings = collect_bindings(&scene);
    let mut r = SceneRuntime::new(scene, BTreeMap::new(), RuntimeOptions::default()).unwrap();
    for binding in bindings {
        r.attach(binding).unwrap()
    }
    r.initialize().unwrap();
    let output = r.step(FrameInput::default()).unwrap();
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert_eq!(output.scene["objects"][0]["color"]["value"], "3 5 7");
    assert_eq!(output.scene["objects"][0]["origin"], "3 0 0");
    assert_eq!(output.storage["screen"]["owner"], "owned");
    assert!(output.commands.iter().any(|c| c.kind == "layer.play"));
}

#[test]
fn resource_scope_uses_exported_workshop_id_and_wraps_image_descriptors() {
    let source = "export let __workshopId='scoped';export function init(v){let a=thisScene.createLayer('models/bar.json');let b=thisScene.createLayer('models/direct.json');let c=thisScene.createLayer('models/mesh.mdl');return v}";
    let scene = fixture(source);
    let mut options = RuntimeOptions::default();
    options.assets.insert(
        "models/workshop/scoped/bar.json".into(),
        json!({"width":4,"height":7,"material":"materials/test.json"}),
    );
    options
        .assets
        .insert("models/direct.json".into(), json!({"width":2,"height":3}));
    options.assets.insert(
        "models/workshop/scoped/direct.json".into(),
        json!({"width":9,"height":9}),
    );
    options
        .resource_names
        .insert("models/workshop/scoped/mesh.mdl".into());
    let bindings = collect_bindings(&scene);
    let mut r = SceneRuntime::new(scene, BTreeMap::new(), options).unwrap();
    for b in bindings {
        r.attach(b).unwrap()
    }
    let out = r.initialize().unwrap();
    assert!(out.diagnostics.is_empty(), "{:?}", out.diagnostics);
    assert_eq!(
        out.scene["objects"][1]["image"],
        "models/workshop/scoped/bar.json"
    );
    assert_eq!(out.scene["objects"][1]["size"], "4 7");
    assert_eq!(out.scene["objects"][2]["image"], "models/direct.json");
    assert_eq!(out.scene["objects"][2]["size"], "2 3");
    assert_eq!(
        out.scene["objects"][3]["model"],
        "models/workshop/scoped/mesh.mdl"
    );
    assert!(out.scene["objects"][1].get("material").is_none());
}

#[test]
fn dynamic_layer_creation_coalesces_array_patches_without_losing_final_order() {
    let mut r = runtime(
        "export function init(v){for(let i=0;i<512;i++){let a=thisScene.createLayer({name:'bar'+i,alpha:i});thisScene.sortLayer(a,1)}return v}",
    );
    let out = r.initialize().unwrap();
    assert!(out.diagnostics.is_empty(), "{:?}", out.diagnostics);
    assert_eq!(out.scene["objects"].as_array().unwrap().len(), 513);
    assert_eq!(out.scene["objects"][1]["name"], "bar511");
    assert_eq!(out.scene["objects"][512]["name"], "bar0");
    let replacements = out
        .patches
        .iter()
        .filter(|p| p.path == "/objects")
        .collect::<Vec<_>>();
    assert_eq!(replacements.len(), 1);
    assert_eq!(replacements[0].value.as_array().unwrap().len(), 513);
    assert_eq!(
        out.commands
            .iter()
            .filter(|c| c.kind == "scene.createLayer")
            .count(),
        512
    );
}

#[test]
fn actual_ecmascript_exports_persist_and_apply_bound_return() {
    let mut r = runtime(
        "let count=0; export function init(v){return v+1} export function update(v){count++;return v+count}",
    );
    assert_eq!(alpha(&r.initialize().unwrap().scene), 2.0);
    assert_eq!(alpha(&r.step(FrameInput::default()).unwrap().scene), 3.0);
    assert_eq!(alpha(&r.step(FrameInput::default()).unwrap().scene), 5.0);
}
#[test]
fn first_step_keeps_init_commands_and_patches() {
    let mut r = runtime(
        "export function init(v){thisLayer.origin=new Vec3(4,5,6);thisLayer.play();return 2}export function update(v){return v}",
    );
    let o = r.step(FrameInput::default()).unwrap();
    assert_eq!(alpha(&o.scene), 2.0);
    assert!(o.patches.iter().any(|p| p.path.ends_with("origin")));
    assert!(o.commands.iter().any(|c| c.kind == "layer.play"));
}
#[test]
fn imports_are_real_case_folded_cached_modules() {
    let mut modules = BTreeMap::new();
    modules.insert("scripts/helper.js".into(), "export const answer=3;".into());
    let source = "import {mix} from 'WEMath';import {answer} from './HELPER.js';export function update(v){return mix(v,answer,0.5)}";
    let scene = fixture(source);
    let b = collect_bindings(&scene).remove(0);
    let mut r = SceneRuntime::new(scene, modules, RuntimeOptions::default()).unwrap();
    r.attach(b).unwrap();
    near(alpha(&r.step(FrameInput::default()).unwrap().scene), 2.0);
}
#[test]
fn imports_cannot_read_network_or_filesystem() {
    let scene =
        fixture("import x from 'https://example.invalid/a.js';export function update(v){return x}");
    let b = collect_bindings(&scene).remove(0);
    let mut r = SceneRuntime::new(scene, BTreeMap::new(), RuntimeOptions::default()).unwrap();
    assert!(r.attach(b).is_err());
    assert!(
        SceneRuntime::new(
            fixture(""),
            BTreeMap::from([("../../outside.js".into(), "".into())]),
            RuntimeOptions::default()
        )
        .is_err()
    );
}
#[test]
fn typed_vectors_are_immutable_and_degree_math_matches_contract() {
    let mut r = runtime(
        "export function init(v){const a=new Vec2(1,0);globalThis.probe={rot:a.rotate(90).toString(),original:a.toString(),dot:new Vec3(1,2,3).dot(new Vec3(4,5,6)),zero:new Vec3(0).normalize().isFinite(),mod:new Vec2(-1.5,2.5).mod(1).toString(),json:JSON.stringify(new Vec3(1,2,3))};return v}",
    );
    r.initialize().unwrap();
    let p = r.evaluate("binding-0", "probe").unwrap();
    assert_eq!(p["original"], "1 0");
    assert_eq!(p["dot"], 32);
    assert_eq!(p["zero"], false);
    assert_eq!(p["mod"], "0.5 0.5");
    assert_eq!(p["json"], "{\"x\":1,\"y\":2,\"z\":3}");
    let xy = p["rot"]
        .as_str()
        .unwrap()
        .split_whitespace()
        .map(|s| s.parse::<f64>().unwrap())
        .collect::<Vec<_>>();
    near(xy[0], 0.0);
    near(xy[1], 1.0);
}
#[test]
fn matrices_compose_inverse_transform_and_color_modules_execute() {
    let mut r = runtime(
        "import {rgb2hsv,hsv2rgb}from 'WEColor';export function init(v){const m=Mat4.compose(new Vec3(4,5,6),new Vec3(0,0,90),new Vec3(2,3,4));const p=m.inverse().transformPoint(m.transformPoint(new Vec3(1,2,3)));globalThis.probe={point:p.toString(),det:m.determinant(),color:hsv2rgb(rgb2hsv(new Vec3(1,0,0))).toString(),normal:m.normalMatrix().m};return v}",
    );
    let init = r.initialize().unwrap();
    assert!(init.diagnostics.is_empty(), "{:?}", init.diagnostics);
    let p = r.evaluate("binding-0", "probe").unwrap();
    for (a, b) in p["point"]
        .as_str()
        .unwrap()
        .split_whitespace()
        .map(|s| s.parse::<f64>().unwrap())
        .zip([1.0, 2.0, 3.0])
    {
        near(a, b)
    }
    near(p["det"].as_f64().unwrap(), 24.0);
    assert_eq!(p["color"], "1 0 0");
    assert_eq!(p["normal"].as_array().unwrap().len(), 9);
}
#[test]
fn serialized_radians_and_script_degree_angles_are_bidirectional() {
    let mut scene = fixture(
        "export function init(v){globalThis.degrees=thisLayer.angles.z;thisLayer.angles=new Vec3(0,0,180);return v}",
    );
    scene["objects"][0]["angles"] = json!(format!("0 0 {}", std::f64::consts::FRAC_PI_2));
    let binding = collect_bindings(&scene).remove(0);
    let mut r = SceneRuntime::new(scene, BTreeMap::new(), RuntimeOptions::default()).unwrap();
    r.attach(binding).unwrap();
    let o = r.initialize().unwrap();
    near(
        r.evaluate("binding-0", "degrees")
            .unwrap()
            .as_f64()
            .unwrap(),
        90.0,
    );
    near(
        o.scene["objects"][0]["angles"]
            .as_str()
            .unwrap()
            .split_whitespace()
            .last()
            .unwrap()
            .parse()
            .unwrap(),
        std::f64::consts::PI,
    );
}
#[test]
fn audio_refresh_precedes_timer_and_update_and_average_precedes_max() {
    let mut r = runtime(
        "const a=engine.registerAudioBuffers(32);export function init(v){engine.setTimeout(()=>{thisLayer.color=new Vec3(a.average[0],a.left[0],a.right[0]);},0);return v}export function update(v){return a.average[0]}",
    );
    r.initialize().unwrap();
    let mut f = FrameInput::default();
    f.audio_left[0] = 1.0;
    f.audio_right[1] = 1.0;
    let o = r.step(f).unwrap();
    near(alpha(&o.scene), 0.5);
    assert_eq!(o.scene["objects"][0]["color"], "0.5 1 1");
}
#[test]
fn interval_overshoot_once_per_tick_and_cancellation_functions() {
    let mut r = runtime(
        "let n=0;export function init(v){const cancel=engine.setTimeout(()=>{n=999},0);cancel();engine.setInterval(()=>{n++;},100);return v}export function update(v){return n}",
    );
    r.initialize().unwrap();
    let mut f = FrameInput {
        delta_seconds: 0.35,
        ..Default::default()
    };
    assert_eq!(alpha(&r.step(f.clone()).unwrap().scene), 1.0);
    f.delta_seconds = 0.09;
    assert_eq!(alpha(&r.step(f.clone()).unwrap().scene), 1.0);
    f.delta_seconds = 0.02;
    assert_eq!(alpha(&r.step(f).unwrap().scene), 2.0);
}
#[test]
fn timer_errors_do_not_disable_update_export() {
    let mut r = runtime(
        "export function init(v){engine.setTimeout(()=>{throw new Error('timer-owned')},0);return v}export function update(v){return v+1}",
    );
    r.initialize().unwrap();
    let first = r.step(FrameInput::default()).unwrap();
    assert!(first.diagnostics.iter().any(|d| d.hook == "timer"));
    assert_eq!(alpha(&first.scene), 2.0);
    assert_eq!(alpha(&r.step(FrameInput::default()).unwrap().scene), 3.0);
}
#[test]
fn failing_hook_is_suppressed_but_prior_side_effects_survive() {
    let mut r =
        runtime("export function update(v){thisLayer.alpha=0.25;throw new Error('owned-failure')}");
    let a = r.step(FrameInput::default()).unwrap();
    near(alpha(&a.scene), 0.25);
    assert_eq!(
        a.diagnostics.iter().filter(|d| d.hook == "update").count(),
        1
    );
    let b = r.step(FrameInput::default()).unwrap();
    assert!(!b.diagnostics.iter().any(|d| d.hook == "update"));
}
#[test]
fn loop_limit_stops_an_actual_infinite_script() {
    let scene = fixture("export function update(v){while(true){}return v}");
    let binding = collect_bindings(&scene).remove(0);
    let opts = RuntimeOptions {
        loop_iterations: 500,
        ..Default::default()
    };
    let mut r = SceneRuntime::new(scene, BTreeMap::new(), opts).unwrap();
    r.attach(binding).unwrap();
    let o = r.step(FrameInput::default()).unwrap();
    assert!(o.diagnostics.iter().any(|d| d.hook == "update"));
}
#[test]
fn promise_job_limit_stops_rescheduling_chain() {
    let scene = fixture(
        "export function update(v){function again(){Promise.resolve().then(again)}again();return v}",
    );
    let binding = collect_bindings(&scene).remove(0);
    let opts = RuntimeOptions {
        max_jobs: 100,
        ..Default::default()
    };
    let mut r = SceneRuntime::new(scene, BTreeMap::new(), opts).unwrap();
    r.attach(binding).unwrap();
    let o = r.step(FrameInput::default()).unwrap();
    assert!(o.diagnostics.iter().any(|d| d.message.contains("budget")));
}
#[test]
fn targeted_cursor_event_has_real_vector_payload() {
    let mut r = runtime("export function cursorDown(e){thisLayer.origin=e.worldPosition}");
    r.initialize().unwrap();
    let o = r
        .dispatch_to(
            Some(&json!(1)),
            "cursorDown",
            json!({"worldPosition":[4,5,6],"localPosition":[1,2,3],"hitBox":true}),
        )
        .unwrap();
    assert_eq!(o.scene["objects"][0]["origin"], "4 5 6");
}
#[test]
fn initial_and_changed_only_property_callbacks() {
    let mut r = runtime(
        "export function applyUserProperties(p){globalThis.last=p;thisLayer.name=String(Object.keys(p).length)}",
    );
    let mut f = FrameInput {
        user_properties: json!({"a":1,"b":2}),
        ..Default::default()
    };
    r.initialize_with_input(f.clone()).unwrap();
    assert_eq!(r.scene()["objects"][0]["name"], "2");
    f.user_properties = json!({"a":3,"b":2});
    r.step(f).unwrap();
    assert_eq!(r.evaluate("binding-0", "last").unwrap(), json!({"a":3}));
}
#[test]
fn storage_namespaces_json_roundtrip_and_destroy_hook() {
    let mut r = runtime(
        "export function init(v){localStorage.set('x',{n:3,v:new Vec3(1,2,3)},'global');localStorage.set('x',9);return v}export function destroy(){localStorage.clear();console.log('destroyed')}",
    );
    let o = r.initialize().unwrap();
    assert_eq!(o.storage["global"]["x"]["v"], json!({"x":1,"y":2,"z":3}));
    assert_eq!(o.storage["screen"]["x"], 9);
    let destroyed = r.destroy().unwrap();
    assert_eq!(destroyed.storage["screen"], json!({}));
    assert!(
        destroyed
            .diagnostics
            .iter()
            .any(|d| d.message == "destroyed")
    );
    assert!(r.step(FrameInput::default()).is_err());
}
#[test]
fn create_destroy_layer_handles_and_parent_transform_adjustment() {
    let mut r = runtime(
        "export function init(v){let a=thisScene.createLayer({name:'child',origin:'1 0 0',angles:'0 0 0',scale:'1 1 1'});a.setParent(thisLayer,true);globalThis.before=a.getTransformMatrix().translation().toString();thisScene.destroyLayer(a);globalThis.valid=engine.isObjectValid(a);let b=thisScene.createLayer({name:'later',origin:'0 0 0'});globalThis.reused=engine.isObjectValid(a);return v}",
    );
    let o = r.initialize().unwrap();
    assert!(o.diagnostics.is_empty(), "{:?}", o.diagnostics);
    assert_eq!(r.evaluate("binding-0", "before").unwrap(), "1 0 0");
    assert_eq!(r.evaluate("binding-0", "valid").unwrap(), false);
    assert_eq!(r.evaluate("binding-0", "reused").unwrap(), false);
    assert!(o.commands.iter().any(|c| c.kind == "scene.createLayer"));
}
#[test]
fn model_bone_morph_particle_and_effect_commands_are_typed() {
    let mut scene = fixture(
        "export function init(v){thisLayer.setLocalBoneOrigin(0,new Vec3(1,2,3));thisLayer.setBlendShapeWeight('smile',0.5);thisLayer.emitParticles(3);thisLayer.instance.colorn=0.5;thisLayer.getEffect(0).setMaterialProperty('amount',0.25);return v}",
    );
    scene["objects"][0]["bones"] = json!([{"name":"root","origin":"0 0 0"}]);
    scene["objects"][0]["blendshapes"] = json!([{"name":"smile","weight":0}]);
    scene["objects"][0]["effects"] =
        json!([{"name":"fx","passes":[{"constantshadervalues":{"amount":0.0}}]}]);
    let b = collect_bindings(&scene).remove(0);
    let mut r = SceneRuntime::new(scene, BTreeMap::new(), RuntimeOptions::default()).unwrap();
    r.attach(b).unwrap();
    let o = r.initialize().unwrap();
    assert!(
        o.diagnostics.iter().all(|d| d.level != "error"),
        "{:?}",
        o.diagnostics
    );
    for kind in [
        "bone.localOrigin",
        "blendShape.weight",
        "particle.emit",
        "particle.instance",
        "effect.materialProperty",
    ] {
        assert!(o.commands.iter().any(|c| c.kind == kind), "missing {kind}")
    }
}
#[test]
fn global_registration_rules_and_missing_resources_fail_explicitly() {
    let mut r = runtime("export function init(v){engine.registerAudioBuffers(16);return v}");
    assert!(
        r.initialize()
            .unwrap()
            .diagnostics
            .iter()
            .any(|d| d.hook == "init")
    );
    let scene = fixture("engine.setTimeout(()=>{},0)");
    let b = collect_bindings(&scene).remove(0);
    let mut r = SceneRuntime::new(scene, BTreeMap::new(), RuntimeOptions::default()).unwrap();
    assert!(r.attach(b).is_err());
}

#[test]
fn module_globals_see_initial_input_before_attach() {
    let source = "const gain=engine.userProperties.gain;const size=engine.screenResolution;export function init(v){thisLayer.origin=new Vec3(size.x,size.y,0);return gain}";
    let scene = fixture(source);
    let binding = collect_bindings(&scene).remove(0);
    let mut r = SceneRuntime::new(scene, BTreeMap::new(), RuntimeOptions::default()).unwrap();
    r.set_initial_input(FrameInput {
        user_properties: json!({"gain":0.3}),
        screen_resolution: [100.0, 50.0],
        ..Default::default()
    })
    .unwrap();
    r.attach(binding).unwrap();
    let o = r.initialize().unwrap();
    near(alpha(&o.scene), 0.3);
    assert_eq!(o.scene["objects"][0]["origin"], "100 50 0");
    assert!(r.set_initial_input(FrameInput::default()).is_err());
}
#[test]
fn queued_input_precedes_timer_update_and_hit_result_is_returned() {
    let mut r = runtime(
        "export function cursorDown(e){thisLayer.alpha=engine.userProperties.gain;thisLayer.name=String(input.cursorLeftDown)}export function cursorHitTest(e){return e.worldPosition.x>0}export function update(v){return v*2}",
    );
    r.enqueue_event(
        Some(json!(1)),
        "cursorDown",
        json!({"worldPosition":[1,0,0]}),
    )
    .unwrap();
    r.enqueue_event(
        Some(json!(1)),
        "cursorHitTest",
        json!({"worldPosition":[-1,0,0]}),
    )
    .unwrap();
    let o = r
        .step(FrameInput {
            user_properties: json!({"gain":0.7}),
            cursor_left_down: true,
            ..Default::default()
        })
        .unwrap();
    near(alpha(&o.scene), 1.4);
    assert_eq!(o.scene["objects"][0]["name"], "true");
    assert_eq!(o.hook_results.len(), 1);
    assert_eq!(o.hook_results[0].value, false);
}
#[test]
fn shared_animation_ticks_once_per_frame_across_realms_and_end_callbacks_all_fire() {
    let mut scene = fixture(
        "const a=thisLayer.getAnimationLayer(0);export function init(v){a.addEndedCallback(()=>localStorage.set('a',true));return v}export function update(v){return a.getFrame()}",
    );
    scene["objects"][0]["visible"] = json!({"value":true,"script":"const a=thisLayer.getAnimationLayer(0);export function init(v){a.addEndedCallback(()=>localStorage.set('b',true));return v}"});
    scene["objects"][0]["animationlayers"] = json!([{"name":"clip","fps":10,"frameCount":10,"duration":1,"__playing":true,"__time":0,"rate":1}]);
    let bindings = collect_bindings(&scene);
    let mut r = SceneRuntime::new(scene, BTreeMap::new(), RuntimeOptions::default()).unwrap();
    for b in bindings {
        r.attach(b).unwrap()
    }
    r.initialize().unwrap();
    let a = r
        .step(FrameInput {
            delta_seconds: 0.5,
            ..Default::default()
        })
        .unwrap();
    near(alpha(&a.scene), 5.0);
    near(
        a.scene["objects"][0]["animationlayers"][0]["__time"]
            .as_f64()
            .unwrap(),
        0.5,
    );
    let b = r
        .step(FrameInput {
            delta_seconds: 0.6,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(b.storage["screen"]["a"], true);
    assert_eq!(b.storage["screen"]["b"], true);
}
#[test]
fn bone_world_transforms_include_ancestor_chain_and_setters_update_matrices() {
    let mut scene = fixture(
        "export function init(v){globalThis.before=thisLayer.getBoneTransform(1).translation().toString();thisLayer.setLocalBoneOrigin(1,new Vec3(4,0,0));globalThis.after=thisLayer.getLocalBoneTransform(1).translation().toString();return v}",
    );
    scene["objects"][0]["bones"] = json!([{"name":"root","parent":-1,"origin":"10 0 0"},{"name":"child","parent":0,"origin":"2 0 0"}]);
    let b = collect_bindings(&scene).remove(0);
    let mut r = SceneRuntime::new(scene, BTreeMap::new(), RuntimeOptions::default()).unwrap();
    r.attach(b).unwrap();
    let o = r.initialize().unwrap();
    assert!(o.diagnostics.is_empty(), "{:?}", o.diagnostics);
    assert_eq!(r.evaluate("binding-0", "before").unwrap(), "13 2 3");
    assert_eq!(r.evaluate("binding-0", "after").unwrap(), "4 0 0");
}
#[test]
fn procedural_typed_geometry_has_real_constants_layer_reference_updates_and_deferred_release() {
    let mut r = runtime(
        "let data,layer;export function init(v){data=thisScene.createModelData({shapes:[{vertexFormat:[IModelData.POSITION],vertexBuffer:new Float32Array([0,0,0,1,0,0,0,1,0]),indexBuffer:new Uint32Array([0,1,2])}]});layer=thisScene.createLayer({name:'triangle',model:data});globalThis.constant=data.POSITION;return v}export function update(v){data.applyData({vertexBuffer:new Float32Array([0,0,0,2,0,0,0,2,0])});thisScene.destroyModelData(data);globalThis.valid=engine.isObjectValid(data);return v}",
    );
    let init = r.initialize().unwrap();
    assert!(init.diagnostics.is_empty(), "{:?}", init.diagnostics);
    assert_eq!(r.evaluate("binding-0", "constant").unwrap(), "position");
    assert_eq!(init.scene["objects"][1]["modeldata"], "model-1");
    let o = r.step(FrameInput::default()).unwrap();
    assert!(o.diagnostics.is_empty(), "{:?}", o.diagnostics);
    assert_eq!(
        o.scene["__modelData"][0]["shapes"][0]["vertexBuffer"][3],
        2.0
    );
    assert_eq!(o.scene["__modelData"][0]["__destroyRequested"], true);
    assert_eq!(r.evaluate("binding-0", "valid").unwrap(), false);
    assert!(
        o.commands
            .iter()
            .any(|c| c.kind == "modelData.destroy" && c.arguments["deferred"] == true)
    );
}
#[test]
fn procedural_replace_inside_update_throws_and_apply_length_change_throws() {
    let mut r = runtime(
        "let data;export function init(v){data=thisScene.createModelData({shapes:[{vertexFormat:[IModelData.POSITION],vertexBuffer:new Float32Array([0,0,0])}]});return v}export function update(v){data.replaceData({vertexBuffer:new Float32Array([1,1,1])});return v}",
    );
    r.initialize().unwrap();
    assert!(
        r.step(FrameInput::default())
            .unwrap()
            .diagnostics
            .iter()
            .any(|d| d.message.contains("forbidden"))
    );
}
#[test]
fn builder_saved_script_properties_user_links_and_shared_cross_realms() {
    let source = "export var scriptProperties=createScriptProperties().addSlider({name:'gain',value:1,min:0,max:4}).addColor({name:'tint',value:new Vec3(1,1,1)}).finish();export function init(v){shared.count=(shared.count||0)+1;return v}export function update(v){thisLayer.color=scriptProperties.tint;return scriptProperties.gain+shared.count}";
    let mut scene = fixture(source);
    scene["objects"][0]["alpha"]["scriptproperties"] =
        json!({"gain":{"value":2,"user":{"name":"gain"}},"tint":"0.1 0.2 0.3"});
    scene["objects"][0]["visible"] = json!({"value":true,"script":"export function init(v){shared.count=(shared.count||0)+1;return v}"});
    let bindings = collect_bindings(&scene);
    let mut r = SceneRuntime::new(scene, BTreeMap::new(), RuntimeOptions::default()).unwrap();
    r.set_initial_input(FrameInput {
        user_properties: json!({"gain":0.25}),
        ..Default::default()
    })
    .unwrap();
    for b in bindings {
        r.attach(b).unwrap()
    }
    let initialized = r.initialize().unwrap();
    assert!(
        initialized.diagnostics.is_empty(),
        "{:?}",
        initialized.diagnostics
    );
    let o = r
        .step(FrameInput {
            user_properties: json!({"gain":0.5}),
            ..Default::default()
        })
        .unwrap();
    near(alpha(&o.scene), 2.5);
    let color = o.scene["objects"][0]["color"]
        .as_str()
        .unwrap()
        .split_whitespace()
        .map(|v| v.parse::<f64>().unwrap())
        .collect::<Vec<_>>();
    near(color[2], 0.3);
}
#[test]
fn init_can_play_a_startpaused_property_curve_and_ended_callback_fires() {
    let source = "export function init(v){const a=thisObject.getAnimation('alpha');a.addEndedCallback(()=>localStorage.set('ended',true));a.play();return v}";
    let mut scene = fixture(source);
    scene["objects"][0]["alpha"]["animation"] = json!({"c0":[{"frame":0,"value":0},{"frame":10,"value":1}],"options":{"fps":10,"length":10,"mode":"single","startpaused":true}});
    let bindings = collect_bindings(&scene);
    let mut r = SceneRuntime::new(scene, BTreeMap::new(), RuntimeOptions::default()).unwrap();
    for b in bindings {
        r.attach(b).unwrap()
    }
    r.initialize().unwrap();
    let half = r
        .step(FrameInput {
            delta_seconds: 0.5,
            ..Default::default()
        })
        .unwrap();
    near(alpha(&half.scene), 0.5);
    let end = r
        .step(FrameInput {
            delta_seconds: 0.6,
            ..Default::default()
        })
        .unwrap();
    near(alpha(&end.scene), 1.0);
    assert_eq!(end.storage["screen"]["ended"], true);
}
#[test]
fn dynamic_binding_initializes_and_removed_script_destroys_once() {
    let mut r = runtime(
        "export function init(v){thisScene.createLayer({name:'dynamic',alpha:{value:1,script:\"export function init(v){localStorage.set('created',true);return 2}export function update(v){return v+1}export function destroy(){localStorage.set('removed',true)}\"}});return v}",
    );
    r.initialize().unwrap();
    let first = r.step(FrameInput::default()).unwrap();
    assert_eq!(first.storage["screen"]["created"], true);
    assert_eq!(first.scene["objects"][1]["alpha"]["value"], 3.0);
    let mut scene = r.scene().clone();
    scene["objects"].as_array_mut().unwrap().remove(1);
    r.replace_scene(scene).unwrap();
    let second = r.step(FrameInput::default()).unwrap();
    assert_eq!(second.storage["screen"]["removed"], true);
}
#[test]
fn immutable_asset_library_survives_multiple_dynamic_state_loads() {
    let source = "const asset=engine.registerAsset('templates/item.json',true);export function update(v){const layer=thisScene.createLayer(asset);return thisScene.getInitialLayerConfig(thisLayer).alpha.value+thisScene.getLayerCount()}";
    let scene = fixture(source);
    let bindings = collect_bindings(&scene);
    let mut options = RuntimeOptions::default();
    options.assets.insert(
        "templates/item.json".into(),
        json!({"name":"asset","alpha":1}),
    );
    let mut r = SceneRuntime::new(scene, BTreeMap::new(), options).unwrap();
    for b in bindings {
        r.attach(b).unwrap()
    }
    let first = r.step(FrameInput::default()).unwrap();
    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    near(alpha(&first.scene), 3.0);
    let second = r.step(FrameInput::default()).unwrap();
    near(alpha(&second.scene), 4.0);
}
#[test]
fn parent_attachment_participates_in_transform_and_keepworld_adjustment() {
    let mut scene = fixture(
        "export function init(v){let child=thisScene.createLayer({name:'child',origin:'1 0 0'});child.setParent(thisLayer,'slot',false);globalThis.attached=child.getTransformMatrix().translation().toString();child.setParent(null,true);globalThis.detached=child.getTransformMatrix().translation().toString();return v}",
    );
    scene["objects"][0]["attachments"] = json!([{"name":"slot","origin":"10 0 0"}]);
    let bindings = collect_bindings(&scene);
    let mut r = SceneRuntime::new(scene, BTreeMap::new(), RuntimeOptions::default()).unwrap();
    for b in bindings {
        r.attach(b).unwrap()
    }
    let o = r.initialize().unwrap();
    assert!(o.diagnostics.is_empty(), "{:?}", o.diagnostics);
    assert_eq!(r.evaluate("binding-0", "attached").unwrap(), "12 2 3");
    assert_eq!(r.evaluate("binding-0", "detached").unwrap(), "12 2 3");
}
#[test]
fn effect_material_dynamic_fields_read_typed_values_and_write_shader_constants() {
    let mut scene = fixture(
        "export function init(v){const m=thisLayer.getEffect(0).getMaterial(0);globalThis.tint=m.g_Tint.toString();m.g_Tint=new Vec3(0.5,0.25,1);m.g_Amount=0.7;return v}",
    );
    scene["objects"][0]["effects"] = json!([{"name":"effect","passes":[{"constantshadervalues":{"g_Tint":"1 0 0","g_Amount":0.2}}]}]);
    let bindings = collect_bindings(&scene);
    let mut r = SceneRuntime::new(scene, BTreeMap::new(), RuntimeOptions::default()).unwrap();
    for b in bindings {
        r.attach(b).unwrap()
    }
    let o = r.initialize().unwrap();
    assert!(o.diagnostics.is_empty(), "{:?}", o.diagnostics);
    assert_eq!(r.evaluate("binding-0", "tint").unwrap(), "1 0 0");
    assert_eq!(
        o.scene["objects"][0]["effects"][0]["passes"][0]["constantshadervalues"]["g_Tint"],
        "0.5 0.25 1"
    );
    near(
        o.scene["objects"][0]["effects"][0]["passes"][0]["constantshadervalues"]["g_Amount"]
            .as_f64()
            .unwrap(),
        0.7,
    );
    assert_eq!(
        o.commands
            .iter()
            .filter(|c| c.kind == "effect.materialProperty")
            .count(),
        2
    );
}
#[test]
fn nested_effect_thisobject_uses_material_provider_and_lazy_asset_config() {
    let source = "export function init(v){const material=thisObject.getMaterial(0);material.amount=0.9;return v}";
    let mut scene = fixture("");
    scene["objects"][0]["alpha"] = json!(1);
    scene["objects"][0]["effects"] =
        json!([{"file":"effects/test/effect.json","visible":{"value":true,"script":source}}]);
    let mut options = RuntimeOptions::default();
    options.assets.insert(
        "effects/test/effect.json".into(),
        json!({"passes":[{"material":"materials/test.json"}]}),
    );
    options.assets.insert(
        "materials/test.json".into(),
        json!({"passes":[{"shader":"genericimage2","constantshadervalues":{"amount":0.2}}]}),
    );
    let bindings = collect_bindings(&scene);
    let mut r = SceneRuntime::new(scene, BTreeMap::new(), options).unwrap();
    for b in bindings {
        r.attach(b).unwrap()
    }
    let o = r.initialize().unwrap();
    assert!(o.diagnostics.is_empty(), "{:?}", o.diagnostics);
    near(
        o.scene["objects"][0]["effects"][0]["passes"][0]["constantshadervalues"]["amount"]
            .as_f64()
            .unwrap(),
        0.9,
    );
}
#[test]
fn user_property_color_is_a_vector_in_globals_and_changed_hooks() {
    let scene = fixture(
        "const initial=engine.userProperties.tint.z;export function init(v){return initial}export function applyUserProperties(p){if(p.tint)thisLayer.color=p.tint.multiply(2)}",
    );
    let bindings = collect_bindings(&scene);
    let mut options = RuntimeOptions::default();
    options
        .user_property_definitions
        .insert("tint".into(), json!({"type":"color"}));
    let mut r = SceneRuntime::new(scene, BTreeMap::new(), options).unwrap();
    r.set_initial_input(FrameInput {
        user_properties: json!({"tint":"0.1 0.2 0.3"}),
        ..Default::default()
    })
    .unwrap();
    for b in bindings {
        r.attach(b).unwrap()
    }
    let initialized = r.initialize().unwrap();
    assert!(
        initialized.diagnostics.is_empty(),
        "{:?}",
        initialized.diagnostics
    );
    near(alpha(&initialized.scene), 0.3);
    let output = r
        .step(FrameInput {
            user_properties: json!({"tint":"0.4 0.5 0.6"}),
            ..Default::default()
        })
        .unwrap();
    let color = output.scene["objects"][0]["color"]
        .as_str()
        .unwrap()
        .split_whitespace()
        .map(|n| n.parse::<f64>().unwrap())
        .collect::<Vec<_>>();
    near(color[2], 1.2);
}
#[test]
fn reconstructed_value_overloads_basis_angle_and_setter_chaining() {
    let mut r = runtime(
        "export function init(v){let m=Mat3.identity();globalThis.values={v3:new Vec3(1,2).toString(),v4:new Vec4(1,2,3).toString(),perp:new Vec2(1,2).perpendicular().toString(),angle:m.angle(),chain:m.translation(new Vec2(3,4))===m,hue:WEColor.hsv2rgb(new Vec3(-0.1,1,1)).toString(),array:new Mat3([1,2,3,4,5,6,7,8,9]).m};return v}",
    );
    r.initialize().unwrap();
    let v = r.evaluate("binding-0", "values").unwrap();
    assert_eq!(v["v3"], "1 2 0");
    assert_eq!(v["v4"], "1 2 3 3");
    assert_eq!(v["perp"], "2 -1");
    assert_eq!(v["angle"], 90.0);
    assert_eq!(v["chain"], true);
    assert_eq!(v["hue"], "0 0 0");
    assert_eq!(v["array"], json!([1, 2, 3, 4, 5, 6, 7, 8, 9]));
}
#[test]
fn shader_aliases_read_defaults_and_sync_existing_overrides() {
    let mut scene = fixture(
        "export function init(v){let m=thisLayer.getEffect(0).getMaterial(0);globalThis.before=m.amount;globalThis.tint=m.tint.toString();m.amount=0.8;thisLayer.getEffect(0).setMaterialProperty('tint',new Vec3(0.2,0.3,0.4));return v}",
    );
    scene["objects"][0]["effects"] = json!([{"passes":[{"__shaderDefaults":{"g_Amount":0.1,"g_Tint":"1 1 1"},"__uniformAliases":{"amount":["g_Amount"],"tint":["g_Tint"]},"constantshadervalues":{"amount":0.4,"g_Amount":0.5}}]}]);
    let bindings = collect_bindings(&scene);
    let mut r = SceneRuntime::new(scene, BTreeMap::new(), RuntimeOptions::default()).unwrap();
    for b in bindings {
        r.attach(b).unwrap()
    }
    let o = r.initialize().unwrap();
    assert!(o.diagnostics.is_empty(), "{:?}", o.diagnostics);
    near(
        r.evaluate("binding-0", "before").unwrap().as_f64().unwrap(),
        0.4,
    );
    assert_eq!(r.evaluate("binding-0", "tint").unwrap(), "1 1 1");
    near(
        o.scene["objects"][0]["effects"][0]["passes"][0]["constantshadervalues"]["g_Amount"]
            .as_f64()
            .unwrap(),
        0.8,
    );
    assert_eq!(
        o.scene["objects"][0]["effects"][0]["passes"][0]["constantshadervalues"]["g_Tint"],
        o.scene["objects"][0]["effects"][0]["passes"][0]["constantshadervalues"]["tint"]
    );
}
#[test]
fn bad_new_binding_is_isolated_and_current_input_is_visible_to_new_globals() {
    let mut r = runtime("export function update(v){return v+1}");
    r.initialize().unwrap();
    let mut scene = r.scene().clone();
    scene["objects"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":2,"alpha":{"value":1,"script":"this is not valid JS !"}}));
    scene["objects"].as_array_mut().unwrap().push(json!({"id":3,"alpha":{"value":1,"script":"const gain=engine.userProperties.gain;export function init(v){return gain}"}}));
    r.replace_scene(scene).unwrap();
    let o = r
        .step(FrameInput {
            user_properties: json!({"gain":0.7}),
            ..Default::default()
        })
        .unwrap();
    near(alpha(&o.scene), 2.0);
    near(
        o.scene["objects"][2]["alpha"]["value"].as_f64().unwrap(),
        0.7,
    );
    assert_eq!(
        o.diagnostics.iter().filter(|d| d.hook == "module").count(),
        1
    );
    let next = r.step(FrameInput::default()).unwrap();
    near(alpha(&next.scene), 3.0);
    assert!(!next.diagnostics.iter().any(|d| d.hook == "module"));
}
#[test]
fn effect_material_indices_preserve_command_rows_and_original_null_boundaries() {
    let mut scene = fixture(
        "export function init(v){let e=thisLayer.getEffect(0);globalThis.result={count:e.getMaterialCount(),command:e.getMaterial(1)===null,invalid:e.getMaterial(-1)===null,outside:e.getMaterial(99)===null,last:e.getMaterial('').amount,named:e.getMaterial('MATERIALS/A.JSON').amount};e.setMaterialProperty('notAUniform',1);return v}",
    );
    scene["objects"][0]["effects"] = json!([{"passes":[{"material":"materials/a.json","__materialHydrated":true,"constantshadervalues":{"amount":0.2}},{"command":"swap","source":"a","target":"b"},{"material":"materials/b.json","__materialHydrated":true,"constantshadervalues":{"amount":0.8}}]}]);
    let bindings = collect_bindings(&scene);
    let mut r = SceneRuntime::new(scene, BTreeMap::new(), RuntimeOptions::default()).unwrap();
    for b in bindings {
        r.attach(b).unwrap()
    }
    let o = r.initialize().unwrap();
    assert!(o.diagnostics.is_empty(), "{:?}", o.diagnostics);
    let result = r.evaluate("binding-0", "result").unwrap();
    assert_eq!(result["count"], 3);
    assert_eq!(result["command"], true);
    assert_eq!(result["invalid"], true);
    assert_eq!(result["outside"], true);
    near(result["last"].as_f64().unwrap(), 0.8);
    near(result["named"].as_f64().unwrap(), 0.2);
}
#[test]
fn animation_clip_lookup_prefers_the_own_model_library() {
    let mut scene = fixture(
        "export function init(v){const a=thisLayer.createAnimationLayer('idle');a.play();return a.fps}",
    );
    scene["objects"][0]["__animationClips"] =
        json!([{"name":"idle","clip":2,"animation":17,"fps":24,"frameCount":48,"duration":2}]);
    scene["animations"] = json!([{"name":"idle","clip":0,"fps":10}]);
    let bindings = collect_bindings(&scene);
    let mut r = SceneRuntime::new(scene, BTreeMap::new(), RuntimeOptions::default()).unwrap();
    for b in bindings {
        r.attach(b).unwrap()
    }
    let o = r.initialize().unwrap();
    assert!(o.diagnostics.is_empty(), "{:?}", o.diagnostics);
    assert_eq!(alpha(&o.scene), 24.0);
    assert_eq!(o.scene["objects"][0]["animationlayers"][0]["clip"], 2);
    assert_eq!(o.scene["objects"][0]["animationlayers"][0]["animation"], 17);
}
#[test]
fn effect_conditions_build_native_effective_row_indices_without_reordering_routes() {
    let mut scene = fixture(
        "export function init(v){const e=thisLayer.getEffect(0);globalThis.count=e.getMaterialCount();globalThis.a=e.getMaterial(0).amount;globalThis.command=e.getMaterial(1)===null;return v}",
    );
    scene["objects"][0]["effects"] = json!([{"passes":[{"shader":"genericimage2","conditions":[{"ENABLED":0},{"OTHER":1}],"combos":{"ENABLED":1},"constantshadervalues":{"amount":0.1}},{"shader":"genericimage2","conditions":{"ENABLED":1,"MISSING":0},"constantshadervalues":{"amount":0.7}},{"command":"swap","conditions":[],"source":"a","target":"b"}]}]);
    let bindings = collect_bindings(&scene);
    let mut r = SceneRuntime::new(scene, BTreeMap::new(), RuntimeOptions::default()).unwrap();
    for b in bindings {
        r.attach(b).unwrap()
    }
    let o = r.initialize().unwrap();
    assert!(o.diagnostics.is_empty(), "{:?}", o.diagnostics);
    assert_eq!(r.evaluate("binding-0", "count").unwrap(), 2);
    near(r.evaluate("binding-0", "a").unwrap().as_f64().unwrap(), 0.7);
    assert_eq!(r.evaluate("binding-0", "command").unwrap(), true);
    assert_eq!(
        o.scene["objects"][0]["effects"][0]["passes"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
}
