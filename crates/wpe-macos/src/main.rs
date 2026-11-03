//! Native macOS presentation host. Portable parsing/render planning lives in wpe-core.
use wpe_macos::Options;

fn options() -> Result<Option<Options>, String> {
    let mut result = Options::default();
    let mut args = std::env::args_os().skip(1);
    while let Some(argument) = args.next() {
        match argument.to_str() {
            Some("--help" | "-h") => {
                println!(
                    "wallpaper-macos [--project PATH] [--assets PATH] [--desktop] [--screen N]\n\nOpens a Wallpaper Engine project using native macOS views. With no project, Settings opens.\nDefault: foreground preview. --desktop creates a click-through window on screen N (zero based).\nScene, video and web providers report content compatibility issues in Settings."
                );
                return Ok(None);
            }
            Some("--project") => {
                result.project = Some(args.next().ok_or("--project requires a path")?.into())
            }
            Some("--assets") => {
                result.assets = Some(args.next().ok_or("--assets requires a path")?.into())
            }
            Some("--desktop") => result.desktop = true,
            Some("--screen") => {
                result.screen = args
                    .next()
                    .ok_or("--screen requires an index")?
                    .to_str()
                    .ok_or("screen index must be UTF-8")?
                    .parse()
                    .map_err(|_| "screen index must be a nonnegative integer")?
            }
            _ => return Err(format!("unknown argument: {}", argument.to_string_lossy())),
        }
    }
    Ok(Some(result))
}

fn main() {
    let options = match options() {
        Ok(Some(options)) => options,
        Ok(None) => return,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    };
    #[cfg(target_os = "macos")]
    if let Err(error) = wpe_macos::run(options) {
        eprintln!("{error}");
        std::process::exit(1);
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = options;
        eprintln!(
            "wallpaper-macos requires macOS. Use the portable wpe-cli for inspection and software rendering on this host."
        );
        std::process::exit(1);
    }
}
