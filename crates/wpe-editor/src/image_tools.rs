//! Owned image tools behind the observed editor argument contracts. The normal
//! kernel, diagonal-Gaussian graph cut and luminance-depth policy are explicit
//! reconstruction algorithms, not the original OpenCV/AI implementation.
use crate::*;
use image::{ImageBuffer, Luma, Rgba, RgbaImage, imageops};
use std::collections::VecDeque;

const MAX_PIXELS: u64 = 16 * 1024 * 1024;
const PREFIX: &str = "projects/temp/edit";
impl Editor {
    pub(crate) fn image_tool(&mut self, method: &str, args: &[Value]) -> Result<Value> {
        match method {
            "performGenerateNormalMap" | "performGenerateDepthMap" => {
                let source = string(args, 0)?;
                let destination = string(args, 1)?.to_owned();
                let image = self.tool_image(source)?;
                let output = if method == "performGenerateNormalMap" {
                    normal_map(&image, args)?
                } else {
                    depth_map(&image, args)?
                };
                self.store_tool_image(&destination, &output)?;
                let profile = if method == "performGenerateNormalMap" {
                    "owned-height-gradient-normal"
                } else {
                    "owned-luminance-depth; semantic AI depth is not available"
                };
                self.tool_profile(method, &destination, profile);
                Ok(json!(destination))
            }
            "updateCharacterSeparationMask" => {
                let path = string(args, 0)?.to_owned();
                let value = arg(args, 1)?;
                let bytes = if let Some(encoded) = value.as_str() {
                    if let Some(data) = encoded.strip_prefix("data:image/png;base64,") {
                        decode_base64(data)?
                    } else {
                        self.read_bytes(encoded)?
                    }
                } else {
                    byte_array(value.get("bytes").unwrap_or(value))?
                };
                let mask = crate::resources::decode_image(&bytes)?;
                bounded_image(&mask)?;
                self.store_tool_image(&path, &mask)?;
                self.session["tools"]["separationMask"] = json!({"path":path,"grabCut":args.get(2).and_then(Value::as_bool).unwrap_or(true)});
                Ok(json!(true))
            }
            "performGrabCut" => {
                let source = self.tool_image(string(args, 1)?)?;
                let mask = self.tool_image(string(args, 2)?)?;
                if source.dimensions() != mask.dimensions() {
                    return Err(Error::new(
                        "segmentation",
                        "source and mask dimensions differ",
                    ));
                }
                let foreground_path = string(args, 4)?.to_owned();
                let background_path = string(args, 3)?.to_owned();
                let refine = args.get(10).and_then(Value::as_bool).unwrap_or(true);
                let smooth = finite(args, 7, 0.)?.clamp(0., 16.);
                let quality = finite(args, 6, 1.)?.clamp(0.25, 2.);
                let bias = finite(args, 5, 0.)?.clamp(-32., 32.);
                let mut alpha = if refine {
                    graph_cut(&source, &mask, quality, bias)?
                } else {
                    ImageBuffer::from_fn(source.width(), source.height(), |x, y| {
                        let p = mask.get_pixel(x, y).0;
                        Luma([if p[0] >= 128 && p[3] > 0 { 255 } else { 0 }])
                    })
                };
                if args.get(8).and_then(Value::as_bool).unwrap_or(false) {
                    fill_holes(&mut alpha, finite(args, 9, 15.)?.clamp(0., 100.) / 100.);
                }
                if smooth > 0. {
                    alpha = imageops::blur(&alpha, smooth);
                }
                let mut foreground = source.clone();
                let mut background = source.clone();
                for (((fg, bg), mask), seed) in foreground
                    .pixels_mut()
                    .zip(background.pixels_mut())
                    .zip(alpha.pixels())
                    .zip(mask.pixels())
                {
                    let hard = if seed.0[3] == 255 {
                        Some(seed.0[0] >= 128)
                    } else {
                        None
                    };
                    let a = hard.map(|fg| if fg { 255 } else { 0 }).unwrap_or(mask.0[0]);
                    fg.0[3] = ((fg.0[3] as u16 * a as u16 + 127) / 255) as u8;
                    bg.0[3] = ((bg.0[3] as u16 * (255 - a) as u16 + 127) / 255) as u8;
                }
                // The background retains the source RGB and complementary alpha.
                self.store_tool_image(&foreground_path, &foreground)?;
                self.store_tool_image(&background_path, &background)?;
                self.session["tools"]["separationResult"] =
                    json!({"fg":foreground_path,"bg":background_path});
                self.tool_profile(method, &foreground_path, "owned-diagonal-Gaussian-min-cut");
                Ok(json!({"foreground":foreground_path,"background":background_path}))
            }
            "finishCharacterSheetCreation" => {
                let paths = arg(args, 0)?
                    .as_array()
                    .ok_or_else(|| Error::new("sheet", "image path array required"))?;
                if paths.is_empty() || paths.len() > 256 {
                    return Err(Error::new("sheet", "1..256 images required"));
                }
                let mut images = Vec::new();
                let mut area = 0u64;
                for name in paths {
                    let name = name
                        .as_str()
                        .ok_or_else(|| Error::new("sheet", "string image path required"))?;
                    let image = self.tool_image(name)?;
                    area = area
                        .checked_add((image.width() as u64 + 4) * (image.height() as u64 + 4))
                        .ok_or_else(|| Error::new("sheet", "area overflow"))?;
                    images.push((name.to_owned(), image));
                }
                if area > MAX_PIXELS {
                    return Err(Error::new("sheet", "atlas exceeds16 megapixels"));
                }
                let width = ((area as f64).sqrt().ceil() as u32)
                    .max(images.iter().map(|(_, i)| i.width() + 4).max().unwrap())
                    .next_power_of_two();
                let mut x = 2;
                let mut y = 2;
                let mut row = 0;
                let mut regions = Vec::new();
                for (name, image) in &images {
                    if x + image.width() + 2 > width {
                        x = 2;
                        y += row + 4;
                        row = 0;
                    }
                    regions.push(json!({"name":name,"x":x,"y":y,"width":image.width(),"height":image.height()}));
                    x += image.width() + 4;
                    row = row.max(image.height());
                }
                let height = (y + row + 2).next_power_of_two();
                if width > 16384 || height > 16384 || width as u64 * height as u64 > MAX_PIXELS {
                    return Err(Error::new("sheet", "atlas dimensions exceed limits"));
                }
                let mut atlas = RgbaImage::new(width, height);
                for ((_, image), rect) in images.iter().zip(&regions) {
                    imageops::replace(
                        &mut atlas,
                        image,
                        rect["x"].as_i64().unwrap(),
                        rect["y"].as_i64().unwrap(),
                    );
                }
                let destination = args
                    .get(1)
                    .and_then(Value::as_str)
                    .unwrap_or("character-sheet.png");
                self.store_tool_image(destination, &atlas)?;
                self.put_resource(
                    &format!("{destination}.json"),
                    Some(json!({"width":width,"height":height,"regions":regions})),
                )?;
                Ok(json!({"path":destination,"width":width,"height":height,"regions":regions}))
            }
            "cancelForegroundCompiler" => {
                // These bounded algorithms run synchronously; report the actual
                // idle state instead of pretending to cancel an unseen worker.
                let previous = self.session["tools"]
                    .as_object_mut()
                    .and_then(|o| o.remove("foregroundCompiler"));
                Ok(
                    json!({"cancelled":previous.is_some(),"active":false,"execution":"bounded-synchronous"}),
                )
            }
            _ => Err(Error::new(
                "image-tool",
                format!("unknown image tool {method}"),
            )),
        }
    }
    pub(crate) fn image_action(&mut self, action: &str, args: &[Value]) -> Result<Value> {
        if action == "finishCharacterSeparation" {
            let value = self.session["tools"]["separationResult"].clone();
            let path = value["fg"]
                .as_str()
                .ok_or_else(|| Error::new("separation", "no generated foreground"))?;
            return Ok(json!({"path":path,"background":value["bg"]}));
        }
        let reference = arg(args, 0)?;
        let index = args.get(1).and_then(Value::as_u64).unwrap_or(0) as usize;
        let name = self.tool_source(reference, index)?;
        let image = self.tool_image(&name)?;
        self.store_tool_image(&format!("{PREFIX}/src.png"), &image)?;
        self.store_tool_image(&format!("{PREFIX}/fg.png"), &image)?;
        self.store_tool_image(
            &format!("{PREFIX}/mask.png"),
            &RgbaImage::new(image.width(), image.height()),
        )?;
        let filename = format!(
            "generated/{}.png",
            if action == "prepareNormapMapGeneration" {
                "normal"
            } else if action == "prepareDepthMapGeneration" {
                "depth"
            } else {
                "foreground"
            }
        );
        self.session["tools"]["separationSource"] =
            json!({"reference":reference,"path":name,"filename":filename});
        Ok(
            json!({"success":true,"w":image.width(),"h":image.height(),"width":image.width(),"height":image.height(),"filename":filename}),
        )
    }
    fn tool_profile(&mut self, method: &str, path: &str, profile: &str) {
        self.events.push(EditorEvent{kind:"imageToolProfile".into(),payload:json!({"method":method,"path":path,"profile":profile,"original_algorithm_equivalence":false})});
    }
    fn tool_source(&mut self, reference: &Value, index: usize) -> Result<String> {
        let mut name = if let Ok(at) = self.index(reference) {
            self.objects()?[at]["image"]
                .as_str()
                .ok_or_else(|| Error::new("image-tool", "image layer required"))?
                .to_owned()
        } else {
            reference
                .as_str()
                .ok_or_else(|| Error::new("image-tool", "layer id or resource path required"))?
                .to_owned()
        };
        for _ in 0..4 {
            if !name.to_ascii_lowercase().ends_with(".json") {
                return Ok(name);
            }
            let value = self.resource(&name)?;
            if let Some(material) = value.get("material").and_then(Value::as_str) {
                name = material.into();
                continue;
            }
            let texture = value
                .pointer("/passes/0/textures")
                .and_then(Value::as_array)
                .and_then(|a| a.get(index))
                .and_then(Value::as_str)
                .ok_or_else(|| Error::new("image-tool", "material texture slot missing"))?;
            name = if texture.ends_with(".tex") || texture.ends_with(".png") {
                texture.into()
            } else {
                format!("materials/{texture}.tex")
            };
        }
        Err(Error::new("image-tool", "resource indirection exceeds4"))
    }
    fn tool_image(&self, name: &str) -> Result<RgbaImage> {
        let bytes = self.read_bytes(name)?;
        let image = if bytes.get(4..8) == Some(b"TEXV") || bytes.starts_with(b"TEXV") {
            let decoded = wpe_formats::tex::Texture::parse(&bytes)
                .and_then(|t| t.decode_rgba8(0, 0))
                .map_err(|e| Error::new("texture", e.to_string()))?;
            RgbaImage::from_raw(decoded.width, decoded.height, decoded.rgba)
                .ok_or_else(|| Error::new("texture", "invalid image dimensions"))?
        } else {
            crate::resources::decode_image(&bytes)?
        };
        bounded_image(&image)?;
        Ok(image)
    }
    fn store_tool_image(&mut self, name: &str, image: &RgbaImage) -> Result<()> {
        let bytes = if name.to_ascii_lowercase().ends_with(".tex") {
            wpe_formats::tex::encode_rgba8(&wpe_formats::RgbaImage {
                width: image.width(),
                height: image.height(),
                rgba: image.as_raw().clone(),
            })
            .map_err(|e| Error::new("texture", e.to_string()))?
        } else {
            let mut cursor = std::io::Cursor::new(Vec::new());
            image
                .write_to(&mut cursor, image::ImageFormat::Png)
                .map_err(|e| Error::new("image", e.to_string()))?;
            cursor.into_inner()
        };
        self.put_bytes(name, Some(bytes))
    }
}
fn bounded_image(image: &RgbaImage) -> Result<()> {
    if image.width() == 0
        || image.height() == 0
        || image.width() as u64 * image.height() as u64 > MAX_PIXELS
    {
        Err(Error::new(
            "image-limit",
            "image tool requires1..16 megapixels",
        ))
    } else {
        Ok(())
    }
}
fn finite(args: &[Value], index: usize, default: f32) -> Result<f32> {
    match args.get(index) {
        None | Some(Value::Null) => Ok(default),
        Some(v) => v
            .as_f64()
            .filter(|v| v.is_finite() && (*v as f32).is_finite())
            .map(|v| v as f32)
            .ok_or_else(|| {
                Error::new(
                    "image-option",
                    format!("finite numeric argument{index} required"),
                )
            }),
    }
}
fn luminance(image: &RgbaImage) -> ImageBuffer<Luma<f32>, Vec<f32>> {
    ImageBuffer::from_fn(image.width(), image.height(), |x, y| {
        let p = image.get_pixel(x, y).0;
        Luma([(p[0] as f32 * 0.2126 + p[1] as f32 * 0.7152 + p[2] as f32 * 0.0722) / 255.])
    })
}
fn normal_map(image: &RgbaImage, args: &[Value]) -> Result<RgbaImage> {
    let filter = finite(args, 2, 13.)? as u32;
    let blur = finite(args, 3, 1.)?.clamp(0., 127.);
    let depth = finite(args, 4, 1.)?.clamp(-1000., 1000.);
    let threshold = finite(args, 5, 1.)?.max(0.0001);
    let details_blur = finite(args, 6, 0.)?.clamp(0., 127.);
    let details_depth = finite(args, 7, 0.)?.clamp(-1000., 1000.);
    let sign = if args.get(8).and_then(Value::as_bool).unwrap_or(false) {
        -1.
    } else {
        1.
    };
    let exponent = finite(args, 9, 1.)?.clamp(0.01, 32.);
    let original = luminance(image);
    let base = if blur > 1. {
        imageops::blur(&original, blur / 6.)
    } else {
        original.clone()
    };
    let details = if details_blur > 1. {
        imageops::blur(&original, details_blur / 6.)
    } else {
        original.clone()
    };
    let height = ImageBuffer::from_fn(image.width(), image.height(), |x, y| {
        Luma([base.get_pixel(x, y).0[0].powf(exponent) * depth
            + (original.get_pixel(x, y).0[0] - details.get_pixel(x, y).0[0])
                * details_depth
                * sign])
    });
    let sample = |x: i64, y: i64| {
        height
            .get_pixel(
                x.clamp(0, image.width() as i64 - 1) as u32,
                y.clamp(0, image.height() as i64 - 1) as u32,
            )
            .0[0]
    };
    Ok(ImageBuffer::from_fn(
        image.width(),
        image.height(),
        |x, y| {
            let (x, y) = (x as i64, y as i64);
            let (dx, dy) = if filter == 0 {
                (
                    (sample(x + 1, y) - sample(x - 1, y)) * 0.5,
                    (sample(x, y + 1) - sample(x, y - 1)) * 0.5,
                )
            } else {
                (
                    (sample(x + 1, y - 1) + 2. * sample(x + 1, y) + sample(x + 1, y + 1)
                        - sample(x - 1, y - 1)
                        - 2. * sample(x - 1, y)
                        - sample(x - 1, y + 1))
                        * 0.125,
                    (sample(x - 1, y + 1) + 2. * sample(x, y + 1) + sample(x + 1, y + 1)
                        - sample(x - 1, y - 1)
                        - 2. * sample(x, y - 1)
                        - sample(x + 1, y - 1))
                        * 0.125,
                )
            };
            let nx = -dx / threshold;
            let ny = dy / threshold;
            let length = (nx * nx + ny * ny + 1.).sqrt();
            Rgba([
                channel(nx / length * 0.5 + 0.5),
                channel(ny / length * 0.5 + 0.5),
                channel(1. / length * 0.5 + 0.5),
                image.get_pixel(x as u32, y as u32).0[3],
            ])
        },
    ))
}
fn depth_map(image: &RgbaImage, args: &[Value]) -> Result<RgbaImage> {
    let blur = finite(args, 2, 1.)?.clamp(0., 64.);
    let outline = finite(args, 3, 0.)?.clamp(0., 32.);
    let auto = args.get(4).and_then(Value::as_bool).unwrap_or(true);
    let invert = args.get(5).and_then(Value::as_bool).unwrap_or(false);
    let quality = finite(args, 6, 1.)?.clamp(0., 2.);
    let extent = if quality < 0.5 {
        512.
    } else if quality < 1.5 {
        1024.
    } else {
        2048.
    };
    let scale = (extent / image.width().max(image.height()) as f32).min(1.);
    let working = imageops::resize(
        image,
        (image.width() as f32 * scale).round().max(1.) as u32,
        (image.height() as f32 * scale).round().max(1.) as u32,
        imageops::FilterType::Triangle,
    );
    let mut depth = luminance(&working);
    if blur > 0. {
        depth = imageops::blur(&depth, blur);
    }
    depth = imageops::resize(
        &depth,
        image.width(),
        image.height(),
        imageops::FilterType::Triangle,
    );
    let (min, max) = if auto {
        depth
            .pixels()
            .zip(image.pixels())
            .filter(|(_, p)| p.0[3] > 0)
            .fold((1f32, 0f32), |(min, max), (p, _)| {
                (min.min(p.0[0]), max.max(p.0[0]))
            })
    } else {
        (0., 1.)
    };
    let range = (max - min).max(1. / 255.);
    Ok(ImageBuffer::from_fn(
        image.width(),
        image.height(),
        |x, y| {
            let mut value = ((depth.get_pixel(x, y).0[0] - min) / range).clamp(0., 1.);
            // Alpha compensation limits bright halos around transparent silhouettes.
            value *= (image.get_pixel(x, y).0[3] as f32 / 255.).powf(outline * 0.25);
            if invert {
                value = 1. - value;
            }
            let v = channel(value);
            Rgba([v, v, v, image.get_pixel(x, y).0[3]])
        },
    ))
}
fn channel(v: f32) -> u8 {
    (v.clamp(0., 1.) * 255.).round() as u8
}
fn byte_array(value: &Value) -> Result<Vec<u8>> {
    let values = value
        .as_array()
        .ok_or_else(|| Error::new("image-bytes", "byte array required"))?;
    if values.len() > 128 * 1024 * 1024 {
        return Err(Error::new("image-limit", "encoded image exceeds128MiB"));
    }
    values
        .iter()
        .map(|v| {
            v.as_u64()
                .filter(|v| *v <= 255)
                .map(|v| v as u8)
                .ok_or_else(|| Error::new("image-bytes", "byte out of range"))
        })
        .collect()
}
fn decode_base64(source: &str) -> Result<Vec<u8>> {
    if source.len() > 176 * 1024 * 1024 {
        return Err(Error::new("image-limit", "base64 image too large"));
    }
    let mut out = Vec::with_capacity(source.len() * 3 / 4);
    let mut bits = 0u32;
    let mut count = 0;
    let mut ended = false;
    for b in source.bytes().filter(|b| !b.is_ascii_whitespace()) {
        if b == b'=' {
            ended = true;
            continue;
        }
        if ended {
            return Err(Error::new("base64", "data follows padding"));
        }
        let n = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return Err(Error::new("base64", "invalid encoded image")),
        };
        bits = (bits << 6) | n as u32;
        count += 6;
        if count >= 8 {
            count -= 8;
            out.push((bits >> count) as u8);
        }
    }
    if count >= 6 || bits & ((1 << count) - 1) != 0 {
        return Err(Error::new("base64", "invalid trailing bits"));
    }
    Ok(out)
}

#[derive(Clone, Copy)]
struct Edge {
    to: usize,
    reverse: usize,
    capacity: f32,
}
struct Flow {
    edges: Vec<Vec<Edge>>,
    level: Vec<i32>,
    cursor: Vec<usize>,
}
impl Flow {
    fn new(n: usize) -> Self {
        Self {
            edges: vec![Vec::new(); n],
            level: vec![-1; n],
            cursor: vec![0; n],
        }
    }
    fn edge(&mut self, a: usize, b: usize, capacity: f32) {
        let ar = self.edges[a].len();
        let br = self.edges[b].len();
        self.edges[a].push(Edge {
            to: b,
            reverse: br,
            capacity,
        });
        self.edges[b].push(Edge {
            to: a,
            reverse: ar,
            capacity: 0.,
        });
    }
    fn cut(&mut self, source: usize, sink: usize) -> Vec<bool> {
        loop {
            self.level.fill(-1);
            self.level[source] = 0;
            let mut queue = VecDeque::from([source]);
            while let Some(u) = queue.pop_front() {
                for edge in &self.edges[u] {
                    if edge.capacity > 1e-5 && self.level[edge.to] < 0 {
                        self.level[edge.to] = self.level[u] + 1;
                        queue.push_back(edge.to);
                    }
                }
            }
            if self.level[sink] < 0 {
                break;
            }
            self.cursor.fill(0);
            loop {
                let mut nodes = vec![source];
                let mut path = Vec::<(usize, usize)>::new();
                let mut found = false;
                while let Some(&u) = nodes.last() {
                    if u == sink {
                        found = true;
                        break;
                    }
                    while self.cursor[u] < self.edges[u].len() {
                        let e = self.edges[u][self.cursor[u]];
                        if e.capacity > 1e-5 && self.level[e.to] == self.level[u] + 1 {
                            break;
                        }
                        self.cursor[u] += 1;
                    }
                    if self.cursor[u] == self.edges[u].len() {
                        self.level[u] = -1;
                        nodes.pop();
                        if let Some((p, _)) = path.pop() {
                            self.cursor[p] += 1;
                        }
                    } else {
                        let index = self.cursor[u];
                        path.push((u, index));
                        nodes.push(self.edges[u][index].to);
                    }
                }
                if !found {
                    break;
                }
                let amount = path
                    .iter()
                    .map(|(u, i)| self.edges[*u][*i].capacity)
                    .fold(f32::INFINITY, f32::min);
                for (u, i) in path {
                    let edge = self.edges[u][i];
                    self.edges[u][i].capacity -= amount;
                    self.edges[edge.to][edge.reverse].capacity += amount;
                }
            }
        }
        let mut seen = vec![false; self.edges.len()];
        seen[source] = true;
        let mut queue = VecDeque::from([source]);
        while let Some(u) = queue.pop_front() {
            for edge in &self.edges[u] {
                if edge.capacity > 1e-5 && !seen[edge.to] {
                    seen[edge.to] = true;
                    queue.push_back(edge.to);
                }
            }
        }
        seen
    }
}
#[derive(Clone, Copy)]
struct Gaussian {
    mean: [f32; 3],
    variance: [f32; 3],
    weight: f32,
}
fn models(colors: &[[f32; 3]], labels: &[bool], class: bool) -> Vec<Gaussian> {
    let indices = labels
        .iter()
        .enumerate()
        .filter_map(|(i, l)| (*l == class).then_some(i))
        .collect::<Vec<_>>();
    if indices.is_empty() {
        return vec![Gaussian {
            mean: [0.5; 3],
            variance: [0.25; 3],
            weight: 1.,
        }];
    }
    let count = 5.min(indices.len());
    let mut centers = (0..count)
        .map(|k| colors[indices[k * indices.len() / count]])
        .collect::<Vec<_>>();
    let mut assigned = vec![0; indices.len()];
    for _ in 0..5 {
        let mut sums = vec![[0f32; 3]; count];
        let mut counts = vec![0usize; count];
        for (at, &i) in indices.iter().enumerate() {
            let best = (0..count)
                .min_by(|&a, &b| {
                    distance(colors[i], centers[a]).total_cmp(&distance(colors[i], centers[b]))
                })
                .unwrap();
            assigned[at] = best;
            counts[best] += 1;
            for c in 0..3 {
                sums[best][c] += colors[i][c];
            }
        }
        for k in 0..count {
            if counts[k] > 0 {
                for c in 0..3 {
                    centers[k][c] = sums[k][c] / counts[k] as f32;
                }
            }
        }
    }
    let mut result = Vec::new();
    for (k, &mean) in centers.iter().enumerate() {
        let mut variance = [0f32; 3];
        let mut n = 0;
        for (at, &i) in indices.iter().enumerate() {
            if assigned[at] == k {
                n += 1;
                for c in 0..3 {
                    variance[c] += (colors[i][c] - mean[c]).powi(2);
                }
            }
        }
        if n > 0 {
            for v in &mut variance {
                *v = (*v / n as f32).max(0.0004);
            }
            result.push(Gaussian {
                mean,
                variance,
                weight: n as f32 / indices.len() as f32,
            });
        }
    }
    result
}
fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    (0..3).map(|c| (a[c] - b[c]).powi(2)).sum()
}
fn cost(color: [f32; 3], models: &[Gaussian]) -> f32 {
    models
        .iter()
        .map(|m| {
            0.5 * (0..3)
                .map(|c| (color[c] - m.mean[c]).powi(2) / m.variance[c] + m.variance[c].ln())
                .sum::<f32>()
                - m.weight.ln()
        })
        .fold(f32::INFINITY, f32::min)
}
fn graph_cut(
    image: &RgbaImage,
    mask: &RgbaImage,
    quality: f32,
    bias: f32,
) -> Result<ImageBuffer<Luma<u8>, Vec<u8>>> {
    let extent = (192. * quality).round().clamp(64., 384.) as u32;
    let scale = (extent as f32 / image.width().max(image.height()) as f32).min(1.);
    let w = (image.width() as f32 * scale).round().max(1.) as u32;
    let h = (image.height() as f32 * scale).round().max(1.) as u32;
    let small = imageops::resize(image, w, h, imageops::FilterType::Triangle);
    let seeds = imageops::resize(mask, w, h, imageops::FilterType::Nearest);
    let colors = small
        .pixels()
        .map(|p| {
            [
                p.0[0] as f32 / 255.,
                p.0[1] as f32 / 255.,
                p.0[2] as f32 / 255.,
            ]
        })
        .collect::<Vec<_>>();
    let mut hard = seeds
        .pixels()
        .map(|p| {
            if p.0[3] == 255 {
                Some(p.0[0] >= 128)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    // A small painted seed must survive analysis downsampling.
    let mut seed_counts = vec![[0usize; 2]; colors.len()];
    for (x, y, p) in mask.enumerate_pixels() {
        if p.0[3] == 255 {
            let xx = (x as u64 * w as u64 / image.width() as u64).min(w as u64 - 1) as u32;
            let yy = (y as u64 * h as u64 / image.height() as u64).min(h as u64 - 1) as u32;
            seed_counts[(yy * w + xx) as usize][usize::from(p.0[0] >= 128)] += 1;
        }
    }
    for (hard, count) in hard.iter_mut().zip(seed_counts) {
        if count[0] + count[1] > 0 {
            *hard = Some(count[1] >= count[0]);
        }
    }
    let mut labels = (0..colors.len())
        .map(|i| {
            hard[i].unwrap_or_else(|| {
                let x = i as u32 % w;
                let y = i as u32 / w;
                x > w / 10 && x < w * 9 / 10 && y > h / 10 && y < h * 9 / 10
            })
        })
        .collect::<Vec<_>>();
    if !hard.contains(&Some(true)) {
        return Err(Error::new(
            "segmentation",
            "paint at least one foreground seed",
        ));
    }
    let mut pairs = Vec::new();
    let mut total = 0.;
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) as usize;
            for (dx, dy) in [(1, 0), (0, 1), (1, 1), (-1, 1)] {
                let xx = x as i32 + dx;
                let yy = y as i32 + dy;
                if xx >= 0 && xx < w as i32 && yy < h as i32 {
                    let j = (yy as u32 * w + xx as u32) as usize;
                    let d = distance(colors[i], colors[j]);
                    total += d;
                    pairs.push((
                        i,
                        j,
                        d,
                        if dx != 0 && dy != 0 {
                            std::f32::consts::SQRT_2
                        } else {
                            1.
                        },
                    ));
                }
            }
        }
    }
    let beta = if total > 1e-8 {
        pairs.len() as f32 / (2. * total)
    } else {
        0.
    };
    for _ in 0..3 {
        let fg = models(&colors, &labels, true);
        let bg = models(&colors, &labels, false);
        let n = colors.len();
        let mut flow = Flow::new(n + 2);
        for i in 0..n {
            let (f, b) = match hard[i] {
                Some(true) => (0., 10000.),
                Some(false) => (10000., 0.),
                None => {
                    let f = cost(colors[i], &fg) - bias;
                    let b = cost(colors[i], &bg);
                    let min = f.min(b);
                    ((f - min).min(100.), (b - min).min(100.))
                }
            };
            flow.edge(n, i, b);
            flow.edge(i, n + 1, f);
        }
        for &(i, j, d, diagonal) in &pairs {
            let value = 35. * (-beta * d).exp() / diagonal;
            flow.edge(i, j, value);
            flow.edge(j, i, value);
        }
        let cut = flow.cut(n, n + 1);
        labels.clone_from_slice(&cut[..n]);
    }
    let result = ImageBuffer::from_fn(w, h, |x, y| {
        Luma([if labels[(y * w + x) as usize] { 255 } else { 0 }])
    });
    Ok(imageops::resize(
        &result,
        image.width(),
        image.height(),
        imageops::FilterType::Triangle,
    ))
}
fn fill_holes(mask: &mut ImageBuffer<Luma<u8>, Vec<u8>>, fraction: f32) {
    let (w, h) = mask.dimensions();
    let mut seen = vec![false; (w * h) as usize];
    let maximum = (w as f32 * h as f32 * fraction) as usize;
    for y in 0..h {
        for x in 0..w {
            let start = (y * w + x) as usize;
            if seen[start] || mask.get_pixel(x, y).0[0] >= 128 {
                continue;
            }
            let mut queue = VecDeque::from([(x, y)]);
            let mut region = Vec::new();
            let mut border = false;
            seen[start] = true;
            while let Some((x, y)) = queue.pop_front() {
                region.push((x, y));
                border |= x == 0 || y == 0 || x == w - 1 || y == h - 1;
                for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                    let xx = x as i32 + dx;
                    let yy = y as i32 + dy;
                    if xx >= 0 && yy >= 0 && xx < w as i32 && yy < h as i32 {
                        let xx = xx as u32;
                        let yy = yy as u32;
                        let i = (yy * w + xx) as usize;
                        if !seen[i] && mask.get_pixel(xx, yy).0[0] < 128 {
                            seen[i] = true;
                            queue.push_back((xx, yy));
                        }
                    }
                }
            }
            if !border && region.len() <= maximum {
                for (x, y) in region {
                    mask.put_pixel(x, y, Luma([255]));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normal_gradient_flat_alpha_and_depth_contrast_are_actual_image_results() {
        let flat = RgbaImage::from_pixel(5, 5, Rgba([90, 90, 90, 128]));
        let normal = normal_map(&flat, &[Value::Null, Value::Null]).unwrap();
        assert_eq!(normal.get_pixel(2, 2).0, [128, 128, 255, 128]);
        let ramp = ImageBuffer::from_fn(8, 5, |x, _| {
            Rgba([(x * 30) as u8, (x * 30) as u8, (x * 30) as u8, 255])
        });
        assert!(
            normal_map(&ramp, &[Value::Null, Value::Null])
                .unwrap()
                .get_pixel(4, 2)
                .0[0]
                < 128
        );
        let depth = depth_map(
            &ramp,
            &[
                Value::Null,
                Value::Null,
                json!(0),
                json!(0),
                json!(true),
                json!(true),
            ],
        )
        .unwrap();
        assert_eq!(depth.get_pixel(0, 2).0, [255; 4]);
        assert_eq!(depth.get_pixel(7, 2).0, [0, 0, 0, 255]);
    }
    #[test]
    fn min_cut_respects_seed_colors_and_splits_unpainted_regions() {
        let image = ImageBuffer::from_fn(24, 16, |x, _| {
            if x < 12 {
                Rgba([230, 15, 10, 255])
            } else {
                Rgba([10, 20, 230, 255])
            }
        });
        let mut seeds = RgbaImage::new(24, 16);
        seeds.put_pixel(2, 8, Rgba([255; 4]));
        seeds.put_pixel(21, 8, Rgba([0, 0, 0, 255]));
        let alpha = graph_cut(&image, &seeds, 1., 0.).unwrap();
        assert_eq!(alpha.get_pixel(5, 8).0[0], 255);
        assert_eq!(alpha.get_pixel(18, 8).0[0], 0);
        assert!(graph_cut(&image, &RgbaImage::new(24, 16), 1., 0.).is_err());
    }
    #[test]
    fn staged_maps_segmentation_sheet_and_undo_are_resource_transactions() {
        let root = std::env::temp_dir().join(format!("wpe-image-tools-{}", std::process::id()));
        let mut editor = Editor::new(
            root,
            json!({"type":"scene","file":"scene.json"}),
            json!({"objects":[]}),
        )
        .unwrap();
        let image = ImageBuffer::from_fn(12, 8, |x, _| {
            if x < 6 {
                Rgba([255, 0, 0, 255])
            } else {
                Rgba([0, 0, 255, 255])
            }
        });
        editor.store_tool_image("source.png", &image).unwrap();
        let mut mask = RgbaImage::new(12, 8);
        mask.put_pixel(1, 4, Rgba([255; 4]));
        mask.put_pixel(10, 4, Rgba([0, 0, 0, 255]));
        let mut cursor = std::io::Cursor::new(Vec::new());
        mask.write_to(&mut cursor, image::ImageFormat::Png).unwrap();
        let encoded = format!(
            "data:image/png;base64,{}",
            crate::resources::base64(cursor.get_ref())
        );
        editor
            .execute(
                "updateCharacterSeparationMask",
                json!(["mask.png", encoded, true]),
            )
            .unwrap();
        let result = editor
            .execute(
                "performGrabCut",
                json!([
                    true,
                    "source.png",
                    "mask.png",
                    "bg.png",
                    "fg.png",
                    0,
                    1,
                    0,
                    false,
                    0,
                    true
                ]),
            )
            .unwrap();
        assert_eq!(result.value["foreground"], "fg.png");
        let fg = editor.tool_image("fg.png").unwrap();
        assert_eq!(fg.get_pixel(2, 4).0[3], 255);
        assert_eq!(fg.get_pixel(9, 4).0[3], 0);
        assert!(editor.undo().unwrap());
        assert!(editor.read_bytes("fg.png").is_err());
        assert!(editor.redo().unwrap());
        let sheet = editor
            .execute(
                "finishCharacterSheetCreation",
                json!([["fg.png", "bg.png"], "sheet.png"]),
            )
            .unwrap();
        assert_eq!(sheet.value["regions"].as_array().unwrap().len(), 2);
        assert_eq!(
            editor.tool_image("sheet.png").unwrap().width(),
            sheet.value["width"].as_u64().unwrap() as u32
        );
        let before = editor.snapshot().revision;
        assert!(
            editor
                .execute(
                    "performGenerateNormalMap",
                    json!(["source.png", "../bad.png"])
                )
                .is_err()
        );
        assert_eq!(before, editor.snapshot().revision);
        assert!(editor.pending_host_requests().is_empty());
    }
}
