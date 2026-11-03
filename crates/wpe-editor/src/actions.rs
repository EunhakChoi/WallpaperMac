use crate::geometry::{
    auto_depth, insert_vertices, island, normalize_weights, open_edges, reduce_vertices,
    remember_reference, remove_inner_vertices, remove_vertices, smooth_weights, vector,
    vertex_stride,
};
use crate::*;
impl Editor {
    pub(crate) fn action(&mut self, model: bool, args: &[Value]) -> Result<Value> {
        let action = string(args, 0)?;
        let known = if model {
            surface::MODEL_ACTIONS
        } else {
            surface::SCENE_ACTIONS
        };
        if !known.contains(&action) {
            return Err(Error::new(
                "unknown-action",
                format!(
                    "unknown {} action {action}",
                    if model { "model" } else { "scene" }
                ),
            ));
        }
        if model && args.get(1).and_then(|id| self.resource_path(id)).is_some() {
            return self.resource_model_action(args);
        }
        let id = args
            .get(1)
            .filter(|v| self.index(v).is_ok())
            .cloned()
            .or_else(|| {
                self.session["selection"]
                    .as_array()
                    .and_then(|v| v.first())
                    .cloned()
            });
        let data = args
            .get(2)
            .map(|v| {
                if v.as_str()
                    .is_some_and(|s| s.starts_with('{') || s.starts_with('['))
                {
                    parse_argument(Some(v))
                } else {
                    Ok(v.clone())
                }
            })
            .transpose()?
            .unwrap_or(Value::Null);
        if action == "cleanLocalStorage" {
            return Ok(self.request_host("cleanLocalStorage", &args[1..]));
        }
        if matches!(
            action,
            "prepareCharacterSeparation"
                | "finishCharacterSeparation"
                | "prepareDepthMapGeneration"
                | "prepareNormapMapGeneration"
        ) {
            return self.image_action(action, &args[1..]);
        }
        if matches!(
            action,
            "runExternalImageEditor"
                | "importPuppetReferenceOverlay"
                | "importPuppetWarpDepth"
                | "exportPuppetWarpDepth"
                | "addFBXfile"
                | "paintPuppetClippingMask"
        ) {
            return Ok(self.request_host(action, &args[1..]));
        }
        let id = id.ok_or_else(|| Error::new("selection", "action requires a scene layer"))?;
        if action == "upgradePuppetModel" {
            return self.upgrade_puppet_model(&id);
        }
        if matches!(action, "bringBoneToBack" | "bringBoneToFront") {
            return self.reorder_puppet_bone(&id, &data, action == "bringBoneToFront");
        }
        if action == "checkImageSuitableForPuppetWarp" {
            let layer = &self.objects()?[self.index(&id)?];
            return Ok(
                json!({"suitable":layer.get("image").is_some(),"reason":if layer.get("image").is_some(){Value::Null}else{json!("image layer required")}}),
            );
        }
        if matches!(
            action,
            "cameraEnterPOV"
                | "cameraEnterPOVPath"
                | "cameraExitPOV"
                | "cameraExitPOVPath"
                | "playClipAnimation"
                | "playStaticAnimation"
                | "stopAnimation"
                | "setEditMode"
        ) {
            self.session["tools"]["action"] = json!({"action":action,"id":id,"value":data});
            return Ok(self.request_host(action, &args[1..]));
        }
        if matches!(
            action,
            "beginUpdateBoneMask"
                | "endUpdateBoneMask"
                | "startMaskEyeDropper"
                | "endMaskEyeDropper"
                | "setPuppetGeometryVertexGizmo"
                | "selectAttachment"
                | "selectConstraintBone"
                | "selectAnimationBoneList"
                | "selectExpressionBlendShapeMatrix"
                | "selectClippingMask"
        ) {
            self.session["tools"][action] = json!({"id":id,"value":data});
            return Ok(json!(true));
        }
        if matches!(
            action,
            "prepareTextureChannelEditing"
                | "cancelTextureChannelEditing"
                | "finishTextureChannelEditing"
        ) {
            self.session["tools"]["textureChannel"] = if action == "cancelTextureChannelEditing"
                || action == "finishTextureChannelEditing"
            {
                Value::Null
            } else {
                json!({"id":id,"value":data})
            };
            return Ok(json!(true));
        }
        if action == "renameEditFile" {
            let source = data
                .as_str()
                .ok_or_else(|| Error::new("argument", "source path required"))?;
            let destination = string(args, 3)?;
            let bytes = self.read_bytes(source)?;
            self.put_bytes(source, None)?;
            self.put_bytes(destination, Some(bytes))?;
            return Ok(json!(destination));
        }
        let layer = self.layer_mut(&id)?;
        if let Some((key, operation)) = collection_operation(action) {
            let array = layer
                .as_object_mut()
                .unwrap()
                .entry(key)
                .or_insert(json!([]))
                .as_array_mut()
                .ok_or_else(|| Error::new("collection", "array required"))?;
            return edit_collection(array, operation, &data, args.get(3));
        }
        match action {
            "removePuppetWarp" => {
                layer.as_object_mut().unwrap().remove("puppet");
                layer.as_object_mut().unwrap().remove("modeldata");
                Ok(json!(true))
            }
            "lockPuppetWarpGeometry"
            | "unlockPuppetWarpGeometry"
            | "checkLockPuppetWarpGeometry" => {
                if action != "checkLockPuppetWarpGeometry" {
                    layer["geometrylocked"] = json!(action == "lockPuppetWarpGeometry")
                }
                Ok(layer["geometrylocked"].clone())
            }
            "setPuppetWarpRootBone" => {
                layer["rootbone"] = data;
                Ok(json!(true))
            }
            "linkBoneToParent" | "unlinkBoneFromParent" | "cancelLinkBoneToParent" => {
                if action == "cancelLinkBoneToParent" {
                    return Ok(json!(false));
                }
                let index = data
                    .as_u64()
                    .ok_or_else(|| Error::new("bone", "bone index required"))?
                    as usize;
                let parent = if action == "unlinkBoneFromParent" {
                    json!(-1)
                } else {
                    arg(args, 3)?.clone()
                };
                let bones = layer["bones"]
                    .as_array_mut()
                    .ok_or_else(|| Error::new("bone", "bones metadata required"))?;
                if index >= bones.len() {
                    return Err(Error::new("bone", "index out of bounds"));
                }
                let mut current = parent.as_i64().unwrap_or(-1);
                let mut seen = BTreeSet::from([index]);
                while current >= 0 {
                    if !seen.insert(current as usize) {
                        return Err(Error::new("bone", "parent cycle"));
                    }
                    current = bones
                        .get(current as usize)
                        .ok_or_else(|| Error::new("bone", "parent out of bounds"))?["parent"]
                        .as_i64()
                        .unwrap_or(-1);
                }
                bones[index]["parent"] = parent;
                Ok(json!(true))
            }
            "removePuppetWarpBone" => {
                let i = data
                    .as_u64()
                    .ok_or_else(|| Error::new("bone", "index required"))?
                    as usize;
                let bones = layer["bones"]
                    .as_array_mut()
                    .ok_or_else(|| Error::new("bone", "bones required"))?;
                if i >= bones.len() {
                    return Err(Error::new("bone", "index out of bounds"));
                }
                bones.remove(i);
                for bone in bones {
                    if let Some(parent) = bone["parent"].as_i64() {
                        bone["parent"] = json!(if parent == i as i64 {
                            -1
                        } else if parent > i as i64 {
                            parent - 1
                        } else {
                            parent
                        });
                    }
                }
                Ok(json!(true))
            }
            "boneMaskSelectAll" | "boneMaskDeselectAll" => {
                let count = layer["bones"].as_array().map_or(0, Vec::len);
                layer["bonemask"] = json!(vec![
                    if action == "boneMaskSelectAll" {
                        1.0
                    } else {
                        0.0
                    };
                    count
                ]);
                Ok(layer["bonemask"].clone())
            }
            "applyBoneConstraints" => {
                if !data.is_array() && !data.is_object() {
                    return Err(Error::new("constraints", "configuration required"));
                }
                layer["constraints"] = data;
                Ok(json!(true))
            }
            "puppetGetBoneConstraintsConfig" => Ok(layer["constraints"].clone()),
            "getPuppetBlendShapeSettings" => Ok(layer["blendshapesettings"].clone()),
            "setPuppetBlendShapeSettings" => {
                layer["blendshapesettings"] = data;
                Ok(json!(true))
            }
            "getPuppetExpressionMatrix" => Ok(layer["expressionmatrix"].clone()),
            "setPuppetExpressionMatrix" => {
                layer["expressionmatrix"] = data;
                Ok(json!(true))
            }
            "saveSkeletalPose" => {
                let pose = if let Some(value) = args.get(3) {
                    parse_argument(Some(value))?
                } else {
                    json!({"bones":layer["bones"]})
                };
                let poses = layer
                    .as_object_mut()
                    .unwrap()
                    .entry("poses")
                    .or_insert(json!([]))
                    .as_array_mut()
                    .ok_or_else(|| Error::new("pose", "poses array required"))?;
                let index = data.as_u64().unwrap_or(poses.len() as u64) as usize;
                if index > poses.len() {
                    return Err(Error::new("pose", "index out of bounds"));
                }
                if index == poses.len() {
                    poses.push(pose.clone())
                } else {
                    poses[index] = pose.clone()
                }
                Ok(json!({"item":pose,"index":index}))
            }
            "applySkeletalPose" => {
                let index = data
                    .as_u64()
                    .ok_or_else(|| Error::new("pose", "index required"))?
                    as usize;
                let pose = layer["poses"]
                    .as_array()
                    .and_then(|a| a.get(index))
                    .cloned()
                    .ok_or_else(|| Error::new("pose", "index out of bounds"))?;
                if let Some(bones) = pose.get("bones") {
                    layer["bones"] = bones.clone();
                } else {
                    return Err(Error::new("pose", "pose.bones required"));
                }
                Ok(json!(true))
            }
            "renameSkeletalPose" => {
                let i = data
                    .as_u64()
                    .ok_or_else(|| Error::new("pose", "index required"))?
                    as usize;
                let value = arg(args, 3)?.clone();
                let pose = layer["poses"]
                    .as_array_mut()
                    .and_then(|a| a.get_mut(i))
                    .ok_or_else(|| Error::new("pose", "index out of bounds"))?;
                pose["name"] = value;
                Ok(json!(true))
            }
            "orderSkeletalPoses" => {
                let order = data
                    .as_array()
                    .ok_or_else(|| Error::new("pose", "index order array required"))?;
                let old = layer["poses"]
                    .as_array()
                    .ok_or_else(|| Error::new("pose", "poses required"))?;
                if order.len() != old.len() {
                    return Err(Error::new("pose", "complete permutation required"));
                }
                let mut seen = BTreeSet::new();
                let reordered = order
                    .iter()
                    .map(|v| {
                        let i = v
                            .as_u64()
                            .ok_or_else(|| Error::new("pose", "index required"))?
                            as usize;
                        if !seen.insert(i) {
                            return Err(Error::new("pose", "duplicate index"));
                        }
                        old.get(i)
                            .cloned()
                            .ok_or_else(|| Error::new("pose", "index out of bounds"))
                    })
                    .collect::<Result<Vec<_>>>()?;
                layer["poses"] = json!(reordered);
                Ok(json!(true))
            }
            "saveAnimation"
            | "saveAnimationOptions"
            | "cameraPathSave"
            | "cameraPathSaveOptions" => {
                let key = if action.starts_with("camera") {
                    "camerapaths"
                } else {
                    "animations"
                };
                let i = data.as_u64().unwrap_or(0) as usize;
                let array = layer[key]
                    .as_array_mut()
                    .ok_or_else(|| Error::new("animation", "collection required"))?;
                let target = array
                    .get_mut(i)
                    .ok_or_else(|| Error::new("animation", "index out of bounds"))?;
                let value = parse_argument(args.get(3))?;
                if action.ends_with("Options") {
                    target["options"] = value
                } else {
                    *target = value
                }
                Ok(json!(true))
            }
            "resetPuppetWarpBindingPose"
            | "resetPuppetWarpReferencePose"
            | "resetPuppetWarpPose"
            | "resetSelectedPuppetWarpPose" => {
                let base = layer
                    .get("bindingpose")
                    .or_else(|| layer.get("referencepose"))
                    .cloned()
                    .ok_or_else(|| {
                        Error::new("pose", "binding/reference pose metadata required")
                    })?;
                layer["bones"] = base;
                Ok(json!(true))
            }
            "getTextureChannels" => Ok(layer["texturechannels"].clone()),
            "getTextureChannelCropInfo" => {
                let i = data.as_u64().unwrap_or(0) as usize;
                Ok(layer["texturechannels"][i]["crops"].clone())
            }
            "getBaseTexturePath" => Ok(layer
                .get("image")
                .or_else(|| layer.get("material"))
                .cloned()
                .unwrap_or(Value::Null)),
            "confirmTextureChannelAvailability" => Ok(json!(
                !layer["texturechannels"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|c| c["name"] == data))
            )),
            "textureAddVariant"
            | "textureRemoveVariant"
            | "textureAddVariantOption"
            | "textureRemoveVariantOption"
            | "textureRenameVariantOption"
            | "textureReplaceVariantOption"
            | "textureSetVariantOptionBlend"
            | "textureSetVariantOptionCondition"
            | "textureSortVariantOption" => texture_variant(layer, action, &data, args.get(3)),
            "puppetGetAllClippingMasks" => Ok(layer["clippingmasks"].clone()),
            "puppetAssignClippingMask" => {
                layer["clippingmask"] = data;
                Ok(json!(true))
            }
            "selectPuppetWarpTopologyVertices"
            | "selectPuppetWarpTopologyEdges"
            | "selectPuppetWarpTopologyFaces" => {
                let key = match action {
                    "selectPuppetWarpTopologyEdges" => "selectededges",
                    "selectPuppetWarpTopologyFaces" => "selectedfaces",
                    _ => "selectedvertices",
                };
                if !data.is_array() {
                    return Err(Error::new("geometry", "selection array required"));
                }
                layer[key] = data;
                Ok(json!(true))
            }
            "selectVertexIsland" => {
                let shape = shape_mut(layer)?;
                let selected = island(shape, data.as_u64().unwrap_or(0) as usize)?;
                layer["selectedvertices"] = json!(selected);
                Ok(json!(selected))
            }
            "normalizePuppetWarpWeights"
            | "smoothPuppetWarpWeights"
            | "resetPuppetWarpWeights"
            | "paintPuppetWarpWeights" => {
                let shape = shape_mut(layer)?;
                match action {
                    "normalizePuppetWarpWeights" => {
                        for row in shape["weights"]
                            .as_array_mut()
                            .ok_or_else(|| Error::new("weights", "weights required"))?
                        {
                            normalize_weights(row)?
                        }
                    }
                    "smoothPuppetWarpWeights" => smooth_weights(shape)?,
                    "resetPuppetWarpWeights" => {
                        let count = shape["vertexBuffer"].as_array().map_or(0, Vec::len)
                            / vertex_stride(shape)?;
                        shape["weights"] = json!(vec![vec![1.0, 0.0, 0.0, 0.0]; count]);
                    }
                    _ => {
                        let rows = data
                            .as_array()
                            .ok_or_else(|| Error::new("weights", "weight rows required"))?;
                        let mut rows = rows.clone();
                        for row in &mut rows {
                            normalize_weights(row)?
                        }
                        shape["weights"] = json!(rows);
                    }
                }
                Ok(json!(true))
            }
            "removePuppetWarpTopologySelectedVertices" => {
                let selected = layer["selectedvertices"]
                    .as_array()
                    .ok_or_else(|| Error::new("geometry", "selectedvertices required"))?
                    .iter()
                    .map(|v| {
                        v.as_u64()
                            .map(|i| i as usize)
                            .ok_or_else(|| Error::new("geometry", "vertex index required"))
                    })
                    .collect::<Result<BTreeSet<_>>>()?;
                let shape = shape_mut(layer)?;
                remember_reference(shape);
                remove_vertices(shape, &selected)?;
                layer["selectedvertices"] = json!([]);
                Ok(json!(true))
            }
            "reducePuppetWarpTopologyVertices" => {
                let selected = layer["selectedvertices"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_u64)
                    .map(|i| i as usize)
                    .collect::<BTreeSet<_>>();
                let shape = shape_mut(layer)?;
                let count =
                    shape["vertexBuffer"].as_array().map_or(0, Vec::len) / vertex_stride(shape)?;
                let target = data
                    .get("targetCount")
                    .and_then(Value::as_u64)
                    .or_else(|| data.as_u64())
                    .map(|n| n as usize)
                    .unwrap_or((count / 2).max(3));
                reduce_vertices(shape, target, &selected)?;
                layer["selectedvertices"] = json!([]);
                Ok(json!(true))
            }
            "removePuppetWarpTopologyInnerVertices" => {
                remove_inner_vertices(shape_mut(layer)?)?;
                layer["selectedvertices"] = json!([]);
                Ok(json!(true))
            }
            "puppetWarpSelectedVerticesDepthAuto" => {
                let selected = layer["selectedvertices"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_u64)
                    .map(|n| n as usize)
                    .collect::<BTreeSet<_>>();
                auto_depth(shape_mut(layer)?, &selected)?;
                Ok(json!(true))
            }
            "addPuppetWarpTopologyVertices" => {
                let additions = vector(&data)?;
                insert_vertices(shape_mut(layer)?, &additions)?;
                Ok(json!(true))
            }
            "resetPuppetWarpDepth"
            | "resetPuppetWarpSelectedVerticesDepth"
            | "paintPuppetWarpDepth"
            | "resetPuppetWarpSelectedVertices" => {
                let selection = layer["selectedvertices"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                let selected = selection
                    .iter()
                    .filter_map(Value::as_u64)
                    .map(|i| i as usize)
                    .collect::<BTreeSet<_>>();
                let shape = shape_mut(layer)?;
                let stride = vertex_stride(shape)?;
                if action == "resetPuppetWarpSelectedVertices" {
                    let reference = shape
                        .get("referenceVertexBuffer")
                        .and_then(Value::as_array)
                        .cloned()
                        .ok_or_else(|| {
                            Error::new("geometry", "reference vertex positions required")
                        })?;
                    let vertices = shape["vertexBuffer"].as_array_mut().unwrap();
                    if reference.len() != vertices.len() {
                        return Err(Error::new("geometry", "reference count mismatch"));
                    }
                    for i in &selected {
                        if *i >= vertices.len() / stride {
                            return Err(Error::new("geometry", "selection out of bounds"));
                        }
                        vertices[*i * stride..(*i + 1) * stride]
                            .clone_from_slice(&reference[*i * stride..(*i + 1) * stride]);
                    }
                    return Ok(json!(true));
                }
                remember_reference(shape);
                let vertices = shape["vertexBuffer"]
                    .as_array_mut()
                    .ok_or_else(|| Error::new("geometry", "vertices required"))?;
                let depth = if action == "paintPuppetWarpDepth" {
                    data.as_f64()
                        .ok_or_else(|| Error::new("geometry", "explicit finite depth required"))?
                } else {
                    0.0
                };
                if !depth.is_finite() {
                    return Err(Error::new("geometry", "finite depth required"));
                }
                for (i, vertex) in vertices.chunks_mut(stride).enumerate() {
                    if action == "resetPuppetWarpDepth" || selected.contains(&i) {
                        vertex[2] = json!(depth);
                    }
                }
                Ok(json!(true))
            }
            "resetPuppetWarpPivot" => {
                layer["origin"] = json!("0 0 0");
                Ok(json!(true))
            }
            "markPuppetWarpTopologyEdgesToOpen" => {
                open_edges(shape_mut(layer)?, &data)?;
                Ok(json!(true))
            }
            "bringTrianglesToBack" | "bringTrianglesToFront" => {
                let selected = data
                    .as_u64()
                    .ok_or_else(|| Error::new("order", "index required"))?
                    as usize;
                let shape = shape_mut(layer)?;
                let indices = shape["indexBuffer"]
                    .as_array_mut()
                    .ok_or_else(|| Error::new("geometry", "triangle indices required"))?;
                if !indices.len().is_multiple_of(3) || selected >= indices.len() / 3 {
                    return Err(Error::new("order", "triangle index out of bounds"));
                }
                let triangle = indices
                    .drain(selected * 3..selected * 3 + 3)
                    .collect::<Vec<_>>();
                if action.ends_with("Front") {
                    indices.extend(triangle)
                } else {
                    indices.splice(0..0, triangle);
                }
                Ok(json!(true))
            }
            _ => Err(Error::new(
                "action-capability",
                format!("action {action} has no validated local representation"),
            )),
        }
    }
}
fn collection_operation(action: &str) -> Option<(&'static str, &'static str)> {
    Some(match action {
        "addAnimation" => ("animations", "add"),
        "removeAnimation" => ("animations", "remove"),
        "duplicateAnimation" => ("animations", "duplicate"),
        "duplicateAnimationLayer" => ("animationlayers", "duplicate"),
        "addBlendShape" => ("blendshapes", "add"),
        "removeBlendShape" => ("blendshapes", "remove"),
        "duplicateBlendShape" => ("blendshapes", "duplicate"),
        "addExpression" => ("expressions", "add"),
        "removeExpression" => ("expressions", "remove"),
        "duplicateExpression" => ("expressions", "duplicate"),
        "addCameraPath" => ("camerapaths", "add"),
        "removeCameraPath" => ("camerapaths", "remove"),
        "duplicateCameraPath" => ("camerapaths", "duplicate"),
        "addTextureChannel" => ("texturechannels", "add"),
        "removeTextureChannel" => ("texturechannels", "remove"),
        "replaceTextureChannel" => ("texturechannels", "replace"),
        "moveTextureChannel" => ("texturechannels", "move"),
        "puppetCreateNewClippingMask" => ("clippingmasks", "add"),
        "puppetDeleteClippingMask" | "removePuppetClippingMask" => ("clippingmasks", "remove"),
        "removeSkeletalPose" => ("poses", "remove"),
        "addAttachment" => ("attachments", "add"),
        "removeAttachment" | "removeSelectedAttachment" => ("attachments", "remove"),
        "addClip" => ("clips", "add"),
        "removeClip" => ("clips", "remove"),
        "addEvent" => ("events", "add"),
        "removeEvent" => ("events", "remove"),
        "removeFBXfile" => ("fbxfiles", "remove"),
        _ => return None,
    })
}
fn edit_collection(
    array: &mut Vec<Value>,
    operation: &str,
    data: &Value,
    next: Option<&Value>,
) -> Result<Value> {
    if operation == "add" {
        let mut value = if data.is_object() {
            data.clone()
        } else {
            json!({"name":data.as_str().unwrap_or("New")})
        };
        let id = array
            .iter()
            .filter_map(|v| v["id"].as_u64())
            .max()
            .unwrap_or(0)
            + 1;
        value["id"] = json!(id);
        array.push(value.clone());
        return Ok(json!({"item":value,"index":array.len()-1,"objectproperties":{}}));
    }
    let index = data
        .as_u64()
        .map(|v| v as usize)
        .or_else(|| {
            array
                .iter()
                .position(|v| v["id"] == *data || v["name"] == *data)
        })
        .ok_or_else(|| Error::new("index", "collection item required"))?;
    if index >= array.len() {
        return Err(Error::new("index", "out of bounds"));
    }
    match operation {
        "remove" => Ok(array.remove(index)),
        "duplicate" => {
            let mut value = array[index].clone();
            value["id"] = json!(
                array
                    .iter()
                    .filter_map(|v| v["id"].as_u64())
                    .max()
                    .unwrap_or(0)
                    + 1
            );
            array.insert(index + 1, value.clone());
            Ok(json!({"item":value,"index":index+1}))
        }
        "replace" => {
            array[index] = next
                .cloned()
                .ok_or_else(|| Error::new("argument", "replacement required"))?;
            Ok(array[index].clone())
        }
        "move" => {
            let to = next
                .and_then(Value::as_u64)
                .ok_or_else(|| Error::new("argument", "destination index required"))?
                as usize;
            if to >= array.len() {
                return Err(Error::new("index", "out of bounds"));
            }
            let value = array.remove(index);
            array.insert(to, value);
            Ok(json!(true))
        }
        _ => Err(Error::new("collection", "unknown operation")),
    }
}
fn shape_mut(layer: &mut Value) -> Result<&mut Value> {
    let shape = layer
        .get_mut("modeldata")
        .and_then(|v| v.get_mut("shapes"))
        .and_then(Value::as_array_mut)
        .and_then(|a| a.first_mut())
        .ok_or_else(|| {
            Error::new(
                "geometry",
                "decoded procedural modeldata.shapes required; opaque MDL is not silently edited",
            )
        })?;
    let stride = vertex_stride(shape)?;
    let vertices = shape["vertexBuffer"]
        .as_array()
        .ok_or_else(|| Error::new("geometry", "vertexBuffer required"))?;
    if !vertices.len().is_multiple_of(stride)
        || !vertices.iter().all(|v| {
            v.as_f64()
                .is_some_and(|n| n.is_finite() && (n as f32).is_finite())
        })
    {
        return Err(Error::new(
            "geometry",
            "finite float32 buffer and matching stride required",
        ));
    }
    let count = vertices.len() / stride;
    if let Some(indices) = shape.get("indexBuffer") {
        let indices = indices
            .as_array()
            .ok_or_else(|| Error::new("geometry", "index array required"))?;
        if !indices.len().is_multiple_of(3)
            || !indices
                .iter()
                .all(|i| i.as_u64().is_some_and(|i| i < count as u64))
        {
            return Err(Error::new(
                "geometry",
                "triangle indices within vertex range required",
            ));
        }
    }
    if let Some(weights) = shape.get("weights")
        && weights.as_array().is_none_or(|rows| rows.len() != count)
    {
        return Err(Error::new(
            "weights",
            "weight row count must equal vertex count",
        ));
    }
    Ok(shape)
}

fn texture_variant(
    layer: &mut Value,
    action: &str,
    data: &Value,
    next: Option<&Value>,
) -> Result<Value> {
    let array = layer
        .as_object_mut()
        .unwrap()
        .entry("texturevariants")
        .or_insert(json!([]))
        .as_array_mut()
        .ok_or_else(|| Error::new("texture", "variant array required"))?;
    if action == "textureAddVariant" {
        return edit_collection(array, "add", data, next);
    }
    let i = data
        .as_u64()
        .ok_or_else(|| Error::new("texture", "variant index required"))? as usize;
    if i >= array.len() {
        return Err(Error::new("texture", "variant index out of bounds"));
    }
    if action == "textureRemoveVariant" {
        return Ok(array.remove(i));
    }
    let config = next
        .cloned()
        .ok_or_else(|| Error::new("texture", "option configuration required"))?;
    let option = config["index"].as_u64().unwrap_or(0) as usize;
    let options = array[i]
        .as_object_mut()
        .ok_or_else(|| Error::new("texture", "variant object required"))?
        .entry("options")
        .or_insert(json!([]))
        .as_array_mut()
        .ok_or_else(|| Error::new("texture", "options array required"))?;
    if action == "textureAddVariantOption" {
        options.push(config.get("value").cloned().unwrap_or(config));
        return Ok(json!(options.len() - 1));
    }
    if option >= options.len() {
        return Err(Error::new("texture", "option index out of bounds"));
    }
    match action {
        "textureRemoveVariantOption" => {
            options.remove(option);
        }
        "textureRenameVariantOption" => options[option]["name"] = config["value"].clone(),
        "textureReplaceVariantOption" => options[option] = config["value"].clone(),
        "textureSetVariantOptionBlend" => options[option]["blend"] = config["value"].clone(),
        "textureSetVariantOptionCondition" => {
            options[option]["condition"] = config["value"].clone()
        }
        "textureSortVariantOption" => {
            let to = config["to"]
                .as_u64()
                .ok_or_else(|| Error::new("texture", "to required"))? as usize;
            if to >= options.len() {
                return Err(Error::new("texture", "destination index out of bounds"));
            }
            let value = options.remove(option);
            options.insert(to, value);
        }
        _ => return Err(Error::new("texture", "unknown variant action")),
    }
    Ok(json!(true))
}
