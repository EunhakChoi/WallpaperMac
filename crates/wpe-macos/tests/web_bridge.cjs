"use strict";
// Executes the actual shipped independent bridge. The host timer models AppKit's
// existing 1Hz property refresh; this is a Node VM test, not a WebKit/Mac test.
const assert = require("node:assert/strict");
const fs = require("node:fs");
const vm = require("node:vm");
const path = require("node:path");
const source = fs.readFileSync(path.join(__dirname,"../src/native/web_bridge.js"),"utf8");
const properties = { speed: { value: 2 }, text: { value: '"};throw new Error("injected");//' } };

function fixture(initialPaused = false) {
  let time = 0, next = 1, heartbeat = 1000;
  const timers = new Map(), frames = new Map(), events = new Map(), messages = [];
  const calls = { properties: [], general: [], paused: [], dom: [] };
  const target = { dispatchEvent(event) { calls.dom.push(event); } };
  class AudioNode { constructor(context) { this.context=context; this.connections=[]; } connect(destination) { this.connections.push(destination); return destination; } disconnect(destination) { this.connections = arguments.length ? this.connections.filter(d=>d!==destination) : []; } }
  class AudioContext { constructor() { this.destination = new AudioNode(this); } createGain() { const node = new AudioNode(this); node.gain={value:1}; return node; } }
  const context = {
    performance: { now: () => time },
    document: { getAnimations: () => [], elementFromPoint: () => target, querySelectorAll: () => [] },
    innerWidth: 800, innerHeight: 600,
    MouseEvent: class { constructor(type, values) { this.type=type;Object.assign(this,values); } }, AudioNode, AudioContext,
    setTimeout(callback,delay) { const id=next++; timers.set(id,{at:time+Number(delay),callback});return id; },
    clearTimeout(id) {timers.delete(id);},
    requestAnimationFrame(callback) {const id=next++;frames.set(id,callback);return id;},
    cancelAnimationFrame(id) {frames.delete(id);},
    addEventListener(name,callback) {if(!events.has(name))events.set(name,[]);events.get(name).push(callback);},
    webkit: {messageHandlers:{wpeHost:{postMessage(message){messages.push(JSON.parse(message));}}}},
  };
  context.window=context;
  vm.createContext(context);
  vm.runInContext(source.replace("__INITIAL_PROPERTIES__",JSON.stringify(properties)).replace("__INITIAL_PAUSED__",JSON.stringify(initialPaused)).replace("__INITIAL_MUTED__","false").replace("__INITIAL_VOLUME__","1"),context);
  function advance(to) {
    let count=0;
    while (true) {
      if (++count>1000) throw new Error("unbounded timer loop in test");
      let first=null;
      for (const [id,timer] of timers) if (timer.at<=to && (!first||timer.at<first[1].at))first=[id,timer];
      if (heartbeat<=to && (!first||heartbeat<first[1].at)) {
        time=heartbeat;heartbeat+=1000;context.__wpeNativeHost.applyInitial();continue;
      }
      if (!first)break;
      time=first[1].at;timers.delete(first[0]);first[1].callback();
    }
    time=to;
  }
  const fire=name=>{for(const callback of events.get(name)||[])callback({});};
  const draw=()=>{const ready=[...frames.values()];frames.clear();for(const callback of ready)callback(time);};
  const listener=()=>({
    applyUserProperties(p){calls.properties.push(JSON.parse(JSON.stringify(p)));},
    applyGeneralProperties(p){calls.general.push(JSON.parse(JSON.stringify(p)));},
    setPaused(p){calls.paused.push(p);},
  });
  return {context,calls,messages,advance,fire,draw,listener,target};
}

let checks=0;
function test(name,run){run();checks++;process.stdout.write(`PASS ${name}\n`);}

test("listener assigned after both DOM and load receives initial properties",()=>{
  const f=fixture();f.fire("DOMContentLoaded");f.fire("load");
  f.context.wallpaperPropertyListener=f.listener();f.advance(0);
  assert.deepEqual(f.calls.properties,[properties]);assert.deepEqual(f.calls.general,[{fps:30}]);
});
test("same empty listener gains methods late through native heartbeat",()=>{
  const f=fixture();const object={};f.context.wallpaperPropertyListener=object;f.advance(0);f.fire("load");
  f.advance(2500);Object.assign(object,f.listener());f.advance(3000);
  assert.deepEqual(f.calls.properties,[properties]);
});
test("callbacks run once per receiver and function, not once per DOM event",()=>{
  const f=fixture();const listener=f.listener();f.context.wallpaperPropertyListener=listener;f.advance(0);
  f.fire("DOMContentLoaded");f.fire("load");f.advance(3000);
  assert.equal(f.calls.properties.length,1);
  f.context.wallpaperPropertyListener={...listener};f.advance(3000);
  assert.equal(f.calls.properties.length,2);
});
test("pause holds pending requestAnimationFrame, resume fires once",()=>{
  const f=fixture();let ticks=0;f.context.requestAnimationFrame(()=>ticks++);
  f.context.__wpeNativeHost.setPaused(true);f.draw();assert.equal(ticks,0);
  f.context.__wpeNativeHost.setPaused(false);f.draw();f.draw();assert.equal(ticks,1);
});
test("cancelled paused animation frame stays cancelled after resume",()=>{
  const f=fixture();f.context.__wpeNativeHost.setPaused(true);let ticks=0;
  const id=f.context.requestAnimationFrame(()=>ticks++);f.context.cancelAnimationFrame(id);
  f.context.__wpeNativeHost.setPaused(false);f.draw();assert.equal(ticks,0);
});
test("timeout preserves remaining delay while paused",()=>{
  const f=fixture();let ticks=0;f.context.setTimeout(()=>ticks++,100);f.advance(30);
  f.context.__wpeNativeHost.setPaused(true);f.advance(300);assert.equal(ticks,0);
  f.context.__wpeNativeHost.setPaused(false);f.advance(369);assert.equal(ticks,0);
  f.advance(370);assert.equal(ticks,1);
});
test("interval resumes once and can cancel itself",()=>{
  const f=fixture();let ticks=0;const id=f.context.setInterval(()=>{ticks++;if(ticks===2)f.context.clearInterval(id);},20);
  f.advance(10);f.context.__wpeNativeHost.setPaused(true);f.advance(100);
  f.context.__wpeNativeHost.setPaused(false);f.advance(110);assert.equal(ticks,1);
  f.advance(180);assert.equal(ticks,2);
});
test("property callback exception is delivered as a real runtime error",()=>{
  const f=fixture();f.context.wallpaperPropertyListener={applyUserProperties(){throw new Error("fixture failure");}};
  f.advance(0);assert.equal(f.messages[0].type,"error");assert.match(f.messages[0].message,/fixture failure/);
});
test("native property update sends changed values",()=>{
  const f=fixture();f.context.wallpaperPropertyListener=f.listener();f.advance(0);
  f.context.__wpeNativeHost.updateProperties({speed:{value:7}});
  assert.deepEqual(f.calls.properties.at(-1),{speed:{value:7}});
});
test("timer and animation callbacks retain browser window receiver",()=>{
  const f=fixture();f.context.fixtureReceiver=731;const seen=[];
  f.context.setTimeout(function(value){seen.push([this.fixtureReceiver,value]);},5,9);
  f.context.requestAnimationFrame(function(){seen.push([this.fixtureReceiver,10]);});
  f.draw();f.advance(5);assert.deepEqual(seen,[[731,10],[731,9]]);
});

test("native stereo callback preserves left64 then right64 and pauses",()=>{
 const f=fixture(); const arrays=[];f.context.wallpaperRegisterAudioListener(v=>arrays.push(v));
 const bands=Array.from({length:128},(_,i)=>i/128);f.context.__wpeNativeHost.audio(bands);assert.deepEqual(Array.from(arrays[0]),bands);
 f.context.__wpeNativeHost.setPaused(true);f.context.__wpeNativeHost.audio(bands);assert.equal(arrays.length,1);
 f.context.__wpeNativeHost.setPaused(false);f.context.__wpeNativeHost.audio([1]);assert.equal(arrays.length,1);assert.match(f.messages.at(-1).message,/Invalid native stereo/);
});
test("desktop input gates foreign windows without synthesizing held-button reentry",()=>{
 const f=fixture(), host=f.context.__wpeNativeHost;
 host.input(.5,.5,1,false);host.input(.5,.5,1,true);host.input(.5,.5,0,true);
 assert.equal(f.calls.dom.filter(e=>e.type==='mousedown').length,0);assert.equal(f.calls.dom.filter(e=>e.type==='click').length,0);
 host.input(.5,.5,1,true);host.input(.5,.5,1,false);host.input(.5,.5,0,true);assert.equal(f.calls.dom.filter(e=>e.type==='click').length,0);
 host.input(.5,.5,1,true);host.input(.5,.5,0,true);assert.equal(f.calls.dom.filter(e=>e.type==='click').length,1);
});
test("AppKit right and middle button bits map to DOM buttons",()=>{
 const f=fixture(),host=f.context.__wpeNativeHost;host.input(.2,.3,0,true);host.input(.2,.3,2,true);host.input(.2,.3,0,true);host.input(.2,.3,4,true);
 assert.deepEqual(f.calls.dom.filter(e=>e.type==='mousedown').map(e=>e.button),[2,1]);
});
test("initial pause exists before page schedules timers or animation",()=>{
 const f=fixture(true);let ticks=0;f.context.setTimeout(()=>ticks++,5);f.context.requestAnimationFrame(()=>ticks++);f.advance(100);f.draw();assert.equal(ticks,0);
 f.context.__wpeNativeHost.setPaused(false);f.draw();f.advance(105);assert.equal(ticks,2);
});
test("mute routes existing and late Web Audio connections without replacing author gain",()=>{
 const f=fixture(),ctx=new f.context.AudioContext(),source=new f.context.AudioNode(ctx);const author=ctx.createGain();author.gain.value=.35;
 source.connect(author);assert.equal(author.connect(ctx.destination),ctx.destination);const hostGain=author.connections[0];assert.notEqual(hostGain,ctx.destination);
 f.context.__wpeNativeHost.setMuted(true);assert.equal(hostGain.gain.value,0);assert.equal(author.gain.value,.35);
 const later=new f.context.AudioContext(),lateSource=new f.context.AudioNode(later);lateSource.connect(later.destination);assert.equal(lateSource.connections[0].gain.value,0);
 f.context.__wpeNativeHost.setMuted(false);assert.equal(hostGain.gain.value,1);author.disconnect(ctx.destination);assert.equal(author.connections.length,0);
});

test("media adapters deliver changed actual payloads, late registration and CSS artwork colors",()=>{
 const f=fixture(),events=[];f.context.wallpaperRegisterMediaStatusListener(e=>events.push(['status',e.enabled]));f.advance(0);assert.deepEqual(events,[['status',false]]);
 const data={status:{enabled:true},properties:{title:'Fixture'},playback:{state:1},thumbnail:{hasThumbnail:true,thumbnail:'data:image/png;base64,fixture',primaryColor:[1,0,.5]},timeline:{position:3,duration:20}};
 f.context.__wpeNativeHost.media(data);f.context.__wpeNativeHost.media(data);assert.deepEqual(events,[['status',false],['status',true]]);
 const thumbs=[];f.context.wallpaperRegisterMediaThumbnailListener(e=>thumbs.push(e));f.advance(0);assert.equal(thumbs[0].primaryColor,'rgb(255,0,128)');assert.equal(thumbs[0].thumbnail,data.thumbnail.thumbnail);
 const positions=[];f.context.wallpaperRegisterMediaTimelineListener(e=>positions.push(e.position));f.advance(0);f.context.__wpeNativeHost.setPaused(true);f.context.__wpeNativeHost.media({...data,timeline:{position:8,duration:20}});assert.deepEqual(positions,[3]);f.context.__wpeNativeHost.setPaused(false);assert.deepEqual(positions,[3,8]);
 assert.equal(f.context.wallpaperMediaIntegration.PLAYBACK_PLAYING,1);
});

test("host playback volume composes with author gain and survives mute",()=>{const f=fixture(),ctx=new f.context.AudioContext(),author=ctx.createGain();author.gain.value=.25;author.connect(ctx.destination);const output=author.connections[0];f.context.__wpeNativeHost.setVolume(.4);assert.equal(output.gain.value,.4);assert.equal(author.gain.value,.25);f.context.__wpeNativeHost.setMuted(true);assert.equal(output.gain.value,0);f.context.__wpeNativeHost.setMuted(false);assert.equal(output.gain.value,.4);});
process.stdout.write(`${checks} bridge regression checks passed; macOS/WebKit execution 0.\n`);
