//! Explicit external-runtime experiment: an owned process group and a public
//! ScreenCaptureKit window stream. No Windows desktop injection or IPC emulation.
use super::{metal::SceneView, *};
use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchRetained};
use libc::{SIGCONT, SIGKILL, SIGSTOP, SIGTERM, kill};
use objc2::{AnyThread, runtime::ProtocolObject};
use objc2_core_media::{CMSampleBuffer, CMTime};
use objc2_core_video::*;
use objc2_foundation::NSBundle;
use objc2_screen_capture_kit::*;
use std::{
    cell::Cell,
    os::unix::process::CommandExt,
    process::{Child, Command, Stdio},
    rc::{Rc, Weak},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use wpe_platform::application::ApplicationRuntime;
use wpe_render::{DrawCall, GpuVertex, Material, RenderFrame, RgbaImage, Texture, TextureBinding};
#[derive(Default)]
struct Shared {
    image: Option<Arc<RgbaImage>>,
    error: Option<String>,
}
define_class!(
 #[unsafe(super=NSObject)]#[thread_kind=AnyThread]#[ivars=Arc<Mutex<Shared>>]
 struct WindowSink;
 unsafe impl NSObjectProtocol for WindowSink{}
 unsafe impl SCStreamOutput for WindowSink{
  #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]unsafe fn output(&self,_stream:&SCStream,sample:&CMSampleBuffer,kind:SCStreamOutputType){if kind!=SCStreamOutputType::Screen{return;}let Some(buffer)=(unsafe{sample.image_buffer()})else{return;};let result=unsafe{copy_pixels(&buffer)};if let Ok(mut state)=self.ivars().lock(){match result{Ok(image)=>state.image=Some(Arc::new(image)),Err(error)=>state.error=Some(error)}}}
 }
 unsafe impl SCStreamDelegate for WindowSink{
  #[unsafe(method(stream:didStopWithError:))]unsafe fn stopped(&self,_stream:&SCStream,error:&NSError){if let Ok(mut state)=self.ivars().lock(){state.error=Some(error.localizedDescription().to_string());}}
 }
);
struct Resources {
    stream: Retained<SCStream>,
    sink: Retained<WindowSink>,
    _queue: DispatchRetained<DispatchQueue>,
}
struct Pending {
    resources: Option<Resources>,
    config: ApplicationRuntime,
    pid: i32,
    querying: bool,
    shared: Arc<Mutex<Shared>>,
}
thread_local! {static REQUESTS:RefCell<BTreeMap<u64,Weak<RefCell<Pending>>>>=const{RefCell::new(BTreeMap::new())};}
static NEXT: AtomicU64 = AtomicU64::new(1);
pub struct ApplicationView {
    pub renderer: SceneView,
    project: wpe_core::Project,
    config: ApplicationRuntime,
    executable: PathBuf,
    child: RefCell<Option<Child>>,
    pending: Rc<RefCell<Pending>>,
    id: Cell<u64>,
    stream_paused: Cell<bool>,
    paused: Cell<bool>,
    muted: Cell<bool>,
    silent: Cell<bool>,
    last_query: Cell<Instant>,
    started: Cell<Option<Instant>>,
    closed: Cell<bool>,
    last_frame: RefCell<Option<Arc<RgbaImage>>>,
}
impl ApplicationView {
    pub fn new(
        project: wpe_core::Project,
        config: ApplicationRuntime,
        frame: NSRect,
        mtm: MainThreadMarker,
    ) -> Result<Self, String> {
        config.validate().map_err(|e| e.to_string())?;
        let executable =
            if config.command.is_dir() && config.command.extension().is_some_and(|e| e == "app") {
                NSBundle::bundleWithPath(&NSString::from_str(&config.command.to_string_lossy()))
                    .and_then(|b| b.executableURL())
                    .and_then(|u| u.path())
                    .map(|s| PathBuf::from(s.to_string()))
                    .ok_or("Native app bundle has no executable URL")?
            } else {
                config.command.clone()
            };
        if !executable.is_file() {
            return Err("Configured external runtime executable does not exist".into());
        }
        let image = Arc::new(RgbaImage {
            width: 1,
            height: 1,
            rgba: vec![0, 0, 0, 255],
        });
        let renderer = SceneView::from_frame(frame_for(image), frame, mtm)?;
        let shared = Arc::new(Mutex::new(Shared::default()));
        Ok(Self {
            renderer,
            project,
            config: config.clone(),
            executable,
            child: RefCell::new(None),
            pending: Rc::new(RefCell::new(Pending {
                resources: None,
                config,
                pid: 0,
                querying: false,
                shared,
            })),
            id: Cell::new(0),
            stream_paused: Cell::new(true),
            paused: Cell::new(true),
            muted: Cell::new(false),
            silent: Cell::new(false),
            last_query: Cell::new(Instant::now() - Duration::from_secs(1)),
            started: Cell::new(None),
            closed: Cell::new(false),
            last_frame: RefCell::new(None),
        })
    }
    pub fn matches(&self, config: &ApplicationRuntime) -> bool {
        &self.config == config
    }
    pub fn config_key(&self) -> String {
        self.project.root.to_string_lossy().into_owned()
    }
    pub fn set_paused(&self, value: bool) {
        self.paused.set(value);
        self.signal_policy();
    }
    pub fn set_volume(&self, volume: f32) {
        self.silent.set(volume <= 0.);
        self.signal_policy();
    }
    pub fn set_muted(&self, value: bool) {
        self.muted.set(value);
        self.signal_policy();
    }
    fn signal_policy(&self) {
        let paused = self.paused.get() || self.muted.get() || self.silent.get();
        if self.stream_paused.replace(paused) != paused {
            if paused {
                self.stop_capture();
            } else {
                self.started.set(Some(Instant::now()));
            }
        }
        if let Some(child) = &*self.child.borrow() {
            unsafe {
                kill(
                    -(child.id() as i32),
                    if self.paused.get() || self.muted.get() || self.silent.get() {
                        SIGSTOP
                    } else {
                        SIGCONT
                    },
                );
            }
        }
    }
    pub fn tick(&self) -> Result<(), String> {
        if self.closed.get() {
            return Ok(());
        }
        if self.paused.get() || self.muted.get() || self.silent.get() {
            return Ok(());
        }
        if self.child.borrow().is_none() {
            let args = self
                .config
                .arguments_for(&self.project.entry, &self.project.root)
                .map_err(|e| e.to_string())?;
            let child = Command::new(&self.executable)
                .args(args)
                .current_dir(&self.project.root)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .process_group(0)
                .spawn()
                .map_err(|e| format!("External runtime launch failed: {e}"))?;
            self.pending.borrow_mut().pid = child.id() as i32;
            *self.child.borrow_mut() = Some(child);
            self.started.set(Some(Instant::now()));
        }
        let exit = {
            let mut child = self.child.borrow_mut();
            if let Some(child) = child.as_mut() {
                child.try_wait().map_err(|e| e.to_string())?
            } else {
                None
            }
        };
        if let Some(status) = exit {
            self.close();
            return Err(format!(
                "External runtime child exited ({status}); configure a foreground runtime that owns its window"
            ));
        }
        let pending = self.pending.borrow();
        let ready = pending.resources.is_some();
        let querying = pending.querying;
        let shared = pending.shared.clone();
        drop(pending);
        if !ready && !querying && self.last_query.get().elapsed() > Duration::from_millis(500) {
            if self
                .started
                .get()
                .is_some_and(|t| t.elapsed() > Duration::from_secs(15))
            {
                self.close();
                return Err("No shareable window matched the owned runtime PID/configured window selector within15s. Check Screen Recording permission, Wine process ownership and window title".into());
            }
            self.request();
        }
        let mut state = shared
            .lock()
            .map_err(|_| "Window capture state unavailable")?;
        if let Some(error) = state.error.take() {
            return Err(error);
        }
        if let Some(image) = state.image.take() {
            let different = self
                .last_frame
                .borrow()
                .as_ref()
                .is_none_or(|old| !Arc::ptr_eq(old, &image));
            if different {
                self.renderer.submit(frame_for(image.clone()))?;
                *self.last_frame.borrow_mut() = Some(image);
            }
        }
        self.renderer.failure().map(Err).unwrap_or(Ok(()))
    }
    fn request(&self) {
        self.last_query.set(Instant::now());
        self.pending.borrow_mut().querying = true;
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        self.id.set(id);
        REQUESTS.with(|m| m.borrow_mut().insert(id, Rc::downgrade(&self.pending)));
        let completion = RcBlock::new(
            move |content: *mut SCShareableContent, error: *mut NSError| {
                let content = unsafe { Retained::retain(content) };
                let error = unsafe { error.as_ref() }.map(|e| e.localizedDescription().to_string());
                let address = content.map(|c| Retained::into_raw(c) as usize).unwrap_or(0);
                DispatchQueue::main().exec_async(move || {
                    let content = unsafe { Retained::from_raw(address as *mut SCShareableContent) };
                    let pending =
                        REQUESTS.with(|m| m.borrow_mut().remove(&id).and_then(|p| p.upgrade()));
                    let Some(pending) = pending else {
                        return;
                    };
                    let mut p = pending.borrow_mut();
                    p.querying = false;
                    let result = if let Some(error) = error {
                        Err(error)
                    } else if let Some(content) = content {
                        unsafe { configure(&mut p, &content) }
                    } else {
                        Err("ScreenCaptureKit returned no shareable windows".into())
                    };
                    if let Err(error) = result
                        && let Ok(mut s) = p.shared.lock()
                    {
                        s.error = Some(error);
                    }
                });
            },
        );
        unsafe {
            SCShareableContent::getShareableContentWithCompletionHandler(&completion);
        }
    }
    fn stop_capture(&self) {
        REQUESTS.with(|m| m.borrow_mut().remove(&self.id.get()));
        self.pending.borrow_mut().querying = false;
        if let Some(resources) = self.pending.borrow_mut().resources.take() {
            let stream = resources.stream.clone();
            let sink = resources.sink.clone();
            let complete = RcBlock::new(move |_error: *mut NSError| unsafe {
                let _ = stream.removeStreamOutput_type_error(
                    ProtocolObject::from_ref(&*sink),
                    SCStreamOutputType::Screen,
                );
            });
            unsafe {
                resources
                    .stream
                    .stopCaptureWithCompletionHandler(Some(&complete));
            }
        }
    }
    fn close(&self) {
        self.closed.set(true);
        self.stop_capture();
        if let Some(mut child) = self.child.borrow_mut().take() {
            let pid = child.id() as i32;
            unsafe {
                kill(-pid, SIGCONT);
                kill(-pid, SIGTERM);
            }
            let _ = std::thread::Builder::new()
                .name("wallpaper-application-close".into())
                .spawn(move || {
                    let end = Instant::now() + Duration::from_secs(2);
                    while Instant::now() < end {
                        if child.try_wait().ok().flatten().is_some() {
                            return;
                        }
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    unsafe {
                        kill(-pid, SIGKILL);
                    }
                    let _ = child.kill();
                    let _ = child.wait();
                });
        }
    }
}
impl Drop for ApplicationView {
    fn drop(&mut self) {
        self.close();
    }
}
unsafe fn configure(p: &mut Pending, content: &SCShareableContent) -> Result<(), String> {
    let windows = unsafe { content.windows() };
    let selected = windows
        .iter()
        .filter(|w| {
            let pid = unsafe { w.owningApplication() }.map(|a| unsafe { a.processID() });
            if pid == Some(std::process::id() as i32) {
                return false;
            }
            if let Some(id) = p.config.window_id {
                return unsafe { w.windowID() } == id;
            }
            let title = unsafe { w.title() }
                .map(|s| s.to_string())
                .unwrap_or_default();
            if let Some(expected) = &p.config.window_title {
                return title.contains(expected);
            }
            pid == Some(p.pid)
        })
        .max_by_key(|w| {
            let f = unsafe { w.frame() };
            (f.size.width * f.size.height) as u64
        });
    let Some(window) = selected else {
        return Ok(());
    };
    let rect = unsafe { window.frame() };
    if !rect.size.width.is_finite()
        || !rect.size.height.is_finite()
        || rect.size.width <= 0.
        || rect.size.height <= 0.
    {
        return Err("Captured window has invalid bounds".into());
    }
    let filter = unsafe {
        SCContentFilter::initWithDesktopIndependentWindow(SCContentFilter::alloc(), &window)
    };
    let config = unsafe { SCStreamConfiguration::new() };
    unsafe {
        config.setWidth((rect.size.width * 2.).clamp(1., 8192.) as usize);
        config.setHeight((rect.size.height * 2.).clamp(1., 8192.) as usize);
        config.setPixelFormat(kCVPixelFormatType_32BGRA);
        config.setQueueDepth(3);
        config.setCapturesAudio(false);
        config.setShowsCursor(false);
        config.setMinimumFrameInterval(CMTime::with_seconds(1. / 30., 60000));
    }
    let sink = WindowSink::alloc().set_ivars(p.shared.clone());
    let sink: Retained<WindowSink> = unsafe { msg_send![super(sink), init] };
    let stream = unsafe {
        SCStream::initWithFilter_configuration_delegate(
            SCStream::alloc(),
            &filter,
            &config,
            Some(ProtocolObject::from_ref(&*sink)),
        )
    };
    let queue = DispatchQueue::new("wallpaper.application.capture", None);
    unsafe {
        stream.addStreamOutput_type_sampleHandlerQueue_error(
            ProtocolObject::from_ref(&*sink),
            SCStreamOutputType::Screen,
            Some(&queue),
        )
    }
    .map_err(|e| e.localizedDescription().to_string())?;
    let state = p.shared.clone();
    let complete = RcBlock::new(move |error: *mut NSError| {
        if let Some(error) = unsafe { error.as_ref() }
            && let Ok(mut state) = state.lock()
        {
            state.error = Some(error.localizedDescription().to_string());
        }
    });
    unsafe {
        stream.startCaptureWithCompletionHandler(Some(&complete));
    }
    p.resources = Some(Resources {
        stream,
        sink,
        _queue: queue,
    });
    Ok(())
}
unsafe fn copy_pixels(buffer: &CVPixelBuffer) -> Result<RgbaImage, String> {
    unsafe {
        if CVPixelBufferLockBaseAddress(buffer, CVPixelBufferLockFlags::ReadOnly) != 0 {
            return Err("Capture pixel buffer lock failed".into());
        }
        struct Unlock<'a>(&'a CVPixelBuffer);
        impl Drop for Unlock<'_> {
            fn drop(&mut self) {
                unsafe {
                    CVPixelBufferUnlockBaseAddress(self.0, CVPixelBufferLockFlags::ReadOnly);
                }
            }
        }
        let _unlock = Unlock(buffer);
        let (w, h, stride) = (
            CVPixelBufferGetWidth(buffer),
            CVPixelBufferGetHeight(buffer),
            CVPixelBufferGetBytesPerRow(buffer),
        );
        let bytes = w
            .checked_mul(h)
            .and_then(|n| n.checked_mul(4))
            .filter(|n| *n <= 256 * 1024 * 1024)
            .ok_or("Capture frame exceeds256MiB")?;
        if w == 0
            || h == 0
            || stride < w * 4
            || CVPixelBufferGetPixelFormatType(buffer) != kCVPixelFormatType_32BGRA
        {
            return Err("Capture frame layout is not packed BGRA".into());
        }
        let base = CVPixelBufferGetBaseAddress(buffer).cast::<u8>();
        if base.is_null() {
            return Err("Capture frame has no CPU plane".into());
        }
        let mut rgba = vec![0; bytes];
        for y in 0..h {
            let row = std::slice::from_raw_parts(base.add(y * stride), w * 4);
            for (x, p) in row.chunks_exact(4).enumerate() {
                rgba[(y * w + x) * 4..(y * w + x + 1) * 4]
                    .copy_from_slice(&[p[2], p[1], p[0], p[3]]);
            }
        }
        RgbaImage::new(w as u32, h as u32, rgba).map_err(|e| e.to_string())
    }
}
fn frame_for(image: Arc<RgbaImage>) -> RenderFrame {
    let mut frame = RenderFrame::new(image.width, image.height);
    frame
        .textures
        .insert(1, Texture::rgba2d("external-window", image));
    let vertices = [
        ([-1., -1., 0., 1.], [0., 1., 0., 0.]),
        ([1., -1., 0., 1.], [1., 1., 0., 0.]),
        ([-1., 1., 0., 1.], [0., 0., 0., 0.]),
        ([1., 1., 0., 1.], [1., 0., 0., 0.]),
    ]
    .into_iter()
    .map(|(position, uv)| GpuVertex {
        position,
        uv,
        normal: [0., 0., 1., 0.],
        color: [1.; 4],
    })
    .collect();
    frame.draws.push(DrawCall {
        vertices,
        indices: vec![0, 2, 1, 1, 2, 3],
        material: Material {
            textures: vec![TextureBinding {
                name: "g_Texture0".into(),
                slot: 0,
                texture: 1,
            }],
            ..Default::default()
        },
        ..Default::default()
    });
    frame
}
