//! Owned MDL-to-editable geometry profile. The source MDL is never rewritten.
//! Unnamed/opaque native data stays in the original resource and metadata.
use crate::resources::safe_basename;
use crate::*;
use wpe_formats::mdl::{Model, SectionData};
use wpe_sim::math::Mat4;

/// Retargets sampled imported local transforms by unique bone name. Existing
/// geometry and rest/skin identity remain intact. The rest-relative matrix
/// retarget is an owned import profile, not a native FBX compiler claim.
pub fn merge_imported_animation(existing: &Value, imported: &Value) -> Result<Value> {
    let target_bones = existing["bones"]
        .as_array()
        .ok_or_else(|| Error::new("fbx-retarget", "target skeleton required"))?;
    let source_bones = imported["bones"]
        .as_array()
        .ok_or_else(|| Error::new("fbx-retarget", "imported skeleton required"))?;
    if target_bones.len() > 4096 || source_bones.len() > 4096 {
        return Err(Error::new(
            "fbx-retarget",
            "skeleton exceeds4096-bone import policy",
        ));
    }
    let mut names = BTreeMap::new();
    for (index, bone) in source_bones.iter().enumerate() {
        let name = bone["name"]
            .as_str()
            .ok_or_else(|| Error::new("fbx-retarget", "imported bone name required"))?;
        if names.insert(name, index).is_some() {
            return Err(Error::new(
                "fbx-retarget",
                format!("duplicate imported bone name: {name}"),
            ));
        }
    }
    let mut used = BTreeSet::new();
    let mapping = target_bones
        .iter()
        .map(|bone| {
            let name = bone["name"].as_str().unwrap_or("");
            if !used.insert(name) {
                return Err(Error::new(
                    "fbx-retarget",
                    format!("duplicate target bone name: {name}"),
                ));
            }
            Ok(names.get(name).copied())
        })
        .collect::<Result<Vec<_>>>()?;
    if !mapping.iter().any(Option::is_some) {
        return Err(Error::new("fbx-retarget", "no matching bone names"));
    }
    let mut document = existing.clone();
    let mut clips = document["clips"].as_array().cloned().unwrap_or_default();
    let retained_clip_count = clips.len();
    let mut next_id = clips
        .iter()
        .filter_map(|c| c["id"].as_u64())
        .max()
        .unwrap_or(0);
    let mut sampled_matrices = 0usize;
    for imported_clip in imported["clips"]
        .as_array()
        .ok_or_else(|| Error::new("fbx-retarget", "imported clips required"))?
    {
        let frames = imported_clip["frames"]
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .filter(|n| *n > 0 && *n <= 1_000_000)
            .ok_or_else(|| Error::new("fbx-retarget", "imported frame count outside limit"))?;
        sampled_matrices = frames
            .checked_add(1)
            .and_then(|keys| keys.checked_mul(target_bones.len()))
            .and_then(|count| sampled_matrices.checked_add(count))
            .filter(|count| *count <= 250_000)
            .ok_or_else(|| {
                Error::new(
                    "fbx-retarget",
                    "sampled output exceeds250000-matrix import policy",
                )
            })?;
        let source = imported_clip["boneMatrices"]
            .as_array()
            .filter(|a| a.len() == source_bones.len())
            .ok_or_else(|| {
                Error::new("fbx-retarget", "imported per-bone matrix tracks required")
            })?;
        let mut tracks = Vec::with_capacity(target_bones.len());
        for (target, source_index) in target_bones.iter().zip(&mapping) {
            let target_rest = model_matrix(&target["transform"])?;
            let keys = if let Some(index) = source_index {
                let source_rest = model_matrix(&source_bones[*index]["transform"])?;
                let inverse = source_rest
                    .inverse()
                    .ok_or_else(|| Error::new("fbx-retarget", "singular imported rest matrix"))?;
                source[*index]
                    .as_array()
                    .filter(|a| a.len() == frames + 1)
                    .ok_or_else(|| Error::new("fbx-retarget", "imported track lacks N+1 keys"))?
                    .iter()
                    .map(|key| {
                        let matrix = target_rest * inverse * model_matrix(key)?;
                        if !matrix.finite() {
                            return Err(Error::new("fbx-retarget", "retarget matrix overflow"));
                        }
                        Ok(json!(matrix.0))
                    })
                    .collect::<Result<Vec<_>>>()?
            } else {
                vec![json!(target_rest.0); frames + 1]
            };
            tracks.push(Value::Array(keys));
        }
        let mut clip = imported_clip.clone();
        next_id = next_id
            .checked_add(1)
            .ok_or_else(|| Error::new("fbx-retarget", "clip id overflow"))?;
        clip["id"] = json!(next_id);
        clip["sourceClip"] = Value::Null;
        clip["boneMatrices"] = json!(tracks);
        clip["nativeMetadata"]["retargetProfile"] =
            json!("rest-relative-local-matrix-by-unique-bone-name-v1");
        clip["nativeMetadata"]["unmatchedTargetBones"] = json!(
            target_bones
                .iter()
                .zip(&mapping)
                .filter(|(_, i)| i.is_none())
                .map(|(b, _)| b["name"].clone())
                .collect::<Vec<_>>()
        );
        clips.push(clip);
    }
    document["clips"] = json!(clips);
    let mut validation = document.clone();
    validation["clips"] = json!(&clips[retained_clip_count..]);
    for bone in validation["bones"].as_array_mut().unwrap() {
        bone["sourceBone"] = Value::Null
    }
    wpe_sim::model::ModelAnimator::from_editable(&validation)
        .map_err(|e| Error::new("fbx-retarget", e.to_string()))?;
    Ok(document)
}
fn model_matrix(value: &Value) -> Result<Mat4> {
    let values = value
        .as_array()
        .filter(|a| a.len() == 16)
        .ok_or_else(|| Error::new("fbx-retarget", "16-component matrix required"))?;
    let values = values
        .iter()
        .map(|v| {
            v.as_f64()
                .map(|n| n as f32)
                .filter(|n| n.is_finite())
                .ok_or_else(|| Error::new("fbx-retarget", "finite matrix required"))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Mat4(values.try_into().unwrap()))
}

/// Produces the actual editable buffers consumed by `ModelAnimator` overlays.
/// Parent/skin indices retain their source identity. This is not a native MDL
/// version upgrade or a claim of binary-writer equivalence.
pub fn decode_model_to_editable(bytes: &[u8]) -> Result<Value> {
    if bytes.len() > 128 * 1024 * 1024 {
        return Err(Error::new(
            "resource-limit",
            "editable model input exceeds128MiB",
        ));
    }
    let model = Model::parse(bytes).map_err(|e| Error::new("model-format", e.to_string()))?;
    let skeleton = model.sections.iter().find_map(|section| {
        if let SectionData::Skeleton(skeleton) = &section.data {
            Some(skeleton)
        } else {
            None
        }
    });
    let bone_count = skeleton.map_or(0, |s| s.bones.len());
    let mut shapes = Vec::with_capacity(model.meshes.len());
    let mut issues = Vec::new();
    for (mesh_index, mesh) in model.meshes.iter().enumerate() {
        if mesh.vertex_format & 1 == 0 {
            return Err(Error::new(
                "model-conversion",
                "position stream is required",
            ));
        }
        if !mesh.indices.len().is_multiple_of(3) {
            return Err(Error::new("model-conversion", "non-triangle index stream"));
        }
        let mut format = vec!["position"];
        if mesh.vertex_format & 2 != 0 {
            format.push("normal")
        }
        if mesh.vertex_format & 4 != 0 {
            format.push("tangentSigned")
        }
        if mesh.vertex_format & 8 != 0 {
            format.push("uv")
        }
        if mesh.vertex_format & 0x10 != 0 {
            format.push("color")
        }
        let mut vertices = Vec::<f32>::new();
        for vertex in mesh.vertices.chunks_exact(mesh.vertex_stride) {
            let mut offset = 0;
            for (bit, width) in [(1, 3), (2, 3), (4, 4), (8, 2), (0x10, 3)] {
                if mesh.vertex_format & bit != 0 {
                    // Exact observed puppet layout inserts indices/weights
                    // between tangent and UV; bit-order stride is not order.
                    let start = if mesh.vertex_format == 0x180000f && bit == 8 {
                        72
                    } else {
                        offset
                    };
                    let end = start + width * 4;
                    let data = vertex.get(start..end).ok_or_else(|| {
                        Error::new("model-conversion", "attribute exceeds source stride")
                    })?;
                    for word in data.chunks_exact(4) {
                        let value = f32::from_le_bytes(word.try_into().unwrap());
                        if !value.is_finite() {
                            return Err(Error::new("model-conversion", "nonfinite source vertex"));
                        }
                        vertices.push(value);
                    }
                    if bit == 0x10 {
                        vertices.push(1.)
                    }
                    offset = end;
                }
            }
        }
        let mut shape = json!({
            "sourceMesh":mesh_index,"vertexFormat":format,"vertexBuffer":vertices,
            "indexBuffer":mesh.indices,"material":mesh.materials.first().cloned().unwrap_or_default(),
            "nativeMetadata":{"flags":mesh.flags,"extraFlags":mesh.extra_flags,
                "vertexFormat":mesh.vertex_format,"vertexStride":mesh.vertex_stride,
                "vertexBytes":mesh.vertices,"materials":mesh.materials,"batches":mesh.batches,
                "extraBuffers":mesh.extra_buffers.iter().map(|b|json!({"kind":b.kind,"bytes":b.data})).collect::<Vec<_>>()}
        });
        if mesh.vertex_format & !0x1f != 0 && mesh.vertex_format != 0x180000f {
            issues.push(format!("mesh {mesh_index}: additional native attributes are retained in nativeMetadata.vertexBytes and are outside the editable renderer attribute profile"));
        }
        if let Some(skin) = &mesh.skin {
            let mut weights = Vec::with_capacity(skin.len());
            for vertex in skin {
                let mut dense = vec![0_f32; bone_count];
                for component in 0..4 {
                    let weight = vertex.weights[component];
                    if !weight.is_finite() {
                        return Err(Error::new("model-conversion", "nonfinite skin weight"));
                    }
                    if weight != 0. {
                        let value = dense
                            .get_mut(vertex.bone_indices[component] as usize)
                            .ok_or_else(|| {
                                Error::new("model-conversion", "active skin index outside skeleton")
                            })?;
                        *value += weight;
                        if !value.is_finite() {
                            return Err(Error::new("model-conversion", "skin sum overflow"));
                        }
                    }
                }
                weights.push(dense);
            }
            shape["weights"] = json!(weights);
            shape["nativeMetadata"]["skin"] = json!(skin);
        }
        shapes.push(shape);
    }
    let bones=skeleton.map(|s|s.bones.iter().enumerate().map(|(index,bone)|json!({
        "sourceBone":index,"name":bone.name,"parent":bone.parent,"transform":bone.matrix,
        "properties":serde_json::from_str::<Value>(&bone.properties_json).unwrap_or_else(|_|json!({"raw":bone.properties_json}))
    })).collect::<Vec<_>>()).unwrap_or_default();
    let mut editable = json!({"profile":"wpe-editable-model-v1","shapes":shapes,"bones":bones,
        "boneDrawDepth":vec![0_f32;bone_count],"issues":issues,
        "nativeMetadata":{"model":model.summary(),"sourceBytes":bytes.len(),
            "originalResourceMustRemain":true,"binaryRewrite":false}});
    editable["clips"]=json!(model.sections.iter().flat_map(|s|{
        if let SectionData::Animation(clips)=&s.data {clips.iter().enumerate().map(|(index,clip)|json!({
            "sourceClip":index,"id":clip.id,"name":clip.name,"fps":clip.fps,"frames":clip.frames,"mode":clip.mode,
            "events":clip.events.iter().map(|event|json!({"seconds":f32::from_bits(event.frame_word),"text":event.text})).collect::<Vec<_>>()
        })).collect::<Vec<_>>()}else{Vec::new()}
    }).collect::<Vec<_>>());
    editable["attachments"] = json!(
        model
            .sections
            .iter()
            .flat_map(|section| {
                if let SectionData::Attachments(rows) = &section.data {
                    rows.iter()
                        .map(|row| json!({"name":row.name,"bone":row.bone,"transform":row.matrix}))
                        .collect::<Vec<_>>()
                } else {
                    Vec::new()
                }
            })
            .collect::<Vec<_>>()
    );
    Ok(editable)
}

impl Editor {
    /// Imports actual FBX geometry, skin and sampled clips into the editable
    /// model consumed by the scene player. Called inside execute() history.
    pub(crate) fn import_fbx_model(&mut self, name: &str, bytes: Vec<u8>) -> Result<Value> {
        let mut model = wpe_formats::fbx::decode_to_editable(&bytes)
            .map_err(|error| Error::new("fbx-import", error.to_string()))?;
        if model["shapes"].as_array().is_none_or(Vec::is_empty) {
            return Err(Error::new(
                "fbx-import",
                "FBX has no renderable mesh; import its clips into an existing model",
            ));
        }
        let source = self.unused_fbx_name(name)?;
        model["sourceModel"] = json!(source);
        wpe_sim::model::ModelAnimator::from_editable(&model)
            .map_err(|error| Error::new("fbx-import", error.to_string()))?;
        let file = format!("{source}.editable.json");
        self.put_bytes(&source, Some(bytes))?;
        self.put_resource(&file, Some(model.clone()))?;
        Ok(json!({"file":file,"modeldata":model,"source":source,"format":"fbx-editable-model-v1"}))
    }

    fn unused_fbx_name(&self, name: &str) -> Result<String> {
        let base = safe_basename(name)?;
        for suffix in 0..1024 {
            let source = format!("models/imports/{base}-{suffix}.fbx");
            bounded(&self.root, &source)?;
            let staged = self.state.files.get(&source).is_some_and(Option::is_some)
                || self
                    .state
                    .resources
                    .get(&source)
                    .is_some_and(Option::is_some);
            let disk = std::iter::once(&self.root)
                .chain(self.asset_roots.iter())
                .any(|root| root.join(&source).is_file());
            if !staged && !disk {
                return Ok(source);
            }
        }
        Err(Error::new(
            "fbx-import",
            "too many imports with the same filename",
        ))
    }

    /// Completes a native FBX animation picker with one atomic undoable edit.
    /// It preserves the source model, target skeleton/rest and existing clips.
    pub fn import_fbx_for_resource(
        &mut self,
        id: &Value,
        name: &str,
        bytes: Arc<Vec<u8>>,
    ) -> Result<Value> {
        let imported = wpe_formats::fbx::decode_to_editable(&bytes)
            .map_err(|error| Error::new("fbx-import", error.to_string()))?;
        if imported["clips"].as_array().is_none_or(Vec::is_empty) {
            return Err(Error::new(
                "fbx-import",
                "FBX contains no supported animation clips",
            ));
        }
        let before = self.state.clone();
        let previous_session = self.session.clone();
        let event_start = self.events.len();
        let result = (|| -> Result<Value> {
            let source = self.unused_fbx_name(name)?;
            let resource = self.resource_path(id).is_some();
            let (sidecar, existing) = if resource {
                let (sidecar, document) = self.ensure_model_document(id)?;
                (Some(sidecar), document)
            } else {
                let index = self.index(id)?;
                if self.objects()?[index].get("modeldata").is_none() {
                    self.upgrade_puppet_model(id)?;
                }
                let data = self.objects()?[index]["modeldata"].clone();
                let document = if let Some(path) = data.as_str() {
                    self.resource(path)?
                } else {
                    data
                };
                (None, document)
            };
            let retained = existing["clips"].as_array().map_or(0, Vec::len);
            let mut document = merge_imported_animation(&existing, &imported)?;
            self.validate_model_document(&document)?;
            let new_clips = document["clips"].as_array_mut().unwrap();
            for clip in &mut new_clips[retained..] {
                clip["sourceFbx"] = json!(source);
            }
            let summaries = new_clips[retained..].iter()
                .map(|clip|json!({"id":clip["id"],"name":clip["name"],"fps":clip["fps"],"frames":clip["frames"]}))
                .collect::<Vec<_>>();
            let mut files = document["fbxfiles"].as_array().cloned().unwrap_or_default();
            files.push(json!({"file":source,"name":name,"clips":summaries,
                "profile":"rest-relative-local-matrix-by-unique-bone-name-v1"}));
            document["fbxfiles"] = json!(files);
            if !self.original_files.contains_key(&source) {
                self.original_files.insert(source.clone(), None);
            }
            self.state.resources.remove(&source);
            self.state.files.insert(source.clone(), Some(bytes));
            if let Some(sidecar) = &sidecar {
                self.put_resource(sidecar, Some(document.clone()))?;
                self.session["modelDocuments"][id.as_str().unwrap_or("")] = json!(sidecar);
                if let Some(object) = self.session.pointer_mut("/modelPreview/objects/0") {
                    object["modeldata"] = document.clone();
                }
            } else {
                let index = self.index(id)?;
                let layer = &mut self.objects_mut()?[index];
                layer["modeldata"] = document.clone();
                layer["bones"] = document["bones"].clone();
            }
            validate_documents(&self.state.project, &self.state.scene)?;
            Ok(json!({"resource":sidecar,"source":source,"clips":summaries,
                "sourceModelUnchanged":true,"profile":"fbx-retarget-editable-model-v1"}))
        })();
        let value = match result {
            Ok(value) => value,
            Err(error) => {
                self.state = before;
                self.session = previous_session;
                self.events.truncate(event_start);
                return Err(error);
            }
        };
        if self.changed(&before) {
            self.revision += 1;
            self.mark_dirty();
            if self.transaction.is_none() {
                self.record(&before);
            }
            self.events.push(EditorEvent {
                kind: "documentChanged".into(),
                payload: json!({"revision":self.revision,"dirty":self.state.dirty}),
            });
        }
        Ok(value)
    }

    /// Stages owned binary bytes without serializing or cloning the payload.
    /// Uses the same bounded paths, history and dirty state as editor commands.
    pub fn stage_resource_bytes(&mut self, name: &str, bytes: Arc<Vec<u8>>) -> Result<()> {
        bounded(&self.root, name)?;
        if bytes.len() > 128 * 1024 * 1024 {
            return Err(Error::new("resource-limit", "file exceeds128MiB"));
        }
        let before = self.state.clone();
        if !self.original_files.contains_key(name) {
            self.original_files
                .insert(name.into(), self.read_bytes(name).ok().map(Arc::new));
        }
        self.state.resources.remove(name);
        self.state.files.insert(name.into(), Some(bytes));
        if self.changed(&before) {
            self.revision += 1;
            self.mark_dirty();
            if self.transaction.is_none() {
                self.record(&before);
            }
            self.events.push(EditorEvent {
                kind: "documentChanged".into(),
                payload: json!({"revision":self.revision,"dirty":self.state.dirty}),
            });
        }
        Ok(())
    }
    fn validate_model_document(&self, document: &Value) -> Result<()> {
        if let Some(source) = document.get("sourceModel").and_then(Value::as_str)
            && source.to_ascii_lowercase().ends_with(".mdl")
        {
            let model = Model::parse(&self.read_bytes(source)?)
                .map_err(|error| Error::new("model-format", error.to_string()))?;
            let mut animator = wpe_sim::model::ModelAnimator::new(model)
                .map_err(|error| Error::new("model-format", error.to_string()))?;
            animator
                .apply_editable_shapes(document)
                .map_err(|error| Error::new("model-editable", error.to_string()))?;
        } else {
            wpe_sim::model::ModelAnimator::from_editable(document)
                .map_err(|error| Error::new("model-editable", error.to_string()))?;
        }
        Ok(())
    }
    fn ensure_model_document(&mut self, id: &Value) -> Result<(String, Value)> {
        let source = self
            .resource_path(id)
            .ok_or_else(|| Error::new("model-resource", "resource handle/path required"))?;
        bounded(&self.root, &source)?;
        if source.to_ascii_lowercase().ends_with(".json") {
            let document = self.resource(&source)?;
            if document["profile"] != "wpe-editable-model-v1" {
                return Err(Error::new(
                    "model-resource",
                    "editable model JSON profile required",
                ));
            }
            return Ok((source, document));
        }
        if !source.to_ascii_lowercase().ends_with(".mdl")
            && !source.to_ascii_lowercase().ends_with(".fbx")
        {
            return Err(Error::new(
                "model-resource",
                "MDL, FBX or editable JSON resource required",
            ));
        }
        let sidecar = format!("{source}.editable.json");
        let existing = match self.read_bytes(&sidecar) {
            Ok(bytes) => Some(wpe_formats::jsonc::parse(&bytes).map_err(|e| {
                Error::new("model-resource", format!("existing sidecar JSON: {e}"))
            })?),
            Err(error) if error.code == "resource" => None,
            Err(error) => return Err(error),
        };
        if let Some(document) = existing {
            if document["profile"] == "wpe-editable-model-v1" && document["sourceModel"] == source {
                return Ok((sidecar, document));
            }
            return Err(Error::new(
                "model-resource",
                "existing sidecar is not the matching editable model",
            ));
        }
        let mut document = if source.to_ascii_lowercase().ends_with(".fbx") {
            wpe_formats::fbx::decode_to_editable(&self.read_bytes(&source)?)
                .map_err(|error| Error::new("fbx-import", error.to_string()))?
        } else {
            decode_model_to_editable(&self.read_bytes(&source)?)?
        };
        document["sourceModel"] = json!(source);
        self.put_resource(&sidecar, Some(document.clone()))?;
        Ok((sidecar, document))
    }
    /// Resource MODEL_ACTIONS reuse the local layer document handlers. Only the
    /// sidecar enters resource history; the temporary layer is removed first.
    pub(crate) fn resource_model_action(&mut self, args: &[Value]) -> Result<Value> {
        let action = string(args, 0)?;
        let id = arg(args, 1)?.clone();
        let (sidecar, mut document) = self.ensure_model_document(&id)?;
        if matches!(
            action,
            "playClipAnimation" | "playStaticAnimation" | "stopAnimation" | "setEditMode"
        ) {
            let mut object = document.clone();
            object["id"] = json!("__model-resource-preview");
            object["modeldata"] = document.clone();
            object["visible"] = json!(true);
            object["origin"] = json!("0 0 0");
            object["scale"] = json!("1 1 1");
            object["angles"] = json!("0 0 0");
            let selection = args.get(2).cloned().unwrap_or(json!(0));
            let clip = selection
                .as_u64()
                .and_then(|v| usize::try_from(v).ok())
                .or_else(|| {
                    selection
                        .get("clip")
                        .and_then(Value::as_u64)
                        .and_then(|v| usize::try_from(v).ok())
                })
                .or_else(|| {
                    selection.as_str().and_then(|name| {
                        document["clips"]
                            .as_array()?
                            .iter()
                            .position(|clip| clip["name"] == name)
                    })
                })
                .unwrap_or(0);
            if action == "playClipAnimation"
                && document["clips"]
                    .as_array()
                    .is_none_or(|clips| clip >= clips.len())
            {
                return Err(Error::new(
                    "model-preview",
                    "clip index outside editable model",
                ));
            }
            if let Some(spec) = document["clips"].get(clip) {
                object["animationlayers"] = json!([{"clip":clip,"name":spec["name"],"fps":spec["fps"],"frameCount":spec["frames"],"__time":0,"__playing":action=="playClipAnimation","rate":1,"blend":1,"loop":true}]);
            }
            let mut scene = self.state.scene.clone();
            scene["objects"] = json!([object]);
            self.session["modelPreview"] = scene;
            self.session["modelEditor"] = json!({"resource":sidecar,"source":id,"clip":clip,"action":action,"value":selection});
            return Ok(self.request_host(action,&[id,json!({"resource":sidecar,"modelResourcePreview":true,"clip":clip,"value":selection})]));
        }
        if action == "addFBXfile" {
            return Ok(self.request_host(
                action,
                &[
                    id,
                    json!({"resource":sidecar,"modelResourceEdit":true,"value":args.get(2)}),
                ],
            ));
        }
        let transient = json!(format!("__resource-model-{}", self.next_request));
        if self.index(&transient).is_ok() {
            return Err(Error::new("model-resource", "temporary model id collision"));
        }
        let selected_clip = args
            .get(2)
            .and_then(|v| v.get("clip"))
            .and_then(Value::as_u64)
            .or_else(|| {
                self.session
                    .pointer("/modelEditor/clip")
                    .and_then(Value::as_u64)
            })
            .unwrap_or(0) as usize;
        let mut proxy = document.clone();
        proxy["id"] = transient.clone();
        proxy["modeldata"] = document.clone();
        if matches!(action, "addEvent" | "removeEvent") {
            proxy["events"] = document["clips"]
                .get(selected_clip)
                .and_then(|clip| clip.get("events"))
                .cloned()
                .ok_or_else(|| Error::new("model-resource", "selected clip events required"))?;
        }
        let mut forwarded = args.to_vec();
        forwarded[1] = transient.clone();
        if matches!(action, "addClip" | "addAttachment" | "addEvent") {
            let data = parse_argument(args.get(2))?;
            let value = if action == "addClip" {
                let source = data
                    .get("sourceClip")
                    .and_then(Value::as_u64)
                    .unwrap_or(selected_clip as u64) as usize;
                let mut clip = document["clips"].get(source).cloned().ok_or_else(|| {
                    Error::new(
                        "model-resource",
                        "addClip requires an imported source animation",
                    )
                })?;
                if let Some(config) = data.as_object() {
                    for (key, value) in config {
                        clip[key] = value.clone();
                    }
                } else if let Some(name) = data.as_str() {
                    clip["name"] = json!(name);
                }
                let maximum = document["clips"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter_map(|clip| clip["id"].as_u64())
                    .filter(|id| *id < 9_007_199_254_740_990)
                    .max()
                    .unwrap_or(0);
                clip["id"] = json!(maximum + 1);
                clip
            } else if action == "addAttachment" {
                let mut attachment = if data.is_object() {
                    data
                } else {
                    json!({"name":data.as_str().unwrap_or("attachment")})
                };
                if attachment.get("bone").is_none() {
                    attachment["bone"] = json!(0);
                }
                if attachment.get("transform").is_none() && attachment.get("matrix").is_none() {
                    attachment["transform"] =
                        json!([1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1]);
                }
                attachment
            } else {
                let mut event = if data.is_object() {
                    data
                } else {
                    json!({"name":data.as_str().unwrap_or("event")})
                };
                if event.get("frame").is_none() && event.get("seconds").is_none() {
                    event["frame"] = json!(0);
                }
                event
            };
            if forwarded.len() < 3 {
                forwarded.resize(3, Value::Null);
            }
            forwarded[2] = value;
        }
        self.objects_mut()?.push(proxy);
        let result = self.action(true, &forwarded);
        let index = self.index(&transient)?;
        let edited = self.objects_mut()?.remove(index);
        let result = result?;
        for key in [
            "clips",
            "attachments",
            "constraints",
            "fbxfiles",
            "bones",
            "poses",
        ] {
            if let Some(value) = edited.get(key) {
                document[key] = value.clone();
            }
        }
        if matches!(action, "addEvent" | "removeEvent") {
            document["clips"][selected_clip]["events"] = edited["events"].clone();
        }
        self.put_resource(&sidecar, Some(document))?;
        self.session["modelDocuments"][id.as_str().unwrap_or("")] = json!(sidecar);
        Ok(
            json!({"resource":sidecar,"value":result,"profile":"wpe-editable-model-v1","sourceUnchanged":true}),
        )
    }
    pub fn model_preview_scene(&self) -> Option<Value> {
        self.session
            .get("modelPreview")
            .filter(|scene| scene.is_object())
            .cloned()
    }
    pub fn clear_model_preview(&mut self) {
        if let Some(session) = self.session.as_object_mut() {
            session.remove("modelPreview");
            session.remove("modelEditor");
        }
    }
    pub fn editable_model_resource(&mut self, id: &Value) -> Result<Value> {
        self.ensure_model_document(id).map(|(_, document)| document)
    }
    /// Called within the ordinary editor command transaction/history boundary.
    pub(crate) fn upgrade_puppet_model(&mut self, id: &Value) -> Result<Value> {
        let index = self.index(id)?;
        let layer = self.objects()?[index].clone();
        if layer
            .get("modeldata")
            .is_some_and(|v| v.get("shapes").is_some())
        {
            return Ok(
                json!({"converted":false,"profile":"wpe-editable-model-v1","reason":"already editable"}),
            );
        }
        let mut material = None;
        let source = if let Some(model) = layer.get("model").and_then(Value::as_str) {
            if model.to_ascii_lowercase().ends_with(".mdl") {
                model.to_owned()
            } else {
                let descriptor = self.resource(model)?;
                descriptor
                    .get("model")
                    .or_else(|| descriptor.get("file"))
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        Error::new("model-conversion", "model descriptor has no MDL path")
                    })?
                    .to_owned()
            }
        } else if let Some(image) = layer.get("image").and_then(Value::as_str) {
            let descriptor = self.resource(image)?;
            material = descriptor
                .get("material")
                .and_then(Value::as_str)
                .map(str::to_owned);
            descriptor
                .get("puppet")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    Error::new("model-conversion", "image descriptor has no puppet MDL")
                })?
                .to_owned()
        } else {
            return Err(Error::new(
                "model-conversion",
                "a source model or image puppet descriptor is required",
            ));
        };
        bounded(&self.root, &source)?;
        let bytes = self.read_bytes(&source)?;
        let mut editable = decode_model_to_editable(&bytes)?;
        editable["sourceModel"] = json!(source);
        editable["nativeMetadata"]["originalLayerFields"] = json!({"image":layer.get("image"),"model":layer.get("model"),"material":layer.get("material")});
        if let Some(material) = material {
            for shape in editable["shapes"].as_array_mut().unwrap() {
                if shape["material"].as_str().is_none_or(str::is_empty) {
                    shape["material"] = json!(material)
                }
            }
        }
        let bones = editable["bones"].clone();
        let layer = &mut self.objects_mut()?[index];
        layer["modeldata"] = editable;
        if layer.get("bones").is_none() {
            layer["bones"] = bones
        }
        let object = layer
            .as_object_mut()
            .ok_or_else(|| Error::new("model-conversion", "layer must be an object"))?;
        object.remove("image");
        object.remove("model");
        Ok(
            json!({"converted":true,"profile":"wpe-editable-model-v1","sourceModel":source,"sourceUnchanged":true}),
        )
    }

    pub(crate) fn reorder_puppet_bone(
        &mut self,
        id: &Value,
        bone: &Value,
        front: bool,
    ) -> Result<Value> {
        let bone = bone
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .ok_or_else(|| Error::new("bone-order", "bone index required"))?;
        let index = self.index(id)?;
        if self.objects()?[index].get("modeldata").is_none() {
            self.upgrade_puppet_model(id)?;
        }
        let layer = &mut self.objects_mut()?[index];
        let bones = layer["bones"]
            .as_array()
            .ok_or_else(|| Error::new("bone-order", "bones metadata required"))?;
        if bone >= bones.len() {
            return Err(Error::new("bone-order", "bone outside skeleton"));
        }
        let count = bones.len();
        let model = layer["modeldata"]
            .as_object_mut()
            .ok_or_else(|| Error::new("bone-order", "inline editable model required"))?;
        let depth = model
            .entry("boneDrawDepth")
            .or_insert_with(|| json!(vec![0_f32; count]));
        let values = depth
            .as_array_mut()
            .filter(|v| v.len() == count)
            .ok_or_else(|| Error::new("bone-order", "draw-depth count mismatch"))?;
        let finite = values
            .iter()
            .map(|v| {
                v.as_f64()
                    .filter(|v| v.is_finite())
                    .ok_or_else(|| Error::new("bone-order", "finite draw depth required"))
            })
            .collect::<Result<Vec<_>>>()?;
        let edge = if front {
            finite.iter().copied().fold(f64::NEG_INFINITY, f64::max)
        } else {
            finite.iter().copied().fold(f64::INFINITY, f64::min)
        };
        let value = edge + if front { 1. } else { -1. };
        if !(value as f32).is_finite() {
            return Err(Error::new("bone-order", "draw-depth overflow"));
        }
        values[bone] = json!(value);
        layer["bones"][bone]["drawDepth"] = json!(value);
        Ok(
            json!({"bone":bone,"drawDepth":value,"profile":"stable-dominant-bone-triangle-order","parentSkinIndicesPreserved":true}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const OWNED_FBX: &[u8] =
        include_bytes!("../../wpe-formats/tests/fixtures/owned-skinned-animation-ascii.fbx");
    const OWNED_FBX_BINARY: &[u8] =
        include_bytes!("../../wpe-formats/tests/fixtures/owned-skinned-animation-binary.fbx");
    #[test]
    fn fbx_geometry_import_has_real_skin_motion_and_atomic_saved_history() {
        let fixture = Fixture::new();
        let mut editor = fixture.editor(json!([]));
        let reply = editor
            .execute(
                "importModelAddToScene",
                json!([{"name":"triangle.fbx","bytes":OWNED_FBX_BINARY}]),
            )
            .unwrap();
        let resource = &reply.value["resource"];
        let source = resource["source"].as_str().unwrap().to_owned();
        let sidecar = resource["file"].as_str().unwrap().to_owned();
        let document = resource["modeldata"].clone();
        let mut animator = wpe_sim::model::ModelAnimator::from_editable(&document).unwrap();
        animator.step(0.5).unwrap();
        assert!((animator.skinned_meshes().unwrap()[0].vertices[0].position[0] + 1.).abs() < 1e-6);
        assert_eq!(editor.snapshot().scene["objects"][0]["modeldata"], document);
        editor.save().unwrap();
        assert_eq!(fs::read(fixture.0.join(&source)).unwrap(), OWNED_FBX_BINARY);
        let saved: Value =
            serde_json::from_slice(&fs::read(fixture.0.join(&sidecar)).unwrap()).unwrap();
        assert_eq!(saved, document);
        editor.undo().unwrap();
        editor.save().unwrap();
        assert!(
            editor.snapshot().scene["objects"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert!(!fixture.0.join(&source).exists());
        assert!(!fixture.0.join(&sidecar).exists());
        editor.redo().unwrap();
        editor.save().unwrap();
        assert_eq!(fs::read(fixture.0.join(source)).unwrap(), OWNED_FBX_BINARY);
    }
    #[test]
    fn fbx_animation_retarget_preserves_native_model_and_executes_the_appended_clip() {
        let fixture = Fixture::new();
        let mut editor = fixture.editor(json!([]));
        let original = fs::read(fixture.0.join("models/owned.mdl")).unwrap();
        let result = editor
            .import_fbx_for_resource(
                &json!("models/owned.mdl"),
                "move.fbx",
                Arc::new(OWNED_FBX.to_vec()),
            )
            .unwrap();
        let source = result["source"].as_str().unwrap().to_owned();
        let sidecar = result["resource"].as_str().unwrap().to_owned();
        let document = editor.snapshot().resources[&sidecar].clone();
        assert_eq!(document["clips"].as_array().unwrap().len(), 2);
        assert_eq!(document["clips"][0]["name"], "owned-motion");
        let mut animator =
            wpe_sim::model::ModelAnimator::new(Model::parse(&original).unwrap()).unwrap();
        animator.apply_editable_shapes(&document).unwrap();
        animator.layers = vec![wpe_sim::model::AnimationLayer::new(1)];
        animator.step(0.5).unwrap();
        assert!((animator.skinned_meshes().unwrap()[0].vertices[0].position[0] + 4.).abs() < 1e-6);
        editor.save().unwrap();
        assert_eq!(
            fs::read(fixture.0.join("models/owned.mdl")).unwrap(),
            original
        );
        assert_eq!(fs::read(fixture.0.join(&source)).unwrap(), OWNED_FBX);
        editor.undo().unwrap();
        assert!(editor.read_bytes(&source).is_err());
        assert!(editor.read_bytes(&sidecar).is_err());
        editor.redo().unwrap();
        editor.save().unwrap();
        assert_eq!(
            fs::read(fixture.0.join("models/owned.mdl")).unwrap(),
            original
        );
    }
    #[test]
    fn fbx_failed_retarget_rolls_back_all_staged_resources_and_dirty_state() {
        let fixture = Fixture::new();
        let mut editor = fixture.editor(json!([]));
        let before = editor.snapshot();
        let bad = std::str::from_utf8(OWNED_FBX)
            .unwrap()
            .replace("owned-root", "unmatched");
        assert!(
            editor
                .import_fbx_for_resource(
                    &json!("models/owned.mdl"),
                    "bad.fbx",
                    Arc::new(bad.into_bytes())
                )
                .is_err()
        );
        assert_eq!(editor.snapshot().resources, before.resources);
        assert_eq!(editor.snapshot().scene, before.scene);
        assert!(!editor.snapshot().dirty);
    }
    #[test]
    fn sampled_animation_retargets_by_name_and_keeps_original_rest_and_clip() {
        let mut existing = decode_model_to_editable(include_bytes!(
            "../tests/fixtures/editable-skinned-triangle.mdl"
        ))
        .unwrap();
        existing["bones"][0]["name"] = json!("root");
        let native_clip = existing["clips"][0].clone();
        let rest = Mat4::trs(
            wpe_sim::math::Vec3::new(10., 0., 0.),
            wpe_sim::math::Quat::IDENTITY,
            wpe_sim::math::Vec3::ONE,
        );
        let moved = Mat4::trs(
            wpe_sim::math::Vec3::new(12., 0., 0.),
            wpe_sim::math::Quat::IDENTITY,
            wpe_sim::math::Vec3::ONE,
        );
        let imported = json!({"bones":[{"name":"root","transform":rest.0}],"clips":[{"name":"walk","fps":1,"frames":1,"boneMatrices":[[rest.0,moved.0]]}]});
        let merged = merge_imported_animation(&existing, &imported).unwrap();
        assert_eq!(merged["bones"], existing["bones"]);
        assert_eq!(merged["shapes"], existing["shapes"]);
        assert_eq!(merged["clips"][0], native_clip);
        let key = model_matrix(&merged["clips"][1]["boneMatrices"][0][1]).unwrap();
        let bind = model_matrix(&existing["bones"][0]["transform"]).unwrap();
        assert!((key.translation().0[0] - bind.translation().0[0] - 2.).abs() < 1e-5);
        let mut bad = imported;
        bad["bones"][0]["name"] = json!("unmapped");
        assert!(merge_imported_animation(&existing, &bad).is_err());
    }
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "wpe-editor-model-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(root.join("models")).unwrap();
            fs::write(
                root.join("models/owned.mdl"),
                include_bytes!("../tests/fixtures/editable-skinned-triangle.mdl"),
            )
            .unwrap();
            Self(root)
        }
        fn editor(&self, objects: Value) -> Editor {
            Editor::new(
                self.0.clone(),
                json!({"title":"owned","type":"scene","file":"scene.json"}),
                json!({"general":{},"objects":objects}),
            )
            .unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn triangle() -> Vec<u8> {
        let mut bytes = b"MDLV0023\0".to_vec();
        for v in [9_u32, 0, 1, 0] {
            bytes.extend(v.to_le_bytes());
        }
        for v in [0_f32, 0., 0., 1., 1., 0.] {
            bytes.extend(v.to_le_bytes());
        }
        bytes.extend(9_u32.to_le_bytes());
        bytes.extend(60_u32.to_le_bytes());
        for v in [
            0_f32, 0., 0., 0., 0., 1., 0., 0., 1., 0., 0., 1., 0., 0., 1.,
        ] {
            bytes.extend(v.to_le_bytes());
        }
        bytes.extend(6_u32.to_le_bytes());
        for i in [0_u16, 1, 2] {
            bytes.extend(i.to_le_bytes());
        }
        bytes.extend([0, 0]);
        bytes.extend(0_u32.to_le_bytes());
        bytes.push(0);
        bytes
    }
    #[test]
    fn actual_geometry_and_unknown_source_metadata_are_preserved() {
        let bytes = triangle();
        let editable = decode_model_to_editable(&bytes).unwrap();
        assert_eq!(
            editable["shapes"][0]["vertexFormat"],
            json!(["position", "uv"])
        );
        assert_eq!(
            editable["shapes"][0]["vertexBuffer"]
                .as_array()
                .unwrap()
                .len(),
            15
        );
        assert_eq!(editable["shapes"][0]["indexBuffer"], json!([0, 1, 2]));
        assert_eq!(editable["nativeMetadata"]["binaryRewrite"], false);
        assert_eq!(
            editable["nativeMetadata"]["originalResourceMustRemain"],
            true
        );
        assert!(decode_model_to_editable(&bytes[..20]).is_err());
    }
    #[test]
    fn arc_binary_staging_uses_history_transactions_and_bounded_paths() {
        let fixture = Fixture::new();
        let mut editor = fixture.editor(json!([]));
        let bytes = Arc::new(vec![1, 2, 3, 255]);
        editor
            .stage_resource_bytes("materials/owned.bin", bytes.clone())
            .unwrap();
        let resources = editor.preview_resources().unwrap();
        assert!(Arc::ptr_eq(
            resources["materials/owned.bin"].as_ref().unwrap(),
            &bytes
        ));
        assert!(editor.snapshot().dirty);
        assert!(editor.undo().unwrap());
        assert!(editor.read_bytes("materials/owned.bin").is_err());
        assert!(editor.redo().unwrap());
        assert!(
            editor
                .stage_resource_bytes("../escape.bin", bytes.clone())
                .is_err()
        );
        editor.begin_transaction().unwrap();
        editor
            .stage_resource_bytes("materials/a.bin", bytes.clone())
            .unwrap();
        editor
            .stage_resource_bytes("materials/b.bin", bytes)
            .unwrap();
        editor.commit().unwrap();
        assert!(editor.undo().unwrap());
        assert!(editor.read_bytes("materials/a.bin").is_err());
        assert!(editor.read_bytes("materials/b.bin").is_err());
        assert_eq!(
            editor.read_bytes("materials/owned.bin").unwrap(),
            [1, 2, 3, 255]
        );
    }
    #[test]
    fn model_resource_actions_use_sidecar_history_and_preserve_binary_source() {
        let fixture = Fixture::new();
        let mut editor = fixture.editor(json!([]));
        let original = fs::read(fixture.0.join("models/owned.mdl")).unwrap();
        editor
            .execute(
                "modelAction",
                json!(["addEvent","models/owned.mdl",{"name":"edited","frame":1}]),
            )
            .unwrap();
        let sidecar = "models/owned.mdl.editable.json";
        assert!(
            editor.snapshot().scene["objects"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            editor.snapshot().resources[sidecar]["clips"][0]["events"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            editor.snapshot().resources[sidecar]["clips"][0]["events"][1]["name"],
            "edited"
        );
        assert!(editor.undo().unwrap());
        assert!(editor.read_bytes(sidecar).is_err());
        assert!(editor.redo().unwrap());
        editor.save().unwrap();
        assert!(fixture.0.join(sidecar).is_file());
        assert_eq!(
            fs::read(fixture.0.join("models/owned.mdl")).unwrap(),
            original
        );
        let before = editor.snapshot().resources[sidecar].clone();
        assert!(
            editor
                .execute(
                    "modelAction",
                    json!(["removeEvent", "models/owned.mdl", 500])
                )
                .is_err()
        );
        assert_eq!(editor.snapshot().resources[sidecar], before);
        assert!(
            editor.snapshot().scene["objects"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }
    #[test]
    fn upgrade_and_bone_order_change_editable_data_and_are_undoable() {
        let fixture = Fixture::new();
        let mut editor = fixture.editor(json!([{"id":1,"model":"models/owned.mdl","name":"own"}]));
        editor
            .execute("genericSceneAction", json!(["upgradePuppetModel", 1]))
            .unwrap();
        let layer = &editor.snapshot().scene["objects"][0];
        assert!(layer.get("model").is_none());
        assert_eq!(layer["modeldata"]["sourceModel"], "models/owned.mdl");
        assert_eq!(
            layer["modeldata"]["shapes"][0]["weights"],
            json!([[1.], [1.], [1.]])
        );
        editor
            .execute("genericSceneAction", json!(["bringBoneToFront", 1, 0]))
            .unwrap();
        assert_eq!(
            editor.snapshot().scene["objects"][0]["modeldata"]["boneDrawDepth"],
            json!([1.])
        );
        assert!(editor.undo().unwrap());
        assert_eq!(
            editor.snapshot().scene["objects"][0]["modeldata"]["boneDrawDepth"],
            json!([0.])
        );
        assert!(editor.undo().unwrap());
        assert_eq!(
            editor.snapshot().scene["objects"][0]["model"],
            "models/owned.mdl"
        );
    }
    #[test]
    fn model_preview_uses_real_scene_and_keeps_author_scene_separate() {
        let fixture = Fixture::new();
        let mut editor = fixture.editor(json!([{"id":1,"text":"keep"}]));
        let original = editor.snapshot().scene;
        let reply = editor
            .execute(
                "modelAction",
                json!(["playClipAnimation", "models/owned.mdl", 0]),
            )
            .unwrap();
        assert_eq!(reply.value["pending"], true);
        let preview = editor.model_preview_scene().unwrap();
        assert_eq!(
            preview["objects"][0]["modeldata"]["sourceModel"],
            "models/owned.mdl"
        );
        assert_eq!(
            preview["objects"][0]["animationlayers"][0]["__playing"],
            true
        );
        assert_eq!(editor.snapshot().scene, original);
        editor.clear_model_preview();
        assert!(editor.model_preview_scene().is_none());
    }
}
