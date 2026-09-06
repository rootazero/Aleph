// THROWAWAY probe 2: single-session latency, idle frame behaviour, typing paths, cross-connection discovery (B connects first).
import fs from "node:fs";
const [port = "9333", localBase = "http://127.0.0.1:18999"] = process.argv.slice(2);
const WS = `ws://127.0.0.1:${port}/devtools/browser`;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms)); const now = () => performance.now();
class Cdp { constructor(n){this.name=n;this.id=0;this.pending=new Map();this.listeners=[];this.events=[];}
  async connect(){this.ws=new WebSocket(WS);await new Promise((res,rej)=>{this.ws.addEventListener("open",res);this.ws.addEventListener("error",()=>rej(new Error("ws error")));});
    this.ws.addEventListener("message",(ev)=>{const m=JSON.parse(ev.data);if(m.id&&this.pending.has(m.id)){const p=this.pending.get(m.id);this.pending.delete(m.id);m.error?p.reject(new Error(`${this.name} ${p.method}: ${m.error.message}`)):p.resolve(m.result);}else if(m.method){this.events.push({t:now(),method:m.method,params:m.params});for(const l of this.listeners)l(m);}});}
  call(method,params={},sessionId,timeoutMs=20000){return new Promise((resolve,reject)=>{const id=++this.id;const t=setTimeout(()=>{if(this.pending.has(id)){this.pending.delete(id);reject(new Error(`${this.name} ${method}: timeout`));}},timeoutMs);this.pending.set(id,{method,resolve:(v)=>{clearTimeout(t);resolve(v);},reject:(e)=>{clearTimeout(t);reject(e);}});const msg={id,method,params};if(sessionId)msg.sessionId=sessionId;this.ws.send(JSON.stringify(msg));});}
  on(fn){this.listeners.push(fn);return()=>{this.listeners=this.listeners.filter(l=>l!==fn);};}
  waitEvent(method,sessionId,timeoutMs=15000){return new Promise((resolve,reject)=>{const t=setTimeout(()=>{off();reject(new Error(`timeout ${method}`));},timeoutMs);const off=this.on((m)=>{if(m.method===method&&(!sessionId||m.sessionId===sessionId)){clearTimeout(t);off();resolve(m.params);}});});}
  close(){try{this.ws.close();}catch{}} }
const out={}; const log=(k,v)=>{out[k]=v;console.log(`## ${k}\n${JSON.stringify(v,null,1)}`);}; const errStr=(e)=>String(e?.message??e);
const ev=(c,s,expr)=>c.call("Runtime.evaluate",{expression:expr,returnByValue:true},s).then(r=>r.result?.value ?? r).catch(errStr);
async function navigate(c,s,url,settle=800){const t0=now();const lp=c.waitEvent("Page.loadEventFired",s,25000).catch(e=>({loadWaitError:errStr(e)}));let nav;try{nav=await c.call("Page.navigate",{url},s,30000);}catch(e){return{url,navigateError:errStr(e)};}await lp;await sleep(settle);return{url,ms:Math.round(now()-t0)};}

// B connects FIRST and asks to discover targets, then A creates one.
const B=new Cdp("B"); await B.connect();
log("B_setDiscoverTargets", await B.call("Target.setDiscoverTargets",{discover:true}).then(()=>"ok").catch(errStr));
const A=new Cdp("A"); await A.connect();
const {targetId}=await A.call("Target.createTarget",{url:"about:blank"}); const sA=`${targetId}-session`; await A.call("Page.enable",{},sA);
await A.call("Emulation.setDeviceMetricsOverride",{width:1280,height:800,deviceScaleFactor:1,mobile:false},sA).catch(()=>{});
await sleep(300);
log("B_targetCreated_events_seen", B.events.filter(e=>e.method.startsWith("Target.")).map(e=>e.method));
log("B_getTargets_after_A_created", await B.call("Target.getTargets").then(r=>r.targetInfos?.map(t=>t.targetId)).catch(errStr));
log("B_attach_A_target", await B.call("Target.attachToTarget",{targetId,flatten:true}).then(r=>r).catch(errStr));
log("A_getTargets", await A.call("Target.getTargets").then(r=>r.targetInfos?.map(t=>({id:t.targetId,type:t.type}))).catch(errStr));

// Runtime.evaluate shape quirks
log("eval_multi_statement", await ev(A,sA,"1; 2"));
log("eval_comma_expr", await ev(A,sA,"(1, 2)"));
log("eval_iife", await ev(A,sA,"(()=>{let a=1; a+=1; return a})()"));
log("eval_await_promise", await A.call("Runtime.evaluate",{expression:"Promise.resolve(41).then(v=>v+1)",awaitPromise:true,returnByValue:true},sA).then(r=>r.result?.value ?? r).catch(errStr));

// Idle behaviour on a static page: does screencast stop when nothing changes?
log("nav_example", await navigate(A,sA,"https://example.com/"));
const frames=[]; A.on(m=>{if(m.method==="Page.screencastFrame"&&m.sessionId===sA){frames.push({t:now(),bytes:Math.floor(m.params.data.length*3/4)});A.call("Page.screencastFrameAck",{sessionId:m.params.sessionId},sA).catch(()=>{});}});
await A.call("Page.startScreencast",{format:"jpeg",quality:60,maxWidth:1280,maxHeight:800},sA);
let t=now(); await sleep(3000); log("frames_static_page_3s", frames.filter(f=>f.t>t).length);
// quality/size sweep on the same static frame
for (const q of [30,60,90]) { const s=await A.call("Page.captureScreenshot",{format:"jpeg",quality:q},sA); out[`jpeg_q${q}_KB`]=Math.round(s.data.length*3/4/1024); }
const p=await A.call("Page.captureScreenshot",{format:"png"},sA); out.png_KB=Math.round(p.data.length*3/4/1024); log("frame_sizes", {jpeg_q30:out.jpeg_q30_KB,jpeg_q60:out.jpeg_q60_KB,jpeg_q90:out.jpeg_q90_KB,png:out.png_KB});

// Click -> next frame latency (single session), 5 samples on the local animated page
log("nav_probe", await navigate(A,sA,`${localBase}/probe.html`));
const lat=[];
for (let i=0;i<5;i++){ const t0=now(); await A.call("Input.dispatchMouseEvent",{type:"mousePressed",x:150,y:100,button:"left",clickCount:1},sA); await A.call("Input.dispatchMouseEvent",{type:"mouseReleased",x:150,y:100,button:"left",clickCount:1},sA); const dl=now()+2000; let f=null; while(!f&&now()<dl){f=frames.find(x=>x.t>t0); if(!f) await sleep(5);} lat.push(f?Math.round(f.t-t0):null); await sleep(250);}
log("click_to_next_frame_ms_x5", lat); log("box_count", await ev(A,sA,"document.getElementById('box').textContent"));
t=now(); await sleep(2000); const fa=frames.filter(f=>f.t>t); log("frames_animated_page_2s", {n:fa.length, fps:Math.round(fa.length/2), bytesAvg:fa.length?Math.round(fa.reduce((a,f)=>a+f.bytes,0)/fa.length):0});
await A.call("Page.stopScreencast",{},sA).catch(()=>{});

// Typing paths (focus via single expression)
log("focused", await ev(A,sA,"(()=>{const i=document.getElementById('name');i.value='';i.focus();return document.activeElement.id})()"));
log("insertText", await A.call("Input.insertText",{text:"中文"},sA).then(()=>"ok").catch(errStr));
for (const ch of "ab") { await A.call("Input.dispatchKeyEvent",{type:"keyDown",key:ch,code:"Key"+ch.toUpperCase(),text:ch,unmodifiedText:ch},sA).catch(e=>log("keyDownErr",errStr(e))); await A.call("Input.dispatchKeyEvent",{type:"keyUp",key:ch,code:"Key"+ch.toUpperCase()},sA).catch(()=>{}); }
log("value_after_keyDown_ab", await ev(A,sA,"document.getElementById('name').value"));
await A.call("Input.dispatchKeyEvent",{type:"char",text:"中",key:"中",unmodifiedText:"中"},sA).catch(e=>log("charCjkErr",errStr(e)));
log("value_after_char_cjk", await ev(A,sA,"document.getElementById('name').value"));
await A.call("Input.dispatchKeyEvent",{type:"rawKeyDown",key:"Backspace",code:"Backspace",windowsVirtualKeyCode:8},sA).catch(e=>log("bkspErr",errStr(e)));
await A.call("Input.dispatchKeyEvent",{type:"keyUp",key:"Backspace",code:"Backspace",windowsVirtualKeyCode:8},sA).catch(()=>{});
log("value_after_backspace", await ev(A,sA,"document.getElementById('name').value"));
// hit-test path for the future element picker
log("elementFromPoint", await ev(A,sA,"(()=>{const e=document.elementFromPoint(150,100);return e?e.id+'/'+e.tagName:null})()"));
log("DOM_getBoxModel_via_query", await (async()=>{const d=await A.call("DOM.getDocument",{depth:0},sA);const q=await A.call("DOM.querySelector",{nodeId:d.root.nodeId,selector:"#box"},sA);const bm=await A.call("DOM.getBoxModel",{nodeId:q.nodeId},sA);return {nodeId:q.nodeId,w:bm.model?.width,h:bm.model?.height};})().catch(errStr));
fs.writeFileSync("results-probe2.json",JSON.stringify(out,null,2)); A.close(); B.close(); process.exit(0);
