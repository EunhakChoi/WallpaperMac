use crate::*;
const MAX_FILE: usize = 128 * 1024 * 1024;
impl Editor {
    pub fn import_external_file(
        &mut self,
        source: &Path,
        destination: &str,
    ) -> Result<EditorReply> {
        let bytes = fs::read(source).map_err(io_error)?;
        if bytes.len() > MAX_FILE {
            return Err(Error::new("resource-limit", "file exceeds128MiB"));
        }
        self.execute("writeProjectFile", json!([destination,{"bytes":bytes}]))
    }
    pub(crate) fn read_bytes(&self, name: &str) -> Result<Vec<u8>> {
        if let Some(value) = self.state.files.get(name) {
            return value
                .as_ref()
                .map(|v| v.as_ref().clone())
                .ok_or_else(|| Error::new("resource", "resource deleted"));
        }
        if let Some(value) = self.state.resources.get(name) {
            return value
                .as_ref()
                .map(serde_json::to_vec_pretty)
                .transpose()
                .map_err(json_error)?
                .ok_or_else(|| Error::new("resource", "resource deleted"));
        }
        for root in std::iter::once(&self.root).chain(self.asset_roots.iter()) {
            let path = bounded(root, name)?;
            if path.is_file() {
                let metadata = fs::metadata(&path).map_err(io_error)?;
                if metadata.len() > MAX_FILE as u64 {
                    return Err(Error::new("resource-limit", "file exceeds128MiB"));
                }
                return fs::read(path).map_err(io_error);
            }
        }
        Err(Error::new(
            "resource",
            format!("resource not found: {name}"),
        ))
    }
    pub(crate) fn resource(&mut self, name: &str) -> Result<Value> {
        if let Some(value) = self.state.resources.get(name) {
            return value
                .clone()
                .ok_or_else(|| Error::new("resource", "resource deleted"));
        }
        let bytes = self.read_bytes(name)?;
        let value =
            wpe_formats::jsonc::parse(&bytes).map_err(|e| Error::new("json", e.to_string()))?;
        self.original_resources
            .entry(name.into())
            .or_insert_with(|| Some(value.clone()));
        self.state
            .resources
            .insert(name.into(), Some(value.clone()));
        Ok(value)
    }
    pub(crate) fn put_resource(&mut self, name: &str, value: Option<Value>) -> Result<()> {
        bounded(&self.root, name)?;
        if !self.original_resources.contains_key(name) {
            let original = self
                .read_bytes(name)
                .ok()
                .and_then(|bytes| wpe_formats::jsonc::parse(&bytes).ok());
            self.original_resources.insert(name.into(), original);
        }
        self.state.files.remove(name);
        self.state.resources.insert(name.into(), value);
        Ok(())
    }
    pub(crate) fn put_bytes(&mut self, name: &str, value: Option<Vec<u8>>) -> Result<()> {
        bounded(&self.root, name)?;
        if value.as_ref().is_some_and(|v| v.len() > MAX_FILE) {
            return Err(Error::new("resource-limit", "file exceeds128MiB"));
        }
        if !self.original_files.contains_key(name) {
            let original = self.read_bytes(name).ok().map(Arc::new);
            self.original_files.insert(name.into(), original);
        }
        self.state.resources.remove(name);
        self.state.files.insert(name.into(), value.map(Arc::new));
        Ok(())
    }
    pub(crate) fn file_list(&self) -> Result<Vec<String>> {
        let mut names = BTreeSet::new();
        fn walk(root: &Path, dir: &Path, out: &mut BTreeSet<String>) -> Result<()> {
            for item in fs::read_dir(dir).map_err(io_error)? {
                let item = item.map_err(io_error)?;
                let ty = item.file_type().map_err(io_error)?;
                if ty.is_symlink() {
                    continue;
                }
                if ty.is_dir() {
                    walk(root, &item.path(), out)?
                } else if ty.is_file() {
                    let name = item
                        .path()
                        .strip_prefix(root)
                        .map_err(|_| Error::new("path", "invalid root"))?
                        .to_string_lossy()
                        .replace('\\', "/");
                    out.insert(name);
                    if out.len() > 100_000 {
                        return Err(Error::new("resource-limit", "file count exceeds100000"));
                    }
                }
            }
            Ok(())
        }
        for root in std::iter::once(&self.root).chain(self.asset_roots.iter()) {
            if root.is_dir() {
                walk(root, root, &mut names)?
            }
        }
        for (k, v) in &self.state.files {
            if v.is_some() {
                names.insert(k.clone());
            } else {
                names.remove(k);
            }
        }
        for (k, v) in &self.state.resources {
            if v.is_some() {
                names.insert(k.clone());
            } else {
                names.remove(k);
            }
        }
        Ok(names.into_iter().collect())
    }
    pub(crate) fn import_image(&mut self, name: &str, bytes: &[u8], add: bool) -> Result<Value> {
        let decoded = decode_image(bytes)?;
        let basename = safe_basename(name)?;
        let tex = format!("materials/{basename}.tex");
        let material = format!("materials/{basename}.json");
        let descriptor = format!("models/{basename}.json");
        let image = wpe_formats::RgbaImage {
            width: decoded.width(),
            height: decoded.height(),
            rgba: decoded.into_raw(),
        };
        let size = [image.width, image.height];
        let encoded = wpe_formats::tex::encode_rgba8(&image)
            .map_err(|e| Error::new("texture", e.to_string()))?;
        self.put_bytes(&tex, Some(encoded))?;
        self.put_resource(&material,Some(json!({"passes":[{"shader":"genericimage2","blending":"translucent","textures":[basename]}]})))?;
        self.put_resource(
            &descriptor,
            Some(json!({"width":size[0],"height":size[1],"autosize":true,"material":material})),
        )?;
        let layer = if add {
            self.add_layer(json!({"name":name,"image":descriptor,"origin":"0 0 0","angles":"0 0 0","scale":"1 1 1","size":format!("{} {}",size[0],size[1]),"visible":true}),None)?
        } else {
            Value::Null
        };
        Ok(
            json!({"texture":tex,"material":material,"descriptor":descriptor,"width":size[0],"height":size[1],"layer":layer}),
        )
    }
    pub(crate) fn import_source(&self, args: &[Value], index: usize) -> Result<(String, Vec<u8>)> {
        let source = arg(args, index)?;
        if let Some(object) = source.as_object() {
            let name = object
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("import")
                .to_owned();
            let bytes = object
                .get("bytes")
                .and_then(Value::as_array)
                .ok_or_else(|| Error::new("import", "bytes array required"))?
                .iter()
                .map(|b| {
                    b.as_u64()
                        .filter(|b| *b <= 255)
                        .map(|b| b as u8)
                        .ok_or_else(|| Error::new("import", "byte out of range"))
                })
                .collect::<Result<Vec<_>>>()?;
            if bytes.len() > MAX_FILE {
                return Err(Error::new("resource-limit", "file exceeds128MiB"));
            }
            Ok((name, bytes))
        } else {
            let name = string(args, index)?.to_owned();
            Ok((name.clone(), self.read_bytes(&name)?))
        }
    }
    pub(crate) fn import_obj(&mut self, name: &str, bytes: &[u8]) -> Result<Value> {
        let source =
            std::str::from_utf8(bytes).map_err(|_| Error::new("obj", "UTF-8 OBJ required"))?;
        let model = parse_obj(source)?;
        let file = format!("models/{}.modeldata.json", safe_basename(name)?);
        self.put_resource(&file, Some(model.clone()))?;
        Ok(json!({"file":file,"modeldata":model,"format":"procedural-model-json"}))
    }
}
pub(crate) fn safe_basename(name: &str) -> Result<String> {
    let name = Path::new(name)
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| Error::new("path", "filename required"))?;
    let value = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect::<String>();
    if value.is_empty() {
        return Err(Error::new("path", "filename required"));
    }
    Ok(value)
}
pub(crate) fn decode_image(bytes: &[u8]) -> Result<image::RgbaImage> {
    let reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(io_error)?;
    let mut reader = reader;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16384);
    limits.max_image_height = Some(16384);
    limits.max_alloc = Some(MAX_FILE as u64);
    reader.limits(limits);
    reader
        .decode()
        .map(|i| i.to_rgba8())
        .map_err(|e| Error::new("image", e.to_string()))
}
pub(crate) fn parse_obj(source: &str) -> Result<Value> {
    let mut positions = Vec::<[f32; 3]>::new();
    let mut vertices = Vec::<f32>::new();
    let mut indices = Vec::<u32>::new();
    for (line_number, line) in source.lines().enumerate() {
        let mut words = line.split_whitespace();
        match words.next() {
            Some("v") => {
                let values = words
                    .take(3)
                    .map(|s| {
                        s.parse::<f32>().map_err(|_| {
                            Error::new("obj", format!("invalid vertex line{}", line_number + 1))
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                if values.len() != 3 || values.iter().any(|v| !v.is_finite()) {
                    return Err(Error::new("obj", "finite xyz required"));
                }
                positions.push([values[0], values[1], values[2]]);
            }
            Some("f") => {
                let polygon = words
                    .map(|word| {
                        let value = word
                            .split('/')
                            .next()
                            .unwrap()
                            .parse::<i64>()
                            .map_err(|_| Error::new("obj", "invalid face index"))?;
                        let index = if value < 0 {
                            positions.len() as i64 + value
                        } else {
                            value - 1
                        };
                        if index < 0 || index as usize >= positions.len() {
                            return Err(Error::new("obj", "face index out of bounds"));
                        }
                        Ok(index as u32)
                    })
                    .collect::<Result<Vec<_>>>()?;
                if polygon.len() < 3 {
                    return Err(Error::new("obj", "at least3vertices per face"));
                }
                for i in 1..polygon.len() - 1 {
                    indices.extend_from_slice(&[polygon[0], polygon[i], polygon[i + 1]]);
                }
            }
            _ => {}
        }
        if positions.len() > 4_000_000 || indices.len() > 12_000_000 {
            return Err(Error::new("obj", "geometry count limit"));
        }
    }
    if indices.is_empty() {
        return Err(Error::new("obj", "no faces"));
    }
    for p in positions {
        vertices.extend_from_slice(&p)
    }
    Ok(
        json!({"shapes":[{"vertexFormat":["position"],"vertexBuffer":vertices,"indexBuffer":indices}]}),
    )
}
pub(crate) fn base64(bytes: &[u8]) -> String {
    const C: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for part in bytes.chunks(3) {
        let value = (part[0] as u32) << 16
            | (part.get(1).copied().unwrap_or(0) as u32) << 8
            | part.get(2).copied().unwrap_or(0) as u32;
        out.push(C[((value >> 18) & 63) as usize] as char);
        out.push(C[((value >> 12) & 63) as usize] as char);
        out.push(if part.len() > 1 {
            C[((value >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if part.len() > 2 {
            C[(value & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}
impl Editor {
    pub(crate) fn begin_paint(&mut self, args: &[Value]) -> Result<Value> {
        let id = arg(args, 0)?.clone();
        self.index(&id)?;
        let material = string(args, 1)?;
        let material = self.resource(material)?;
        let slot = args.get(2).and_then(Value::as_u64).unwrap_or(0) as usize;
        let basename = material["passes"]
            .as_array()
            .and_then(|a| a.first())
            .and_then(|p| p["textures"].as_array())
            .and_then(|a| a.get(slot))
            .and_then(Value::as_str)
            .ok_or_else(|| Error::new("paint", "material texture slot required"))?;
        let file = format!("materials/{basename}.tex");
        let texture = wpe_formats::tex::Texture::parse(&self.read_bytes(&file)?)
            .map_err(|e| Error::new("paint", e.to_string()))?;
        texture
            .decode_rgba8(0, 0)
            .map_err(|e| Error::new("paint", e.to_string()))?;
        self.begin_transaction()?;
        self.session["paint"] = json!({"file":file,"layer":id,"component":args.get(3).cloned().unwrap_or(json!(-1)),"options":args.get(4).cloned().unwrap_or(json!({}))});
        Ok(self.session["paint"].clone())
    }
    pub(crate) fn paint(&mut self, args: &[Value], clear: bool) -> Result<Value> {
        let session = self.session["paint"].clone();
        let file = session["file"]
            .as_str()
            .ok_or_else(|| Error::new("paint", "beginPaint required"))?;
        let texture = wpe_formats::tex::Texture::parse(&self.read_bytes(file)?)
            .map_err(|e| Error::new("paint", e.to_string()))?;
        let decoded = texture
            .decode_rgba8(0, 0)
            .map_err(|e| Error::new("paint", e.to_string()))?;
        let width = texture.header.image_width;
        let height = texture.header.image_height;
        if width == 0 || height == 0 || width > decoded.width || height > decoded.height {
            return Err(Error::new("paint", "invalid logical texture bounds"));
        }
        let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
        for row in 0..height as usize {
            let start = row * decoded.width as usize * 4;
            pixels.extend_from_slice(&decoded.rgba[start..start + width as usize * 4]);
        }
        let config = args.first().cloned().unwrap_or(json!({}));
        let options = session.get("options").unwrap_or(&Value::Null);
        let radius = config["radius"]
            .as_f64()
            .or_else(|| options["radius"].as_f64())
            .unwrap_or(8.0);
        let opacity = config["opacity"]
            .as_f64()
            .or_else(|| options["opacity"].as_f64())
            .unwrap_or(1.0);
        let hardness = config["hardness"]
            .as_f64()
            .or_else(|| options["hardness"].as_f64())
            .unwrap_or(1.0);
        if !radius.is_finite() || radius <= 0.0 || !opacity.is_finite() || !hardness.is_finite() {
            return Err(Error::new(
                "paint",
                "finite radius,opacity,hardness required",
            ));
        }
        let color = config
            .get("color")
            .or_else(|| options.get("color"))
            .cloned()
            .unwrap_or(json!([1, 1, 1, 1]));
        let color = crate::geometry::vector(&color)?;
        if color.len() != 4 {
            return Err(Error::new("paint", "RGBA brush color required"));
        }
        let x = config["x"].as_f64().unwrap_or(0.0);
        let y = config["y"].as_f64().unwrap_or(0.0);
        if !x.is_finite() || !y.is_finite() {
            return Err(Error::new("paint", "finite brush location required"));
        }
        let channel = session["component"].as_i64().unwrap_or(-1);
        if !(-1..=3).contains(&channel) {
            return Err(Error::new("paint", "component -1 or0..3 required"));
        }
        for (py, row) in pixels.chunks_exact_mut(width as usize * 4).enumerate() {
            for (px, pixel) in row.chunks_exact_mut(4).enumerate() {
                let distance =
                    ((px as f64 + 0.5 - x).powi(2) + (py as f64 + 0.5 - y).powi(2)).sqrt() / radius;
                if !clear && distance >= 1.0 {
                    continue;
                }
                let strength = if clear {
                    1.0
                } else {
                    let edge = hardness.clamp(0.0, 1.0);
                    let falloff = if distance <= edge || edge == 1.0 {
                        1.0
                    } else {
                        (1.0 - distance) / (1.0 - edge)
                    };
                    opacity.clamp(0.0, 1.0) * falloff
                };
                for c in 0..4 {
                    if channel >= 0 && c != channel as usize {
                        continue;
                    }
                    let target = if clear {
                        0.0
                    } else {
                        color[c].clamp(0.0, 1.0) * 255.0
                    };
                    pixel[c] = (pixel[c] as f64 * (1.0 - strength) + target * strength)
                        .round()
                        .clamp(0.0, 255.0) as u8;
                }
            }
        }
        let encoded = wpe_formats::tex::encode_rgba8(&wpe_formats::RgbaImage {
            width,
            height,
            rgba: pixels,
        })
        .map_err(|e| Error::new("paint", e.to_string()))?;
        self.put_bytes(file, Some(encoded))?;
        self.events.push(EditorEvent {
            kind: "resourcesChanged".into(),
            payload: json!({"files":[file]}),
        });
        Ok(json!({"file":file,"width":width,"height":height}))
    }
}

impl Editor {
    /// Unsaved project overlay, including binary edits and deletion tombstones.
    /// The host adds preview_scene() under the selected scene entry path.
    pub fn preview_resources(&self) -> Result<BTreeMap<String, Option<Arc<Vec<u8>>>>> {
        let mut resources = self.state.files.clone();
        for (name, value) in &self.state.resources {
            resources.insert(
                name.clone(),
                value
                    .as_ref()
                    .map(|v| serde_json::to_vec_pretty(v).map(Arc::new))
                    .transpose()
                    .map_err(json_error)?,
            );
        }
        Ok(resources)
    }
}
