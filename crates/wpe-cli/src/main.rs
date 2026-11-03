use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::error::Error;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use wpe_formats::{RgbaImage, mdl::Model, pkg::Package, tex::Texture};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

const HELP: &str = "Wallpaper reconstruction tools

  wpe inspect PATH [--assets DIR]
  wpe render PROJECT --output FILE.png [--assets DIR] [--width N --height N]
  wpe frame PROJECT [--time SECONDS --fps N --frames N] [--output FILE_OR_DIRECTORY] [--assets DIR]
  wpe simulate PROJECT [--time SECONDS] [--output REPORT.json]
  wpe demo EMPTY_DIRECTORY
  wpe corpus DIRECTORY... [--output REPORT.json]
  wpe corpus FILE_INVENTORY.json [--output REPORT.json]

inspect reads PKG/TEX/MDL/FBX metadata or a project and its compatibility issues.
render preserves the initial static CPU preview.
frame runs the complete mutable scene pipeline and renders portable CPU programs.
simulate validates complete render graphs and translated shaders without a GPU.
demo creates original generated textures, a scene project and a local web demo.
corpus validates observed binary containers, including TEX/MDL package members.
The native macOS player is the separate wallpaper-macos executable.
";

#[derive(Default)]
struct Args {
    command: String,
    paths: Vec<PathBuf>,
    assets: Option<PathBuf>,
    output: Option<PathBuf>,
    width: Option<u32>,
    height: Option<u32>,
    time: f64,
    fps: Option<u32>,
    frames: Option<u32>,
}

fn parse_args() -> Result<Args> {
    let mut values = std::env::args_os().skip(1);
    let command = values.next().unwrap_or_else(|| OsString::from("help"));
    let command = command.to_str().ok_or("Command must be UTF-8")?.to_owned();
    let mut args = Args {
        command,
        ..Args::default()
    };
    while let Some(arg) = values.next() {
        match arg.to_str() {
            Some("--assets") => {
                args.assets = Some(values.next().ok_or("--assets requires a directory")?.into())
            }
            Some("--output" | "-o") => {
                args.output = Some(values.next().ok_or("--output requires a file")?.into())
            }
            Some("--time") => {
                args.time = values
                    .next()
                    .ok_or("--time requires seconds")?
                    .to_str()
                    .ok_or("seconds must be UTF-8")?
                    .parse()?;
                if !args.time.is_finite() || !(0.0..=86400.0).contains(&args.time) {
                    return Err("--time must be finite and between0 and86400".into());
                }
            }
            Some("--fps") => args.fps = Some(parse_dimension(values.next(), "--fps")?.min(240)),
            Some("--frames") => args.frames = Some(parse_dimension(values.next(), "--frames")?),
            Some("--width") => args.width = Some(parse_dimension(values.next(), "--width")?),
            Some("--height") => args.height = Some(parse_dimension(values.next(), "--height")?),
            Some("--") => {
                args.paths.extend(values.map(PathBuf::from));
                break;
            }
            Some(text) if text.starts_with('-') => {
                return Err(format!("Unknown option: {text}").into());
            }
            _ => args.paths.push(arg.into()),
        }
    }
    Ok(args)
}

fn parse_dimension(value: Option<OsString>, option: &str) -> Result<u32> {
    let value = value.ok_or_else(|| format!("{option} requires a number"))?;
    let number: u32 = value
        .to_str()
        .ok_or("Dimension must be a number")?
        .parse()?;
    if number == 0 || number > 16384 {
        return Err("Dimension must be between 1 and 16384".into());
    }
    Ok(number)
}

fn one_path(args: &Args) -> Result<&Path> {
    if args.paths.len() != 1 {
        return Err(format!("{} requires exactly one input path", args.command).into());
    }
    Ok(&args.paths[0])
}

fn extension(path: &Path) -> String {
    path.extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

fn emit(value: &Value, output: Option<&Path>) -> Result<()> {
    let text = serde_json::to_string_pretty(value)? + "\n";
    if let Some(path) = output {
        fs::write(path, text)?;
    } else {
        print!("{text}");
    }
    Ok(())
}

fn inspect(args: &Args) -> Result<()> {
    let path = one_path(args)?;
    let value = match extension(path).as_str() {
        "pkg" => {
            let package = Package::parse(fs::read(path)?)?;
            let members: Vec<_> = package
                .members()
                .iter()
                .map(|m| json!({"name":m.name,"offset":m.offset,"size":m.size}))
                .collect();
            json!({"kind":"package","version":package.version,"members":members})
        }
        "tex" => Texture::parse(&fs::read(path)?)?.summary(),
        "mdl" => Model::parse(&fs::read(path)?)?.summary(),
        "fbx" => {
            let model = wpe_formats::fbx::decode_to_editable(&fs::read(path)?)?;
            json!({"kind":"fbx","profile":model.get("profile"),
                "shapes":model.get("shapes").and_then(Value::as_array).map(Vec::len).unwrap_or(0),
                "bones":model.get("bones").and_then(Value::as_array).map(Vec::len).unwrap_or(0),
                "clips":model.get("clips").and_then(Value::as_array).map(|clips|clips.iter().map(|clip|
                    json!({"name":clip.get("name"),"fps":clip.get("fps"),"frames":clip.get("frames")})).collect::<Vec<_>>()),
                "source_metadata":model.get("nativeMetadata")})
        }
        _ => {
            let project = wpe_core::load_project(path, args.assets.as_deref())?;
            let mut value = json!({"kind":format!("{:?}",project.kind),"title":project.title,"root":project.root,"entry":project.entry});
            if let wpe_core::WallpaperKind::Scene = project.kind {
                let scene = wpe_core::load_scene(&project)?;
                value["scene"] = json!({
                    "width":scene.width,"height":scene.height,"clear_color":scene.clear_color,
                    "quads":scene.quads.iter().map(|q|json!({"name":q.name,"texture_width":q.image.width,"texture_height":q.image.height,"vertices":q.vertices.iter().map(|v|json!({"position":v.position,"uv":v.uv})).collect::<Vec<_>>(),"color":q.color})).collect::<Vec<_>>(),
                    "issues":scene.issues,
                    "profile":"static unlit images; declared orthographic projection and source-over array order",
                });
            }
            value
        }
    };
    emit(&value, args.output.as_deref())
}

fn save_png(image: &RgbaImage, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    image::save_buffer_with_format(
        path,
        &image.rgba,
        image.width,
        image.height,
        image::ColorType::Rgba8,
        image::ImageFormat::Png,
    )?;
    Ok(())
}

fn render(args: &Args) -> Result<()> {
    let project = wpe_core::load_project(one_path(args)?, args.assets.as_deref())?;
    let plan = wpe_core::load_scene(&project)?;
    let width = args.width.unwrap_or(plan.width.ceil() as u32);
    let height = args.height.unwrap_or(plan.height.ceil() as u32);
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > 64 * 1024 * 1024 {
        return Err(
            "Reference image dimensions must be positive and at most 64 million pixels".into(),
        );
    }
    let rgba = wpe_core::render_software(&plan, width, height)?;
    let output = args
        .output
        .as_deref()
        .unwrap_or_else(|| Path::new("render.png"));
    save_png(&rgba, output)?;
    emit(
        &json!({"output":output,"width":width,"height":height,"quads":plan.quads.len(),"issues":plan.issues,"oracle":"new CPU implementation; not an original-engine frame comparison"}),
        None,
    )
}

fn playback(args: &Args, render_pixels: bool) -> Result<()> {
    let project = wpe_core::load_project(one_path(args)?, args.assets.as_deref())?;
    let mut player = wpe_core::ScenePlayer::new(project)?;
    let input = wpe_core::PlayerInput::default();
    let fps = args.fps.unwrap_or(60);
    let dt = 1.0 / fps as f32;
    let mut remaining = args.time;
    while remaining > 0.0 {
        let delta = remaining.min(dt as f64);
        player.advance(delta as f32, &input)?;
        remaining -= delta;
    }
    let frames = args.frames.unwrap_or(1);
    let mut renderer = wpe_render::cpu::Renderer::new();
    let mut reports = Vec::new();
    let destination = args.output.clone().unwrap_or_else(|| {
        PathBuf::from(if render_pixels {
            if frames == 1 { "frame.png" } else { "frames" }
        } else {
            "simulation.json"
        })
    });
    if render_pixels && frames > 1 {
        fs::create_dir_all(&destination)?;
    }
    for index in 0..frames {
        let frame = player.advance(if index == 0 { 0.0 } else { dt }, &input)?;
        let mut zero_initialized_uniforms = BTreeSet::new();
        for draw in &frame.draws {
            for stage in [&draw.material.vertex, &draw.material.fragment]
                .into_iter()
                .flatten()
            {
                stage
                    .pack_uniforms(&draw.material.uniforms.values)
                    .map_err(|error| {
                        format!(
                            "draw {} shader {} uniform packing: {error}",
                            draw.name, stage.name
                        )
                    })?;
                for member in &stage.uniforms {
                    if !draw.material.uniforms.values.contains_key(&member.name)
                        && !member
                            .aliases
                            .iter()
                            .any(|name| draw.material.uniforms.values.contains_key(name))
                        && !stage.default_uniforms.contains_key(&member.name)
                    {
                        zero_initialized_uniforms.insert((
                            stage.name.clone(),
                            format!("{:?}", stage.stage),
                            member.name.clone(),
                        ));
                    }
                }
            }
        }
        let output = if render_pixels {
            let image = renderer.render(&frame)?;
            let output = if frames == 1 {
                destination.clone()
            } else {
                destination.join(format!("frame-{index:05}.png"))
            };
            save_png(&image, &output)?;
            Some(output)
        } else {
            None
        };
        let zero_initialized_uniforms = zero_initialized_uniforms
            .into_iter()
            .map(|(shader, stage, name)| json!({"shader":shader,"stage":stage,"name":name}))
            .collect::<Vec<_>>();
        reports.push(json!({"time":player.time(),"width":frame.width,"height":frame.height,"draws":frame.draws.len(),"triangles":frame.draws.iter().map(|d|d.indices.len()/3).sum::<usize>(),"textures":frame.textures.len(),"passes":frame.passes.len(),"translated_shader_draws":frame.draws.iter().filter(|d|d.material.vertex.is_some()).count(),"issues":frame.issues,"zero_initialized_reflected_uniforms":zero_initialized_uniforms,"output":output}));
    }
    let report = json!({"frames":reports,"storage":player.storage(),"provider_commands":player.drain_host_commands(),"oracle":"new implementation; original-engine pixel/behavior equivalence is not established"});
    emit(
        &report,
        if render_pixels {
            None
        } else {
            Some(&destination)
        },
    )
}

fn write_json(path: &Path, value: &Value) -> Result<()> {
    fs::write(path, serde_json::to_string_pretty(value)? + "\n")?;
    Ok(())
}

fn demo(args: &Args) -> Result<()> {
    let root = one_path(args)?;
    if root.exists() && fs::read_dir(root)?.next().is_some() {
        return Err("Demo destination must be empty".into());
    }
    for directory in ["models", "materials", "web"] {
        fs::create_dir_all(root.join(directory))?;
    }
    for (name, width, height, foreground) in [
        ("demo.gradient", 240, 135, false),
        ("demo.card", 256, 144, true),
    ] {
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                let u = x as f32 / (width - 1) as f32;
                let v = y as f32 / (height - 1) as f32;
                let color = if foreground {
                    let corner_x =
                        (x as f32 - width as f32 * 0.5).abs() - (width as f32 * 0.5 - 18.0);
                    let corner_y =
                        (y as f32 - height as f32 * 0.5).abs() - (height as f32 * 0.5 - 18.0);
                    let inside = corner_x.max(0.0).hypot(corner_y.max(0.0)) < 18.0;
                    let stripe = ((u * 7.0 + v * 2.0).sin() * 0.5 + 0.5) * 25.0;
                    [
                        ((40.0 + 180.0 * u) + stripe) as u8,
                        (170.0 - 60.0 * v) as u8,
                        (220.0 - 140.0 * u) as u8,
                        if inside { 245 } else { 0 },
                    ]
                } else {
                    [
                        (12.0 + 30.0 * u + 15.0 * v) as u8,
                        (24.0 + 65.0 * u + 15.0 * v) as u8,
                        (54.0 + 90.0 * (1.0 - v)) as u8,
                        255,
                    ]
                };
                rgba.extend_from_slice(&color);
            }
        }
        let image = RgbaImage {
            width,
            height,
            rgba,
        };
        fs::write(
            root.join(format!("materials/{name}.tex")),
            wpe_formats::tex::encode_rgba8(&image)?,
        )?;
        write_json(
            &root.join(format!("materials/{name}.json")),
            &json!({"passes":[{"shader":"genericimage2","blending":"translucent","textures":[name]}]}),
        )?;
        write_json(
            &root.join(format!("models/{name}.json")),
            &json!({"material":format!("materials/{name}.json"),"width":width,"height":height,"autosize":true}),
        )?;
    }
    write_json(
        &root.join("project.json"),
        &json!({"type":"scene","title":"Generated static image demonstration","file":"scene.json","description":"Original generated fixtures; no Wallpaper Engine artwork bundled."}),
    )?;
    write_json(
        &root.join("scene.json"),
        &json!({
            "general":{"orthogonalprojection":{"width":960,"height":540},"clearcolor":"0.03 0.04 0.07","bloom":false},
            "objects":[
                {"id":1,"name":"Generated gradient","image":"models/demo.gradient.json","origin":"480 270 0","scale":"4 4 1","angles":"0 0 0","size":"240 135","visible":true},
                {"id":2,"name":"Generated card","image":"models/demo.card.json","origin":"480 270 0","scale":"1.7 1.7 1","angles":"0 0 0.12","size":"256 144","alpha":0.94,"visible":true}
            ]
        }),
    )?;
    write_json(
        &root.join("web/project.json"),
        &json!({"type":"web","title":"Generated web animation","file":"index.html","general":{"properties":{"accent":{"type":"color","value":"0.2 0.7 0.9"}}}}),
    )?;
    fs::write(root.join("web/index.html"), include_str!("demo.html"))?;
    expanded_demo(root)?;
    emit(
        &json!({"project":root.join("project.json"),"web_project":root.join("web/project.json"),"next":format!("wpe render '{}' --output preview.png",root.display())}),
        None,
    )
}

fn expanded_demo(root: &Path) -> Result<()> {
    const FILES: &[(&str, &str)] = &[
        (
            "animated/project.json",
            include_str!("../fixtures/animated/project.json"),
        ),
        (
            "animated/scene.json",
            include_str!("../fixtures/animated/scene.json"),
        ),
        (
            "animated/particles/owned.json",
            include_str!("../fixtures/animated/particles/owned.json"),
        ),
        (
            "effects/project.json",
            include_str!("../fixtures/effects/project.json"),
        ),
        (
            "effects/scene.json",
            include_str!("../fixtures/effects/scene.json"),
        ),
        (
            "effects/effects/owned.json",
            include_str!("../fixtures/effects/effects/owned.json"),
        ),
        (
            "effects/materials/owned.effect.json",
            include_str!("../fixtures/effects/materials/owned.effect.json"),
        ),
        (
            "effects/shaders/owned.effect.vert",
            include_str!("../fixtures/effects/shaders/owned.effect.vert"),
        ),
        (
            "effects/shaders/owned.effect.frag",
            include_str!("../fixtures/effects/shaders/owned.effect.frag"),
        ),
    ];
    for (name, source) in FILES {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap())?;
        fs::write(path, source)?;
    }
    for folder in ["animated", "effects", "text", "sound"] {
        for directory in ["materials", "models"] {
            fs::create_dir_all(root.join(folder).join(directory))?;
            for file in fs::read_dir(root.join(directory))? {
                let file = file?;
                if file.file_type()?.is_file() {
                    fs::copy(
                        file.path(),
                        root.join(folder).join(directory).join(file.file_name()),
                    )?;
                }
            }
        }
    }
    let white = wpe_formats::RgbaImage::new(1, 1, vec![255; 4])?;
    fs::write(
        root.join("animated/materials/owned.particle.tex"),
        wpe_formats::tex::encode_rgba8(&white)?,
    )?;
    write_json(
        &root.join("animated/materials/owned.particle.json"),
        &json!({"passes":[{"shader":"genericimage2","textures":["owned.particle"],"blending":"additive"}]}),
    )?;
    write_json(
        &root.join("text/project.json"),
        &json!({"type":"scene","file":"scene.json","title":"WallpaperMac — Unicode text and font provider"}),
    )?;
    write_json(
        &root.join("text/scene.json"),
        &json!({"general":{"orthogonalprojection":{"width":960,"height":540},"clearcolor":"0.03 0.04 0.09"},"objects":[{"id":1,"name":"Unicode text","font":"Helvetica","pointsize":64,"color":"0.6 0.85 1","text":"WallpaperMac\nMotion · Metal · SceneScript\nمرحبا بالعالم","origin":"480 270 0","horizontalalign":"center","padding":12,"outline":1,"outlinecolor":"0 0 0"}]}),
    )?;
    fs::create_dir_all(root.join("sound/sounds"))?;
    // Own PCM16 stereo440/660Hz tones, tapered to prevent edge discontinuities.
    let rate = 44100u32;
    let frames = rate * 2;
    let mut pcm = Vec::new();
    for frame in 0..frames {
        let t = frame as f32 / rate as f32;
        let envelope = (t / 0.03).min(1.) * ((2. - t) / 0.03).min(1.);
        for frequency in [440., 660.] {
            pcm.extend((((t*frequency*std::f32::consts::TAU).sin()*envelope*0.08*i16::MAX as f32)as i16).to_le_bytes());
        }
    }
    let mut wav = Vec::new();
    wav.extend(b"RIFF");
    wav.extend((36 + pcm.len() as u32).to_le_bytes());
    wav.extend(b"WAVEfmt ");
    wav.extend(16u32.to_le_bytes());
    wav.extend(1u16.to_le_bytes());
    wav.extend(2u16.to_le_bytes());
    wav.extend(rate.to_le_bytes());
    wav.extend((rate * 4).to_le_bytes());
    wav.extend(4u16.to_le_bytes());
    wav.extend(16u16.to_le_bytes());
    wav.extend(b"data");
    wav.extend((pcm.len() as u32).to_le_bytes());
    wav.extend(pcm);
    fs::write(root.join("sound/sounds/owned.wav"), wav)?;
    fs::create_dir_all(root.join("video"))?;
    fs::write(
        root.join("video/owned.mp4"),
        include_bytes!("../fixtures/video/owned.mp4"),
    )?;
    write_json(
        &root.join("video/project.json"),
        &json!({"type":"video","file":"owned.mp4","title":"WallpaperMac — generated video and tone test"}),
    )?;
    write_json(
        &root.join("sound/project.json"),
        &json!({"type":"scene","file":"scene.json","title":"WallpaperMac — click to play stereo tone"}),
    )?;
    write_json(
        &root.join("sound/scene.json"),
        &json!({"general":{"orthogonalprojection":{"width":960,"height":540},"clearcolor":"0.03 0.04 0.09"},"objects":[{"id":1,"image":"models/demo.card.json","origin":"480 270 0","scale":"2 2 1","visible":{"value":true,"script":"export function cursorClick(){thisScene.getLayerByID(2).play();return true;}"}},{"id":2,"name":"Owned tone","sound":["sounds/owned.wav"],"playbackmode":"single","startsilent":true,"volume":0.6}]}),
    )?;
    Ok(())
}

#[derive(Default)]
struct Corpus {
    files: usize,
    file_bytes: u64,
    packages: usize,
    members: usize,
    textures: usize,
    models: usize,
    errors: Vec<Value>,
}

impl Corpus {
    fn payload(&mut self, label: &str, bytes: &[u8], ext: &str) {
        let parsed: Result<()> = (|| {
            match ext {
                "tex" => {
                    Texture::parse(bytes)?;
                    self.textures += 1;
                }
                "mdl" => {
                    Model::parse(bytes)?;
                    self.models += 1;
                }
                _ => {}
            }
            Ok(())
        })();
        if let Err(error) = parsed {
            self.errors
                .push(json!({"path":label,"error":error.to_string()}));
        }
    }

    fn file(&mut self, path: &Path) {
        let ext = extension(path);
        if !matches!(ext.as_str(), "pkg" | "tex" | "mdl") {
            return;
        }
        self.files += 1;
        let read: Result<()> = (|| {
            let bytes = fs::read(path)?;
            self.file_bytes += bytes.len() as u64;
            if ext == "pkg" {
                let package = Package::parse(bytes)?;
                self.packages += 1;
                self.members += package.members().len();
                for member in package.members() {
                    let ext = extension(Path::new(&member.name));
                    if matches!(ext.as_str(), "tex" | "mdl") {
                        let data = package
                            .get(&member.name)
                            .ok_or("Indexed package member is missing")?;
                        self.payload(&format!("{}!{}", path.display(), member.name), data, &ext);
                    }
                }
            } else {
                self.payload(&path.display().to_string(), &bytes, &ext);
            }
            Ok(())
        })();
        if let Err(error) = read {
            self.errors
                .push(json!({"path":path,"error":error.to_string()}));
        }
    }
}

fn collect_paths(path: &Path, result: &mut BTreeSet<PathBuf>) -> Result<()> {
    if path.is_dir() {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_dir() {
                collect_paths(&entry.path(), result)?;
            } else if kind.is_file() {
                result.insert(entry.path());
            }
        }
    } else if extension(path) == "json" {
        let inventory: Value = serde_json::from_slice(&fs::read(path)?)?;
        let files = inventory
            .get("files")
            .and_then(Value::as_array)
            .ok_or("Corpus JSON must be a file inventory with files[].path")?;
        for row in files {
            result.insert(PathBuf::from(
                row["path"]
                    .as_str()
                    .ok_or("Inventory entry is missing path")?,
            ));
        }
    } else {
        result.insert(path.to_owned());
    }
    Ok(())
}

fn corpus(args: &Args) -> Result<()> {
    if args.paths.is_empty() {
        return Err("corpus requires at least one directory or file inventory".into());
    }
    let mut paths = BTreeSet::new();
    for path in &args.paths {
        collect_paths(path, &mut paths)?;
    }
    let mut report = Corpus::default();
    for path in paths {
        report.file(&path);
    }
    let success = report.errors.is_empty() && report.files > 0;
    emit(
        &json!({"passed":success,"binary_disk_files":report.files,"disk_bytes":report.file_bytes,"packages":report.packages,"package_members":report.members,"texture_occurrences":report.textures,"model_occurrences":report.models,"errors":report.errors,"scope":"Structural parsers and bounds, not GPU/script/behavior compatibility or a complete texture pixel oracle"}),
        args.output.as_deref(),
    )?;
    if !success {
        return Err("Corpus validation failed; see the report".into());
    }
    Ok(())
}

fn run() -> Result<()> {
    let args = parse_args()?;
    match args.command.as_str() {
        "help" | "--help" | "-h" => {
            print!("{HELP}");
            Ok(())
        }
        "inspect" => inspect(&args),
        "render" => render(&args),
        "demo" => demo(&args),
        "frame" => playback(&args, true),
        "simulate" => playback(&args, false),
        "corpus" => corpus(&args),
        other => Err(format!("Unknown command: {other}\n{HELP}").into()),
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("wpe: {error}");
        std::process::exit(2);
    }
}
