//! Run this executable on the physical Mac to validate real Metal execution.
#[cfg(target_os = "macos")]
fn main() -> Result<(), String> {
    let mut args = std::env::args_os().skip(1);
    let output = std::path::PathBuf::from(
        args.next()
            .ok_or("usage: metal_smoke OUTPUT_DIRECTORY [OPTIONAL_RETAINED_SHADER_ROOT]")?,
    );
    let shaders = args.next().map(std::path::PathBuf::from);
    objc2::rc::autoreleasepool(|_| {
        let mtm = objc2_foundation::MainThreadMarker::new()
            .ok_or("Metal smoke must start on the process main thread")?;
        let report = wpe_macos::metal_smoke(&output, shaders.as_deref(), mtm)?;
        println!(
            "{}",
            serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?
        );
        Ok(())
    })
}
#[cfg(not(target_os = "macos"))]
fn main() -> Result<(), String> {
    Err("Metal smoke requires a physical macOS Metal device; no GPU result is synthesized on this host".into())
}
