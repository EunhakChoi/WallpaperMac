//! Working planar reflections for legacy `_rt_Reflection` materials. The owned
//! profile mirrors geometry above a configurable horizontal plane (Y=0 by
//! default); native plane selection, clipping and filtering parity is unknown.
use super::*;
use std::sync::Arc;
use wpe_render::{CullMode, RenderPass, Target, shader::UniformValue};

impl ScenePlayer {
    pub(super) fn emit_reflection(
        &mut self,
        frame: &mut RenderFrame,
        target: u64,
        source_draws: &[usize],
        projection: Mat4,
        view: Mat4,
        eye: Vec3,
    ) -> Result<()> {
        if !frame.textures.contains_key(&target) {
            return Ok(());
        }
        let plane = number(self.scene.pointer("/general/reflectionheight"), 0.);
        let mut mirror = Mat4::IDENTITY;
        mirror.0[5] = -1.;
        mirror.0[13] = plane * 2.;
        let reflected_view = view * mirror;
        let reflected_camera = projection * reflected_view;
        let inverse_camera = (projection * view).inverse().unwrap_or_default();
        let black = resource_id("reflection-recursion-neutral");
        frame.textures.insert(
            black,
            Texture::rgba2d(
                "reflection-recursion-neutral",
                Arc::new(crate::RgbaImage::new(1, 1, vec![0; 4])?),
            ),
        );
        let mut draws = Vec::new();
        for &source in source_draws {
            let mut draw = frame.draws[source].clone();
            let world = draw
                .material
                .uniforms
                .values
                .get("g_ModelMatrix")
                .and_then(|v| {
                    if let UniformValue::Matrix {
                        columns: 4,
                        rows: 4,
                        values,
                    } = v
                    {
                        values.as_slice().try_into().ok().map(Mat4)
                    } else {
                        None
                    }
                })
                .unwrap_or_default();
            let shader_local = draw.material.vertex.is_some();
            let world_point = |index: u32| {
                let p = draw.vertices[index as usize].position;
                if shader_local {
                    world.point(Vec3::new(p[0], p[1], p[2]))
                } else {
                    let p = inverse_camera.transform4(p);
                    Vec3::new(p[0] / p[3], p[1] / p[3], p[2] / p[3])
                }
            };
            draw.indices = draw
                .indices
                .chunks_exact(3)
                .filter(|tri| tri.iter().any(|&i| world_point(i).0[1] > plane + 0.00001))
                .flatten()
                .copied()
                .collect();
            if draw.indices.is_empty() {
                continue;
            }
            draw.name = format!("{}:planar-reflection", draw.name);
            if !shader_local {
                for vertex in &mut draw.vertices {
                    vertex.position =
                        (reflected_camera * inverse_camera).transform4(vertex.position);
                }
            }
            for (name, matrix) in [
                ("g_ViewMatrix", reflected_view),
                (
                    "g_ViewMatrixInverse",
                    reflected_view.inverse().unwrap_or_default(),
                ),
                ("g_ModelViewMatrix", reflected_view * world),
                ("g_ViewProjectionMatrix", reflected_camera),
                (
                    "g_ViewProjectionMatrixInverse",
                    reflected_camera.inverse().unwrap_or_default(),
                ),
                ("g_ModelViewProjectionMatrix", reflected_camera * world),
                (
                    "g_ModelViewProjectionMatrixInverse",
                    (reflected_camera * world).inverse().unwrap_or_default(),
                ),
            ] {
                draw.material.uniforms.values.insert(
                    name.into(),
                    UniformValue::Matrix {
                        columns: 4,
                        rows: 4,
                        values: matrix.0.to_vec(),
                    },
                );
            }
            draw.material.uniforms.values.insert(
                "g_EyePosition".into(),
                UniformValue::Vector(mirror.point(eye).0.to_vec()),
            );
            draw.material.state.cull = match draw.material.state.cull {
                CullMode::Front => CullMode::Back,
                CullMode::Back => CullMode::Front,
                CullMode::None => CullMode::None,
            };
            for binding in &mut draw.material.textures {
                if binding.texture == target {
                    binding.texture = black;
                }
            }
            draws.push(frame.draws.len());
            frame.draws.push(draw);
        }
        // Reflection reads shadow/cookie resources, so retain those producer
        // passes before it; every scene/base pass must follow the reflection.
        let at = frame
            .passes
            .iter()
            .position(|p| {
                !p.name.starts_with("shadow-")
                    && !p.name.starts_with("cookie-")
                    && p.name != "scene-clear"
            })
            .unwrap_or(frame.passes.len());
        frame.passes.insert(
            at,
            RenderPass {
                name: "scene-planar-reflection".into(),
                target: Target::Texture(target),
                clear: Some([0.; 4]),
                draws,
                ..Default::default()
            },
        );
        self.issue("owned-reflection-profile",None,"Legacy planar reflection uses an owned horizontal Y=0 default plane and half-resolution target. Native plane selection and cross-plane clipping equivalence are unverified.");
        Ok(())
    }
}
