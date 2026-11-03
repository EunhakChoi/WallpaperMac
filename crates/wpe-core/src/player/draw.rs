use super::*;
use std::sync::Arc;
use wpe_render::shader::UniformValue;
use wpe_render::{
    BlendState, CpuProgram, RenderPass, Target, TextureFormat, VertexStream, VertexStreamData,
};
impl ScenePlayer {
    pub(super) fn render_scene(
        &mut self,
        delta: f32,
        input: &PlayerInput,
        simulate: bool,
    ) -> Result<RenderFrame> {
        let [w, h] = self.canvas();
        let mut frame = RenderFrame::new(w, h);
        frame.time = self.time as f32;
        let rgb = vec3(self.scene.pointer("/general/clearcolor"), Vec3::ZERO);
        frame.clear = [rgb.0[0], rgb.0[1], rgb.0[2], 1.];
        let format = if boolean(self.scene.pointer("/general/hdr"), false) {
            TextureFormat::Rgba16Float
        } else {
            TextureFormat::Rgba8Unorm
        };
        let canvas = target(&mut frame, "scene-canvas", w, h, format);
        frame.passes.push(RenderPass {
            name: "scene-clear".into(),
            target: Target::Texture(canvas),
            clear: Some(frame.clear),
            ..Default::default()
        });
        self.assets.targets.clear();
        let background = target(&mut frame, "scene-background", w, h, format);
        self.assets
            .targets
            .insert("_rt_fullframebuffer".into(), background);
        self.assets.targets.insert("_rt_scene".into(), background);
        let reflection_id = resource_id("scene-planar-reflection");
        self.assets
            .targets
            .insert("_rt_reflection".into(), reflection_id);
        self.assets
            .targets
            .insert("_rt_mipmappedframebuffer".into(), background);
        self.assets.global_defines = BTreeMap::from([
            (
                "FOG_DIST".into(),
                i32::from(boolean(self.scene.pointer("/general/fogdistance"), false)),
            ),
            (
                "FOG_HEIGHT".into(),
                i32::from(boolean(self.scene.pointer("/general/fogheight"), false)),
            ),
        ]);
        let mut objects = self
            .scene
            .get("objects")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut transforms = hierarchy(&objects)?;
        let (projection, view, eye) = self.camera_parts(w, h, input);
        let camera = projection * view;
        let mut reflection_draws = Vec::new();
        self.hits.clear();
        let mut collision = Vec::new();
        let mut motions = Vec::new();
        // Animated triangle colliders use the same pose/space as the visible mesh.
        for (at, object) in objects.iter().enumerate() {
            let key = object_key(object, at);
            let editable = self.editable_model_data(object)?;
            if let Some(model) = self.models.get_mut(&key) {
                if let Some(editable) = editable {
                    model
                        .apply_editable_shapes(&editable)
                        .map_err(|e| Error::invalid(&key, e.to_string()))?;
                }
                Self::sync_model(model, object);
                if simulate && let Err(e) = model.step(delta) {
                    self.issue("model-pose", Some(&key), e.to_string());
                    continue;
                }
                if simulate {
                    motions.push((at, model.pose.root_motion));
                }
            }
        }
        if apply_root_motion(&mut objects, &transforms, &motions)? {
            transforms = hierarchy(&objects)?;
            self.scene["objects"] = json!(objects);
        }
        for (at, object) in objects.iter().enumerate() {
            let key = object_key(object, at);
            if let Some(model) = self.models.get(&key) {
                let meshes = model
                    .skinned_meshes()
                    .map_err(|e| Error::invalid(&key, e.to_string()))?;
                for mesh in meshes
                    .into_iter()
                    .filter(|mesh| mesh.texture_channels.is_none())
                {
                    for tri in mesh.indices.chunks_exact(3) {
                        let get =
                            |i: u32| transforms[at].point(Vec3(mesh.vertices[i as usize].position));
                        collision.push(wpe_sim::particle::TriangleCollider {
                            a: get(tri[0]),
                            b: get(tri[1]),
                            c: get(tri[2]),
                        });
                    }
                }
            }
        }
        self.lighting = self.prepare_lighting(&objects, &transforms, &mut frame)?;
        let order = dependency_order(&objects)?;
        let mut required = std::collections::BTreeSet::new();
        let mut pending = objects
            .iter()
            .enumerate()
            .filter(|(_, o)| boolean(o.get("visible"), true))
            .map(|(i, _)| i)
            .collect::<Vec<_>>();
        let ids = objects
            .iter()
            .enumerate()
            .map(|(i, o)| (object_key(o, i), i))
            .collect::<BTreeMap<_, _>>();
        while let Some(at) = pending.pop() {
            for dep in objects[at]
                .get("dependencies")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let id = dep.get("id").unwrap_or(dep).to_string();
                if required.insert(id.clone())
                    && let Some(&at) = ids.get(&id)
                {
                    pending.push(at);
                }
            }
        }
        for at in order {
            let object = &objects[at];
            let key = object_key(object, at);
            let resource_only = !boolean(object.get("visible"), true);
            if resource_only && !required.contains(&key) {
                continue;
            }
            let label = object
                .get("name")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .unwrap_or(&key);
            let mut world = transforms[at];
            if boolean(self.scene.pointer("/general/cameraparallax"), false) {
                let depth = vec3(
                    object
                        .get("parallaxDepth")
                        .or_else(|| object.get("parallaxdepth")),
                    Vec3::ZERO,
                );
                let amount = number(self.scene.pointer("/general/cameraparallaxamount"), 0.5);
                world.0[12] += (input.cursor[0] - 0.5) * amount * w as f32 * depth.0[0];
                world.0[13] += (0.5 - input.cursor[1]) * amount * h as f32 * depth.0[1];
            }
            let start = frame.draws.len();
            let background_pass = frame.passes.len();
            copy_pass(
                &mut frame,
                "background-snapshot",
                canvas,
                Target::Texture(background),
            );
            let result = (|| -> Result<()> {
                if let Some(file) = object.get("image").and_then(Value::as_str) {
                    self.image(
                        object,
                        &key,
                        label,
                        file,
                        world,
                        projection,
                        view,
                        eye,
                        input,
                        canvas,
                        background,
                        resource_only,
                        &mut frame,
                    )?;
                }
                if resource_only {
                    return Ok(());
                }
                if object.get("text").is_some() {
                    self.text(
                        object, &key, label, world, projection, view, eye, input, canvas,
                        &mut frame,
                    )?;
                }
                if let Some(mut system) = self.particles.remove(&key) {
                    let inv = world.inverse().unwrap_or_default();
                    let cursor = Vec3(self.script_input(delta, input).cursor_world);
                    if let Some(points) = object.get("controlpoint").and_then(Value::as_array) {
                        for point in points {
                            let id = number(point.get("id"), 0.) as usize;
                            let origin = if number(point.get("flags"), 0.) as u32 & 1 != 0 {
                                inv.point(cursor)
                            } else {
                                vec3(point.get("origin"), Vec3::ZERO)
                            };
                            system
                                .set_control_point(
                                    id,
                                    Mat4::trs(
                                        origin,
                                        Quat::euler_xyz(vec3(point.get("angles"), Vec3::ZERO)),
                                        vec3(point.get("scale"), Vec3::ONE),
                                    ),
                                )
                                .map_err(|e| Error::invalid(label, e.to_string()))?;
                        }
                    }
                    system.collision_triangles = collision
                        .iter()
                        .map(|t| wpe_sim::particle::TriangleCollider {
                            a: inv.point(t.a),
                            b: inv.point(t.b),
                            c: inv.point(t.c),
                        })
                        .collect();
                    system.bounds = Some((
                        inv.point(Vec3::ZERO),
                        inv.point(Vec3::new(w as f32, h as f32, 0.)),
                    ));
                    system.emission_enabled = boolean(object.get("__playing"), true);
                    let outcome = if simulate {
                        system.step(
                            delta,
                            number(input.general_settings.get("fps"), 60.) as u32,
                            delta.max(0.000001),
                        )
                    } else {
                        Ok(())
                    };
                    let child_geometry = if let Some(children) = self.children.get_mut(&key) {
                        if simulate {
                            children
                                .step(
                                    &system,
                                    delta,
                                    number(input.general_settings.get("fps"), 60.) as u32,
                                    delta.max(0.000001),
                                )
                                .map_err(|e| Error::invalid(label, e.to_string()))?
                        } else {
                            children.geometry()
                        }
                    } else {
                        Vec::new()
                    };
                    let inverse = world.inverse().unwrap_or_default();
                    let right = inverse
                        .vector(Vec3::new(view.0[0], view.0[4], view.0[8]))
                        .normalize();
                    let up = inverse
                        .vector(Vec3::new(view.0[1], view.0[5], view.0[9]))
                        .normalize();
                    let geometry = system.geometry_batches(right, up);
                    let material = system.config.material.clone();
                    self.particles.insert(key.clone(), system);
                    outcome.map_err(|e| Error::invalid(label, e.to_string()))?;
                    for geometry in geometry {
                        self.particle_mesh(
                            label, &material, geometry, world, projection, view, eye, input,
                            &mut frame,
                        )?;
                    }
                    for child in child_geometry {
                        self.particle_mesh(
                            label,
                            &child.material,
                            child.geometry,
                            world * child.transform,
                            projection,
                            view,
                            eye,
                            input,
                            &mut frame,
                        )?;
                    }
                }
                if let Some(model) = self
                    .models
                    .get(&key)
                    .filter(|_| object.get("image").is_none())
                {
                    let meshes = model
                        .skinned_meshes()
                        .map_err(|e| Error::invalid(label, e.to_string()))?;
                    for mesh in meshes
                        .into_iter()
                        .filter(|mesh| mesh.texture_channels.is_none())
                    {
                        self.mesh_hit_region(
                            object,
                            world,
                            &mesh.vertices.iter().map(|v| v.position).collect::<Vec<_>>(),
                            &mesh.indices,
                        );
                        let materials = if mesh.material.is_empty() {
                            vec![Material {
                                cpu: CpuProgram::Solid { color: [1.; 4] },
                                state: wpe_render::PipelineState {
                                    depth_test: true,
                                    depth_write: true,
                                    ..Default::default()
                                },
                                ..Default::default()
                            }]
                        } else {
                            self.assets.model_materials(
                                &self.project,
                                &mesh.material,
                                Some(&cpu_deformed_material(
                                    object.get("instance").or_else(|| object.get("material")),
                                )),
                                &mut frame,
                                self.time as f32,
                            )?
                        };
                        for mut material in materials {
                            self.uniforms(
                                &mut material,
                                world,
                                projection,
                                view,
                                eye,
                                object,
                                input,
                            );
                            let vertices = mesh
                                .vertices
                                .iter()
                                .map(|v| {
                                    vertex(
                                        Vec3(v.position),
                                        Vec3(v.normal),
                                        v.uv,
                                        puppet::model_color(v, tint(object), &material),
                                        world,
                                        camera,
                                        material.vertex.is_some(),
                                    )
                                })
                                .collect();
                            reflection_draws.push(frame.draws.len());
                            frame.draws.push(DrawCall {
                                name: label.into(),
                                vertices,
                                indices: mesh.indices.clone(),
                                vertex_streams: if material.vertex.is_some() {
                                    puppet::model_streams(&mesh.vertices, &material)
                                } else {
                                    BTreeMap::new()
                                },
                                instances: draw_instances(&material),
                                material,
                                ..Default::default()
                            });
                        }
                    }
                }
                if let Some(id) = object
                    .get("modeldata")
                    .filter(|_| !self.models.contains_key(&key))
                {
                    let first = frame.draws.len();
                    self.procedural(
                        object, id, label, world, projection, view, eye, input, &mut frame,
                    )?;
                    reflection_draws.extend(first..frame.draws.len());
                }
                Ok(())
            })();
            if let Err(e) = result {
                self.issue("layer-render", Some(label), e.to_string());
            }
            let samples_background = frame.draws[start..]
                .iter()
                .any(|d| d.material.textures.iter().any(|b| b.texture == background))
                || frame.passes[background_pass + 1..]
                    .iter()
                    .filter_map(|p| p.effect.as_ref())
                    .any(|e| e.inputs().contains(&background));
            if !samples_background {
                frame.passes.remove(background_pass);
            }
            // Image/effect passes are emitted in place. Remaining geometry is submitted
            // once in a scene pass, preserving dependency and material pass order.
            let scheduled = frame
                .passes
                .iter()
                .flat_map(|p| p.draws.iter().copied())
                .collect::<std::collections::BTreeSet<_>>();
            let draws = (start..frame.draws.len())
                .filter(|i| !scheduled.contains(i))
                .collect::<Vec<_>>();
            if !draws.is_empty() {
                frame.passes.push(RenderPass {
                    name: label.into(),
                    target: Target::Texture(canvas),
                    draws,
                    ..Default::default()
                });
            }
        }
        self.emit_reflection(
            &mut frame,
            reflection_id,
            &reflection_draws,
            projection,
            view,
            eye,
        )?;
        self.update_pose_metadata(&objects)?;
        let mut result = canvas;
        if boolean(self.scene.pointer("/general/bloom"), false) {
            let out = target(&mut frame, "scene-bloom", w, h, format);
            frame.passes.push(RenderPass {
                name: "scene-bloom".into(),
                target: Target::Texture(out),
                effect: Some(wpe_render::effect::Effect::Bloom {
                    input: result,
                    threshold: number(self.scene.pointer("/general/bloomthreshold"), 0.8),
                    intensity: number(self.scene.pointer("/general/bloomstrength"), 1.),
                    sigma: 2.,
                    radius: 4,
                }),
                ..Default::default()
            });
            result = out;
        }
        if let Some(lut) = self
            .scene
            .pointer("/general/colorcorrection")
            .map(literal)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            match self
                .assets
                .bind_texture(&self.project, lut, &mut frame, self.time as f32)
            {
                Ok(lut) => {
                    let out = target(&mut frame, "scene-colorgrade", w, h, format);
                    frame.passes.push(RenderPass {
                        name: "scene-colorgrade".into(),
                        target: Target::Texture(out),
                        effect: Some(wpe_render::effect::Effect::Lut3D {
                            input: result,
                            lut,
                            strength: 1.,
                        }),
                        ..Default::default()
                    });
                    result = out;
                }
                Err(e) => self.issue("colorgrade", None, e.to_string()),
            }
        }
        copy_pass(&mut frame, "present", result, Target::Screen);
        frame
            .issues
            .extend(self.issues.iter().map(|i| wpe_render::CapabilityIssue {
                code: i.code.clone(),
                resource: i.object.clone(),
                message: i.message.clone(),
            }));
        frame.validate()?;
        Ok(frame)
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn uniforms(
        &self,
        material: &mut Material,
        world: Mat4,
        projection: Mat4,
        view: Mat4,
        eye: Vec3,
        object: &Value,
        input: &PlayerInput,
    ) {
        let mvp = projection * view * world;
        let matrices = [
            ("g_ModelMatrix", world),
            ("g_ViewMatrix", view),
            ("g_ProjectionMatrix", projection),
            ("g_ModelViewProjectionMatrix", mvp),
            ("g_ViewProjectionMatrix", projection * view),
            (
                "g_ModelViewProjectionMatrixInverse",
                mvp.inverse().unwrap_or_default(),
            ),
            (
                "g_ViewProjectionMatrixInverse",
                (projection * view).inverse().unwrap_or_default(),
            ),
            ("g_ModelMatrixInverse", world.inverse().unwrap_or_default()),
            ("g_ViewMatrixInverse", view.inverse().unwrap_or_default()),
        ];
        for (name, matrix) in matrices {
            material.uniforms.values.insert(
                name.into(),
                UniformValue::Matrix {
                    columns: 4,
                    rows: 4,
                    values: matrix.0.to_vec(),
                },
            );
        }
        let normal = world.inverse().unwrap_or_default();
        material.uniforms.values.insert(
            "g_NormalModelMatrix".into(),
            UniformValue::Matrix {
                columns: 3,
                rows: 3,
                values: (0..3)
                    .flat_map(|c| (0..3).map(move |r| normal.0[r * 4 + c]))
                    .collect(),
            },
        );
        let rgb = vec3(object.get("color"), Vec3::ONE) * number(object.get("brightness"), 1.);
        let alpha = number(object.get("alpha"), 1.);
        for (name, value) in [
            ("g_OrientationRight", UniformValue::Vector(vec![1., 0., 0.])),
            ("g_OrientationUp", UniformValue::Vector(vec![0., 1., 0.])),
            (
                "g_OrientationForward",
                UniformValue::Vector(vec![0., 0., 1.]),
            ),
            (
                "g_ViewRight",
                UniformValue::Vector(vec![view.0[0], view.0[4], view.0[8]]),
            ),
            (
                "g_ViewUp",
                UniformValue::Vector(vec![view.0[1], view.0[5], view.0[9]]),
            ),
            ("g_EyePosition", UniformValue::Vector(eye.0.to_vec())),
            ("g_Time", UniformValue::Float(self.time as f32)),
            ("g_Alpha", UniformValue::Float(alpha)),
            ("g_Color", UniformValue::Vector(rgb.0.to_vec())),
            (
                "g_Color4",
                UniformValue::Vector(vec![rgb.0[0], rgb.0[1], rgb.0[2], alpha]),
            ),
            (
                "g_Texture0Rotation",
                UniformValue::Vector(vec![1., 0., 0., 1.]),
            ),
            ("g_Texture0Translation", UniformValue::Vector(vec![0., 0.])),
            (
                "g_CursorPosition",
                UniformValue::Vector(vec![input.cursor[0], 1. - input.cursor[1]]),
            ),
            (
                "g_PointerPosition",
                UniformValue::Vector(vec![input.cursor[0], 1. - input.cursor[1]]),
            ),
        ] {
            material.uniforms.values.entry(name.into()).or_insert(value);
        }
        let ambient = vec3(self.scene.pointer("/general/ambientcolor"), Vec3([0.2; 3]));
        material.uniforms.values.insert(
            "g_AmbientColor".into(),
            UniformValue::Vector(ambient.0.to_vec()),
        );
        let lights = self
            .scene
            .get("objects")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|o| o.get("light").is_some() && boolean(o.get("visible"), true))
            .take(16)
            .collect::<Vec<_>>();
        material.uniforms.values.insert(
            "wpe_LightCount".into(),
            UniformValue::Int(lights.len() as i32),
        );
        for (name, values) in [
            (
                "wpe_LightPositionRadius",
                lights
                    .iter()
                    .map(|o| {
                        let p = vec3(o.get("origin"), Vec3::ZERO);
                        UniformValue::Vector(vec![
                            p.0[0],
                            p.0[1],
                            p.0[2],
                            number(o.get("radius"), 100.),
                        ])
                    })
                    .collect(),
            ),
            (
                "wpe_LightColorExponent",
                lights
                    .iter()
                    .map(|o| {
                        let p = vec3(o.get("color"), Vec3::ONE) * number(o.get("intensity"), 1.);
                        UniformValue::Vector(vec![
                            p.0[0],
                            p.0[1],
                            p.0[2],
                            number(o.get("exponent"), 2.),
                        ])
                    })
                    .collect(),
            ),
        ] {
            material
                .uniforms
                .values
                .insert(name.into(), UniformValue::Array(values));
        }
        material
            .uniforms
            .values
            .extend(self.lighting.uniforms.clone());
        self.environment(material, world, projection, view, object, input);
        puppet::skin_uniforms(material);
    }
    #[allow(clippy::too_many_arguments)]
    fn image(
        &mut self,
        object: &Value,
        key: &str,
        label: &str,
        file: &str,
        world: Mat4,
        projection: Mat4,
        view: Mat4,
        eye: Vec3,
        input: &PlayerInput,
        canvas: u64,
        background: u64,
        resource_only: bool,
        frame: &mut RenderFrame,
    ) -> Result<()> {
        let descriptor = self.assets.json(&self.project, file)?;
        let fullscreen = boolean(descriptor.get("fullscreen"), false);
        let passthrough = boolean(descriptor.get("passthrough"), false)
            || boolean(descriptor.get("projectlayer"), false);
        let solid = boolean(descriptor.get("solidlayer"), false);
        let time = number(
            object.pointer("/__textureAnimation/__time"),
            self.time as f32,
        );
        let material_name = descriptor.get("material").and_then(Value::as_str);
        let mut size = vec3(
            object.get("size"),
            Vec3::new(
                number(descriptor.get("width"), 1.),
                number(descriptor.get("height"), 1.),
                0.,
            ),
        )
        .0;
        let anchor = anchor(
            object
                .get("alignment")
                .or_else(|| object.get("anchor"))
                .and_then(Value::as_str)
                .unwrap_or("center"),
        );
        let texture_name = if let Some(name) = material_name {
            self.assets
                .json(&self.project, name)?
                .pointer("/passes/0/textures/0")
                .and_then(Value::as_str)
                .map(str::to_owned)
        } else {
            None
        };
        let asset = if let Some(name) = texture_name.as_ref().filter(|s| !s.starts_with("_rt_")) {
            Some(self.assets.texture_asset(&self.project, name)?)
        } else {
            None
        };
        if boolean(descriptor.get("autosize"), false)
            && !passthrough
            && let Some(asset) = &asset
        {
            let s = asset.logical_frame(asset.frame_index(time));
            size[0] = s[0];
            size[1] = s[1];
        }
        if fullscreen {
            size = [frame.width as f32, frame.height as f32, 0.];
        }
        let size = [size[0].max(0.).trunc(), size[1].max(0.).trunc()];
        if size[0] == 0. || size[1] == 0. {
            return Ok(());
        }
        let effects = object
            .get("effects")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter(|e| boolean(e.get("visible"), true))
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let offscreen = !effects.is_empty() || passthrough || resource_only;
        let format = frame.textures[&canvas].format;
        let layer = if offscreen {
            target(
                frame,
                &format!("layer-{key}-base"),
                size[0] as u32,
                size[1] as u32,
                format,
            )
        } else {
            canvas
        };
        let mut draw_world = world;
        let mut draw_projection = projection;
        let mut draw_view = view;
        let mut base_anchor = anchor;
        if offscreen {
            draw_world = Mat4::IDENTITY;
            draw_projection = centered_ortho(size);
            draw_view = Mat4::IDENTITY;
            base_anchor = [0.5; 2];
        }
        if fullscreen {
            draw_world = Mat4::IDENTITY;
            draw_projection = centered_ortho(size);
            draw_view = Mat4::IDENTITY;
            base_anchor = [0.5; 2];
        }
        let color = tint(object);
        let mut materials = if passthrough {
            vec![Material {
                textures: vec![TextureBinding {
                    name: "g_Texture0".into(),
                    slot: 0,
                    texture: background,
                }],
                state: wpe_render::PipelineState {
                    blend: BlendState::Replace,
                    ..Default::default()
                },
                ..Default::default()
            }]
        } else if let Some(name) = material_name {
            if self.models.contains_key(key) {
                self.assets.puppet_materials(
                    &self.project,
                    name,
                    Some(&cpu_deformed_material(
                        object.get("instance").or_else(|| object.get("material")),
                    )),
                    frame,
                    time,
                )?
            } else {
                self.assets.materials(
                    &self.project,
                    name,
                    object.get("instance").or_else(|| object.get("material")),
                    frame,
                    time,
                )?
            }
        } else if solid {
            vec![Material {
                cpu: CpuProgram::Solid { color: [1.; 4] },
                ..Default::default()
            }]
        } else {
            return Err(Error::invalid(label, "image has no material"));
        };
        let puppet = self
            .models
            .get(key)
            .map(|model| {
                model
                    .skinned_meshes()
                    .map(|meshes| (meshes, model.pose.blend_map))
                    .map_err(|error| Error::invalid(label, error.to_string()))
            })
            .transpose()?;
        if let Some((meshes, blend_map)) = &puppet
            && let Some(base) = materials.first().and_then(|material| {
                material
                    .textures
                    .iter()
                    .find(|binding| binding.slot == 0)
                    .map(|binding| binding.texture)
            })
        {
            let texture = &frame.textures[&base];
            let [width, height, _] = texture.data.dimensions();
            let base_asset = self.assets.textures.values().find(|asset| asset.id == base);
            let logical = base_asset
                .map(|asset| [asset.logical[0] as u32, asset.logical[1] as u32])
                .unwrap_or([width, height]);
            let uv_max = base_asset
                .map(|asset| {
                    [
                        asset.logical[0] / asset.storage[0],
                        asset.logical[1] / asset.storage[1],
                    ]
                })
                .unwrap_or([1.; 2]);
            if let Some(albedo) = self.puppet_albedo(
                puppet::ChannelImage {
                    key,
                    label,
                    object,
                    base,
                    logical,
                    uv_max,
                    meshes,
                    blend_map: *blend_map,
                },
                input,
                frame,
            )? {
                for material in &mut materials {
                    puppet::bind_albedo(material, albedo, frame);
                }
            }
        }
        let mut draws = Vec::new();
        for material in &mut materials {
            if !offscreen {
                object_blend(material, object);
            }
            if object.get("clampuvs").is_some() {
                let mode = if boolean(object.get("clampuvs"), true) {
                    wpe_render::AddressMode::Clamp
                } else {
                    wpe_render::AddressMode::Repeat
                };
                for binding in &material.textures {
                    let mut sampler = material
                        .samplers
                        .get(&binding.slot)
                        .cloned()
                        .unwrap_or_else(|| frame.textures[&binding.texture].sampler.clone());
                    sampler.address_u = mode;
                    sampler.address_v = mode;
                    material.samplers.insert(binding.slot, sampler);
                }
            }
            self.uniforms(
                material,
                draw_world,
                draw_projection,
                draw_view,
                eye,
                object,
                input,
            );
            let uv = |p: [f32; 2]| {
                if !offscreen || !passthrough {
                    asset
                        .as_ref()
                        .map(|a| {
                            let logical = a.logical_frame(a.frame_index(time));
                            let inset = if boolean(descriptor.get("nopadding"), false) {
                                [0.; 2]
                            } else {
                                [0.15 / logical[0], 0.15 / logical[1]]
                            };
                            a.uv(
                                a.frame_index(time),
                                [
                                    inset[0] + p[0] * (1. - 2. * inset[0]),
                                    inset[1] + p[1] * (1. - 2. * inset[1]),
                                ],
                            )
                        })
                        .unwrap_or(p)
                } else {
                    p
                }
            };
            if let Some((meshes, _)) = &puppet {
                let offset = vec3(descriptor.get("cropoffset"), Vec3::ZERO);
                for mesh in meshes.iter().filter(|mesh| mesh.texture_channels.is_none()) {
                    let vertices = mesh
                        .vertices
                        .iter()
                        .map(|v| {
                            vertex(
                                Vec3(v.position) + offset,
                                Vec3(v.normal),
                                v.uv,
                                puppet::model_color(v, color, material),
                                draw_world,
                                draw_projection * draw_view,
                                material.vertex.is_some(),
                            )
                        })
                        .collect();
                    let index = frame.draws.len();
                    frame.draws.push(DrawCall {
                        name: label.into(),
                        vertices,
                        indices: mesh.indices.clone(),
                        vertex_streams: if material.vertex.is_some() {
                            puppet::model_streams(&mesh.vertices, material)
                        } else {
                            BTreeMap::new()
                        },
                        material: material.clone(),
                        ..Default::default()
                    });
                    draws.push(index);
                }
            } else {
                let vertices = quad_vertices(
                    size,
                    base_anchor,
                    color,
                    uv,
                    draw_world,
                    draw_projection * draw_view,
                    material.vertex.is_some(),
                );
                let index = frame.draws.len();
                frame.draws.push(DrawCall {
                    name: label.into(),
                    vertices,
                    indices: vec![0, 2, 1, 1, 2, 3],
                    instances: draw_instances(material),
                    material: material.clone(),
                    ..Default::default()
                });
                draws.push(index);
            }
        }
        frame.passes.push(RenderPass {
            name: format!("{label}:base"),
            target: Target::Texture(layer),
            clear: offscreen.then_some([0.; 4]),
            draws,
            ..Default::default()
        });
        let image_alias = format!(
            "_rt_imagelayercomposite_{}_a",
            object.get("id").unwrap_or(&Value::Null)
        );
        self.assets.targets.insert(image_alias.clone(), layer);
        if offscreen {
            let effect_world = if fullscreen {
                Mat4::IDENTITY
            } else {
                world
                    * Mat4::trs(
                        Vec3::new((0.5 - anchor[0]) * size[0], (0.5 - anchor[1]) * size[1], 0.),
                        Quat::IDENTITY,
                        Vec3::new(size[0] * 0.5, size[1] * 0.5, 1.),
                    )
            };
            let effect_camera = if fullscreen {
                Mat4::IDENTITY
            } else {
                projection * view
            };
            let output = self.effects(
                &effects,
                key,
                label,
                layer,
                background,
                size,
                effect_world,
                effect_camera,
                object,
                input,
                frame,
            )?;
            self.assets.targets.insert(image_alias, output);
            if resource_only {
                return Ok(());
            }
            let world = if fullscreen { Mat4::IDENTITY } else { world };
            let camera = if fullscreen {
                centered_ortho(size)
            } else {
                projection * view
            };
            let mut material = Material {
                textures: vec![TextureBinding {
                    name: "g_Texture0".into(),
                    slot: 0,
                    texture: output,
                }],
                ..Default::default()
            };
            if fullscreen || passthrough {
                material.state.blend = BlendState::Replace;
            }
            object_blend(&mut material, object);
            let vertices = quad_vertices(
                size,
                if fullscreen { [0.5; 2] } else { anchor },
                [1.; 4],
                |uv| uv,
                world,
                camera,
                false,
            );
            let i = frame.draws.len();
            frame.draws.push(DrawCall {
                name: format!("{label}:composite"),
                vertices,
                indices: vec![0, 2, 1, 1, 2, 3],
                material,
                ..Default::default()
            });
            frame.passes.push(RenderPass {
                name: format!("{label}:composite"),
                target: Target::Texture(canvas),
                draws: vec![i],
                ..Default::default()
            });
        }
        {
            self.hits.push(HitRegion {
                id: object.get("id").cloned().unwrap_or(Value::Null),
                inverse: world.inverse().unwrap_or_default(),
                size,
                anchor,
                screen_space: fullscreen,
                triangles: None,
            });
        }
        Ok(())
    }
    #[allow(clippy::too_many_arguments)]
    fn effects(
        &mut self,
        effects: &[Value],
        key: &str,
        label: &str,
        input_texture: u64,
        background: u64,
        size: [f32; 2],
        effect_world: Mat4,
        effect_camera: Mat4,
        object: &Value,
        input: &PlayerInput,
        frame: &mut RenderFrame,
    ) -> Result<u64> {
        let mut current = input_texture;
        let format = frame.textures[&input_texture].format;
        for (fallback_index, effect) in effects.iter().enumerate() {
            let ei = effect
                .get("__wpe_index")
                .and_then(Value::as_u64)
                .map(|n| n as usize)
                .unwrap_or(fallback_index);
            let file = effect
                .get("file")
                .and_then(Value::as_str)
                .ok_or_else(|| Error::invalid(label, "effect file missing"))?;
            let scope = file.rsplit_once('/').map(|p| p.0).unwrap_or("");
            let config = self.assets.json(&self.project, file)?;
            let passes = config
                .get("passes")
                .and_then(Value::as_array)
                .ok_or_else(|| Error::invalid(file, "effect passes missing"))?;
            let frozen_feedback = self.render_only
                && passes.iter().any(|p| p.get("command").is_some())
                && self
                    .assets
                    .fbo_state
                    .keys()
                    .any(|name| name.starts_with(&format!("{key}:{ei}:")));
            let mut combos = BTreeMap::new();
            for (pi, route) in passes.iter().enumerate() {
                if let Some(material) = route.get("material").and_then(Value::as_str)
                    && let Ok(material) = self.assets.json_scoped(&self.project, material, scope)
                {
                    collect_combos(material.pointer("/passes/0/combos"), &mut combos);
                }
                collect_combos(
                    effect
                        .get("passes")
                        .and_then(|p| p.get(pi))
                        .and_then(|p| p.get("combos")),
                    &mut combos,
                );
            }
            for fbo in config
                .get("fbos")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if !conditions(fbo, &combos) {
                    continue;
                }
                let Some(name) = fbo.get("name").and_then(Value::as_str) else {
                    continue;
                };
                let mut scale = number(fbo.get("scale"), 1.).max(0.001);
                if let Some(fit) = fbo.get("fit") {
                    scale = size[0].max(size[1]) / number(Some(fit), 256.).max(1.);
                }
                let w = number(fbo.get("width"), size[0] / scale).clamp(1., 16384.) as u32;
                let h = number(fbo.get("height"), size[1] / scale).clamp(1., 16384.) as u32;
                let fmt = texture_format(fbo.get("format").and_then(Value::as_str), format);
                let logical = format!("{key}:{ei}:{name}");
                let physical = *self
                    .assets
                    .fbo_state
                    .entry(logical.clone())
                    .or_insert_with(|| {
                        resource_id(&if boolean(fbo.get("unique"), false) {
                            format!("layer-{key}-effect-{ei}-{name}")
                        } else {
                            format!(
                                "shared-fbo:{name}:{w}:{h}:{fmt:?}:{}",
                                fbo.get("uvs").unwrap_or(&Value::Null)
                            )
                        })
                    });
                frame
                    .textures
                    .entry(physical)
                    .or_insert_with(|| Texture::target_with_format(name, w, h, fmt));
                if fbo.get("uvs").and_then(Value::as_str) == Some("repeat")
                    && let Some(texture) = frame.textures.get_mut(&physical)
                {
                    texture.sampler.address_u = wpe_render::AddressMode::Repeat;
                    texture.sampler.address_v = wpe_render::AddressMode::Repeat;
                }
                self.assets
                    .targets
                    .insert(name.to_ascii_lowercase(), physical);
                let initial = self
                    .assets
                    .initialized_targets
                    .insert(physical, (w, h, fmt))
                    != Some((w, h, fmt));
                let clear =
                    self.assets.pending_clears.remove(&logical).or_else(|| {
                        initial.then(|| fbo.get("clear").map(color4).unwrap_or([0.; 4]))
                    });
                if let Some(clear) = clear {
                    frame.passes.push(RenderPass {
                        name: format!("{label}:fbo-clear:{name}"),
                        target: Target::Texture(physical),
                        clear: Some(clear),
                        ..Default::default()
                    });
                }
            }
            for (pi, route) in passes.iter().enumerate() {
                if !conditions(route, &combos) {
                    continue;
                }
                if frozen_feedback
                    && (route.get("command").is_some() || route.get("target").is_some())
                {
                    continue;
                }
                if let Some(command) = route.get("command").and_then(Value::as_str) {
                    let source = route.get("source").and_then(Value::as_str).unwrap_or("");
                    let target_name = route
                        .get("target")
                        .and_then(Value::as_str)
                        .ok_or_else(|| Error::invalid(file, "effect command target missing"))?;
                    let target_id = *self
                        .assets
                        .targets
                        .get(&target_name.to_ascii_lowercase())
                        .ok_or_else(|| {
                            Error::invalid(target_name, "effect command target missing")
                        })?;
                    match command {
                        "clear" => frame.passes.push(RenderPass {
                            name: format!("{label}:command-clear"),
                            target: Target::Texture(target_id),
                            clear: Some(route.get("clear").map(color4).unwrap_or([0.; 4])),
                            ..Default::default()
                        }),
                        "copy" | "swap" => {
                            let source_id = *self
                                .assets
                                .targets
                                .get(&source.to_ascii_lowercase())
                                .ok_or_else(|| {
                                    Error::invalid(source, "effect command source missing")
                                })?;
                            if command == "copy" {
                                if source_id != target_id {
                                    copy_pass(
                                        frame,
                                        "effect-command-copy",
                                        source_id,
                                        Target::Texture(target_id),
                                    );
                                }
                            } else {
                                if frame.textures[&source_id].data.dimensions()
                                    != frame.textures[&target_id].data.dimensions()
                                    || frame.textures[&source_id].format
                                        != frame.textures[&target_id].format
                                {
                                    return Err(Error::invalid(
                                        file,
                                        "swap targets have incompatible dimensions/format",
                                    ));
                                }
                                self.assets
                                    .targets
                                    .insert(source.to_ascii_lowercase(), target_id);
                                self.assets
                                    .targets
                                    .insert(target_name.to_ascii_lowercase(), source_id);
                                self.assets
                                    .fbo_state
                                    .insert(format!("{key}:{ei}:{source}"), target_id);
                                self.assets
                                    .fbo_state
                                    .insert(format!("{key}:{ei}:{target_name}"), source_id);
                            }
                        }
                        other => return Err(Error::Unsupported(format!("effect command {other}"))),
                    }
                    continue;
                }
                let overrides = effect.get("passes").and_then(|p| p.get(pi));
                let material_name = route
                    .get("material")
                    .or_else(|| overrides.and_then(|v| v.get("material")))
                    .and_then(Value::as_str)
                    .ok_or_else(|| Error::invalid(file, "effect material missing"))?;
                let material_config =
                    self.assets
                        .json_scoped(&self.project, material_name, scope)?;
                let material_passes = material_config
                    .get("passes")
                    .and_then(Value::as_array)
                    .ok_or_else(|| Error::invalid(material_name, "material passes missing"))?;
                self.assets.targets.insert("_rt_previous".into(), current);
                self.assets
                    .targets
                    .insert("_rt_original".into(), input_texture);
                self.assets
                    .targets
                    .insert("_rt_image".into(), input_texture);
                self.assets
                    .targets
                    .insert("_rt_fullframebuffer".into(), background);
                let target_name = route.get("target").and_then(Value::as_str);
                let target_id = if let Some(name) = target_name {
                    *self
                        .assets
                        .targets
                        .get(&name.to_ascii_lowercase())
                        .ok_or_else(|| Error::invalid(name, "effect target is not declared"))?
                } else {
                    target(
                        frame,
                        &format!("layer-{key}-effect-{ei}-output-{pi}"),
                        size[0] as u32,
                        size[1] as u32,
                        format,
                    )
                };
                let dimensions = frame.textures[&target_id].data.dimensions();
                let mut draws = Vec::new();
                let mut restore_aliases = Vec::new();
                for (mi, base) in material_passes.iter().enumerate() {
                    let mut pass = base.clone();
                    if let Some(overrides) = overrides {
                        merge_pass(&mut pass, overrides);
                    }
                    let textures = pass
                        .as_object_mut()
                        .ok_or_else(|| {
                            Error::invalid(material_name, "material pass must be object")
                        })?
                        .entry("textures")
                        .or_insert_with(|| json!([]));
                    if !textures.is_array() {
                        *textures = json!([])
                    }
                    let textures = textures.as_array_mut().unwrap();
                    if textures.is_empty() {
                        textures.push(json!("_rt_previous"));
                    } else if textures[0].is_null() {
                        textures[0] = json!("_rt_previous");
                    }
                    for bind in route
                        .get("bind")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                    {
                        if !conditions(bind, &combos) {
                            continue;
                        }
                        let slot = number(bind.get("index"), 0.) as usize;
                        if slot > 31 {
                            return Err(Error::invalid(file, "effect binding slot exceeds31"));
                        }
                        let name = match bind
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("previous")
                        {
                            "previous" | "prev" => "_rt_previous",
                            "original" | "image" => "_rt_original",
                            name => name,
                        };
                        while textures.len() <= slot {
                            textures.push(Value::Null)
                        }
                        textures[slot] = json!(name);
                    }
                    // Editor-authored unique FBO references can retain a former
                    // layer suffix. Resolve them only against this effect's own
                    // declared FBO names; never select an unrelated layer's target.
                    for texture in textures.iter_mut() {
                        if let Some(name) = texture.as_str()
                            && let Some(base) = config
                                .get("fbos")
                                .and_then(Value::as_array)
                                .into_iter()
                                .flatten()
                                .filter_map(|f| f.get("name").and_then(Value::as_str))
                                .filter(|base| {
                                    name.to_ascii_lowercase()
                                        .starts_with(&format!("{}_", base.to_ascii_lowercase()))
                                })
                                .max_by_key(|base| base.len())
                            && self.assets.targets.contains_key(&base.to_ascii_lowercase())
                        {
                            *texture = json!(base);
                        }
                    }
                    let aliases = textures
                        .iter()
                        .filter_map(Value::as_str)
                        .filter(|name| {
                            self.assets.targets.get(&name.to_ascii_lowercase()) == Some(&target_id)
                        })
                        .map(str::to_owned)
                        .collect::<Vec<_>>();
                    if !aliases.is_empty() {
                        let snapshot = target(
                            frame,
                            &format!("layer-{key}-effect-{ei}-feedback-{pi}-{mi}"),
                            dimensions[0],
                            dimensions[1],
                            frame.textures[&target_id].format,
                        );
                        copy_pass(
                            frame,
                            "effect-feedback-snapshot",
                            target_id,
                            Target::Texture(snapshot),
                        );
                        for alias in aliases {
                            self.assets
                                .targets
                                .insert(alias.to_ascii_lowercase(), snapshot);
                            restore_aliases.push(alias);
                        }
                    }
                    let mut material = self.assets.pass_scoped(
                        &self.project,
                        &format!("{file}:{pi}:{mi}"),
                        &pass,
                        scope,
                        frame,
                        self.time as f32,
                    )?;
                    self.uniforms(
                        &mut material,
                        Mat4::IDENTITY,
                        Mat4::IDENTITY,
                        Mat4::IDENTITY,
                        Vec3::new(0., 0., 1.),
                        object,
                        input,
                    );
                    self.effect_environment(&mut material, effect_world, effect_camera);
                    let vertices = quad_vertices(
                        [2.; 2],
                        [0.5; 2],
                        [1.; 4],
                        |uv| uv,
                        Mat4::IDENTITY,
                        Mat4::IDENTITY,
                        material.vertex.is_some(),
                    );
                    let index = frame.draws.len();
                    frame.draws.push(DrawCall {
                        name: format!("{label}:effect-{ei}-{pi}-{mi}"),
                        vertices,
                        indices: vec![0, 2, 1, 1, 2, 3],
                        material,
                        ..Default::default()
                    });
                    draws.push(index);
                }
                frame.passes.push(RenderPass {
                    name: format!("{label}:effect-{ei}-{pi}"),
                    target: Target::Texture(target_id),
                    clear: target_name.is_none().then_some([0.; 4]),
                    draws,
                    ..Default::default()
                });
                for alias in restore_aliases {
                    self.assets
                        .targets
                        .insert(alias.to_ascii_lowercase(), target_id);
                }
                if target_name.is_none() {
                    current = target_id;
                }
            }
        }
        Ok(current)
    }
    #[allow(clippy::too_many_arguments)]
    fn particle_mesh(
        &mut self,
        label: &str,
        material: &str,
        geometry: wpe_sim::particle::ParticleGeometry,
        world: Mat4,
        projection: Mat4,
        view: Mat4,
        eye: Vec3,
        input: &PlayerInput,
        frame: &mut RenderFrame,
    ) -> Result<()> {
        if geometry.indices.is_empty() {
            return Ok(());
        }
        let renderer = &geometry.renderer;
        let kind = renderer
            .get("name")
            .or_else(|| renderer.get("type"))
            .and_then(Value::as_str)
            .unwrap_or("sprite");
        let materials = if material.is_empty() {
            vec![Material {
                cpu: CpuProgram::Solid { color: [1.; 4] },
                ..Default::default()
            }]
        } else {
            let config = self.assets.json(&self.project, material)?;
            let particle_shader = config
                .pointer("/passes/0/shader")
                .and_then(Value::as_str)
                .is_some_and(|name| name.contains("particle"));
            let overrides=particle_shader.then(||json!({"combos":{
                "THICKFORMAT":1,"TRAILRENDERER":i32::from(matches!(kind,"spritetrail"|"ropetrail")),
                "TRAILSCROLLALPHA":i32::from(boolean(renderer.get("uvscrolling"),false)),
                "TRAILFADEALPHA":i32::from(boolean(renderer.get("fadealpha"),false)),
                "TRAILFADESIZE":i32::from(boolean(renderer.get("fadesize"),false))
            }}));
            self.assets.materials(
                &self.project,
                material,
                overrides.as_ref(),
                frame,
                self.time as f32,
            )?
        };
        for mut material in materials {
            self.uniforms(
                &mut material,
                world,
                projection,
                view,
                eye,
                &Value::Null,
                input,
            );
            let inverse = world.inverse().unwrap_or_default();
            let (right, up) =
                if renderer.get("orientation").and_then(Value::as_str) == Some("world") {
                    (Vec3::new(1., 0., 0.), Vec3::new(0., 1., 0.))
                } else {
                    (
                        inverse
                            .vector(Vec3::new(view.0[0], view.0[4], view.0[8]))
                            .normalize(),
                        inverse
                            .vector(Vec3::new(view.0[1], view.0[5], view.0[9]))
                            .normalize(),
                    )
                };
            for (name, value) in [
                ("g_OrientationRight", right),
                ("g_OrientationUp", up),
                ("g_OrientationForward", right.cross(up).normalize()),
            ] {
                material
                    .uniforms
                    .values
                    .insert(name.into(), UniformValue::Vector(value.0.to_vec()));
            }
            let vertices = geometry
                .positions
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    vertex(
                        Vec3(*p),
                        Vec3::new(0., 0., 1.),
                        geometry.uvs[i],
                        geometry.colors[i],
                        world,
                        projection * view,
                        material.vertex.is_some(),
                    )
                })
                .collect();
            let vertex_streams = if material.vertex.is_some() {
                geometry
                    .attributes
                    .iter()
                    .map(|(name, (components, data))| {
                        (
                            name.clone(),
                            VertexStream {
                                components: *components,
                                data: VertexStreamData::Float32(data.clone()),
                            },
                        )
                    })
                    .collect()
            } else {
                BTreeMap::new()
            };
            let segments = number(renderer.get("segments"), 4.).max(2.);
            let vars = if matches!(kind, "rope" | "ropetrail") {
                vec![
                    segments,
                    0.,
                    1.,
                    segments * number(renderer.get("length"), 1.),
                ]
            } else {
                vec![
                    number(renderer.get("length"), 0.05),
                    number(renderer.get("maxlength"), 10.),
                    number(renderer.get("minlength"), 0.),
                    0.,
                ]
            };
            material
                .uniforms
                .values
                .insert("g_RenderVar0".into(), UniformValue::Vector(vars));
            let sprite = material
                .textures
                .iter()
                .find(|b| b.slot == 0)
                .and_then(|b| self.assets.textures.values().find(|a| a.id == b.texture));
            let sprite_vars = if let Some(sprite) = sprite {
                let size = sprite.logical_frame(0);
                vec![
                    size[0] / sprite.storage[0],
                    size[1] / sprite.storage[1],
                    sprite.frames.len().max(1) as f32,
                    size[1] / size[0].max(0.00001),
                ]
            } else {
                vec![1.; 4]
            };
            material
                .uniforms
                .values
                .insert("g_RenderVar1".into(), UniformValue::Vector(sprite_vars));
            frame.draws.push(DrawCall {
                name: label.into(),
                vertices,
                indices: geometry.indices.clone(),
                material,
                vertex_streams,
                ..Default::default()
            });
        }
        Ok(())
    }
    #[allow(clippy::too_many_arguments)]
    fn procedural(
        &mut self,
        object: &Value,
        id: &Value,
        label: &str,
        world: Mat4,
        projection: Mat4,
        view: Mat4,
        eye: Vec3,
        input: &PlayerInput,
        frame: &mut RenderFrame,
    ) -> Result<()> {
        let mut row = if let Some(id) = id.as_str() {
            if let Some(row) = self
                .scene
                .get("__modelData")
                .and_then(Value::as_array)
                .and_then(|a| {
                    a.iter()
                        .find(|m| m.get("id").and_then(Value::as_str) == Some(id))
                })
            {
                row.clone()
            } else {
                self.assets.json(&self.project, id)?
            }
        } else {
            id.clone()
        };
        if row.get("shapes").is_none() {
            row = json!({"shapes":if row.is_array(){row}else{json!([row])}});
        }
        for shape in row
            .get("shapes")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|s| !s.is_null())
        {
            let format = shape
                .get("vertexFormat")
                .and_then(Value::as_array)
                .ok_or_else(|| Error::invalid(label, "vertex format missing"))?;
            let widths = format
                .iter()
                .map(|name| match name.as_str() {
                    Some("position" | "normal") => 3,
                    Some("uv") => 2,
                    Some("tangentSigned" | "color") => 4,
                    _ => 0,
                })
                .collect::<Vec<_>>();
            let stride = widths.iter().sum::<usize>();
            if stride == 0 || widths.contains(&0) {
                return Err(Error::invalid(label, "unknown procedural attribute"));
            }
            let data = shape
                .get("vertexBuffer")
                .and_then(Value::as_array)
                .ok_or_else(|| Error::invalid(label, "vertex buffer missing"))?;
            if data.len() % stride != 0 {
                return Err(Error::invalid(label, "vertex stride mismatch"));
            }
            let mut vertices = Vec::new();
            let mut tangents = Vec::new();
            for row in data.chunks_exact(stride) {
                let mut v = GpuVertex::default();
                v.position[3] = 1.;
                let mut offset = 0;
                for (name, width) in format.iter().zip(&widths) {
                    let values = row[offset..offset + width]
                        .iter()
                        .map(|n| {
                            n.as_f64()
                                .map(|n| n as f32)
                                .filter(|n| n.is_finite())
                                .ok_or_else(|| Error::invalid(label, "nonfinite vertex"))
                        })
                        .collect::<Result<Vec<_>>>()?;
                    match name.as_str() {
                        Some("position") => v.position[..3].copy_from_slice(&values),
                        Some("normal") => v.normal[..3].copy_from_slice(&values),
                        Some("uv") => v.uv[..2].copy_from_slice(&values),
                        Some("color") => v.color.copy_from_slice(&values),
                        Some("tangentSigned") => tangents.extend(values),
                        _ => {}
                    }
                    offset += width;
                }
                vertices.push(v);
            }
            let indices = if let Some(indices) = shape.get("indexBuffer").and_then(Value::as_array)
            {
                indices
                    .iter()
                    .map(|n| {
                        n.as_u64()
                            .filter(|n| *n < (vertices.len() as u64))
                            .map(|n| n as u32)
                            .ok_or_else(|| Error::invalid(label, "index out of bounds"))
                    })
                    .collect::<Result<Vec<_>>>()?
            } else {
                (0..vertices.len() as u32).collect()
            };
            self.mesh_hit_region(
                object,
                world,
                &vertices
                    .iter()
                    .map(|v| [v.position[0], v.position[1], v.position[2]])
                    .collect::<Vec<_>>(),
                &indices,
            );
            let materials = if let Some(name) = shape.get("material").and_then(Value::as_str) {
                self.assets
                    .materials(&self.project, name, None, frame, self.time as f32)?
            } else {
                vec![Material {
                    cpu: CpuProgram::Solid { color: [1.; 4] },
                    ..Default::default()
                }]
            };
            for mut material in materials {
                self.uniforms(&mut material, world, projection, view, eye, object, input);
                let mut vertices = vertices.clone();
                if material.vertex.is_none() {
                    for v in &mut vertices {
                        v.position = (projection * view * world).transform4(v.position);
                    }
                }
                let mut draw = DrawCall {
                    name: label.into(),
                    vertices,
                    indices: indices.clone(),
                    material,
                    ..Default::default()
                };
                if !tangents.is_empty() {
                    draw.vertex_streams.insert(
                        "a_TangentSigned".into(),
                        VertexStream {
                            components: 4,
                            data: VertexStreamData::Float32(tangents.clone()),
                        },
                    );
                }
                frame.draws.push(draw);
            }
        }
        Ok(())
    }
    #[allow(clippy::too_many_arguments)]
    fn text(
        &mut self,
        object: &Value,
        key: &str,
        label: &str,
        world: Mat4,
        projection: Mat4,
        view: Mat4,
        _eye: Vec3,
        _input: &PlayerInput,
        _canvas: u64,
        frame: &mut RenderFrame,
    ) -> Result<()> {
        use wpe_render::text::{TextAlign, TextLayouter, TextStyle};
        if self.native_text {
            let image = self
                .text_rasters
                .get(key)
                .filter(|(state, _)| *state == text_key(object))
                .map(|(_, image)| image.clone())
                .unwrap_or_else(|| {
                    Arc::new(wpe_render::RgbaImage {
                        width: 1,
                        height: 1,
                        rgba: vec![0; 4],
                    })
                });
            self.submit_text_raster(object, key, label, world, projection * view, image, frame);
            return Ok(());
        }

        let text = object
            .get("text")
            .map(literal)
            .and_then(Value::as_str)
            .unwrap_or("");
        if text.is_empty() {
            return Ok(());
        }
        let font = object
            .get("font")
            .and_then(Value::as_str)
            .unwrap_or("fonts/arial.ttf");
        if !self.assets.fonts.contains_key(font) {
            let bytes = self.project.read_resource(font)?.bytes;
            self.assets
                .fonts
                .insert(font.into(), TextLayouter::new(bytes)?);
        }
        let rgb = tint(object);
        let style = TextStyle {
            font_size: number(object.get("pointsize"), 32.),
            line_height: object.get("lineheight").map(|v| number(Some(v), 1.)),
            letter_spacing: number(object.get("spacing"), 0.),
            wrap_width: boolean(object.get("limitwidth"), false)
                .then(|| number(object.get("maxwidth"), 500.)),
            align: match object.get("horizontalalign").and_then(Value::as_str) {
                Some("left") => TextAlign::Left,
                Some("right") => TextAlign::Right,
                Some("center") => TextAlign::Center,
                _ => TextAlign::Start,
            },
            color: rgb,
            padding: number(object.get("padding"), 0.).max(0.) as u32,
            max_lines: boolean(object.get("limitrows"), false)
                .then(|| number(object.get("maxrows"), 1.).max(1.) as u32),
            ellipsis: boolean(object.get("limituseellipsis"), false),
            outline_width: number(object.get("outline"), 0.),
            outline_color: {
                let c = vec3(object.get("outlinecolor"), Vec3::ZERO);
                [c.0[0], c.0[1], c.0[2], 1.]
            },
            shadow_offset: {
                let c = vec3(object.get("shadowoffset"), Vec3::ZERO);
                [c.0[0], c.0[1]]
            },
            shadow_blur: number(object.get("shadowblur"), 0.),
            ..Default::default()
        };
        let rendered = self.assets.fonts[font].layout(text, &style)?;
        let size = [rendered.image.width as f32, rendered.image.height as f32];
        frame.issues.extend(rendered.issues);
        let id = resource_id(&format!("text:{key}"));
        let mut image = rendered.image;
        if boolean(object.get("opaquebackground"), false) {
            let c = vec3(object.get("backgroundcolor"), Vec3::ZERO)
                * number(object.get("backgroundbrightness"), 1.);
            for p in image.rgba.chunks_exact_mut(4) {
                let a = p[3] as f32 / 255.;
                for (i, channel) in p.iter_mut().enumerate().take(3) {
                    *channel =
                        (*channel as f32 * a + c.0[i] * 255. * (1. - a)).clamp(0., 255.) as u8;
                }
                p[3] = 255;
            }
        }
        frame
            .textures
            .insert(id, Texture::rgba2d(label, Arc::new(image)));
        let anchor = anchor(
            object
                .get("anchor")
                .or_else(|| object.get("alignment"))
                .and_then(Value::as_str)
                .unwrap_or("center"),
        );
        let material = Material {
            textures: vec![TextureBinding {
                name: "g_Texture0".into(),
                slot: 0,
                texture: id,
            }],
            ..Default::default()
        };
        let vertices = quad_vertices(
            size,
            anchor,
            [1.; 4],
            |uv| uv,
            world,
            projection * view,
            false,
        );
        frame.draws.push(DrawCall {
            name: label.into(),
            vertices,
            indices: vec![0, 2, 1, 1, 2, 3],
            material,
            ..Default::default()
        });
        self.hits.push(HitRegion {
            id: object.get("id").cloned().unwrap_or(Value::Null),
            inverse: world.inverse().unwrap_or_default(),
            size,
            anchor,
            screen_space: false,
            triangles: None,
        });
        Ok(())
    }
    #[allow(clippy::too_many_arguments)]
    fn submit_text_raster(
        &mut self,
        object: &Value,
        key: &str,
        label: &str,
        world: Mat4,
        camera: Mat4,
        image: Arc<wpe_render::RgbaImage>,
        frame: &mut RenderFrame,
    ) {
        let size = [image.width as f32, image.height as f32];
        let id = resource_id(&format!("text:{key}"));
        frame.textures.insert(id, Texture::rgba2d(label, image));
        let anchor = anchor(
            object
                .get("anchor")
                .or_else(|| object.get("alignment"))
                .and_then(Value::as_str)
                .unwrap_or("center"),
        );
        let material = Material {
            textures: vec![TextureBinding {
                name: "g_Texture0".into(),
                slot: 0,
                texture: id,
            }],
            ..Default::default()
        };
        frame.draws.push(DrawCall {
            name: label.into(),
            vertices: quad_vertices(size, anchor, [1.; 4], |uv| uv, world, camera, false),
            indices: vec![0, 2, 1, 1, 2, 3],
            material,
            ..Default::default()
        });
        self.hits.push(HitRegion {
            id: object.get("id").cloned().unwrap_or(Value::Null),
            inverse: world.inverse().unwrap_or_default(),
            size,
            anchor,
            screen_space: false,
            triangles: None,
        });
    }
    /// Patch current-frame native glyph rasters after script update, before GPU
    /// submit, without running any script/clock/simulation a second time.
    pub fn apply_text_rasters(&mut self, frame: &mut RenderFrame) -> Result<usize> {
        let objects = self
            .scene
            .get("objects")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let transforms = hierarchy(&objects)?;
        let camera = self.camera(frame.width, frame.height, &self.render_input);
        let mut applied = 0;
        for (at, object) in objects.iter().enumerate() {
            let key = object_key(object, at);
            let Some((state, image)) = self.text_rasters.get(&key) else {
                continue;
            };
            if *state != text_key(object) {
                continue;
            }
            let size = [image.width as f32, image.height as f32];
            let id = resource_id(&format!("text:{key}"));
            frame
                .textures
                .insert(id, Texture::rgba2d("native-text", image.clone()));
            let anchor = anchor(
                object
                    .get("anchor")
                    .or_else(|| object.get("alignment"))
                    .and_then(Value::as_str)
                    .unwrap_or("center"),
            );
            for draw in &mut frame.draws {
                if draw
                    .material
                    .textures
                    .iter()
                    .any(|binding| binding.texture == id)
                {
                    draw.vertices = quad_vertices(
                        size,
                        anchor,
                        [1.; 4],
                        |uv| uv,
                        transforms[at],
                        camera,
                        false,
                    );
                    applied += 1;
                }
            }
            for hit in &mut self.hits {
                if hit.id == object["id"] {
                    hit.size = size;
                }
            }
        }
        frame.validate()?;
        Ok(applied)
    }
    fn update_pose_metadata(&mut self, objects: &[Value]) -> Result<()> {
        let Some(array) = self.scene.get_mut("objects").and_then(Value::as_array_mut) else {
            return Ok(());
        };
        for (at, original) in objects.iter().enumerate() {
            let key = object_key(original, at);
            if let Some(model) = self.models.get(&key)
                && let Some(object) = array.get_mut(at)
            {
                if let Some(bones) = object.get_mut("bones").and_then(Value::as_array_mut) {
                    for (i, bone) in bones.iter_mut().enumerate() {
                        if let Some(transform) = model.pose.local.get(i) {
                            bone["transform"] = json!(transform.matrix().0);
                        }
                    }
                }
                if let Some(attachments) =
                    object.get_mut("attachments").and_then(Value::as_array_mut)
                {
                    for a in attachments {
                        if let Some(name) = a.get("name").and_then(Value::as_str)
                            && let Some(transform) = model.attachment(name)
                        {
                            a["transform"] = json!(transform.0);
                        }
                    }
                }
            }
        }
        if let Some(vm) = &mut self.vm {
            vm.replace_scene(self.scene.clone()).map_err(script_error)?;
        }
        Ok(())
    }
}
fn apply_root_motion(
    objects: &mut [Value],
    transforms: &[Mat4],
    motions: &[(usize, wpe_sim::model::RootMotion)],
) -> Result<bool> {
    let ids = objects
        .iter()
        .enumerate()
        .map(|(i, o)| (object_key(o, i), i))
        .collect::<BTreeMap<_, _>>();
    let mut changed = false;
    for &(at, motion) in motions {
        if motion.translation.length_squared() == 0. && motion.yaw == 0. {
            continue;
        }
        let object = &mut objects[at];
        let parent = object
            .get("parent")
            .and_then(|id| ids.get(&id.to_string()))
            .map(|i| transforms[*i])
            .unwrap_or_default();
        let delta = parent
            .inverse()
            .unwrap_or_default()
            .vector(transforms[at].vector(motion.translation));
        let origin = vec3(object.get("origin"), Vec3::ZERO) + delta;
        let mut angles = vec3(object.get("angles"), Vec3::ZERO);
        angles.0[1] += motion.yaw;
        if !origin.finite() || !angles.finite() {
            return Err(Error::invalid("root motion", "nonfinite layer transform"));
        }
        for (name, value) in [("origin", origin), ("angles", angles)] {
            let text = json!(format!("{} {} {}", value.0[0], value.0[1], value.0[2]));
            if object
                .get(name)
                .is_some_and(|v| v.is_object() && v.get("value").is_some())
            {
                object[name]["value"] = text;
            } else {
                object[name] = text;
            }
        }
        changed = true;
    }
    Ok(changed)
}
pub(super) fn vertex(
    position: Vec3,
    normal: Vec3,
    uv: [f32; 2],
    color: [f32; 4],
    world: Mat4,
    camera: Mat4,
    custom: bool,
) -> GpuVertex {
    let n = if custom {
        normal
    } else {
        world.vector(normal).normalize()
    };
    GpuVertex {
        position: if custom {
            [position.0[0], position.0[1], position.0[2], 1.]
        } else {
            (camera * world).transform4([position.0[0], position.0[1], position.0[2], 1.])
        },
        normal: [n.0[0], n.0[1], n.0[2], 0.],
        uv: [uv[0], uv[1], 0., 0.],
        color,
    }
}
pub(super) fn quad_vertices(
    size: [f32; 2],
    anchor: [f32; 2],
    color: [f32; 4],
    uv: impl Fn([f32; 2]) -> [f32; 2],
    world: Mat4,
    camera: Mat4,
    custom: bool,
) -> Vec<GpuVertex> {
    let [x, y] = [-anchor[0] * size[0], -anchor[1] * size[1]];
    [
        (Vec3::new(x, y + size[1], 0.), [0., 0.]),
        (Vec3::new(x + size[0], y + size[1], 0.), [1., 0.]),
        (Vec3::new(x, y, 0.), [0., 1.]),
        (Vec3::new(x + size[0], y, 0.), [1., 1.]),
    ]
    .into_iter()
    .map(|(p, t)| {
        vertex(
            p,
            Vec3::new(0., 0., 1.),
            uv(t),
            color,
            world,
            camera,
            custom,
        )
    })
    .collect()
}
fn anchor(name: &str) -> [f32; 2] {
    [
        if name.contains("left") {
            0.
        } else if name.contains("right") {
            1.
        } else {
            0.5
        },
        if name.contains("top") {
            1.
        } else if name.contains("bottom") {
            0.
        } else {
            0.5
        },
    ]
}
fn cpu_deformed_material(overrides: Option<&Value>) -> Value {
    let mut value = overrides
        .filter(|v| v.is_object())
        .cloned()
        .unwrap_or_else(|| json!({}));
    if !value.get("combos").is_some_and(Value::is_object) {
        value["combos"] = json!({});
    }
    for name in ["SKINNING", "MORPHING", "MORPHING_NORMALS"] {
        value["combos"][name] = json!(0);
    }
    value
}
fn tint(object: &Value) -> [f32; 4] {
    let color = vec3(object.get("color"), Vec3::ONE) * number(object.get("brightness"), 1.);
    [
        color.0[0],
        color.0[1],
        color.0[2],
        number(object.get("alpha"), 1.),
    ]
}

fn centered_ortho(size: [f32; 2]) -> Mat4 {
    let mut projection = Mat4::orthographic(size[0], size[1], -10000., 10000.);
    projection.0[12] = 0.;
    projection.0[13] = 0.;
    projection
}
pub(super) fn target(
    frame: &mut RenderFrame,
    name: &str,
    width: u32,
    height: u32,
    format: TextureFormat,
) -> u64 {
    let id = resource_id(name);
    frame.textures.entry(id).or_insert_with(|| {
        Texture::target_with_format(name, width.clamp(1, 16384), height.clamp(1, 16384), format)
    });
    id
}
fn copy_pass(frame: &mut RenderFrame, name: &str, input: u64, target: Target) {
    frame.passes.push(RenderPass {
        name: name.into(),
        target,
        effect: Some(wpe_render::effect::Effect::Copy { input }),
        ..Default::default()
    });
}
fn texture_format(name: Option<&str>, fallback: TextureFormat) -> TextureFormat {
    match name {
        Some("rgba16f" | "RGBA16F") => TextureFormat::Rgba16Float,
        Some("rgba32f" | "RGBA32F") => TextureFormat::Rgba32Float,
        Some("rg16f" | "rg1616f") => TextureFormat::Rg16Float,
        Some("r16f") => TextureFormat::R16Float,
        Some("rgba8888" | "rgba8") => TextureFormat::Rgba8Unorm,
        Some("r32f") => TextureFormat::R32Float,
        Some("r8") => TextureFormat::R8Unorm,
        _ => fallback,
    }
}
pub(super) fn merge_pass(target: &mut Value, source: &Value) {
    if let (Some(a), Some(b)) = (target.as_object_mut(), source.as_object()) {
        for (k, v) in b {
            if k == "textures"
                && let (Some(old), Some(new)) =
                    (a.get_mut(k).and_then(Value::as_array_mut), v.as_array())
            {
                for (i, v) in new.iter().enumerate() {
                    if v.is_null() {
                        continue;
                    }
                    while old.len() <= i {
                        old.push(Value::Null)
                    }
                    old[i] = v.clone();
                }
                continue;
            }
            if let Some(old) = a.get_mut(k)
                && old.is_object()
                && v.is_object()
            {
                merge_pass(old, v)
            } else {
                a.insert(k.clone(), v.clone());
            }
        }
    }
}
fn dependency_order(objects: &[Value]) -> Result<Vec<usize>> {
    fn visit(
        i: usize,
        objects: &[Value],
        ids: &BTreeMap<String, usize>,
        seen: &mut [u8],
        out: &mut Vec<usize>,
    ) -> Result<()> {
        if seen[i] == 2 {
            return Ok(());
        }
        if seen[i] == 1 {
            return Err(Error::invalid("scene dependencies", "cycle"));
        }
        seen[i] = 1;
        for dep in objects[i]
            .get("dependencies")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(index) = ids.get(&dep.get("id").unwrap_or(dep).to_string())
                && *index != i
            {
                visit(*index, objects, ids, seen, out)?;
            }
        }
        seen[i] = 2;
        out.push(i);
        Ok(())
    }
    let ids = objects
        .iter()
        .enumerate()
        .map(|(i, o)| (object_key(o, i), i))
        .collect();
    let mut seen = vec![0; objects.len()];
    let mut out = Vec::new();
    for i in 0..objects.len() {
        visit(i, objects, &ids, &mut seen, &mut out)?;
    }
    Ok(out)
}

pub(super) fn tangent_streams(
    vertices: &[wpe_sim::model::ModelVertex],
) -> BTreeMap<String, VertexStream> {
    BTreeMap::from([
        (
            "a_Tangent4".into(),
            VertexStream {
                components: 4,
                data: VertexStreamData::Float32(vertices.iter().flat_map(|v| v.tangent).collect()),
            },
        ),
        (
            "a_TangentSigned".into(),
            VertexStream {
                components: 4,
                data: VertexStreamData::Float32(vertices.iter().flat_map(|v| v.tangent).collect()),
            },
        ),
        (
            "a_Tangent".into(),
            VertexStream {
                components: 3,
                data: VertexStreamData::Float32(
                    vertices
                        .iter()
                        .flat_map(|v| v.tangent[..3].iter().copied())
                        .collect(),
                ),
            },
        ),
    ])
}

pub(super) fn color4(value: &Value) -> [f32; 4] {
    let values = if let Some(string) = literal(value).as_str() {
        string
            .split_whitespace()
            .filter_map(|n| n.parse::<f32>().ok())
            .collect::<Vec<_>>()
    } else {
        literal(value)
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|n| n.as_f64().map(|n| n as f32))
                    .collect()
            })
            .unwrap_or_default()
    };
    std::array::from_fn(|i| {
        values
            .get(i)
            .copied()
            .filter(|n| n.is_finite())
            .unwrap_or(0.)
    })
}
fn collect_combos(value: Option<&Value>, output: &mut BTreeMap<String, i32>) {
    if let Some(values) = value.and_then(Value::as_object) {
        for (k, v) in values {
            output.insert(k.clone(), number(Some(v), 0.) as i32);
        }
    }
}
fn conditions(value: &Value, combos: &BTreeMap<String, i32>) -> bool {
    let Some(condition) = value.get("conditions") else {
        return true;
    };
    let evaluate = |condition: &Value| {
        condition.as_object().is_some_and(|map| {
            map.iter().all(|(name, expected)| {
                combos.get(name).copied().unwrap_or(0) == number(Some(expected), 0.) as i32
            })
        })
    };
    if let Some(alternatives) = condition.as_array() {
        alternatives.is_empty() || alternatives.iter().any(evaluate)
    } else {
        evaluate(condition)
    }
}

pub(super) fn text_key(object: &Value) -> String {
    let keys = [
        "text",
        "font",
        "pointsize",
        "lineheight",
        "spacing",
        "limitwidth",
        "maxwidth",
        "limitrows",
        "maxrows",
        "limituseellipsis",
        "horizontalalign",
        "verticalalign",
        "blockalign",
        "padding",
        "outline",
        "outlinecolor",
        "shadowoffset",
        "shadowblur",
        "shadowcolor",
        "opaquebackground",
        "backgroundcolor",
        "backgroundbrightness",
        "alpha",
        "color",
        "brightness",
    ];
    let values = keys
        .iter()
        .map(|key| {
            (
                *key,
                object
                    .get(*key)
                    .map(literal)
                    .cloned()
                    .unwrap_or(Value::Null),
            )
        })
        .collect::<BTreeMap<_, _>>();
    serde_json::to_string(&values).expect("JSON text state")
}

fn object_blend(material: &mut Material, object: &Value) {
    let mode = number(object.get("colorBlendMode"), 0.) as u8;
    if (1..=32).contains(&mode) {
        material.state.blend = BlendState::Custom(mode);
    }
}

fn draw_instances(material: &Material) -> u32 {
    match material.uniforms.values.get("wpe_DrawInstances") {
        Some(UniformValue::Int(value)) => (*value).clamp(2, 128) as u32,
        _ => 1,
    }
}
