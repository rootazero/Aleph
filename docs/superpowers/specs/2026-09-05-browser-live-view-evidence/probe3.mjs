// THROWAWAY probe 3: AX name-from-content, AX timing vs V8 contention, screencast survival across navigation.
import fs from "node:fs";
const [port = "9333"] = process.argv.slice(2);
const WS = `ws://127.0.0.1:${port}/devtools/browser`;
const sleep=(ms)=>new Promise(r=>setTimeout(r,ms)); const now=()=>performance.now();
class Cdp{constructor(n){this.name=n;this.id=0;this.pending=new Map();this.listeners=[];}
 async connect(){this.ws=new WebSocket(WS);await new Promise((res,rej)=>{this.ws.addEventListener("open",res);this.ws.addEventListener("error",()=>rej(new Error("ws")));});this.ws.addEventListener("message",(ev)=>{const m=JSON.parse(ev.data);if(m.id&&this.pending.has(m.id)){const p=this.pending.get(m.id);this.pending.delete(m.id);m.error?p.reject(new Error(`${p.method}: ${m.error.message}`)):p.resolve(m.result);}else if(m.method){for(const l of this.listeners)l(m);}});}
 call(method,params={},sessionId,timeoutMs=60000){return new Promise((resolve,reject)=>{const id=++this.id;const t=setTimeout(()=>{if(this.pending.has(id)){this.pending.delete(id);reject(new Error(`${method}: timeout`));}},timeoutMs);this.pending.set(id,{method,resolve:(v)=>{clearTimeout(t);resolve(v);},reject:(e)=>{clearTimeout(t);reject(e);}});const msg={id,method,params};if(sessionId)msg.sessionId=sessionId;this.ws.send(JSON.stringify(msg));});}
 on(fn){this.listeners.push(fn);return()=>{this.listeners=this.listeners.filter(l=>l!==fn);};}
 waitEvent(method,sessionId,timeoutMs=40000){return new Promise((resolve,reject)=>{const t=setTimeout(()=>{off();reject(new Error(`timeout ${method}`));},timeoutMs);const off=this.on((m)=>{if(m.method===method&&(!sessionId||m.sessionId===sessionId)){clearTimeout(t);off();resolve(m.params);}});});}
 close(){try{this.ws.close();}catch{}}}
const out={};const log=(k,v)=>{out[k]=v;console.log(`## ${k}\n${JSON.stringify(v,null,1)}`);};const errStr=(e)=>String(e?.message??e);
async function navigate(c,s,url,settle=800){const t0=now();const lp=c.waitEvent("Page.loadEventFired",s).catch(e=>({loadWaitError:errStr(e)}));try{await c.call("Page.navigate",{url},s);}catch(e){return{url,navigateError:errStr(e)};}await lp;await sleep(settle);return{url,ms:Math.round(now()-t0)};}
const A=new Cdp("A");await A.connect();const {targetId}=await A.call("Target.createTarget",{url:"about:blank"});const s=`${targetId}-session`;await A.call("Page.enable",{},s);
await A.call("Emulation.setDeviceMetricsOverride",{width:1280,height:800,deviceScaleFactor:1,mobile:false},s).catch(()=>{});

// screencast survival across navigations (started once, never restarted)
const frames=[];A.on(m=>{if(m.method==="Page.screencastFrame"&&m.sessionId===s){frames.push({t:now(),bytes:Math.floor(m.params.data.length*3/4)});A.call("Page.screencastFrameAck",{sessionId:m.params.sessionId},s).catch(()=>{});}});
await A.call("Page.startScreencast",{format:"jpeg",quality:60,maxWidth:1280,maxHeight:800},s);
let t=now(); log("nav_hn", await navigate(A,s,"https://news.ycombinator.com/",1500)); log("frames_after_nav1", frames.filter(f=>f.t>t).length);

// AX name-from-content check on HN
const tree=await A.call("Accessibility.getFullAXTree",{},s);const nodes=tree.nodes;const byId=new Map(nodes.map(n=>[n.nodeId,n]));
const links=nodes.filter(n=>n.role?.value==="link");
const sample=links.slice(0,12).map(n=>({name:n.name?.value??null,nameFrom:n.name?.sources?.map(x=>x.type).join(",")??null,children:(n.childIds??[]).map(c=>{const k=byId.get(c);return k?`${k.role?.value}:${(k.name?.value??"").slice(0,30)}`:"?";}),props:(n.properties??[]).map(p=>p.name).join(",")}));
log("hn_link_samples", sample);
log("hn_links_with_name_vs_total", {withName:links.filter(n=>n.name?.value).length,total:links.length});
const someNode=nodes.find(n=>n.role?.value==="link"&&n.childIds?.length); log("raw_link_node_keys", someNode?Object.keys(someNode):null);
log("raw_link_node", someNode?JSON.stringify(someNode).slice(0,600):null);

// AX timing vs V8 contention on GitHub: measure evaluate("1") RTT before/after load, AX after settle
t=now(); log("nav_github", await navigate(A,s,"https://github.com/h4ckf0r0day/obscura",500)); log("frames_after_nav2", frames.filter(f=>f.t>t).length);
const rtt=async()=>{const t0=now();await A.call("Runtime.evaluate",{expression:"1",returnByValue:true},s);return Math.round(now()-t0);};
log("eval_rtt_right_after_load_ms", await rtt());
let a0=now(); let ax1; try{ax1=await A.call("Accessibility.getFullAXTree",{},s);}catch(e){ax1={err:errStr(e)};} log("ax_github_immediate_ms", Math.round(now()-a0));
await sleep(8000);
log("eval_rtt_after_8s_settle_ms", await rtt());
a0=now(); try{await A.call("Accessibility.getFullAXTree",{},s);}catch(e){log("ax2err",errStr(e));} log("ax_github_after_settle_ms", Math.round(now()-a0));
log("frames_total_across_two_navs", frames.length);
t=now(); log("nav_wiki", await navigate(A,s,"https://en.wikipedia.org/wiki/Web_scraping",1500)); log("frames_after_nav3", frames.filter(f=>f.t>t).length);
a0=now(); try{await A.call("Accessibility.getFullAXTree",{},s);}catch(e){log("ax3err",errStr(e));} log("ax_wiki_ms", Math.round(now()-a0));
log("screencast_still_alive_after_3_navs", frames.length>0 && frames.at(-1).t>t);
fs.writeFileSync("results-probe3.json",JSON.stringify(out,null,2));A.close();process.exit(0);
