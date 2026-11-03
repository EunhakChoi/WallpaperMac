// Independent native-host bridge. No original Wallpaper Engine JavaScript is bundled.
(() => {
  "use strict";
  const initialProperties = __INITIAL_PROPERTIES__;
  const generalProperties = { fps: 30 };
  const rawRAF = window.requestAnimationFrame.bind(window);
  const rawCancelRAF = window.cancelAnimationFrame.bind(window);
  const rawTimeout = window.setTimeout.bind(window);
  const rawClearTimeout = window.clearTimeout.bind(window);
  const frames = new Map(), timers = new Map(), pausedAnimations = new Set();
  let audioListener = null, muted = __INITIAL_MUTED__, outputVolume = __INITIAL_VOLUME__, pointer = null;
  window.wallpaperRegisterAudioListener = callback => { if(typeof callback !== "function") throw new TypeError("Audio listener must be a function"); audioListener=callback; };
  // Route Web Audio destination connections through a host gain. Author gain
  // nodes keep their own values; muting never replaces the wallpaper's volume.
  const audioRoutes = new WeakMap(), audioContexts = new Set(), mediaMuted = new WeakMap(), mediaVolume = new WeakMap();
  const NodeClass = window.AudioNode;
  if (NodeClass?.prototype?.connect) {
    const connect = NodeClass.prototype.connect, disconnect = NodeClass.prototype.disconnect;
    function route(context) {
      let gain = audioRoutes.get(context);
      if (!gain) { gain = context.createGain(); gain.gain.value = muted ? 0 : outputVolume; connect.call(gain, context.destination); audioRoutes.set(context, gain); audioContexts.add(typeof WeakRef === "function" ? new WeakRef(context) : context); }
      return gain;
    }
    NodeClass.prototype.connect = function(destination, ...args) { const routed=destination === this.context?.destination; const result=connect.call(this, routed ? route(this.context) : destination, ...args); return routed ? destination : result; };
    if (disconnect) NodeClass.prototype.disconnect = function(destination, ...args) { return arguments.length === 0 ? disconnect.call(this) : disconnect.call(this, destination === this.context?.destination ? route(this.context) : destination, ...args); };
  }
  const mediaVolumeDescriptor = window.HTMLMediaElement?.prototype && Object.getOwnPropertyDescriptor(window.HTMLMediaElement.prototype, 'volume');
  if(mediaVolumeDescriptor?.configurable && mediaVolumeDescriptor.get && mediaVolumeDescriptor.set) Object.defineProperty(window.HTMLMediaElement.prototype,'volume',{...mediaVolumeDescriptor,set(value){value=Number(value);if(!Number.isFinite(value)||value<0||value>1)return mediaVolumeDescriptor.set.call(this,value);mediaVolume.set(this,value);return mediaVolumeDescriptor.set.call(this,value*outputVolume);}});
  function applyMediaMute() {
    for (const media of document.querySelectorAll?.("audio,video") || []) {
      if(mediaVolumeDescriptor?.set){if(!mediaVolume.has(media))mediaVolume.set(media,mediaVolumeDescriptor.get.call(media));mediaVolumeDescriptor.set.call(media,mediaVolume.get(media)*outputVolume);}
      if (muted) { if (!mediaMuted.has(media)) mediaMuted.set(media, Boolean(media.muted)); media.muted = true; }
      else if (mediaMuted.has(media)) { media.muted = mediaMuted.get(media); mediaMuted.delete(media); }
    }
  }
  if (typeof MutationObserver === "function") new MutationObserver(applyMediaMute).observe(document, { childList: true, subtree: true });
  let paused = __INITIAL_PAUSED__, nextHandle = 1000000;
  const mediaListeners = new Map(), mediaDelivered = new Map();
  let mediaLatest = { status: {enabled:false} };
  window.wallpaperMediaIntegration = Object.freeze({PLAYBACK_STOPPED:0, PLAYBACK_PLAYING:1, PLAYBACK_PAUSED:2, playback:Object.freeze({STOPPED:0,PLAYING:1,PAUSED:2})});
  function emitMedia(field) {
    const callback=mediaListeners.get(field),value=mediaLatest[field];
    if(paused||!callback||value===undefined)return;
    const key=JSON.stringify(value);if(mediaDelivered.get(field)===key)return;
    let event=JSON.parse(key);
    if(field==='thumbnail')for(const name of ['primaryColor','secondaryColor','tertiaryColor','textColor','highContrastColor'])if(Array.isArray(event[name]))event[name]=`rgb(${event[name].map(v=>Math.round(Math.max(0,Math.min(1,v))*255)).join(',')})`;
    try { callback(event);mediaDelivered.set(field,key); } catch(error) {report('error',error?.stack||error);}
  }
  for(const [name,field]of[['Status','status'],['Properties','properties'],['Thumbnail','thumbnail'],['Playback','playback'],['Timeline','timeline']])window[`wallpaperRegisterMedia${name}Listener`]=callback=>{if(typeof callback!=='function')throw new TypeError('Media listener must be a function');mediaListeners.set(field,callback);mediaDelivered.delete(field);rawTimeout(()=>emitMedia(field),0);};

  let appliedGeneral = null, appliedUser = null, appliedPause = null;
  let generalOwner = null, userOwner = null, pauseOwner = null;
  const report = (type, message) => {
    try { window.webkit.messageHandlers.wpeHost.postMessage(JSON.stringify({ type, message: String(message) })); } catch (_) {}
  };
  function scheduleFrame(id, frame) {
    frame.real = rawRAF(timestamp => {
      frame.real = null;
      if (paused || !frames.has(id)) return;
      frames.delete(id);
      Reflect.apply(frame.callback, window, [timestamp]);
    });
  }
  window.requestAnimationFrame = callback => {
    const id = nextHandle++, frame = { callback, real: null };
    frames.set(id, frame);
    if (!paused) scheduleFrame(id, frame);
    return id;
  };
  window.cancelAnimationFrame = id => {
    const frame = frames.get(id);
    if (frame && frame.real !== null) rawCancelRAF(frame.real);
    frames.delete(id);
  };
  function scheduleTimer(id, timer) {
    timer.due = performance.now() + timer.remaining;
    timer.real = rawTimeout(() => {
      timer.real = null;
      if (paused || !timers.has(id)) return;
      if (!timer.repeat) timers.delete(id);
      try {
        if (typeof timer.callback === "function") Reflect.apply(timer.callback, window, timer.args);
        else window.eval(String(timer.callback));
      } finally {
        if (timer.repeat && timers.has(id)) {
          timer.remaining = timer.delay;
          if (!paused) scheduleTimer(id, timer);
        }
      }
    }, timer.remaining);
  }
  function addTimer(callback, delay, repeat, args) {
    const id = nextHandle++;
    delay = Math.max(0, Number(delay) || 0);
    const timer = { callback, delay, remaining: delay, repeat, args, real: null, due: 0 };
    timers.set(id, timer);
    if (!paused) scheduleTimer(id, timer);
    return id;
  }
  window.setTimeout = (callback, delay, ...args) => addTimer(callback, delay, false, args);
  window.setInterval = (callback, delay, ...args) => addTimer(callback, delay, true, args);
  window.clearTimeout = window.clearInterval = id => {
    const timer = timers.get(id);
    if (timer && timer.real !== null) rawClearTimeout(timer.real);
    timers.delete(id);
  };
  function applyInitial() {
    const listener = window.wallpaperPropertyListener;
    if (!listener) return;
    try {
      if (typeof listener.applyGeneralProperties === "function" && (generalOwner !== listener || appliedGeneral !== listener.applyGeneralProperties)) {
        listener.applyGeneralProperties(generalProperties); appliedGeneral = listener.applyGeneralProperties; generalOwner = listener;
      }
      if (typeof listener.applyUserProperties === "function" && (userOwner !== listener || appliedUser !== listener.applyUserProperties)) {
        listener.applyUserProperties(initialProperties); appliedUser = listener.applyUserProperties; userOwner = listener;
      }
      if (typeof listener.setPaused === "function" && (pauseOwner !== listener || appliedPause !== listener.setPaused)) {
        listener.setPaused(paused); appliedPause = listener.setPaused; pauseOwner = listener;
      }
    } catch (error) { report("error", error && error.stack || error); }
  }
  // Assignments can happen after load. Preserve the actual listener object and its
  // prototype; the native heartbeat also observes methods added to that object later.
  const originalDescriptor = Object.getOwnPropertyDescriptor(window, "wallpaperPropertyListener");
  if (!originalDescriptor || (originalDescriptor.configurable && !originalDescriptor.get && !originalDescriptor.set && originalDescriptor.writable)) {
    let listener = window.wallpaperPropertyListener;
    Object.defineProperty(window, "wallpaperPropertyListener", {
      configurable: true, enumerable: originalDescriptor ? originalDescriptor.enumerable : true,
      get() { return listener; },
      set(value) { listener = value; rawTimeout(applyInitial, 0); }
    });
  }
  window.__wpeNativeHost = {
    applyInitial,
    media(value) { if(!value||typeof value!=='object')return;mediaLatest=value;for(const field of mediaListeners.keys())emitMedia(field); },
    audio(bands) {
      if (paused || !audioListener) return;
      if (!Array.isArray(bands) || bands.length !== 128 || !bands.every(Number.isFinite)) { report("error", "Invalid native stereo spectrum"); return; }
      try { audioListener(bands.slice()); } catch(error) { report("error", error?.stack || error); }
    },
    input(x,y,buttons,active=true) {
      if(paused || !Number.isFinite(x) || !Number.isFinite(y)) return;
      const previous=pointer?.buttons||0;
      if(!active){if(pointer?.active){pointer.target?.dispatchEvent?.(new MouseEvent("mouseleave",{bubbles:false}));}pointer={x,y,buttons,active:false,target:null,down:null};return;}
      if(pointer?.active&&pointer.x===x&&pointer.y===y&&previous===buttons)return;
      const px=x*window.innerWidth, py=y*window.innerHeight;
      const target=document.elementFromPoint?.(px,py)||document;
      const send=(type,button=0)=>target.dispatchEvent(new MouseEvent(type,{clientX:px,clientY:py,buttons,button,bubbles:true}));
      if(!pointer?.active)send("mouseenter");send("mousemove");let down=pointer?.down;
      for(let button=0;button<3;button++){const mask=[1,4,2][button];if((buttons&mask)!==(previous&mask)){send(buttons&mask?"mousedown":"mouseup",button);if(button===0){if(buttons&mask)down=target;else if(down===target)send("click",button);}}}
      pointer={x,y,buttons,active:true,target,down};
    },
    setVolume(value) { if(!Number.isFinite(value)||value<0||value>1){report("error","Invalid native playback volume");return;}outputVolume=value;this.setMuted(muted); },
    setMuted(value) { muted=Boolean(value); applyMediaMute(); for(const reference of audioContexts) { const context=reference.deref ? reference.deref() : reference; if(!context){audioContexts.delete(reference);continue;} audioRoutes.get(context).gain.value=muted ? 0 : outputVolume; } },
    updateGeneralProperties(properties) {const changed=Object.keys(properties).some(key=>generalProperties[key]!==properties[key]);Object.assign(generalProperties,properties);const listener=window.wallpaperPropertyListener;if(changed&&typeof listener?.applyGeneralProperties==="function")listener.applyGeneralProperties(generalProperties);},
    updateProperties(properties) {
      Object.assign(initialProperties, properties);
      const listener = window.wallpaperPropertyListener;
      if (listener && typeof listener.applyUserProperties === "function") listener.applyUserProperties(properties);
    },
    setPaused(value) {
      value = Boolean(value);
      if (paused === value) return;
      paused = value;
      if (paused) {
        for (const frame of frames.values()) {
          if (frame.real !== null) rawCancelRAF(frame.real);
          frame.real = null;
        }
        const now = performance.now();
        for (const timer of timers.values()) {
          if (timer.real !== null) {
            timer.remaining = Math.max(0, timer.due - now);
            rawClearTimeout(timer.real);
            timer.real = null;
          }
        }
        for (const animation of document.getAnimations()) {
          if (animation.playState === "running") { animation.pause(); pausedAnimations.add(animation); }
        }
      } else {
        for (const [id, frame] of frames) if (frame.real === null) scheduleFrame(id, frame);
        for (const [id, timer] of timers) if (timer.real === null) scheduleTimer(id, timer);
        for (const animation of pausedAnimations) animation.play();
        pausedAnimations.clear();
        for(const field of mediaListeners.keys())emitMedia(field);
      }
      const listener = window.wallpaperPropertyListener;
      if (listener && typeof listener.setPaused === "function") listener.setPaused(paused);
    }
  };
  window.addEventListener("DOMContentLoaded", applyInitial, { once: true });
  window.addEventListener("load", applyInitial, { once: true });
  window.addEventListener("error", event => report("error", event.message));
  window.addEventListener("unhandledrejection", event => report("error", event.reason));
})();
