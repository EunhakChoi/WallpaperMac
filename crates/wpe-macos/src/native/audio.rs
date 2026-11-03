//! ScreenCaptureKit audio capture. Sample callbacks only validate/copy PCM and
//! try_send to a bounded worker; FFT work never runs on the capture callback.
use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchRetained};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{AnyThread, DefinedClass, define_class, msg_send, sel};
use objc2_core_audio_types::*;
use objc2_core_foundation::CFRetained;
use objc2_core_media::{
    CMAudioFormatDescriptionGetStreamBasicDescription, CMBlockBuffer, CMSampleBuffer, CMTime,
};
use objc2_foundation::{NSArray, NSError, NSObject, NSObjectProtocol, NSString};
use objc2_screen_capture_kit::*;
use std::{
    cell::RefCell,
    collections::BTreeMap,
    ptr::NonNull,
    rc::{Rc, Weak},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, SyncSender},
    },
    thread,
    time::{Duration, Instant},
};
use wpe_platform::{
    audio::{PcmBlock, PcmEncoding, RawSpectrum, SpectrumAnalyzer},
    config::AudioSettings,
};

#[derive(Clone, Debug)]
pub struct AudioStatus {
    pub state: String,
    pub received_blocks: u64,
    pub dropped_blocks: u64,
    pub peak: f32,
    pub age_seconds: f32,
}
#[derive(Default)]
struct Shared {
    latest: RawSpectrum,
    updated: Option<Instant>,
    status: String,
    peak: f32,
}
struct SinkState {
    tx: SyncSender<PcmBlock>,
    shared: Arc<Mutex<Shared>>,
    received: Arc<AtomicU64>,
    dropped: Arc<AtomicU64>,
    input_route: bool,
}

define_class!(
    #[unsafe(super=NSObject)]
    #[thread_kind=AnyThread]
    #[ivars=SinkState]
    struct AudioSink;
    unsafe impl NSObjectProtocol for AudioSink {}
    unsafe impl SCStreamOutput for AudioSink {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        unsafe fn output(
            &self,
            _stream: &SCStream,
            sample: &CMSampleBuffer,
            kind: SCStreamOutputType,
        ) {
            if kind != SCStreamOutputType::Audio && kind != SCStreamOutputType::Microphone {
                return;
            }
            match unsafe {
                read_pcm(
                    sample,
                    kind == SCStreamOutputType::Microphone || self.ivars().input_route,
                )
            } {
                Ok(block) => {
                    self.ivars().received.fetch_add(1, Ordering::Relaxed);
                    if self.ivars().tx.try_send(block).is_err() {
                        self.ivars().dropped.fetch_add(1, Ordering::Relaxed);
                    }
                }
                Err(error) => {
                    if let Ok(mut shared) = self.ivars().shared.lock() {
                        shared.status = error;
                    }
                }
            }
        }
    }
    unsafe impl SCStreamDelegate for AudioSink {
        #[unsafe(method(stream:didStopWithError:))]
        unsafe fn stopped(&self, _stream: &SCStream, error: &NSError) {
            if let Ok(mut shared) = self.ivars().shared.lock() {
                shared.status = format!("Capture stopped: {}", error.localizedDescription());
            }
        }
    }
);

struct Resources {
    stream: Retained<SCStream>,
    sink: Retained<AudioSink>,
    _queue: DispatchRetained<DispatchQueue>,
}
struct Pending {
    resources: Option<Resources>,
    settings: AudioSettings,
    tx: SyncSender<PcmBlock>,
    shared: Arc<Mutex<Shared>>,
    received: Arc<AtomicU64>,
    dropped: Arc<AtomicU64>,
    target_pid: Option<i32>,
    exclude_pids: Vec<i32>,
}
thread_local! {static REQUESTS:RefCell<BTreeMap<u64,Weak<RefCell<Pending>>>>=const{RefCell::new(BTreeMap::new())};}
static NEXT: AtomicU64 = AtomicU64::new(1);

pub struct AudioCapture {
    id: u64,
    pending: Rc<RefCell<Pending>>,
    shared: Arc<Mutex<Shared>>,
    cancel: Arc<AtomicBool>,
    analyze: Arc<AtomicBool>,
    received: Arc<AtomicU64>,
    dropped: Arc<AtomicU64>,
}
impl AudioCapture {
    pub fn start(settings: AudioSettings) -> Result<Self, String> {
        let analyze = settings.enabled;
        Self::start_target(settings, None, vec![], analyze)
    }
    pub fn start_for_pid(pid: i32) -> Result<Self, String> {
        Self::start_target(AudioSettings::default(), Some(pid), vec![], false)
    }
    pub fn start_excluding(pids: Vec<i32>) -> Result<Self, String> {
        Self::start_target(AudioSettings::default(), None, pids, false)
    }
    fn start_target(
        settings: AudioSettings,
        target_pid: Option<i32>,
        exclude_pids: Vec<i32>,
        analyze: bool,
    ) -> Result<Self, String> {
        objc2_foundation::MainThreadMarker::new()
            .ok_or("Audio capture must be created on the main thread")?;
        let shared = Arc::new(Mutex::new(Shared {
            status: "Requesting Screen Recording permission…".into(),
            ..Shared::default()
        }));
        let (tx, rx) = mpsc::sync_channel::<PcmBlock>(4);
        let cancel = Arc::new(AtomicBool::new(false));
        let analyze = Arc::new(AtomicBool::new(analyze));
        let worker_analyze = analyze.clone();
        let (worker_shared, worker_cancel, worker_settings) =
            (shared.clone(), cancel.clone(), settings.clone());
        thread::Builder::new()
            .name("wallpaper-spectrum".into())
            .spawn(move || {
                let mut analyzer = SpectrumAnalyzer::new(worker_settings);
                while !worker_cancel.load(Ordering::Acquire) {
                    match rx.recv_timeout(Duration::from_millis(100)) {
                        Ok(block) => {
                            let peak = block.samples.iter().map(|v| v.abs()).fold(0.0f32, f32::max);
                            if let Ok(mut state) = worker_shared.lock() {
                                state.peak = peak;
                                state.updated = Some(Instant::now());
                                if !worker_analyze.load(Ordering::Acquire) {
                                    state.status = "Capturing audio activity".into();
                                }
                            }
                            if !worker_analyze.load(Ordering::Acquire) {
                                continue;
                            }
                            match analyzer.push(&block) {
                                Ok(Some(raw)) => {
                                    if let Ok(mut state) = worker_shared.lock() {
                                        state.latest = raw;
                                        state.updated = Some(Instant::now());
                                        state.peak = peak;
                                        state.status = "Capturing".into();
                                    }
                                }
                                Ok(None) => {}
                                Err(error) => {
                                    if let Ok(mut state) = worker_shared.lock() {
                                        state.status = error.to_string();
                                    }
                                }
                            }
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        let received = Arc::new(AtomicU64::new(0));
        let dropped = Arc::new(AtomicU64::new(0));
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let pending = Rc::new(RefCell::new(Pending {
            resources: None,
            settings,
            tx,
            shared: shared.clone(),
            received: received.clone(),
            dropped: dropped.clone(),
            target_pid,
            exclude_pids,
        }));
        REQUESTS.with(|requests| requests.borrow_mut().insert(id, Rc::downgrade(&pending)));
        let completion = RcBlock::new(
            move |content: *mut SCShareableContent, error: *mut NSError| {
                // The immutable result is retained across a hop to the main queue.
                // UIKit/AppKit state and stream ownership remain on that queue.
                let content = unsafe { Retained::retain(content) };
                let error = unsafe { error.as_ref() }.map(|e| e.localizedDescription().to_string());
                let address = content
                    .map(|content| Retained::into_raw(content) as usize)
                    .unwrap_or(0);
                DispatchQueue::main().exec_async(move || {
                    let content = unsafe { Retained::from_raw(address as *mut SCShareableContent) };
                    let pending = REQUESTS.with(|requests| {
                        requests
                            .borrow_mut()
                            .remove(&id)
                            .and_then(|weak| weak.upgrade())
                    });
                    let Some(pending) = pending else {
                        return;
                    };
                    let outcome = if let Some(error) = error {
                        Err(error)
                    } else if let Some(content) = content {
                        unsafe { configure(&mut pending.borrow_mut(), &content) }
                    } else {
                        Err("ScreenCaptureKit returned no shareable content".into())
                    };
                    if let Err(error) = outcome
                        && let Ok(mut shared) = pending.borrow().shared.lock()
                    {
                        shared.status = error;
                    }
                });
            },
        );
        unsafe {
            SCShareableContent::getShareableContentWithCompletionHandler(&completion);
        }
        Ok(Self {
            id,
            pending,
            shared,
            cancel,
            analyze,
            received,
            dropped,
        })
    }
    pub fn set_analysis_enabled(&self, enabled: bool) {
        self.analyze.store(enabled, Ordering::Release);
    }
    pub fn latest(&self) -> RawSpectrum {
        let Ok(state) = self.shared.lock() else {
            return RawSpectrum::default();
        };
        if state
            .updated
            .is_some_and(|t| t.elapsed() < Duration::from_millis(300))
        {
            state.latest.clone()
        } else {
            RawSpectrum::default()
        }
    }
    pub fn status(&self) -> AudioStatus {
        let Ok(state) = self.shared.lock() else {
            return AudioStatus {
                state: "Capture worker unavailable".into(),
                received_blocks: 0,
                dropped_blocks: 0,
                peak: 0.,
                age_seconds: f32::INFINITY,
            };
        };
        AudioStatus {
            state: state.status.clone(),
            received_blocks: self.received.load(Ordering::Relaxed),
            dropped_blocks: self.dropped.load(Ordering::Relaxed),
            peak: state.peak,
            age_seconds: state
                .updated
                .map(|t| t.elapsed().as_secs_f32())
                .unwrap_or(f32::INFINITY),
        }
    }
}
impl Drop for AudioCapture {
    fn drop(&mut self) {
        REQUESTS.with(|requests| requests.borrow_mut().remove(&self.id));
        self.cancel.store(true, Ordering::Release);
        if let Some(resources) = self.pending.borrow_mut().resources.take() {
            let stream = resources.stream.clone();
            let sink = resources.sink.clone();
            let completion = RcBlock::new(move |_error: *mut NSError| unsafe {
                let _ = stream.removeStreamOutput_type_error(
                    ProtocolObject::from_ref(&*sink),
                    SCStreamOutputType::Audio,
                );
                let _ = stream.removeStreamOutput_type_error(
                    ProtocolObject::from_ref(&*sink),
                    SCStreamOutputType::Microphone,
                );
            });
            unsafe {
                resources
                    .stream
                    .stopCaptureWithCompletionHandler(Some(&completion));
            }
        }
    }
}

unsafe fn configure(pending: &mut Pending, content: &SCShareableContent) -> Result<(), String> {
    let displays = unsafe { content.displays() };
    if displays.count() == 0 {
        return Err("No display is available for ScreenCaptureKit audio capture".into());
    }
    let display = displays.objectAtIndex(0);
    let filter = if let Some(pid) = pending.target_pid {
        let apps = unsafe { content.applications() };
        let matching = apps
            .iter()
            .filter(|app| {
                let id: i32 = unsafe { msg_send![&**app, processID] };
                id == pid
            })
            .collect::<Vec<_>>();
        if matching.is_empty() {
            return Err(format!("Audio process {pid} is no longer shareable"));
        }
        let apps = NSArray::from_retained_slice(&matching);
        unsafe {
            SCContentFilter::initWithDisplay_includingApplications_exceptingWindows(
                SCContentFilter::alloc(),
                &display,
                &apps,
                &NSArray::new(),
            )
        }
    } else if !pending.exclude_pids.is_empty() {
        let apps = unsafe { content.applications() };
        let matching = apps
            .iter()
            .filter(|app| {
                let id: i32 = unsafe { msg_send![&**app, processID] };
                pending.exclude_pids.contains(&id)
            })
            .collect::<Vec<_>>();
        unsafe {
            SCContentFilter::initWithDisplay_excludingApplications_exceptingWindows(
                SCContentFilter::alloc(),
                &display,
                &NSArray::from_retained_slice(&matching),
                &NSArray::new(),
            )
        }
    } else {
        unsafe {
            SCContentFilter::initWithDisplay_excludingWindows(
                SCContentFilter::alloc(),
                &display,
                &NSArray::new(),
            )
        }
    };
    let configuration = unsafe { SCStreamConfiguration::new() };
    let input = pending.settings.input_device.as_ref();
    unsafe {
        configuration.setWidth(2);
        configuration.setHeight(2);
        configuration.setQueueDepth(3);
        configuration.setMinimumFrameInterval(CMTime {
            value: 1,
            timescale: 1,
            flags: objc2_core_media::CMTimeFlags::Valid,
            epoch: 0,
        });
        configuration.setSampleRate(pending.settings.sample_rate as isize);
        configuration.setChannelCount(2);
        configuration.setCapturesAudio(input.is_none());
        configuration.setExcludesCurrentProcessAudio(pending.settings.exclude_self);
    }
    if let Some(device) = input {
        if !configuration.respondsToSelector(sel!(setCaptureMicrophone:)) {
            return Err("Microphone capture requires macOS 15 or later".into());
        }
        unsafe {
            configuration.setCaptureMicrophone(true);
            if device != "default" {
                configuration.setMicrophoneCaptureDeviceID(Some(&NSString::from_str(device)));
            }
        }
    }
    let sink = AudioSink::alloc().set_ivars(SinkState {
        tx: pending.tx.clone(),
        shared: pending.shared.clone(),
        received: pending.received.clone(),
        dropped: pending.dropped.clone(),
        input_route: input.is_some(),
    });
    let sink: Retained<AudioSink> = unsafe { msg_send![super(sink), init] };
    let stream = unsafe {
        SCStream::initWithFilter_configuration_delegate(
            SCStream::alloc(),
            &filter,
            &configuration,
            Some(ProtocolObject::from_ref(&*sink)),
        )
    };
    let queue = DispatchQueue::new("wallpaper.audio", None);
    let kind = if input.is_some() {
        SCStreamOutputType::Microphone
    } else {
        SCStreamOutputType::Audio
    };
    unsafe {
        stream.addStreamOutput_type_sampleHandlerQueue_error(
            ProtocolObject::from_ref(&*sink),
            kind,
            Some(&queue),
        )
    }
    .map_err(|e| e.localizedDescription().to_string())?;
    let shared = pending.shared.clone();
    let completion = RcBlock::new(move |error: *mut NSError| {
        if let Ok(mut state) = shared.lock() {
            state.status = unsafe { error.as_ref() }
                .map(|e| {
                    format!(
                        "Capture permission/start failed: {}",
                        e.localizedDescription()
                    )
                })
                .unwrap_or_else(|| "Capture started; waiting for PCM".into());
        }
    });
    unsafe {
        stream.startCaptureWithCompletionHandler(Some(&completion));
    }
    pending.resources = Some(Resources {
        stream,
        sink,
        _queue: queue,
    });
    Ok(())
}

unsafe fn read_pcm(sample: &CMSampleBuffer, input_route: bool) -> Result<PcmBlock, String> {
    let format =
        unsafe { sample.format_description() }.ok_or("Audio sample has no format description")?;
    let asbd = unsafe { CMAudioFormatDescriptionGetStreamBasicDescription(&format).as_ref() }
        .ok_or("Audio sample has no stream description")?;
    if asbd.mFormatID != kAudioFormatLinearPCM {
        return Err("ScreenCaptureKit sample is not linear PCM".into());
    }
    let float = asbd.mFormatFlags & kAudioFormatFlagIsFloat != 0;
    let signed = asbd.mFormatFlags & kAudioFormatFlagIsSignedInteger != 0;
    let encoding = match (float, signed, asbd.mBitsPerChannel) {
        (true, _, 32) => PcmEncoding::Float32,
        (true, _, 64) => PcmEncoding::Float64,
        (false, true, 1..=32) => {
            let planes = asbd.mFormatFlags & kAudioFormatFlagIsNonInterleaved != 0;
            let divisor = if planes {
                1
            } else {
                asbd.mChannelsPerFrame.max(1)
            };
            let storage = asbd.mBytesPerFrame / divisor;
            if storage == 0 || storage > 4 {
                return Err("Unsupported signed PCM storage width".into());
            }
            PcmEncoding::Signed {
                bits: asbd.mBitsPerChannel as u8,
                storage: storage as u8,
                high_aligned: asbd.mFormatFlags & kAudioFormatFlagIsAlignedHigh != 0,
            }
        }
        (false, false, 8) => PcmEncoding::Unsigned8,
        _ => return Err("Unsupported PCM sample encoding".into()),
    };
    let bytes_per_sample = encoding.storage_bytes();
    let big_endian = asbd.mFormatFlags & kAudioFormatFlagIsBigEndian != 0;
    let channels = asbd.mChannelsPerFrame as usize;
    let frames = unsafe { sample.num_samples() };
    if channels == 0
        || channels > 32
        || frames <= 0
        || frames as usize > 192000
        || !asbd.mSampleRate.is_finite()
    {
        return Err("Invalid audio sample dimensions/rate".into());
    }
    #[repr(C)]
    struct List {
        count: u32,
        buffers: [AudioBuffer; 32],
    }
    let mut list = List {
        count: 0,
        buffers: [AudioBuffer {
            mNumberChannels: 0,
            mDataByteSize: 0,
            mData: std::ptr::null_mut(),
        }; 32],
    };
    let mut retained = std::ptr::null_mut::<CMBlockBuffer>();
    let status = unsafe {
        sample.audio_buffer_list_with_retained_block_buffer(
            std::ptr::null_mut(),
            (&mut list as *mut List).cast(),
            std::mem::size_of::<List>(),
            None,
            None,
            0,
            &mut retained,
        )
    };
    let _retained = NonNull::new(retained).map(|pointer| unsafe { CFRetained::from_raw(pointer) });
    if status != 0 {
        return Err(format!("CMSampleBuffer PCM access failed ({status})"));
    }
    if list.count == 0 || list.count as usize > 32 {
        return Err("Invalid AudioBufferList count".into());
    }
    let frames = frames as usize;
    let mut samples = vec![0.0f32; frames * channels];
    let mut channel_base = 0;
    for buffer in list.buffers.iter().take(list.count as usize) {
        let count = buffer.mNumberChannels as usize;
        let stride = asbd.mBytesPerFrame as usize;
        if count == 0
            || channel_base + count > channels
            || buffer.mData.is_null()
            || stride < count * bytes_per_sample
            || (buffer.mDataByteSize as usize) < frames * stride
        {
            return Err("AudioBufferList has invalid channel/data layout".into());
        }
        for frame in 0..frames {
            for channel in 0..count {
                let offset = frame * stride + channel * bytes_per_sample;
                let bytes = unsafe {
                    std::slice::from_raw_parts(
                        buffer.mData.cast::<u8>().add(offset),
                        bytes_per_sample,
                    )
                };
                samples[frame * channels + channel_base + channel] = encoding
                    .decode(bytes, big_endian)
                    .map_err(|e| e.to_string())?;
            }
        }
        channel_base += count;
    }
    if channel_base != channels {
        return Err("AudioBufferList channel count differs from its stream format".into());
    }
    Ok(PcmBlock {
        sample_rate: asbd.mSampleRate.round() as u32,
        channels,
        samples,
        input_route,
    })
}
