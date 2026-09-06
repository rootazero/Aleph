// THROWAWAY: two CDP connections on one Chrome — driver (conn A) vs observer (conn B).
// usage: node probe-chrome.mjs <browserWsUrl> <localBase>
import fs from "node:fs";
const [wsUrl, localBase = "http://127.0.0.1:18999"] = process.argv.slice(2);
const sleep=(ms)=>new Promise(r=>setTimeout(r,ms)); const now=()=>performance.now();
class Cdp{constructor(n){this.name=n;this.id=0;this.pending=new Map();this.listeners=[];this.events=[];}
 async connect(){this.ws=new WebSocket(wsUrl);await new Promise((res,rej)=>{this.ws.addEventListener("open",res);this.ws.addEventListener("error",()=>rej(new Error("ws")));});this.ws.addEventListener("message",(ev)=>{const m=JSON.parse(ev.data);if(m.id&&this.pending.has(m.id)){const p=this.pending.get(m.id);this.pending.delete(m.id);m.error?p.reject(new Error(`${this.name} ${p.method}: ${m.error.message}`)):p.resolve(m.result);}else if(m.method){this.events.push({t:now(),method:m.method,sessionId:m.sessionId,params:m.params});for(const l of this.listeners)l(m);}});}
 call(method,params={},sessionId,timeoutMs=30000){return new Promise((resolve,reject)=>{const id=++this.id;const t=setTimeout(()=>{if(this.pending.has(id)){this.pending.delete(id);reject(new Error(`${this.name} ${method}: timeout`));}},timeoutMs);this.pending.set(id,{method,resolve:(v)=>{clearTimeout(t);resolve(v);},reject:(e)=>{clearTimeout(t);reject(e);}});const msg={id,method,params};if(sessionId)msg.sessionId=sessionId;this.ws.send(JSON.stringify(msg));});}
 on(fn){this.listeners.push(fn);return()=>{this.listeners=this.listeners.filter(l=>l!==fn);};}
 waitEvent(method,sessionId,timeoutMs=30000){return new Promise((resolve,reject)=>{const t=setTimeout(()=>{off();reject(new Error(`${this.name} timeout ${method}`));},timeoutMs);const off=this.on((m)=>{if(m.method===method&&(!sessionId||m.sessionId===sessionId)){clearTimeout(t);off();resolve(m.params);}});});}
 close(){try{this.ws.close();}catch{}}}
const out={};const log=(k,v)=>{out[k]=v;console.log(`## ${k}\n${JSON.stringify(v,null,1)}`);};const errStr=(e)=>String(e?.message??e);
const ev=(c,s,expr)=>c.call("Runtime.evaluate",{expression:expr,returnByValue:true},s).then(r=>r.result?.value??r).catch(errStr);
async function navigate(c,s,url,settle=500){const t0=now();const lp=c.waitEvent("Page.loadEventFired",s).catch(e=>({loadWaitError:errStr(e)}));try{await c.call("Page.navigate",{url},s);}catch(e){return{url,navigateError:errStr(e)};}await lp;await sleep(settle);return{url,ms:Math.round(now()-t0)};}
function collector(c,s){const frames=[];c.on(m=>{if(m.method==="Page.screencastFrame"&&m.sessionId===s){frames.push({t:now(),bytes:Math.floor(m.params.data.length*3/4)});c.call("Page.screencastFrameAck",{sessionId:m.params.sessionId},s).catch(()=>{});}});return frames;}
const stats=(f)=>({n:f.length,bytesAvg:f.length?Math.round(f.reduce((a,x)=>a+x.bytes,0)/f.length):0});

// Driver A (stands in for playwright-cli): use an existing page target or create one.
const A=new Cdp("A");await A.connect();
log("version", await A.call("Browser.getVersion").then(v=>v.product).catch(errStr));
let targets=(await A.call("Target.getTargets")).targetInfos.filter(t=>t.type==="page");
let targetId=targets[0]?.targetId; if(!targetId){targetId=(await A.call("Target.createTarget",{url:"about:blank"})).targetId;}
const sA=(await A.call("Target.attachToTarget",{targetId,flatten:true})).sessionId; await A.call("Page.enable",{},sA);
await A.call("Emulation.setDeviceMetricsOverride",{width:1280,height:800,deviceScaleFactor:1,mobile:false},sA).catch(()=>{});
log("A_session",{targetId,sA});

// Observer B: separate connection, attach to the SAME target, screencast only (no interception).
const B=new Cdp("B");await B.connect();
log("B_getTargets", (await B.call("Target.getTargets")).targetInfos.map(t=>({type:t.type,id:t.targetId.slice(0,8),url:t.url.slice(0,40),attached:t.attached})));
const sB=(await B.call("Target.attachToTarget",{targetId,flatten:true})).sessionId; await B.call("Page.enable",{},sB);
log("B_session",{sB});
const fB=collector(B,sB);
log("B_startScreencast", await B.call("Page.startScreencast",{format:"jpeg",quality:60,maxWidth:1280,maxHeight:800,everyNthFrame:1},sB).then(()=>"ok").catch(errStr));

let t=now(); log("A_nav_probe", await navigate(A,sA,`${localBase}/probe.html`,1000)); log("B_frames_during_A_nav", stats(fB.filter(f=>f.t>t)));
t=now(); await sleep(3000); log("B_frames_idle_animation_3s", stats(fB.filter(f=>f.t>t)));
// A drives; does B see it? click->frame latency measured on B.
const lat=[];for(let i=0;i<5;i++){const t0=now();await A.call("Input.dispatchMouseEvent",{type:"mousePressed",x:150,y:100,button:"left",clickCount:1},sA);await A.call("Input.dispatchMouseEvent",{type:"mouseReleased",x:150,y:100,button:"left",clickCount:1},sA);const dl=now()+2000;let f=null;while(!f&&now()<dl){f=fB.find(x=>x.t>t0);if(!f)await sleep(5);}lat.push(f?Math.round(f.t-t0):null);await sleep(200);}
log("A_click_to_B_frame_ms_x5", lat); log("box_count_after_A_clicks", await ev(A,sA,"document.getElementById('box').textContent"));
// Human takeover: B dispatches input on the same page while A holds its session.
log("B_click", await B.call("Input.dispatchMouseEvent",{type:"mousePressed",x:150,y:100,button:"left",clickCount:1},sB).then(()=>B.call("Input.dispatchMouseEvent",{type:"mouseReleased",x:150,y:100,button:"left",clickCount:1},sB)).then(()=>"ok").catch(errStr));
log("box_count_after_B_click", await ev(A,sA,"document.getElementById('box').textContent"));
await B.call("Runtime.enable",{},sB).catch(()=>{});
await ev(B,sB,"(()=>{const i=document.getElementById('name');i.value='';i.focus();return document.activeElement.id})()");
log("B_insertText", await B.call("Input.insertText",{text:"中文 hello"},sB).then(()=>"ok").catch(errStr));
log("value_seen_by_A", await ev(A,sA,"document.getElementById('name').value"));
await B.call("Input.dispatchMouseEvent",{type:"mouseWheel",x:400,y:400,deltaX:0,deltaY:600},sB).catch(e=>log("B_wheelErr",errStr(e)));
await sleep(300); log("scrollY_after_B_wheel", await ev(A,sA,"window.scrollY"));
// Does B observe A's navigations (for the intervention summary)?
const bNavs=[];B.on(m=>{if(m.method==="Page.frameNavigated"&&m.sessionId===sB&&!m.params.frame.parentId)bNavs.push(m.params.frame.url);});
t=now(); log("A_nav_hn", await navigate(A,sA,"https://news.ycombinator.com/",1500)); log("B_saw_frameNavigated", bNavs); log("B_frames_across_nav", stats(fB.filter(f=>f.t>t)));
log("B_screencast_alive_after_nav", fB.length>0 && fB.at(-1).t>t);
// Interference check: A's own evaluate RTT with B attached and streaming
const r0=now(); await ev(A,sA,"1"); log("A_eval_rtt_ms_with_B_streaming", Math.round(now()-r0));
// AX tree with names (what Aleph's snapshot would get from Chrome)
const ax=await A.call("Accessibility.getFullAXTree",{},sA).catch(e=>({err:errStr(e)}));
if(ax.nodes){const links=ax.nodes.filter(n=>n.role?.value==="link");log("chrome_ax",{nodes:ax.nodes.length,links:links.length,linksNamed:links.filter(n=>n.name?.value).length,sample:links.slice(1,4).map(n=>n.name?.value)});}else log("chrome_ax",ax);
// B detaches: A unaffected?
await B.call("Page.stopScreencast",{},sB).catch(()=>{}); await B.call("Target.detachFromTarget",{sessionId:sB}).catch(()=>{}); B.close();
await sleep(300); log("A_alive_after_B_detach", await ev(A,sA,"document.title"));
fs.writeFileSync("results-chrome.json",JSON.stringify(out,null,2)); A.close(); process.exit(0);
