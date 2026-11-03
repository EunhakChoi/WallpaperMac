//! Opt-in AppleEvents metadata adapter. No private MediaRemote framework.
//! The fixed JXA scripts only read a running Music/Spotify instance; the host
//! checks running bundle IDs first. The process is bounded and runs off-main.
use super::*;
use objc2::ClassType;
use objc2_foundation::{NSData, NSHTTPURLResponse, NSURLRequest, NSURLResponse, NSURLSession};
use std::{
    io::Read,
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use wpe_platform::media::{MediaSettings, MediaSource};
struct Shared {
    value: Value,
    error: Option<String>,
    artwork_url: Option<String>,
    polls: u64,
}
pub struct MediaMonitor {
    settings: MediaSettings,
    shared: Arc<Mutex<Shared>>,
    busy: Arc<AtomicBool>,
    last: Instant,
}
impl MediaMonitor {
    pub fn new(settings: MediaSettings) -> Self {
        Self {
            shared: Arc::new(Mutex::new(Shared {
                value: wpe_platform::media::unavailable(settings.source != MediaSource::Disabled),
                error: None,
                artwork_url: None,
                polls: 0,
            })),
            settings,
            busy: Arc::new(AtomicBool::new(false)),
            last: Instant::now() - Duration::from_secs(61),
        }
    }
    pub fn configure(&mut self, settings: MediaSettings) {
        if self.settings != settings {
            *self = Self::new(settings);
        }
    }
    pub fn set_owned(&self, value: Value) {
        if self.settings.source == MediaSource::Owned
            && let Ok(mut state) = self.shared.lock()
        {
            state.value = value;
        }
    }
    pub fn value(&self) -> Value {
        self.shared
            .lock()
            .map(|s| s.value.clone())
            .unwrap_or_else(|_| wpe_platform::media::unavailable(false))
    }
    pub fn status(&self) -> Value {
        self.shared.lock().map(|s|json!({"settings":self.settings,"error":s.error,"polls":s.polls,"transport":"public JXA/AppleEvents; exact installed scripting dictionaries and TCC require Mac validation"})).unwrap_or(Value::Null)
    }
    pub fn poll(&mut self) {
        if !matches!(
            self.settings.source,
            MediaSource::Music | MediaSource::Spotify
        ) || self.last.elapsed() < Duration::from_secs(self.settings.poll_seconds as u64)
            || self.busy.swap(true, Ordering::AcqRel)
        {
            return;
        }
        self.last = Instant::now();
        let source = self.settings.source;
        let bundle = if source == MediaSource::Music {
            "com.apple.Music"
        } else {
            "com.spotify.client"
        };
        if !NSWorkspace::sharedWorkspace()
            .runningApplications()
            .iter()
            .any(|app| {
                app.bundleIdentifier()
                    .is_some_and(|id| id.to_string() == bundle)
            })
        {
            if let Ok(mut s) = self.shared.lock() {
                s.value = wpe_platform::media::unavailable(true);
                s.error = None;
                s.artwork_url = None;
            }
            self.busy.store(false, Ordering::Release);
            return;
        }
        let shared = self.shared.clone();
        let busy = self.busy.clone();
        let artwork = self.settings.artwork;
        if let Err(error) = std::thread::Builder::new()
            .name("wallpaper-media-metadata".into())
            .spawn(move || {
                struct Done(Arc<AtomicBool>);
                impl Drop for Done {
                    fn drop(&mut self) {
                        self.0.store(false, Ordering::Release);
                    }
                }
                let _done = Done(busy);
                let result = read_metadata(source);
                let mut fetch = None;
                if let Ok(mut state) = shared.lock() {
                    state.polls += 1;
                    match result {
                        Ok(transport) => {
                            match wpe_platform::media::from_transport(&transport, true) {
                                Ok(mut value) => {
                                    let url = artwork
                                        .then(|| {
                                            transport["artwork_url"]
                                                .as_str()
                                                .filter(|s| !s.is_empty())
                                                .map(str::to_owned)
                                        })
                                        .flatten();
                                    if url == state.artwork_url {
                                        value["thumbnail"] = state.value["thumbnail"].clone();
                                    } else {
                                        state.artwork_url = url.clone();
                                        fetch = url;
                                    }
                                    state.value = value;
                                    state.error = None;
                                }
                                Err(error) => {
                                    state.value = wpe_platform::media::unavailable(true);
                                    state.error = Some(error.to_string());
                                    state.artwork_url = None;
                                }
                            }
                        }
                        Err(error) => {
                            state.value = wpe_platform::media::unavailable(true);
                            state.error = Some(error);
                            state.artwork_url = None;
                        }
                    }
                }
                if let Some(url) = fetch {
                    fetch_artwork(shared, &url);
                }
            })
        {
            self.busy.store(false, Ordering::Release);
            if let Ok(mut state) = self.shared.lock() {
                state.error = Some(error.to_string());
            }
        }
    }
}
fn read_metadata(source: MediaSource) -> Result<Value, String> {
    let bundle = if source == MediaSource::Music {
        "com.apple.Music"
    } else {
        "com.spotify.client"
    };
    let divisor = if source == MediaSource::Spotify {
        1000
    } else {
        1
    };
    // No caller-controlled text is interpolated. Script dictionaries may vary by
    // installed app; failures are reported, never replaced by a synthetic song.
    let script = format!(
        r#"(()=>{{const app=Application('{bundle}');if(!app.running())return JSON.stringify({{available:false}});const state=String(app.playerState()).toLowerCase();if(state==='stopped')return JSON.stringify({{available:false,state}});const track=app.currentTrack();const get=(key,fallback='')=>{{try{{const value=track[key]();return value===null?fallback:value;}}catch(error){{return fallback;}}}};return JSON.stringify({{available:true,state,title:String(get('name')),artist:String(get('artist')),album:String(get('album')),album_artist:String(get('albumArtist')),genre:String(get('genre')),duration:Number(get('duration',0))/{divisor},position:Number(app.playerPosition()),artwork_url:String(get('artworkUrl'))}});}})()"#
    );
    let mut child = Command::new("/usr/bin/osascript")
        .args(["-l", "JavaScript", "-e", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Could not start public automation helper: {e}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or("Automation output pipe missing")?;
    let stderr = child.stderr.take().ok_or("Automation error pipe missing")?;
    let out = std::thread::spawn(move || {
        let mut data = vec![];
        let result = stdout.take(262145).read_to_end(&mut data);
        (result, data)
    });
    let err = std::thread::spawn(move || {
        let mut data = vec![];
        let _ = stderr.take(16384).read_to_end(&mut data);
        data
    });
    let deadline = Instant::now() + Duration::from_millis(2500);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(
                    "Media automation timed out after2.5s; check Automation permission".to_string(),
                );
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(error.to_string());
            }
        }
    };
    let stdout = out.join().map_err(|_| "Automation output reader failed")?;
    let stderr = err.join().map_err(|_| "Automation error reader failed")?;
    let status = status?;
    stdout.0.map_err(|e| e.to_string())?;
    if !status.success() {
        return Err(format!(
            "Media automation failed: {}",
            String::from_utf8_lossy(&stderr)
        ));
    }
    if stdout.1.len() > 262144 {
        return Err("Media metadata exceeds256KiB".into());
    }
    serde_json::from_slice(&stdout.1).map_err(|e| format!("Invalid automation metadata: {e}"))
}
fn fetch_artwork(shared: Arc<Mutex<Shared>>, text: &str) {
    let Some(url) = NSURL::URLWithString(&NSString::from_str(text)) else {
        return;
    };
    let host = url
        .host()
        .map(|s| s.to_string().to_lowercase())
        .unwrap_or_default();
    if url.scheme().is_none_or(|s| s.to_string() != "https")
        || !(host.ends_with(".scdn.co") || host.ends_with(".spotifycdn.com"))
    {
        return;
    }
    let request = NSURLRequest::requestWithURL(&url);
    let request: Retained<objc2_foundation::NSMutableURLRequest> =
        unsafe { msg_send![&*request, mutableCopy] };
    request.setTimeoutInterval(5.);
    let expected = text.to_owned();
    let complete = block2::RcBlock::new(
        move |data: *mut NSData, response: *mut NSURLResponse, error: *mut NSError| {
            let result = (|| -> Result<Value, String> {
                if let Some(error) = unsafe { error.as_ref() } {
                    return Err(error.localizedDescription().to_string());
                }
                let response = unsafe { response.as_ref() }.ok_or("Artwork response missing")?;
                if !response.isKindOfClass(NSHTTPURLResponse::class()) {
                    return Err("Artwork response is not HTTP".into());
                }
                let status: isize = unsafe { msg_send![response, statusCode] };
                if !(200..300).contains(&status) {
                    return Err(format!("Artwork HTTP{status}"));
                }
                let data = unsafe { data.as_ref() }.ok_or("Artwork response has no bytes")?;
                if data.length() > 5 * 1024 * 1024 {
                    return Err("Artwork response exceeds5MiB".into());
                }
                wpe_platform::media::thumbnail(&data.to_vec()).map_err(|e| e.to_string())
            })();
            if let Ok(mut state) = shared.lock()
                && state.artwork_url.as_deref() == Some(expected.as_str())
            {
                match result {
                    Ok(value) => state.value["thumbnail"] = value,
                    Err(error) => state.error = Some(error),
                }
            }
        },
    );
    let task = unsafe {
        NSURLSession::sharedSession().dataTaskWithRequest_completionHandler(&request, &complete)
    };
    task.resume();
}
