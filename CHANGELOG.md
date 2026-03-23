# Changelog

All notable changes to the Aleph project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.11] - 2026-03-23### Added- fe
t(p
nel): 
dd stre
ming, render_mode, typing_indic
tor fields to Feishu settings- fe
t(feishu): wire FeishuEventEmitter into execution flow- fe
t(feishu): 
dd m
rkdown c
rd rendering 
nd upd
ted c
p
bilities- fe
t(feishu): 
dd FeishuEventEmitter with stre
ming c
rds 
nd typing indic
tors- fe
t(feishu): 
dd C
rd Kit stre
ming, st
tic c
rd, 
nd re
ction API methods- fe
t(feishu): 
dd stre
ming, render_mode, typing config fields 
nd API types- fe
t(p
nel): 
dd Feishu/L
rk ch
nnel settings c
rd- fe
t(feishu): fix clippy w
rnings — unused import, visibility, closure- fe
t(feishu): 
dd FeishuCh
nnel impl 
nd wire into f
ctory registry- fe
t(feishu): 
dd FeishuClient with token, HTTP API, 
nd medi
 support- fe
t(feishu): 
dd WebSocket event p
rsing 
nd text extr
ction- fe
t(feishu): 
dd types, config, 
nd API response structs- fe
t: 
dd Persistent Completion Protocol for 
gent t
sk verific
tion- desktop-m
cos: implement PimC
p
bility vi
 SwiftBridge- desktop-m
cos: implement SystemC
p
bility (
pps, notific
tions, clipbo
rd, sysinfo)- desktop-m
cos: implement Autom
tionC
p
bility (os
script + Shortcuts CLI)- desktop: wire N
tiveScreen into 
ll pl
tform cr
tes- desktop: 
dd N
tiveScreen sh
red ScreenC
p
bility implement
tion- core: 
dd SystemTool 
nd Autom
tionTool builtin tools- desktop: 
dd per-pl
tform cr
te skeletons (m
cos, linux, windows)- desktop: 
dd SwiftBridge utility for m
cOS n
tive API c
lls- desktop: upd
te cr
te doc to reflect two-l
yer 
rchitecture- desktop: 
dd c
p
bility tr
it hier
rchy 
nd sh
red types- core: 
dd 
leph-client dependency for server bin
ry- fe
t: en
ble n
tive tool c
lling for Ch
tGPT/Codex Responses API- core: 
dd Strict Mode support (schem
 strictific
tion + provider integr
tion)- core: 
dd #[cfg(unix)] gu
rds for Unix socket code on Windows- desktop: fix Windows OCR compil
tion errors- fe
t(browser): 
dd profile config types 
nd browser system configur
tion- fe
t(browser): 
dd SsrfPolicy for URL v
lid
tion 
nd priv
te network blocking- fe
t(config): 
dd queue_mode session configur
tion with g
tew
y wiring- fe
t(
nthropic): wire c
che_control ephemer
l bre
kpoint for system prompt c
ching- fe
t(thinker): p
rtition system prompt into st
ble/dyn
mic zones for c
che optimiz
tion- fe
t(compressor): 
dd pre-comp
ction silent memory flush- fe
t(
gent-loop): 
dd CollectQueue with time-window mess
ge merging- fe
t(
gent-loop): 
dd SteerQueue with interrupt sign
ling- fe
t(
gent-loop): 
dd SessionQueue tr
it 
nd FollowupQueue implement
tion- fe
t(
gent-loop): wire interrupt ch
nnel into RunContext 
nd loop execution- fe
t(
gent-loop): 
dd InterruptCh
nnel for steering support- core: 
dd missing tr
cing::w
rn import for non-m
cOS builds- fe
t: unified sl
sh comm
nd system- fe
t: wire memory tools into 
gent execution + Two-Ph
se Sm
rt Rec
ll- fe
t(server): 
dd desktop fe
ture g
te for in-process desktop c
p
bilities- fe
t(desktop): integr
te DesktopC
p
bility into DesktopTool with du
l-p
th execution- fe
t(desktop): implement input 
ctions with enigo- fe
t(desktop): implement screenshot 
nd OCR vi
 xc
p- fe
t: 
dd 
leph-desktop cr
te skeleton with DesktopC
p
bility tr
it- desktop: fix T
uri build for m
cOS 
nd 
dd 
pp/dmg bundle t
rgets- fe
t(w
sm): register host functions vi
 PluginBuilder with c
p
bility kernel- fe
t(m
nifest): p
rse WASM c
p
bilities from 
leph.plugin.toml- fe
t(w
sm): 
dd W
smC
p
bilityKernel — per-execution security enforcement- fe
t(w
sm): 
dd Credenti
lInjector — plugins never see secrets- fe
t(w
sm): 
dd AllowlistV
lid
tor with 
nti-byp
ss security- fe
t(w
sm): 
dd W
smC
p
bilities types with def
ult-deny model- fe
t(exec): 
dd Le
kDetector with Aho-Cor
sick bidirection
l sc
nning- desktop: 
dd 
ll_d
y 
nd c
lend
r_id to PimC
lend
rUpd
te- desktop: 
dd PIM v
ri
nts to DesktopRequest 
nd JSON-RPC m
pping- desktop: remove m
cOS t
rget, 
dd server embedding for Linux/Windows- desktop: fix fl
ky tests th
t 
ssumed bridge socket 
bsence- desktop-bridge: implement Windows OCR (WinRT) 
nd UI Autom
tion AX tree- desktop-bridge: implement window m
n
gement (list, focus, l
unch)- desktop-bridge: implement Windows input simul
tion (click, type, key combo, scroll)- desktop: wire sn
pshot 
nd new 
ctions in DesktopBridgeServer disp
tch- desktop: implement scroll, double-click, dr
g, hover, p
ste, 
nd ref-
w
re t
rgeting- desktop: implement UI sn
pshot with ref gener
tion in Perception.swift- desktop: 
dd RefStore for sn
pshot ref m
n
gement (Swift)- desktop: upd
te tool 
rgs 
nd build_request for sn
pshot, ref t
rgeting, 
nd new 
ctions- desktop: 
dd core types for sn
pshot, ref system, 
nd new 
ction primitives- desktop: upd
te tool mess
ging for bridge 
rchitecture- desktop: probe m
n
ged 
nd st
nd
lone socket p
ths- fe
t(runtimes): 
dd ensure_c
p
bility orchestr
tion (Probe -> Bootstr
p -> Register)- fe
t(runtimes): wire C
p
bilityLedger into prompt system- fe
t(runtimes): 
dd bootstr
p module with shell-driven inst
ll
tion- fe
t(runtimes): wire ledger into exec l
yer PATH- fe
t(runtimes): 
dd Probe module for system-first c
p
bility detection- fe
t(runtimes): 
dd leg
cy m
nifest.json migr
tion to ledger.json- fe
t(runtimes): 
dd C
p
bilityLedger for lightweight runtime st
te tr
cking- fe
t(desktop): implement desktop.screenshot in T
uri DesktopBridge- fe
t(desktop): 
dd DesktopBridge UDS server with ping support- fe
t(protocol): 
dd desktop_bridge types for cross-pl
tform Bridge- fe
t(h
lo): switch m
cOS H
loWindow from SwiftUI to WKWebView- fe
t(h
lo): 
dd /h
lo route with ch
t UI, mess
ge list, 
nd input 
re
- fe
t(h
lo): 
dd event h
ndler to wire run.* stre
ming events to H
loSt
te- fe
t(h
lo): 
dd H
loSt
te re
ctive sign
ls for ch
t st
te m
n
gement- fe
t(h
lo): 
dd Ch
tApi module for ch
t.send/
bort/history/cle
r- fe
t(desktop): T
sk 11 complete — DesktopTool 
ctive in 
gent vi
 builtin registry- fe
t(desktop): implement WKWebView c
nv
s overl
y with A2UI p
tch support- fe
t(desktop): implement mouse, keybo
rd, 
nd window 
ctions in Action.swift- fe
t(desktop): 
dd 
ccessibility permission description 
nd runtime check- fe
t(desktop): implement screenshot, OCR, 
nd AX tree in Perception.swift- fe
t(desktop): point settings window to Leptos Control Pl
ne server- fe
t(m
cos): 
dd Settings menu item opening Control Pl
ne WebView- fe
t(m
cos): 
dd SettingsWebView WKWebView wr
pper- fe
t(desktop): 
dd Swift UDS server skeleton with stub h
ndlers- fe
t(desktop): register DesktopTool in executor builtin registry- fe
t(desktop): 
dd DesktopTool builtin with gr
ceful degr
d
tion- fe
t(desktop): 
dd UDS client with JSON-RPC 2.0 
nd unit tests- fe
t(desktop): 
dd types, error, 
nd module sc
ffold- fe
t(skill): integr
te SkillSystem v2 into ExtensionM
n
ger 
nd ExecutionEngine- fe
t(skill): 
dd SkillSystem f
c
de with Arc<Inner> p
ttern- fe
t(skill): 
dd sl
sh comm
nd resolution- fe
t(skill): 
dd Inst
llSpec to shell comm
nd converter- fe
t(skill): 
dd SkillSt
tusReport for eligibility d
shbo
rd- fe
t(skill): 
dd SkillSn
pshot with version-inv
lid
ted c
che- fe
t(skill): 
dd XML prompt builder for skill injection- fe
t(skill): 
dd EligibilityService with OS/bin
ry/env checks- fe
t(skill): 
dd SKILL.md p
rser with YAML frontm
tter support- fe
t(skill): 
dd SkillRegistry with priority-b
sed dedup- fe
t(skill): 
dd SkillM
nifest Aggreg
teRoot with Entity tr
it- fe
t(skill): 
dd EligibilitySpec, Inst
llSpec, Invoc
tionPolicy, PromptScope V
lueObjects- fe
t(skill): 
dd SkillId, PluginId, SkillSource dom
in types- fe
t(thinker): 
dd skill_instructions to PromptConfig for SkillSystem v2- fe
t(extension): 
dd SkillSystem v2 
nd wire skill XML into 
gent prompts- fe
t(sw
rm): 
dd event st
tistics 
nd logging- fe
t(
gent_loop): integr
te ContextProvider into Mess
geBuilder- fe
t(sw
rm): implement Sw
rmContextProvider- fe
t(
gent_loop): define ContextProvider tr
it- fe
t(
gent_loop): implement event publishing (sh
dow mode)- fe
t(
gent_loop): define AgentLoopEvent enum- fe
t(
gent_loop): implement Builder build() method- fe
t(
gent_loop): 
dd AgentLoopBuilder structure- fe
t(perception): integr
te PAL with SystemSt
teBus- fe
t(perception): 
dd Pl
tform Abstr
ction L
yer (PAL)- fe
t(sw
rm): Ph
se 5 - End-to-End Integr
tion- fe
t(perception): implement Ph
se 5 - Document
tion, Ex
mples & Testing- fe
t(perception): implement Ph
se 4 - Vision Connector 
rchitecture- fe
t(ssb): implement Ph
se 3 - 
ction disp
tcher- fe
t(ssb): implement Ph
se 2 - robustness & priv
cy- fe
t(ssb): implement Ph
se 1 - core infr
structure- fe
t(control-pl
ne): implement WebSocket subscription for re
l-time 
lerts- fe
t(sh
red_ui_logic): 
dd 
lerts API module for system he
lth 
nd memory monitoring- fe
t(skill-evolution): integr
te SuccessM
nifest with tool execution- fe
t(control-pl
ne): p
ss mode 
nd 
lert_key to Sideb
rItems- fe
t(control-pl
ne): integr
te Tooltip 
nd B
dge into Sideb
rItem- fe
t(control-pl
ne): 
dd St
tusB
dge component for 
lert indic
tors- fe
t(control-pl
ne): 
dd Tooltip component for n
rrow mode l
bels- fe
t(skill-evolution): implement Coll
bor
tiveSolidific
tionPipeline- fe
t(control-pl
ne): implement Sideb
r n
rrow/wide mode switching- fe
t(skill-evolution): implement Constr
intV
lid
tor- fe
t(skill-evolution): implement SuccessM
nifest d
t
 structure- fe
t(control-pl
ne): 
dd SettingsL
yout for nested routing- fe
t(control-pl
ne): 
dd 
lert bus 
nd sideb
r mode override to D
shbo
rdSt
te- fe
t(control-pl
ne): 
dd sideb
r types (Sideb
rMode, AlertLevel, SystemAlert)- fe
t(control-pl
ne): compile T
ilwind CSS loc
lly for production- fe
t(d
shbo
rd): 
dd Plugins, Skills, 
nd Policies settings p
ges- fe
t(d
shbo
rd): 
dd sideb
r n
vig
tion to settings UI- fe
t(d
shbo
rd): 
dd Gener
tion Providers n
vig
tion c
rd to Settings p
ge- fe
t(d
shbo
rd): implement Gener
tion Providers CRUD function
lity- fe
t(d
shbo
rd): 
dd Gener
tion Providers frontend UI- fe
t(d
shbo
rd): 
dd Gener
tion Providers b
ckend 
nd API l
yer- fe
t(d
shbo
rd): implement comprehensive configur
tion m
n
gement UI- fe
t(m
cos): implement WebSocket client for G
tew
y connection- fe
t(m
cos): complete Ph
se 4 client simplific
tion for ControlPl
ne integr
tion- fe
t(d
shbo
rd): complete Ph
se 3 SDK integr
tion with RPC, events, 
nd API l
yer- fe
t(d
shbo
rd): complete Ph
se 2 SDK integr
tion with error h
ndling 
nd reconnection- fe
t(d
shbo
rd): 
dd connection st
te 
w
reness to Memory view- fe
t(d
shbo
rd): integr
te sh
red_ui_logic SDK into D
shbo
rd- fe
t(d
shbo
rd): full 
rchitectu
l ref
ctor with Leptos 0.8.15 
nd rust-ui components- fe
t(d
shbo
rd): complete Memory Explorer view 
nd fix System St
tus- fe
t(d
shbo
rd): initi
lize Aleph D
shbo
rd with Leptos 0.6- fe
t(sh
red-ui-logic): implement Plugins 
nd Providers APIs- fe
t(sh
red-ui-logic): implement WASM WebSocket connector- fe
t(sh
red-ui-logic): implement API 
nd Observ
bility l
yers- fe
t(sh
red_ui_logic): implement protocol l
yer- fe
t(sh
red_ui_logic): implement n
tive WebSocket connector- fe
t(sh
red_ui_logic): initi
lize Aleph UI Logic SDK- fe
t(cortex): implement LLM-b
sed critic report gener
tion- fe
t(cortex): 
dd AiProvider to CriticAgent- fe
t(cortex): implement LLM-b
sed root c
use 
n
lysis- fe
t(cortex): 
dd AiProvider to Re
ctiveReflector- fe
t(
gent_loop): 
dd met
-cognition integr
tion for Ph
se 6- fe
t(cortex): implement CortexIntegr
tion orchestr
tor (T
sk #11)- fe
t(cortex): implement experience clustering 
nd deduplic
tion- fe
t(disp
tcher): implement L1.5 ExperienceRepl
yL
yer- fe
t(cortex): implement Cortex Dre
ming b
ckground service- fe
t(cortex): implement LLM-b
sed p
ttern extr
ction- fe
t(cortex): implement Distill
tionService core structure- fe
t(engine): 
dd Fe
tureExtr
ctor for 
dv
nced ML rule le
rning- fe
t(cortex): implement multi-dimension
l experience v
lue estim
tor- fe
t(cortex): 
dd 
gent loop telemetry c
pture- fe
t(cortex): implement Experience CRUD oper
tions- fe
t(cortex): define core d
t
 structures- fe
t(engine): 
dd ML-b
sed L2 rule gener
tion (RuleLe
rner)- fe
t(cortex): 
dd experience_repl
ys d
t
b
se t
ble- fe
t(builtin_tools): 
dd AtomicOpsTool for 
tomic oper
tions- fe
t(browser): implement J
v
Script-b
sed context freeze/resume- fe
t(browser): implement Ph
se 2.4 CDP integr
tion for context freeze/resume- fe
t(engine): 
dd comprehensive testing 
nd perform
nce v
lid
tion- fe
t(executor): 
dd AtomicActionExecutor with L1/L2 routing- fe
t(engine): implement 
tomic engine with L1/L2/L3 routing- fe
t(disp
tcher): implement Ph
se 2 Intelligent Scheduling for Liquid Hub- fe
t(m
cos): 
dd guest session 
ctivity log UI- fe
t(m
cos): 
dd 
ctivity log RPC types 
nd methods- fe
t(g
tew
y): 
dd RPC request 
ctivity logging for guest sessions- fe
t(g
tew
y): 
dd guests.getActivityLogs RPC h
ndler- fe
t(g
tew
y): integr
te 
ctivity logging into GuestSessionM
n
ger- fe
t: implement guests.revokeInvit
tion RPC method- fe
t(m
cos): 
dd Guest m
n
gement UI in Settings- fe
t(g
tew
y): register config.get 
nd config.p
tch RPC h
ndlers- fe
t(g
tew
y): 
dd SessionIdentityMet
 for identity stor
ge- fe
t(protocol): 
dd IdentityContext for st
teless security- fe
t(g
tew
y): 
dd config.p
tch RPC h
ndler with events- fe
t(memory): 
dd idempotent n
mesp
ce migr
tion- fe
t(g
tew
y): 
dd RPC h
ndlers for guest m
n
gement- fe
t(memory): 
dd n
mesp
ce column for d
t
 isol
tion- fe
t(protocol): 
dd discovery types for mDNS- fe
t(protocol): 
dd ConfigCh
ngedEvent for config sync- fe
t(g
tew
y): 
dd Invit
tionM
n
ger for guest invit
tions- fe
t(protocol): 
dd invit
tion types for guest m
n
gement- fe
t(g
tew
y): 
dd PolicyEngine for permission checks- fe
t(g
tew
y): 
dd IdentityM
p for extern
l identity resolution- fe
t(protocol): 
dd Role 
nd GuestScope for Owner+Guest model- fe
t(ph
se3): complete T
uri Desktop migr
tion to thin client- fe
t(ph
se3): migr
te T
uri Desktop to SDK 
rchitecture (WIP)- fe
t(ph
se2): ref
ctor CLI to use SDK- fe
t(ph
se2): implement G
tew
yClient with 
uthentic
tion- fe
t(ph
se2): implement tr
nsport 
nd RPC l
yers in SDK- fe
t(ph
se2): cre
te 
leph-client-sdk skeleton- fe
t(g
tew
y): 
dd Server-Client routing infr
structure to ConnectionSt
te- fe
t: 
dd tool routing config 
nd scope checking for Server-Client 
rchitecture- fe
t(executor): integr
te RoutedExecutor with Agent Loop- fe
t(cli): cre
te 
leph-cli 
s protocol reference implement
tion- fe
t(protocol): cre
te 
leph-protocol cr
te for sh
red types- fe
t(executor): integr
te ToolRouter with execution engine- fe
t(disp
tcher): 
dd execution_policy field to UnifiedTool- fe
t(executor): 
dd ToolRouter for Server-Client routing decisions- fe
t(g
tew
y): 
dd tool.c
ll protocol mess
ges- fe
t(g
tew
y): 
dd ReverseRpcM
n
ger for Server-to-Client c
lls- fe
t(g
tew
y): store ClientM
nifest in ConnectionSt
te- fe
t(g
tew
y): extend ConnectP
r
ms to 
ccept ClientM
nifest- fe
t(g
tew
y): 
dd ClientM
nifest for c
p
bility negoti
tion- fe
t(disp
tcher): 
dd ExecutionPolicy enum for Server-Client routing- fe
t(spec_driven): implement BDD du
l-tr
ck testing system- fe
t(dom
in): implement DDD found
tion with m
rker tr
its- fe
t(disp
tcher): implement L2 
sync LLM enh
ncement for tool descriptions- fe
t(memory): 
dd perform
nce monitoring for LLM c
lls- fe
t(scheduler): implement recursion depth tr
cking- fe
t(scheduler): implement 
nti-st
rv
tion logic- fe
t(scheduler): implement L
neScheduler core- fe
t: implement CompressionD
emon for b
ckground compression scheduling- fe
t(scheduler): implement L
neSt
te with queue 
nd sem
phore- fe
t: enh
nce ContextComptroller with priority-b
sed token m
n
gement- fe
t: implement V
lueEstim
tor for memory import
nce scoring- fe
t(scheduler): 
dd l
ne scheduler infr
structure- fe
t: 
dd sliding window chunking to Tr
nscriptIndexer- fe
t: 
dd Tr
nscriptIndexer for ne
r-re
ltime memory indexing- fe
t(sub_
gents): 
dd 
ctive runs query 
nd st
ts to SubAgentRegistry- fe
t(sub_
gents): 
dd F
ctsDB persistence helpers for SubAgentRun- fe
t(sub_
gents): 
dd st
te tr
nsition to SubAgentRegistry- fe
t(sub_
gents): 
dd SubAgentRegistry with in-memory indexing- fe
t(memory): 
dd SubAgent f
ct types for Multi-Agent 2.0 persistence- fe
t(sub_
gents): 
dd SubAgentRun d
t
 model for Multi-Agent 2.0- fe
t(disp
tcher): integr
te Hydr
tionPipeline into Agent Loop- fe
t(core): export tool_index types from lib.rs- fe
t(memory): 
dd VectorD
t
b
se::in_memory() for testing- fe
t(disp
tcher): 
dd ToolRetriev
l with du
l-threshold hydr
tion- fe
t(disp
tcher): 
dd ToolIndexCoordin
tor for Memory synchroniz
tion- fe
t(disp
tcher): 
dd Sem
nticPurposeInferrer for L0/L1 inference- fe
t(disp
tcher): 
dd tool_index module with ToolRetriev
lConfig- fe
t(memory): 
dd Tool v
ri
nt to F
ctType for tool-
s-resource- fe
t(memory): 
dd Multi-Agent Resilience d
t
b
se l
yer- fe
t(g
tew
y): 
dd identity m
n
gement RPC h
ndlers- fe
t(thinker): 
dd thinking tr
nsp
rency guid
nce to PromptBuilder- fe
t(
gent_loop): integr
te ThinkingP
rser into DecisionP
rser- fe
t(g
tew
y): 
dd Re
soningBlock 
nd Uncert
intySign
l stre
m events- fe
t(
gent_loop): 
dd ThinkingP
rser for sem
ntic re
soning extr
ction- fe
t(
gent_loop): 
dd StructuredThinking types for CoT Tr
nsp
rency- fe
t(thinker): integr
te Soul into PromptBuilder- fe
t(thinker): 
dd m
rkdown p
rser for soul.md files- fe
t(thinker): 
dd IdentityResolver for l
yered identity resolution- fe
t(thinker): 
dd SoulM
nifest types for Embodiment Engine- fe
t(test): migr
te logging, security, 
nd e2e tests to BDD- fe
t(test): migr
te iMess
ge routing 
nd sub
gent tests to BDD- fe
t(g
tew
y): 
dd Ch
nnelProvider tr
it for inter
ction m
nifests- fe
t(
gent_loop): 
dd Silent 
nd He
rtbe
tOk decision types- fe
t(thinker): 
dd environment contr
ct 
nd security sections to PromptBuilder- fe
t(thinker): 
dd ContextAggreg
tor for environment reconcili
tion- fe
t(test): migr
te m
rkdown skills tests to BDD- fe
t(thinker): 
dd SecurityContext for policy-driven permissions- fe
t(thinker): 
dd Inter
ctionM
nifest for ch
nnel c
p
bility 
w
reness- fe
t(test): migr
te models 
nd protocol integr
tion tests to BDD- fe
t(test): migr
te DAG 
nd worldmodel disp
tcher tests to BDD- fe
t(test): migr
te sm
rt tool discovery 
nd sessions tests to BDD- fe
t(thinker): 
dd provider-specific context c
ching str
tegies- fe
t(disp
tcher): 
dd du
l-l
yer profile-b
sed tool filtering- fe
t(test): migr
te extension v2 
nd runtime tests to BDD- fe
t(g
tew
y): 
dd Worksp
ceM
n
ger for Anti-Gr
vity Architecture- fe
t(test): migr
te extension plugin registry tests to BDD- fe
t(test): migr
te tool server tests to BDD- fe
t(test): migr
te g
tew
y inbound router tests to BDD- fe
t(test): migr
te disp
tcher cortex tests to BDD- fe
t(test): migr
te memory integr
tion tests to BDD- fe
t(tests): migr
te memory f
cts tests to BDD- fe
t(tests): migr
te mess
ge builder tests to BDD- fe
t(tests): migr
te thinker prompt builder tests to BDD- fe
t(tests): migr
te POE tests to BDD- fe
t(tests): migr
te 
gent loop tests to BDD- fe
t(config): 
dd ProfileConfig for Worksp
ce Architecture- fe
t(tests): migr
te perception 
nd w
tcher tests to BDD- fe
t(tests): migr
te d
emon IPC 
nd l
unchd tests to BDD- fe
t(tests): migr
te d
emon core tests to BDD- fe
t(tests): migr
te config v
lid
tion tests to BDD- fe
t(tests): migr
te config b
sic tests to BDD- fe
t(tests): migr
te scripting engine tests to BDD- fe
t(tests): 
dd cucumber BDD infr
structure- fe
t: 
dd ex
mple YAML policies 
nd E2E tests- fe
t(disp
tcher): 
dd YAML policy lo
der 
nd PolicyEngine integr
tion- fe
t(disp
tcher): implement Y
mlPolicy with Rh
i ev
lu
tion- fe
t(scripting): 
dd B
selineApi with l
zy TTL c
ching- fe
t(scripting): implement HistoryApi.l
st() with WorldModel queries- fe
t(scripting): implement EventApi 
nd EventCollection filtering- fe
t(scripting): 
dd HistoryApi 
nd EventCollection stubs- fe
t(scripting): 
dd dur
tion p
rsing 
nd helpers for Rh
i- fe
t(disp
tcher): 
dd YAML rule schem
 p
rsing- fe
t(disp
tcher): 
dd Rh
i s
ndbox engine with strict limits- fe
t(worldmodel): 
dd JSON st
te persistence- fe
t(disp
tcher): 
dd core d
t
 structures- fe
t(d
emon): integr
te perception l
yer with d
emon CLI- fe
t(d
emon): implement FSEventW
tcher- fe
t(d
emon): implement SystemSt
teW
tcher- fe
t(d
emon): implement ProcessW
tcher- fe
t(d
emon): implement TimeW
tcher- fe
t(d
emon): 
dd w
tcher tr
it 
nd registry- fe
t(d
emon): 
dd perception configur
tion system- fe
t(d
emon): 
dd event system found
tion- fe
t(protocols): implement hot relo
d with notify file w
tching- fe
t(protocols): implement ProtocolLo
der file 
nd directory lo
ding- fe
t(protocols): implement Configur
bleProtocol custom mode with templ
te rendering- fe
t(protocols): implement Configur
bleProtocol minim
l mode (extends b
se + differences)- fe
t(protocols): 
dd JSONP
th p
rser for response v
lue extr
ction- fe
t(protocols): 
dd templ
te engine wr
pper for request/response tr
nsform
tion- fe
t(protocols): 
dd dependencies for configur
ble protocols (h
ndleb
rs, jsonp
th, notify)- fe
t(providers): 
dd ProtocolLo
der stub for hot relo
d- fe
t(providers): 
dd Configur
bleProtocol stub- fe
t(providers): implement ProtocolRegistry for dyn
mic protocol m
n
gement- fe
t(providers): 
dd ProtocolDefinition types for YAML configs- fe
t(tools): implement Virtu
lFs s
ndbox mode- fe
t(tools): 
dd Evolution 
uto-lo
d integr
tion- fe
t(g
tew
y): 
dd M
rkdown Skills RPC h
ndlers- fe
t(tools): 
dd repl
ce_tool() API with explicit upd
te sem
ntics- fe
t(tools): 
dd hot relo
d support for M
rkdown Skills (Ph
se 4)- fe
t(tools): 
dd Evolution Loop integr
tion for M
rkdown Skills (Ph
se 3)- fe
t(tools): 
dd ex
mples() method to AetherTool tr
it (Ph
se 2)- fe
t(tools): complete M
rkdown Tool Ad
pter integr
tion- fe
t(tools): implement M
rkdown Tool Ad
pter (Ph
se 1)- fe
t(providers): 
dd Tier 3 speci
lized OpenAI-comp
tible provider presets- fe
t(providers): 
dd Tier 2 OpenAI-comp
tible provider presets- fe
t(providers): 
dd Tier 1 OpenAI-comp
tible provider presets- fe
t(providers): 
dd Gemini presets 
nd upd
te f
ctory- fe
t(providers): implement GeminiProtocol 
d
pter- fe
t(providers): 
dd Gemini API types module- fe
t(providers): 
dd Cl
ude/Anthropic presets- fe
t(providers): implement AnthropicProtocol 
d
pter- fe
t(providers): 
dd Anthropic API types module- fe
t(g
tew
y): 
dd 
pprov
l RPC h
ndlers- fe
t(mcp): 
dd Approv
lH
ndler for hum
n-in-the-loop- fe
t(mcp): 
dd 
pprov
l request types for hum
n-in-the-loop- fe
t(mcp): 
dd stre
ming types for s
mpling responses- fe
t(mcp): 
dd TokenRefreshM
n
ger for 
utom
tic token refresh- fe
t(mcp): 
dd OAuth token refresh support- fe
t(mcp): integr
te context injection with S
mplingH
ndler- fe
t(mcp): 
dd ContextInjector for cross-server context- fe
t(mcp): 
dd IncludeContext enum type for s
mpling requests- fe
t(config): 
dd protocol field to ProviderConfig- fe
t(providers): 
dd provider presets registry- fe
t(providers): 
dd HttpProvider cont
iner with ProtocolAd
pter- fe
t(providers): implement OpenAiProtocol 
d
pter- fe
t(providers): 
dd ProtocolAd
pter tr
it with stre
ming support- fe
t(providers): 
dd RequestP
ylo
d DTO for protocol 
d
pters- fe
t(mcp): 
dd s
mpling c
llb
ck integr
tion to McpM
n
ger- fe
t(mcp): 
dd response mech
nism for server-initi
ted requests- fe
t(mcp): integr
te S
mplingH
ndler with McpClient- fe
t(memory): complete Memory v3 Milestones 4-6- fe
t(mcp): 
dd S
mplingH
ndler for server-initi
ted LLM c
lls- fe
t(mcp): implement re
l SSE event listening with reqwest-eventsource- fe
t(mcp): 
dd SSE event types 
nd reqwest-eventsource dependency- fe
t(memory): implement CLI list 
nd show comm
nds- fe
t(memory): implement AuditLogger for oper
tion tr
cking- fe
t(mcp): 
dd S
mpling RPC types for P2 server-initi
ted LLM c
lls- fe
t(memory): 
dd 
udit log schem
 
nd types- fe
t(memory): 
dd CLI module with file locking- fe
t(memory): implement Archiv
lService for scr
tchp
d 
rchiving- fe
t(memory): implement HybridTrigger with token threshold s
fety net- fe
t(memory): implement L
zyDec
yEngine for re
d-time dec
y ev
lu
tion- fe
t(memory): 
dd type-
w
re dec
y c
lcul
tion with tempor
l scope- fe
t(memory): 
dd dec
y_inv
lid
ted_
t field for recycle bin- fe
t(memory): complete Milestone 1 - Scr
tchp
d Found
tion- fe
t(memory): implement Scr
tchp
dM
n
ger with CRUD oper
tions- fe
t(memory): implement SessionHistory for scr
tchp
d 
rchiv
l- fe
t(memory): 
dd scr
tchp
d module structure 
nd templ
te- fe
t(mcp): implement re
l McpResourceM
n
ger 
nd McpPromptM
n
ger- fe
t(tools): 
dd mcp_get_prompt builtin tool- fe
t(tools): 
dd mcp_re
d_resource builtin tool- fe
t(mcp): implement re
l 
ggreg
tion for resources 
nd prompts- fe
t(mcp): 
dd resources 
nd prompts methods to McpClient- fe
t(mcp): 
dd resources 
nd prompts support to McpServerConnection- fe
t(mcp): 
dd Resources 
nd Prompts RPC types- fe
t(mcp): 
dd he
lth check logic for servers- fe
t(g
tew
y): wire MCP h
ndlers to McpM
n
gerH
ndle- fe
t(mcp): implement McpM
n
gerActor core loop- fe
t(mcp): 
dd config persistence for McpM
n
ger- fe
t(mcp): 
dd McpM
n
gerH
ndle public API- fe
t(mcp): 
dd McpComm
nd 
nd McpM
n
gerEvent types- fe
t(cortex): implement DecisionConfig with session override- fe
t(cortex): implement security rules (t
g injection, PII m
sking, instruction override)- fe
t(cortex): 
dd S
nitizerRule tr
it 
nd SecurityPipeline- fe
t(cortex): 
dd greedy JSON rep
ir logic- fe
t(cortex): implement JsonStre
mDetector st
te m
chine- fe
t(cortex): 
dd module skeleton with unified error types- fe
t(extension): 
dd PluginHttpH
ndler for plugin REST routes- fe
t(extension): 
dd PluginProviderAd
pter for plugin AI providers- fe
t(extension): 
dd Ch
nnelM
n
ger skeleton for plugin ch
nnels- fe
t(extension): 
dd HTTP route types- fe
t(extension): 
dd provider plugin types- fe
t(extension): 
dd ch
nnel plugin types- fe
t(g
tew
y): 
dd service lifecycle RPC h
ndlers- fe
t(extension): integr
te ServiceM
n
ger with ExtensionM
n
ger- fe
t(extension): 
dd ServiceM
n
ger for b
ckground services- fe
t(extension): 
dd service lifecycle types- fe
t(g
tew
y): 
dd plugins.executeComm
nd RPC h
ndler- fe
t(extension): 
dd comm
nd execution to PluginLo
der- fe
t(extension): 
dd DirectComm
ndResult type- fe
t(extension): implement scope-
w
re skill injection- fe
t(extension): implement V2 prompt lo
ding with scope support- fe
t(extension): 
dd scope 
nd bound_tool to ExtensionSkill- fe
t(extension): 
dd PromptScope enum for V2 skill injection- fe
t(extension): 
dd V2 hook conversion from TOML m
nifest- fe
t(extension): implement typed hook execution (interceptor/observer/resolver)- fe
t(extension): 
dd kind 
nd priority to HookConfig- fe
t(extension): 
dd HookKind 
nd HookPriority enums- fe
t(extension): integr
te TOML p
rser with 
uto-detection (TOML > JSON)- fe
t(extension): 
dd V2 fields to PluginM
nifest- fe
t(extension): 
dd TOML m
nifest p
rser types- fe
t(exec): check skill_
llowlist in 
pprov
l decision- fe
t(exec): 
dd skill_
llowlist config option- fe
t(exec): extend ExecContext with skill origin info- fe
t(skills): implement CLI Wr
pper v
lid
tor- fe
t(skills): 
dd he
lth checking methods to SkillsRegistry- fe
t(skills): 
dd inst
ll suggestion methods to SkillsInst
ller- fe
t(skills): implement He
lthChecker for dependency v
lid
tion- fe
t(skills): extend SkillFrontm
tter with requirements 
nd met
d
t
- fe
t(skills): 
dd types for requirements 
nd he
lth checking- fe
t(poe): repl
ce Pl
ceholderWorker with re
l AgentLoopWorker- fe
t(g
tew
y): wire POE contr
ct signing to G
tew
y- fe
t(poe): implement contr
ct signing workflow for first principles closure- fe
t(core): 
dd sn
pshot c
pture tool 
nd registry upd
tes- fe
t(config): 
dd memory configur
tion types 
nd v
lid
tion- fe
t(memory): enh
nce retriev
l 
nd 
dd dre
ming module- fe
t(m
cos): 
dd tool emoji form
tting to H
loStre
mingView- fe
t(m
cos): upd
te G
tew
yStre
mAd
pter with enh
nced summ
ry- fe
t(m
cos): 
dd H
loResultViewV2 with det
il popover support- fe
t(m
cos): 
dd H
loResultDet
ilPopover for det
iled results- fe
t(m
cos): 
dd Enh
ncedRunSumm
ry 
nd ToolSumm
ryItem models- fe
t(g
tew
y): 
dd Enh
ncedRunSumm
ry 
nd per-runId sequences- fe
t(g
tew
y): 
dd mess
ge deduplic
tion with text norm
liz
tion- fe
t(g
tew
y): 
dd stre
m buffer for block-level text flushing- fe
t(g
tew
y): 
dd tool displ
y module with emoji 
nd sm
rt form
tting- fe
t(h
lo): integr
te comm
ndList st
te into H
loViewV2- fe
t(h
lo): 
dd H
loComm
ndListView for / comm
nd p
nel- fe
t(h
lo): 
dd Comm
ndItem 
nd Comm
ndListContext types for / comm
nd- fe
t(h
lo): 
dd H
loInputCoordin
tor for lightweight input h
ndling- fe
t(g
tew
y): 
dd 150ms throttling for response chunks- fe
t(h
lo): 
dd H
loViewV2 m
in component integr
ting 
ll st
te views- fe
t(h
lo): 
dd H
loHistoryListView for convers
tion history- fe
t(h
lo): 
dd H
loResultView for comp
ct result displ
y- fe
t(h
lo): 
dd H
loStre
mingView for unified stre
ming displ
y- fe
t(h
lo): 
dd H
loSt
teV2 with 6 simplified st
tes- fe
t(h
lo): 
dd new stre
ming types for simplified st
te model- fe
t(skill-evolution): implement Skill Compiler (Ph
se 10)- fe
t(
gent-loop): 
dd on_user_question method to LoopC
llb
ck- fe
t(
gent-loop): 
dd AskUserRich decision v
ri
nt with QuestionKind- fe
t(
gent-loop): export question 
nd 
nswer modules- fe
t(
gent-loop): 
dd UserAnswer type for structured responses- fe
t(
gent-loop): 
dd QuestionKind types for structured user inter
ction- fe
t(resilient): 
dd cron integr
tion with Podc
stT
sk ex
mple- fe
t(resilient): implement ResilientExecutor with retry 
nd f
llb
ck- fe
t(resilient): define ResilientT
sk tr
it- fe
t(resilient): 
dd core types for resilient t
sk execution- fe
t(skill_evolution): implement GitCommitter for 
uto-commit- fe
t(skill_evolution): implement SkillGener
tor for SKILL.md cre
tion- fe
t(skill_evolution): implement Solidific
tionDetector for p
ttern detection- fe
t(skill_evolution): implement EvolutionTr
cker for execution logging- fe
t(skill_evolution): 
dd core types for skill evolution system- fe
t(spec_driven): implement SpecDrivenWorkflow orchestr
tor- fe
t(spec_driven): implement LlmJudge for ev
lu
tion- fe
t(spec_driven): implement TestWriter for test gener
tion- fe
t(spec_driven): implement SpecWriter for requirement 
n
lysis- fe
t(spec_driven): 
dd core types for spec-driven workflow- fe
t(g
tew
y): 
dd exec.c
llb
ck.h
ndle RPC for 
pprov
l c
llb
cks- fe
t(telegr
m): 
dd edit_mess
ge method for 
pprov
l upd
tes- fe
t(g
tew
y): 
dd 
pprov
l bridge h
ndler utilities- fe
t(exec): 
dd Approv
lBridge for ch
nnel integr
tion- fe
t(telegr
m): 
dd c
llb
ck query h
ndling- fe
t(telegr
m): 
dd inline keybo
rd support### Fixed- fix: 
dd tool_c
ll_id to OpenAI tool result mess
ges- fix: unignore CHANGELOG.md, fix rele
se recipe git 
dd- fix: remove unused imports 
cross codeb
se (c
rgo fix)- fix: resolve 42 test w
rnings — deprec
ted API, unused imports, de
d code- fix: sl
sh comm
nd f
st-p
th + CLI 
rg p
rser + E2E tests- fix: en
ble sl
sh comm
nd f
st-p
th for WebCh
t ch
t.send- fix: repl
ce env!("HOME") with dirs::home_dir() for Windows comp
tibility- fix: correct PluginKind::Mcp m
pping 
nd remove debug output- fix: upd
te discovery to find CC-form
t plugins in inst
lled/ directory- fix: ch
nnel binding not repl
cing old peer_id rows- fix: ch
nnel st
tus showing disconnected 
fter p
ge refresh- fix: p
ss session_m
n
ger to BuiltinToolConfig for session tools- fix: resolve 
gent from session_key inste
d of Worksp
ceM
n
ger- fix: sep
r
te 
gent identity files from worksp
ce directory- fix: use bold *n
me* for 
gent prefix inste
d of [n
me]- fix: use M
rkdown (leg
cy) inste
d of M
rkdownV2 for Telegr
m mess
ges- fix: remove b
cksl
sh esc
ping from 
gent n
me prefix in replies- fix: override rel
tive working_dir with 
gent worksp
ce- fix: ch
nge def
ult worksp
ce root from 
gents/ to worksp
ces/- fix: def
ult b
sh/code_exec working directory to 
gent worksp
ce- fix: register JSON Schem
 for 
ll builtin tools + Codex protocol 
lignment- fix: prevent token regener
tion on HMAC mism
tch to protect v
ult secrets- fix: Codex SSE function_c
ll_
rguments delt
 collection + logging- fix: use v
ult_key() function inste
d of undefined VAULT_KEY const
nt- fix: unify rer
nking v
ult key form
t with other modules- fix: rer
nking P
nel fetches per-provider API key from v
ult- fix: cle
r 
pi_key from rer
nking config sign
l 
fter s
ve- fix: isol
te rer
nk API keys per provider in v
ult- fix: move rer
nk API key from config.toml to encrypted v
ult- fix: correct def
ult rer
nking model n
me in P
nel 
nd tests- fix: ACP p
nel buttons h
ng due to sp
wn_loc
l context loss- fix: ACP test/s
ve button h
ng 
nd preset mode def
ults- fix: ACP p
nel gemini preset ID mism
tch 
nd test button h
ng- fix: resolve 
ll 75 compil
tion errors from provider routing ref
ctor- fix: v
ult-b
cked provider API keys 
nd config h
ndler improvements- fix(
cp): 
d
pt h
rnesses to re
l CLI protocols 
fter e2e probe testing- fix: worksp
ce schem
 migr
tion, worksp
ce.getActive response, 
nd providers p
ge freeze- fix: remove redund
nt binding in ConfigP
tcher- fix: session history, 
gent.list RPC, 
nd embedding dedup- fix: count only running runs for concurrency limit, reduce cle
nup del
y- fix: 
dd multi-dimension vector columns to memories t
ble schem
- fix: hot-sw
p runtime provider when switching def
ult vi
 P
nel UI- fix: resolve ch
t qu
lity issues — bootstr
p, esc
l
tion, 
nd response form
t- fix: resolve pre-existing test compil
tion errors- fix: wire missing RPC h
ndlers 
nd correct TUI method n
mes- fix: upd
te rem
ining port 18789 references to 18790- fix: unify ch
nnel config persistence — P
nel UI s
ve/lo
d/connect now works- fix: resolve compil
tion errors from fe
ture fl
g remov
l- fix(desktop): 
ddress fin
l review — version 
lignment, input v
lid
tion, Unicode- fix(desktop): 
ddress clippy needless-borrow w
rning in 
gent h
ndler- fix(desktop): 
ddress code qu
lity review — v
lid
tion, 
pprov
l g
tes- fix(desktop): wire N
tiveDesktop into registry + complete re-exports- fix: logic review R2 
rchitecture — 14 findings 
cross 5 c
tegories- fix: logic review R2 — 29 files 
cross 4 priority b
tches- fix: 
ddress code review findings for self-configur
tion- fix: RAII sem
phore gu
rd 
nd env v
r exp
nsion ordering (Known Issues)- fix: repl
ce std::sync::RwLock with cr
te::sync_primitives (P2-15)- fix: sort H
shM
p-derived collections for deterministic ordering (P2-14)- fix: repl
ce SystemTime UNIX_EPOCH .unwr
p() with .unwr
p_or_def
ult() (P2-12)- fix: rele
se locks before 
w
iting in 4 
sync p
tterns (P2-11)- fix: norm
lize t
sk_type 
nd t
sk_id in SessionKey::t
sk() (P1-9)- fix: use bounded c
st for POE token count u32 conversion (P1-8)- fix: resolve rem
ining UTF-8 byte slicing p
nics (P1-7)- fix: ConfigP
tcher use s
ve_increment
l 
nd h
rd-error on conflict- fix: logic review Ph
se 6 — 45 fixes 
cross g
tew
y, memory, poe, exec, providers, 
nd 15 more modules- fix: resolve 5 rem
ining W
rning-level issues from logic review Ph
se 5- fix: logic review Ph
se 4 — 18 fixes 
cross d
emon, engine, secrets, skills, components, cron- fix: resolve 5 Known Issues from logic review- fix: comprehensive logic review fixes 
cross 53 files in 77 modules- fix: use cfg(fe
ture = "loom") inste
d of cfg(loom) to 
void poisoning dependencies- fix(g
tew
y): elimin
te TOCTOU in execution_engine concurrent run limit check- fix(g
tew
y): use Mutex for ch
nnel_registry t
ke-once inbound_rx p
ttern- fix(resilience): simplify governor session_tokens from AtomicU64 to u64- fix: upd
te doctest to use poe::met
_cognition::Beh
vior
lAnchor- fix: 
dd Clone derive to NoiseFilter 
nd remove duplic
te mod decl
r
tions- fix: remove duplic
te scoring_pipeline module decl
r
tion in memory/mod.rs- fix(clippy): resolve print_liter
l w
rnings in secret providers comm
nd- fix(tests): migr
te secret_bound
ry_integr
tion tests to 
sync- fix(runtimes): 
ddress critic
l 
nd import
nt code review findings- fix: resolve 
ll clippy w
rnings in 
leph-t
uri 
nd 
lephcore- fix(desktop): use ERR_NOT_IMPLEMENTED for stubbed methods, 
dd debug logging- fix(h
lo): 
ddress code review findings for view 
nd events- fix(h
lo): gu
rd 
g
inst empty run_id in event h
ndler- fix(h
lo): use monotonic counter for unique mess
ge IDs, remove redund
nt ph
se gu
rd- fix(desktop): restrict UDS socket to owner-only 
ccess- fix(desktop): 
dd 30s timeout to UDS request to prevent indefinite t
sk h
ng- fix(desktop): log ev
lu
teJ
v
Script errors in C
nv
s, 
dd runAsync m
in-thre
d 
ssert- fix(desktop): repl
ce deprec
ted 
ctiv
te(options:) with 
ctiv
te() for m
cOS 15- fix(desktop): 
void PNG round-trip in OCR p
th by sh
ring c
ptureCurrentScreen- fix: 
ddress code review findings- fix(desktop): repl
ce strcpy with strncpy to prevent buffer overflow- fix(desktop): require x/y for click 
nd window_id for focus_window- fix(desktop): remove misle
ding serde t
gs from DesktopRequest, 
dd From conversions- fix(skill): 
ddress code review findings- fix(skill): resolve clippy w
rnings in skill module- fix(skill): use single colon sep
r
tor for SkillId (m
tches OpenCl
w convention)- fix(st
rt): 
dd cfg gu
rd for builder mod, tighten h
ndler visibility to pub(in cr
te::comm
nds::st
rt)- fix(st
rt): move session b
nner print into register_session_h
ndlers for consistency- fix: resolve 
ll compil
tion errors from server purific
tion- fix: cle
n up rem
ining Server-Client terminology in source comments- fix: rep
ir 2 broken doc-tests in skill_evolution module- fix: resolve 8 pre-existing test f
ilures- fix(control-pl
ne): document AlertsApi integr
tion limit
tion- fix(control-pl
ne): complete mock d
t
 remov
l- fix(control-pl
ne): fix memory le
ks 
nd improve error h
ndling in 
lert subscriptions- fix(sh
red-ui-logic): improve error h
ndling in 
lerts API- fix(control-pl
ne): use T
ilwind CDN for CSS compil
tion- fix(control-pl
ne): 
dd WASM initi
liz
tion in lib.rs- fix(control-pl
ne): upd
te st
rtup log mess
ge to show correct URL- fix(control-pl
ne): fix root p
th 
ccess 
nd st
tic 
sset lo
ding- fix: resolve compil
tion errors 
nd 
dd missing imports- fix(d
shbo
rd): 
dd w
sm_bindgen entry point to en
ble 
pp initi
liz
tion- fix(g
tew
y): extr
ct guest_session_id when require_
uth=f
lse- fix: resolve compil
tion errors in 
uth 
nd guest h
ndlers- fix: use rowid inste
d of id for sqlite-vec virtu
l t
ble upd
tes- fix(ph
se2): fix RPC tests 
nd upd
te progress report- fix(cli): use correct method n
mes for session comm
nds- fix(cli): resolve event stre
ming issue between g
tew
y 
nd CLI- fix(cli): 
lign comm
nd h
ndlers with g
tew
y API- fix(memory): h
ndle new SubAgent F
ctType v
ri
nts in consolid
tion- fix: resolve f
iling BDD tests for embodiment 
nd CoT tr
nsp
rency- fix: resolve f
iling unit tests- fix: resolve module export 
nd test compil
tion errors- fix: resolve 
ll 29 compiler w
rnings- fix: 
dd dylib.* p
ttern to gitignore- fix: upd
te .gitignore for Aleph ren
me 
nd remove dylib from tr
cking- fix(compressor): fix string conc
ten
tion in tests- fix(protocols): error on nonexistent JSONP
th inste
d of returning null- fix(scr
tchp
d): use EAFP p
ttern inste
d of sync exists() checks- fix(scr
tchp
d): remove 
sync from exists() 
nd export Scr
tchp
dConfig- fix(core): fix form
t strings in m
nifest.rs 
nd doctest in pty.rs- fix: cle
n up rem
ining MultiTurnCoordin
tor references- fix(g
tew
y): remove MultiTurnCoordin
tor dependency from 
d
pter- fix(h
lo): upd
te DependencyCont
iner comment for H
loInputCoordin
tor- fix(h
lo): upd
te AppDeleg
te to use H
loInputCoordin
tor- fix(h
lo): upd
te HotkeyService to use H
loInputCoordin
tor- fix: upd
te tests for 5 builtin tools 
nd skill evolution- fix: compil
tion errors in skill evolution 
nd perception modules- fix: resolve test compil
tion errors### Ch
nged- ref
ctor: ren
me ch
tgpt → codex protocol 
cross codeb
se- ref
ctor: ren
me ToolGroup → ToolC
tegory to 
void confusion with Te
m- ph
se4: cle
n 
ll T
uri references from codeb
se- ph
se4: remove T
uri, 
rchive old 
pps, move Swift bridge to cr
tes/desktop-m
cos/bridge- ref
ctor: move CLI/TUI/WebCh
t to interf
ces/, client to sh
red/- cle
nup: remove bootstr
p 
uto-clone 
nd leg
cy plugin index code- cle
nup: remove AgentLifecycleEvent::Switched 
nd AgentRouter from inbound router- cle
nup: remove 
gent switching (tool, intent detector, /switch comm
nd)- cle
nup: remove unregistered self-m
n
gement tool source files- cle
nup: remove old sub
gent tools (sp
wn/steer/kill + deleg
te)- cle
nup: move e2e tests into tests/, remove unused sh
red_ui_logic cr
te, 
dd secret sc
nning exclusion- cle
nup: remove tempor
ry debug logging for ch
tgpt protocol- ref
ctor: ren
me worksp
ce to 
gent 
cross memory/config/p
ths, enh
nce 
gent loop 
nd Ch
tGPT protocol- cle
nup: remove zombie code, upd
te def
ult config 
nd sh
red_ui_logic- cle
nup: remove st
le ALEPH_MASTER_KEY references from docs 
nd error mess
ges- ref
ctor: fl
tten 
gent_loop/ — remove minim
l/ subdirectory- cle
nup: remove deprec
ted APIs (register_
gent_tools, with_working_dir, ToolC
tegory::N
tive, PolicyEngine stubs, AuditStore, Inv
lid
teOld)- ref
ctor: ren
me Minim
l* types to st
nd
rd n
mes — this IS the loop- cle
nup: fix clippy w
rning in leg
cy_
d
pter detect_entry_point- cle
nup: elimin
te 
ll clippy w
rnings (58→0)- cle
nup: fix clippy w
rnings (derive Def
ult, redund
nt closures, simplified condition
ls)- cle
nup: remove st
le 
pp_bundle_id references from comments 
nd BDD tests- cle
nup: remove TypeScript webch
t (repl
ced by P
nel /ch
t route)- cle
nup: remove de
d Sub
gentAuthority 
nd tools/sessions dom
in l
yer- ref
ctor: simplify memory types, use floor_ch
r_bound
ry, 
dd mtime c
che to d
ily memory- ref
ctor(pdf): split pdf_gener
te.rs into module directory- ref
ctor: strip #[cfg(fe
ture)] from g
tew
y, server, extension, 
nd misc modules- ref
ctor: strip #[cfg(fe
ture)] from 
ll 12 ch
nnel implement
tions- ref
ctor: strip 20+ C
rgo fe
ture fl
gs from core cr
te- ref
ctor: Occ
m's R
zor p
ss — elimin
te clippy w
rnings 
nd de
d code- cle
nup: remove f
stembed 
nd loc
l embedding model remn
nts- cle
nup: fix unused import in host_functions.rs- ref
ctor(w
sm): simplify PermissionChecker to f
c
de over W
smC
p
bilities- cle
nup: bro
d DRY ref
ctoring 
nd clippy compli
nce 
cross codeb
se- cle
nup: remove st
le f
stembed references, fix integr
tion tests- cle
nup: remove m
cOS-specific CI workflow 
nd build scripts (C8-C12)- cle
nup: remove deprec
ted m
cOS Swift 
pp (C7)- cle
nup: remove UniFFI Swift bindings (C1-C2)- ref
ctor(core): introduce register_h
ndler! m
cro, elimin
te h
ndler boilerpl
te (W
ve 4)- ref
ctor(core): repl
ce &Vec<T> with &[T] in 
rrow_convert 
nd sh
dow_repl
y (W
ve 3B)- ref
ctor(core): convert Intern
lEventH
ndler String p
r
ms to &str (W
ve 3A)- ref
ctor(core): m
nu
l Clippy fixes — expect_fun_c
ll, useless_vec, ptr_
rg, type_complexity, module_inception, needless_borrows, 
nd more (W
ve 2B)- ref
ctor(core): repl
ce Def
ult::def
ult() field re
ssignment with struct liter
ls (W
ve 2A)- ref
ctor(core): 
uto-fix Clippy w
rnings 
nd remove unused imports (W
ve 1)- ref
ctor(runtimes): delete old runtime m
n
gers, repl
ce with Ledger/Probe system- ref
ctor(video): repl
ce RuntimeRegistry with C
p
bilityLedger in c
ption.rs- ref
ctor(init): repl
ce forced runtime inst
ll
tion with zero-inst
ll ledger- ref
ctor(desktop): delete RPC proxy comm
nds 
nd cle
n up de
d code (~1600 lines)- ref
ctor(h
lo): delete Re
ct frontend source from T
uri 
pp- ref
ctor(h
lo): point T
uri h
lo window to Leptos server URL- ref
ctor(h
lo): delete leg
cy Swift H
lo views 
nd fix references (~4500 lines removed)- ref
ctor(st
rt): split initi
lize_
uth, extr
ct lo
d_
pp_config, restore register c
lls to orchestr
tor- ref
ctor(st
rt): move register_* h
ndler functions to comm
nds/builder/h
ndlers.rs- ref
ctor(extension): thin mod.rs f
c
de, deleg
te lo
d_
ll to ComponentLo
der- ref
ctor(st
rt): extr
ct subsystem initi
lizers from st
rt_server- ref
ctor: remove distributed execution infr
structure (ExecutionPolicy, ClientM
nifest, ReverseRpc, ToolRouter, RoutedExecutor)- ref
ctor: cle
n up 
uth h
ndler by removing ClientM
nifest references- ref
ctor: simplify g
tew
y server by removing client routing infr
structure- ref
ctor: simplify ExecutionEngine by removing client routing- ref
ctor: ren
me g
tew
y/ch
nnels/ to g
tew
y/interf
ces/- ref
ctor: ren
me clients/ to 
pps/- cle
nup: remove unused imports from exec_security_g
te (post-reb
se)- cle
nup: fix Arc misuse, l
rge v
ri
nts, 
nd priv
te interf
ces (P
ss 3 fin
l)- cle
nup: extr
ct type 
li
ses 
nd p
r
meter structs (P
ss 3)- cle
nup: suppress module_inception for intention
l nested module p
ttern- cle
nup: fix 22 miscell
neous clippy w
rnings- cle
nup: P
ss 2 loc
l ref
ctoring (clone, strip_prefix, de
d code, redund
nt closures)- cle
nup: fix boole
n simplific
tions, identity ops, 
nd &P
thBuf sign
tures- cle
nup: remove unused imports 
nd repl
ce deriv
ble impls- cle
nup: 
pply c
rgo clippy --fix 
uto-corrections- ref
ctor(control-pl
ne): split Sideb
r into sideb
r/ directory- ref
ctor(control-pl
ne): use nested routes for Settings with SettingsL
yout- ref
ctor(control-pl
ne): remove /cp prefix from routing- ref
ctor(core): ren
me 
leph-g
tew
y to 
leph-server- ref
ctor(m
cos): completely remove settings UI from m
cOS client- ref
ctor(desktop): completely remove settings UI from T
uri client- ref
ctor(desktop): migr
te Plugins, Skills, 
nd Policies settings to D
shbo
rd- ref
ctor(clients): complete Ph
se 4 - remove Gener
tion Providers UI- ref
ctor(clients): migr
te Providers, Memory, 
nd MCP config to D
shbo
rd- ref
ctor(
gent_loop): introduce RunContext p
ttern for cle
ner API- ref
ctor(
gent-loop): 
dd RunContext structure (WIP)- ref
ctor(dom
in): implement Newtype p
ttern for Answer 
nd Ruleset- ref
ctor(dom
in): implement Newtype p
ttern for 5 ID types- ref
ctor(
pi): implement FromStr tr
it for rem
ining types- ref
ctor(
pi): implement FromStr tr
it for extension 
nd resilience types- ref
ctor(
pi): implement FromStr tr
it for memory context types- ref
ctor(perf): repl
ce trim_st
rt_m
tches with strip_prefix for fixed prefixes- ref
ctor(perf): optimize &P
thBuf → &P
th in 6 files- ref
ctor(core): 
dd #[
llow(de
d_code)] to 12 reserved fields- ref
ctor(deps): remove 5 unused dependencies- ref
ctor(core): remove 2 confirmed de
d code items- ref
ctor(core): remove 160+ unused imports 
cross 50 files- ref
ctor(tools): extr
ct builtin tool registr
tion 
nd types (Ph
se 6)- ref
ctor(g
tew
y): modul
rize plugins h
ndlers (Ph
se 5.1)- ref
ctor(poe): extr
ct services to dedic
ted modules (Ph
se 4.2 - P1)- ref
ctor(poe): extr
ct h
ndler types to dedic
ted modules (Ph
se 4.1 - P0)- ref
ctor(browser): extr
ct types 
nd scripts modules (Ph
se 3 - P
rt 1)- ref
ctor(engine): complete 
tomic executor composition ref
ctoring (Ph
se 2)- ref
ctor(engine): 
dd 
tomic module b
se 
rchitecture (Ph
se 2 WIP)- ref
ctor(extension): split types.rs into modul
r structure- ref
ctor(security): tr
nsform PolicyEngine to st
teless- ref
ctor(protocol): 
dd equ
lity derives 
nd helper methods to 
uth types- ref
ctor(ph
se1): reorg
nize client directory structure- ref
ctor: complete fin
l Aether to Aleph cle
nup- ref
ctor: complete Aether to Aleph ren
me - scripts, workflows, 
nd rem
ining code- ref
ctor: complete Aether to Aleph ren
me 
cross entire codeb
se- ref
ctor(providers): use ProtocolRegistry in cre
te_provider f
ctory- ref
ctor(providers): remove technic
l 
li
s presets- ref
ctor(config): remove provider_type field from ProviderConfig- ref
ctor: fix P3 clippy w
rnings - b
tch 2- ref
ctor: fix P3 clippy w
rnings - b
tch 1- ref
ctor: fix P1/P2 clippy w
rnings 
nd improve code qu
lity- ref
ctor(providers): delete leg
cy OpenAiProvider- ref
ctor(providers): delete leg
cy GeminiProvider- ref
ctor(providers): delete leg
cy Cl
udeProvider- ref
ctor(providers): use HttpProvider for Anthropic protocol- ref
ctor(providers): remove redund
nt vendor wr
ppers (~850 lines)- ref
ctor(providers): use HttpProvider for OpenAI protocol in f
ctory- ref
ctor(m
cos): cle
nup 
nd improve hotkey/h
lo components- ref
ctor(h
lo): repl
ce H
loSt
te with simplified 6-st
te version- ref
ctor(h
lo): switch H
loWindow to V2 components- ref
ctor(h
lo): remove MultiTurn references from EventH
ndler- ref
ctor(h
lo): remove MultiTurn directory (~3000 lines)- ref
ctor: split l
rge modules into sm
ller files- cle
nup: remove unused modules 
nd merge thinking into thinker- cle
nup: elimin
te 
ll compil
tion w
rnings- cle
nup(lib): slim down exports from 590 to 272 lines- cle
nup: remove FFI-rel
ted comments- cle
nup: ren
me FFI types to st
nd
rd n
mes- cle
nup(disp
tcher): ren
me ffi.rs to tool_info.rs- cle
nup(intent): remove Type A FFI residu
ls### Build- build: fix inst
ll scripts — proper upgr
de flow 
nd service m
n
gement- rele
se: v0.2.10- docs: 
dd skill scope filtering implement
tion pl
n- docs: fix skill scope filtering spec per review- docs: 
dd skill scope filtering design spec- rele
se: v0.2.9- docs: 
dd voice convers
tion implement
tion pl
n- docs: fix PromptBuilder voice st
te 
ccess p
th in voice spec- docs: upd
te voice convers
tion spec with review fixes- docs: 
dd voice convers
tion system design spec- docs: 
dd rele
se workflow 
nd version m
n
gement to CLAUDE.md- rele
se: v0.2.8- build: unify version source — VERSION file drives 
ll version strings- rele
se: v0.2.8- docs: 
dd multimod
l probe tests implement
tion pl
n- docs: 
dd multimod
l probe tests design spec- docs: 
dd core multimod
l enh
ncement implement
tion pl
n- docs: fix spec review issues in core multimod
l design- docs: 
dd core multimod
l enh
ncement design spec- docs: 
dd Telegr
m ch
nnel enh
ncement implement
tion pl
n- docs: fix spec review issues in Telegr
m enh
ncement design- docs: 
dd Telegr
m ch
nnel enh
ncement design spec- docs: 
dd Feishu enh
nced fe
tures implement
tion pl
n- docs: 
ddress spec review — FeishuEventEmitter, typing lifecycle, c
p
bilities- docs: 
dd Feishu enh
nced fe
tures design spec- docs: 
dd Feishu ch
nnel implement
tion pl
n- docs: 
ddress spec review feedb
ck for Feishu ch
nnel- docs: 
dd Feishu/L
rk ch
nnel design spec- rele
se: v0.2.7 — multi-
gent system, UI upd
tes, bug fixes- docs: fix spec issues from review — st
le fin
l_text, test pl
n, consecutive_errors- docs: 
dd Persistent Completion Protocol design spec- docs: fix multi-
gent modes spec per review findings- docs: 
dd multi-
gent modes t
xonomy design spec- docs: 
dd t
sk coordin
tion implement
tion pl
n (12 t
sks)- docs: fix event type conventions in t
sk coordin
tion spec- docs: 
ddress spec review findings for t
sk coordin
tion- docs: 
dd t
sk coordin
tion system design spec- build: upd
te WASM p
nel dist- ci: upgr
de GitHub Actions to Node.js 24 comp
tible versions- ci: scope fmt check to m
int
ined cr
tes (skip leg
cy form
tting issues)- build: consolid
te to single rele
se workflow, fix CI protoc dependency- build: remove 
rchive from git (l
rge bin
ries exceed GitHub limit)- rele
se: bump version to 0.2.6- build: upd
te inst
ll scripts for 
leph-server bin
ry n
me- build: ren
me workflows, fix --bin 
leph→
leph-server, 
dd pl
tform rele
se workflows- build: upd
te justfile 
nd CI workflows for post-T
uri 
rchitecture- build: 
dd swift-bridge recipe to justfile for m
cOS n
tive APIs- docs: 
dd Ph
se 3 implement
tion pl
n for m
cOS PIM & system c
p
bilities- docs: 
dd Ph
se 2 implement
tion pl
n for screen control n
tive migr
tion- docs: 
ddress spec review feedb
ck for hier
rchic
l comm
nds- docs: 
dd hier
rchic
l sl
sh comm
nds design spec- docs: 
dd Ph
se 1 implement
tion pl
n for desktop n
tive c
p
bilities- docs: 
dd desktop n
tive c
p
bilities design spec- docs: upd
te design spec with new directory structure- docs: 
dd implement
tion pl
n for intermedi
te mess
ge delivery- docs: 
dd PLUGIN_SYSTEM.md — CC-comp
tible plugin 
rchitecture reference- docs: 
ddress spec review feedb
ck for CLI/TUI sep
r
tion- docs: 
dd CLI/TUI sep
r
tion design spec- docs: 
dd P4 runtime migr
tion implement
tion pl
n- docs: 
dd prompt guid
nce 
s in-scope ch
nges to intermedi
te mess
ge spec- docs: 
dd edge c
ses to intermedi
te mess
ge delivery spec- docs: 
dd intermedi
te mess
ge delivery design spec- docs: 
dd P3 scope m
n
gement implement
tion pl
n- docs: 
dd P2 m
rketpl
ce system implement
tion pl
n- docs: 
dd P0+P1 implement
tion pl
n for plugin CC comp
t- docs: fix rem
ining spec review items (round 2)- docs: 
ddress spec review findings for plugin comp
t design- docs: 
dd plugin system Cl
ude Code comp
tibility redesign spec- docs: upd
te spec 
nd pl
n — keep peer_id sign
tures unch
nged- docs: upd
te 
gent-bot 1:1 binding spec with review fixes- docs: 
dd 
gent-bot 1:1 binding simplific
tion design spec- docs: 
dd ch
t sideb
r redesign spec 
nd implement
tion pl
n- docs: 
dd p
nel 
gent routing fix design spec- docs: 
dd worksp
ce output migr
tion implement
tion pl
n- docs: revise worksp
ce output migr
tion spec 
fter review- docs: 
dd worksp
ce output migr
tion design spec- docs: 
dd gener
tion providers wiring implement
tion pl
n- docs: fix gener
tion providers spec 
fter review- docs: 
dd gener
tion providers wiring design spec- docs: 
dd Cl
wHub integr
tion implement
tion pl
n- docs: 
ddress spec review feedb
ck for Cl
wHub integr
tion- docs: 
dd Cl
wHub integr
tion design spec- ci: upgr
de GitHub Actions to Node.js 24, fix Windows de
d-code w
rnings- docs: fix pl
n review issues (3 blockers + 6 w
rnings)- docs: 
ddress spec review feedb
ck for Chrome DevTools MCP Mode- docs: 
dd Chrome DevTools MCP Mode design spec- docs: 
dd process m
n
gement rules to CLAUDE.md- docs: 
dd tool permission system implement
tion pl
n- docs: upd
te tool permission spec 
fter review- docs: 
dd tool permission system design spec- docs: 
dd ACP probe tests design document- docs: 
dd ACP h
rness m
n
gement implement
tion pl
n- docs: 
dd ACP h
rness m
n
gement design document- docs: 
dd provider routing ref
ctor implement
tion pl
n- docs: fix rem
ining spec review issues- docs: fix spec issues from review- docs: 
dd provider routing ref
ctor design spec- docs: 
dd provider config testing implement
tion pl
n- docs: upd
te provider config testing spec 
fter review- docs: 
dd provider config testing design spec- docs: 
dd simplify-model-config implement
tion pl
n- docs: upd
te simplify-model-config spec 
fter review- docs: 
dd simplify-model-config design spec- ci: re
d rele
se version from VERSION file inste
d of m
nu
l input- docs: 
dd cron probe tests implement
tion pl
n- docs: 
dd cron probe tests design spec- docs: 
dd cron module redesign implement
tion pl
n- docs: 
dd cron module redesign spec- build: rebuild p
nel WASM 
nd upd
te docs 
fter worktree merges- docs: 
dd provider zero-config implement
tion pl
n- docs: 
dd mess
ge pipeline implement
tion pl
n- docs: 
dd provider zero-config UX design spec- docs: 
dd mess
ge pipeline design for g
tew
y pre-processing- docs: 
dd model discovery probe tests implement
tion pl
n- docs: 
dd model discovery probe tests design spec- docs: 
dd model discovery implement
tion pl
n- docs: fix model discovery spec issues from review- docs: 
dd model discovery design spec- docs: 
dd cognitive evolution bet
 implement
tion pl
n- docs: 
dd cognitive evolution bet
 design (immune-complete loop)- docs: 
dd POE Ph
se 2+3 implement
tion pl
n- docs: 
dd POE Ph
se 1 implement
tion pl
n (Bl
stR
dius + T
boo)- docs: 
dd POE Architecture Evolution Whitep
per 2026- ci: fix Linux/Windows compil
tion errors for missing imports- docs: upd
te extension system 
rchitecture document
tion- docs: 
dd unified plugin system implement
tion pl
n- docs: 
dd unified plugin system design- docs: 
dd one-line inst
ll comm
nds 
s prim
ry inst
ll
tion method- docs: remove ref
ctoring b
ckstory from intent section- docs: upd
te intent detection section to reflect unified LLM pipeline- docs: 
dd det
iled Aleph vs OpenCl
w comp
rison- docs: 
dd P4.3 core plugins implement
tion pl
n- docs: 
dd plugin development guide- docs: 
dd P4 plugin ecosystem implement
tion pl
n- ci: 
dd Windows x86_64 build t
rget 
nd PowerShell inst
ller- docs: 
dd P3 medi
 pipeline implement
tion pl
n- ci: fix Linux w
rn import, remove d
rwin-x86_64 t
rget- ci: 
dd libxdo-dev for Linux, fix d
rwin x86_64 AVX-512 link error- ci: fix Linux pipewire comp
t (ubuntu-24.04) 
nd m
cOS x86_64 openssl- ci: 
dd libegl 
nd X11 extension deps for Linux build- ci: use m
cos-l
test for x86_64 cross-compile (m
cos-13 EOL)- ci: 
dd dbus, drm, gbm deps for Linux build- ci: 
dd pipewire 
nd cl
ng deps for Linux xc
p build- ci: 
dd libw
yl
nd-dev to Linux build dependencies- docs: 
dd 
uthor note to README- docs: ren
me p
nel screenshots with consistent numbering- docs: restore d
shbo
rd screenshot, keep 
ll 3 p
nel im
ges- docs: upd
te README screenshots with P
nel ch
t 
nd settings views- build: remove webch
t recipes from justfile- docs: 
dd webch
t Rust rewrite implement
tion pl
n- docs: 
dd webch
t Rust rewrite design- docs: remove 
cknowledgments section from README- ci: en
ble 
ll pl
tform build t
rgets for server rele
se- ci: 
dd m
nu
l server rele
se workflow 
nd improve inst
ll script- docs: overh
ul README.md, CLAUDE.md 
nd 
dd LICENSE- docs: 
dd inline directives 
nd leg
cy cle
nup implement
tion pl
n- docs: 
dd inline directives 
nd leg
cy cle
nup design- docs: 
dd l
ngu
ge-
gnostic intent detection implement
tion pl
n- docs: 
dd l
ngu
ge-
gnostic intent detection design- docs: upd
te cle
nup pl
n with execution results- docs: cl
rify cle
nup str
tegy — scoped responsibility, not f
llb
ck- docs: 
dd multi-
gent code redund
ncy cle
nup pl
n- docs: 
dd A2A protocol implement
tion pl
n- docs: 
dd A2A protocol design document- docs: 
dd per-
gent tool configur
tion implement
tion pl
n- docs: 
dd per-
gent tool configur
tion design- docs: 
dd multi-bot P
nel UI implement
tion pl
n- docs: 
dd multi-bot P
nel UI design- docs: 
dd multi-bot ch
nnel implement
tion pl
n- docs: 
dd multi-bot ch
nnel support design- docs: 
dd memory 
lignment design for du
l-directory 
rchitecture- docs: 
dd 
gent-worksp
ce sep
r
tion implement
tion pl
n- docs: 
dd 
gent-worksp
ce sep
r
tion design- docs: 
dd 
gent m
n
gement p
nel implement
tion pl
n- docs: 
dd 
gent m
n
gement p
nel design- docs: 
dd webch
t restructure implement
tion pl
n- docs: 
dd webch
t restructure design- docs: 
dd 
gent switching enh
ncement implement
tion pl
n- docs: 
dd 
gent switching enh
ncement design- docs: 
dd unified comm
nd registry implement
tion pl
n- docs: 
dd unified comm
nd registry design- docs: 
dd dyn
mic 
gent switching implement
tion pl
n- docs: 
dd dyn
mic 
gent switching design- docs: 
dd system prompt optimiz
tion implement
tion pl
n- docs: 
dd system prompt 
rchitecture optimiz
tion design- docs: 
dd Agent/Worksp
ce/Session unific
tion implement
tion pl
n- docs: 
dd Agent/Worksp
ce/Session rel
tionship design- docs: 
dd t
sk routing decision l
yer implement
tion pl
n- docs: 
dd t
sk routing decision l
yer design- docs: 
dd 
rchitecture 
ctiv
tion di
gnostic report- docs: 
dd 
rchitecture 
ctiv
tion di
gnostic implement
tion pl
n- docs: 
dd 
rchitecture 
ctiv
tion di
gnostic design- docs: 
dd n
tive tool_use implement
tion pl
n (9 t
sks)- docs: 
dd n
tive tool_use migr
tion design- docs: 
dd PDF du
l-engine implement
tion pl
n- docs: 
dd PDF du
l-engine rendering design- docs: 
dd cron 
nd group ch
t b
ckend implement
tion pl
n- docs: 
dd cron 
nd group ch
t b
ckend implement
tion design- docs: 
dd scheduled t
sks p
nel implement
tion pl
n- docs: 
dd scheduled t
sks p
nel design- docs: 
dd CLI full RPC cover
ge implement
tion pl
n- docs: 
dd CLI full RPC cover
ge design- docs: 
dd CLI bugfix 
nd JSON unific
tion design- docs: 
dd CLI full comm
nds implement
tion pl
n- docs: 
dd CLI full comm
nds design- docs: 
dd CLI infr
structure enh
ncement implement
tion pl
n- docs: 
dd CLI infr
structure enh
ncement design- docs: 
dd lifecycle observ
bility logging implement
tion pl
n- docs: 
dd lifecycle observ
bility logging design- docs: 
dd system prompt enh
ncement implement
tion pl
n- docs: 
dd system prompt enh
ncement design- docs: 
dd 
gent system Ph
se 2 full cover
ge implement
tion pl
n- docs: 
dd 
gent system full cover
ge design (Ph
se 2)- docs: 
dd Codex p
nel UI design 
nd implement
tion pl
n- docs: 
dd Codex Responses API implement
tion pl
n- docs: 
dd Codex Responses API protocol 
d
pter design- docs: 
dd g
tew
y enh
ncement implement
tion pl
n (20 t
sks)- docs: 
dd g
tew
y enh
ncement design (OpenCl
w-inspired)- docs: 
dd implement
tion pl
n for 
gent/worksp
ce/binding- docs: 
dd 
gent definition + worksp
ce + binding design- docs: 
dd OpenAI subscription provider implement
tion pl
n- docs: 
dd OpenAI subscription provider design- docs: 
dd L
zy POE Activ
tion design- build: ren
me just server → just build, 
dd just 
ll- docs: upd
te bin
ry n
me 
nd port references 
cross 
ll document
tion- build: en
ble 
xum ws fe
ture for port unific
tion- docs: 
dd port unific
tion implement
tion pl
n- docs: 
dd port unific
tion 
nd bin
ry ren
me design- docs: 
dd ch
nnel infr
structure fix implement
tion pl
n- docs: 
dd ch
nnel infr
structure fix design- docs: upd
te CLAUDE.md for fe
ture fl
g remov
l- build: simplify justfile — remove 
ll --fe
tures fl
gs- docs: 
dd runtime ch
nnel control implement
tion pl
n- docs: 
dd runtime ch
nnel control design — elimin
te fe
ture fl
g fr
gment
tion- docs: 
dd ch
t persistence & memory pipeline implement
tion pl
n- docs: 
dd ch
t persistence & memory pipeline fix design- docs: 
dd full ch
in + sm
rt rec
ll implement
tion pl
n- docs: 
dd full ch
in + sm
rt rec
ll design- docs: 
dd worksp
ce enh
ncements implement
tion pl
n (9 t
sks)- docs: 
dd worksp
ce enh
ncements design (4 fe
tures)- docs: 
dd worksp
ce wiring implement
tion pl
n (11 t
sks)- docs: 
dd worksp
ce wiring design for multi-role person
 system- docs: 
dd config extern
liz
tion implement
tion pl
n- docs: 
dd config extern
liz
tion design for ~/.
leph worksp
ce- ci: keep only m
cOS ARM64 build, document other pl
tform blockers- ci: fix rem
ining build issues 
cross pl
tforms- ci: fix cross-pl
tform build issues- ci: pin w
sm-bindgen-cli to 0.2.108 m
tching C
rgo.lock- ci: 
llow test job to f
il without blocking builds- ci: 
dd X11/xscrns
ver dev libr
ries for Linux builds- ci: inst
ll protoc for l
nce-encoding build dependency- ci: improve rele
se workflow with WASM build, test job, 
nd cross-pl
tform desktop- build: rewrite justfile for desktop-
s-muscle 
rchitecture- docs: 
dd cr
tes/desktop to project structure 
nd build comm
nds- docs: 
dd Desktop-
s-Muscle implement
tion pl
n- docs: 
dd Desktop-
s-Muscle 
rchitecture design- docs: 
dd self-configur
tion implement
tion pl
n- docs: 
dd self-configur
tion design document- ci: 
dd loom concurrency test job 
nd incre
se proptest cover
ge- build: 
dd test-proptest, test-loom, test-logic just recipes- docs: 
dd logic review system implement
tion pl
n (15 t
sks, 49 properties)- docs: 
dd logic review system design (three-l
yer defense 
rchitecture)- docs: move obsolete embedding/sqlite-vec pl
ns to leg
cy- docs: upd
te memory system docs to reflect remote embedding migr
tion- build: repl
ce trunk with m
nu
l WASM pipeline in justfile- docs: fix m
cOS Resources p
th in build pipeline design- build: 
dd justfile for unified build pipeline- docs: 
dd unified build pipeline design- docs: 
dd ch
nnel config p
nel implement
tion pl
n- docs: 
dd ch
nnel config p
nel design document- docs: 
dd POE full evolution implement
tion pl
n (19 t
sks, 4 ph
ses)- docs: 
dd POE full evolution design (event-driven closed loop)- docs: 
dd WASM c
p
bility kernel implement
tion pl
n- docs: 
dd WASM c
p
bility kernel design- docs: 
dd m
cOS PIM n
tive API implement
tion pl
n- docs: 
dd m
cOS PIM n
tive API integr
tion design- docs: 
dd POE cognitive hub implement
tion pl
n- docs: 
dd POE cognitive hub upgr
de design- docs: 
dd soci
l bot ch
nnels exp
nsion implement
tion pl
n- docs: 
dd soci
l bot ch
nnels exp
nsion design- docs: 
dd surgic
l DRY ref
ctoring implement
tion pl
n- docs: 
dd surgic
l DRY ref
ctoring design for embedding provider files- docs: 
dd embedding provider LLM migr
tion implement
tion pl
n- docs: 
dd embedding provider LLM migr
tion design- docs: 
dd l
rge file ref
ctoring implement
tion pl
n — 6 t
sks, 5 files- docs: 
dd l
rge file ref
ctoring design — 5 files, pure module splitting- ci: 
dd server, m
cOS 
pp, 
nd T
uri rele
se workflows- docs: 
dd distribution implement
tion pl
n (24 t
sks, 9 ph
ses)- docs: 
dd distribution 
rchitecture design- docs: 
dd PromptPipeline implement
tion pl
n — 10 t
sks, TDD, str
ngler fig- docs: 
dd PromptPipeline design — Tr
it-per-L
yer evolution from Pl
n A- docs: 
dd 
utom
tion skills implement
tion pl
n- docs: 
dd 
utom
tion skills (#21-30) design- docs: 
dd memory event sourcing implement
tion pl
n- docs: 
dd memory event sourcing design (CQRS Light)- docs: 
dd prompt system enh
ncement implement
tion pl
n- docs: 
dd prompt system enh
ncement design- docs: 
dd skills system, upd
te runtimes refs, 
dd m
cOS components- docs: upd
te 
ccept
nce results 
fter bridge fixes (27/30 p
ss)- docs: 
dd implement
tion pl
n for fixing bridge known issues- docs: 
dd design for fixing bridge known issues- docs: remove rem
ining Swift references from CLAUDE.md- docs: upd
te CLAUDE.md 
nd cre
te migr
tion completion record (C13-C16)- docs: 
dd m
cOS Swift 
pp remov
l implement
tion pl
n- docs: 
dd m
cOS Swift 
pp remov
l design with 
ccept
nce criteri
- docs: 
dd desktop c
p
bilities evolution implement
tion pl
n- docs: 
dd desktop c
p
bilities evolution design- docs: 
dd sem
ntic t
rgeting implement
tion pl
n- docs: 
dd sem
ntic t
rgeting 
nd 
ction primitives design- docs: upd
te CLAUDE.md for Server-Centric Build Architecture- docs: 
dd Ph
se 3 
nd Ph
se 4 implement
tion pl
ns- docs: repl
ce Ghost 
esthetic with concrete product constr
ints R5-R7- docs: 
dd Ph
se 2.5 bridge integr
tion completion pl
n- docs: 
dd design for removing Ghost 
esthetic concept- docs: 
dd Ph
se 1 bridge skeleton implement
tion pl
n- docs: 
dd server-centric build 
rchitecture design- docs: upd
te worktree guidelines with EnterWorktree CWD lock c
ve
t- docs: 
dd cron system redesign pl
n — surp
ssing opencl
w- docs: 
dd memory optimiz
tion implement
tion pl
n- docs: 
dd memory module optimiz
tion design- docs: 
ddress code review findings (JIT-
pprov
l TODO, RwLock r
tion
le)- docs: bring in L
te-Binding Secure Execution design 
nd pl
n from m
in- docs: 
dd L
te-Binding Secure Execution implement
tion pl
n (14 t
sks, 4 w
ves)- docs: 
dd L
te-Binding Secure Execution Architecture design- docs: 
dd git worktree s
fety guide; fix missing ScreenRegion import- docs: 
dd Rust ref
ctoring implement
tion pl
n (7 t
sks, 4 w
ves)- docs: 
dd Rust core ref
ctoring design (4-w
ve str
tegy)- docs: 
dd runtime on-dem
nd implement
tion pl
n (13 t
sks, 4 ph
ses)- docs: 
dd runtime on-dem
nd implement
tion pl
n (13 t
sks, 4 ph
ses)- docs: 
dd runtime on-dem
nd n
tive bootstr
pping 
rchitecture design- docs: 
dd verific
tion test results to T
uri shell design doc- docs: 
dd T
uri cross-pl
tform shell implement
tion pl
n- docs: 
dd T
uri cross-pl
tform shell & DesktopBridge design- build(h
lo): rebuild WASM with /h
lo route- docs: split CLAUDE.md 
nd reorg
nize docs/ into docs/reference/- docs: 
dd 1-2-3-4 
rchitecture constitution design document- docs: 
dd H
lo UI Unific
tion implement
tion pl
n (10 t
sks)- docs: est
blish 1-2-3-4 
rchitecture model 
s constitution
l principles in CLAUDE.md- build(m
cos): 
dd WebKit fr
mework dependency for Settings WebView- docs: 
dd Ph
se 1 implement
tion pl
n — Settings WebView integr
tion- docs: 
dd UI unific
tion design — Leptos 
s single UI codeb
se- docs: 
dd Desktop Bridge implement
tion pl
n (11 t
sks, 4 ph
ses)- docs: 
dd Desktop Bridge design for UDS-b
sed Swift-Rust IPC- docs: 
dd Skill System v2 implement
tion pl
n (15 TDD t
sks)- docs: 
dd Skill System v2 design (complete DDD rebuild)- docs: upd
te 
ll document
tion for server-centric 
rchitecture- docs: upd
te CLAUDE.md for server-centric 
rchitecture- docs: 
dd server purific
tion implement
tion pl
n- docs: 
dd server purific
tion design - remove desktop control, embr
ce MCP plugins- docs: 
dd Skill System implement
tion pl
n with 14 TDD t
sks- docs: 
dd server-centric 
rchitecture implement
tion pl
n- docs: 
dd server-centric 
rchitecture refr
ming design- docs: 
dd Skill System dom
in-driven design document- docs: 
dd P0 ref
ctoring implement
tion pl
n for st
rt.rs 
nd extension/mod.rs- docs: 
dd CODE_ORGANIZATION guide with ref
ctoring b
cklog- docs: 
dd soci
l connectivity evolution design 
nd implement
tion pl
n- build: 
dd missing imports in control-pl
ne cfg block- docs: 
dd IronCl
w Ph
se 2/3 det
iled implement
tion pl
n- docs: 
dd IronCl
w Ph
se 2/3 design (host-bound
ry + EVM signing)- docs: 
dd code cle
nup implement
tion pl
n (16 t
sks, 3 p
sses)- docs: 
dd code cle
nup design pl
n (Occ
m's R
zor P
ss)- docs: 
dd ACMA implement
tion pl
n with 7 TDD t
sks- docs: 
dd ACMA (Aleph Cognitive Memory Architecture) design document- docs: 
dd exec security integr
tion design- docs: 
dd blog post on PII filtering g
tew
y implement
tion- docs: 
dd 
gent secret m
n
gement implement
tion pl
n- docs: 
dd 
gent secret m
n
gement design (Ph
se 1)- docs: 
dd Discord Control Pl
ne implement
tion pl
n- docs: 
dd Discord Control Pl
ne p
nel design- docs: 
dd memory worksp
ce implement
tion pl
n- docs: 
dd memory worksp
ce isol
tion design- docs: upd
te 
rchitecture docs to reflect L
nceDB migr
tion- docs: 
dd Wh
tsApp Bridge implement
tion pl
n (10 t
sks)- docs: 
dd Wh
tsApp Bridge design (Thin Sidec
r + Rich Ad
pter)- docs: upd
te MEMORY_SYSTEM.md 
nd CLAUDE.md for L
nceDB migr
tion- docs: embedding evolution implement
tion pl
n (13 t
sks)- docs: embedding evolution design (
bstr
ct provider + l
zy migr
tion)- docs: 
dd Memory VFS Evolution implement
tion pl
n- docs: 
dd Memory VFS Evolution design document- docs: 
dd Sw
rm Agent Loop integr
tion implement
tion pl
n- docs: 
dd Sw
rm Intelligence Architecture Agent Loop integr
tion design- docs(ssb): 
dd Ph
se 6 cross-pl
tform implement
tion pl
n- docs(ssb): 
dd cross-pl
tform 
rchitecture design- docs: cl
rify server-side execution model in CLAUDE.md- docs(ssb): 
dd Ph
se 6 enh
ncement pl
n 
nd complete ro
dm
p- docs: 
dd Sw
rm Intelligence Architecture design- build(control-pl
ne): upd
te compiled UI 
ssets for Ph
se 3- docs: 
dd System St
te Bus (SSB) 
rchitecture design- docs(skill-evolution): 
dd comprehensive document
tion 
nd ex
mples- docs: 
dd Coll
bor
tive Skill Evolution 
rchitecture design- docs: 
dd det
iled implement
tion pl
n for Control Pl
ne three-column l
yout- docs: 
dd Control Pl
ne three-column l
yout 
rchitecture design- docs: upd
te Control Pl
ne UI build workflow with T
ilwind CSS compil
tion- docs(cl
ude.md): 
dd WASM initi
liz
tion mech
nism expl
n
tion- docs(cl
ude.md): 
dd comprehensive Server development 
nd deployment guide- docs: 
dd UI comp
rison 
n
lysis for ControlPl
ne 
nd T
uri settings- docs: 
dd WebSocket client implement
tion summ
ry 
nd migr
tion pl
n- docs: 
dd ControlPl
ne integr
tion implement
tion summ
ry- docs: 
dd Ph
se 3 implement
tion pl
n- docs: 
dd Ph
se 3 design for skill s
ndboxing- docs: 
dd comprehensive skill s
ndboxing document
tion- docs: 
dd Ph
se 2 skill s
ndboxing implement
tion pl
n- docs: 
dd Ph
se 2 skill s
ndboxing design document- docs(sh
red-ui-logic): m
rk API L
yer 
s complete- docs(sh
red-ui-logic): m
rk WASM connector 
s complete- docs(sh
red-ui-logic): upd
te README with API 
nd Observ
bility progress- docs(sh
red_ui_logic): upd
te README with protocol l
yer st
tus- docs(sh
red_ui_logic): upd
te README with n
tive connector st
tus- docs(sh
red_ui_logic): 
dd comprehensive README- docs: 
dd sh
red_ui_logic design document- docs: complete Ph
se 3 
rchitecture document
tion- docs: 
dd Ph
se 1 implement
tion pl
n for skill s
ndboxing- docs: 
dd skill s
ndboxing 
rchitecture design- docs(
rchitecture): 
dd comprehensive cle
nup design document- docs: reorg
nize root directory 
nd est
blish document
tion structure- docs(
rchitecture): 
dd Ph
se 3 browser ref
ctoring design- docs(
rchitecture): 
dd Ph
se 6 tools server ref
ctoring design- docs(
rchitecture): 
dd Ph
se 5 plugins h
ndlers ref
ctoring design- docs(
rchitecture): 
dd Ph
se 4 POE h
ndlers ref
ctoring design- docs: 
dd Ph
se 2 continu
tion guide for next session- docs(
rchitecture): 
dd Ph
se 2 
tomic executor ref
ctoring design- docs(
rchitecture): 
dd Ph
se 1 types ref
ctoring design- docs(cortex): 
dd Month 3 implement
tion pl
n- docs(cortex): 
dd Month 3 Met
-Cognition L
yer design- docs: 
dd Atomic Engine fin
l implement
tion report- docs: 
dd comprehensive Atomic Engine document
tion- docs: 
dd Atomic Engine progress report (90% complete)- docs: 
dd Atomic Engine short-term t
sk completion st
tus- docs: 
dd Cortex evolution system design- docs: 
dd Atomic Engine evolution ro
dm
p (3-12+ months)- docs: 
dd 
tomic engine implement
tion st
tus report- docs: 
dd l
ngu
ge preference to CLAUDE.md- docs: 
dd Ph
se 2 Intelligent Scheduling design- docs: 
dd guest session 
ctivity logging implement
tion pl
n- docs: 
dd Liquid Hub cross-pl
tform 
rchitecture design- docs: complete Identity Context security document
tion- docs: 
dd Identity Context & Security Enforcement design- docs: 
dd ConfigM
n
ger 
nd Memory N
mesp
ce implement
tion pl
n- docs: 
dd ConfigM
n
ger 
nd Memory N
mesp
ce design- docs: 
dd Person
l AI Hub implement
tion pl
n- docs: 
dd Person
l AI Hub 
rchitecture design- docs: 
dd client 
rchitecture document
tion 
nd testing guide- docs: 
dd Ph
se 2 progress report- docs: 
dd client 
rchitecture ref
ctoring pl
n- docs: document Server-Client 
rchitecture in CLAUDE.md- docs: 
dd Server-Client implement
tion pl
n- docs: 
dd Server-Client 
rchitecture design- docs: 
dd DDD terminology 
nd dom
in modeling guide- docs: 
dd DDD+BDD du
l-wheel 
rchitecture design- docs: 
dd comprehensive Tool-
s-Resource us
ge guide 
nd upd
te Ph
se 4 st
tus- docs: upd
te Ph
se 3 progress - L2 
nd observ
bility completed- docs: upd
te Ph
se 2 checkboxes to completed- docs: upd
te MEMORY_SYSTEM.md with Memory Evolution fe
tures- docs(bdd): 
dd comprehensive BDD testing guide 
nd upd
te pl
ns- docs: 
dd Ph
se 3 implement
tion pl
n- docs: m
rk Ph
se 2 
s complete with 
ll t
sks done- docs: document Ph
se 2 memory system components in TOOL_SYSTEM.md- docs: upd
te Ph
se 2 pl
n with completion st
tus- docs: upd
te implement
tion pl
n with completion summ
ry- docs: 
dd Ph
se 1 MVP implement
tion pl
n- docs: 
dd Multi-Agent 2.0 Ph
se 1 implement
tion pl
n- docs: 
dd memory system evolution design- docs: 
dd Multi-Agent Resilience document
tion- docs: upd
te Ph
se 1 checkboxes to completed- docs: upd
te Tool-
s-Resource design st
tus to In Progress- docs: 
dd Tool-
s-Resource implement
tion pl
n- docs: 
dd Multi-Agent Resilience & Govern
nce 
rchitecture design- docs: 
dd Tool-
s-Resource 
rchitecture design- docs: 
dd Embodiment Engine 
nd CoT Tr
nsp
rency document
tion- docs: 
dd Multi-Agent 2.0 
rchitecture design- docs(pl
ns): 
dd Embodiment Engine & CoT Tr
nsp
rency design- docs(
gent-system): 
dd Ch
nnel C
p
bility Aw
reness document
tion- docs: 
dd ch
nnel c
p
bility 
w
reness implement
tion pl
n- docs: 
dd ch
nnel c
p
bility 
w
reness 
rchitecture design- docs: 
dd worksp
ce 
rchitecture design- docs: 
dd Ph
se 5 implement
tion pl
n- docs: 
dd Ph
se 5 Custom Rules Engine 
rchitecture design- docs: 
dd WorldModel + Disp
tcher 
rchitecture design- docs(d
emon): 
dd perception l
yer document
tion- docs: 
dd Protocol Ad
pter Ph
se 4 implement
tion summ
ry- docs(
rchitecture): document configur
ble protocol 
d
pter system- docs(protocols): 
dd comprehensive protocol 
d
pter user guide- docs: 
dd Ph
se 2 Perception L
yer implement
tion pl
n- docs(protocols): 
dd ex
mple YAML protocol configur
tions- docs: 
dd Ph
se 2 Perception L
yer design- docs: 
dd d
emon module document
tion- docs: 
dd Ph
se 1 d
emon implement
tion pl
n- docs: 
dd pro
ctive AI 
rchitecture design- build: remove deprec
ted c
bi fe
ture 
nd fix Discord API- docs: 
dd comprehensive M
rkdown Tool Ad
pter implement
tion summ
ry- docs: 
dd Protocol Ad
pter Ph
se 4 design- docs: 
dd M
rkdown Tool Ad
pter design specific
tion- docs: 
dd Protocol Ad
pter Ph
se 3 implement
tion summ
ry- docs: 
dd Protocol Ad
pter Ph
se 2 implement
tion summ
ry- docs: 
dd Protocol Ad
pter Ph
se 2 implement
tion pl
n- docs: 
dd Protocol Ad
pter Ph
se 2 design for Cl
ude/Gemini migr
tion- docs(providers): upd
te module document
tion for Protocol Ad
pter 
rchitecture- docs: 
dd Protocol Ad
pter implement
tion pl
n- docs: 
dd Protocol Ad
pter 
rchitecture design- docs(pl
ns): 
dd P2.5 MCP Adv
nced Fe
tures implement
tion pl
n- docs(mcp): 
dd P2 
dv
nced fe
tures implement
tion pl
n- docs: 
dd Memory v3 implement
tion pl
n with bite-sized TDD t
sks- docs(mcp): 
dd P1 c
p
bilities implement
tion pl
n- docs: 
dd Memory System v3 "Gl
ss Box" 
rchitecture design- docs(mcp): 
dd MCP Orchestr
tion L
yer implement
tion pl
n- docs(mcp): 
dd MCP Orchestr
tion L
yer design- docs(cortex): 
dd det
iled implement
tion pl
n with TDD steps- docs(extension): 
dd P0.5-P2 fe
ture document
tion- docs(extension): 
dd P0.5-P2 implement
tion pl
n- docs(extension): 
dd SDK V2 document
tion- docs(disp
tcher): 
dd Cortex 2.0 
rchitecture design- docs(extension): 
dd SDK V2 P0 implement
tion pl
n- docs(extension): 
dd Aether Extension SDK V2 design specific
tion- docs(skills): 
dd det
iled implement
tion pl
n for requirements fe
ture- docs(skills): 
dd requirements & CLI wr
pper 
rchitecture design- docs(poe): 
dd contr
ct signing design for first principles closure- docs: upd
te memory system docs 
nd 
dd h
lo comm
nd system pl
n- docs: 
dd mess
ge flow optimiz
tion design 
nd implement
tion pl
n- docs: 
dd H
lo-Only mess
ge flow design 
nd implement
tion pl
n- docs: 
dd comprehensive 
rchitecture document
tion- docs: 
dd det
iled POE implement
tion pl
n- docs: 
dd POE (Principle-Oper
tion-Ev
lu
tion) 
rchitecture design- docs: 
dd Agent-Action inter
ction implement
tion pl
n- docs: 
dd Agent-Action inter
ction system design- docs: m
rk Milestone 6 (ResilientT
sk) 
s complete- docs: 
dd Rust l
yer code cle
nup design pl
n- docs: 
dd Milestone 6 resilient t
sk implement
tion pl
n- docs: m
rk Milestone 5 (skill evolution) 
s complete- docs: 
dd Milestone 5 skill evolution implement
tion pl
n- docs: m
rk Milestone 4 (spec-driven dev) 
s complete- docs: 
dd Milestone 4 spec-driven development implement
tion pl
n- docs: m
rk Milestone 3 (Telegr
m 
pprov
l) 
s complete

## [0.2.10] - 2026-03-23### Added- fe
t(p
nel): 
dd stre
ming, render_mode, typing_indic
tor fields to Feishu settings- fe
t(feishu): wire FeishuEventEmitter into execution flow- fe
t(feishu): 
dd m
rkdown c
rd rendering 
nd upd
ted c
p
bilities- fe
t(feishu): 
dd FeishuEventEmitter with stre
ming c
rds 
nd typing indic
tors- fe
t(feishu): 
dd C
rd Kit stre
ming, st
tic c
rd, 
nd re
ction API methods- fe
t(feishu): 
dd stre
ming, render_mode, typing config fields 
nd API types- fe
t(p
nel): 
dd Feishu/L
rk ch
nnel settings c
rd- fe
t(feishu): fix clippy w
rnings — unused import, visibility, closure- fe
t(feishu): 
dd FeishuCh
nnel impl 
nd wire into f
ctory registry- fe
t(feishu): 
dd FeishuClient with token, HTTP API, 
nd medi
 support- fe
t(feishu): 
dd WebSocket event p
rsing 
nd text extr
ction- fe
t(feishu): 
dd types, config, 
nd API response structs- fe
t: 
dd Persistent Completion Protocol for 
gent t
sk verific
tion- desktop-m
cos: implement PimC
p
bility vi
 SwiftBridge- desktop-m
cos: implement SystemC
p
bility (
pps, notific
tions, clipbo
rd, sysinfo)- desktop-m
cos: implement Autom
tionC
p
bility (os
script + Shortcuts CLI)- desktop: wire N
tiveScreen into 
ll pl
tform cr
tes- desktop: 
dd N
tiveScreen sh
red ScreenC
p
bility implement
tion- core: 
dd SystemTool 
nd Autom
tionTool builtin tools- desktop: 
dd per-pl
tform cr
te skeletons (m
cos, linux, windows)- desktop: 
dd SwiftBridge utility for m
cOS n
tive API c
lls- desktop: upd
te cr
te doc to reflect two-l
yer 
rchitecture- desktop: 
dd c
p
bility tr
it hier
rchy 
nd sh
red types- core: 
dd 
leph-client dependency for server bin
ry- fe
t: en
ble n
tive tool c
lling for Ch
tGPT/Codex Responses API- core: 
dd Strict Mode support (schem
 strictific
tion + provider integr
tion)- core: 
dd #[cfg(unix)] gu
rds for Unix socket code on Windows- desktop: fix Windows OCR compil
tion errors- fe
t(browser): 
dd profile config types 
nd browser system configur
tion- fe
t(browser): 
dd SsrfPolicy for URL v
lid
tion 
nd priv
te network blocking- fe
t(config): 
dd queue_mode session configur
tion with g
tew
y wiring- fe
t(
nthropic): wire c
che_control ephemer
l bre
kpoint for system prompt c
ching- fe
t(thinker): p
rtition system prompt into st
ble/dyn
mic zones for c
che optimiz
tion- fe
t(compressor): 
dd pre-comp
ction silent memory flush- fe
t(
gent-loop): 
dd CollectQueue with time-window mess
ge merging- fe
t(
gent-loop): 
dd SteerQueue with interrupt sign
ling- fe
t(
gent-loop): 
dd SessionQueue tr
it 
nd FollowupQueue implement
tion- fe
t(
gent-loop): wire interrupt ch
nnel into RunContext 
nd loop execution- fe
t(
gent-loop): 
dd InterruptCh
nnel for steering support- core: 
dd missing tr
cing::w
rn import for non-m
cOS builds- fe
t: unified sl
sh comm
nd system- fe
t: wire memory tools into 
gent execution + Two-Ph
se Sm
rt Rec
ll- fe
t(server): 
dd desktop fe
ture g
te for in-process desktop c
p
bilities- fe
t(desktop): integr
te DesktopC
p
bility into DesktopTool with du
l-p
th execution- fe
t(desktop): implement input 
ctions with enigo- fe
t(desktop): implement screenshot 
nd OCR vi
 xc
p- fe
t: 
dd 
leph-desktop cr
te skeleton with DesktopC
p
bility tr
it- desktop: fix T
uri build for m
cOS 
nd 
dd 
pp/dmg bundle t
rgets- fe
t(w
sm): register host functions vi
 PluginBuilder with c
p
bility kernel- fe
t(m
nifest): p
rse WASM c
p
bilities from 
leph.plugin.toml- fe
t(w
sm): 
dd W
smC
p
bilityKernel — per-execution security enforcement- fe
t(w
sm): 
dd Credenti
lInjector — plugins never see secrets- fe
t(w
sm): 
dd AllowlistV
lid
tor with 
nti-byp
ss security- fe
t(w
sm): 
dd W
smC
p
bilities types with def
ult-deny model- fe
t(exec): 
dd Le
kDetector with Aho-Cor
sick bidirection
l sc
nning- desktop: 
dd 
ll_d
y 
nd c
lend
r_id to PimC
lend
rUpd
te- desktop: 
dd PIM v
ri
nts to DesktopRequest 
nd JSON-RPC m
pping- desktop: remove m
cOS t
rget, 
dd server embedding for Linux/Windows- desktop: fix fl
ky tests th
t 
ssumed bridge socket 
bsence- desktop-bridge: implement Windows OCR (WinRT) 
nd UI Autom
tion AX tree- desktop-bridge: implement window m
n
gement (list, focus, l
unch)- desktop-bridge: implement Windows input simul
tion (click, type, key combo, scroll)- desktop: wire sn
pshot 
nd new 
ctions in DesktopBridgeServer disp
tch- desktop: implement scroll, double-click, dr
g, hover, p
ste, 
nd ref-
w
re t
rgeting- desktop: implement UI sn
pshot with ref gener
tion in Perception.swift- desktop: 
dd RefStore for sn
pshot ref m
n
gement (Swift)- desktop: upd
te tool 
rgs 
nd build_request for sn
pshot, ref t
rgeting, 
nd new 
ctions- desktop: 
dd core types for sn
pshot, ref system, 
nd new 
ction primitives- desktop: upd
te tool mess
ging for bridge 
rchitecture- desktop: probe m
n
ged 
nd st
nd
lone socket p
ths- fe
t(runtimes): 
dd ensure_c
p
bility orchestr
tion (Probe -> Bootstr
p -> Register)- fe
t(runtimes): wire C
p
bilityLedger into prompt system- fe
t(runtimes): 
dd bootstr
p module with shell-driven inst
ll
tion- fe
t(runtimes): wire ledger into exec l
yer PATH- fe
t(runtimes): 
dd Probe module for system-first c
p
bility detection- fe
t(runtimes): 
dd leg
cy m
nifest.json migr
tion to ledger.json- fe
t(runtimes): 
dd C
p
bilityLedger for lightweight runtime st
te tr
cking- fe
t(desktop): implement desktop.screenshot in T
uri DesktopBridge- fe
t(desktop): 
dd DesktopBridge UDS server with ping support- fe
t(protocol): 
dd desktop_bridge types for cross-pl
tform Bridge- fe
t(h
lo): switch m
cOS H
loWindow from SwiftUI to WKWebView- fe
t(h
lo): 
dd /h
lo route with ch
t UI, mess
ge list, 
nd input 
re
- fe
t(h
lo): 
dd event h
ndler to wire run.* stre
ming events to H
loSt
te- fe
t(h
lo): 
dd H
loSt
te re
ctive sign
ls for ch
t st
te m
n
gement- fe
t(h
lo): 
dd Ch
tApi module for ch
t.send/
bort/history/cle
r- fe
t(desktop): T
sk 11 complete — DesktopTool 
ctive in 
gent vi
 builtin registry- fe
t(desktop): implement WKWebView c
nv
s overl
y with A2UI p
tch support- fe
t(desktop): implement mouse, keybo
rd, 
nd window 
ctions in Action.swift- fe
t(desktop): 
dd 
ccessibility permission description 
nd runtime check- fe
t(desktop): implement screenshot, OCR, 
nd AX tree in Perception.swift- fe
t(desktop): point settings window to Leptos Control Pl
ne server- fe
t(m
cos): 
dd Settings menu item opening Control Pl
ne WebView- fe
t(m
cos): 
dd SettingsWebView WKWebView wr
pper- fe
t(desktop): 
dd Swift UDS server skeleton with stub h
ndlers- fe
t(desktop): register DesktopTool in executor builtin registry- fe
t(desktop): 
dd DesktopTool builtin with gr
ceful degr
d
tion- fe
t(desktop): 
dd UDS client with JSON-RPC 2.0 
nd unit tests- fe
t(desktop): 
dd types, error, 
nd module sc
ffold- fe
t(skill): integr
te SkillSystem v2 into ExtensionM
n
ger 
nd ExecutionEngine- fe
t(skill): 
dd SkillSystem f
c
de with Arc<Inner> p
ttern- fe
t(skill): 
dd sl
sh comm
nd resolution- fe
t(skill): 
dd Inst
llSpec to shell comm
nd converter- fe
t(skill): 
dd SkillSt
tusReport for eligibility d
shbo
rd- fe
t(skill): 
dd SkillSn
pshot with version-inv
lid
ted c
che- fe
t(skill): 
dd XML prompt builder for skill injection- fe
t(skill): 
dd EligibilityService with OS/bin
ry/env checks- fe
t(skill): 
dd SKILL.md p
rser with YAML frontm
tter support- fe
t(skill): 
dd SkillRegistry with priority-b
sed dedup- fe
t(skill): 
dd SkillM
nifest Aggreg
teRoot with Entity tr
it- fe
t(skill): 
dd EligibilitySpec, Inst
llSpec, Invoc
tionPolicy, PromptScope V
lueObjects- fe
t(skill): 
dd SkillId, PluginId, SkillSource dom
in types- fe
t(thinker): 
dd skill_instructions to PromptConfig for SkillSystem v2- fe
t(extension): 
dd SkillSystem v2 
nd wire skill XML into 
gent prompts- fe
t(sw
rm): 
dd event st
tistics 
nd logging- fe
t(
gent_loop): integr
te ContextProvider into Mess
geBuilder- fe
t(sw
rm): implement Sw
rmContextProvider- fe
t(
gent_loop): define ContextProvider tr
it- fe
t(
gent_loop): implement event publishing (sh
dow mode)- fe
t(
gent_loop): define AgentLoopEvent enum- fe
t(
gent_loop): implement Builder build() method- fe
t(
gent_loop): 
dd AgentLoopBuilder structure- fe
t(perception): integr
te PAL with SystemSt
teBus- fe
t(perception): 
dd Pl
tform Abstr
ction L
yer (PAL)- fe
t(sw
rm): Ph
se 5 - End-to-End Integr
tion- fe
t(perception): implement Ph
se 5 - Document
tion, Ex
mples & Testing- fe
t(perception): implement Ph
se 4 - Vision Connector 
rchitecture- fe
t(ssb): implement Ph
se 3 - 
ction disp
tcher- fe
t(ssb): implement Ph
se 2 - robustness & priv
cy- fe
t(ssb): implement Ph
se 1 - core infr
structure- fe
t(control-pl
ne): implement WebSocket subscription for re
l-time 
lerts- fe
t(sh
red_ui_logic): 
dd 
lerts API module for system he
lth 
nd memory monitoring- fe
t(skill-evolution): integr
te SuccessM
nifest with tool execution- fe
t(control-pl
ne): p
ss mode 
nd 
lert_key to Sideb
rItems- fe
t(control-pl
ne): integr
te Tooltip 
nd B
dge into Sideb
rItem- fe
t(control-pl
ne): 
dd St
tusB
dge component for 
lert indic
tors- fe
t(control-pl
ne): 
dd Tooltip component for n
rrow mode l
bels- fe
t(skill-evolution): implement Coll
bor
tiveSolidific
tionPipeline- fe
t(control-pl
ne): implement Sideb
r n
rrow/wide mode switching- fe
t(skill-evolution): implement Constr
intV
lid
tor- fe
t(skill-evolution): implement SuccessM
nifest d
t
 structure- fe
t(control-pl
ne): 
dd SettingsL
yout for nested routing- fe
t(control-pl
ne): 
dd 
lert bus 
nd sideb
r mode override to D
shbo
rdSt
te- fe
t(control-pl
ne): 
dd sideb
r types (Sideb
rMode, AlertLevel, SystemAlert)- fe
t(control-pl
ne): compile T
ilwind CSS loc
lly for production- fe
t(d
shbo
rd): 
dd Plugins, Skills, 
nd Policies settings p
ges- fe
t(d
shbo
rd): 
dd sideb
r n
vig
tion to settings UI- fe
t(d
shbo
rd): 
dd Gener
tion Providers n
vig
tion c
rd to Settings p
ge- fe
t(d
shbo
rd): implement Gener
tion Providers CRUD function
lity- fe
t(d
shbo
rd): 
dd Gener
tion Providers frontend UI- fe
t(d
shbo
rd): 
dd Gener
tion Providers b
ckend 
nd API l
yer- fe
t(d
shbo
rd): implement comprehensive configur
tion m
n
gement UI- fe
t(m
cos): implement WebSocket client for G
tew
y connection- fe
t(m
cos): complete Ph
se 4 client simplific
tion for ControlPl
ne integr
tion- fe
t(d
shbo
rd): complete Ph
se 3 SDK integr
tion with RPC, events, 
nd API l
yer- fe
t(d
shbo
rd): complete Ph
se 2 SDK integr
tion with error h
ndling 
nd reconnection- fe
t(d
shbo
rd): 
dd connection st
te 
w
reness to Memory view- fe
t(d
shbo
rd): integr
te sh
red_ui_logic SDK into D
shbo
rd- fe
t(d
shbo
rd): full 
rchitectu
l ref
ctor with Leptos 0.8.15 
nd rust-ui components- fe
t(d
shbo
rd): complete Memory Explorer view 
nd fix System St
tus- fe
t(d
shbo
rd): initi
lize Aleph D
shbo
rd with Leptos 0.6- fe
t(sh
red-ui-logic): implement Plugins 
nd Providers APIs- fe
t(sh
red-ui-logic): implement WASM WebSocket connector- fe
t(sh
red-ui-logic): implement API 
nd Observ
bility l
yers- fe
t(sh
red_ui_logic): implement protocol l
yer- fe
t(sh
red_ui_logic): implement n
tive WebSocket connector- fe
t(sh
red_ui_logic): initi
lize Aleph UI Logic SDK- fe
t(cortex): implement LLM-b
sed critic report gener
tion- fe
t(cortex): 
dd AiProvider to CriticAgent- fe
t(cortex): implement LLM-b
sed root c
use 
n
lysis- fe
t(cortex): 
dd AiProvider to Re
ctiveReflector- fe
t(
gent_loop): 
dd met
-cognition integr
tion for Ph
se 6- fe
t(cortex): implement CortexIntegr
tion orchestr
tor (T
sk #11)- fe
t(cortex): implement experience clustering 
nd deduplic
tion- fe
t(disp
tcher): implement L1.5 ExperienceRepl
yL
yer- fe
t(cortex): implement Cortex Dre
ming b
ckground service- fe
t(cortex): implement LLM-b
sed p
ttern extr
ction- fe
t(cortex): implement Distill
tionService core structure- fe
t(engine): 
dd Fe
tureExtr
ctor for 
dv
nced ML rule le
rning- fe
t(cortex): implement multi-dimension
l experience v
lue estim
tor- fe
t(cortex): 
dd 
gent loop telemetry c
pture- fe
t(cortex): implement Experience CRUD oper
tions- fe
t(cortex): define core d
t
 structures- fe
t(engine): 
dd ML-b
sed L2 rule gener
tion (RuleLe
rner)- fe
t(cortex): 
dd experience_repl
ys d
t
b
se t
ble- fe
t(builtin_tools): 
dd AtomicOpsTool for 
tomic oper
tions- fe
t(browser): implement J
v
Script-b
sed context freeze/resume- fe
t(browser): implement Ph
se 2.4 CDP integr
tion for context freeze/resume- fe
t(engine): 
dd comprehensive testing 
nd perform
nce v
lid
tion- fe
t(executor): 
dd AtomicActionExecutor with L1/L2 routing- fe
t(engine): implement 
tomic engine with L1/L2/L3 routing- fe
t(disp
tcher): implement Ph
se 2 Intelligent Scheduling for Liquid Hub- fe
t(m
cos): 
dd guest session 
ctivity log UI- fe
t(m
cos): 
dd 
ctivity log RPC types 
nd methods- fe
t(g
tew
y): 
dd RPC request 
ctivity logging for guest sessions- fe
t(g
tew
y): 
dd guests.getActivityLogs RPC h
ndler- fe
t(g
tew
y): integr
te 
ctivity logging into GuestSessionM
n
ger- fe
t: implement guests.revokeInvit
tion RPC method- fe
t(m
cos): 
dd Guest m
n
gement UI in Settings- fe
t(g
tew
y): register config.get 
nd config.p
tch RPC h
ndlers- fe
t(g
tew
y): 
dd SessionIdentityMet
 for identity stor
ge- fe
t(protocol): 
dd IdentityContext for st
teless security- fe
t(g
tew
y): 
dd config.p
tch RPC h
ndler with events- fe
t(memory): 
dd idempotent n
mesp
ce migr
tion- fe
t(g
tew
y): 
dd RPC h
ndlers for guest m
n
gement- fe
t(memory): 
dd n
mesp
ce column for d
t
 isol
tion- fe
t(protocol): 
dd discovery types for mDNS- fe
t(protocol): 
dd ConfigCh
ngedEvent for config sync- fe
t(g
tew
y): 
dd Invit
tionM
n
ger for guest invit
tions- fe
t(protocol): 
dd invit
tion types for guest m
n
gement- fe
t(g
tew
y): 
dd PolicyEngine for permission checks- fe
t(g
tew
y): 
dd IdentityM
p for extern
l identity resolution- fe
t(protocol): 
dd Role 
nd GuestScope for Owner+Guest model- fe
t(ph
se3): complete T
uri Desktop migr
tion to thin client- fe
t(ph
se3): migr
te T
uri Desktop to SDK 
rchitecture (WIP)- fe
t(ph
se2): ref
ctor CLI to use SDK- fe
t(ph
se2): implement G
tew
yClient with 
uthentic
tion- fe
t(ph
se2): implement tr
nsport 
nd RPC l
yers in SDK- fe
t(ph
se2): cre
te 
leph-client-sdk skeleton- fe
t(g
tew
y): 
dd Server-Client routing infr
structure to ConnectionSt
te- fe
t: 
dd tool routing config 
nd scope checking for Server-Client 
rchitecture- fe
t(executor): integr
te RoutedExecutor with Agent Loop- fe
t(cli): cre
te 
leph-cli 
s protocol reference implement
tion- fe
t(protocol): cre
te 
leph-protocol cr
te for sh
red types- fe
t(executor): integr
te ToolRouter with execution engine- fe
t(disp
tcher): 
dd execution_policy field to UnifiedTool- fe
t(executor): 
dd ToolRouter for Server-Client routing decisions- fe
t(g
tew
y): 
dd tool.c
ll protocol mess
ges- fe
t(g
tew
y): 
dd ReverseRpcM
n
ger for Server-to-Client c
lls- fe
t(g
tew
y): store ClientM
nifest in ConnectionSt
te- fe
t(g
tew
y): extend ConnectP
r
ms to 
ccept ClientM
nifest- fe
t(g
tew
y): 
dd ClientM
nifest for c
p
bility negoti
tion- fe
t(disp
tcher): 
dd ExecutionPolicy enum for Server-Client routing- fe
t(spec_driven): implement BDD du
l-tr
ck testing system- fe
t(dom
in): implement DDD found
tion with m
rker tr
its- fe
t(disp
tcher): implement L2 
sync LLM enh
ncement for tool descriptions- fe
t(memory): 
dd perform
nce monitoring for LLM c
lls- fe
t(scheduler): implement recursion depth tr
cking- fe
t(scheduler): implement 
nti-st
rv
tion logic- fe
t(scheduler): implement L
neScheduler core- fe
t: implement CompressionD
emon for b
ckground compression scheduling- fe
t(scheduler): implement L
neSt
te with queue 
nd sem
phore- fe
t: enh
nce ContextComptroller with priority-b
sed token m
n
gement- fe
t: implement V
lueEstim
tor for memory import
nce scoring- fe
t(scheduler): 
dd l
ne scheduler infr
structure- fe
t: 
dd sliding window chunking to Tr
nscriptIndexer- fe
t: 
dd Tr
nscriptIndexer for ne
r-re
ltime memory indexing- fe
t(sub_
gents): 
dd 
ctive runs query 
nd st
ts to SubAgentRegistry- fe
t(sub_
gents): 
dd F
ctsDB persistence helpers for SubAgentRun- fe
t(sub_
gents): 
dd st
te tr
nsition to SubAgentRegistry- fe
t(sub_
gents): 
dd SubAgentRegistry with in-memory indexing- fe
t(memory): 
dd SubAgent f
ct types for Multi-Agent 2.0 persistence- fe
t(sub_
gents): 
dd SubAgentRun d
t
 model for Multi-Agent 2.0- fe
t(disp
tcher): integr
te Hydr
tionPipeline into Agent Loop- fe
t(core): export tool_index types from lib.rs- fe
t(memory): 
dd VectorD
t
b
se::in_memory() for testing- fe
t(disp
tcher): 
dd ToolRetriev
l with du
l-threshold hydr
tion- fe
t(disp
tcher): 
dd ToolIndexCoordin
tor for Memory synchroniz
tion- fe
t(disp
tcher): 
dd Sem
nticPurposeInferrer for L0/L1 inference- fe
t(disp
tcher): 
dd tool_index module with ToolRetriev
lConfig- fe
t(memory): 
dd Tool v
ri
nt to F
ctType for tool-
s-resource- fe
t(memory): 
dd Multi-Agent Resilience d
t
b
se l
yer- fe
t(g
tew
y): 
dd identity m
n
gement RPC h
ndlers- fe
t(thinker): 
dd thinking tr
nsp
rency guid
nce to PromptBuilder- fe
t(
gent_loop): integr
te ThinkingP
rser into DecisionP
rser- fe
t(g
tew
y): 
dd Re
soningBlock 
nd Uncert
intySign
l stre
m events- fe
t(
gent_loop): 
dd ThinkingP
rser for sem
ntic re
soning extr
ction- fe
t(
gent_loop): 
dd StructuredThinking types for CoT Tr
nsp
rency- fe
t(thinker): integr
te Soul into PromptBuilder- fe
t(thinker): 
dd m
rkdown p
rser for soul.md files- fe
t(thinker): 
dd IdentityResolver for l
yered identity resolution- fe
t(thinker): 
dd SoulM
nifest types for Embodiment Engine- fe
t(test): migr
te logging, security, 
nd e2e tests to BDD- fe
t(test): migr
te iMess
ge routing 
nd sub
gent tests to BDD- fe
t(g
tew
y): 
dd Ch
nnelProvider tr
it for inter
ction m
nifests- fe
t(
gent_loop): 
dd Silent 
nd He
rtbe
tOk decision types- fe
t(thinker): 
dd environment contr
ct 
nd security sections to PromptBuilder- fe
t(thinker): 
dd ContextAggreg
tor for environment reconcili
tion- fe
t(test): migr
te m
rkdown skills tests to BDD- fe
t(thinker): 
dd SecurityContext for policy-driven permissions- fe
t(thinker): 
dd Inter
ctionM
nifest for ch
nnel c
p
bility 
w
reness- fe
t(test): migr
te models 
nd protocol integr
tion tests to BDD- fe
t(test): migr
te DAG 
nd worldmodel disp
tcher tests to BDD- fe
t(test): migr
te sm
rt tool discovery 
nd sessions tests to BDD- fe
t(thinker): 
dd provider-specific context c
ching str
tegies- fe
t(disp
tcher): 
dd du
l-l
yer profile-b
sed tool filtering- fe
t(test): migr
te extension v2 
nd runtime tests to BDD- fe
t(g
tew
y): 
dd Worksp
ceM
n
ger for Anti-Gr
vity Architecture- fe
t(test): migr
te extension plugin registry tests to BDD- fe
t(test): migr
te tool server tests to BDD- fe
t(test): migr
te g
tew
y inbound router tests to BDD- fe
t(test): migr
te disp
tcher cortex tests to BDD- fe
t(test): migr
te memory integr
tion tests to BDD- fe
t(tests): migr
te memory f
cts tests to BDD- fe
t(tests): migr
te mess
ge builder tests to BDD- fe
t(tests): migr
te thinker prompt builder tests to BDD- fe
t(tests): migr
te POE tests to BDD- fe
t(tests): migr
te 
gent loop tests to BDD- fe
t(config): 
dd ProfileConfig for Worksp
ce Architecture- fe
t(tests): migr
te perception 
nd w
tcher tests to BDD- fe
t(tests): migr
te d
emon IPC 
nd l
unchd tests to BDD- fe
t(tests): migr
te d
emon core tests to BDD- fe
t(tests): migr
te config v
lid
tion tests to BDD- fe
t(tests): migr
te config b
sic tests to BDD- fe
t(tests): migr
te scripting engine tests to BDD- fe
t(tests): 
dd cucumber BDD infr
structure- fe
t: 
dd ex
mple YAML policies 
nd E2E tests- fe
t(disp
tcher): 
dd YAML policy lo
der 
nd PolicyEngine integr
tion- fe
t(disp
tcher): implement Y
mlPolicy with Rh
i ev
lu
tion- fe
t(scripting): 
dd B
selineApi with l
zy TTL c
ching- fe
t(scripting): implement HistoryApi.l
st() with WorldModel queries- fe
t(scripting): implement EventApi 
nd EventCollection filtering- fe
t(scripting): 
dd HistoryApi 
nd EventCollection stubs- fe
t(scripting): 
dd dur
tion p
rsing 
nd helpers for Rh
i- fe
t(disp
tcher): 
dd YAML rule schem
 p
rsing- fe
t(disp
tcher): 
dd Rh
i s
ndbox engine with strict limits- fe
t(worldmodel): 
dd JSON st
te persistence- fe
t(disp
tcher): 
dd core d
t
 structures- fe
t(d
emon): integr
te perception l
yer with d
emon CLI- fe
t(d
emon): implement FSEventW
tcher- fe
t(d
emon): implement SystemSt
teW
tcher- fe
t(d
emon): implement ProcessW
tcher- fe
t(d
emon): implement TimeW
tcher- fe
t(d
emon): 
dd w
tcher tr
it 
nd registry- fe
t(d
emon): 
dd perception configur
tion system- fe
t(d
emon): 
dd event system found
tion- fe
t(protocols): implement hot relo
d with notify file w
tching- fe
t(protocols): implement ProtocolLo
der file 
nd directory lo
ding- fe
t(protocols): implement Configur
bleProtocol custom mode with templ
te rendering- fe
t(protocols): implement Configur
bleProtocol minim
l mode (extends b
se + differences)- fe
t(protocols): 
dd JSONP
th p
rser for response v
lue extr
ction- fe
t(protocols): 
dd templ
te engine wr
pper for request/response tr
nsform
tion- fe
t(protocols): 
dd dependencies for configur
ble protocols (h
ndleb
rs, jsonp
th, notify)- fe
t(providers): 
dd ProtocolLo
der stub for hot relo
d- fe
t(providers): 
dd Configur
bleProtocol stub- fe
t(providers): implement ProtocolRegistry for dyn
mic protocol m
n
gement- fe
t(providers): 
dd ProtocolDefinition types for YAML configs- fe
t(tools): implement Virtu
lFs s
ndbox mode- fe
t(tools): 
dd Evolution 
uto-lo
d integr
tion- fe
t(g
tew
y): 
dd M
rkdown Skills RPC h
ndlers- fe
t(tools): 
dd repl
ce_tool() API with explicit upd
te sem
ntics- fe
t(tools): 
dd hot relo
d support for M
rkdown Skills (Ph
se 4)- fe
t(tools): 
dd Evolution Loop integr
tion for M
rkdown Skills (Ph
se 3)- fe
t(tools): 
dd ex
mples() method to AetherTool tr
it (Ph
se 2)- fe
t(tools): complete M
rkdown Tool Ad
pter integr
tion- fe
t(tools): implement M
rkdown Tool Ad
pter (Ph
se 1)- fe
t(providers): 
dd Tier 3 speci
lized OpenAI-comp
tible provider presets- fe
t(providers): 
dd Tier 2 OpenAI-comp
tible provider presets- fe
t(providers): 
dd Tier 1 OpenAI-comp
tible provider presets- fe
t(providers): 
dd Gemini presets 
nd upd
te f
ctory- fe
t(providers): implement GeminiProtocol 
d
pter- fe
t(providers): 
dd Gemini API types module- fe
t(providers): 
dd Cl
ude/Anthropic presets- fe
t(providers): implement AnthropicProtocol 
d
pter- fe
t(providers): 
dd Anthropic API types module- fe
t(g
tew
y): 
dd 
pprov
l RPC h
ndlers- fe
t(mcp): 
dd Approv
lH
ndler for hum
n-in-the-loop- fe
t(mcp): 
dd 
pprov
l request types for hum
n-in-the-loop- fe
t(mcp): 
dd stre
ming types for s
mpling responses- fe
t(mcp): 
dd TokenRefreshM
n
ger for 
utom
tic token refresh- fe
t(mcp): 
dd OAuth token refresh support- fe
t(mcp): integr
te context injection with S
mplingH
ndler- fe
t(mcp): 
dd ContextInjector for cross-server context- fe
t(mcp): 
dd IncludeContext enum type for s
mpling requests- fe
t(config): 
dd protocol field to ProviderConfig- fe
t(providers): 
dd provider presets registry- fe
t(providers): 
dd HttpProvider cont
iner with ProtocolAd
pter- fe
t(providers): implement OpenAiProtocol 
d
pter- fe
t(providers): 
dd ProtocolAd
pter tr
it with stre
ming support- fe
t(providers): 
dd RequestP
ylo
d DTO for protocol 
d
pters- fe
t(mcp): 
dd s
mpling c
llb
ck integr
tion to McpM
n
ger- fe
t(mcp): 
dd response mech
nism for server-initi
ted requests- fe
t(mcp): integr
te S
mplingH
ndler with McpClient- fe
t(memory): complete Memory v3 Milestones 4-6- fe
t(mcp): 
dd S
mplingH
ndler for server-initi
ted LLM c
lls- fe
t(mcp): implement re
l SSE event listening with reqwest-eventsource- fe
t(mcp): 
dd SSE event types 
nd reqwest-eventsource dependency- fe
t(memory): implement CLI list 
nd show comm
nds- fe
t(memory): implement AuditLogger for oper
tion tr
cking- fe
t(mcp): 
dd S
mpling RPC types for P2 server-initi
ted LLM c
lls- fe
t(memory): 
dd 
udit log schem
 
nd types- fe
t(memory): 
dd CLI module with file locking- fe
t(memory): implement Archiv
lService for scr
tchp
d 
rchiving- fe
t(memory): implement HybridTrigger with token threshold s
fety net- fe
t(memory): implement L
zyDec
yEngine for re
d-time dec
y ev
lu
tion- fe
t(memory): 
dd type-
w
re dec
y c
lcul
tion with tempor
l scope- fe
t(memory): 
dd dec
y_inv
lid
ted_
t field for recycle bin- fe
t(memory): complete Milestone 1 - Scr
tchp
d Found
tion- fe
t(memory): implement Scr
tchp
dM
n
ger with CRUD oper
tions- fe
t(memory): implement SessionHistory for scr
tchp
d 
rchiv
l- fe
t(memory): 
dd scr
tchp
d module structure 
nd templ
te- fe
t(mcp): implement re
l McpResourceM
n
ger 
nd McpPromptM
n
ger- fe
t(tools): 
dd mcp_get_prompt builtin tool- fe
t(tools): 
dd mcp_re
d_resource builtin tool- fe
t(mcp): implement re
l 
ggreg
tion for resources 
nd prompts- fe
t(mcp): 
dd resources 
nd prompts methods to McpClient- fe
t(mcp): 
dd resources 
nd prompts support to McpServerConnection- fe
t(mcp): 
dd Resources 
nd Prompts RPC types- fe
t(mcp): 
dd he
lth check logic for servers- fe
t(g
tew
y): wire MCP h
ndlers to McpM
n
gerH
ndle- fe
t(mcp): implement McpM
n
gerActor core loop- fe
t(mcp): 
dd config persistence for McpM
n
ger- fe
t(mcp): 
dd McpM
n
gerH
ndle public API- fe
t(mcp): 
dd McpComm
nd 
nd McpM
n
gerEvent types- fe
t(cortex): implement DecisionConfig with session override- fe
t(cortex): implement security rules (t
g injection, PII m
sking, instruction override)- fe
t(cortex): 
dd S
nitizerRule tr
it 
nd SecurityPipeline- fe
t(cortex): 
dd greedy JSON rep
ir logic- fe
t(cortex): implement JsonStre
mDetector st
te m
chine- fe
t(cortex): 
dd module skeleton with unified error types- fe
t(extension): 
dd PluginHttpH
ndler for plugin REST routes- fe
t(extension): 
dd PluginProviderAd
pter for plugin AI providers- fe
t(extension): 
dd Ch
nnelM
n
ger skeleton for plugin ch
nnels- fe
t(extension): 
dd HTTP route types- fe
t(extension): 
dd provider plugin types- fe
t(extension): 
dd ch
nnel plugin types- fe
t(g
tew
y): 
dd service lifecycle RPC h
ndlers- fe
t(extension): integr
te ServiceM
n
ger with ExtensionM
n
ger- fe
t(extension): 
dd ServiceM
n
ger for b
ckground services- fe
t(extension): 
dd service lifecycle types- fe
t(g
tew
y): 
dd plugins.executeComm
nd RPC h
ndler- fe
t(extension): 
dd comm
nd execution to PluginLo
der- fe
t(extension): 
dd DirectComm
ndResult type- fe
t(extension): implement scope-
w
re skill injection- fe
t(extension): implement V2 prompt lo
ding with scope support- fe
t(extension): 
dd scope 
nd bound_tool to ExtensionSkill- fe
t(extension): 
dd PromptScope enum for V2 skill injection- fe
t(extension): 
dd V2 hook conversion from TOML m
nifest- fe
t(extension): implement typed hook execution (interceptor/observer/resolver)- fe
t(extension): 
dd kind 
nd priority to HookConfig- fe
t(extension): 
dd HookKind 
nd HookPriority enums- fe
t(extension): integr
te TOML p
rser with 
uto-detection (TOML > JSON)- fe
t(extension): 
dd V2 fields to PluginM
nifest- fe
t(extension): 
dd TOML m
nifest p
rser types- fe
t(exec): check skill_
llowlist in 
pprov
l decision- fe
t(exec): 
dd skill_
llowlist config option- fe
t(exec): extend ExecContext with skill origin info- fe
t(skills): implement CLI Wr
pper v
lid
tor- fe
t(skills): 
dd he
lth checking methods to SkillsRegistry- fe
t(skills): 
dd inst
ll suggestion methods to SkillsInst
ller- fe
t(skills): implement He
lthChecker for dependency v
lid
tion- fe
t(skills): extend SkillFrontm
tter with requirements 
nd met
d
t
- fe
t(skills): 
dd types for requirements 
nd he
lth checking- fe
t(poe): repl
ce Pl
ceholderWorker with re
l AgentLoopWorker- fe
t(g
tew
y): wire POE contr
ct signing to G
tew
y- fe
t(poe): implement contr
ct signing workflow for first principles closure- fe
t(core): 
dd sn
pshot c
pture tool 
nd registry upd
tes- fe
t(config): 
dd memory configur
tion types 
nd v
lid
tion- fe
t(memory): enh
nce retriev
l 
nd 
dd dre
ming module- fe
t(m
cos): 
dd tool emoji form
tting to H
loStre
mingView- fe
t(m
cos): upd
te G
tew
yStre
mAd
pter with enh
nced summ
ry- fe
t(m
cos): 
dd H
loResultViewV2 with det
il popover support- fe
t(m
cos): 
dd H
loResultDet
ilPopover for det
iled results- fe
t(m
cos): 
dd Enh
ncedRunSumm
ry 
nd ToolSumm
ryItem models- fe
t(g
tew
y): 
dd Enh
ncedRunSumm
ry 
nd per-runId sequences- fe
t(g
tew
y): 
dd mess
ge deduplic
tion with text norm
liz
tion- fe
t(g
tew
y): 
dd stre
m buffer for block-level text flushing- fe
t(g
tew
y): 
dd tool displ
y module with emoji 
nd sm
rt form
tting- fe
t(h
lo): integr
te comm
ndList st
te into H
loViewV2- fe
t(h
lo): 
dd H
loComm
ndListView for / comm
nd p
nel- fe
t(h
lo): 
dd Comm
ndItem 
nd Comm
ndListContext types for / comm
nd- fe
t(h
lo): 
dd H
loInputCoordin
tor for lightweight input h
ndling- fe
t(g
tew
y): 
dd 150ms throttling for response chunks- fe
t(h
lo): 
dd H
loViewV2 m
in component integr
ting 
ll st
te views- fe
t(h
lo): 
dd H
loHistoryListView for convers
tion history- fe
t(h
lo): 
dd H
loResultView for comp
ct result displ
y- fe
t(h
lo): 
dd H
loStre
mingView for unified stre
ming displ
y- fe
t(h
lo): 
dd H
loSt
teV2 with 6 simplified st
tes- fe
t(h
lo): 
dd new stre
ming types for simplified st
te model- fe
t(skill-evolution): implement Skill Compiler (Ph
se 10)- fe
t(
gent-loop): 
dd on_user_question method to LoopC
llb
ck- fe
t(
gent-loop): 
dd AskUserRich decision v
ri
nt with QuestionKind- fe
t(
gent-loop): export question 
nd 
nswer modules- fe
t(
gent-loop): 
dd UserAnswer type for structured responses- fe
t(
gent-loop): 
dd QuestionKind types for structured user inter
ction- fe
t(resilient): 
dd cron integr
tion with Podc
stT
sk ex
mple- fe
t(resilient): implement ResilientExecutor with retry 
nd f
llb
ck- fe
t(resilient): define ResilientT
sk tr
it- fe
t(resilient): 
dd core types for resilient t
sk execution- fe
t(skill_evolution): implement GitCommitter for 
uto-commit- fe
t(skill_evolution): implement SkillGener
tor for SKILL.md cre
tion- fe
t(skill_evolution): implement Solidific
tionDetector for p
ttern detection- fe
t(skill_evolution): implement EvolutionTr
cker for execution logging- fe
t(skill_evolution): 
dd core types for skill evolution system- fe
t(spec_driven): implement SpecDrivenWorkflow orchestr
tor- fe
t(spec_driven): implement LlmJudge for ev
lu
tion- fe
t(spec_driven): implement TestWriter for test gener
tion- fe
t(spec_driven): implement SpecWriter for requirement 
n
lysis- fe
t(spec_driven): 
dd core types for spec-driven workflow- fe
t(g
tew
y): 
dd exec.c
llb
ck.h
ndle RPC for 
pprov
l c
llb
cks- fe
t(telegr
m): 
dd edit_mess
ge method for 
pprov
l upd
tes- fe
t(g
tew
y): 
dd 
pprov
l bridge h
ndler utilities- fe
t(exec): 
dd Approv
lBridge for ch
nnel integr
tion- fe
t(telegr
m): 
dd c
llb
ck query h
ndling- fe
t(telegr
m): 
dd inline keybo
rd support### Fixed- fix: 
dd tool_c
ll_id to OpenAI tool result mess
ges- fix: unignore CHANGELOG.md, fix rele
se recipe git 
dd- fix: remove unused imports 
cross codeb
se (c
rgo fix)- fix: resolve 42 test w
rnings — deprec
ted API, unused imports, de
d code- fix: sl
sh comm
nd f
st-p
th + CLI 
rg p
rser + E2E tests- fix: en
ble sl
sh comm
nd f
st-p
th for WebCh
t ch
t.send- fix: repl
ce env!("HOME") with dirs::home_dir() for Windows comp
tibility- fix: correct PluginKind::Mcp m
pping 
nd remove debug output- fix: upd
te discovery to find CC-form
t plugins in inst
lled/ directory- fix: ch
nnel binding not repl
cing old peer_id rows- fix: ch
nnel st
tus showing disconnected 
fter p
ge refresh- fix: p
ss session_m
n
ger to BuiltinToolConfig for session tools- fix: resolve 
gent from session_key inste
d of Worksp
ceM
n
ger- fix: sep
r
te 
gent identity files from worksp
ce directory- fix: use bold *n
me* for 
gent prefix inste
d of [n
me]- fix: use M
rkdown (leg
cy) inste
d of M
rkdownV2 for Telegr
m mess
ges- fix: remove b
cksl
sh esc
ping from 
gent n
me prefix in replies- fix: override rel
tive working_dir with 
gent worksp
ce- fix: ch
nge def
ult worksp
ce root from 
gents/ to worksp
ces/- fix: def
ult b
sh/code_exec working directory to 
gent worksp
ce- fix: register JSON Schem
 for 
ll builtin tools + Codex protocol 
lignment- fix: prevent token regener
tion on HMAC mism
tch to protect v
ult secrets- fix: Codex SSE function_c
ll_
rguments delt
 collection + logging- fix: use v
ult_key() function inste
d of undefined VAULT_KEY const
nt- fix: unify rer
nking v
ult key form
t with other modules- fix: rer
nking P
nel fetches per-provider API key from v
ult- fix: cle
r 
pi_key from rer
nking config sign
l 
fter s
ve- fix: isol
te rer
nk API keys per provider in v
ult- fix: move rer
nk API key from config.toml to encrypted v
ult- fix: correct def
ult rer
nking model n
me in P
nel 
nd tests- fix: ACP p
nel buttons h
ng due to sp
wn_loc
l context loss- fix: ACP test/s
ve button h
ng 
nd preset mode def
ults- fix: ACP p
nel gemini preset ID mism
tch 
nd test button h
ng- fix: resolve 
ll 75 compil
tion errors from provider routing ref
ctor- fix: v
ult-b
cked provider API keys 
nd config h
ndler improvements- fix(
cp): 
d
pt h
rnesses to re
l CLI protocols 
fter e2e probe testing- fix: worksp
ce schem
 migr
tion, worksp
ce.getActive response, 
nd providers p
ge freeze- fix: remove redund
nt binding in ConfigP
tcher- fix: session history, 
gent.list RPC, 
nd embedding dedup- fix: count only running runs for concurrency limit, reduce cle
nup del
y- fix: 
dd multi-dimension vector columns to memories t
ble schem
- fix: hot-sw
p runtime provider when switching def
ult vi
 P
nel UI- fix: resolve ch
t qu
lity issues — bootstr
p, esc
l
tion, 
nd response form
t- fix: resolve pre-existing test compil
tion errors- fix: wire missing RPC h
ndlers 
nd correct TUI method n
mes- fix: upd
te rem
ining port 18789 references to 18790- fix: unify ch
nnel config persistence — P
nel UI s
ve/lo
d/connect now works- fix: resolve compil
tion errors from fe
ture fl
g remov
l- fix(desktop): 
ddress fin
l review — version 
lignment, input v
lid
tion, Unicode- fix(desktop): 
ddress clippy needless-borrow w
rning in 
gent h
ndler- fix(desktop): 
ddress code qu
lity review — v
lid
tion, 
pprov
l g
tes- fix(desktop): wire N
tiveDesktop into registry + complete re-exports- fix: logic review R2 
rchitecture — 14 findings 
cross 5 c
tegories- fix: logic review R2 — 29 files 
cross 4 priority b
tches- fix: 
ddress code review findings for self-configur
tion- fix: RAII sem
phore gu
rd 
nd env v
r exp
nsion ordering (Known Issues)- fix: repl
ce std::sync::RwLock with cr
te::sync_primitives (P2-15)- fix: sort H
shM
p-derived collections for deterministic ordering (P2-14)- fix: repl
ce SystemTime UNIX_EPOCH .unwr
p() with .unwr
p_or_def
ult() (P2-12)- fix: rele
se locks before 
w
iting in 4 
sync p
tterns (P2-11)- fix: norm
lize t
sk_type 
nd t
sk_id in SessionKey::t
sk() (P1-9)- fix: use bounded c
st for POE token count u32 conversion (P1-8)- fix: resolve rem
ining UTF-8 byte slicing p
nics (P1-7)- fix: ConfigP
tcher use s
ve_increment
l 
nd h
rd-error on conflict- fix: logic review Ph
se 6 — 45 fixes 
cross g
tew
y, memory, poe, exec, providers, 
nd 15 more modules- fix: resolve 5 rem
ining W
rning-level issues from logic review Ph
se 5- fix: logic review Ph
se 4 — 18 fixes 
cross d
emon, engine, secrets, skills, components, cron- fix: resolve 5 Known Issues from logic review- fix: comprehensive logic review fixes 
cross 53 files in 77 modules- fix: use cfg(fe
ture = "loom") inste
d of cfg(loom) to 
void poisoning dependencies- fix(g
tew
y): elimin
te TOCTOU in execution_engine concurrent run limit check- fix(g
tew
y): use Mutex for ch
nnel_registry t
ke-once inbound_rx p
ttern- fix(resilience): simplify governor session_tokens from AtomicU64 to u64- fix: upd
te doctest to use poe::met
_cognition::Beh
vior
lAnchor- fix: 
dd Clone derive to NoiseFilter 
nd remove duplic
te mod decl
r
tions- fix: remove duplic
te scoring_pipeline module decl
r
tion in memory/mod.rs- fix(clippy): resolve print_liter
l w
rnings in secret providers comm
nd- fix(tests): migr
te secret_bound
ry_integr
tion tests to 
sync- fix(runtimes): 
ddress critic
l 
nd import
nt code review findings- fix: resolve 
ll clippy w
rnings in 
leph-t
uri 
nd 
lephcore- fix(desktop): use ERR_NOT_IMPLEMENTED for stubbed methods, 
dd debug logging- fix(h
lo): 
ddress code review findings for view 
nd events- fix(h
lo): gu
rd 
g
inst empty run_id in event h
ndler- fix(h
lo): use monotonic counter for unique mess
ge IDs, remove redund
nt ph
se gu
rd- fix(desktop): restrict UDS socket to owner-only 
ccess- fix(desktop): 
dd 30s timeout to UDS request to prevent indefinite t
sk h
ng- fix(desktop): log ev
lu
teJ
v
Script errors in C
nv
s, 
dd runAsync m
in-thre
d 
ssert- fix(desktop): repl
ce deprec
ted 
ctiv
te(options:) with 
ctiv
te() for m
cOS 15- fix(desktop): 
void PNG round-trip in OCR p
th by sh
ring c
ptureCurrentScreen- fix: 
ddress code review findings- fix(desktop): repl
ce strcpy with strncpy to prevent buffer overflow- fix(desktop): require x/y for click 
nd window_id for focus_window- fix(desktop): remove misle
ding serde t
gs from DesktopRequest, 
dd From conversions- fix(skill): 
ddress code review findings- fix(skill): resolve clippy w
rnings in skill module- fix(skill): use single colon sep
r
tor for SkillId (m
tches OpenCl
w convention)- fix(st
rt): 
dd cfg gu
rd for builder mod, tighten h
ndler visibility to pub(in cr
te::comm
nds::st
rt)- fix(st
rt): move session b
nner print into register_session_h
ndlers for consistency- fix: resolve 
ll compil
tion errors from server purific
tion- fix: cle
n up rem
ining Server-Client terminology in source comments- fix: rep
ir 2 broken doc-tests in skill_evolution module- fix: resolve 8 pre-existing test f
ilures- fix(control-pl
ne): document AlertsApi integr
tion limit
tion- fix(control-pl
ne): complete mock d
t
 remov
l- fix(control-pl
ne): fix memory le
ks 
nd improve error h
ndling in 
lert subscriptions- fix(sh
red-ui-logic): improve error h
ndling in 
lerts API- fix(control-pl
ne): use T
ilwind CDN for CSS compil
tion- fix(control-pl
ne): 
dd WASM initi
liz
tion in lib.rs- fix(control-pl
ne): upd
te st
rtup log mess
ge to show correct URL- fix(control-pl
ne): fix root p
th 
ccess 
nd st
tic 
sset lo
ding- fix: resolve compil
tion errors 
nd 
dd missing imports- fix(d
shbo
rd): 
dd w
sm_bindgen entry point to en
ble 
pp initi
liz
tion- fix(g
tew
y): extr
ct guest_session_id when require_
uth=f
lse- fix: resolve compil
tion errors in 
uth 
nd guest h
ndlers- fix: use rowid inste
d of id for sqlite-vec virtu
l t
ble upd
tes- fix(ph
se2): fix RPC tests 
nd upd
te progress report- fix(cli): use correct method n
mes for session comm
nds- fix(cli): resolve event stre
ming issue between g
tew
y 
nd CLI- fix(cli): 
lign comm
nd h
ndlers with g
tew
y API- fix(memory): h
ndle new SubAgent F
ctType v
ri
nts in consolid
tion- fix: resolve f
iling BDD tests for embodiment 
nd CoT tr
nsp
rency- fix: resolve f
iling unit tests- fix: resolve module export 
nd test compil
tion errors- fix: resolve 
ll 29 compiler w
rnings- fix: 
dd dylib.* p
ttern to gitignore- fix: upd
te .gitignore for Aleph ren
me 
nd remove dylib from tr
cking- fix(compressor): fix string conc
ten
tion in tests- fix(protocols): error on nonexistent JSONP
th inste
d of returning null- fix(scr
tchp
d): use EAFP p
ttern inste
d of sync exists() checks- fix(scr
tchp
d): remove 
sync from exists() 
nd export Scr
tchp
dConfig- fix(core): fix form
t strings in m
nifest.rs 
nd doctest in pty.rs- fix: cle
n up rem
ining MultiTurnCoordin
tor references- fix(g
tew
y): remove MultiTurnCoordin
tor dependency from 
d
pter- fix(h
lo): upd
te DependencyCont
iner comment for H
loInputCoordin
tor- fix(h
lo): upd
te AppDeleg
te to use H
loInputCoordin
tor- fix(h
lo): upd
te HotkeyService to use H
loInputCoordin
tor- fix: upd
te tests for 5 builtin tools 
nd skill evolution- fix: compil
tion errors in skill evolution 
nd perception modules- fix: resolve test compil
tion errors### Ch
nged- ref
ctor: ren
me ch
tgpt → codex protocol 
cross codeb
se- ref
ctor: ren
me ToolGroup → ToolC
tegory to 
void confusion with Te
m- ph
se4: cle
n 
ll T
uri references from codeb
se- ph
se4: remove T
uri, 
rchive old 
pps, move Swift bridge to cr
tes/desktop-m
cos/bridge- ref
ctor: move CLI/TUI/WebCh
t to interf
ces/, client to sh
red/- cle
nup: remove bootstr
p 
uto-clone 
nd leg
cy plugin index code- cle
nup: remove AgentLifecycleEvent::Switched 
nd AgentRouter from inbound router- cle
nup: remove 
gent switching (tool, intent detector, /switch comm
nd)- cle
nup: remove unregistered self-m
n
gement tool source files- cle
nup: remove old sub
gent tools (sp
wn/steer/kill + deleg
te)- cle
nup: move e2e tests into tests/, remove unused sh
red_ui_logic cr
te, 
dd secret sc
nning exclusion- cle
nup: remove tempor
ry debug logging for ch
tgpt protocol- ref
ctor: ren
me worksp
ce to 
gent 
cross memory/config/p
ths, enh
nce 
gent loop 
nd Ch
tGPT protocol- cle
nup: remove zombie code, upd
te def
ult config 
nd sh
red_ui_logic- cle
nup: remove st
le ALEPH_MASTER_KEY references from docs 
nd error mess
ges- ref
ctor: fl
tten 
gent_loop/ — remove minim
l/ subdirectory- cle
nup: remove deprec
ted APIs (register_
gent_tools, with_working_dir, ToolC
tegory::N
tive, PolicyEngine stubs, AuditStore, Inv
lid
teOld)- ref
ctor: ren
me Minim
l* types to st
nd
rd n
mes — this IS the loop- cle
nup: fix clippy w
rning in leg
cy_
d
pter detect_entry_point- cle
nup: elimin
te 
ll clippy w
rnings (58→0)- cle
nup: fix clippy w
rnings (derive Def
ult, redund
nt closures, simplified condition
ls)- cle
nup: remove st
le 
pp_bundle_id references from comments 
nd BDD tests- cle
nup: remove TypeScript webch
t (repl
ced by P
nel /ch
t route)- cle
nup: remove de
d Sub
gentAuthority 
nd tools/sessions dom
in l
yer- ref
ctor: simplify memory types, use floor_ch
r_bound
ry, 
dd mtime c
che to d
ily memory- ref
ctor(pdf): split pdf_gener
te.rs into module directory- ref
ctor: strip #[cfg(fe
ture)] from g
tew
y, server, extension, 
nd misc modules- ref
ctor: strip #[cfg(fe
ture)] from 
ll 12 ch
nnel implement
tions- ref
ctor: strip 20+ C
rgo fe
ture fl
gs from core cr
te- ref
ctor: Occ
m's R
zor p
ss — elimin
te clippy w
rnings 
nd de
d code- cle
nup: remove f
stembed 
nd loc
l embedding model remn
nts- cle
nup: fix unused import in host_functions.rs- ref
ctor(w
sm): simplify PermissionChecker to f
c
de over W
smC
p
bilities- cle
nup: bro
d DRY ref
ctoring 
nd clippy compli
nce 
cross codeb
se- cle
nup: remove st
le f
stembed references, fix integr
tion tests- cle
nup: remove m
cOS-specific CI workflow 
nd build scripts (C8-C12)- cle
nup: remove deprec
ted m
cOS Swift 
pp (C7)- cle
nup: remove UniFFI Swift bindings (C1-C2)- ref
ctor(core): introduce register_h
ndler! m
cro, elimin
te h
ndler boilerpl
te (W
ve 4)- ref
ctor(core): repl
ce &Vec<T> with &[T] in 
rrow_convert 
nd sh
dow_repl
y (W
ve 3B)- ref
ctor(core): convert Intern
lEventH
ndler String p
r
ms to &str (W
ve 3A)- ref
ctor(core): m
nu
l Clippy fixes — expect_fun_c
ll, useless_vec, ptr_
rg, type_complexity, module_inception, needless_borrows, 
nd more (W
ve 2B)- ref
ctor(core): repl
ce Def
ult::def
ult() field re
ssignment with struct liter
ls (W
ve 2A)- ref
ctor(core): 
uto-fix Clippy w
rnings 
nd remove unused imports (W
ve 1)- ref
ctor(runtimes): delete old runtime m
n
gers, repl
ce with Ledger/Probe system- ref
ctor(video): repl
ce RuntimeRegistry with C
p
bilityLedger in c
ption.rs- ref
ctor(init): repl
ce forced runtime inst
ll
tion with zero-inst
ll ledger- ref
ctor(desktop): delete RPC proxy comm
nds 
nd cle
n up de
d code (~1600 lines)- ref
ctor(h
lo): delete Re
ct frontend source from T
uri 
pp- ref
ctor(h
lo): point T
uri h
lo window to Leptos server URL- ref
ctor(h
lo): delete leg
cy Swift H
lo views 
nd fix references (~4500 lines removed)- ref
ctor(st
rt): split initi
lize_
uth, extr
ct lo
d_
pp_config, restore register c
lls to orchestr
tor- ref
ctor(st
rt): move register_* h
ndler functions to comm
nds/builder/h
ndlers.rs- ref
ctor(extension): thin mod.rs f
c
de, deleg
te lo
d_
ll to ComponentLo
der- ref
ctor(st
rt): extr
ct subsystem initi
lizers from st
rt_server- ref
ctor: remove distributed execution infr
structure (ExecutionPolicy, ClientM
nifest, ReverseRpc, ToolRouter, RoutedExecutor)- ref
ctor: cle
n up 
uth h
ndler by removing ClientM
nifest references- ref
ctor: simplify g
tew
y server by removing client routing infr
structure- ref
ctor: simplify ExecutionEngine by removing client routing- ref
ctor: ren
me g
tew
y/ch
nnels/ to g
tew
y/interf
ces/- ref
ctor: ren
me clients/ to 
pps/- cle
nup: remove unused imports from exec_security_g
te (post-reb
se)- cle
nup: fix Arc misuse, l
rge v
ri
nts, 
nd priv
te interf
ces (P
ss 3 fin
l)- cle
nup: extr
ct type 
li
ses 
nd p
r
meter structs (P
ss 3)- cle
nup: suppress module_inception for intention
l nested module p
ttern- cle
nup: fix 22 miscell
neous clippy w
rnings- cle
nup: P
ss 2 loc
l ref
ctoring (clone, strip_prefix, de
d code, redund
nt closures)- cle
nup: fix boole
n simplific
tions, identity ops, 
nd &P
thBuf sign
tures- cle
nup: remove unused imports 
nd repl
ce deriv
ble impls- cle
nup: 
pply c
rgo clippy --fix 
uto-corrections- ref
ctor(control-pl
ne): split Sideb
r into sideb
r/ directory- ref
ctor(control-pl
ne): use nested routes for Settings with SettingsL
yout- ref
ctor(control-pl
ne): remove /cp prefix from routing- ref
ctor(core): ren
me 
leph-g
tew
y to 
leph-server- ref
ctor(m
cos): completely remove settings UI from m
cOS client- ref
ctor(desktop): completely remove settings UI from T
uri client- ref
ctor(desktop): migr
te Plugins, Skills, 
nd Policies settings to D
shbo
rd- ref
ctor(clients): complete Ph
se 4 - remove Gener
tion Providers UI- ref
ctor(clients): migr
te Providers, Memory, 
nd MCP config to D
shbo
rd- ref
ctor(
gent_loop): introduce RunContext p
ttern for cle
ner API- ref
ctor(
gent-loop): 
dd RunContext structure (WIP)- ref
ctor(dom
in): implement Newtype p
ttern for Answer 
nd Ruleset- ref
ctor(dom
in): implement Newtype p
ttern for 5 ID types- ref
ctor(
pi): implement FromStr tr
it for rem
ining types- ref
ctor(
pi): implement FromStr tr
it for extension 
nd resilience types- ref
ctor(
pi): implement FromStr tr
it for memory context types- ref
ctor(perf): repl
ce trim_st
rt_m
tches with strip_prefix for fixed prefixes- ref
ctor(perf): optimize &P
thBuf → &P
th in 6 files- ref
ctor(core): 
dd #[
llow(de
d_code)] to 12 reserved fields- ref
ctor(deps): remove 5 unused dependencies- ref
ctor(core): remove 2 confirmed de
d code items- ref
ctor(core): remove 160+ unused imports 
cross 50 files- ref
ctor(tools): extr
ct builtin tool registr
tion 
nd types (Ph
se 6)- ref
ctor(g
tew
y): modul
rize plugins h
ndlers (Ph
se 5.1)- ref
ctor(poe): extr
ct services to dedic
ted modules (Ph
se 4.2 - P1)- ref
ctor(poe): extr
ct h
ndler types to dedic
ted modules (Ph
se 4.1 - P0)- ref
ctor(browser): extr
ct types 
nd scripts modules (Ph
se 3 - P
rt 1)- ref
ctor(engine): complete 
tomic executor composition ref
ctoring (Ph
se 2)- ref
ctor(engine): 
dd 
tomic module b
se 
rchitecture (Ph
se 2 WIP)- ref
ctor(extension): split types.rs into modul
r structure- ref
ctor(security): tr
nsform PolicyEngine to st
teless- ref
ctor(protocol): 
dd equ
lity derives 
nd helper methods to 
uth types- ref
ctor(ph
se1): reorg
nize client directory structure- ref
ctor: complete fin
l Aether to Aleph cle
nup- ref
ctor: complete Aether to Aleph ren
me - scripts, workflows, 
nd rem
ining code- ref
ctor: complete Aether to Aleph ren
me 
cross entire codeb
se- ref
ctor(providers): use ProtocolRegistry in cre
te_provider f
ctory- ref
ctor(providers): remove technic
l 
li
s presets- ref
ctor(config): remove provider_type field from ProviderConfig- ref
ctor: fix P3 clippy w
rnings - b
tch 2- ref
ctor: fix P3 clippy w
rnings - b
tch 1- ref
ctor: fix P1/P2 clippy w
rnings 
nd improve code qu
lity- ref
ctor(providers): delete leg
cy OpenAiProvider- ref
ctor(providers): delete leg
cy GeminiProvider- ref
ctor(providers): delete leg
cy Cl
udeProvider- ref
ctor(providers): use HttpProvider for Anthropic protocol- ref
ctor(providers): remove redund
nt vendor wr
ppers (~850 lines)- ref
ctor(providers): use HttpProvider for OpenAI protocol in f
ctory- ref
ctor(m
cos): cle
nup 
nd improve hotkey/h
lo components- ref
ctor(h
lo): repl
ce H
loSt
te with simplified 6-st
te version- ref
ctor(h
lo): switch H
loWindow to V2 components- ref
ctor(h
lo): remove MultiTurn references from EventH
ndler- ref
ctor(h
lo): remove MultiTurn directory (~3000 lines)- ref
ctor: split l
rge modules into sm
ller files- cle
nup: remove unused modules 
nd merge thinking into thinker- cle
nup: elimin
te 
ll compil
tion w
rnings- cle
nup(lib): slim down exports from 590 to 272 lines- cle
nup: remove FFI-rel
ted comments- cle
nup: ren
me FFI types to st
nd
rd n
mes- cle
nup(disp
tcher): ren
me ffi.rs to tool_info.rs- cle
nup(intent): remove Type A FFI residu
ls### Build- docs: 
dd skill scope filtering implement
tion pl
n- docs: fix skill scope filtering spec per review- docs: 
dd skill scope filtering design spec- rele
se: v0.2.9- docs: 
dd voice convers
tion implement
tion pl
n- docs: fix PromptBuilder voice st
te 
ccess p
th in voice spec- docs: upd
te voice convers
tion spec with review fixes- docs: 
dd voice convers
tion system design spec- docs: 
dd rele
se workflow 
nd version m
n
gement to CLAUDE.md- rele
se: v0.2.8- build: unify version source — VERSION file drives 
ll version strings- rele
se: v0.2.8- docs: 
dd multimod
l probe tests implement
tion pl
n- docs: 
dd multimod
l probe tests design spec- docs: 
dd core multimod
l enh
ncement implement
tion pl
n- docs: fix spec review issues in core multimod
l design- docs: 
dd core multimod
l enh
ncement design spec- docs: 
dd Telegr
m ch
nnel enh
ncement implement
tion pl
n- docs: fix spec review issues in Telegr
m enh
ncement design- docs: 
dd Telegr
m ch
nnel enh
ncement design spec- docs: 
dd Feishu enh
nced fe
tures implement
tion pl
n- docs: 
ddress spec review — FeishuEventEmitter, typing lifecycle, c
p
bilities- docs: 
dd Feishu enh
nced fe
tures design spec- docs: 
dd Feishu ch
nnel implement
tion pl
n- docs: 
ddress spec review feedb
ck for Feishu ch
nnel- docs: 
dd Feishu/L
rk ch
nnel design spec- rele
se: v0.2.7 — multi-
gent system, UI upd
tes, bug fixes- docs: fix spec issues from review — st
le fin
l_text, test pl
n, consecutive_errors- docs: 
dd Persistent Completion Protocol design spec- docs: fix multi-
gent modes spec per review findings- docs: 
dd multi-
gent modes t
xonomy design spec- docs: 
dd t
sk coordin
tion implement
tion pl
n (12 t
sks)- docs: fix event type conventions in t
sk coordin
tion spec- docs: 
ddress spec review findings for t
sk coordin
tion- docs: 
dd t
sk coordin
tion system design spec- build: upd
te WASM p
nel dist- ci: upgr
de GitHub Actions to Node.js 24 comp
tible versions- ci: scope fmt check to m
int
ined cr
tes (skip leg
cy form
tting issues)- build: consolid
te to single rele
se workflow, fix CI protoc dependency- build: remove 
rchive from git (l
rge bin
ries exceed GitHub limit)- rele
se: bump version to 0.2.6- build: upd
te inst
ll scripts for 
leph-server bin
ry n
me- build: ren
me workflows, fix --bin 
leph→
leph-server, 
dd pl
tform rele
se workflows- build: upd
te justfile 
nd CI workflows for post-T
uri 
rchitecture- build: 
dd swift-bridge recipe to justfile for m
cOS n
tive APIs- docs: 
dd Ph
se 3 implement
tion pl
n for m
cOS PIM & system c
p
bilities- docs: 
dd Ph
se 2 implement
tion pl
n for screen control n
tive migr
tion- docs: 
ddress spec review feedb
ck for hier
rchic
l comm
nds- docs: 
dd hier
rchic
l sl
sh comm
nds design spec- docs: 
dd Ph
se 1 implement
tion pl
n for desktop n
tive c
p
bilities- docs: 
dd desktop n
tive c
p
bilities design spec- docs: upd
te design spec with new directory structure- docs: 
dd implement
tion pl
n for intermedi
te mess
ge delivery- docs: 
dd PLUGIN_SYSTEM.md — CC-comp
tible plugin 
rchitecture reference- docs: 
ddress spec review feedb
ck for CLI/TUI sep
r
tion- docs: 
dd CLI/TUI sep
r
tion design spec- docs: 
dd P4 runtime migr
tion implement
tion pl
n- docs: 
dd prompt guid
nce 
s in-scope ch
nges to intermedi
te mess
ge spec- docs: 
dd edge c
ses to intermedi
te mess
ge delivery spec- docs: 
dd intermedi
te mess
ge delivery design spec- docs: 
dd P3 scope m
n
gement implement
tion pl
n- docs: 
dd P2 m
rketpl
ce system implement
tion pl
n- docs: 
dd P0+P1 implement
tion pl
n for plugin CC comp
t- docs: fix rem
ining spec review items (round 2)- docs: 
ddress spec review findings for plugin comp
t design- docs: 
dd plugin system Cl
ude Code comp
tibility redesign spec- docs: upd
te spec 
nd pl
n — keep peer_id sign
tures unch
nged- docs: upd
te 
gent-bot 1:1 binding spec with review fixes- docs: 
dd 
gent-bot 1:1 binding simplific
tion design spec- docs: 
dd ch
t sideb
r redesign spec 
nd implement
tion pl
n- docs: 
dd p
nel 
gent routing fix design spec- docs: 
dd worksp
ce output migr
tion implement
tion pl
n- docs: revise worksp
ce output migr
tion spec 
fter review- docs: 
dd worksp
ce output migr
tion design spec- docs: 
dd gener
tion providers wiring implement
tion pl
n- docs: fix gener
tion providers spec 
fter review- docs: 
dd gener
tion providers wiring design spec- docs: 
dd Cl
wHub integr
tion implement
tion pl
n- docs: 
ddress spec review feedb
ck for Cl
wHub integr
tion- docs: 
dd Cl
wHub integr
tion design spec- ci: upgr
de GitHub Actions to Node.js 24, fix Windows de
d-code w
rnings- docs: fix pl
n review issues (3 blockers + 6 w
rnings)- docs: 
ddress spec review feedb
ck for Chrome DevTools MCP Mode- docs: 
dd Chrome DevTools MCP Mode design spec- docs: 
dd process m
n
gement rules to CLAUDE.md- docs: 
dd tool permission system implement
tion pl
n- docs: upd
te tool permission spec 
fter review- docs: 
dd tool permission system design spec- docs: 
dd ACP probe tests design document- docs: 
dd ACP h
rness m
n
gement implement
tion pl
n- docs: 
dd ACP h
rness m
n
gement design document- docs: 
dd provider routing ref
ctor implement
tion pl
n- docs: fix rem
ining spec review issues- docs: fix spec issues from review- docs: 
dd provider routing ref
ctor design spec- docs: 
dd provider config testing implement
tion pl
n- docs: upd
te provider config testing spec 
fter review- docs: 
dd provider config testing design spec- docs: 
dd simplify-model-config implement
tion pl
n- docs: upd
te simplify-model-config spec 
fter review- docs: 
dd simplify-model-config design spec- ci: re
d rele
se version from VERSION file inste
d of m
nu
l input- docs: 
dd cron probe tests implement
tion pl
n- docs: 
dd cron probe tests design spec- docs: 
dd cron module redesign implement
tion pl
n- docs: 
dd cron module redesign spec- build: rebuild p
nel WASM 
nd upd
te docs 
fter worktree merges- docs: 
dd provider zero-config implement
tion pl
n- docs: 
dd mess
ge pipeline implement
tion pl
n- docs: 
dd provider zero-config UX design spec- docs: 
dd mess
ge pipeline design for g
tew
y pre-processing- docs: 
dd model discovery probe tests implement
tion pl
n- docs: 
dd model discovery probe tests design spec- docs: 
dd model discovery implement
tion pl
n- docs: fix model discovery spec issues from review- docs: 
dd model discovery design spec- docs: 
dd cognitive evolution bet
 implement
tion pl
n- docs: 
dd cognitive evolution bet
 design (immune-complete loop)- docs: 
dd POE Ph
se 2+3 implement
tion pl
n- docs: 
dd POE Ph
se 1 implement
tion pl
n (Bl
stR
dius + T
boo)- docs: 
dd POE Architecture Evolution Whitep
per 2026- ci: fix Linux/Windows compil
tion errors for missing imports- docs: upd
te extension system 
rchitecture document
tion- docs: 
dd unified plugin system implement
tion pl
n- docs: 
dd unified plugin system design- docs: 
dd one-line inst
ll comm
nds 
s prim
ry inst
ll
tion method- docs: remove ref
ctoring b
ckstory from intent section- docs: upd
te intent detection section to reflect unified LLM pipeline- docs: 
dd det
iled Aleph vs OpenCl
w comp
rison- docs: 
dd P4.3 core plugins implement
tion pl
n- docs: 
dd plugin development guide- docs: 
dd P4 plugin ecosystem implement
tion pl
n- ci: 
dd Windows x86_64 build t
rget 
nd PowerShell inst
ller- docs: 
dd P3 medi
 pipeline implement
tion pl
n- ci: fix Linux w
rn import, remove d
rwin-x86_64 t
rget- ci: 
dd libxdo-dev for Linux, fix d
rwin x86_64 AVX-512 link error- ci: fix Linux pipewire comp
t (ubuntu-24.04) 
nd m
cOS x86_64 openssl- ci: 
dd libegl 
nd X11 extension deps for Linux build- ci: use m
cos-l
test for x86_64 cross-compile (m
cos-13 EOL)- ci: 
dd dbus, drm, gbm deps for Linux build- ci: 
dd pipewire 
nd cl
ng deps for Linux xc
p build- ci: 
dd libw
yl
nd-dev to Linux build dependencies- docs: 
dd 
uthor note to README- docs: ren
me p
nel screenshots with consistent numbering- docs: restore d
shbo
rd screenshot, keep 
ll 3 p
nel im
ges- docs: upd
te README screenshots with P
nel ch
t 
nd settings views- build: remove webch
t recipes from justfile- docs: 
dd webch
t Rust rewrite implement
tion pl
n- docs: 
dd webch
t Rust rewrite design- docs: remove 
cknowledgments section from README- ci: en
ble 
ll pl
tform build t
rgets for server rele
se- ci: 
dd m
nu
l server rele
se workflow 
nd improve inst
ll script- docs: overh
ul README.md, CLAUDE.md 
nd 
dd LICENSE- docs: 
dd inline directives 
nd leg
cy cle
nup implement
tion pl
n- docs: 
dd inline directives 
nd leg
cy cle
nup design- docs: 
dd l
ngu
ge-
gnostic intent detection implement
tion pl
n- docs: 
dd l
ngu
ge-
gnostic intent detection design- docs: upd
te cle
nup pl
n with execution results- docs: cl
rify cle
nup str
tegy — scoped responsibility, not f
llb
ck- docs: 
dd multi-
gent code redund
ncy cle
nup pl
n- docs: 
dd A2A protocol implement
tion pl
n- docs: 
dd A2A protocol design document- docs: 
dd per-
gent tool configur
tion implement
tion pl
n- docs: 
dd per-
gent tool configur
tion design- docs: 
dd multi-bot P
nel UI implement
tion pl
n- docs: 
dd multi-bot P
nel UI design- docs: 
dd multi-bot ch
nnel implement
tion pl
n- docs: 
dd multi-bot ch
nnel support design- docs: 
dd memory 
lignment design for du
l-directory 
rchitecture- docs: 
dd 
gent-worksp
ce sep
r
tion implement
tion pl
n- docs: 
dd 
gent-worksp
ce sep
r
tion design- docs: 
dd 
gent m
n
gement p
nel implement
tion pl
n- docs: 
dd 
gent m
n
gement p
nel design- docs: 
dd webch
t restructure implement
tion pl
n- docs: 
dd webch
t restructure design- docs: 
dd 
gent switching enh
ncement implement
tion pl
n- docs: 
dd 
gent switching enh
ncement design- docs: 
dd unified comm
nd registry implement
tion pl
n- docs: 
dd unified comm
nd registry design- docs: 
dd dyn
mic 
gent switching implement
tion pl
n- docs: 
dd dyn
mic 
gent switching design- docs: 
dd system prompt optimiz
tion implement
tion pl
n- docs: 
dd system prompt 
rchitecture optimiz
tion design- docs: 
dd Agent/Worksp
ce/Session unific
tion implement
tion pl
n- docs: 
dd Agent/Worksp
ce/Session rel
tionship design- docs: 
dd t
sk routing decision l
yer implement
tion pl
n- docs: 
dd t
sk routing decision l
yer design- docs: 
dd 
rchitecture 
ctiv
tion di
gnostic report- docs: 
dd 
rchitecture 
ctiv
tion di
gnostic implement
tion pl
n- docs: 
dd 
rchitecture 
ctiv
tion di
gnostic design- docs: 
dd n
tive tool_use implement
tion pl
n (9 t
sks)- docs: 
dd n
tive tool_use migr
tion design- docs: 
dd PDF du
l-engine implement
tion pl
n- docs: 
dd PDF du
l-engine rendering design- docs: 
dd cron 
nd group ch
t b
ckend implement
tion pl
n- docs: 
dd cron 
nd group ch
t b
ckend implement
tion design- docs: 
dd scheduled t
sks p
nel implement
tion pl
n- docs: 
dd scheduled t
sks p
nel design- docs: 
dd CLI full RPC cover
ge implement
tion pl
n- docs: 
dd CLI full RPC cover
ge design- docs: 
dd CLI bugfix 
nd JSON unific
tion design- docs: 
dd CLI full comm
nds implement
tion pl
n- docs: 
dd CLI full comm
nds design- docs: 
dd CLI infr
structure enh
ncement implement
tion pl
n- docs: 
dd CLI infr
structure enh
ncement design- docs: 
dd lifecycle observ
bility logging implement
tion pl
n- docs: 
dd lifecycle observ
bility logging design- docs: 
dd system prompt enh
ncement implement
tion pl
n- docs: 
dd system prompt enh
ncement design- docs: 
dd 
gent system Ph
se 2 full cover
ge implement
tion pl
n- docs: 
dd 
gent system full cover
ge design (Ph
se 2)- docs: 
dd Codex p
nel UI design 
nd implement
tion pl
n- docs: 
dd Codex Responses API implement
tion pl
n- docs: 
dd Codex Responses API protocol 
d
pter design- docs: 
dd g
tew
y enh
ncement implement
tion pl
n (20 t
sks)- docs: 
dd g
tew
y enh
ncement design (OpenCl
w-inspired)- docs: 
dd implement
tion pl
n for 
gent/worksp
ce/binding- docs: 
dd 
gent definition + worksp
ce + binding design- docs: 
dd OpenAI subscription provider implement
tion pl
n- docs: 
dd OpenAI subscription provider design- docs: 
dd L
zy POE Activ
tion design- build: ren
me just server → just build, 
dd just 
ll- docs: upd
te bin
ry n
me 
nd port references 
cross 
ll document
tion- build: en
ble 
xum ws fe
ture for port unific
tion- docs: 
dd port unific
tion implement
tion pl
n- docs: 
dd port unific
tion 
nd bin
ry ren
me design- docs: 
dd ch
nnel infr
structure fix implement
tion pl
n- docs: 
dd ch
nnel infr
structure fix design- docs: upd
te CLAUDE.md for fe
ture fl
g remov
l- build: simplify justfile — remove 
ll --fe
tures fl
gs- docs: 
dd runtime ch
nnel control implement
tion pl
n- docs: 
dd runtime ch
nnel control design — elimin
te fe
ture fl
g fr
gment
tion- docs: 
dd ch
t persistence & memory pipeline implement
tion pl
n- docs: 
dd ch
t persistence & memory pipeline fix design- docs: 
dd full ch
in + sm
rt rec
ll implement
tion pl
n- docs: 
dd full ch
in + sm
rt rec
ll design- docs: 
dd worksp
ce enh
ncements implement
tion pl
n (9 t
sks)- docs: 
dd worksp
ce enh
ncements design (4 fe
tures)- docs: 
dd worksp
ce wiring implement
tion pl
n (11 t
sks)- docs: 
dd worksp
ce wiring design for multi-role person
 system- docs: 
dd config extern
liz
tion implement
tion pl
n- docs: 
dd config extern
liz
tion design for ~/.
leph worksp
ce- ci: keep only m
cOS ARM64 build, document other pl
tform blockers- ci: fix rem
ining build issues 
cross pl
tforms- ci: fix cross-pl
tform build issues- ci: pin w
sm-bindgen-cli to 0.2.108 m
tching C
rgo.lock- ci: 
llow test job to f
il without blocking builds- ci: 
dd X11/xscrns
ver dev libr
ries for Linux builds- ci: inst
ll protoc for l
nce-encoding build dependency- ci: improve rele
se workflow with WASM build, test job, 
nd cross-pl
tform desktop- build: rewrite justfile for desktop-
s-muscle 
rchitecture- docs: 
dd cr
tes/desktop to project structure 
nd build comm
nds- docs: 
dd Desktop-
s-Muscle implement
tion pl
n- docs: 
dd Desktop-
s-Muscle 
rchitecture design- docs: 
dd self-configur
tion implement
tion pl
n- docs: 
dd self-configur
tion design document- ci: 
dd loom concurrency test job 
nd incre
se proptest cover
ge- build: 
dd test-proptest, test-loom, test-logic just recipes- docs: 
dd logic review system implement
tion pl
n (15 t
sks, 49 properties)- docs: 
dd logic review system design (three-l
yer defense 
rchitecture)- docs: move obsolete embedding/sqlite-vec pl
ns to leg
cy- docs: upd
te memory system docs to reflect remote embedding migr
tion- build: repl
ce trunk with m
nu
l WASM pipeline in justfile- docs: fix m
cOS Resources p
th in build pipeline design- build: 
dd justfile for unified build pipeline- docs: 
dd unified build pipeline design- docs: 
dd ch
nnel config p
nel implement
tion pl
n- docs: 
dd ch
nnel config p
nel design document- docs: 
dd POE full evolution implement
tion pl
n (19 t
sks, 4 ph
ses)- docs: 
dd POE full evolution design (event-driven closed loop)- docs: 
dd WASM c
p
bility kernel implement
tion pl
n- docs: 
dd WASM c
p
bility kernel design- docs: 
dd m
cOS PIM n
tive API implement
tion pl
n- docs: 
dd m
cOS PIM n
tive API integr
tion design- docs: 
dd POE cognitive hub implement
tion pl
n- docs: 
dd POE cognitive hub upgr
de design- docs: 
dd soci
l bot ch
nnels exp
nsion implement
tion pl
n- docs: 
dd soci
l bot ch
nnels exp
nsion design- docs: 
dd surgic
l DRY ref
ctoring implement
tion pl
n- docs: 
dd surgic
l DRY ref
ctoring design for embedding provider files- docs: 
dd embedding provider LLM migr
tion implement
tion pl
n- docs: 
dd embedding provider LLM migr
tion design- docs: 
dd l
rge file ref
ctoring implement
tion pl
n — 6 t
sks, 5 files- docs: 
dd l
rge file ref
ctoring design — 5 files, pure module splitting- ci: 
dd server, m
cOS 
pp, 
nd T
uri rele
se workflows- docs: 
dd distribution implement
tion pl
n (24 t
sks, 9 ph
ses)- docs: 
dd distribution 
rchitecture design- docs: 
dd PromptPipeline implement
tion pl
n — 10 t
sks, TDD, str
ngler fig- docs: 
dd PromptPipeline design — Tr
it-per-L
yer evolution from Pl
n A- docs: 
dd 
utom
tion skills implement
tion pl
n- docs: 
dd 
utom
tion skills (#21-30) design- docs: 
dd memory event sourcing implement
tion pl
n- docs: 
dd memory event sourcing design (CQRS Light)- docs: 
dd prompt system enh
ncement implement
tion pl
n- docs: 
dd prompt system enh
ncement design- docs: 
dd skills system, upd
te runtimes refs, 
dd m
cOS components- docs: upd
te 
ccept
nce results 
fter bridge fixes (27/30 p
ss)- docs: 
dd implement
tion pl
n for fixing bridge known issues- docs: 
dd design for fixing bridge known issues- docs: remove rem
ining Swift references from CLAUDE.md- docs: upd
te CLAUDE.md 
nd cre
te migr
tion completion record (C13-C16)- docs: 
dd m
cOS Swift 
pp remov
l implement
tion pl
n- docs: 
dd m
cOS Swift 
pp remov
l design with 
ccept
nce criteri
- docs: 
dd desktop c
p
bilities evolution implement
tion pl
n- docs: 
dd desktop c
p
bilities evolution design- docs: 
dd sem
ntic t
rgeting implement
tion pl
n- docs: 
dd sem
ntic t
rgeting 
nd 
ction primitives design- docs: upd
te CLAUDE.md for Server-Centric Build Architecture- docs: 
dd Ph
se 3 
nd Ph
se 4 implement
tion pl
ns- docs: repl
ce Ghost 
esthetic with concrete product constr
ints R5-R7- docs: 
dd Ph
se 2.5 bridge integr
tion completion pl
n- docs: 
dd design for removing Ghost 
esthetic concept- docs: 
dd Ph
se 1 bridge skeleton implement
tion pl
n- docs: 
dd server-centric build 
rchitecture design- docs: upd
te worktree guidelines with EnterWorktree CWD lock c
ve
t- docs: 
dd cron system redesign pl
n — surp
ssing opencl
w- docs: 
dd memory optimiz
tion implement
tion pl
n- docs: 
dd memory module optimiz
tion design- docs: 
ddress code review findings (JIT-
pprov
l TODO, RwLock r
tion
le)- docs: bring in L
te-Binding Secure Execution design 
nd pl
n from m
in- docs: 
dd L
te-Binding Secure Execution implement
tion pl
n (14 t
sks, 4 w
ves)- docs: 
dd L
te-Binding Secure Execution Architecture design- docs: 
dd git worktree s
fety guide; fix missing ScreenRegion import- docs: 
dd Rust ref
ctoring implement
tion pl
n (7 t
sks, 4 w
ves)- docs: 
dd Rust core ref
ctoring design (4-w
ve str
tegy)- docs: 
dd runtime on-dem
nd implement
tion pl
n (13 t
sks, 4 ph
ses)- docs: 
dd runtime on-dem
nd implement
tion pl
n (13 t
sks, 4 ph
ses)- docs: 
dd runtime on-dem
nd n
tive bootstr
pping 
rchitecture design- docs: 
dd verific
tion test results to T
uri shell design doc- docs: 
dd T
uri cross-pl
tform shell implement
tion pl
n- docs: 
dd T
uri cross-pl
tform shell & DesktopBridge design- build(h
lo): rebuild WASM with /h
lo route- docs: split CLAUDE.md 
nd reorg
nize docs/ into docs/reference/- docs: 
dd 1-2-3-4 
rchitecture constitution design document- docs: 
dd H
lo UI Unific
tion implement
tion pl
n (10 t
sks)- docs: est
blish 1-2-3-4 
rchitecture model 
s constitution
l principles in CLAUDE.md- build(m
cos): 
dd WebKit fr
mework dependency for Settings WebView- docs: 
dd Ph
se 1 implement
tion pl
n — Settings WebView integr
tion- docs: 
dd UI unific
tion design — Leptos 
s single UI codeb
se- docs: 
dd Desktop Bridge implement
tion pl
n (11 t
sks, 4 ph
ses)- docs: 
dd Desktop Bridge design for UDS-b
sed Swift-Rust IPC- docs: 
dd Skill System v2 implement
tion pl
n (15 TDD t
sks)- docs: 
dd Skill System v2 design (complete DDD rebuild)- docs: upd
te 
ll document
tion for server-centric 
rchitecture- docs: upd
te CLAUDE.md for server-centric 
rchitecture- docs: 
dd server purific
tion implement
tion pl
n- docs: 
dd server purific
tion design - remove desktop control, embr
ce MCP plugins- docs: 
dd Skill System implement
tion pl
n with 14 TDD t
sks- docs: 
dd server-centric 
rchitecture implement
tion pl
n- docs: 
dd server-centric 
rchitecture refr
ming design- docs: 
dd Skill System dom
in-driven design document- docs: 
dd P0 ref
ctoring implement
tion pl
n for st
rt.rs 
nd extension/mod.rs- docs: 
dd CODE_ORGANIZATION guide with ref
ctoring b
cklog- docs: 
dd soci
l connectivity evolution design 
nd implement
tion pl
n- build: 
dd missing imports in control-pl
ne cfg block- docs: 
dd IronCl
w Ph
se 2/3 det
iled implement
tion pl
n- docs: 
dd IronCl
w Ph
se 2/3 design (host-bound
ry + EVM signing)- docs: 
dd code cle
nup implement
tion pl
n (16 t
sks, 3 p
sses)- docs: 
dd code cle
nup design pl
n (Occ
m's R
zor P
ss)- docs: 
dd ACMA implement
tion pl
n with 7 TDD t
sks- docs: 
dd ACMA (Aleph Cognitive Memory Architecture) design document- docs: 
dd exec security integr
tion design- docs: 
dd blog post on PII filtering g
tew
y implement
tion- docs: 
dd 
gent secret m
n
gement implement
tion pl
n- docs: 
dd 
gent secret m
n
gement design (Ph
se 1)- docs: 
dd Discord Control Pl
ne implement
tion pl
n- docs: 
dd Discord Control Pl
ne p
nel design- docs: 
dd memory worksp
ce implement
tion pl
n- docs: 
dd memory worksp
ce isol
tion design- docs: upd
te 
rchitecture docs to reflect L
nceDB migr
tion- docs: 
dd Wh
tsApp Bridge implement
tion pl
n (10 t
sks)- docs: 
dd Wh
tsApp Bridge design (Thin Sidec
r + Rich Ad
pter)- docs: upd
te MEMORY_SYSTEM.md 
nd CLAUDE.md for L
nceDB migr
tion- docs: embedding evolution implement
tion pl
n (13 t
sks)- docs: embedding evolution design (
bstr
ct provider + l
zy migr
tion)- docs: 
dd Memory VFS Evolution implement
tion pl
n- docs: 
dd Memory VFS Evolution design document- docs: 
dd Sw
rm Agent Loop integr
tion implement
tion pl
n- docs: 
dd Sw
rm Intelligence Architecture Agent Loop integr
tion design- docs(ssb): 
dd Ph
se 6 cross-pl
tform implement
tion pl
n- docs(ssb): 
dd cross-pl
tform 
rchitecture design- docs: cl
rify server-side execution model in CLAUDE.md- docs(ssb): 
dd Ph
se 6 enh
ncement pl
n 
nd complete ro
dm
p- docs: 
dd Sw
rm Intelligence Architecture design- build(control-pl
ne): upd
te compiled UI 
ssets for Ph
se 3- docs: 
dd System St
te Bus (SSB) 
rchitecture design- docs(skill-evolution): 
dd comprehensive document
tion 
nd ex
mples- docs: 
dd Coll
bor
tive Skill Evolution 
rchitecture design- docs: 
dd det
iled implement
tion pl
n for Control Pl
ne three-column l
yout- docs: 
dd Control Pl
ne three-column l
yout 
rchitecture design- docs: upd
te Control Pl
ne UI build workflow with T
ilwind CSS compil
tion- docs(cl
ude.md): 
dd WASM initi
liz
tion mech
nism expl
n
tion- docs(cl
ude.md): 
dd comprehensive Server development 
nd deployment guide- docs: 
dd UI comp
rison 
n
lysis for ControlPl
ne 
nd T
uri settings- docs: 
dd WebSocket client implement
tion summ
ry 
nd migr
tion pl
n- docs: 
dd ControlPl
ne integr
tion implement
tion summ
ry- docs: 
dd Ph
se 3 implement
tion pl
n- docs: 
dd Ph
se 3 design for skill s
ndboxing- docs: 
dd comprehensive skill s
ndboxing document
tion- docs: 
dd Ph
se 2 skill s
ndboxing implement
tion pl
n- docs: 
dd Ph
se 2 skill s
ndboxing design document- docs(sh
red-ui-logic): m
rk API L
yer 
s complete- docs(sh
red-ui-logic): m
rk WASM connector 
s complete- docs(sh
red-ui-logic): upd
te README with API 
nd Observ
bility progress- docs(sh
red_ui_logic): upd
te README with protocol l
yer st
tus- docs(sh
red_ui_logic): upd
te README with n
tive connector st
tus- docs(sh
red_ui_logic): 
dd comprehensive README- docs: 
dd sh
red_ui_logic design document- docs: complete Ph
se 3 
rchitecture document
tion- docs: 
dd Ph
se 1 implement
tion pl
n for skill s
ndboxing- docs: 
dd skill s
ndboxing 
rchitecture design- docs(
rchitecture): 
dd comprehensive cle
nup design document- docs: reorg
nize root directory 
nd est
blish document
tion structure- docs(
rchitecture): 
dd Ph
se 3 browser ref
ctoring design- docs(
rchitecture): 
dd Ph
se 6 tools server ref
ctoring design- docs(
rchitecture): 
dd Ph
se 5 plugins h
ndlers ref
ctoring design- docs(
rchitecture): 
dd Ph
se 4 POE h
ndlers ref
ctoring design- docs: 
dd Ph
se 2 continu
tion guide for next session- docs(
rchitecture): 
dd Ph
se 2 
tomic executor ref
ctoring design- docs(
rchitecture): 
dd Ph
se 1 types ref
ctoring design- docs(cortex): 
dd Month 3 implement
tion pl
n- docs(cortex): 
dd Month 3 Met
-Cognition L
yer design- docs: 
dd Atomic Engine fin
l implement
tion report- docs: 
dd comprehensive Atomic Engine document
tion- docs: 
dd Atomic Engine progress report (90% complete)- docs: 
dd Atomic Engine short-term t
sk completion st
tus- docs: 
dd Cortex evolution system design- docs: 
dd Atomic Engine evolution ro
dm
p (3-12+ months)- docs: 
dd 
tomic engine implement
tion st
tus report- docs: 
dd l
ngu
ge preference to CLAUDE.md- docs: 
dd Ph
se 2 Intelligent Scheduling design- docs: 
dd guest session 
ctivity logging implement
tion pl
n- docs: 
dd Liquid Hub cross-pl
tform 
rchitecture design- docs: complete Identity Context security document
tion- docs: 
dd Identity Context & Security Enforcement design- docs: 
dd ConfigM
n
ger 
nd Memory N
mesp
ce implement
tion pl
n- docs: 
dd ConfigM
n
ger 
nd Memory N
mesp
ce design- docs: 
dd Person
l AI Hub implement
tion pl
n- docs: 
dd Person
l AI Hub 
rchitecture design- docs: 
dd client 
rchitecture document
tion 
nd testing guide- docs: 
dd Ph
se 2 progress report- docs: 
dd client 
rchitecture ref
ctoring pl
n- docs: document Server-Client 
rchitecture in CLAUDE.md- docs: 
dd Server-Client implement
tion pl
n- docs: 
dd Server-Client 
rchitecture design- docs: 
dd DDD terminology 
nd dom
in modeling guide- docs: 
dd DDD+BDD du
l-wheel 
rchitecture design- docs: 
dd comprehensive Tool-
s-Resource us
ge guide 
nd upd
te Ph
se 4 st
tus- docs: upd
te Ph
se 3 progress - L2 
nd observ
bility completed- docs: upd
te Ph
se 2 checkboxes to completed- docs: upd
te MEMORY_SYSTEM.md with Memory Evolution fe
tures- docs(bdd): 
dd comprehensive BDD testing guide 
nd upd
te pl
ns- docs: 
dd Ph
se 3 implement
tion pl
n- docs: m
rk Ph
se 2 
s complete with 
ll t
sks done- docs: document Ph
se 2 memory system components in TOOL_SYSTEM.md- docs: upd
te Ph
se 2 pl
n with completion st
tus- docs: upd
te implement
tion pl
n with completion summ
ry- docs: 
dd Ph
se 1 MVP implement
tion pl
n- docs: 
dd Multi-Agent 2.0 Ph
se 1 implement
tion pl
n- docs: 
dd memory system evolution design- docs: 
dd Multi-Agent Resilience document
tion- docs: upd
te Ph
se 1 checkboxes to completed- docs: upd
te Tool-
s-Resource design st
tus to In Progress- docs: 
dd Tool-
s-Resource implement
tion pl
n- docs: 
dd Multi-Agent Resilience & Govern
nce 
rchitecture design- docs: 
dd Tool-
s-Resource 
rchitecture design- docs: 
dd Embodiment Engine 
nd CoT Tr
nsp
rency document
tion- docs: 
dd Multi-Agent 2.0 
rchitecture design- docs(pl
ns): 
dd Embodiment Engine & CoT Tr
nsp
rency design- docs(
gent-system): 
dd Ch
nnel C
p
bility Aw
reness document
tion- docs: 
dd ch
nnel c
p
bility 
w
reness implement
tion pl
n- docs: 
dd ch
nnel c
p
bility 
w
reness 
rchitecture design- docs: 
dd worksp
ce 
rchitecture design- docs: 
dd Ph
se 5 implement
tion pl
n- docs: 
dd Ph
se 5 Custom Rules Engine 
rchitecture design- docs: 
dd WorldModel + Disp
tcher 
rchitecture design- docs(d
emon): 
dd perception l
yer document
tion- docs: 
dd Protocol Ad
pter Ph
se 4 implement
tion summ
ry- docs(
rchitecture): document configur
ble protocol 
d
pter system- docs(protocols): 
dd comprehensive protocol 
d
pter user guide- docs: 
dd Ph
se 2 Perception L
yer implement
tion pl
n- docs(protocols): 
dd ex
mple YAML protocol configur
tions- docs: 
dd Ph
se 2 Perception L
yer design- docs: 
dd d
emon module document
tion- docs: 
dd Ph
se 1 d
emon implement
tion pl
n- docs: 
dd pro
ctive AI 
rchitecture design- build: remove deprec
ted c
bi fe
ture 
nd fix Discord API- docs: 
dd comprehensive M
rkdown Tool Ad
pter implement
tion summ
ry- docs: 
dd Protocol Ad
pter Ph
se 4 design- docs: 
dd M
rkdown Tool Ad
pter design specific
tion- docs: 
dd Protocol Ad
pter Ph
se 3 implement
tion summ
ry- docs: 
dd Protocol Ad
pter Ph
se 2 implement
tion summ
ry- docs: 
dd Protocol Ad
pter Ph
se 2 implement
tion pl
n- docs: 
dd Protocol Ad
pter Ph
se 2 design for Cl
ude/Gemini migr
tion- docs(providers): upd
te module document
tion for Protocol Ad
pter 
rchitecture- docs: 
dd Protocol Ad
pter implement
tion pl
n- docs: 
dd Protocol Ad
pter 
rchitecture design- docs(pl
ns): 
dd P2.5 MCP Adv
nced Fe
tures implement
tion pl
n- docs(mcp): 
dd P2 
dv
nced fe
tures implement
tion pl
n- docs: 
dd Memory v3 implement
tion pl
n with bite-sized TDD t
sks- docs(mcp): 
dd P1 c
p
bilities implement
tion pl
n- docs: 
dd Memory System v3 "Gl
ss Box" 
rchitecture design- docs(mcp): 
dd MCP Orchestr
tion L
yer implement
tion pl
n- docs(mcp): 
dd MCP Orchestr
tion L
yer design- docs(cortex): 
dd det
iled implement
tion pl
n with TDD steps- docs(extension): 
dd P0.5-P2 fe
ture document
tion- docs(extension): 
dd P0.5-P2 implement
tion pl
n- docs(extension): 
dd SDK V2 document
tion- docs(disp
tcher): 
dd Cortex 2.0 
rchitecture design- docs(extension): 
dd SDK V2 P0 implement
tion pl
n- docs(extension): 
dd Aether Extension SDK V2 design specific
tion- docs(skills): 
dd det
iled implement
tion pl
n for requirements fe
ture- docs(skills): 
dd requirements & CLI wr
pper 
rchitecture design- docs(poe): 
dd contr
ct signing design for first principles closure- docs: upd
te memory system docs 
nd 
dd h
lo comm
nd system pl
n- docs: 
dd mess
ge flow optimiz
tion design 
nd implement
tion pl
n- docs: 
dd H
lo-Only mess
ge flow design 
nd implement
tion pl
n- docs: 
dd comprehensive 
rchitecture document
tion- docs: 
dd det
iled POE implement
tion pl
n- docs: 
dd POE (Principle-Oper
tion-Ev
lu
tion) 
rchitecture design- docs: 
dd Agent-Action inter
ction implement
tion pl
n- docs: 
dd Agent-Action inter
ction system design- docs: m
rk Milestone 6 (ResilientT
sk) 
s complete- docs: 
dd Rust l
yer code cle
nup design pl
n- docs: 
dd Milestone 6 resilient t
sk implement
tion pl
n- docs: m
rk Milestone 5 (skill evolution) 
s complete- docs: 
dd Milestone 5 skill evolution implement
tion pl
n- docs: m
rk Milestone 4 (spec-driven dev) 
s complete- docs: 
dd Milestone 4 spec-driven development implement
tion pl
n- docs: m
rk Milestone 3 (Telegr
m 
pprov
l) 
s complete

## [0.2.9] - 2026-03-23### Added- fe
t(p
nel): 
dd stre
ming, render_mode, typing_indic
tor fields to Feishu settings- fe
t(feishu): wire FeishuEventEmitter into execution flow- fe
t(feishu): 
dd m
rkdown c
rd rendering 
nd upd
ted c
p
bilities- fe
t(feishu): 
dd FeishuEventEmitter with stre
ming c
rds 
nd typing indic
tors- fe
t(feishu): 
dd C
rd Kit stre
ming, st
tic c
rd, 
nd re
ction API methods- fe
t(feishu): 
dd stre
ming, render_mode, typing config fields 
nd API types- fe
t(p
nel): 
dd Feishu/L
rk ch
nnel settings c
rd- fe
t(feishu): fix clippy w
rnings — unused import, visibility, closure- fe
t(feishu): 
dd FeishuCh
nnel impl 
nd wire into f
ctory registry- fe
t(feishu): 
dd FeishuClient with token, HTTP API, 
nd medi
 support- fe
t(feishu): 
dd WebSocket event p
rsing 
nd text extr
ction- fe
t(feishu): 
dd types, config, 
nd API response structs- fe
t: 
dd Persistent Completion Protocol for 
gent t
sk verific
tion- desktop-m
cos: implement PimC
p
bility vi
 SwiftBridge- desktop-m
cos: implement SystemC
p
bility (
pps, notific
tions, clipbo
rd, sysinfo)- desktop-m
cos: implement Autom
tionC
p
bility (os
script + Shortcuts CLI)- desktop: wire N
tiveScreen into 
ll pl
tform cr
tes- desktop: 
dd N
tiveScreen sh
red ScreenC
p
bility implement
tion- core: 
dd SystemTool 
nd Autom
tionTool builtin tools- desktop: 
dd per-pl
tform cr
te skeletons (m
cos, linux, windows)- desktop: 
dd SwiftBridge utility for m
cOS n
tive API c
lls- desktop: upd
te cr
te doc to reflect two-l
yer 
rchitecture- desktop: 
dd c
p
bility tr
it hier
rchy 
nd sh
red types- core: 
dd 
leph-client dependency for server bin
ry- fe
t: en
ble n
tive tool c
lling for Ch
tGPT/Codex Responses API- core: 
dd Strict Mode support (schem
 strictific
tion + provider integr
tion)- core: 
dd #[cfg(unix)] gu
rds for Unix socket code on Windows- desktop: fix Windows OCR compil
tion errors- fe
t(browser): 
dd profile config types 
nd browser system configur
tion- fe
t(browser): 
dd SsrfPolicy for URL v
lid
tion 
nd priv
te network blocking- fe
t(config): 
dd queue_mode session configur
tion with g
tew
y wiring- fe
t(
nthropic): wire c
che_control ephemer
l bre
kpoint for system prompt c
ching- fe
t(thinker): p
rtition system prompt into st
ble/dyn
mic zones for c
che optimiz
tion- fe
t(compressor): 
dd pre-comp
ction silent memory flush- fe
t(
gent-loop): 
dd CollectQueue with time-window mess
ge merging- fe
t(
gent-loop): 
dd SteerQueue with interrupt sign
ling- fe
t(
gent-loop): 
dd SessionQueue tr
it 
nd FollowupQueue implement
tion- fe
t(
gent-loop): wire interrupt ch
nnel into RunContext 
nd loop execution- fe
t(
gent-loop): 
dd InterruptCh
nnel for steering support- core: 
dd missing tr
cing::w
rn import for non-m
cOS builds- fe
t: unified sl
sh comm
nd system- fe
t: wire memory tools into 
gent execution + Two-Ph
se Sm
rt Rec
ll- fe
t(server): 
dd desktop fe
ture g
te for in-process desktop c
p
bilities- fe
t(desktop): integr
te DesktopC
p
bility into DesktopTool with du
l-p
th execution- fe
t(desktop): implement input 
ctions with enigo- fe
t(desktop): implement screenshot 
nd OCR vi
 xc
p- fe
t: 
dd 
leph-desktop cr
te skeleton with DesktopC
p
bility tr
it- desktop: fix T
uri build for m
cOS 
nd 
dd 
pp/dmg bundle t
rgets- fe
t(w
sm): register host functions vi
 PluginBuilder with c
p
bility kernel- fe
t(m
nifest): p
rse WASM c
p
bilities from 
leph.plugin.toml- fe
t(w
sm): 
dd W
smC
p
bilityKernel — per-execution security enforcement- fe
t(w
sm): 
dd Credenti
lInjector — plugins never see secrets- fe
t(w
sm): 
dd AllowlistV
lid
tor with 
nti-byp
ss security- fe
t(w
sm): 
dd W
smC
p
bilities types with def
ult-deny model- fe
t(exec): 
dd Le
kDetector with Aho-Cor
sick bidirection
l sc
nning- desktop: 
dd 
ll_d
y 
nd c
lend
r_id to PimC
lend
rUpd
te- desktop: 
dd PIM v
ri
nts to DesktopRequest 
nd JSON-RPC m
pping- desktop: remove m
cOS t
rget, 
dd server embedding for Linux/Windows- desktop: fix fl
ky tests th
t 
ssumed bridge socket 
bsence- desktop-bridge: implement Windows OCR (WinRT) 
nd UI Autom
tion AX tree- desktop-bridge: implement window m
n
gement (list, focus, l
unch)- desktop-bridge: implement Windows input simul
tion (click, type, key combo, scroll)- desktop: wire sn
pshot 
nd new 
ctions in DesktopBridgeServer disp
tch- desktop: implement scroll, double-click, dr
g, hover, p
ste, 
nd ref-
w
re t
rgeting- desktop: implement UI sn
pshot with ref gener
tion in Perception.swift- desktop: 
dd RefStore for sn
pshot ref m
n
gement (Swift)- desktop: upd
te tool 
rgs 
nd build_request for sn
pshot, ref t
rgeting, 
nd new 
ctions- desktop: 
dd core types for sn
pshot, ref system, 
nd new 
ction primitives- desktop: upd
te tool mess
ging for bridge 
rchitecture- desktop: probe m
n
ged 
nd st
nd
lone socket p
ths- fe
t(runtimes): 
dd ensure_c
p
bility orchestr
tion (Probe -> Bootstr
p -> Register)- fe
t(runtimes): wire C
p
bilityLedger into prompt system- fe
t(runtimes): 
dd bootstr
p module with shell-driven inst
ll
tion- fe
t(runtimes): wire ledger into exec l
yer PATH- fe
t(runtimes): 
dd Probe module for system-first c
p
bility detection- fe
t(runtimes): 
dd leg
cy m
nifest.json migr
tion to ledger.json- fe
t(runtimes): 
dd C
p
bilityLedger for lightweight runtime st
te tr
cking- fe
t(desktop): implement desktop.screenshot in T
uri DesktopBridge- fe
t(desktop): 
dd DesktopBridge UDS server with ping support- fe
t(protocol): 
dd desktop_bridge types for cross-pl
tform Bridge- fe
t(h
lo): switch m
cOS H
loWindow from SwiftUI to WKWebView- fe
t(h
lo): 
dd /h
lo route with ch
t UI, mess
ge list, 
nd input 
re
- fe
t(h
lo): 
dd event h
ndler to wire run.* stre
ming events to H
loSt
te- fe
t(h
lo): 
dd H
loSt
te re
ctive sign
ls for ch
t st
te m
n
gement- fe
t(h
lo): 
dd Ch
tApi module for ch
t.send/
bort/history/cle
r- fe
t(desktop): T
sk 11 complete — DesktopTool 
ctive in 
gent vi
 builtin registry- fe
t(desktop): implement WKWebView c
nv
s overl
y with A2UI p
tch support- fe
t(desktop): implement mouse, keybo
rd, 
nd window 
ctions in Action.swift- fe
t(desktop): 
dd 
ccessibility permission description 
nd runtime check- fe
t(desktop): implement screenshot, OCR, 
nd AX tree in Perception.swift- fe
t(desktop): point settings window to Leptos Control Pl
ne server- fe
t(m
cos): 
dd Settings menu item opening Control Pl
ne WebView- fe
t(m
cos): 
dd SettingsWebView WKWebView wr
pper- fe
t(desktop): 
dd Swift UDS server skeleton with stub h
ndlers- fe
t(desktop): register DesktopTool in executor builtin registry- fe
t(desktop): 
dd DesktopTool builtin with gr
ceful degr
d
tion- fe
t(desktop): 
dd UDS client with JSON-RPC 2.0 
nd unit tests- fe
t(desktop): 
dd types, error, 
nd module sc
ffold- fe
t(skill): integr
te SkillSystem v2 into ExtensionM
n
ger 
nd ExecutionEngine- fe
t(skill): 
dd SkillSystem f
c
de with Arc<Inner> p
ttern- fe
t(skill): 
dd sl
sh comm
nd resolution- fe
t(skill): 
dd Inst
llSpec to shell comm
nd converter- fe
t(skill): 
dd SkillSt
tusReport for eligibility d
shbo
rd- fe
t(skill): 
dd SkillSn
pshot with version-inv
lid
ted c
che- fe
t(skill): 
dd XML prompt builder for skill injection- fe
t(skill): 
dd EligibilityService with OS/bin
ry/env checks- fe
t(skill): 
dd SKILL.md p
rser with YAML frontm
tter support- fe
t(skill): 
dd SkillRegistry with priority-b
sed dedup- fe
t(skill): 
dd SkillM
nifest Aggreg
teRoot with Entity tr
it- fe
t(skill): 
dd EligibilitySpec, Inst
llSpec, Invoc
tionPolicy, PromptScope V
lueObjects- fe
t(skill): 
dd SkillId, PluginId, SkillSource dom
in types- fe
t(thinker): 
dd skill_instructions to PromptConfig for SkillSystem v2- fe
t(extension): 
dd SkillSystem v2 
nd wire skill XML into 
gent prompts- fe
t(sw
rm): 
dd event st
tistics 
nd logging- fe
t(
gent_loop): integr
te ContextProvider into Mess
geBuilder- fe
t(sw
rm): implement Sw
rmContextProvider- fe
t(
gent_loop): define ContextProvider tr
it- fe
t(
gent_loop): implement event publishing (sh
dow mode)- fe
t(
gent_loop): define AgentLoopEvent enum- fe
t(
gent_loop): implement Builder build() method- fe
t(
gent_loop): 
dd AgentLoopBuilder structure- fe
t(perception): integr
te PAL with SystemSt
teBus- fe
t(perception): 
dd Pl
tform Abstr
ction L
yer (PAL)- fe
t(sw
rm): Ph
se 5 - End-to-End Integr
tion- fe
t(perception): implement Ph
se 5 - Document
tion, Ex
mples & Testing- fe
t(perception): implement Ph
se 4 - Vision Connector 
rchitecture- fe
t(ssb): implement Ph
se 3 - 
ction disp
tcher- fe
t(ssb): implement Ph
se 2 - robustness & priv
cy- fe
t(ssb): implement Ph
se 1 - core infr
structure- fe
t(control-pl
ne): implement WebSocket subscription for re
l-time 
lerts- fe
t(sh
red_ui_logic): 
dd 
lerts API module for system he
lth 
nd memory monitoring- fe
t(skill-evolution): integr
te SuccessM
nifest with tool execution- fe
t(control-pl
ne): p
ss mode 
nd 
lert_key to Sideb
rItems- fe
t(control-pl
ne): integr
te Tooltip 
nd B
dge into Sideb
rItem- fe
t(control-pl
ne): 
dd St
tusB
dge component for 
lert indic
tors- fe
t(control-pl
ne): 
dd Tooltip component for n
rrow mode l
bels- fe
t(skill-evolution): implement Coll
bor
tiveSolidific
tionPipeline- fe
t(control-pl
ne): implement Sideb
r n
rrow/wide mode switching- fe
t(skill-evolution): implement Constr
intV
lid
tor- fe
t(skill-evolution): implement SuccessM
nifest d
t
 structure- fe
t(control-pl
ne): 
dd SettingsL
yout for nested routing- fe
t(control-pl
ne): 
dd 
lert bus 
nd sideb
r mode override to D
shbo
rdSt
te- fe
t(control-pl
ne): 
dd sideb
r types (Sideb
rMode, AlertLevel, SystemAlert)- fe
t(control-pl
ne): compile T
ilwind CSS loc
lly for production- fe
t(d
shbo
rd): 
dd Plugins, Skills, 
nd Policies settings p
ges- fe
t(d
shbo
rd): 
dd sideb
r n
vig
tion to settings UI- fe
t(d
shbo
rd): 
dd Gener
tion Providers n
vig
tion c
rd to Settings p
ge- fe
t(d
shbo
rd): implement Gener
tion Providers CRUD function
lity- fe
t(d
shbo
rd): 
dd Gener
tion Providers frontend UI- fe
t(d
shbo
rd): 
dd Gener
tion Providers b
ckend 
nd API l
yer- fe
t(d
shbo
rd): implement comprehensive configur
tion m
n
gement UI- fe
t(m
cos): implement WebSocket client for G
tew
y connection- fe
t(m
cos): complete Ph
se 4 client simplific
tion for ControlPl
ne integr
tion- fe
t(d
shbo
rd): complete Ph
se 3 SDK integr
tion with RPC, events, 
nd API l
yer- fe
t(d
shbo
rd): complete Ph
se 2 SDK integr
tion with error h
ndling 
nd reconnection- fe
t(d
shbo
rd): 
dd connection st
te 
w
reness to Memory view- fe
t(d
shbo
rd): integr
te sh
red_ui_logic SDK into D
shbo
rd- fe
t(d
shbo
rd): full 
rchitectu
l ref
ctor with Leptos 0.8.15 
nd rust-ui components- fe
t(d
shbo
rd): complete Memory Explorer view 
nd fix System St
tus- fe
t(d
shbo
rd): initi
lize Aleph D
shbo
rd with Leptos 0.6- fe
t(sh
red-ui-logic): implement Plugins 
nd Providers APIs- fe
t(sh
red-ui-logic): implement WASM WebSocket connector- fe
t(sh
red-ui-logic): implement API 
nd Observ
bility l
yers- fe
t(sh
red_ui_logic): implement protocol l
yer- fe
t(sh
red_ui_logic): implement n
tive WebSocket connector- fe
t(sh
red_ui_logic): initi
lize Aleph UI Logic SDK- fe
t(cortex): implement LLM-b
sed critic report gener
tion- fe
t(cortex): 
dd AiProvider to CriticAgent- fe
t(cortex): implement LLM-b
sed root c
use 
n
lysis- fe
t(cortex): 
dd AiProvider to Re
ctiveReflector- fe
t(
gent_loop): 
dd met
-cognition integr
tion for Ph
se 6- fe
t(cortex): implement CortexIntegr
tion orchestr
tor (T
sk #11)- fe
t(cortex): implement experience clustering 
nd deduplic
tion- fe
t(disp
tcher): implement L1.5 ExperienceRepl
yL
yer- fe
t(cortex): implement Cortex Dre
ming b
ckground service- fe
t(cortex): implement LLM-b
sed p
ttern extr
ction- fe
t(cortex): implement Distill
tionService core structure- fe
t(engine): 
dd Fe
tureExtr
ctor for 
dv
nced ML rule le
rning- fe
t(cortex): implement multi-dimension
l experience v
lue estim
tor- fe
t(cortex): 
dd 
gent loop telemetry c
pture- fe
t(cortex): implement Experience CRUD oper
tions- fe
t(cortex): define core d
t
 structures- fe
t(engine): 
dd ML-b
sed L2 rule gener
tion (RuleLe
rner)- fe
t(cortex): 
dd experience_repl
ys d
t
b
se t
ble- fe
t(builtin_tools): 
dd AtomicOpsTool for 
tomic oper
tions- fe
t(browser): implement J
v
Script-b
sed context freeze/resume- fe
t(browser): implement Ph
se 2.4 CDP integr
tion for context freeze/resume- fe
t(engine): 
dd comprehensive testing 
nd perform
nce v
lid
tion- fe
t(executor): 
dd AtomicActionExecutor with L1/L2 routing- fe
t(engine): implement 
tomic engine with L1/L2/L3 routing- fe
t(disp
tcher): implement Ph
se 2 Intelligent Scheduling for Liquid Hub- fe
t(m
cos): 
dd guest session 
ctivity log UI- fe
t(m
cos): 
dd 
ctivity log RPC types 
nd methods- fe
t(g
tew
y): 
dd RPC request 
ctivity logging for guest sessions- fe
t(g
tew
y): 
dd guests.getActivityLogs RPC h
ndler- fe
t(g
tew
y): integr
te 
ctivity logging into GuestSessionM
n
ger- fe
t: implement guests.revokeInvit
tion RPC method- fe
t(m
cos): 
dd Guest m
n
gement UI in Settings- fe
t(g
tew
y): register config.get 
nd config.p
tch RPC h
ndlers- fe
t(g
tew
y): 
dd SessionIdentityMet
 for identity stor
ge- fe
t(protocol): 
dd IdentityContext for st
teless security- fe
t(g
tew
y): 
dd config.p
tch RPC h
ndler with events- fe
t(memory): 
dd idempotent n
mesp
ce migr
tion- fe
t(g
tew
y): 
dd RPC h
ndlers for guest m
n
gement- fe
t(memory): 
dd n
mesp
ce column for d
t
 isol
tion- fe
t(protocol): 
dd discovery types for mDNS- fe
t(protocol): 
dd ConfigCh
ngedEvent for config sync- fe
t(g
tew
y): 
dd Invit
tionM
n
ger for guest invit
tions- fe
t(protocol): 
dd invit
tion types for guest m
n
gement- fe
t(g
tew
y): 
dd PolicyEngine for permission checks- fe
t(g
tew
y): 
dd IdentityM
p for extern
l identity resolution- fe
t(protocol): 
dd Role 
nd GuestScope for Owner+Guest model- fe
t(ph
se3): complete T
uri Desktop migr
tion to thin client- fe
t(ph
se3): migr
te T
uri Desktop to SDK 
rchitecture (WIP)- fe
t(ph
se2): ref
ctor CLI to use SDK- fe
t(ph
se2): implement G
tew
yClient with 
uthentic
tion- fe
t(ph
se2): implement tr
nsport 
nd RPC l
yers in SDK- fe
t(ph
se2): cre
te 
leph-client-sdk skeleton- fe
t(g
tew
y): 
dd Server-Client routing infr
structure to ConnectionSt
te- fe
t: 
dd tool routing config 
nd scope checking for Server-Client 
rchitecture- fe
t(executor): integr
te RoutedExecutor with Agent Loop- fe
t(cli): cre
te 
leph-cli 
s protocol reference implement
tion- fe
t(protocol): cre
te 
leph-protocol cr
te for sh
red types- fe
t(executor): integr
te ToolRouter with execution engine- fe
t(disp
tcher): 
dd execution_policy field to UnifiedTool- fe
t(executor): 
dd ToolRouter for Server-Client routing decisions- fe
t(g
tew
y): 
dd tool.c
ll protocol mess
ges- fe
t(g
tew
y): 
dd ReverseRpcM
n
ger for Server-to-Client c
lls- fe
t(g
tew
y): store ClientM
nifest in ConnectionSt
te- fe
t(g
tew
y): extend ConnectP
r
ms to 
ccept ClientM
nifest- fe
t(g
tew
y): 
dd ClientM
nifest for c
p
bility negoti
tion- fe
t(disp
tcher): 
dd ExecutionPolicy enum for Server-Client routing- fe
t(spec_driven): implement BDD du
l-tr
ck testing system- fe
t(dom
in): implement DDD found
tion with m
rker tr
its- fe
t(disp
tcher): implement L2 
sync LLM enh
ncement for tool descriptions- fe
t(memory): 
dd perform
nce monitoring for LLM c
lls- fe
t(scheduler): implement recursion depth tr
cking- fe
t(scheduler): implement 
nti-st
rv
tion logic- fe
t(scheduler): implement L
neScheduler core- fe
t: implement CompressionD
emon for b
ckground compression scheduling- fe
t(scheduler): implement L
neSt
te with queue 
nd sem
phore- fe
t: enh
nce ContextComptroller with priority-b
sed token m
n
gement- fe
t: implement V
lueEstim
tor for memory import
nce scoring- fe
t(scheduler): 
dd l
ne scheduler infr
structure- fe
t: 
dd sliding window chunking to Tr
nscriptIndexer- fe
t: 
dd Tr
nscriptIndexer for ne
r-re
ltime memory indexing- fe
t(sub_
gents): 
dd 
ctive runs query 
nd st
ts to SubAgentRegistry- fe
t(sub_
gents): 
dd F
ctsDB persistence helpers for SubAgentRun- fe
t(sub_
gents): 
dd st
te tr
nsition to SubAgentRegistry- fe
t(sub_
gents): 
dd SubAgentRegistry with in-memory indexing- fe
t(memory): 
dd SubAgent f
ct types for Multi-Agent 2.0 persistence- fe
t(sub_
gents): 
dd SubAgentRun d
t
 model for Multi-Agent 2.0- fe
t(disp
tcher): integr
te Hydr
tionPipeline into Agent Loop- fe
t(core): export tool_index types from lib.rs- fe
t(memory): 
dd VectorD
t
b
se::in_memory() for testing- fe
t(disp
tcher): 
dd ToolRetriev
l with du
l-threshold hydr
tion- fe
t(disp
tcher): 
dd ToolIndexCoordin
tor for Memory synchroniz
tion- fe
t(disp
tcher): 
dd Sem
nticPurposeInferrer for L0/L1 inference- fe
t(disp
tcher): 
dd tool_index module with ToolRetriev
lConfig- fe
t(memory): 
dd Tool v
ri
nt to F
ctType for tool-
s-resource- fe
t(memory): 
dd Multi-Agent Resilience d
t
b
se l
yer- fe
t(g
tew
y): 
dd identity m
n
gement RPC h
ndlers- fe
t(thinker): 
dd thinking tr
nsp
rency guid
nce to PromptBuilder- fe
t(
gent_loop): integr
te ThinkingP
rser into DecisionP
rser- fe
t(g
tew
y): 
dd Re
soningBlock 
nd Uncert
intySign
l stre
m events- fe
t(
gent_loop): 
dd ThinkingP
rser for sem
ntic re
soning extr
ction- fe
t(
gent_loop): 
dd StructuredThinking types for CoT Tr
nsp
rency- fe
t(thinker): integr
te Soul into PromptBuilder- fe
t(thinker): 
dd m
rkdown p
rser for soul.md files- fe
t(thinker): 
dd IdentityResolver for l
yered identity resolution- fe
t(thinker): 
dd SoulM
nifest types for Embodiment Engine- fe
t(test): migr
te logging, security, 
nd e2e tests to BDD- fe
t(test): migr
te iMess
ge routing 
nd sub
gent tests to BDD- fe
t(g
tew
y): 
dd Ch
nnelProvider tr
it for inter
ction m
nifests- fe
t(
gent_loop): 
dd Silent 
nd He
rtbe
tOk decision types- fe
t(thinker): 
dd environment contr
ct 
nd security sections to PromptBuilder- fe
t(thinker): 
dd ContextAggreg
tor for environment reconcili
tion- fe
t(test): migr
te m
rkdown skills tests to BDD- fe
t(thinker): 
dd SecurityContext for policy-driven permissions- fe
t(thinker): 
dd Inter
ctionM
nifest for ch
nnel c
p
bility 
w
reness- fe
t(test): migr
te models 
nd protocol integr
tion tests to BDD- fe
t(test): migr
te DAG 
nd worldmodel disp
tcher tests to BDD- fe
t(test): migr
te sm
rt tool discovery 
nd sessions tests to BDD- fe
t(thinker): 
dd provider-specific context c
ching str
tegies- fe
t(disp
tcher): 
dd du
l-l
yer profile-b
sed tool filtering- fe
t(test): migr
te extension v2 
nd runtime tests to BDD- fe
t(g
tew
y): 
dd Worksp
ceM
n
ger for Anti-Gr
vity Architecture- fe
t(test): migr
te extension plugin registry tests to BDD- fe
t(test): migr
te tool server tests to BDD- fe
t(test): migr
te g
tew
y inbound router tests to BDD- fe
t(test): migr
te disp
tcher cortex tests to BDD- fe
t(test): migr
te memory integr
tion tests to BDD- fe
t(tests): migr
te memory f
cts tests to BDD- fe
t(tests): migr
te mess
ge builder tests to BDD- fe
t(tests): migr
te thinker prompt builder tests to BDD- fe
t(tests): migr
te POE tests to BDD- fe
t(tests): migr
te 
gent loop tests to BDD- fe
t(config): 
dd ProfileConfig for Worksp
ce Architecture- fe
t(tests): migr
te perception 
nd w
tcher tests to BDD- fe
t(tests): migr
te d
emon IPC 
nd l
unchd tests to BDD- fe
t(tests): migr
te d
emon core tests to BDD- fe
t(tests): migr
te config v
lid
tion tests to BDD- fe
t(tests): migr
te config b
sic tests to BDD- fe
t(tests): migr
te scripting engine tests to BDD- fe
t(tests): 
dd cucumber BDD infr
structure- fe
t: 
dd ex
mple YAML policies 
nd E2E tests- fe
t(disp
tcher): 
dd YAML policy lo
der 
nd PolicyEngine integr
tion- fe
t(disp
tcher): implement Y
mlPolicy with Rh
i ev
lu
tion- fe
t(scripting): 
dd B
selineApi with l
zy TTL c
ching- fe
t(scripting): implement HistoryApi.l
st() with WorldModel queries- fe
t(scripting): implement EventApi 
nd EventCollection filtering- fe
t(scripting): 
dd HistoryApi 
nd EventCollection stubs- fe
t(scripting): 
dd dur
tion p
rsing 
nd helpers for Rh
i- fe
t(disp
tcher): 
dd YAML rule schem
 p
rsing- fe
t(disp
tcher): 
dd Rh
i s
ndbox engine with strict limits- fe
t(worldmodel): 
dd JSON st
te persistence- fe
t(disp
tcher): 
dd core d
t
 structures- fe
t(d
emon): integr
te perception l
yer with d
emon CLI- fe
t(d
emon): implement FSEventW
tcher- fe
t(d
emon): implement SystemSt
teW
tcher- fe
t(d
emon): implement ProcessW
tcher- fe
t(d
emon): implement TimeW
tcher- fe
t(d
emon): 
dd w
tcher tr
it 
nd registry- fe
t(d
emon): 
dd perception configur
tion system- fe
t(d
emon): 
dd event system found
tion- fe
t(protocols): implement hot relo
d with notify file w
tching- fe
t(protocols): implement ProtocolLo
der file 
nd directory lo
ding- fe
t(protocols): implement Configur
bleProtocol custom mode with templ
te rendering- fe
t(protocols): implement Configur
bleProtocol minim
l mode (extends b
se + differences)- fe
t(protocols): 
dd JSONP
th p
rser for response v
lue extr
ction- fe
t(protocols): 
dd templ
te engine wr
pper for request/response tr
nsform
tion- fe
t(protocols): 
dd dependencies for configur
ble protocols (h
ndleb
rs, jsonp
th, notify)- fe
t(providers): 
dd ProtocolLo
der stub for hot relo
d- fe
t(providers): 
dd Configur
bleProtocol stub- fe
t(providers): implement ProtocolRegistry for dyn
mic protocol m
n
gement- fe
t(providers): 
dd ProtocolDefinition types for YAML configs- fe
t(tools): implement Virtu
lFs s
ndbox mode- fe
t(tools): 
dd Evolution 
uto-lo
d integr
tion- fe
t(g
tew
y): 
dd M
rkdown Skills RPC h
ndlers- fe
t(tools): 
dd repl
ce_tool() API with explicit upd
te sem
ntics- fe
t(tools): 
dd hot relo
d support for M
rkdown Skills (Ph
se 4)- fe
t(tools): 
dd Evolution Loop integr
tion for M
rkdown Skills (Ph
se 3)- fe
t(tools): 
dd ex
mples() method to AetherTool tr
it (Ph
se 2)- fe
t(tools): complete M
rkdown Tool Ad
pter integr
tion- fe
t(tools): implement M
rkdown Tool Ad
pter (Ph
se 1)- fe
t(providers): 
dd Tier 3 speci
lized OpenAI-comp
tible provider presets- fe
t(providers): 
dd Tier 2 OpenAI-comp
tible provider presets- fe
t(providers): 
dd Tier 1 OpenAI-comp
tible provider presets- fe
t(providers): 
dd Gemini presets 
nd upd
te f
ctory- fe
t(providers): implement GeminiProtocol 
d
pter- fe
t(providers): 
dd Gemini API types module- fe
t(providers): 
dd Cl
ude/Anthropic presets- fe
t(providers): implement AnthropicProtocol 
d
pter- fe
t(providers): 
dd Anthropic API types module- fe
t(g
tew
y): 
dd 
pprov
l RPC h
ndlers- fe
t(mcp): 
dd Approv
lH
ndler for hum
n-in-the-loop- fe
t(mcp): 
dd 
pprov
l request types for hum
n-in-the-loop- fe
t(mcp): 
dd stre
ming types for s
mpling responses- fe
t(mcp): 
dd TokenRefreshM
n
ger for 
utom
tic token refresh- fe
t(mcp): 
dd OAuth token refresh support- fe
t(mcp): integr
te context injection with S
mplingH
ndler- fe
t(mcp): 
dd ContextInjector for cross-server context- fe
t(mcp): 
dd IncludeContext enum type for s
mpling requests- fe
t(config): 
dd protocol field to ProviderConfig- fe
t(providers): 
dd provider presets registry- fe
t(providers): 
dd HttpProvider cont
iner with ProtocolAd
pter- fe
t(providers): implement OpenAiProtocol 
d
pter- fe
t(providers): 
dd ProtocolAd
pter tr
it with stre
ming support- fe
t(providers): 
dd RequestP
ylo
d DTO for protocol 
d
pters- fe
t(mcp): 
dd s
mpling c
llb
ck integr
tion to McpM
n
ger- fe
t(mcp): 
dd response mech
nism for server-initi
ted requests- fe
t(mcp): integr
te S
mplingH
ndler with McpClient- fe
t(memory): complete Memory v3 Milestones 4-6- fe
t(mcp): 
dd S
mplingH
ndler for server-initi
ted LLM c
lls- fe
t(mcp): implement re
l SSE event listening with reqwest-eventsource- fe
t(mcp): 
dd SSE event types 
nd reqwest-eventsource dependency- fe
t(memory): implement CLI list 
nd show comm
nds- fe
t(memory): implement AuditLogger for oper
tion tr
cking- fe
t(mcp): 
dd S
mpling RPC types for P2 server-initi
ted LLM c
lls- fe
t(memory): 
dd 
udit log schem
 
nd types- fe
t(memory): 
dd CLI module with file locking- fe
t(memory): implement Archiv
lService for scr
tchp
d 
rchiving- fe
t(memory): implement HybridTrigger with token threshold s
fety net- fe
t(memory): implement L
zyDec
yEngine for re
d-time dec
y ev
lu
tion- fe
t(memory): 
dd type-
w
re dec
y c
lcul
tion with tempor
l scope- fe
t(memory): 
dd dec
y_inv
lid
ted_
t field for recycle bin- fe
t(memory): complete Milestone 1 - Scr
tchp
d Found
tion- fe
t(memory): implement Scr
tchp
dM
n
ger with CRUD oper
tions- fe
t(memory): implement SessionHistory for scr
tchp
d 
rchiv
l- fe
t(memory): 
dd scr
tchp
d module structure 
nd templ
te- fe
t(mcp): implement re
l McpResourceM
n
ger 
nd McpPromptM
n
ger- fe
t(tools): 
dd mcp_get_prompt builtin tool- fe
t(tools): 
dd mcp_re
d_resource builtin tool- fe
t(mcp): implement re
l 
ggreg
tion for resources 
nd prompts- fe
t(mcp): 
dd resources 
nd prompts methods to McpClient- fe
t(mcp): 
dd resources 
nd prompts support to McpServerConnection- fe
t(mcp): 
dd Resources 
nd Prompts RPC types- fe
t(mcp): 
dd he
lth check logic for servers- fe
t(g
tew
y): wire MCP h
ndlers to McpM
n
gerH
ndle- fe
t(mcp): implement McpM
n
gerActor core loop- fe
t(mcp): 
dd config persistence for McpM
n
ger- fe
t(mcp): 
dd McpM
n
gerH
ndle public API- fe
t(mcp): 
dd McpComm
nd 
nd McpM
n
gerEvent types- fe
t(cortex): implement DecisionConfig with session override- fe
t(cortex): implement security rules (t
g injection, PII m
sking, instruction override)- fe
t(cortex): 
dd S
nitizerRule tr
it 
nd SecurityPipeline- fe
t(cortex): 
dd greedy JSON rep
ir logic- fe
t(cortex): implement JsonStre
mDetector st
te m
chine- fe
t(cortex): 
dd module skeleton with unified error types- fe
t(extension): 
dd PluginHttpH
ndler for plugin REST routes- fe
t(extension): 
dd PluginProviderAd
pter for plugin AI providers- fe
t(extension): 
dd Ch
nnelM
n
ger skeleton for plugin ch
nnels- fe
t(extension): 
dd HTTP route types- fe
t(extension): 
dd provider plugin types- fe
t(extension): 
dd ch
nnel plugin types- fe
t(g
tew
y): 
dd service lifecycle RPC h
ndlers- fe
t(extension): integr
te ServiceM
n
ger with ExtensionM
n
ger- fe
t(extension): 
dd ServiceM
n
ger for b
ckground services- fe
t(extension): 
dd service lifecycle types- fe
t(g
tew
y): 
dd plugins.executeComm
nd RPC h
ndler- fe
t(extension): 
dd comm
nd execution to PluginLo
der- fe
t(extension): 
dd DirectComm
ndResult type- fe
t(extension): implement scope-
w
re skill injection- fe
t(extension): implement V2 prompt lo
ding with scope support- fe
t(extension): 
dd scope 
nd bound_tool to ExtensionSkill- fe
t(extension): 
dd PromptScope enum for V2 skill injection- fe
t(extension): 
dd V2 hook conversion from TOML m
nifest- fe
t(extension): implement typed hook execution (interceptor/observer/resolver)- fe
t(extension): 
dd kind 
nd priority to HookConfig- fe
t(extension): 
dd HookKind 
nd HookPriority enums- fe
t(extension): integr
te TOML p
rser with 
uto-detection (TOML > JSON)- fe
t(extension): 
dd V2 fields to PluginM
nifest- fe
t(extension): 
dd TOML m
nifest p
rser types- fe
t(exec): check skill_
llowlist in 
pprov
l decision- fe
t(exec): 
dd skill_
llowlist config option- fe
t(exec): extend ExecContext with skill origin info- fe
t(skills): implement CLI Wr
pper v
lid
tor- fe
t(skills): 
dd he
lth checking methods to SkillsRegistry- fe
t(skills): 
dd inst
ll suggestion methods to SkillsInst
ller- fe
t(skills): implement He
lthChecker for dependency v
lid
tion- fe
t(skills): extend SkillFrontm
tter with requirements 
nd met
d
t
- fe
t(skills): 
dd types for requirements 
nd he
lth checking- fe
t(poe): repl
ce Pl
ceholderWorker with re
l AgentLoopWorker- fe
t(g
tew
y): wire POE contr
ct signing to G
tew
y- fe
t(poe): implement contr
ct signing workflow for first principles closure- fe
t(core): 
dd sn
pshot c
pture tool 
nd registry upd
tes- fe
t(config): 
dd memory configur
tion types 
nd v
lid
tion- fe
t(memory): enh
nce retriev
l 
nd 
dd dre
ming module- fe
t(m
cos): 
dd tool emoji form
tting to H
loStre
mingView- fe
t(m
cos): upd
te G
tew
yStre
mAd
pter with enh
nced summ
ry- fe
t(m
cos): 
dd H
loResultViewV2 with det
il popover support- fe
t(m
cos): 
dd H
loResultDet
ilPopover for det
iled results- fe
t(m
cos): 
dd Enh
ncedRunSumm
ry 
nd ToolSumm
ryItem models- fe
t(g
tew
y): 
dd Enh
ncedRunSumm
ry 
nd per-runId sequences- fe
t(g
tew
y): 
dd mess
ge deduplic
tion with text norm
liz
tion- fe
t(g
tew
y): 
dd stre
m buffer for block-level text flushing- fe
t(g
tew
y): 
dd tool displ
y module with emoji 
nd sm
rt form
tting- fe
t(h
lo): integr
te comm
ndList st
te into H
loViewV2- fe
t(h
lo): 
dd H
loComm
ndListView for / comm
nd p
nel- fe
t(h
lo): 
dd Comm
ndItem 
nd Comm
ndListContext types for / comm
nd- fe
t(h
lo): 
dd H
loInputCoordin
tor for lightweight input h
ndling- fe
t(g
tew
y): 
dd 150ms throttling for response chunks- fe
t(h
lo): 
dd H
loViewV2 m
in component integr
ting 
ll st
te views- fe
t(h
lo): 
dd H
loHistoryListView for convers
tion history- fe
t(h
lo): 
dd H
loResultView for comp
ct result displ
y- fe
t(h
lo): 
dd H
loStre
mingView for unified stre
ming displ
y- fe
t(h
lo): 
dd H
loSt
teV2 with 6 simplified st
tes- fe
t(h
lo): 
dd new stre
ming types for simplified st
te model- fe
t(skill-evolution): implement Skill Compiler (Ph
se 10)- fe
t(
gent-loop): 
dd on_user_question method to LoopC
llb
ck- fe
t(
gent-loop): 
dd AskUserRich decision v
ri
nt with QuestionKind- fe
t(
gent-loop): export question 
nd 
nswer modules- fe
t(
gent-loop): 
dd UserAnswer type for structured responses- fe
t(
gent-loop): 
dd QuestionKind types for structured user inter
ction- fe
t(resilient): 
dd cron integr
tion with Podc
stT
sk ex
mple- fe
t(resilient): implement ResilientExecutor with retry 
nd f
llb
ck- fe
t(resilient): define ResilientT
sk tr
it- fe
t(resilient): 
dd core types for resilient t
sk execution- fe
t(skill_evolution): implement GitCommitter for 
uto-commit- fe
t(skill_evolution): implement SkillGener
tor for SKILL.md cre
tion- fe
t(skill_evolution): implement Solidific
tionDetector for p
ttern detection- fe
t(skill_evolution): implement EvolutionTr
cker for execution logging- fe
t(skill_evolution): 
dd core types for skill evolution system- fe
t(spec_driven): implement SpecDrivenWorkflow orchestr
tor- fe
t(spec_driven): implement LlmJudge for ev
lu
tion- fe
t(spec_driven): implement TestWriter for test gener
tion- fe
t(spec_driven): implement SpecWriter for requirement 
n
lysis- fe
t(spec_driven): 
dd core types for spec-driven workflow- fe
t(g
tew
y): 
dd exec.c
llb
ck.h
ndle RPC for 
pprov
l c
llb
cks- fe
t(telegr
m): 
dd edit_mess
ge method for 
pprov
l upd
tes- fe
t(g
tew
y): 
dd 
pprov
l bridge h
ndler utilities- fe
t(exec): 
dd Approv
lBridge for ch
nnel integr
tion- fe
t(telegr
m): 
dd c
llb
ck query h
ndling- fe
t(telegr
m): 
dd inline keybo
rd support### Fixed- fix: 
dd tool_c
ll_id to OpenAI tool result mess
ges- fix: unignore CHANGELOG.md, fix rele
se recipe git 
dd- fix: remove unused imports 
cross codeb
se (c
rgo fix)- fix: resolve 42 test w
rnings — deprec
ted API, unused imports, de
d code- fix: sl
sh comm
nd f
st-p
th + CLI 
rg p
rser + E2E tests- fix: en
ble sl
sh comm
nd f
st-p
th for WebCh
t ch
t.send- fix: repl
ce env!("HOME") with dirs::home_dir() for Windows comp
tibility- fix: correct PluginKind::Mcp m
pping 
nd remove debug output- fix: upd
te discovery to find CC-form
t plugins in inst
lled/ directory- fix: ch
nnel binding not repl
cing old peer_id rows- fix: ch
nnel st
tus showing disconnected 
fter p
ge refresh- fix: p
ss session_m
n
ger to BuiltinToolConfig for session tools- fix: resolve 
gent from session_key inste
d of Worksp
ceM
n
ger- fix: sep
r
te 
gent identity files from worksp
ce directory- fix: use bold *n
me* for 
gent prefix inste
d of [n
me]- fix: use M
rkdown (leg
cy) inste
d of M
rkdownV2 for Telegr
m mess
ges- fix: remove b
cksl
sh esc
ping from 
gent n
me prefix in replies- fix: override rel
tive working_dir with 
gent worksp
ce- fix: ch
nge def
ult worksp
ce root from 
gents/ to worksp
ces/- fix: def
ult b
sh/code_exec working directory to 
gent worksp
ce- fix: register JSON Schem
 for 
ll builtin tools + Codex protocol 
lignment- fix: prevent token regener
tion on HMAC mism
tch to protect v
ult secrets- fix: Codex SSE function_c
ll_
rguments delt
 collection + logging- fix: use v
ult_key() function inste
d of undefined VAULT_KEY const
nt- fix: unify rer
nking v
ult key form
t with other modules- fix: rer
nking P
nel fetches per-provider API key from v
ult- fix: cle
r 
pi_key from rer
nking config sign
l 
fter s
ve- fix: isol
te rer
nk API keys per provider in v
ult- fix: move rer
nk API key from config.toml to encrypted v
ult- fix: correct def
ult rer
nking model n
me in P
nel 
nd tests- fix: ACP p
nel buttons h
ng due to sp
wn_loc
l context loss- fix: ACP test/s
ve button h
ng 
nd preset mode def
ults- fix: ACP p
nel gemini preset ID mism
tch 
nd test button h
ng- fix: resolve 
ll 75 compil
tion errors from provider routing ref
ctor- fix: v
ult-b
cked provider API keys 
nd config h
ndler improvements- fix(
cp): 
d
pt h
rnesses to re
l CLI protocols 
fter e2e probe testing- fix: worksp
ce schem
 migr
tion, worksp
ce.getActive response, 
nd providers p
ge freeze- fix: remove redund
nt binding in ConfigP
tcher- fix: session history, 
gent.list RPC, 
nd embedding dedup- fix: count only running runs for concurrency limit, reduce cle
nup del
y- fix: 
dd multi-dimension vector columns to memories t
ble schem
- fix: hot-sw
p runtime provider when switching def
ult vi
 P
nel UI- fix: resolve ch
t qu
lity issues — bootstr
p, esc
l
tion, 
nd response form
t- fix: resolve pre-existing test compil
tion errors- fix: wire missing RPC h
ndlers 
nd correct TUI method n
mes- fix: upd
te rem
ining port 18789 references to 18790- fix: unify ch
nnel config persistence — P
nel UI s
ve/lo
d/connect now works- fix: resolve compil
tion errors from fe
ture fl
g remov
l- fix(desktop): 
ddress fin
l review — version 
lignment, input v
lid
tion, Unicode- fix(desktop): 
ddress clippy needless-borrow w
rning in 
gent h
ndler- fix(desktop): 
ddress code qu
lity review — v
lid
tion, 
pprov
l g
tes- fix(desktop): wire N
tiveDesktop into registry + complete re-exports- fix: logic review R2 
rchitecture — 14 findings 
cross 5 c
tegories- fix: logic review R2 — 29 files 
cross 4 priority b
tches- fix: 
ddress code review findings for self-configur
tion- fix: RAII sem
phore gu
rd 
nd env v
r exp
nsion ordering (Known Issues)- fix: repl
ce std::sync::RwLock with cr
te::sync_primitives (P2-15)- fix: sort H
shM
p-derived collections for deterministic ordering (P2-14)- fix: repl
ce SystemTime UNIX_EPOCH .unwr
p() with .unwr
p_or_def
ult() (P2-12)- fix: rele
se locks before 
w
iting in 4 
sync p
tterns (P2-11)- fix: norm
lize t
sk_type 
nd t
sk_id in SessionKey::t
sk() (P1-9)- fix: use bounded c
st for POE token count u32 conversion (P1-8)- fix: resolve rem
ining UTF-8 byte slicing p
nics (P1-7)- fix: ConfigP
tcher use s
ve_increment
l 
nd h
rd-error on conflict- fix: logic review Ph
se 6 — 45 fixes 
cross g
tew
y, memory, poe, exec, providers, 
nd 15 more modules- fix: resolve 5 rem
ining W
rning-level issues from logic review Ph
se 5- fix: logic review Ph
se 4 — 18 fixes 
cross d
emon, engine, secrets, skills, components, cron- fix: resolve 5 Known Issues from logic review- fix: comprehensive logic review fixes 
cross 53 files in 77 modules- fix: use cfg(fe
ture = "loom") inste
d of cfg(loom) to 
void poisoning dependencies- fix(g
tew
y): elimin
te TOCTOU in execution_engine concurrent run limit check- fix(g
tew
y): use Mutex for ch
nnel_registry t
ke-once inbound_rx p
ttern- fix(resilience): simplify governor session_tokens from AtomicU64 to u64- fix: upd
te doctest to use poe::met
_cognition::Beh
vior
lAnchor- fix: 
dd Clone derive to NoiseFilter 
nd remove duplic
te mod decl
r
tions- fix: remove duplic
te scoring_pipeline module decl
r
tion in memory/mod.rs- fix(clippy): resolve print_liter
l w
rnings in secret providers comm
nd- fix(tests): migr
te secret_bound
ry_integr
tion tests to 
sync- fix(runtimes): 
ddress critic
l 
nd import
nt code review findings- fix: resolve 
ll clippy w
rnings in 
leph-t
uri 
nd 
lephcore- fix(desktop): use ERR_NOT_IMPLEMENTED for stubbed methods, 
dd debug logging- fix(h
lo): 
ddress code review findings for view 
nd events- fix(h
lo): gu
rd 
g
inst empty run_id in event h
ndler- fix(h
lo): use monotonic counter for unique mess
ge IDs, remove redund
nt ph
se gu
rd- fix(desktop): restrict UDS socket to owner-only 
ccess- fix(desktop): 
dd 30s timeout to UDS request to prevent indefinite t
sk h
ng- fix(desktop): log ev
lu
teJ
v
Script errors in C
nv
s, 
dd runAsync m
in-thre
d 
ssert- fix(desktop): repl
ce deprec
ted 
ctiv
te(options:) with 
ctiv
te() for m
cOS 15- fix(desktop): 
void PNG round-trip in OCR p
th by sh
ring c
ptureCurrentScreen- fix: 
ddress code review findings- fix(desktop): repl
ce strcpy with strncpy to prevent buffer overflow- fix(desktop): require x/y for click 
nd window_id for focus_window- fix(desktop): remove misle
ding serde t
gs from DesktopRequest, 
dd From conversions- fix(skill): 
ddress code review findings- fix(skill): resolve clippy w
rnings in skill module- fix(skill): use single colon sep
r
tor for SkillId (m
tches OpenCl
w convention)- fix(st
rt): 
dd cfg gu
rd for builder mod, tighten h
ndler visibility to pub(in cr
te::comm
nds::st
rt)- fix(st
rt): move session b
nner print into register_session_h
ndlers for consistency- fix: resolve 
ll compil
tion errors from server purific
tion- fix: cle
n up rem
ining Server-Client terminology in source comments- fix: rep
ir 2 broken doc-tests in skill_evolution module- fix: resolve 8 pre-existing test f
ilures- fix(control-pl
ne): document AlertsApi integr
tion limit
tion- fix(control-pl
ne): complete mock d
t
 remov
l- fix(control-pl
ne): fix memory le
ks 
nd improve error h
ndling in 
lert subscriptions- fix(sh
red-ui-logic): improve error h
ndling in 
lerts API- fix(control-pl
ne): use T
ilwind CDN for CSS compil
tion- fix(control-pl
ne): 
dd WASM initi
liz
tion in lib.rs- fix(control-pl
ne): upd
te st
rtup log mess
ge to show correct URL- fix(control-pl
ne): fix root p
th 
ccess 
nd st
tic 
sset lo
ding- fix: resolve compil
tion errors 
nd 
dd missing imports- fix(d
shbo
rd): 
dd w
sm_bindgen entry point to en
ble 
pp initi
liz
tion- fix(g
tew
y): extr
ct guest_session_id when require_
uth=f
lse- fix: resolve compil
tion errors in 
uth 
nd guest h
ndlers- fix: use rowid inste
d of id for sqlite-vec virtu
l t
ble upd
tes- fix(ph
se2): fix RPC tests 
nd upd
te progress report- fix(cli): use correct method n
mes for session comm
nds- fix(cli): resolve event stre
ming issue between g
tew
y 
nd CLI- fix(cli): 
lign comm
nd h
ndlers with g
tew
y API- fix(memory): h
ndle new SubAgent F
ctType v
ri
nts in consolid
tion- fix: resolve f
iling BDD tests for embodiment 
nd CoT tr
nsp
rency- fix: resolve f
iling unit tests- fix: resolve module export 
nd test compil
tion errors- fix: resolve 
ll 29 compiler w
rnings- fix: 
dd dylib.* p
ttern to gitignore- fix: upd
te .gitignore for Aleph ren
me 
nd remove dylib from tr
cking- fix(compressor): fix string conc
ten
tion in tests- fix(protocols): error on nonexistent JSONP
th inste
d of returning null- fix(scr
tchp
d): use EAFP p
ttern inste
d of sync exists() checks- fix(scr
tchp
d): remove 
sync from exists() 
nd export Scr
tchp
dConfig- fix(core): fix form
t strings in m
nifest.rs 
nd doctest in pty.rs- fix: cle
n up rem
ining MultiTurnCoordin
tor references- fix(g
tew
y): remove MultiTurnCoordin
tor dependency from 
d
pter- fix(h
lo): upd
te DependencyCont
iner comment for H
loInputCoordin
tor- fix(h
lo): upd
te AppDeleg
te to use H
loInputCoordin
tor- fix(h
lo): upd
te HotkeyService to use H
loInputCoordin
tor- fix: upd
te tests for 5 builtin tools 
nd skill evolution- fix: compil
tion errors in skill evolution 
nd perception modules- fix: resolve test compil
tion errors### Ch
nged- ref
ctor: ren
me ch
tgpt → codex protocol 
cross codeb
se- ref
ctor: ren
me ToolGroup → ToolC
tegory to 
void confusion with Te
m- ph
se4: cle
n 
ll T
uri references from codeb
se- ph
se4: remove T
uri, 
rchive old 
pps, move Swift bridge to cr
tes/desktop-m
cos/bridge- ref
ctor: move CLI/TUI/WebCh
t to interf
ces/, client to sh
red/- cle
nup: remove bootstr
p 
uto-clone 
nd leg
cy plugin index code- cle
nup: remove AgentLifecycleEvent::Switched 
nd AgentRouter from inbound router- cle
nup: remove 
gent switching (tool, intent detector, /switch comm
nd)- cle
nup: remove unregistered self-m
n
gement tool source files- cle
nup: remove old sub
gent tools (sp
wn/steer/kill + deleg
te)- cle
nup: move e2e tests into tests/, remove unused sh
red_ui_logic cr
te, 
dd secret sc
nning exclusion- cle
nup: remove tempor
ry debug logging for ch
tgpt protocol- ref
ctor: ren
me worksp
ce to 
gent 
cross memory/config/p
ths, enh
nce 
gent loop 
nd Ch
tGPT protocol- cle
nup: remove zombie code, upd
te def
ult config 
nd sh
red_ui_logic- cle
nup: remove st
le ALEPH_MASTER_KEY references from docs 
nd error mess
ges- ref
ctor: fl
tten 
gent_loop/ — remove minim
l/ subdirectory- cle
nup: remove deprec
ted APIs (register_
gent_tools, with_working_dir, ToolC
tegory::N
tive, PolicyEngine stubs, AuditStore, Inv
lid
teOld)- ref
ctor: ren
me Minim
l* types to st
nd
rd n
mes — this IS the loop- cle
nup: fix clippy w
rning in leg
cy_
d
pter detect_entry_point- cle
nup: elimin
te 
ll clippy w
rnings (58→0)- cle
nup: fix clippy w
rnings (derive Def
ult, redund
nt closures, simplified condition
ls)- cle
nup: remove st
le 
pp_bundle_id references from comments 
nd BDD tests- cle
nup: remove TypeScript webch
t (repl
ced by P
nel /ch
t route)- cle
nup: remove de
d Sub
gentAuthority 
nd tools/sessions dom
in l
yer- ref
ctor: simplify memory types, use floor_ch
r_bound
ry, 
dd mtime c
che to d
ily memory- ref
ctor(pdf): split pdf_gener
te.rs into module directory- ref
ctor: strip #[cfg(fe
ture)] from g
tew
y, server, extension, 
nd misc modules- ref
ctor: strip #[cfg(fe
ture)] from 
ll 12 ch
nnel implement
tions- ref
ctor: strip 20+ C
rgo fe
ture fl
gs from core cr
te- ref
ctor: Occ
m's R
zor p
ss — elimin
te clippy w
rnings 
nd de
d code- cle
nup: remove f
stembed 
nd loc
l embedding model remn
nts- cle
nup: fix unused import in host_functions.rs- ref
ctor(w
sm): simplify PermissionChecker to f
c
de over W
smC
p
bilities- cle
nup: bro
d DRY ref
ctoring 
nd clippy compli
nce 
cross codeb
se- cle
nup: remove st
le f
stembed references, fix integr
tion tests- cle
nup: remove m
cOS-specific CI workflow 
nd build scripts (C8-C12)- cle
nup: remove deprec
ted m
cOS Swift 
pp (C7)- cle
nup: remove UniFFI Swift bindings (C1-C2)- ref
ctor(core): introduce register_h
ndler! m
cro, elimin
te h
ndler boilerpl
te (W
ve 4)- ref
ctor(core): repl
ce &Vec<T> with &[T] in 
rrow_convert 
nd sh
dow_repl
y (W
ve 3B)- ref
ctor(core): convert Intern
lEventH
ndler String p
r
ms to &str (W
ve 3A)- ref
ctor(core): m
nu
l Clippy fixes — expect_fun_c
ll, useless_vec, ptr_
rg, type_complexity, module_inception, needless_borrows, 
nd more (W
ve 2B)- ref
ctor(core): repl
ce Def
ult::def
ult() field re
ssignment with struct liter
ls (W
ve 2A)- ref
ctor(core): 
uto-fix Clippy w
rnings 
nd remove unused imports (W
ve 1)- ref
ctor(runtimes): delete old runtime m
n
gers, repl
ce with Ledger/Probe system- ref
ctor(video): repl
ce RuntimeRegistry with C
p
bilityLedger in c
ption.rs- ref
ctor(init): repl
ce forced runtime inst
ll
tion with zero-inst
ll ledger- ref
ctor(desktop): delete RPC proxy comm
nds 
nd cle
n up de
d code (~1600 lines)- ref
ctor(h
lo): delete Re
ct frontend source from T
uri 
pp- ref
ctor(h
lo): point T
uri h
lo window to Leptos server URL- ref
ctor(h
lo): delete leg
cy Swift H
lo views 
nd fix references (~4500 lines removed)- ref
ctor(st
rt): split initi
lize_
uth, extr
ct lo
d_
pp_config, restore register c
lls to orchestr
tor- ref
ctor(st
rt): move register_* h
ndler functions to comm
nds/builder/h
ndlers.rs- ref
ctor(extension): thin mod.rs f
c
de, deleg
te lo
d_
ll to ComponentLo
der- ref
ctor(st
rt): extr
ct subsystem initi
lizers from st
rt_server- ref
ctor: remove distributed execution infr
structure (ExecutionPolicy, ClientM
nifest, ReverseRpc, ToolRouter, RoutedExecutor)- ref
ctor: cle
n up 
uth h
ndler by removing ClientM
nifest references- ref
ctor: simplify g
tew
y server by removing client routing infr
structure- ref
ctor: simplify ExecutionEngine by removing client routing- ref
ctor: ren
me g
tew
y/ch
nnels/ to g
tew
y/interf
ces/- ref
ctor: ren
me clients/ to 
pps/- cle
nup: remove unused imports from exec_security_g
te (post-reb
se)- cle
nup: fix Arc misuse, l
rge v
ri
nts, 
nd priv
te interf
ces (P
ss 3 fin
l)- cle
nup: extr
ct type 
li
ses 
nd p
r
meter structs (P
ss 3)- cle
nup: suppress module_inception for intention
l nested module p
ttern- cle
nup: fix 22 miscell
neous clippy w
rnings- cle
nup: P
ss 2 loc
l ref
ctoring (clone, strip_prefix, de
d code, redund
nt closures)- cle
nup: fix boole
n simplific
tions, identity ops, 
nd &P
thBuf sign
tures- cle
nup: remove unused imports 
nd repl
ce deriv
ble impls- cle
nup: 
pply c
rgo clippy --fix 
uto-corrections- ref
ctor(control-pl
ne): split Sideb
r into sideb
r/ directory- ref
ctor(control-pl
ne): use nested routes for Settings with SettingsL
yout- ref
ctor(control-pl
ne): remove /cp prefix from routing- ref
ctor(core): ren
me 
leph-g
tew
y to 
leph-server- ref
ctor(m
cos): completely remove settings UI from m
cOS client- ref
ctor(desktop): completely remove settings UI from T
uri client- ref
ctor(desktop): migr
te Plugins, Skills, 
nd Policies settings to D
shbo
rd- ref
ctor(clients): complete Ph
se 4 - remove Gener
tion Providers UI- ref
ctor(clients): migr
te Providers, Memory, 
nd MCP config to D
shbo
rd- ref
ctor(
gent_loop): introduce RunContext p
ttern for cle
ner API- ref
ctor(
gent-loop): 
dd RunContext structure (WIP)- ref
ctor(dom
in): implement Newtype p
ttern for Answer 
nd Ruleset- ref
ctor(dom
in): implement Newtype p
ttern for 5 ID types- ref
ctor(
pi): implement FromStr tr
it for rem
ining types- ref
ctor(
pi): implement FromStr tr
it for extension 
nd resilience types- ref
ctor(
pi): implement FromStr tr
it for memory context types- ref
ctor(perf): repl
ce trim_st
rt_m
tches with strip_prefix for fixed prefixes- ref
ctor(perf): optimize &P
thBuf → &P
th in 6 files- ref
ctor(core): 
dd #[
llow(de
d_code)] to 12 reserved fields- ref
ctor(deps): remove 5 unused dependencies- ref
ctor(core): remove 2 confirmed de
d code items- ref
ctor(core): remove 160+ unused imports 
cross 50 files- ref
ctor(tools): extr
ct builtin tool registr
tion 
nd types (Ph
se 6)- ref
ctor(g
tew
y): modul
rize plugins h
ndlers (Ph
se 5.1)- ref
ctor(poe): extr
ct services to dedic
ted modules (Ph
se 4.2 - P1)- ref
ctor(poe): extr
ct h
ndler types to dedic
ted modules (Ph
se 4.1 - P0)- ref
ctor(browser): extr
ct types 
nd scripts modules (Ph
se 3 - P
rt 1)- ref
ctor(engine): complete 
tomic executor composition ref
ctoring (Ph
se 2)- ref
ctor(engine): 
dd 
tomic module b
se 
rchitecture (Ph
se 2 WIP)- ref
ctor(extension): split types.rs into modul
r structure- ref
ctor(security): tr
nsform PolicyEngine to st
teless- ref
ctor(protocol): 
dd equ
lity derives 
nd helper methods to 
uth types- ref
ctor(ph
se1): reorg
nize client directory structure- ref
ctor: complete fin
l Aether to Aleph cle
nup- ref
ctor: complete Aether to Aleph ren
me - scripts, workflows, 
nd rem
ining code- ref
ctor: complete Aether to Aleph ren
me 
cross entire codeb
se- ref
ctor(providers): use ProtocolRegistry in cre
te_provider f
ctory- ref
ctor(providers): remove technic
l 
li
s presets- ref
ctor(config): remove provider_type field from ProviderConfig- ref
ctor: fix P3 clippy w
rnings - b
tch 2- ref
ctor: fix P3 clippy w
rnings - b
tch 1- ref
ctor: fix P1/P2 clippy w
rnings 
nd improve code qu
lity- ref
ctor(providers): delete leg
cy OpenAiProvider- ref
ctor(providers): delete leg
cy GeminiProvider- ref
ctor(providers): delete leg
cy Cl
udeProvider- ref
ctor(providers): use HttpProvider for Anthropic protocol- ref
ctor(providers): remove redund
nt vendor wr
ppers (~850 lines)- ref
ctor(providers): use HttpProvider for OpenAI protocol in f
ctory- ref
ctor(m
cos): cle
nup 
nd improve hotkey/h
lo components- ref
ctor(h
lo): repl
ce H
loSt
te with simplified 6-st
te version- ref
ctor(h
lo): switch H
loWindow to V2 components- ref
ctor(h
lo): remove MultiTurn references from EventH
ndler- ref
ctor(h
lo): remove MultiTurn directory (~3000 lines)- ref
ctor: split l
rge modules into sm
ller files- cle
nup: remove unused modules 
nd merge thinking into thinker- cle
nup: elimin
te 
ll compil
tion w
rnings- cle
nup(lib): slim down exports from 590 to 272 lines- cle
nup: remove FFI-rel
ted comments- cle
nup: ren
me FFI types to st
nd
rd n
mes- cle
nup(disp
tcher): ren
me ffi.rs to tool_info.rs- cle
nup(intent): remove Type A FFI residu
ls### Build- docs: 
dd voice convers
tion implement
tion pl
n- docs: fix PromptBuilder voice st
te 
ccess p
th in voice spec- docs: upd
te voice convers
tion spec with review fixes- docs: 
dd voice convers
tion system design spec- docs: 
dd rele
se workflow 
nd version m
n
gement to CLAUDE.md- rele
se: v0.2.8- build: unify version source — VERSION file drives 
ll version strings- rele
se: v0.2.8- docs: 
dd multimod
l probe tests implement
tion pl
n- docs: 
dd multimod
l probe tests design spec- docs: 
dd core multimod
l enh
ncement implement
tion pl
n- docs: fix spec review issues in core multimod
l design- docs: 
dd core multimod
l enh
ncement design spec- docs: 
dd Telegr
m ch
nnel enh
ncement implement
tion pl
n- docs: fix spec review issues in Telegr
m enh
ncement design- docs: 
dd Telegr
m ch
nnel enh
ncement design spec- docs: 
dd Feishu enh
nced fe
tures implement
tion pl
n- docs: 
ddress spec review — FeishuEventEmitter, typing lifecycle, c
p
bilities- docs: 
dd Feishu enh
nced fe
tures design spec- docs: 
dd Feishu ch
nnel implement
tion pl
n- docs: 
ddress spec review feedb
ck for Feishu ch
nnel- docs: 
dd Feishu/L
rk ch
nnel design spec- rele
se: v0.2.7 — multi-
gent system, UI upd
tes, bug fixes- docs: fix spec issues from review — st
le fin
l_text, test pl
n, consecutive_errors- docs: 
dd Persistent Completion Protocol design spec- docs: fix multi-
gent modes spec per review findings- docs: 
dd multi-
gent modes t
xonomy design spec- docs: 
dd t
sk coordin
tion implement
tion pl
n (12 t
sks)- docs: fix event type conventions in t
sk coordin
tion spec- docs: 
ddress spec review findings for t
sk coordin
tion- docs: 
dd t
sk coordin
tion system design spec- build: upd
te WASM p
nel dist- ci: upgr
de GitHub Actions to Node.js 24 comp
tible versions- ci: scope fmt check to m
int
ined cr
tes (skip leg
cy form
tting issues)- build: consolid
te to single rele
se workflow, fix CI protoc dependency- build: remove 
rchive from git (l
rge bin
ries exceed GitHub limit)- rele
se: bump version to 0.2.6- build: upd
te inst
ll scripts for 
leph-server bin
ry n
me- build: ren
me workflows, fix --bin 
leph→
leph-server, 
dd pl
tform rele
se workflows- build: upd
te justfile 
nd CI workflows for post-T
uri 
rchitecture- build: 
dd swift-bridge recipe to justfile for m
cOS n
tive APIs- docs: 
dd Ph
se 3 implement
tion pl
n for m
cOS PIM & system c
p
bilities- docs: 
dd Ph
se 2 implement
tion pl
n for screen control n
tive migr
tion- docs: 
ddress spec review feedb
ck for hier
rchic
l comm
nds- docs: 
dd hier
rchic
l sl
sh comm
nds design spec- docs: 
dd Ph
se 1 implement
tion pl
n for desktop n
tive c
p
bilities- docs: 
dd desktop n
tive c
p
bilities design spec- docs: upd
te design spec with new directory structure- docs: 
dd implement
tion pl
n for intermedi
te mess
ge delivery- docs: 
dd PLUGIN_SYSTEM.md — CC-comp
tible plugin 
rchitecture reference- docs: 
ddress spec review feedb
ck for CLI/TUI sep
r
tion- docs: 
dd CLI/TUI sep
r
tion design spec- docs: 
dd P4 runtime migr
tion implement
tion pl
n- docs: 
dd prompt guid
nce 
s in-scope ch
nges to intermedi
te mess
ge spec- docs: 
dd edge c
ses to intermedi
te mess
ge delivery spec- docs: 
dd intermedi
te mess
ge delivery design spec- docs: 
dd P3 scope m
n
gement implement
tion pl
n- docs: 
dd P2 m
rketpl
ce system implement
tion pl
n- docs: 
dd P0+P1 implement
tion pl
n for plugin CC comp
t- docs: fix rem
ining spec review items (round 2)- docs: 
ddress spec review findings for plugin comp
t design- docs: 
dd plugin system Cl
ude Code comp
tibility redesign spec- docs: upd
te spec 
nd pl
n — keep peer_id sign
tures unch
nged- docs: upd
te 
gent-bot 1:1 binding spec with review fixes- docs: 
dd 
gent-bot 1:1 binding simplific
tion design spec- docs: 
dd ch
t sideb
r redesign spec 
nd implement
tion pl
n- docs: 
dd p
nel 
gent routing fix design spec- docs: 
dd worksp
ce output migr
tion implement
tion pl
n- docs: revise worksp
ce output migr
tion spec 
fter review- docs: 
dd worksp
ce output migr
tion design spec- docs: 
dd gener
tion providers wiring implement
tion pl
n- docs: fix gener
tion providers spec 
fter review- docs: 
dd gener
tion providers wiring design spec- docs: 
dd Cl
wHub integr
tion implement
tion pl
n- docs: 
ddress spec review feedb
ck for Cl
wHub integr
tion- docs: 
dd Cl
wHub integr
tion design spec- ci: upgr
de GitHub Actions to Node.js 24, fix Windows de
d-code w
rnings- docs: fix pl
n review issues (3 blockers + 6 w
rnings)- docs: 
ddress spec review feedb
ck for Chrome DevTools MCP Mode- docs: 
dd Chrome DevTools MCP Mode design spec- docs: 
dd process m
n
gement rules to CLAUDE.md- docs: 
dd tool permission system implement
tion pl
n- docs: upd
te tool permission spec 
fter review- docs: 
dd tool permission system design spec- docs: 
dd ACP probe tests design document- docs: 
dd ACP h
rness m
n
gement implement
tion pl
n- docs: 
dd ACP h
rness m
n
gement design document- docs: 
dd provider routing ref
ctor implement
tion pl
n- docs: fix rem
ining spec review issues- docs: fix spec issues from review- docs: 
dd provider routing ref
ctor design spec- docs: 
dd provider config testing implement
tion pl
n- docs: upd
te provider config testing spec 
fter review- docs: 
dd provider config testing design spec- docs: 
dd simplify-model-config implement
tion pl
n- docs: upd
te simplify-model-config spec 
fter review- docs: 
dd simplify-model-config design spec- ci: re
d rele
se version from VERSION file inste
d of m
nu
l input- docs: 
dd cron probe tests implement
tion pl
n- docs: 
dd cron probe tests design spec- docs: 
dd cron module redesign implement
tion pl
n- docs: 
dd cron module redesign spec- build: rebuild p
nel WASM 
nd upd
te docs 
fter worktree merges- docs: 
dd provider zero-config implement
tion pl
n- docs: 
dd mess
ge pipeline implement
tion pl
n- docs: 
dd provider zero-config UX design spec- docs: 
dd mess
ge pipeline design for g
tew
y pre-processing- docs: 
dd model discovery probe tests implement
tion pl
n- docs: 
dd model discovery probe tests design spec- docs: 
dd model discovery implement
tion pl
n- docs: fix model discovery spec issues from review- docs: 
dd model discovery design spec- docs: 
dd cognitive evolution bet
 implement
tion pl
n- docs: 
dd cognitive evolution bet
 design (immune-complete loop)- docs: 
dd POE Ph
se 2+3 implement
tion pl
n- docs: 
dd POE Ph
se 1 implement
tion pl
n (Bl
stR
dius + T
boo)- docs: 
dd POE Architecture Evolution Whitep
per 2026- ci: fix Linux/Windows compil
tion errors for missing imports- docs: upd
te extension system 
rchitecture document
tion- docs: 
dd unified plugin system implement
tion pl
n- docs: 
dd unified plugin system design- docs: 
dd one-line inst
ll comm
nds 
s prim
ry inst
ll
tion method- docs: remove ref
ctoring b
ckstory from intent section- docs: upd
te intent detection section to reflect unified LLM pipeline- docs: 
dd det
iled Aleph vs OpenCl
w comp
rison- docs: 
dd P4.3 core plugins implement
tion pl
n- docs: 
dd plugin development guide- docs: 
dd P4 plugin ecosystem implement
tion pl
n- ci: 
dd Windows x86_64 build t
rget 
nd PowerShell inst
ller- docs: 
dd P3 medi
 pipeline implement
tion pl
n- ci: fix Linux w
rn import, remove d
rwin-x86_64 t
rget- ci: 
dd libxdo-dev for Linux, fix d
rwin x86_64 AVX-512 link error- ci: fix Linux pipewire comp
t (ubuntu-24.04) 
nd m
cOS x86_64 openssl- ci: 
dd libegl 
nd X11 extension deps for Linux build- ci: use m
cos-l
test for x86_64 cross-compile (m
cos-13 EOL)- ci: 
dd dbus, drm, gbm deps for Linux build- ci: 
dd pipewire 
nd cl
ng deps for Linux xc
p build- ci: 
dd libw
yl
nd-dev to Linux build dependencies- docs: 
dd 
uthor note to README- docs: ren
me p
nel screenshots with consistent numbering- docs: restore d
shbo
rd screenshot, keep 
ll 3 p
nel im
ges- docs: upd
te README screenshots with P
nel ch
t 
nd settings views- build: remove webch
t recipes from justfile- docs: 
dd webch
t Rust rewrite implement
tion pl
n- docs: 
dd webch
t Rust rewrite design- docs: remove 
cknowledgments section from README- ci: en
ble 
ll pl
tform build t
rgets for server rele
se- ci: 
dd m
nu
l server rele
se workflow 
nd improve inst
ll script- docs: overh
ul README.md, CLAUDE.md 
nd 
dd LICENSE- docs: 
dd inline directives 
nd leg
cy cle
nup implement
tion pl
n- docs: 
dd inline directives 
nd leg
cy cle
nup design- docs: 
dd l
ngu
ge-
gnostic intent detection implement
tion pl
n- docs: 
dd l
ngu
ge-
gnostic intent detection design- docs: upd
te cle
nup pl
n with execution results- docs: cl
rify cle
nup str
tegy — scoped responsibility, not f
llb
ck- docs: 
dd multi-
gent code redund
ncy cle
nup pl
n- docs: 
dd A2A protocol implement
tion pl
n- docs: 
dd A2A protocol design document- docs: 
dd per-
gent tool configur
tion implement
tion pl
n- docs: 
dd per-
gent tool configur
tion design- docs: 
dd multi-bot P
nel UI implement
tion pl
n- docs: 
dd multi-bot P
nel UI design- docs: 
dd multi-bot ch
nnel implement
tion pl
n- docs: 
dd multi-bot ch
nnel support design- docs: 
dd memory 
lignment design for du
l-directory 
rchitecture- docs: 
dd 
gent-worksp
ce sep
r
tion implement
tion pl
n- docs: 
dd 
gent-worksp
ce sep
r
tion design- docs: 
dd 
gent m
n
gement p
nel implement
tion pl
n- docs: 
dd 
gent m
n
gement p
nel design- docs: 
dd webch
t restructure implement
tion pl
n- docs: 
dd webch
t restructure design- docs: 
dd 
gent switching enh
ncement implement
tion pl
n- docs: 
dd 
gent switching enh
ncement design- docs: 
dd unified comm
nd registry implement
tion pl
n- docs: 
dd unified comm
nd registry design- docs: 
dd dyn
mic 
gent switching implement
tion pl
n- docs: 
dd dyn
mic 
gent switching design- docs: 
dd system prompt optimiz
tion implement
tion pl
n- docs: 
dd system prompt 
rchitecture optimiz
tion design- docs: 
dd Agent/Worksp
ce/Session unific
tion implement
tion pl
n- docs: 
dd Agent/Worksp
ce/Session rel
tionship design- docs: 
dd t
sk routing decision l
yer implement
tion pl
n- docs: 
dd t
sk routing decision l
yer design- docs: 
dd 
rchitecture 
ctiv
tion di
gnostic report- docs: 
dd 
rchitecture 
ctiv
tion di
gnostic implement
tion pl
n- docs: 
dd 
rchitecture 
ctiv
tion di
gnostic design- docs: 
dd n
tive tool_use implement
tion pl
n (9 t
sks)- docs: 
dd n
tive tool_use migr
tion design- docs: 
dd PDF du
l-engine implement
tion pl
n- docs: 
dd PDF du
l-engine rendering design- docs: 
dd cron 
nd group ch
t b
ckend implement
tion pl
n- docs: 
dd cron 
nd group ch
t b
ckend implement
tion design- docs: 
dd scheduled t
sks p
nel implement
tion pl
n- docs: 
dd scheduled t
sks p
nel design- docs: 
dd CLI full RPC cover
ge implement
tion pl
n- docs: 
dd CLI full RPC cover
ge design- docs: 
dd CLI bugfix 
nd JSON unific
tion design- docs: 
dd CLI full comm
nds implement
tion pl
n- docs: 
dd CLI full comm
nds design- docs: 
dd CLI infr
structure enh
ncement implement
tion pl
n- docs: 
dd CLI infr
structure enh
ncement design- docs: 
dd lifecycle observ
bility logging implement
tion pl
n- docs: 
dd lifecycle observ
bility logging design- docs: 
dd system prompt enh
ncement implement
tion pl
n- docs: 
dd system prompt enh
ncement design- docs: 
dd 
gent system Ph
se 2 full cover
ge implement
tion pl
n- docs: 
dd 
gent system full cover
ge design (Ph
se 2)- docs: 
dd Codex p
nel UI design 
nd implement
tion pl
n- docs: 
dd Codex Responses API implement
tion pl
n- docs: 
dd Codex Responses API protocol 
d
pter design- docs: 
dd g
tew
y enh
ncement implement
tion pl
n (20 t
sks)- docs: 
dd g
tew
y enh
ncement design (OpenCl
w-inspired)- docs: 
dd implement
tion pl
n for 
gent/worksp
ce/binding- docs: 
dd 
gent definition + worksp
ce + binding design- docs: 
dd OpenAI subscription provider implement
tion pl
n- docs: 
dd OpenAI subscription provider design- docs: 
dd L
zy POE Activ
tion design- build: ren
me just server → just build, 
dd just 
ll- docs: upd
te bin
ry n
me 
nd port references 
cross 
ll document
tion- build: en
ble 
xum ws fe
ture for port unific
tion- docs: 
dd port unific
tion implement
tion pl
n- docs: 
dd port unific
tion 
nd bin
ry ren
me design- docs: 
dd ch
nnel infr
structure fix implement
tion pl
n- docs: 
dd ch
nnel infr
structure fix design- docs: upd
te CLAUDE.md for fe
ture fl
g remov
l- build: simplify justfile — remove 
ll --fe
tures fl
gs- docs: 
dd runtime ch
nnel control implement
tion pl
n- docs: 
dd runtime ch
nnel control design — elimin
te fe
ture fl
g fr
gment
tion- docs: 
dd ch
t persistence & memory pipeline implement
tion pl
n- docs: 
dd ch
t persistence & memory pipeline fix design- docs: 
dd full ch
in + sm
rt rec
ll implement
tion pl
n- docs: 
dd full ch
in + sm
rt rec
ll design- docs: 
dd worksp
ce enh
ncements implement
tion pl
n (9 t
sks)- docs: 
dd worksp
ce enh
ncements design (4 fe
tures)- docs: 
dd worksp
ce wiring implement
tion pl
n (11 t
sks)- docs: 
dd worksp
ce wiring design for multi-role person
 system- docs: 
dd config extern
liz
tion implement
tion pl
n- docs: 
dd config extern
liz
tion design for ~/.
leph worksp
ce- ci: keep only m
cOS ARM64 build, document other pl
tform blockers- ci: fix rem
ining build issues 
cross pl
tforms- ci: fix cross-pl
tform build issues- ci: pin w
sm-bindgen-cli to 0.2.108 m
tching C
rgo.lock- ci: 
llow test job to f
il without blocking builds- ci: 
dd X11/xscrns
ver dev libr
ries for Linux builds- ci: inst
ll protoc for l
nce-encoding build dependency- ci: improve rele
se workflow with WASM build, test job, 
nd cross-pl
tform desktop- build: rewrite justfile for desktop-
s-muscle 
rchitecture- docs: 
dd cr
tes/desktop to project structure 
nd build comm
nds- docs: 
dd Desktop-
s-Muscle implement
tion pl
n- docs: 
dd Desktop-
s-Muscle 
rchitecture design- docs: 
dd self-configur
tion implement
tion pl
n- docs: 
dd self-configur
tion design document- ci: 
dd loom concurrency test job 
nd incre
se proptest cover
ge- build: 
dd test-proptest, test-loom, test-logic just recipes- docs: 
dd logic review system implement
tion pl
n (15 t
sks, 49 properties)- docs: 
dd logic review system design (three-l
yer defense 
rchitecture)- docs: move obsolete embedding/sqlite-vec pl
ns to leg
cy- docs: upd
te memory system docs to reflect remote embedding migr
tion- build: repl
ce trunk with m
nu
l WASM pipeline in justfile- docs: fix m
cOS Resources p
th in build pipeline design- build: 
dd justfile for unified build pipeline- docs: 
dd unified build pipeline design- docs: 
dd ch
nnel config p
nel implement
tion pl
n- docs: 
dd ch
nnel config p
nel design document- docs: 
dd POE full evolution implement
tion pl
n (19 t
sks, 4 ph
ses)- docs: 
dd POE full evolution design (event-driven closed loop)- docs: 
dd WASM c
p
bility kernel implement
tion pl
n- docs: 
dd WASM c
p
bility kernel design- docs: 
dd m
cOS PIM n
tive API implement
tion pl
n- docs: 
dd m
cOS PIM n
tive API integr
tion design- docs: 
dd POE cognitive hub implement
tion pl
n- docs: 
dd POE cognitive hub upgr
de design- docs: 
dd soci
l bot ch
nnels exp
nsion implement
tion pl
n- docs: 
dd soci
l bot ch
nnels exp
nsion design- docs: 
dd surgic
l DRY ref
ctoring implement
tion pl
n- docs: 
dd surgic
l DRY ref
ctoring design for embedding provider files- docs: 
dd embedding provider LLM migr
tion implement
tion pl
n- docs: 
dd embedding provider LLM migr
tion design- docs: 
dd l
rge file ref
ctoring implement
tion pl
n — 6 t
sks, 5 files- docs: 
dd l
rge file ref
ctoring design — 5 files, pure module splitting- ci: 
dd server, m
cOS 
pp, 
nd T
uri rele
se workflows- docs: 
dd distribution implement
tion pl
n (24 t
sks, 9 ph
ses)- docs: 
dd distribution 
rchitecture design- docs: 
dd PromptPipeline implement
tion pl
n — 10 t
sks, TDD, str
ngler fig- docs: 
dd PromptPipeline design — Tr
it-per-L
yer evolution from Pl
n A- docs: 
dd 
utom
tion skills implement
tion pl
n- docs: 
dd 
utom
tion skills (#21-30) design- docs: 
dd memory event sourcing implement
tion pl
n- docs: 
dd memory event sourcing design (CQRS Light)- docs: 
dd prompt system enh
ncement implement
tion pl
n- docs: 
dd prompt system enh
ncement design- docs: 
dd skills system, upd
te runtimes refs, 
dd m
cOS components- docs: upd
te 
ccept
nce results 
fter bridge fixes (27/30 p
ss)- docs: 
dd implement
tion pl
n for fixing bridge known issues- docs: 
dd design for fixing bridge known issues- docs: remove rem
ining Swift references from CLAUDE.md- docs: upd
te CLAUDE.md 
nd cre
te migr
tion completion record (C13-C16)- docs: 
dd m
cOS Swift 
pp remov
l implement
tion pl
n- docs: 
dd m
cOS Swift 
pp remov
l design with 
ccept
nce criteri
- docs: 
dd desktop c
p
bilities evolution implement
tion pl
n- docs: 
dd desktop c
p
bilities evolution design- docs: 
dd sem
ntic t
rgeting implement
tion pl
n- docs: 
dd sem
ntic t
rgeting 
nd 
ction primitives design- docs: upd
te CLAUDE.md for Server-Centric Build Architecture- docs: 
dd Ph
se 3 
nd Ph
se 4 implement
tion pl
ns- docs: repl
ce Ghost 
esthetic with concrete product constr
ints R5-R7- docs: 
dd Ph
se 2.5 bridge integr
tion completion pl
n- docs: 
dd design for removing Ghost 
esthetic concept- docs: 
dd Ph
se 1 bridge skeleton implement
tion pl
n- docs: 
dd server-centric build 
rchitecture design- docs: upd
te worktree guidelines with EnterWorktree CWD lock c
ve
t- docs: 
dd cron system redesign pl
n — surp
ssing opencl
w- docs: 
dd memory optimiz
tion implement
tion pl
n- docs: 
dd memory module optimiz
tion design- docs: 
ddress code review findings (JIT-
pprov
l TODO, RwLock r
tion
le)- docs: bring in L
te-Binding Secure Execution design 
nd pl
n from m
in- docs: 
dd L
te-Binding Secure Execution implement
tion pl
n (14 t
sks, 4 w
ves)- docs: 
dd L
te-Binding Secure Execution Architecture design- docs: 
dd git worktree s
fety guide; fix missing ScreenRegion import- docs: 
dd Rust ref
ctoring implement
tion pl
n (7 t
sks, 4 w
ves)- docs: 
dd Rust core ref
ctoring design (4-w
ve str
tegy)- docs: 
dd runtime on-dem
nd implement
tion pl
n (13 t
sks, 4 ph
ses)- docs: 
dd runtime on-dem
nd implement
tion pl
n (13 t
sks, 4 ph
ses)- docs: 
dd runtime on-dem
nd n
tive bootstr
pping 
rchitecture design- docs: 
dd verific
tion test results to T
uri shell design doc- docs: 
dd T
uri cross-pl
tform shell implement
tion pl
n- docs: 
dd T
uri cross-pl
tform shell & DesktopBridge design- build(h
lo): rebuild WASM with /h
lo route- docs: split CLAUDE.md 
nd reorg
nize docs/ into docs/reference/- docs: 
dd 1-2-3-4 
rchitecture constitution design document- docs: 
dd H
lo UI Unific
tion implement
tion pl
n (10 t
sks)- docs: est
blish 1-2-3-4 
rchitecture model 
s constitution
l principles in CLAUDE.md- build(m
cos): 
dd WebKit fr
mework dependency for Settings WebView- docs: 
dd Ph
se 1 implement
tion pl
n — Settings WebView integr
tion- docs: 
dd UI unific
tion design — Leptos 
s single UI codeb
se- docs: 
dd Desktop Bridge implement
tion pl
n (11 t
sks, 4 ph
ses)- docs: 
dd Desktop Bridge design for UDS-b
sed Swift-Rust IPC- docs: 
dd Skill System v2 implement
tion pl
n (15 TDD t
sks)- docs: 
dd Skill System v2 design (complete DDD rebuild)- docs: upd
te 
ll document
tion for server-centric 
rchitecture- docs: upd
te CLAUDE.md for server-centric 
rchitecture- docs: 
dd server purific
tion implement
tion pl
n- docs: 
dd server purific
tion design - remove desktop control, embr
ce MCP plugins- docs: 
dd Skill System implement
tion pl
n with 14 TDD t
sks- docs: 
dd server-centric 
rchitecture implement
tion pl
n- docs: 
dd server-centric 
rchitecture refr
ming design- docs: 
dd Skill System dom
in-driven design document- docs: 
dd P0 ref
ctoring implement
tion pl
n for st
rt.rs 
nd extension/mod.rs- docs: 
dd CODE_ORGANIZATION guide with ref
ctoring b
cklog- docs: 
dd soci
l connectivity evolution design 
nd implement
tion pl
n- build: 
dd missing imports in control-pl
ne cfg block- docs: 
dd IronCl
w Ph
se 2/3 det
iled implement
tion pl
n- docs: 
dd IronCl
w Ph
se 2/3 design (host-bound
ry + EVM signing)- docs: 
dd code cle
nup implement
tion pl
n (16 t
sks, 3 p
sses)- docs: 
dd code cle
nup design pl
n (Occ
m's R
zor P
ss)- docs: 
dd ACMA implement
tion pl
n with 7 TDD t
sks- docs: 
dd ACMA (Aleph Cognitive Memory Architecture) design document- docs: 
dd exec security integr
tion design- docs: 
dd blog post on PII filtering g
tew
y implement
tion- docs: 
dd 
gent secret m
n
gement implement
tion pl
n- docs: 
dd 
gent secret m
n
gement design (Ph
se 1)- docs: 
dd Discord Control Pl
ne implement
tion pl
n- docs: 
dd Discord Control Pl
ne p
nel design- docs: 
dd memory worksp
ce implement
tion pl
n- docs: 
dd memory worksp
ce isol
tion design- docs: upd
te 
rchitecture docs to reflect L
nceDB migr
tion- docs: 
dd Wh
tsApp Bridge implement
tion pl
n (10 t
sks)- docs: 
dd Wh
tsApp Bridge design (Thin Sidec
r + Rich Ad
pter)- docs: upd
te MEMORY_SYSTEM.md 
nd CLAUDE.md for L
nceDB migr
tion- docs: embedding evolution implement
tion pl
n (13 t
sks)- docs: embedding evolution design (
bstr
ct provider + l
zy migr
tion)- docs: 
dd Memory VFS Evolution implement
tion pl
n- docs: 
dd Memory VFS Evolution design document- docs: 
dd Sw
rm Agent Loop integr
tion implement
tion pl
n- docs: 
dd Sw
rm Intelligence Architecture Agent Loop integr
tion design- docs(ssb): 
dd Ph
se 6 cross-pl
tform implement
tion pl
n- docs(ssb): 
dd cross-pl
tform 
rchitecture design- docs: cl
rify server-side execution model in CLAUDE.md- docs(ssb): 
dd Ph
se 6 enh
ncement pl
n 
nd complete ro
dm
p- docs: 
dd Sw
rm Intelligence Architecture design- build(control-pl
ne): upd
te compiled UI 
ssets for Ph
se 3- docs: 
dd System St
te Bus (SSB) 
rchitecture design- docs(skill-evolution): 
dd comprehensive document
tion 
nd ex
mples- docs: 
dd Coll
bor
tive Skill Evolution 
rchitecture design- docs: 
dd det
iled implement
tion pl
n for Control Pl
ne three-column l
yout- docs: 
dd Control Pl
ne three-column l
yout 
rchitecture design- docs: upd
te Control Pl
ne UI build workflow with T
ilwind CSS compil
tion- docs(cl
ude.md): 
dd WASM initi
liz
tion mech
nism expl
n
tion- docs(cl
ude.md): 
dd comprehensive Server development 
nd deployment guide- docs: 
dd UI comp
rison 
n
lysis for ControlPl
ne 
nd T
uri settings- docs: 
dd WebSocket client implement
tion summ
ry 
nd migr
tion pl
n- docs: 
dd ControlPl
ne integr
tion implement
tion summ
ry- docs: 
dd Ph
se 3 implement
tion pl
n- docs: 
dd Ph
se 3 design for skill s
ndboxing- docs: 
dd comprehensive skill s
ndboxing document
tion- docs: 
dd Ph
se 2 skill s
ndboxing implement
tion pl
n- docs: 
dd Ph
se 2 skill s
ndboxing design document- docs(sh
red-ui-logic): m
rk API L
yer 
s complete- docs(sh
red-ui-logic): m
rk WASM connector 
s complete- docs(sh
red-ui-logic): upd
te README with API 
nd Observ
bility progress- docs(sh
red_ui_logic): upd
te README with protocol l
yer st
tus- docs(sh
red_ui_logic): upd
te README with n
tive connector st
tus- docs(sh
red_ui_logic): 
dd comprehensive README- docs: 
dd sh
red_ui_logic design document- docs: complete Ph
se 3 
rchitecture document
tion- docs: 
dd Ph
se 1 implement
tion pl
n for skill s
ndboxing- docs: 
dd skill s
ndboxing 
rchitecture design- docs(
rchitecture): 
dd comprehensive cle
nup design document- docs: reorg
nize root directory 
nd est
blish document
tion structure- docs(
rchitecture): 
dd Ph
se 3 browser ref
ctoring design- docs(
rchitecture): 
dd Ph
se 6 tools server ref
ctoring design- docs(
rchitecture): 
dd Ph
se 5 plugins h
ndlers ref
ctoring design- docs(
rchitecture): 
dd Ph
se 4 POE h
ndlers ref
ctoring design- docs: 
dd Ph
se 2 continu
tion guide for next session- docs(
rchitecture): 
dd Ph
se 2 
tomic executor ref
ctoring design- docs(
rchitecture): 
dd Ph
se 1 types ref
ctoring design- docs(cortex): 
dd Month 3 implement
tion pl
n- docs(cortex): 
dd Month 3 Met
-Cognition L
yer design- docs: 
dd Atomic Engine fin
l implement
tion report- docs: 
dd comprehensive Atomic Engine document
tion- docs: 
dd Atomic Engine progress report (90% complete)- docs: 
dd Atomic Engine short-term t
sk completion st
tus- docs: 
dd Cortex evolution system design- docs: 
dd Atomic Engine evolution ro
dm
p (3-12+ months)- docs: 
dd 
tomic engine implement
tion st
tus report- docs: 
dd l
ngu
ge preference to CLAUDE.md- docs: 
dd Ph
se 2 Intelligent Scheduling design- docs: 
dd guest session 
ctivity logging implement
tion pl
n- docs: 
dd Liquid Hub cross-pl
tform 
rchitecture design- docs: complete Identity Context security document
tion- docs: 
dd Identity Context & Security Enforcement design- docs: 
dd ConfigM
n
ger 
nd Memory N
mesp
ce implement
tion pl
n- docs: 
dd ConfigM
n
ger 
nd Memory N
mesp
ce design- docs: 
dd Person
l AI Hub implement
tion pl
n- docs: 
dd Person
l AI Hub 
rchitecture design- docs: 
dd client 
rchitecture document
tion 
nd testing guide- docs: 
dd Ph
se 2 progress report- docs: 
dd client 
rchitecture ref
ctoring pl
n- docs: document Server-Client 
rchitecture in CLAUDE.md- docs: 
dd Server-Client implement
tion pl
n- docs: 
dd Server-Client 
rchitecture design- docs: 
dd DDD terminology 
nd dom
in modeling guide- docs: 
dd DDD+BDD du
l-wheel 
rchitecture design- docs: 
dd comprehensive Tool-
s-Resource us
ge guide 
nd upd
te Ph
se 4 st
tus- docs: upd
te Ph
se 3 progress - L2 
nd observ
bility completed- docs: upd
te Ph
se 2 checkboxes to completed- docs: upd
te MEMORY_SYSTEM.md with Memory Evolution fe
tures- docs(bdd): 
dd comprehensive BDD testing guide 
nd upd
te pl
ns- docs: 
dd Ph
se 3 implement
tion pl
n- docs: m
rk Ph
se 2 
s complete with 
ll t
sks done- docs: document Ph
se 2 memory system components in TOOL_SYSTEM.md- docs: upd
te Ph
se 2 pl
n with completion st
tus- docs: upd
te implement
tion pl
n with completion summ
ry- docs: 
dd Ph
se 1 MVP implement
tion pl
n- docs: 
dd Multi-Agent 2.0 Ph
se 1 implement
tion pl
n- docs: 
dd memory system evolution design- docs: 
dd Multi-Agent Resilience document
tion- docs: upd
te Ph
se 1 checkboxes to completed- docs: upd
te Tool-
s-Resource design st
tus to In Progress- docs: 
dd Tool-
s-Resource implement
tion pl
n- docs: 
dd Multi-Agent Resilience & Govern
nce 
rchitecture design- docs: 
dd Tool-
s-Resource 
rchitecture design- docs: 
dd Embodiment Engine 
nd CoT Tr
nsp
rency document
tion- docs: 
dd Multi-Agent 2.0 
rchitecture design- docs(pl
ns): 
dd Embodiment Engine & CoT Tr
nsp
rency design- docs(
gent-system): 
dd Ch
nnel C
p
bility Aw
reness document
tion- docs: 
dd ch
nnel c
p
bility 
w
reness implement
tion pl
n- docs: 
dd ch
nnel c
p
bility 
w
reness 
rchitecture design- docs: 
dd worksp
ce 
rchitecture design- docs: 
dd Ph
se 5 implement
tion pl
n- docs: 
dd Ph
se 5 Custom Rules Engine 
rchitecture design- docs: 
dd WorldModel + Disp
tcher 
rchitecture design- docs(d
emon): 
dd perception l
yer document
tion- docs: 
dd Protocol Ad
pter Ph
se 4 implement
tion summ
ry- docs(
rchitecture): document configur
ble protocol 
d
pter system- docs(protocols): 
dd comprehensive protocol 
d
pter user guide- docs: 
dd Ph
se 2 Perception L
yer implement
tion pl
n- docs(protocols): 
dd ex
mple YAML protocol configur
tions- docs: 
dd Ph
se 2 Perception L
yer design- docs: 
dd d
emon module document
tion- docs: 
dd Ph
se 1 d
emon implement
tion pl
n- docs: 
dd pro
ctive AI 
rchitecture design- build: remove deprec
ted c
bi fe
ture 
nd fix Discord API- docs: 
dd comprehensive M
rkdown Tool Ad
pter implement
tion summ
ry- docs: 
dd Protocol Ad
pter Ph
se 4 design- docs: 
dd M
rkdown Tool Ad
pter design specific
tion- docs: 
dd Protocol Ad
pter Ph
se 3 implement
tion summ
ry- docs: 
dd Protocol Ad
pter Ph
se 2 implement
tion summ
ry- docs: 
dd Protocol Ad
pter Ph
se 2 implement
tion pl
n- docs: 
dd Protocol Ad
pter Ph
se 2 design for Cl
ude/Gemini migr
tion- docs(providers): upd
te module document
tion for Protocol Ad
pter 
rchitecture- docs: 
dd Protocol Ad
pter implement
tion pl
n- docs: 
dd Protocol Ad
pter 
rchitecture design- docs(pl
ns): 
dd P2.5 MCP Adv
nced Fe
tures implement
tion pl
n- docs(mcp): 
dd P2 
dv
nced fe
tures implement
tion pl
n- docs: 
dd Memory v3 implement
tion pl
n with bite-sized TDD t
sks- docs(mcp): 
dd P1 c
p
bilities implement
tion pl
n- docs: 
dd Memory System v3 "Gl
ss Box" 
rchitecture design- docs(mcp): 
dd MCP Orchestr
tion L
yer implement
tion pl
n- docs(mcp): 
dd MCP Orchestr
tion L
yer design- docs(cortex): 
dd det
iled implement
tion pl
n with TDD steps- docs(extension): 
dd P0.5-P2 fe
ture document
tion- docs(extension): 
dd P0.5-P2 implement
tion pl
n- docs(extension): 
dd SDK V2 document
tion- docs(disp
tcher): 
dd Cortex 2.0 
rchitecture design- docs(extension): 
dd SDK V2 P0 implement
tion pl
n- docs(extension): 
dd Aether Extension SDK V2 design specific
tion- docs(skills): 
dd det
iled implement
tion pl
n for requirements fe
ture- docs(skills): 
dd requirements & CLI wr
pper 
rchitecture design- docs(poe): 
dd contr
ct signing design for first principles closure- docs: upd
te memory system docs 
nd 
dd h
lo comm
nd system pl
n- docs: 
dd mess
ge flow optimiz
tion design 
nd implement
tion pl
n- docs: 
dd H
lo-Only mess
ge flow design 
nd implement
tion pl
n- docs: 
dd comprehensive 
rchitecture document
tion- docs: 
dd det
iled POE implement
tion pl
n- docs: 
dd POE (Principle-Oper
tion-Ev
lu
tion) 
rchitecture design- docs: 
dd Agent-Action inter
ction implement
tion pl
n- docs: 
dd Agent-Action inter
ction system design- docs: m
rk Milestone 6 (ResilientT
sk) 
s complete- docs: 
dd Rust l
yer code cle
nup design pl
n- docs: 
dd Milestone 6 resilient t
sk implement
tion pl
n- docs: m
rk Milestone 5 (skill evolution) 
s complete- docs: 
dd Milestone 5 skill evolution implement
tion pl
n- docs: m
rk Milestone 4 (spec-driven dev) 
s complete- docs: 
dd Milestone 4 spec-driven development implement
tion pl
n- docs: m
rk Milestone 3 (Telegr
m 
pprov
l) 
s complete

## [0.2.8] - 2026-03-22### Added- fe
t(p
nel): 
dd stre
ming, render_mode, typing_indic
tor fields to Feishu settings- fe
t(feishu): wire FeishuEventEmitter into execution flow- fe
t(feishu): 
dd m
rkdown c
rd rendering 
nd upd
ted c
p
bilities- fe
t(feishu): 
dd FeishuEventEmitter with stre
ming c
rds 
nd typing indic
tors- fe
t(feishu): 
dd C
rd Kit stre
ming, st
tic c
rd, 
nd re
ction API methods- fe
t(feishu): 
dd stre
ming, render_mode, typing config fields 
nd API types- fe
t(p
nel): 
dd Feishu/L
rk ch
nnel settings c
rd- fe
t(feishu): fix clippy w
rnings — unused import, visibility, closure- fe
t(feishu): 
dd FeishuCh
nnel impl 
nd wire into f
ctory registry- fe
t(feishu): 
dd FeishuClient with token, HTTP API, 
nd medi
 support- fe
t(feishu): 
dd WebSocket event p
rsing 
nd text extr
ction- fe
t(feishu): 
dd types, config, 
nd API response structs- fe
t: 
dd Persistent Completion Protocol for 
gent t
sk verific
tion- desktop-m
cos: implement PimC
p
bility vi
 SwiftBridge- desktop-m
cos: implement SystemC
p
bility (
pps, notific
tions, clipbo
rd, sysinfo)- desktop-m
cos: implement Autom
tionC
p
bility (os
script + Shortcuts CLI)- desktop: wire N
tiveScreen into 
ll pl
tform cr
tes- desktop: 
dd N
tiveScreen sh
red ScreenC
p
bility implement
tion- core: 
dd SystemTool 
nd Autom
tionTool builtin tools- desktop: 
dd per-pl
tform cr
te skeletons (m
cos, linux, windows)- desktop: 
dd SwiftBridge utility for m
cOS n
tive API c
lls- desktop: upd
te cr
te doc to reflect two-l
yer 
rchitecture- desktop: 
dd c
p
bility tr
it hier
rchy 
nd sh
red types- core: 
dd 
leph-client dependency for server bin
ry- fe
t: en
ble n
tive tool c
lling for Ch
tGPT/Codex Responses API- core: 
dd Strict Mode support (schem
 strictific
tion + provider integr
tion)- core: 
dd #[cfg(unix)] gu
rds for Unix socket code on Windows- desktop: fix Windows OCR compil
tion errors- fe
t(browser): 
dd profile config types 
nd browser system configur
tion- fe
t(browser): 
dd SsrfPolicy for URL v
lid
tion 
nd priv
te network blocking- fe
t(config): 
dd queue_mode session configur
tion with g
tew
y wiring- fe
t(
nthropic): wire c
che_control ephemer
l bre
kpoint for system prompt c
ching- fe
t(thinker): p
rtition system prompt into st
ble/dyn
mic zones for c
che optimiz
tion- fe
t(compressor): 
dd pre-comp
ction silent memory flush- fe
t(
gent-loop): 
dd CollectQueue with time-window mess
ge merging- fe
t(
gent-loop): 
dd SteerQueue with interrupt sign
ling- fe
t(
gent-loop): 
dd SessionQueue tr
it 
nd FollowupQueue implement
tion- fe
t(
gent-loop): wire interrupt ch
nnel into RunContext 
nd loop execution- fe
t(
gent-loop): 
dd InterruptCh
nnel for steering support- core: 
dd missing tr
cing::w
rn import for non-m
cOS builds- fe
t: unified sl
sh comm
nd system- fe
t: wire memory tools into 
gent execution + Two-Ph
se Sm
rt Rec
ll- fe
t(server): 
dd desktop fe
ture g
te for in-process desktop c
p
bilities- fe
t(desktop): integr
te DesktopC
p
bility into DesktopTool with du
l-p
th execution- fe
t(desktop): implement input 
ctions with enigo- fe
t(desktop): implement screenshot 
nd OCR vi
 xc
p- fe
t: 
dd 
leph-desktop cr
te skeleton with DesktopC
p
bility tr
it- desktop: fix T
uri build for m
cOS 
nd 
dd 
pp/dmg bundle t
rgets- fe
t(w
sm): register host functions vi
 PluginBuilder with c
p
bility kernel- fe
t(m
nifest): p
rse WASM c
p
bilities from 
leph.plugin.toml- fe
t(w
sm): 
dd W
smC
p
bilityKernel — per-execution security enforcement- fe
t(w
sm): 
dd Credenti
lInjector — plugins never see secrets- fe
t(w
sm): 
dd AllowlistV
lid
tor with 
nti-byp
ss security- fe
t(w
sm): 
dd W
smC
p
bilities types with def
ult-deny model- fe
t(exec): 
dd Le
kDetector with Aho-Cor
sick bidirection
l sc
nning- desktop: 
dd 
ll_d
y 
nd c
lend
r_id to PimC
lend
rUpd
te- desktop: 
dd PIM v
ri
nts to DesktopRequest 
nd JSON-RPC m
pping- desktop: remove m
cOS t
rget, 
dd server embedding for Linux/Windows- desktop: fix fl
ky tests th
t 
ssumed bridge socket 
bsence- desktop-bridge: implement Windows OCR (WinRT) 
nd UI Autom
tion AX tree- desktop-bridge: implement window m
n
gement (list, focus, l
unch)- desktop-bridge: implement Windows input simul
tion (click, type, key combo, scroll)- desktop: wire sn
pshot 
nd new 
ctions in DesktopBridgeServer disp
tch- desktop: implement scroll, double-click, dr
g, hover, p
ste, 
nd ref-
w
re t
rgeting- desktop: implement UI sn
pshot with ref gener
tion in Perception.swift- desktop: 
dd RefStore for sn
pshot ref m
n
gement (Swift)- desktop: upd
te tool 
rgs 
nd build_request for sn
pshot, ref t
rgeting, 
nd new 
ctions- desktop: 
dd core types for sn
pshot, ref system, 
nd new 
ction primitives- desktop: upd
te tool mess
ging for bridge 
rchitecture- desktop: probe m
n
ged 
nd st
nd
lone socket p
ths- fe
t(runtimes): 
dd ensure_c
p
bility orchestr
tion (Probe -> Bootstr
p -> Register)- fe
t(runtimes): wire C
p
bilityLedger into prompt system- fe
t(runtimes): 
dd bootstr
p module with shell-driven inst
ll
tion- fe
t(runtimes): wire ledger into exec l
yer PATH- fe
t(runtimes): 
dd Probe module for system-first c
p
bility detection- fe
t(runtimes): 
dd leg
cy m
nifest.json migr
tion to ledger.json- fe
t(runtimes): 
dd C
p
bilityLedger for lightweight runtime st
te tr
cking- fe
t(desktop): implement desktop.screenshot in T
uri DesktopBridge- fe
t(desktop): 
dd DesktopBridge UDS server with ping support- fe
t(protocol): 
dd desktop_bridge types for cross-pl
tform Bridge- fe
t(h
lo): switch m
cOS H
loWindow from SwiftUI to WKWebView- fe
t(h
lo): 
dd /h
lo route with ch
t UI, mess
ge list, 
nd input 
re
- fe
t(h
lo): 
dd event h
ndler to wire run.* stre
ming events to H
loSt
te- fe
t(h
lo): 
dd H
loSt
te re
ctive sign
ls for ch
t st
te m
n
gement- fe
t(h
lo): 
dd Ch
tApi module for ch
t.send/
bort/history/cle
r- fe
t(desktop): T
sk 11 complete — DesktopTool 
ctive in 
gent vi
 builtin registry- fe
t(desktop): implement WKWebView c
nv
s overl
y with A2UI p
tch support- fe
t(desktop): implement mouse, keybo
rd, 
nd window 
ctions in Action.swift- fe
t(desktop): 
dd 
ccessibility permission description 
nd runtime check- fe
t(desktop): implement screenshot, OCR, 
nd AX tree in Perception.swift- fe
t(desktop): point settings window to Leptos Control Pl
ne server- fe
t(m
cos): 
dd Settings menu item opening Control Pl
ne WebView- fe
t(m
cos): 
dd SettingsWebView WKWebView wr
pper- fe
t(desktop): 
dd Swift UDS server skeleton with stub h
ndlers- fe
t(desktop): register DesktopTool in executor builtin registry- fe
t(desktop): 
dd DesktopTool builtin with gr
ceful degr
d
tion- fe
t(desktop): 
dd UDS client with JSON-RPC 2.0 
nd unit tests- fe
t(desktop): 
dd types, error, 
nd module sc
ffold- fe
t(skill): integr
te SkillSystem v2 into ExtensionM
n
ger 
nd ExecutionEngine- fe
t(skill): 
dd SkillSystem f
c
de with Arc<Inner> p
ttern- fe
t(skill): 
dd sl
sh comm
nd resolution- fe
t(skill): 
dd Inst
llSpec to shell comm
nd converter- fe
t(skill): 
dd SkillSt
tusReport for eligibility d
shbo
rd- fe
t(skill): 
dd SkillSn
pshot with version-inv
lid
ted c
che- fe
t(skill): 
dd XML prompt builder for skill injection- fe
t(skill): 
dd EligibilityService with OS/bin
ry/env checks- fe
t(skill): 
dd SKILL.md p
rser with YAML frontm
tter support- fe
t(skill): 
dd SkillRegistry with priority-b
sed dedup- fe
t(skill): 
dd SkillM
nifest Aggreg
teRoot with Entity tr
it- fe
t(skill): 
dd EligibilitySpec, Inst
llSpec, Invoc
tionPolicy, PromptScope V
lueObjects- fe
t(skill): 
dd SkillId, PluginId, SkillSource dom
in types- fe
t(thinker): 
dd skill_instructions to PromptConfig for SkillSystem v2- fe
t(extension): 
dd SkillSystem v2 
nd wire skill XML into 
gent prompts- fe
t(sw
rm): 
dd event st
tistics 
nd logging- fe
t(
gent_loop): integr
te ContextProvider into Mess
geBuilder- fe
t(sw
rm): implement Sw
rmContextProvider- fe
t(
gent_loop): define ContextProvider tr
it- fe
t(
gent_loop): implement event publishing (sh
dow mode)- fe
t(
gent_loop): define AgentLoopEvent enum- fe
t(
gent_loop): implement Builder build() method- fe
t(
gent_loop): 
dd AgentLoopBuilder structure- fe
t(perception): integr
te PAL with SystemSt
teBus- fe
t(perception): 
dd Pl
tform Abstr
ction L
yer (PAL)- fe
t(sw
rm): Ph
se 5 - End-to-End Integr
tion- fe
t(perception): implement Ph
se 5 - Document
tion, Ex
mples & Testing- fe
t(perception): implement Ph
se 4 - Vision Connector 
rchitecture- fe
t(ssb): implement Ph
se 3 - 
ction disp
tcher- fe
t(ssb): implement Ph
se 2 - robustness & priv
cy- fe
t(ssb): implement Ph
se 1 - core infr
structure- fe
t(control-pl
ne): implement WebSocket subscription for re
l-time 
lerts- fe
t(sh
red_ui_logic): 
dd 
lerts API module for system he
lth 
nd memory monitoring- fe
t(skill-evolution): integr
te SuccessM
nifest with tool execution- fe
t(control-pl
ne): p
ss mode 
nd 
lert_key to Sideb
rItems- fe
t(control-pl
ne): integr
te Tooltip 
nd B
dge into Sideb
rItem- fe
t(control-pl
ne): 
dd St
tusB
dge component for 
lert indic
tors- fe
t(control-pl
ne): 
dd Tooltip component for n
rrow mode l
bels- fe
t(skill-evolution): implement Coll
bor
tiveSolidific
tionPipeline- fe
t(control-pl
ne): implement Sideb
r n
rrow/wide mode switching- fe
t(skill-evolution): implement Constr
intV
lid
tor- fe
t(skill-evolution): implement SuccessM
nifest d
t
 structure- fe
t(control-pl
ne): 
dd SettingsL
yout for nested routing- fe
t(control-pl
ne): 
dd 
lert bus 
nd sideb
r mode override to D
shbo
rdSt
te- fe
t(control-pl
ne): 
dd sideb
r types (Sideb
rMode, AlertLevel, SystemAlert)- fe
t(control-pl
ne): compile T
ilwind CSS loc
lly for production- fe
t(d
shbo
rd): 
dd Plugins, Skills, 
nd Policies settings p
ges- fe
t(d
shbo
rd): 
dd sideb
r n
vig
tion to settings UI- fe
t(d
shbo
rd): 
dd Gener
tion Providers n
vig
tion c
rd to Settings p
ge- fe
t(d
shbo
rd): implement Gener
tion Providers CRUD function
lity- fe
t(d
shbo
rd): 
dd Gener
tion Providers frontend UI- fe
t(d
shbo
rd): 
dd Gener
tion Providers b
ckend 
nd API l
yer- fe
t(d
shbo
rd): implement comprehensive configur
tion m
n
gement UI- fe
t(m
cos): implement WebSocket client for G
tew
y connection- fe
t(m
cos): complete Ph
se 4 client simplific
tion for ControlPl
ne integr
tion- fe
t(d
shbo
rd): complete Ph
se 3 SDK integr
tion with RPC, events, 
nd API l
yer- fe
t(d
shbo
rd): complete Ph
se 2 SDK integr
tion with error h
ndling 
nd reconnection- fe
t(d
shbo
rd): 
dd connection st
te 
w
reness to Memory view- fe
t(d
shbo
rd): integr
te sh
red_ui_logic SDK into D
shbo
rd- fe
t(d
shbo
rd): full 
rchitectu
l ref
ctor with Leptos 0.8.15 
nd rust-ui components- fe
t(d
shbo
rd): complete Memory Explorer view 
nd fix System St
tus- fe
t(d
shbo
rd): initi
lize Aleph D
shbo
rd with Leptos 0.6- fe
t(sh
red-ui-logic): implement Plugins 
nd Providers APIs- fe
t(sh
red-ui-logic): implement WASM WebSocket connector- fe
t(sh
red-ui-logic): implement API 
nd Observ
bility l
yers- fe
t(sh
red_ui_logic): implement protocol l
yer- fe
t(sh
red_ui_logic): implement n
tive WebSocket connector- fe
t(sh
red_ui_logic): initi
lize Aleph UI Logic SDK- fe
t(cortex): implement LLM-b
sed critic report gener
tion- fe
t(cortex): 
dd AiProvider to CriticAgent- fe
t(cortex): implement LLM-b
sed root c
use 
n
lysis- fe
t(cortex): 
dd AiProvider to Re
ctiveReflector- fe
t(
gent_loop): 
dd met
-cognition integr
tion for Ph
se 6- fe
t(cortex): implement CortexIntegr
tion orchestr
tor (T
sk #11)- fe
t(cortex): implement experience clustering 
nd deduplic
tion- fe
t(disp
tcher): implement L1.5 ExperienceRepl
yL
yer- fe
t(cortex): implement Cortex Dre
ming b
ckground service- fe
t(cortex): implement LLM-b
sed p
ttern extr
ction- fe
t(cortex): implement Distill
tionService core structure- fe
t(engine): 
dd Fe
tureExtr
ctor for 
dv
nced ML rule le
rning- fe
t(cortex): implement multi-dimension
l experience v
lue estim
tor- fe
t(cortex): 
dd 
gent loop telemetry c
pture- fe
t(cortex): implement Experience CRUD oper
tions- fe
t(cortex): define core d
t
 structures- fe
t(engine): 
dd ML-b
sed L2 rule gener
tion (RuleLe
rner)- fe
t(cortex): 
dd experience_repl
ys d
t
b
se t
ble- fe
t(builtin_tools): 
dd AtomicOpsTool for 
tomic oper
tions- fe
t(browser): implement J
v
Script-b
sed context freeze/resume- fe
t(browser): implement Ph
se 2.4 CDP integr
tion for context freeze/resume- fe
t(engine): 
dd comprehensive testing 
nd perform
nce v
lid
tion- fe
t(executor): 
dd AtomicActionExecutor with L1/L2 routing- fe
t(engine): implement 
tomic engine with L1/L2/L3 routing- fe
t(disp
tcher): implement Ph
se 2 Intelligent Scheduling for Liquid Hub- fe
t(m
cos): 
dd guest session 
ctivity log UI- fe
t(m
cos): 
dd 
ctivity log RPC types 
nd methods- fe
t(g
tew
y): 
dd RPC request 
ctivity logging for guest sessions- fe
t(g
tew
y): 
dd guests.getActivityLogs RPC h
ndler- fe
t(g
tew
y): integr
te 
ctivity logging into GuestSessionM
n
ger- fe
t: implement guests.revokeInvit
tion RPC method- fe
t(m
cos): 
dd Guest m
n
gement UI in Settings- fe
t(g
tew
y): register config.get 
nd config.p
tch RPC h
ndlers- fe
t(g
tew
y): 
dd SessionIdentityMet
 for identity stor
ge- fe
t(protocol): 
dd IdentityContext for st
teless security- fe
t(g
tew
y): 
dd config.p
tch RPC h
ndler with events- fe
t(memory): 
dd idempotent n
mesp
ce migr
tion- fe
t(g
tew
y): 
dd RPC h
ndlers for guest m
n
gement- fe
t(memory): 
dd n
mesp
ce column for d
t
 isol
tion- fe
t(protocol): 
dd discovery types for mDNS- fe
t(protocol): 
dd ConfigCh
ngedEvent for config sync- fe
t(g
tew
y): 
dd Invit
tionM
n
ger for guest invit
tions- fe
t(protocol): 
dd invit
tion types for guest m
n
gement- fe
t(g
tew
y): 
dd PolicyEngine for permission checks- fe
t(g
tew
y): 
dd IdentityM
p for extern
l identity resolution- fe
t(protocol): 
dd Role 
nd GuestScope for Owner+Guest model- fe
t(ph
se3): complete T
uri Desktop migr
tion to thin client- fe
t(ph
se3): migr
te T
uri Desktop to SDK 
rchitecture (WIP)- fe
t(ph
se2): ref
ctor CLI to use SDK- fe
t(ph
se2): implement G
tew
yClient with 
uthentic
tion- fe
t(ph
se2): implement tr
nsport 
nd RPC l
yers in SDK- fe
t(ph
se2): cre
te 
leph-client-sdk skeleton- fe
t(g
tew
y): 
dd Server-Client routing infr
structure to ConnectionSt
te- fe
t: 
dd tool routing config 
nd scope checking for Server-Client 
rchitecture- fe
t(executor): integr
te RoutedExecutor with Agent Loop- fe
t(cli): cre
te 
leph-cli 
s protocol reference implement
tion- fe
t(protocol): cre
te 
leph-protocol cr
te for sh
red types- fe
t(executor): integr
te ToolRouter with execution engine- fe
t(disp
tcher): 
dd execution_policy field to UnifiedTool- fe
t(executor): 
dd ToolRouter for Server-Client routing decisions- fe
t(g
tew
y): 
dd tool.c
ll protocol mess
ges- fe
t(g
tew
y): 
dd ReverseRpcM
n
ger for Server-to-Client c
lls- fe
t(g
tew
y): store ClientM
nifest in ConnectionSt
te- fe
t(g
tew
y): extend ConnectP
r
ms to 
ccept ClientM
nifest- fe
t(g
tew
y): 
dd ClientM
nifest for c
p
bility negoti
tion- fe
t(disp
tcher): 
dd ExecutionPolicy enum for Server-Client routing- fe
t(spec_driven): implement BDD du
l-tr
ck testing system- fe
t(dom
in): implement DDD found
tion with m
rker tr
its- fe
t(disp
tcher): implement L2 
sync LLM enh
ncement for tool descriptions- fe
t(memory): 
dd perform
nce monitoring for LLM c
lls- fe
t(scheduler): implement recursion depth tr
cking- fe
t(scheduler): implement 
nti-st
rv
tion logic- fe
t(scheduler): implement L
neScheduler core- fe
t: implement CompressionD
emon for b
ckground compression scheduling- fe
t(scheduler): implement L
neSt
te with queue 
nd sem
phore- fe
t: enh
nce ContextComptroller with priority-b
sed token m
n
gement- fe
t: implement V
lueEstim
tor for memory import
nce scoring- fe
t(scheduler): 
dd l
ne scheduler infr
structure- fe
t: 
dd sliding window chunking to Tr
nscriptIndexer- fe
t: 
dd Tr
nscriptIndexer for ne
r-re
ltime memory indexing- fe
t(sub_
gents): 
dd 
ctive runs query 
nd st
ts to SubAgentRegistry- fe
t(sub_
gents): 
dd F
ctsDB persistence helpers for SubAgentRun- fe
t(sub_
gents): 
dd st
te tr
nsition to SubAgentRegistry- fe
t(sub_
gents): 
dd SubAgentRegistry with in-memory indexing- fe
t(memory): 
dd SubAgent f
ct types for Multi-Agent 2.0 persistence- fe
t(sub_
gents): 
dd SubAgentRun d
t
 model for Multi-Agent 2.0- fe
t(disp
tcher): integr
te Hydr
tionPipeline into Agent Loop- fe
t(core): export tool_index types from lib.rs- fe
t(memory): 
dd VectorD
t
b
se::in_memory() for testing- fe
t(disp
tcher): 
dd ToolRetriev
l with du
l-threshold hydr
tion- fe
t(disp
tcher): 
dd ToolIndexCoordin
tor for Memory synchroniz
tion- fe
t(disp
tcher): 
dd Sem
nticPurposeInferrer for L0/L1 inference- fe
t(disp
tcher): 
dd tool_index module with ToolRetriev
lConfig- fe
t(memory): 
dd Tool v
ri
nt to F
ctType for tool-
s-resource- fe
t(memory): 
dd Multi-Agent Resilience d
t
b
se l
yer- fe
t(g
tew
y): 
dd identity m
n
gement RPC h
ndlers- fe
t(thinker): 
dd thinking tr
nsp
rency guid
nce to PromptBuilder- fe
t(
gent_loop): integr
te ThinkingP
rser into DecisionP
rser- fe
t(g
tew
y): 
dd Re
soningBlock 
nd Uncert
intySign
l stre
m events- fe
t(
gent_loop): 
dd ThinkingP
rser for sem
ntic re
soning extr
ction- fe
t(
gent_loop): 
dd StructuredThinking types for CoT Tr
nsp
rency- fe
t(thinker): integr
te Soul into PromptBuilder- fe
t(thinker): 
dd m
rkdown p
rser for soul.md files- fe
t(thinker): 
dd IdentityResolver for l
yered identity resolution- fe
t(thinker): 
dd SoulM
nifest types for Embodiment Engine- fe
t(test): migr
te logging, security, 
nd e2e tests to BDD- fe
t(test): migr
te iMess
ge routing 
nd sub
gent tests to BDD- fe
t(g
tew
y): 
dd Ch
nnelProvider tr
it for inter
ction m
nifests- fe
t(
gent_loop): 
dd Silent 
nd He
rtbe
tOk decision types- fe
t(thinker): 
dd environment contr
ct 
nd security sections to PromptBuilder- fe
t(thinker): 
dd ContextAggreg
tor for environment reconcili
tion- fe
t(test): migr
te m
rkdown skills tests to BDD- fe
t(thinker): 
dd SecurityContext for policy-driven permissions- fe
t(thinker): 
dd Inter
ctionM
nifest for ch
nnel c
p
bility 
w
reness- fe
t(test): migr
te models 
nd protocol integr
tion tests to BDD- fe
t(test): migr
te DAG 
nd worldmodel disp
tcher tests to BDD- fe
t(test): migr
te sm
rt tool discovery 
nd sessions tests to BDD- fe
t(thinker): 
dd provider-specific context c
ching str
tegies- fe
t(disp
tcher): 
dd du
l-l
yer profile-b
sed tool filtering- fe
t(test): migr
te extension v2 
nd runtime tests to BDD- fe
t(g
tew
y): 
dd Worksp
ceM
n
ger for Anti-Gr
vity Architecture- fe
t(test): migr
te extension plugin registry tests to BDD- fe
t(test): migr
te tool server tests to BDD- fe
t(test): migr
te g
tew
y inbound router tests to BDD- fe
t(test): migr
te disp
tcher cortex tests to BDD- fe
t(test): migr
te memory integr
tion tests to BDD- fe
t(tests): migr
te memory f
cts tests to BDD- fe
t(tests): migr
te mess
ge builder tests to BDD- fe
t(tests): migr
te thinker prompt builder tests to BDD- fe
t(tests): migr
te POE tests to BDD- fe
t(tests): migr
te 
gent loop tests to BDD- fe
t(config): 
dd ProfileConfig for Worksp
ce Architecture- fe
t(tests): migr
te perception 
nd w
tcher tests to BDD- fe
t(tests): migr
te d
emon IPC 
nd l
unchd tests to BDD- fe
t(tests): migr
te d
emon core tests to BDD- fe
t(tests): migr
te config v
lid
tion tests to BDD- fe
t(tests): migr
te config b
sic tests to BDD- fe
t(tests): migr
te scripting engine tests to BDD- fe
t(tests): 
dd cucumber BDD infr
structure- fe
t: 
dd ex
mple YAML policies 
nd E2E tests- fe
t(disp
tcher): 
dd YAML policy lo
der 
nd PolicyEngine integr
tion- fe
t(disp
tcher): implement Y
mlPolicy with Rh
i ev
lu
tion- fe
t(scripting): 
dd B
selineApi with l
zy TTL c
ching- fe
t(scripting): implement HistoryApi.l
st() with WorldModel queries- fe
t(scripting): implement EventApi 
nd EventCollection filtering- fe
t(scripting): 
dd HistoryApi 
nd EventCollection stubs- fe
t(scripting): 
dd dur
tion p
rsing 
nd helpers for Rh
i- fe
t(disp
tcher): 
dd YAML rule schem
 p
rsing- fe
t(disp
tcher): 
dd Rh
i s
ndbox engine with strict limits- fe
t(worldmodel): 
dd JSON st
te persistence- fe
t(disp
tcher): 
dd core d
t
 structures- fe
t(d
emon): integr
te perception l
yer with d
emon CLI- fe
t(d
emon): implement FSEventW
tcher- fe
t(d
emon): implement SystemSt
teW
tcher- fe
t(d
emon): implement ProcessW
tcher- fe
t(d
emon): implement TimeW
tcher- fe
t(d
emon): 
dd w
tcher tr
it 
nd registry- fe
t(d
emon): 
dd perception configur
tion system- fe
t(d
emon): 
dd event system found
tion- fe
t(protocols): implement hot relo
d with notify file w
tching- fe
t(protocols): implement ProtocolLo
der file 
nd directory lo
ding- fe
t(protocols): implement Configur
bleProtocol custom mode with templ
te rendering- fe
t(protocols): implement Configur
bleProtocol minim
l mode (extends b
se + differences)- fe
t(protocols): 
dd JSONP
th p
rser for response v
lue extr
ction- fe
t(protocols): 
dd templ
te engine wr
pper for request/response tr
nsform
tion- fe
t(protocols): 
dd dependencies for configur
ble protocols (h
ndleb
rs, jsonp
th, notify)- fe
t(providers): 
dd ProtocolLo
der stub for hot relo
d- fe
t(providers): 
dd Configur
bleProtocol stub- fe
t(providers): implement ProtocolRegistry for dyn
mic protocol m
n
gement- fe
t(providers): 
dd ProtocolDefinition types for YAML configs- fe
t(tools): implement Virtu
lFs s
ndbox mode- fe
t(tools): 
dd Evolution 
uto-lo
d integr
tion- fe
t(g
tew
y): 
dd M
rkdown Skills RPC h
ndlers- fe
t(tools): 
dd repl
ce_tool() API with explicit upd
te sem
ntics- fe
t(tools): 
dd hot relo
d support for M
rkdown Skills (Ph
se 4)- fe
t(tools): 
dd Evolution Loop integr
tion for M
rkdown Skills (Ph
se 3)- fe
t(tools): 
dd ex
mples() method to AetherTool tr
it (Ph
se 2)- fe
t(tools): complete M
rkdown Tool Ad
pter integr
tion- fe
t(tools): implement M
rkdown Tool Ad
pter (Ph
se 1)- fe
t(providers): 
dd Tier 3 speci
lized OpenAI-comp
tible provider presets- fe
t(providers): 
dd Tier 2 OpenAI-comp
tible provider presets- fe
t(providers): 
dd Tier 1 OpenAI-comp
tible provider presets- fe
t(providers): 
dd Gemini presets 
nd upd
te f
ctory- fe
t(providers): implement GeminiProtocol 
d
pter- fe
t(providers): 
dd Gemini API types module- fe
t(providers): 
dd Cl
ude/Anthropic presets- fe
t(providers): implement AnthropicProtocol 
d
pter- fe
t(providers): 
dd Anthropic API types module- fe
t(g
tew
y): 
dd 
pprov
l RPC h
ndlers- fe
t(mcp): 
dd Approv
lH
ndler for hum
n-in-the-loop- fe
t(mcp): 
dd 
pprov
l request types for hum
n-in-the-loop- fe
t(mcp): 
dd stre
ming types for s
mpling responses- fe
t(mcp): 
dd TokenRefreshM
n
ger for 
utom
tic token refresh- fe
t(mcp): 
dd OAuth token refresh support- fe
t(mcp): integr
te context injection with S
mplingH
ndler- fe
t(mcp): 
dd ContextInjector for cross-server context- fe
t(mcp): 
dd IncludeContext enum type for s
mpling requests- fe
t(config): 
dd protocol field to ProviderConfig- fe
t(providers): 
dd provider presets registry- fe
t(providers): 
dd HttpProvider cont
iner with ProtocolAd
pter- fe
t(providers): implement OpenAiProtocol 
d
pter- fe
t(providers): 
dd ProtocolAd
pter tr
it with stre
ming support- fe
t(providers): 
dd RequestP
ylo
d DTO for protocol 
d
pters- fe
t(mcp): 
dd s
mpling c
llb
ck integr
tion to McpM
n
ger- fe
t(mcp): 
dd response mech
nism for server-initi
ted requests- fe
t(mcp): integr
te S
mplingH
ndler with McpClient- fe
t(memory): complete Memory v3 Milestones 4-6- fe
t(mcp): 
dd S
mplingH
ndler for server-initi
ted LLM c
lls- fe
t(mcp): implement re
l SSE event listening with reqwest-eventsource- fe
t(mcp): 
dd SSE event types 
nd reqwest-eventsource dependency- fe
t(memory): implement CLI list 
nd show comm
nds- fe
t(memory): implement AuditLogger for oper
tion tr
cking- fe
t(mcp): 
dd S
mpling RPC types for P2 server-initi
ted LLM c
lls- fe
t(memory): 
dd 
udit log schem
 
nd types- fe
t(memory): 
dd CLI module with file locking- fe
t(memory): implement Archiv
lService for scr
tchp
d 
rchiving- fe
t(memory): implement HybridTrigger with token threshold s
fety net- fe
t(memory): implement L
zyDec
yEngine for re
d-time dec
y ev
lu
tion- fe
t(memory): 
dd type-
w
re dec
y c
lcul
tion with tempor
l scope- fe
t(memory): 
dd dec
y_inv
lid
ted_
t field for recycle bin- fe
t(memory): complete Milestone 1 - Scr
tchp
d Found
tion- fe
t(memory): implement Scr
tchp
dM
n
ger with CRUD oper
tions- fe
t(memory): implement SessionHistory for scr
tchp
d 
rchiv
l- fe
t(memory): 
dd scr
tchp
d module structure 
nd templ
te- fe
t(mcp): implement re
l McpResourceM
n
ger 
nd McpPromptM
n
ger- fe
t(tools): 
dd mcp_get_prompt builtin tool- fe
t(tools): 
dd mcp_re
d_resource builtin tool- fe
t(mcp): implement re
l 
ggreg
tion for resources 
nd prompts- fe
t(mcp): 
dd resources 
nd prompts methods to McpClient- fe
t(mcp): 
dd resources 
nd prompts support to McpServerConnection- fe
t(mcp): 
dd Resources 
nd Prompts RPC types- fe
t(mcp): 
dd he
lth check logic for servers- fe
t(g
tew
y): wire MCP h
ndlers to McpM
n
gerH
ndle- fe
t(mcp): implement McpM
n
gerActor core loop- fe
t(mcp): 
dd config persistence for McpM
n
ger- fe
t(mcp): 
dd McpM
n
gerH
ndle public API- fe
t(mcp): 
dd McpComm
nd 
nd McpM
n
gerEvent types- fe
t(cortex): implement DecisionConfig with session override- fe
t(cortex): implement security rules (t
g injection, PII m
sking, instruction override)- fe
t(cortex): 
dd S
nitizerRule tr
it 
nd SecurityPipeline- fe
t(cortex): 
dd greedy JSON rep
ir logic- fe
t(cortex): implement JsonStre
mDetector st
te m
chine- fe
t(cortex): 
dd module skeleton with unified error types- fe
t(extension): 
dd PluginHttpH
ndler for plugin REST routes- fe
t(extension): 
dd PluginProviderAd
pter for plugin AI providers- fe
t(extension): 
dd Ch
nnelM
n
ger skeleton for plugin ch
nnels- fe
t(extension): 
dd HTTP route types- fe
t(extension): 
dd provider plugin types- fe
t(extension): 
dd ch
nnel plugin types- fe
t(g
tew
y): 
dd service lifecycle RPC h
ndlers- fe
t(extension): integr
te ServiceM
n
ger with ExtensionM
n
ger- fe
t(extension): 
dd ServiceM
n
ger for b
ckground services- fe
t(extension): 
dd service lifecycle types- fe
t(g
tew
y): 
dd plugins.executeComm
nd RPC h
ndler- fe
t(extension): 
dd comm
nd execution to PluginLo
der- fe
t(extension): 
dd DirectComm
ndResult type- fe
t(extension): implement scope-
w
re skill injection- fe
t(extension): implement V2 prompt lo
ding with scope support- fe
t(extension): 
dd scope 
nd bound_tool to ExtensionSkill- fe
t(extension): 
dd PromptScope enum for V2 skill injection- fe
t(extension): 
dd V2 hook conversion from TOML m
nifest- fe
t(extension): implement typed hook execution (interceptor/observer/resolver)- fe
t(extension): 
dd kind 
nd priority to HookConfig- fe
t(extension): 
dd HookKind 
nd HookPriority enums- fe
t(extension): integr
te TOML p
rser with 
uto-detection (TOML > JSON)- fe
t(extension): 
dd V2 fields to PluginM
nifest- fe
t(extension): 
dd TOML m
nifest p
rser types- fe
t(exec): check skill_
llowlist in 
pprov
l decision- fe
t(exec): 
dd skill_
llowlist config option- fe
t(exec): extend ExecContext with skill origin info- fe
t(skills): implement CLI Wr
pper v
lid
tor- fe
t(skills): 
dd he
lth checking methods to SkillsRegistry- fe
t(skills): 
dd inst
ll suggestion methods to SkillsInst
ller- fe
t(skills): implement He
lthChecker for dependency v
lid
tion- fe
t(skills): extend SkillFrontm
tter with requirements 
nd met
d
t
- fe
t(skills): 
dd types for requirements 
nd he
lth checking- fe
t(poe): repl
ce Pl
ceholderWorker with re
l AgentLoopWorker- fe
t(g
tew
y): wire POE contr
ct signing to G
tew
y- fe
t(poe): implement contr
ct signing workflow for first principles closure- fe
t(core): 
dd sn
pshot c
pture tool 
nd registry upd
tes- fe
t(config): 
dd memory configur
tion types 
nd v
lid
tion- fe
t(memory): enh
nce retriev
l 
nd 
dd dre
ming module- fe
t(m
cos): 
dd tool emoji form
tting to H
loStre
mingView- fe
t(m
cos): upd
te G
tew
yStre
mAd
pter with enh
nced summ
ry- fe
t(m
cos): 
dd H
loResultViewV2 with det
il popover support- fe
t(m
cos): 
dd H
loResultDet
ilPopover for det
iled results- fe
t(m
cos): 
dd Enh
ncedRunSumm
ry 
nd ToolSumm
ryItem models- fe
t(g
tew
y): 
dd Enh
ncedRunSumm
ry 
nd per-runId sequences- fe
t(g
tew
y): 
dd mess
ge deduplic
tion with text norm
liz
tion- fe
t(g
tew
y): 
dd stre
m buffer for block-level text flushing- fe
t(g
tew
y): 
dd tool displ
y module with emoji 
nd sm
rt form
tting- fe
t(h
lo): integr
te comm
ndList st
te into H
loViewV2- fe
t(h
lo): 
dd H
loComm
ndListView for / comm
nd p
nel- fe
t(h
lo): 
dd Comm
ndItem 
nd Comm
ndListContext types for / comm
nd- fe
t(h
lo): 
dd H
loInputCoordin
tor for lightweight input h
ndling- fe
t(g
tew
y): 
dd 150ms throttling for response chunks- fe
t(h
lo): 
dd H
loViewV2 m
in component integr
ting 
ll st
te views- fe
t(h
lo): 
dd H
loHistoryListView for convers
tion history- fe
t(h
lo): 
dd H
loResultView for comp
ct result displ
y- fe
t(h
lo): 
dd H
loStre
mingView for unified stre
ming displ
y- fe
t(h
lo): 
dd H
loSt
teV2 with 6 simplified st
tes- fe
t(h
lo): 
dd new stre
ming types for simplified st
te model- fe
t(skill-evolution): implement Skill Compiler (Ph
se 10)- fe
t(
gent-loop): 
dd on_user_question method to LoopC
llb
ck- fe
t(
gent-loop): 
dd AskUserRich decision v
ri
nt with QuestionKind- fe
t(
gent-loop): export question 
nd 
nswer modules- fe
t(
gent-loop): 
dd UserAnswer type for structured responses- fe
t(
gent-loop): 
dd QuestionKind types for structured user inter
ction- fe
t(resilient): 
dd cron integr
tion with Podc
stT
sk ex
mple- fe
t(resilient): implement ResilientExecutor with retry 
nd f
llb
ck- fe
t(resilient): define ResilientT
sk tr
it- fe
t(resilient): 
dd core types for resilient t
sk execution- fe
t(skill_evolution): implement GitCommitter for 
uto-commit- fe
t(skill_evolution): implement SkillGener
tor for SKILL.md cre
tion- fe
t(skill_evolution): implement Solidific
tionDetector for p
ttern detection- fe
t(skill_evolution): implement EvolutionTr
cker for execution logging- fe
t(skill_evolution): 
dd core types for skill evolution system- fe
t(spec_driven): implement SpecDrivenWorkflow orchestr
tor- fe
t(spec_driven): implement LlmJudge for ev
lu
tion- fe
t(spec_driven): implement TestWriter for test gener
tion- fe
t(spec_driven): implement SpecWriter for requirement 
n
lysis- fe
t(spec_driven): 
dd core types for spec-driven workflow- fe
t(g
tew
y): 
dd exec.c
llb
ck.h
ndle RPC for 
pprov
l c
llb
cks- fe
t(telegr
m): 
dd edit_mess
ge method for 
pprov
l upd
tes- fe
t(g
tew
y): 
dd 
pprov
l bridge h
ndler utilities- fe
t(exec): 
dd Approv
lBridge for ch
nnel integr
tion- fe
t(telegr
m): 
dd c
llb
ck query h
ndling- fe
t(telegr
m): 
dd inline keybo
rd support### Fixed- fix: unignore CHANGELOG.md, fix rele
se recipe git 
dd- fix: remove unused imports 
cross codeb
se (c
rgo fix)- fix: resolve 42 test w
rnings — deprec
ted API, unused imports, de
d code- fix: sl
sh comm
nd f
st-p
th + CLI 
rg p
rser + E2E tests- fix: en
ble sl
sh comm
nd f
st-p
th for WebCh
t ch
t.send- fix: repl
ce env!("HOME") with dirs::home_dir() for Windows comp
tibility- fix: correct PluginKind::Mcp m
pping 
nd remove debug output- fix: upd
te discovery to find CC-form
t plugins in inst
lled/ directory- fix: ch
nnel binding not repl
cing old peer_id rows- fix: ch
nnel st
tus showing disconnected 
fter p
ge refresh- fix: p
ss session_m
n
ger to BuiltinToolConfig for session tools- fix: resolve 
gent from session_key inste
d of Worksp
ceM
n
ger- fix: sep
r
te 
gent identity files from worksp
ce directory- fix: use bold *n
me* for 
gent prefix inste
d of [n
me]- fix: use M
rkdown (leg
cy) inste
d of M
rkdownV2 for Telegr
m mess
ges- fix: remove b
cksl
sh esc
ping from 
gent n
me prefix in replies- fix: override rel
tive working_dir with 
gent worksp
ce- fix: ch
nge def
ult worksp
ce root from 
gents/ to worksp
ces/- fix: def
ult b
sh/code_exec working directory to 
gent worksp
ce- fix: register JSON Schem
 for 
ll builtin tools + Codex protocol 
lignment- fix: prevent token regener
tion on HMAC mism
tch to protect v
ult secrets- fix: Codex SSE function_c
ll_
rguments delt
 collection + logging- fix: use v
ult_key() function inste
d of undefined VAULT_KEY const
nt- fix: unify rer
nking v
ult key form
t with other modules- fix: rer
nking P
nel fetches per-provider API key from v
ult- fix: cle
r 
pi_key from rer
nking config sign
l 
fter s
ve- fix: isol
te rer
nk API keys per provider in v
ult- fix: move rer
nk API key from config.toml to encrypted v
ult- fix: correct def
ult rer
nking model n
me in P
nel 
nd tests- fix: ACP p
nel buttons h
ng due to sp
wn_loc
l context loss- fix: ACP test/s
ve button h
ng 
nd preset mode def
ults- fix: ACP p
nel gemini preset ID mism
tch 
nd test button h
ng- fix: resolve 
ll 75 compil
tion errors from provider routing ref
ctor- fix: v
ult-b
cked provider API keys 
nd config h
ndler improvements- fix(
cp): 
d
pt h
rnesses to re
l CLI protocols 
fter e2e probe testing- fix: worksp
ce schem
 migr
tion, worksp
ce.getActive response, 
nd providers p
ge freeze- fix: remove redund
nt binding in ConfigP
tcher- fix: session history, 
gent.list RPC, 
nd embedding dedup- fix: count only running runs for concurrency limit, reduce cle
nup del
y- fix: 
dd multi-dimension vector columns to memories t
ble schem
- fix: hot-sw
p runtime provider when switching def
ult vi
 P
nel UI- fix: resolve ch
t qu
lity issues — bootstr
p, esc
l
tion, 
nd response form
t- fix: resolve pre-existing test compil
tion errors- fix: wire missing RPC h
ndlers 
nd correct TUI method n
mes- fix: upd
te rem
ining port 18789 references to 18790- fix: unify ch
nnel config persistence — P
nel UI s
ve/lo
d/connect now works- fix: resolve compil
tion errors from fe
ture fl
g remov
l- fix(desktop): 
ddress fin
l review — version 
lignment, input v
lid
tion, Unicode- fix(desktop): 
ddress clippy needless-borrow w
rning in 
gent h
ndler- fix(desktop): 
ddress code qu
lity review — v
lid
tion, 
pprov
l g
tes- fix(desktop): wire N
tiveDesktop into registry + complete re-exports- fix: logic review R2 
rchitecture — 14 findings 
cross 5 c
tegories- fix: logic review R2 — 29 files 
cross 4 priority b
tches- fix: 
ddress code review findings for self-configur
tion- fix: RAII sem
phore gu
rd 
nd env v
r exp
nsion ordering (Known Issues)- fix: repl
ce std::sync::RwLock with cr
te::sync_primitives (P2-15)- fix: sort H
shM
p-derived collections for deterministic ordering (P2-14)- fix: repl
ce SystemTime UNIX_EPOCH .unwr
p() with .unwr
p_or_def
ult() (P2-12)- fix: rele
se locks before 
w
iting in 4 
sync p
tterns (P2-11)- fix: norm
lize t
sk_type 
nd t
sk_id in SessionKey::t
sk() (P1-9)- fix: use bounded c
st for POE token count u32 conversion (P1-8)- fix: resolve rem
ining UTF-8 byte slicing p
nics (P1-7)- fix: ConfigP
tcher use s
ve_increment
l 
nd h
rd-error on conflict- fix: logic review Ph
se 6 — 45 fixes 
cross g
tew
y, memory, poe, exec, providers, 
nd 15 more modules- fix: resolve 5 rem
ining W
rning-level issues from logic review Ph
se 5- fix: logic review Ph
se 4 — 18 fixes 
cross d
emon, engine, secrets, skills, components, cron- fix: resolve 5 Known Issues from logic review- fix: comprehensive logic review fixes 
cross 53 files in 77 modules- fix: use cfg(fe
ture = "loom") inste
d of cfg(loom) to 
void poisoning dependencies- fix(g
tew
y): elimin
te TOCTOU in execution_engine concurrent run limit check- fix(g
tew
y): use Mutex for ch
nnel_registry t
ke-once inbound_rx p
ttern- fix(resilience): simplify governor session_tokens from AtomicU64 to u64- fix: upd
te doctest to use poe::met
_cognition::Beh
vior
lAnchor- fix: 
dd Clone derive to NoiseFilter 
nd remove duplic
te mod decl
r
tions- fix: remove duplic
te scoring_pipeline module decl
r
tion in memory/mod.rs- fix(clippy): resolve print_liter
l w
rnings in secret providers comm
nd- fix(tests): migr
te secret_bound
ry_integr
tion tests to 
sync- fix(runtimes): 
ddress critic
l 
nd import
nt code review findings- fix: resolve 
ll clippy w
rnings in 
leph-t
uri 
nd 
lephcore- fix(desktop): use ERR_NOT_IMPLEMENTED for stubbed methods, 
dd debug logging- fix(h
lo): 
ddress code review findings for view 
nd events- fix(h
lo): gu
rd 
g
inst empty run_id in event h
ndler- fix(h
lo): use monotonic counter for unique mess
ge IDs, remove redund
nt ph
se gu
rd- fix(desktop): restrict UDS socket to owner-only 
ccess- fix(desktop): 
dd 30s timeout to UDS request to prevent indefinite t
sk h
ng- fix(desktop): log ev
lu
teJ
v
Script errors in C
nv
s, 
dd runAsync m
in-thre
d 
ssert- fix(desktop): repl
ce deprec
ted 
ctiv
te(options:) with 
ctiv
te() for m
cOS 15- fix(desktop): 
void PNG round-trip in OCR p
th by sh
ring c
ptureCurrentScreen- fix: 
ddress code review findings- fix(desktop): repl
ce strcpy with strncpy to prevent buffer overflow- fix(desktop): require x/y for click 
nd window_id for focus_window- fix(desktop): remove misle
ding serde t
gs from DesktopRequest, 
dd From conversions- fix(skill): 
ddress code review findings- fix(skill): resolve clippy w
rnings in skill module- fix(skill): use single colon sep
r
tor for SkillId (m
tches OpenCl
w convention)- fix(st
rt): 
dd cfg gu
rd for builder mod, tighten h
ndler visibility to pub(in cr
te::comm
nds::st
rt)- fix(st
rt): move session b
nner print into register_session_h
ndlers for consistency- fix: resolve 
ll compil
tion errors from server purific
tion- fix: cle
n up rem
ining Server-Client terminology in source comments- fix: rep
ir 2 broken doc-tests in skill_evolution module- fix: resolve 8 pre-existing test f
ilures- fix(control-pl
ne): document AlertsApi integr
tion limit
tion- fix(control-pl
ne): complete mock d
t
 remov
l- fix(control-pl
ne): fix memory le
ks 
nd improve error h
ndling in 
lert subscriptions- fix(sh
red-ui-logic): improve error h
ndling in 
lerts API- fix(control-pl
ne): use T
ilwind CDN for CSS compil
tion- fix(control-pl
ne): 
dd WASM initi
liz
tion in lib.rs- fix(control-pl
ne): upd
te st
rtup log mess
ge to show correct URL- fix(control-pl
ne): fix root p
th 
ccess 
nd st
tic 
sset lo
ding- fix: resolve compil
tion errors 
nd 
dd missing imports- fix(d
shbo
rd): 
dd w
sm_bindgen entry point to en
ble 
pp initi
liz
tion- fix(g
tew
y): extr
ct guest_session_id when require_
uth=f
lse- fix: resolve compil
tion errors in 
uth 
nd guest h
ndlers- fix: use rowid inste
d of id for sqlite-vec virtu
l t
ble upd
tes- fix(ph
se2): fix RPC tests 
nd upd
te progress report- fix(cli): use correct method n
mes for session comm
nds- fix(cli): resolve event stre
ming issue between g
tew
y 
nd CLI- fix(cli): 
lign comm
nd h
ndlers with g
tew
y API- fix(memory): h
ndle new SubAgent F
ctType v
ri
nts in consolid
tion- fix: resolve f
iling BDD tests for embodiment 
nd CoT tr
nsp
rency- fix: resolve f
iling unit tests- fix: resolve module export 
nd test compil
tion errors- fix: resolve 
ll 29 compiler w
rnings- fix: 
dd dylib.* p
ttern to gitignore- fix: upd
te .gitignore for Aleph ren
me 
nd remove dylib from tr
cking- fix(compressor): fix string conc
ten
tion in tests- fix(protocols): error on nonexistent JSONP
th inste
d of returning null- fix(scr
tchp
d): use EAFP p
ttern inste
d of sync exists() checks- fix(scr
tchp
d): remove 
sync from exists() 
nd export Scr
tchp
dConfig- fix(core): fix form
t strings in m
nifest.rs 
nd doctest in pty.rs- fix: cle
n up rem
ining MultiTurnCoordin
tor references- fix(g
tew
y): remove MultiTurnCoordin
tor dependency from 
d
pter- fix(h
lo): upd
te DependencyCont
iner comment for H
loInputCoordin
tor- fix(h
lo): upd
te AppDeleg
te to use H
loInputCoordin
tor- fix(h
lo): upd
te HotkeyService to use H
loInputCoordin
tor- fix: upd
te tests for 5 builtin tools 
nd skill evolution- fix: compil
tion errors in skill evolution 
nd perception modules- fix: resolve test compil
tion errors### Ch
nged- ref
ctor: ren
me ch
tgpt → codex protocol 
cross codeb
se- ref
ctor: ren
me ToolGroup → ToolC
tegory to 
void confusion with Te
m- ph
se4: cle
n 
ll T
uri references from codeb
se- ph
se4: remove T
uri, 
rchive old 
pps, move Swift bridge to cr
tes/desktop-m
cos/bridge- ref
ctor: move CLI/TUI/WebCh
t to interf
ces/, client to sh
red/- cle
nup: remove bootstr
p 
uto-clone 
nd leg
cy plugin index code- cle
nup: remove AgentLifecycleEvent::Switched 
nd AgentRouter from inbound router- cle
nup: remove 
gent switching (tool, intent detector, /switch comm
nd)- cle
nup: remove unregistered self-m
n
gement tool source files- cle
nup: remove old sub
gent tools (sp
wn/steer/kill + deleg
te)- cle
nup: move e2e tests into tests/, remove unused sh
red_ui_logic cr
te, 
dd secret sc
nning exclusion- cle
nup: remove tempor
ry debug logging for ch
tgpt protocol- ref
ctor: ren
me worksp
ce to 
gent 
cross memory/config/p
ths, enh
nce 
gent loop 
nd Ch
tGPT protocol- cle
nup: remove zombie code, upd
te def
ult config 
nd sh
red_ui_logic- cle
nup: remove st
le ALEPH_MASTER_KEY references from docs 
nd error mess
ges- ref
ctor: fl
tten 
gent_loop/ — remove minim
l/ subdirectory- cle
nup: remove deprec
ted APIs (register_
gent_tools, with_working_dir, ToolC
tegory::N
tive, PolicyEngine stubs, AuditStore, Inv
lid
teOld)- ref
ctor: ren
me Minim
l* types to st
nd
rd n
mes — this IS the loop- cle
nup: fix clippy w
rning in leg
cy_
d
pter detect_entry_point- cle
nup: elimin
te 
ll clippy w
rnings (58→0)- cle
nup: fix clippy w
rnings (derive Def
ult, redund
nt closures, simplified condition
ls)- cle
nup: remove st
le 
pp_bundle_id references from comments 
nd BDD tests- cle
nup: remove TypeScript webch
t (repl
ced by P
nel /ch
t route)- cle
nup: remove de
d Sub
gentAuthority 
nd tools/sessions dom
in l
yer- ref
ctor: simplify memory types, use floor_ch
r_bound
ry, 
dd mtime c
che to d
ily memory- ref
ctor(pdf): split pdf_gener
te.rs into module directory- ref
ctor: strip #[cfg(fe
ture)] from g
tew
y, server, extension, 
nd misc modules- ref
ctor: strip #[cfg(fe
ture)] from 
ll 12 ch
nnel implement
tions- ref
ctor: strip 20+ C
rgo fe
ture fl
gs from core cr
te- ref
ctor: Occ
m's R
zor p
ss — elimin
te clippy w
rnings 
nd de
d code- cle
nup: remove f
stembed 
nd loc
l embedding model remn
nts- cle
nup: fix unused import in host_functions.rs- ref
ctor(w
sm): simplify PermissionChecker to f
c
de over W
smC
p
bilities- cle
nup: bro
d DRY ref
ctoring 
nd clippy compli
nce 
cross codeb
se- cle
nup: remove st
le f
stembed references, fix integr
tion tests- cle
nup: remove m
cOS-specific CI workflow 
nd build scripts (C8-C12)- cle
nup: remove deprec
ted m
cOS Swift 
pp (C7)- cle
nup: remove UniFFI Swift bindings (C1-C2)- ref
ctor(core): introduce register_h
ndler! m
cro, elimin
te h
ndler boilerpl
te (W
ve 4)- ref
ctor(core): repl
ce &Vec<T> with &[T] in 
rrow_convert 
nd sh
dow_repl
y (W
ve 3B)- ref
ctor(core): convert Intern
lEventH
ndler String p
r
ms to &str (W
ve 3A)- ref
ctor(core): m
nu
l Clippy fixes — expect_fun_c
ll, useless_vec, ptr_
rg, type_complexity, module_inception, needless_borrows, 
nd more (W
ve 2B)- ref
ctor(core): repl
ce Def
ult::def
ult() field re
ssignment with struct liter
ls (W
ve 2A)- ref
ctor(core): 
uto-fix Clippy w
rnings 
nd remove unused imports (W
ve 1)- ref
ctor(runtimes): delete old runtime m
n
gers, repl
ce with Ledger/Probe system- ref
ctor(video): repl
ce RuntimeRegistry with C
p
bilityLedger in c
ption.rs- ref
ctor(init): repl
ce forced runtime inst
ll
tion with zero-inst
ll ledger- ref
ctor(desktop): delete RPC proxy comm
nds 
nd cle
n up de
d code (~1600 lines)- ref
ctor(h
lo): delete Re
ct frontend source from T
uri 
pp- ref
ctor(h
lo): point T
uri h
lo window to Leptos server URL- ref
ctor(h
lo): delete leg
cy Swift H
lo views 
nd fix references (~4500 lines removed)- ref
ctor(st
rt): split initi
lize_
uth, extr
ct lo
d_
pp_config, restore register c
lls to orchestr
tor- ref
ctor(st
rt): move register_* h
ndler functions to comm
nds/builder/h
ndlers.rs- ref
ctor(extension): thin mod.rs f
c
de, deleg
te lo
d_
ll to ComponentLo
der- ref
ctor(st
rt): extr
ct subsystem initi
lizers from st
rt_server- ref
ctor: remove distributed execution infr
structure (ExecutionPolicy, ClientM
nifest, ReverseRpc, ToolRouter, RoutedExecutor)- ref
ctor: cle
n up 
uth h
ndler by removing ClientM
nifest references- ref
ctor: simplify g
tew
y server by removing client routing infr
structure- ref
ctor: simplify ExecutionEngine by removing client routing- ref
ctor: ren
me g
tew
y/ch
nnels/ to g
tew
y/interf
ces/- ref
ctor: ren
me clients/ to 
pps/- cle
nup: remove unused imports from exec_security_g
te (post-reb
se)- cle
nup: fix Arc misuse, l
rge v
ri
nts, 
nd priv
te interf
ces (P
ss 3 fin
l)- cle
nup: extr
ct type 
li
ses 
nd p
r
meter structs (P
ss 3)- cle
nup: suppress module_inception for intention
l nested module p
ttern- cle
nup: fix 22 miscell
neous clippy w
rnings- cle
nup: P
ss 2 loc
l ref
ctoring (clone, strip_prefix, de
d code, redund
nt closures)- cle
nup: fix boole
n simplific
tions, identity ops, 
nd &P
thBuf sign
tures- cle
nup: remove unused imports 
nd repl
ce deriv
ble impls- cle
nup: 
pply c
rgo clippy --fix 
uto-corrections- ref
ctor(control-pl
ne): split Sideb
r into sideb
r/ directory- ref
ctor(control-pl
ne): use nested routes for Settings with SettingsL
yout- ref
ctor(control-pl
ne): remove /cp prefix from routing- ref
ctor(core): ren
me 
leph-g
tew
y to 
leph-server- ref
ctor(m
cos): completely remove settings UI from m
cOS client- ref
ctor(desktop): completely remove settings UI from T
uri client- ref
ctor(desktop): migr
te Plugins, Skills, 
nd Policies settings to D
shbo
rd- ref
ctor(clients): complete Ph
se 4 - remove Gener
tion Providers UI- ref
ctor(clients): migr
te Providers, Memory, 
nd MCP config to D
shbo
rd- ref
ctor(
gent_loop): introduce RunContext p
ttern for cle
ner API- ref
ctor(
gent-loop): 
dd RunContext structure (WIP)- ref
ctor(dom
in): implement Newtype p
ttern for Answer 
nd Ruleset- ref
ctor(dom
in): implement Newtype p
ttern for 5 ID types- ref
ctor(
pi): implement FromStr tr
it for rem
ining types- ref
ctor(
pi): implement FromStr tr
it for extension 
nd resilience types- ref
ctor(
pi): implement FromStr tr
it for memory context types- ref
ctor(perf): repl
ce trim_st
rt_m
tches with strip_prefix for fixed prefixes- ref
ctor(perf): optimize &P
thBuf → &P
th in 6 files- ref
ctor(core): 
dd #[
llow(de
d_code)] to 12 reserved fields- ref
ctor(deps): remove 5 unused dependencies- ref
ctor(core): remove 2 confirmed de
d code items- ref
ctor(core): remove 160+ unused imports 
cross 50 files- ref
ctor(tools): extr
ct builtin tool registr
tion 
nd types (Ph
se 6)- ref
ctor(g
tew
y): modul
rize plugins h
ndlers (Ph
se 5.1)- ref
ctor(poe): extr
ct services to dedic
ted modules (Ph
se 4.2 - P1)- ref
ctor(poe): extr
ct h
ndler types to dedic
ted modules (Ph
se 4.1 - P0)- ref
ctor(browser): extr
ct types 
nd scripts modules (Ph
se 3 - P
rt 1)- ref
ctor(engine): complete 
tomic executor composition ref
ctoring (Ph
se 2)- ref
ctor(engine): 
dd 
tomic module b
se 
rchitecture (Ph
se 2 WIP)- ref
ctor(extension): split types.rs into modul
r structure- ref
ctor(security): tr
nsform PolicyEngine to st
teless- ref
ctor(protocol): 
dd equ
lity derives 
nd helper methods to 
uth types- ref
ctor(ph
se1): reorg
nize client directory structure- ref
ctor: complete fin
l Aether to Aleph cle
nup- ref
ctor: complete Aether to Aleph ren
me - scripts, workflows, 
nd rem
ining code- ref
ctor: complete Aether to Aleph ren
me 
cross entire codeb
se- ref
ctor(providers): use ProtocolRegistry in cre
te_provider f
ctory- ref
ctor(providers): remove technic
l 
li
s presets- ref
ctor(config): remove provider_type field from ProviderConfig- ref
ctor: fix P3 clippy w
rnings - b
tch 2- ref
ctor: fix P3 clippy w
rnings - b
tch 1- ref
ctor: fix P1/P2 clippy w
rnings 
nd improve code qu
lity- ref
ctor(providers): delete leg
cy OpenAiProvider- ref
ctor(providers): delete leg
cy GeminiProvider- ref
ctor(providers): delete leg
cy Cl
udeProvider- ref
ctor(providers): use HttpProvider for Anthropic protocol- ref
ctor(providers): remove redund
nt vendor wr
ppers (~850 lines)- ref
ctor(providers): use HttpProvider for OpenAI protocol in f
ctory- ref
ctor(m
cos): cle
nup 
nd improve hotkey/h
lo components- ref
ctor(h
lo): repl
ce H
loSt
te with simplified 6-st
te version- ref
ctor(h
lo): switch H
loWindow to V2 components- ref
ctor(h
lo): remove MultiTurn references from EventH
ndler- ref
ctor(h
lo): remove MultiTurn directory (~3000 lines)- ref
ctor: split l
rge modules into sm
ller files- cle
nup: remove unused modules 
nd merge thinking into thinker- cle
nup: elimin
te 
ll compil
tion w
rnings- cle
nup(lib): slim down exports from 590 to 272 lines- cle
nup: remove FFI-rel
ted comments- cle
nup: ren
me FFI types to st
nd
rd n
mes- cle
nup(disp
tcher): ren
me ffi.rs to tool_info.rs- cle
nup(intent): remove Type A FFI residu
ls### Build- build: unify version source — VERSION file drives 
ll version strings- rele
se: v0.2.8- docs: 
dd multimod
l probe tests implement
tion pl
n- docs: 
dd multimod
l probe tests design spec- docs: 
dd core multimod
l enh
ncement implement
tion pl
n- docs: fix spec review issues in core multimod
l design- docs: 
dd core multimod
l enh
ncement design spec- docs: 
dd Telegr
m ch
nnel enh
ncement implement
tion pl
n- docs: fix spec review issues in Telegr
m enh
ncement design- docs: 
dd Telegr
m ch
nnel enh
ncement design spec- docs: 
dd Feishu enh
nced fe
tures implement
tion pl
n- docs: 
ddress spec review — FeishuEventEmitter, typing lifecycle, c
p
bilities- docs: 
dd Feishu enh
nced fe
tures design spec- docs: 
dd Feishu ch
nnel implement
tion pl
n- docs: 
ddress spec review feedb
ck for Feishu ch
nnel- docs: 
dd Feishu/L
rk ch
nnel design spec- rele
se: v0.2.7 — multi-
gent system, UI upd
tes, bug fixes- docs: fix spec issues from review — st
le fin
l_text, test pl
n, consecutive_errors- docs: 
dd Persistent Completion Protocol design spec- docs: fix multi-
gent modes spec per review findings- docs: 
dd multi-
gent modes t
xonomy design spec- docs: 
dd t
sk coordin
tion implement
tion pl
n (12 t
sks)- docs: fix event type conventions in t
sk coordin
tion spec- docs: 
ddress spec review findings for t
sk coordin
tion- docs: 
dd t
sk coordin
tion system design spec- build: upd
te WASM p
nel dist- ci: upgr
de GitHub Actions to Node.js 24 comp
tible versions- ci: scope fmt check to m
int
ined cr
tes (skip leg
cy form
tting issues)- build: consolid
te to single rele
se workflow, fix CI protoc dependency- build: remove 
rchive from git (l
rge bin
ries exceed GitHub limit)- rele
se: bump version to 0.2.6- build: upd
te inst
ll scripts for 
leph-server bin
ry n
me- build: ren
me workflows, fix --bin 
leph→
leph-server, 
dd pl
tform rele
se workflows- build: upd
te justfile 
nd CI workflows for post-T
uri 
rchitecture- build: 
dd swift-bridge recipe to justfile for m
cOS n
tive APIs- docs: 
dd Ph
se 3 implement
tion pl
n for m
cOS PIM & system c
p
bilities- docs: 
dd Ph
se 2 implement
tion pl
n for screen control n
tive migr
tion- docs: 
ddress spec review feedb
ck for hier
rchic
l comm
nds- docs: 
dd hier
rchic
l sl
sh comm
nds design spec- docs: 
dd Ph
se 1 implement
tion pl
n for desktop n
tive c
p
bilities- docs: 
dd desktop n
tive c
p
bilities design spec- docs: upd
te design spec with new directory structure- docs: 
dd implement
tion pl
n for intermedi
te mess
ge delivery- docs: 
dd PLUGIN_SYSTEM.md — CC-comp
tible plugin 
rchitecture reference- docs: 
ddress spec review feedb
ck for CLI/TUI sep
r
tion- docs: 
dd CLI/TUI sep
r
tion design spec- docs: 
dd P4 runtime migr
tion implement
tion pl
n- docs: 
dd prompt guid
nce 
s in-scope ch
nges to intermedi
te mess
ge spec- docs: 
dd edge c
ses to intermedi
te mess
ge delivery spec- docs: 
dd intermedi
te mess
ge delivery design spec- docs: 
dd P3 scope m
n
gement implement
tion pl
n- docs: 
dd P2 m
rketpl
ce system implement
tion pl
n- docs: 
dd P0+P1 implement
tion pl
n for plugin CC comp
t- docs: fix rem
ining spec review items (round 2)- docs: 
ddress spec review findings for plugin comp
t design- docs: 
dd plugin system Cl
ude Code comp
tibility redesign spec- docs: upd
te spec 
nd pl
n — keep peer_id sign
tures unch
nged- docs: upd
te 
gent-bot 1:1 binding spec with review fixes- docs: 
dd 
gent-bot 1:1 binding simplific
tion design spec- docs: 
dd ch
t sideb
r redesign spec 
nd implement
tion pl
n- docs: 
dd p
nel 
gent routing fix design spec- docs: 
dd worksp
ce output migr
tion implement
tion pl
n- docs: revise worksp
ce output migr
tion spec 
fter review- docs: 
dd worksp
ce output migr
tion design spec- docs: 
dd gener
tion providers wiring implement
tion pl
n- docs: fix gener
tion providers spec 
fter review- docs: 
dd gener
tion providers wiring design spec- docs: 
dd Cl
wHub integr
tion implement
tion pl
n- docs: 
ddress spec review feedb
ck for Cl
wHub integr
tion- docs: 
dd Cl
wHub integr
tion design spec- ci: upgr
de GitHub Actions to Node.js 24, fix Windows de
d-code w
rnings- docs: fix pl
n review issues (3 blockers + 6 w
rnings)- docs: 
ddress spec review feedb
ck for Chrome DevTools MCP Mode- docs: 
dd Chrome DevTools MCP Mode design spec- docs: 
dd process m
n
gement rules to CLAUDE.md- docs: 
dd tool permission system implement
tion pl
n- docs: upd
te tool permission spec 
fter review- docs: 
dd tool permission system design spec- docs: 
dd ACP probe tests design document- docs: 
dd ACP h
rness m
n
gement implement
tion pl
n- docs: 
dd ACP h
rness m
n
gement design document- docs: 
dd provider routing ref
ctor implement
tion pl
n- docs: fix rem
ining spec review issues- docs: fix spec issues from review- docs: 
dd provider routing ref
ctor design spec- docs: 
dd provider config testing implement
tion pl
n- docs: upd
te provider config testing spec 
fter review- docs: 
dd provider config testing design spec- docs: 
dd simplify-model-config implement
tion pl
n- docs: upd
te simplify-model-config spec 
fter review- docs: 
dd simplify-model-config design spec- ci: re
d rele
se version from VERSION file inste
d of m
nu
l input- docs: 
dd cron probe tests implement
tion pl
n- docs: 
dd cron probe tests design spec- docs: 
dd cron module redesign implement
tion pl
n- docs: 
dd cron module redesign spec- build: rebuild p
nel WASM 
nd upd
te docs 
fter worktree merges- docs: 
dd provider zero-config implement
tion pl
n- docs: 
dd mess
ge pipeline implement
tion pl
n- docs: 
dd provider zero-config UX design spec- docs: 
dd mess
ge pipeline design for g
tew
y pre-processing- docs: 
dd model discovery probe tests implement
tion pl
n- docs: 
dd model discovery probe tests design spec- docs: 
dd model discovery implement
tion pl
n- docs: fix model discovery spec issues from review- docs: 
dd model discovery design spec- docs: 
dd cognitive evolution bet
 implement
tion pl
n- docs: 
dd cognitive evolution bet
 design (immune-complete loop)- docs: 
dd POE Ph
se 2+3 implement
tion pl
n- docs: 
dd POE Ph
se 1 implement
tion pl
n (Bl
stR
dius + T
boo)- docs: 
dd POE Architecture Evolution Whitep
per 2026- ci: fix Linux/Windows compil
tion errors for missing imports- docs: upd
te extension system 
rchitecture document
tion- docs: 
dd unified plugin system implement
tion pl
n- docs: 
dd unified plugin system design- docs: 
dd one-line inst
ll comm
nds 
s prim
ry inst
ll
tion method- docs: remove ref
ctoring b
ckstory from intent section- docs: upd
te intent detection section to reflect unified LLM pipeline- docs: 
dd det
iled Aleph vs OpenCl
w comp
rison- docs: 
dd P4.3 core plugins implement
tion pl
n- docs: 
dd plugin development guide- docs: 
dd P4 plugin ecosystem implement
tion pl
n- ci: 
dd Windows x86_64 build t
rget 
nd PowerShell inst
ller- docs: 
dd P3 medi
 pipeline implement
tion pl
n- ci: fix Linux w
rn import, remove d
rwin-x86_64 t
rget- ci: 
dd libxdo-dev for Linux, fix d
rwin x86_64 AVX-512 link error- ci: fix Linux pipewire comp
t (ubuntu-24.04) 
nd m
cOS x86_64 openssl- ci: 
dd libegl 
nd X11 extension deps for Linux build- ci: use m
cos-l
test for x86_64 cross-compile (m
cos-13 EOL)- ci: 
dd dbus, drm, gbm deps for Linux build- ci: 
dd pipewire 
nd cl
ng deps for Linux xc
p build- ci: 
dd libw
yl
nd-dev to Linux build dependencies- docs: 
dd 
uthor note to README- docs: ren
me p
nel screenshots with consistent numbering- docs: restore d
shbo
rd screenshot, keep 
ll 3 p
nel im
ges- docs: upd
te README screenshots with P
nel ch
t 
nd settings views- build: remove webch
t recipes from justfile- docs: 
dd webch
t Rust rewrite implement
tion pl
n- docs: 
dd webch
t Rust rewrite design- docs: remove 
cknowledgments section from README- ci: en
ble 
ll pl
tform build t
rgets for server rele
se- ci: 
dd m
nu
l server rele
se workflow 
nd improve inst
ll script- docs: overh
ul README.md, CLAUDE.md 
nd 
dd LICENSE- docs: 
dd inline directives 
nd leg
cy cle
nup implement
tion pl
n- docs: 
dd inline directives 
nd leg
cy cle
nup design- docs: 
dd l
ngu
ge-
gnostic intent detection implement
tion pl
n- docs: 
dd l
ngu
ge-
gnostic intent detection design- docs: upd
te cle
nup pl
n with execution results- docs: cl
rify cle
nup str
tegy — scoped responsibility, not f
llb
ck- docs: 
dd multi-
gent code redund
ncy cle
nup pl
n- docs: 
dd A2A protocol implement
tion pl
n- docs: 
dd A2A protocol design document- docs: 
dd per-
gent tool configur
tion implement
tion pl
n- docs: 
dd per-
gent tool configur
tion design- docs: 
dd multi-bot P
nel UI implement
tion pl
n- docs: 
dd multi-bot P
nel UI design- docs: 
dd multi-bot ch
nnel implement
tion pl
n- docs: 
dd multi-bot ch
nnel support design- docs: 
dd memory 
lignment design for du
l-directory 
rchitecture- docs: 
dd 
gent-worksp
ce sep
r
tion implement
tion pl
n- docs: 
dd 
gent-worksp
ce sep
r
tion design- docs: 
dd 
gent m
n
gement p
nel implement
tion pl
n- docs: 
dd 
gent m
n
gement p
nel design- docs: 
dd webch
t restructure implement
tion pl
n- docs: 
dd webch
t restructure design- docs: 
dd 
gent switching enh
ncement implement
tion pl
n- docs: 
dd 
gent switching enh
ncement design- docs: 
dd unified comm
nd registry implement
tion pl
n- docs: 
dd unified comm
nd registry design- docs: 
dd dyn
mic 
gent switching implement
tion pl
n- docs: 
dd dyn
mic 
gent switching design- docs: 
dd system prompt optimiz
tion implement
tion pl
n- docs: 
dd system prompt 
rchitecture optimiz
tion design- docs: 
dd Agent/Worksp
ce/Session unific
tion implement
tion pl
n- docs: 
dd Agent/Worksp
ce/Session rel
tionship design- docs: 
dd t
sk routing decision l
yer implement
tion pl
n- docs: 
dd t
sk routing decision l
yer design- docs: 
dd 
rchitecture 
ctiv
tion di
gnostic report- docs: 
dd 
rchitecture 
ctiv
tion di
gnostic implement
tion pl
n- docs: 
dd 
rchitecture 
ctiv
tion di
gnostic design- docs: 
dd n
tive tool_use implement
tion pl
n (9 t
sks)- docs: 
dd n
tive tool_use migr
tion design- docs: 
dd PDF du
l-engine implement
tion pl
n- docs: 
dd PDF du
l-engine rendering design- docs: 
dd cron 
nd group ch
t b
ckend implement
tion pl
n- docs: 
dd cron 
nd group ch
t b
ckend implement
tion design- docs: 
dd scheduled t
sks p
nel implement
tion pl
n- docs: 
dd scheduled t
sks p
nel design- docs: 
dd CLI full RPC cover
ge implement
tion pl
n- docs: 
dd CLI full RPC cover
ge design- docs: 
dd CLI bugfix 
nd JSON unific
tion design- docs: 
dd CLI full comm
nds implement
tion pl
n- docs: 
dd CLI full comm
nds design- docs: 
dd CLI infr
structure enh
ncement implement
tion pl
n- docs: 
dd CLI infr
structure enh
ncement design- docs: 
dd lifecycle observ
bility logging implement
tion pl
n- docs: 
dd lifecycle observ
bility logging design- docs: 
dd system prompt enh
ncement implement
tion pl
n- docs: 
dd system prompt enh
ncement design- docs: 
dd 
gent system Ph
se 2 full cover
ge implement
tion pl
n- docs: 
dd 
gent system full cover
ge design (Ph
se 2)- docs: 
dd Codex p
nel UI design 
nd implement
tion pl
n- docs: 
dd Codex Responses API implement
tion pl
n- docs: 
dd Codex Responses API protocol 
d
pter design- docs: 
dd g
tew
y enh
ncement implement
tion pl
n (20 t
sks)- docs: 
dd g
tew
y enh
ncement design (OpenCl
w-inspired)- docs: 
dd implement
tion pl
n for 
gent/worksp
ce/binding- docs: 
dd 
gent definition + worksp
ce + binding design- docs: 
dd OpenAI subscription provider implement
tion pl
n- docs: 
dd OpenAI subscription provider design- docs: 
dd L
zy POE Activ
tion design- build: ren
me just server → just build, 
dd just 
ll- docs: upd
te bin
ry n
me 
nd port references 
cross 
ll document
tion- build: en
ble 
xum ws fe
ture for port unific
tion- docs: 
dd port unific
tion implement
tion pl
n- docs: 
dd port unific
tion 
nd bin
ry ren
me design- docs: 
dd ch
nnel infr
structure fix implement
tion pl
n- docs: 
dd ch
nnel infr
structure fix design- docs: upd
te CLAUDE.md for fe
ture fl
g remov
l- build: simplify justfile — remove 
ll --fe
tures fl
gs- docs: 
dd runtime ch
nnel control implement
tion pl
n- docs: 
dd runtime ch
nnel control design — elimin
te fe
ture fl
g fr
gment
tion- docs: 
dd ch
t persistence & memory pipeline implement
tion pl
n- docs: 
dd ch
t persistence & memory pipeline fix design- docs: 
dd full ch
in + sm
rt rec
ll implement
tion pl
n- docs: 
dd full ch
in + sm
rt rec
ll design- docs: 
dd worksp
ce enh
ncements implement
tion pl
n (9 t
sks)- docs: 
dd worksp
ce enh
ncements design (4 fe
tures)- docs: 
dd worksp
ce wiring implement
tion pl
n (11 t
sks)- docs: 
dd worksp
ce wiring design for multi-role person
 system- docs: 
dd config extern
liz
tion implement
tion pl
n- docs: 
dd config extern
liz
tion design for ~/.
leph worksp
ce- ci: keep only m
cOS ARM64 build, document other pl
tform blockers- ci: fix rem
ining build issues 
cross pl
tforms- ci: fix cross-pl
tform build issues- ci: pin w
sm-bindgen-cli to 0.2.108 m
tching C
rgo.lock- ci: 
llow test job to f
il without blocking builds- ci: 
dd X11/xscrns
ver dev libr
ries for Linux builds- ci: inst
ll protoc for l
nce-encoding build dependency- ci: improve rele
se workflow with WASM build, test job, 
nd cross-pl
tform desktop- build: rewrite justfile for desktop-
s-muscle 
rchitecture- docs: 
dd cr
tes/desktop to project structure 
nd build comm
nds- docs: 
dd Desktop-
s-Muscle implement
tion pl
n- docs: 
dd Desktop-
s-Muscle 
rchitecture design- docs: 
dd self-configur
tion implement
tion pl
n- docs: 
dd self-configur
tion design document- ci: 
dd loom concurrency test job 
nd incre
se proptest cover
ge- build: 
dd test-proptest, test-loom, test-logic just recipes- docs: 
dd logic review system implement
tion pl
n (15 t
sks, 49 properties)- docs: 
dd logic review system design (three-l
yer defense 
rchitecture)- docs: move obsolete embedding/sqlite-vec pl
ns to leg
cy- docs: upd
te memory system docs to reflect remote embedding migr
tion- build: repl
ce trunk with m
nu
l WASM pipeline in justfile- docs: fix m
cOS Resources p
th in build pipeline design- build: 
dd justfile for unified build pipeline- docs: 
dd unified build pipeline design- docs: 
dd ch
nnel config p
nel implement
tion pl
n- docs: 
dd ch
nnel config p
nel design document- docs: 
dd POE full evolution implement
tion pl
n (19 t
sks, 4 ph
ses)- docs: 
dd POE full evolution design (event-driven closed loop)- docs: 
dd WASM c
p
bility kernel implement
tion pl
n- docs: 
dd WASM c
p
bility kernel design- docs: 
dd m
cOS PIM n
tive API implement
tion pl
n- docs: 
dd m
cOS PIM n
tive API integr
tion design- docs: 
dd POE cognitive hub implement
tion pl
n- docs: 
dd POE cognitive hub upgr
de design- docs: 
dd soci
l bot ch
nnels exp
nsion implement
tion pl
n- docs: 
dd soci
l bot ch
nnels exp
nsion design- docs: 
dd surgic
l DRY ref
ctoring implement
tion pl
n- docs: 
dd surgic
l DRY ref
ctoring design for embedding provider files- docs: 
dd embedding provider LLM migr
tion implement
tion pl
n- docs: 
dd embedding provider LLM migr
tion design- docs: 
dd l
rge file ref
ctoring implement
tion pl
n — 6 t
sks, 5 files- docs: 
dd l
rge file ref
ctoring design — 5 files, pure module splitting- ci: 
dd server, m
cOS 
pp, 
nd T
uri rele
se workflows- docs: 
dd distribution implement
tion pl
n (24 t
sks, 9 ph
ses)- docs: 
dd distribution 
rchitecture design- docs: 
dd PromptPipeline implement
tion pl
n — 10 t
sks, TDD, str
ngler fig- docs: 
dd PromptPipeline design — Tr
it-per-L
yer evolution from Pl
n A- docs: 
dd 
utom
tion skills implement
tion pl
n- docs: 
dd 
utom
tion skills (#21-30) design- docs: 
dd memory event sourcing implement
tion pl
n- docs: 
dd memory event sourcing design (CQRS Light)- docs: 
dd prompt system enh
ncement implement
tion pl
n- docs: 
dd prompt system enh
ncement design- docs: 
dd skills system, upd
te runtimes refs, 
dd m
cOS components- docs: upd
te 
ccept
nce results 
fter bridge fixes (27/30 p
ss)- docs: 
dd implement
tion pl
n for fixing bridge known issues- docs: 
dd design for fixing bridge known issues- docs: remove rem
ining Swift references from CLAUDE.md- docs: upd
te CLAUDE.md 
nd cre
te migr
tion completion record (C13-C16)- docs: 
dd m
cOS Swift 
pp remov
l implement
tion pl
n- docs: 
dd m
cOS Swift 
pp remov
l design with 
ccept
nce criteri
- docs: 
dd desktop c
p
bilities evolution implement
tion pl
n- docs: 
dd desktop c
p
bilities evolution design- docs: 
dd sem
ntic t
rgeting implement
tion pl
n- docs: 
dd sem
ntic t
rgeting 
nd 
ction primitives design- docs: upd
te CLAUDE.md for Server-Centric Build Architecture- docs: 
dd Ph
se 3 
nd Ph
se 4 implement
tion pl
ns- docs: repl
ce Ghost 
esthetic with concrete product constr
ints R5-R7- docs: 
dd Ph
se 2.5 bridge integr
tion completion pl
n- docs: 
dd design for removing Ghost 
esthetic concept- docs: 
dd Ph
se 1 bridge skeleton implement
tion pl
n- docs: 
dd server-centric build 
rchitecture design- docs: upd
te worktree guidelines with EnterWorktree CWD lock c
ve
t- docs: 
dd cron system redesign pl
n — surp
ssing opencl
w- docs: 
dd memory optimiz
tion implement
tion pl
n- docs: 
dd memory module optimiz
tion design- docs: 
ddress code review findings (JIT-
pprov
l TODO, RwLock r
tion
le)- docs: bring in L
te-Binding Secure Execution design 
nd pl
n from m
in- docs: 
dd L
te-Binding Secure Execution implement
tion pl
n (14 t
sks, 4 w
ves)- docs: 
dd L
te-Binding Secure Execution Architecture design- docs: 
dd git worktree s
fety guide; fix missing ScreenRegion import- docs: 
dd Rust ref
ctoring implement
tion pl
n (7 t
sks, 4 w
ves)- docs: 
dd Rust core ref
ctoring design (4-w
ve str
tegy)- docs: 
dd runtime on-dem
nd implement
tion pl
n (13 t
sks, 4 ph
ses)- docs: 
dd runtime on-dem
nd implement
tion pl
n (13 t
sks, 4 ph
ses)- docs: 
dd runtime on-dem
nd n
tive bootstr
pping 
rchitecture design- docs: 
dd verific
tion test results to T
uri shell design doc- docs: 
dd T
uri cross-pl
tform shell implement
tion pl
n- docs: 
dd T
uri cross-pl
tform shell & DesktopBridge design- build(h
lo): rebuild WASM with /h
lo route- docs: split CLAUDE.md 
nd reorg
nize docs/ into docs/reference/- docs: 
dd 1-2-3-4 
rchitecture constitution design document- docs: 
dd H
lo UI Unific
tion implement
tion pl
n (10 t
sks)- docs: est
blish 1-2-3-4 
rchitecture model 
s constitution
l principles in CLAUDE.md- build(m
cos): 
dd WebKit fr
mework dependency for Settings WebView- docs: 
dd Ph
se 1 implement
tion pl
n — Settings WebView integr
tion- docs: 
dd UI unific
tion design — Leptos 
s single UI codeb
se- docs: 
dd Desktop Bridge implement
tion pl
n (11 t
sks, 4 ph
ses)- docs: 
dd Desktop Bridge design for UDS-b
sed Swift-Rust IPC- docs: 
dd Skill System v2 implement
tion pl
n (15 TDD t
sks)- docs: 
dd Skill System v2 design (complete DDD rebuild)- docs: upd
te 
ll document
tion for server-centric 
rchitecture- docs: upd
te CLAUDE.md for server-centric 
rchitecture- docs: 
dd server purific
tion implement
tion pl
n- docs: 
dd server purific
tion design - remove desktop control, embr
ce MCP plugins- docs: 
dd Skill System implement
tion pl
n with 14 TDD t
sks- docs: 
dd server-centric 
rchitecture implement
tion pl
n- docs: 
dd server-centric 
rchitecture refr
ming design- docs: 
dd Skill System dom
in-driven design document- docs: 
dd P0 ref
ctoring implement
tion pl
n for st
rt.rs 
nd extension/mod.rs- docs: 
dd CODE_ORGANIZATION guide with ref
ctoring b
cklog- docs: 
dd soci
l connectivity evolution design 
nd implement
tion pl
n- build: 
dd missing imports in control-pl
ne cfg block- docs: 
dd IronCl
w Ph
se 2/3 det
iled implement
tion pl
n- docs: 
dd IronCl
w Ph
se 2/3 design (host-bound
ry + EVM signing)- docs: 
dd code cle
nup implement
tion pl
n (16 t
sks, 3 p
sses)- docs: 
dd code cle
nup design pl
n (Occ
m's R
zor P
ss)- docs: 
dd ACMA implement
tion pl
n with 7 TDD t
sks- docs: 
dd ACMA (Aleph Cognitive Memory Architecture) design document- docs: 
dd exec security integr
tion design- docs: 
dd blog post on PII filtering g
tew
y implement
tion- docs: 
dd 
gent secret m
n
gement implement
tion pl
n- docs: 
dd 
gent secret m
n
gement design (Ph
se 1)- docs: 
dd Discord Control Pl
ne implement
tion pl
n- docs: 
dd Discord Control Pl
ne p
nel design- docs: 
dd memory worksp
ce implement
tion pl
n- docs: 
dd memory worksp
ce isol
tion design- docs: upd
te 
rchitecture docs to reflect L
nceDB migr
tion- docs: 
dd Wh
tsApp Bridge implement
tion pl
n (10 t
sks)- docs: 
dd Wh
tsApp Bridge design (Thin Sidec
r + Rich Ad
pter)- docs: upd
te MEMORY_SYSTEM.md 
nd CLAUDE.md for L
nceDB migr
tion- docs: embedding evolution implement
tion pl
n (13 t
sks)- docs: embedding evolution design (
bstr
ct provider + l
zy migr
tion)- docs: 
dd Memory VFS Evolution implement
tion pl
n- docs: 
dd Memory VFS Evolution design document- docs: 
dd Sw
rm Agent Loop integr
tion implement
tion pl
n- docs: 
dd Sw
rm Intelligence Architecture Agent Loop integr
tion design- docs(ssb): 
dd Ph
se 6 cross-pl
tform implement
tion pl
n- docs(ssb): 
dd cross-pl
tform 
rchitecture design- docs: cl
rify server-side execution model in CLAUDE.md- docs(ssb): 
dd Ph
se 6 enh
ncement pl
n 
nd complete ro
dm
p- docs: 
dd Sw
rm Intelligence Architecture design- build(control-pl
ne): upd
te compiled UI 
ssets for Ph
se 3- docs: 
dd System St
te Bus (SSB) 
rchitecture design- docs(skill-evolution): 
dd comprehensive document
tion 
nd ex
mples- docs: 
dd Coll
bor
tive Skill Evolution 
rchitecture design- docs: 
dd det
iled implement
tion pl
n for Control Pl
ne three-column l
yout- docs: 
dd Control Pl
ne three-column l
yout 
rchitecture design- docs: upd
te Control Pl
ne UI build workflow with T
ilwind CSS compil
tion- docs(cl
ude.md): 
dd WASM initi
liz
tion mech
nism expl
n
tion- docs(cl
ude.md): 
dd comprehensive Server development 
nd deployment guide- docs: 
dd UI comp
rison 
n
lysis for ControlPl
ne 
nd T
uri settings- docs: 
dd WebSocket client implement
tion summ
ry 
nd migr
tion pl
n- docs: 
dd ControlPl
ne integr
tion implement
tion summ
ry- docs: 
dd Ph
se 3 implement
tion pl
n- docs: 
dd Ph
se 3 design for skill s
ndboxing- docs: 
dd comprehensive skill s
ndboxing document
tion- docs: 
dd Ph
se 2 skill s
ndboxing implement
tion pl
n- docs: 
dd Ph
se 2 skill s
ndboxing design document- docs(sh
red-ui-logic): m
rk API L
yer 
s complete- docs(sh
red-ui-logic): m
rk WASM connector 
s complete- docs(sh
red-ui-logic): upd
te README with API 
nd Observ
bility progress- docs(sh
red_ui_logic): upd
te README with protocol l
yer st
tus- docs(sh
red_ui_logic): upd
te README with n
tive connector st
tus- docs(sh
red_ui_logic): 
dd comprehensive README- docs: 
dd sh
red_ui_logic design document- docs: complete Ph
se 3 
rchitecture document
tion- docs: 
dd Ph
se 1 implement
tion pl
n for skill s
ndboxing- docs: 
dd skill s
ndboxing 
rchitecture design- docs(
rchitecture): 
dd comprehensive cle
nup design document- docs: reorg
nize root directory 
nd est
blish document
tion structure- docs(
rchitecture): 
dd Ph
se 3 browser ref
ctoring design- docs(
rchitecture): 
dd Ph
se 6 tools server ref
ctoring design- docs(
rchitecture): 
dd Ph
se 5 plugins h
ndlers ref
ctoring design- docs(
rchitecture): 
dd Ph
se 4 POE h
ndlers ref
ctoring design- docs: 
dd Ph
se 2 continu
tion guide for next session- docs(
rchitecture): 
dd Ph
se 2 
tomic executor ref
ctoring design- docs(
rchitecture): 
dd Ph
se 1 types ref
ctoring design- docs(cortex): 
dd Month 3 implement
tion pl
n- docs(cortex): 
dd Month 3 Met
-Cognition L
yer design- docs: 
dd Atomic Engine fin
l implement
tion report- docs: 
dd comprehensive Atomic Engine document
tion- docs: 
dd Atomic Engine progress report (90% complete)- docs: 
dd Atomic Engine short-term t
sk completion st
tus- docs: 
dd Cortex evolution system design- docs: 
dd Atomic Engine evolution ro
dm
p (3-12+ months)- docs: 
dd 
tomic engine implement
tion st
tus report- docs: 
dd l
ngu
ge preference to CLAUDE.md- docs: 
dd Ph
se 2 Intelligent Scheduling design- docs: 
dd guest session 
ctivity logging implement
tion pl
n- docs: 
dd Liquid Hub cross-pl
tform 
rchitecture design- docs: complete Identity Context security document
tion- docs: 
dd Identity Context & Security Enforcement design- docs: 
dd ConfigM
n
ger 
nd Memory N
mesp
ce implement
tion pl
n- docs: 
dd ConfigM
n
ger 
nd Memory N
mesp
ce design- docs: 
dd Person
l AI Hub implement
tion pl
n- docs: 
dd Person
l AI Hub 
rchitecture design- docs: 
dd client 
rchitecture document
tion 
nd testing guide- docs: 
dd Ph
se 2 progress report- docs: 
dd client 
rchitecture ref
ctoring pl
n- docs: document Server-Client 
rchitecture in CLAUDE.md- docs: 
dd Server-Client implement
tion pl
n- docs: 
dd Server-Client 
rchitecture design- docs: 
dd DDD terminology 
nd dom
in modeling guide- docs: 
dd DDD+BDD du
l-wheel 
rchitecture design- docs: 
dd comprehensive Tool-
s-Resource us
ge guide 
nd upd
te Ph
se 4 st
tus- docs: upd
te Ph
se 3 progress - L2 
nd observ
bility completed- docs: upd
te Ph
se 2 checkboxes to completed- docs: upd
te MEMORY_SYSTEM.md with Memory Evolution fe
tures- docs(bdd): 
dd comprehensive BDD testing guide 
nd upd
te pl
ns- docs: 
dd Ph
se 3 implement
tion pl
n- docs: m
rk Ph
se 2 
s complete with 
ll t
sks done- docs: document Ph
se 2 memory system components in TOOL_SYSTEM.md- docs: upd
te Ph
se 2 pl
n with completion st
tus- docs: upd
te implement
tion pl
n with completion summ
ry- docs: 
dd Ph
se 1 MVP implement
tion pl
n- docs: 
dd Multi-Agent 2.0 Ph
se 1 implement
tion pl
n- docs: 
dd memory system evolution design- docs: 
dd Multi-Agent Resilience document
tion- docs: upd
te Ph
se 1 checkboxes to completed- docs: upd
te Tool-
s-Resource design st
tus to In Progress- docs: 
dd Tool-
s-Resource implement
tion pl
n- docs: 
dd Multi-Agent Resilience & Govern
nce 
rchitecture design- docs: 
dd Tool-
s-Resource 
rchitecture design- docs: 
dd Embodiment Engine 
nd CoT Tr
nsp
rency document
tion- docs: 
dd Multi-Agent 2.0 
rchitecture design- docs(pl
ns): 
dd Embodiment Engine & CoT Tr
nsp
rency design- docs(
gent-system): 
dd Ch
nnel C
p
bility Aw
reness document
tion- docs: 
dd ch
nnel c
p
bility 
w
reness implement
tion pl
n- docs: 
dd ch
nnel c
p
bility 
w
reness 
rchitecture design- docs: 
dd worksp
ce 
rchitecture design- docs: 
dd Ph
se 5 implement
tion pl
n- docs: 
dd Ph
se 5 Custom Rules Engine 
rchitecture design- docs: 
dd WorldModel + Disp
tcher 
rchitecture design- docs(d
emon): 
dd perception l
yer document
tion- docs: 
dd Protocol Ad
pter Ph
se 4 implement
tion summ
ry- docs(
rchitecture): document configur
ble protocol 
d
pter system- docs(protocols): 
dd comprehensive protocol 
d
pter user guide- docs: 
dd Ph
se 2 Perception L
yer implement
tion pl
n- docs(protocols): 
dd ex
mple YAML protocol configur
tions- docs: 
dd Ph
se 2 Perception L
yer design- docs: 
dd d
emon module document
tion- docs: 
dd Ph
se 1 d
emon implement
tion pl
n- docs: 
dd pro
ctive AI 
rchitecture design- build: remove deprec
ted c
bi fe
ture 
nd fix Discord API- docs: 
dd comprehensive M
rkdown Tool Ad
pter implement
tion summ
ry- docs: 
dd Protocol Ad
pter Ph
se 4 design- docs: 
dd M
rkdown Tool Ad
pter design specific
tion- docs: 
dd Protocol Ad
pter Ph
se 3 implement
tion summ
ry- docs: 
dd Protocol Ad
pter Ph
se 2 implement
tion summ
ry- docs: 
dd Protocol Ad
pter Ph
se 2 implement
tion pl
n- docs: 
dd Protocol Ad
pter Ph
se 2 design for Cl
ude/Gemini migr
tion- docs(providers): upd
te module document
tion for Protocol Ad
pter 
rchitecture- docs: 
dd Protocol Ad
pter implement
tion pl
n- docs: 
dd Protocol Ad
pter 
rchitecture design- docs(pl
ns): 
dd P2.5 MCP Adv
nced Fe
tures implement
tion pl
n- docs(mcp): 
dd P2 
dv
nced fe
tures implement
tion pl
n- docs: 
dd Memory v3 implement
tion pl
n with bite-sized TDD t
sks- docs(mcp): 
dd P1 c
p
bilities implement
tion pl
n- docs: 
dd Memory System v3 "Gl
ss Box" 
rchitecture design- docs(mcp): 
dd MCP Orchestr
tion L
yer implement
tion pl
n- docs(mcp): 
dd MCP Orchestr
tion L
yer design- docs(cortex): 
dd det
iled implement
tion pl
n with TDD steps- docs(extension): 
dd P0.5-P2 fe
ture document
tion- docs(extension): 
dd P0.5-P2 implement
tion pl
n- docs(extension): 
dd SDK V2 document
tion- docs(disp
tcher): 
dd Cortex 2.0 
rchitecture design- docs(extension): 
dd SDK V2 P0 implement
tion pl
n- docs(extension): 
dd Aether Extension SDK V2 design specific
tion- docs(skills): 
dd det
iled implement
tion pl
n for requirements fe
ture- docs(skills): 
dd requirements & CLI wr
pper 
rchitecture design- docs(poe): 
dd contr
ct signing design for first principles closure- docs: upd
te memory system docs 
nd 
dd h
lo comm
nd system pl
n- docs: 
dd mess
ge flow optimiz
tion design 
nd implement
tion pl
n- docs: 
dd H
lo-Only mess
ge flow design 
nd implement
tion pl
n- docs: 
dd comprehensive 
rchitecture document
tion- docs: 
dd det
iled POE implement
tion pl
n- docs: 
dd POE (Principle-Oper
tion-Ev
lu
tion) 
rchitecture design- docs: 
dd Agent-Action inter
ction implement
tion pl
n- docs: 
dd Agent-Action inter
ction system design- docs: m
rk Milestone 6 (ResilientT
sk) 
s complete- docs: 
dd Rust l
yer code cle
nup design pl
n- docs: 
dd Milestone 6 resilient t
sk implement
tion pl
n- docs: m
rk Milestone 5 (skill evolution) 
s complete- docs: 
dd Milestone 5 skill evolution implement
tion pl
n- docs: m
rk Milestone 4 (spec-driven dev) 
s complete- docs: 
dd Milestone 4 spec-driven development implement
tion pl
n- docs: m
rk Milestone 3 (Telegr
m 
pprov
l) 
s complete

## [0.2.8] - 2026-03-22### Added- fe
t(p
nel): 
dd stre
ming, render_mode, typing_indic
tor fields to Feishu settings- fe
t(feishu): wire FeishuEventEmitter into execution flow- fe
t(feishu): 
dd m
rkdown c
rd rendering 
nd upd
ted c
p
bilities- fe
t(feishu): 
dd FeishuEventEmitter with stre
ming c
rds 
nd typing indic
tors- fe
t(feishu): 
dd C
rd Kit stre
ming, st
tic c
rd, 
nd re
ction API methods- fe
t(feishu): 
dd stre
ming, render_mode, typing config fields 
nd API types- fe
t(p
nel): 
dd Feishu/L
rk ch
nnel settings c
rd- fe
t(feishu): fix clippy w
rnings — unused import, visibility, closure- fe
t(feishu): 
dd FeishuCh
nnel impl 
nd wire into f
ctory registry- fe
t(feishu): 
dd FeishuClient with token, HTTP API, 
nd medi
 support- fe
t(feishu): 
dd WebSocket event p
rsing 
nd text extr
ction- fe
t(feishu): 
dd types, config, 
nd API response structs- fe
t: 
dd Persistent Completion Protocol for 
gent t
sk verific
tion- desktop-m
cos: implement PimC
p
bility vi
 SwiftBridge- desktop-m
cos: implement SystemC
p
bility (
pps, notific
tions, clipbo
rd, sysinfo)- desktop-m
cos: implement Autom
tionC
p
bility (os
script + Shortcuts CLI)- desktop: wire N
tiveScreen into 
ll pl
tform cr
tes- desktop: 
dd N
tiveScreen sh
red ScreenC
p
bility implement
tion- core: 
dd SystemTool 
nd Autom
tionTool builtin tools- desktop: 
dd per-pl
tform cr
te skeletons (m
cos, linux, windows)- desktop: 
dd SwiftBridge utility for m
cOS n
tive API c
lls- desktop: upd
te cr
te doc to reflect two-l
yer 
rchitecture- desktop: 
dd c
p
bility tr
it hier
rchy 
nd sh
red types- core: 
dd 
leph-client dependency for server bin
ry- fe
t: en
ble n
tive tool c
lling for Ch
tGPT/Codex Responses API- core: 
dd Strict Mode support (schem
 strictific
tion + provider integr
tion)- core: 
dd #[cfg(unix)] gu
rds for Unix socket code on Windows- desktop: fix Windows OCR compil
tion errors- fe
t(browser): 
dd profile config types 
nd browser system configur
tion- fe
t(browser): 
dd SsrfPolicy for URL v
lid
tion 
nd priv
te network blocking- fe
t(config): 
dd queue_mode session configur
tion with g
tew
y wiring- fe
t(
nthropic): wire c
che_control ephemer
l bre
kpoint for system prompt c
ching- fe
t(thinker): p
rtition system prompt into st
ble/dyn
mic zones for c
che optimiz
tion- fe
t(compressor): 
dd pre-comp
ction silent memory flush- fe
t(
gent-loop): 
dd CollectQueue with time-window mess
ge merging- fe
t(
gent-loop): 
dd SteerQueue with interrupt sign
ling- fe
t(
gent-loop): 
dd SessionQueue tr
it 
nd FollowupQueue implement
tion- fe
t(
gent-loop): wire interrupt ch
nnel into RunContext 
nd loop execution- fe
t(
gent-loop): 
dd InterruptCh
nnel for steering support- core: 
dd missing tr
cing::w
rn import for non-m
cOS builds- fe
t: unified sl
sh comm
nd system- fe
t: wire memory tools into 
gent execution + Two-Ph
se Sm
rt Rec
ll- fe
t(server): 
dd desktop fe
ture g
te for in-process desktop c
p
bilities- fe
t(desktop): integr
te DesktopC
p
bility into DesktopTool with du
l-p
th execution- fe
t(desktop): implement input 
ctions with enigo- fe
t(desktop): implement screenshot 
nd OCR vi
 xc
p- fe
t: 
dd 
leph-desktop cr
te skeleton with DesktopC
p
bility tr
it- desktop: fix T
uri build for m
cOS 
nd 
dd 
pp/dmg bundle t
rgets- fe
t(w
sm): register host functions vi
 PluginBuilder with c
p
bility kernel- fe
t(m
nifest): p
rse WASM c
p
bilities from 
leph.plugin.toml- fe
t(w
sm): 
dd W
smC
p
bilityKernel — per-execution security enforcement- fe
t(w
sm): 
dd Credenti
lInjector — plugins never see secrets- fe
t(w
sm): 
dd AllowlistV
lid
tor with 
nti-byp
ss security- fe
t(w
sm): 
dd W
smC
p
bilities types with def
ult-deny model- fe
t(exec): 
dd Le
kDetector with Aho-Cor
sick bidirection
l sc
nning- desktop: 
dd 
ll_d
y 
nd c
lend
r_id to PimC
lend
rUpd
te- desktop: 
dd PIM v
ri
nts to DesktopRequest 
nd JSON-RPC m
pping- desktop: remove m
cOS t
rget, 
dd server embedding for Linux/Windows- desktop: fix fl
ky tests th
t 
ssumed bridge socket 
bsence- desktop-bridge: implement Windows OCR (WinRT) 
nd UI Autom
tion AX tree- desktop-bridge: implement window m
n
gement (list, focus, l
unch)- desktop-bridge: implement Windows input simul
tion (click, type, key combo, scroll)- desktop: wire sn
pshot 
nd new 
ctions in DesktopBridgeServer disp
tch- desktop: implement scroll, double-click, dr
g, hover, p
ste, 
nd ref-
w
re t
rgeting- desktop: implement UI sn
pshot with ref gener
tion in Perception.swift- desktop: 
dd RefStore for sn
pshot ref m
n
gement (Swift)- desktop: upd
te tool 
rgs 
nd build_request for sn
pshot, ref t
rgeting, 
nd new 
ctions- desktop: 
dd core types for sn
pshot, ref system, 
nd new 
ction primitives- desktop: upd
te tool mess
ging for bridge 
rchitecture- desktop: probe m
n
ged 
nd st
nd
lone socket p
ths- fe
t(runtimes): 
dd ensure_c
p
bility orchestr
tion (Probe -> Bootstr
p -> Register)- fe
t(runtimes): wire C
p
bilityLedger into prompt system- fe
t(runtimes): 
dd bootstr
p module with shell-driven inst
ll
tion- fe
t(runtimes): wire ledger into exec l
yer PATH- fe
t(runtimes): 
dd Probe module for system-first c
p
bility detection- fe
t(runtimes): 
dd leg
cy m
nifest.json migr
tion to ledger.json- fe
t(runtimes): 
dd C
p
bilityLedger for lightweight runtime st
te tr
cking- fe
t(desktop): implement desktop.screenshot in T
uri DesktopBridge- fe
t(desktop): 
dd DesktopBridge UDS server with ping support- fe
t(protocol): 
dd desktop_bridge types for cross-pl
tform Bridge- fe
t(h
lo): switch m
cOS H
loWindow from SwiftUI to WKWebView- fe
t(h
lo): 
dd /h
lo route with ch
t UI, mess
ge list, 
nd input 
re
- fe
t(h
lo): 
dd event h
ndler to wire run.* stre
ming events to H
loSt
te- fe
t(h
lo): 
dd H
loSt
te re
ctive sign
ls for ch
t st
te m
n
gement- fe
t(h
lo): 
dd Ch
tApi module for ch
t.send/
bort/history/cle
r- fe
t(desktop): T
sk 11 complete — DesktopTool 
ctive in 
gent vi
 builtin registry- fe
t(desktop): implement WKWebView c
nv
s overl
y with A2UI p
tch support- fe
t(desktop): implement mouse, keybo
rd, 
nd window 
ctions in Action.swift- fe
t(desktop): 
dd 
ccessibility permission description 
nd runtime check- fe
t(desktop): implement screenshot, OCR, 
nd AX tree in Perception.swift- fe
t(desktop): point settings window to Leptos Control Pl
ne server- fe
t(m
cos): 
dd Settings menu item opening Control Pl
ne WebView- fe
t(m
cos): 
dd SettingsWebView WKWebView wr
pper- fe
t(desktop): 
dd Swift UDS server skeleton with stub h
ndlers- fe
t(desktop): register DesktopTool in executor builtin registry- fe
t(desktop): 
dd DesktopTool builtin with gr
ceful degr
d
tion- fe
t(desktop): 
dd UDS client with JSON-RPC 2.0 
nd unit tests- fe
t(desktop): 
dd types, error, 
nd module sc
ffold- fe
t(skill): integr
te SkillSystem v2 into ExtensionM
n
ger 
nd ExecutionEngine- fe
t(skill): 
dd SkillSystem f
c
de with Arc<Inner> p
ttern- fe
t(skill): 
dd sl
sh comm
nd resolution- fe
t(skill): 
dd Inst
llSpec to shell comm
nd converter- fe
t(skill): 
dd SkillSt
tusReport for eligibility d
shbo
rd- fe
t(skill): 
dd SkillSn
pshot with version-inv
lid
ted c
che- fe
t(skill): 
dd XML prompt builder for skill injection- fe
t(skill): 
dd EligibilityService with OS/bin
ry/env checks- fe
t(skill): 
dd SKILL.md p
rser with YAML frontm
tter support- fe
t(skill): 
dd SkillRegistry with priority-b
sed dedup- fe
t(skill): 
dd SkillM
nifest Aggreg
teRoot with Entity tr
it- fe
t(skill): 
dd EligibilitySpec, Inst
llSpec, Invoc
tionPolicy, PromptScope V
lueObjects- fe
t(skill): 
dd SkillId, PluginId, SkillSource dom
in types- fe
t(thinker): 
dd skill_instructions to PromptConfig for SkillSystem v2- fe
t(extension): 
dd SkillSystem v2 
nd wire skill XML into 
gent prompts- fe
t(sw
rm): 
dd event st
tistics 
nd logging- fe
t(
gent_loop): integr
te ContextProvider into Mess
geBuilder- fe
t(sw
rm): implement Sw
rmContextProvider- fe
t(
gent_loop): define ContextProvider tr
it- fe
t(
gent_loop): implement event publishing (sh
dow mode)- fe
t(
gent_loop): define AgentLoopEvent enum- fe
t(
gent_loop): implement Builder build() method- fe
t(
gent_loop): 
dd AgentLoopBuilder structure- fe
t(perception): integr
te PAL with SystemSt
teBus- fe
t(perception): 
dd Pl
tform Abstr
ction L
yer (PAL)- fe
t(sw
rm): Ph
se 5 - End-to-End Integr
tion- fe
t(perception): implement Ph
se 5 - Document
tion, Ex
mples & Testing- fe
t(perception): implement Ph
se 4 - Vision Connector 
rchitecture- fe
t(ssb): implement Ph
se 3 - 
ction disp
tcher- fe
t(ssb): implement Ph
se 2 - robustness & priv
cy- fe
t(ssb): implement Ph
se 1 - core infr
structure- fe
t(control-pl
ne): implement WebSocket subscription for re
l-time 
lerts- fe
t(sh
red_ui_logic): 
dd 
lerts API module for system he
lth 
nd memory monitoring- fe
t(skill-evolution): integr
te SuccessM
nifest with tool execution- fe
t(control-pl
ne): p
ss mode 
nd 
lert_key to Sideb
rItems- fe
t(control-pl
ne): integr
te Tooltip 
nd B
dge into Sideb
rItem- fe
t(control-pl
ne): 
dd St
tusB
dge component for 
lert indic
tors- fe
t(control-pl
ne): 
dd Tooltip component for n
rrow mode l
bels- fe
t(skill-evolution): implement Coll
bor
tiveSolidific
tionPipeline- fe
t(control-pl
ne): implement Sideb
r n
rrow/wide mode switching- fe
t(skill-evolution): implement Constr
intV
lid
tor- fe
t(skill-evolution): implement SuccessM
nifest d
t
 structure- fe
t(control-pl
ne): 
dd SettingsL
yout for nested routing- fe
t(control-pl
ne): 
dd 
lert bus 
nd sideb
r mode override to D
shbo
rdSt
te- fe
t(control-pl
ne): 
dd sideb
r types (Sideb
rMode, AlertLevel, SystemAlert)- fe
t(control-pl
ne): compile T
ilwind CSS loc
lly for production- fe
t(d
shbo
rd): 
dd Plugins, Skills, 
nd Policies settings p
ges- fe
t(d
shbo
rd): 
dd sideb
r n
vig
tion to settings UI- fe
t(d
shbo
rd): 
dd Gener
tion Providers n
vig
tion c
rd to Settings p
ge- fe
t(d
shbo
rd): implement Gener
tion Providers CRUD function
lity- fe
t(d
shbo
rd): 
dd Gener
tion Providers frontend UI- fe
t(d
shbo
rd): 
dd Gener
tion Providers b
ckend 
nd API l
yer- fe
t(d
shbo
rd): implement comprehensive configur
tion m
n
gement UI- fe
t(m
cos): implement WebSocket client for G
tew
y connection- fe
t(m
cos): complete Ph
se 4 client simplific
tion for ControlPl
ne integr
tion- fe
t(d
shbo
rd): complete Ph
se 3 SDK integr
tion with RPC, events, 
nd API l
yer- fe
t(d
shbo
rd): complete Ph
se 2 SDK integr
tion with error h
ndling 
nd reconnection- fe
t(d
shbo
rd): 
dd connection st
te 
w
reness to Memory view- fe
t(d
shbo
rd): integr
te sh
red_ui_logic SDK into D
shbo
rd- fe
t(d
shbo
rd): full 
rchitectu
l ref
ctor with Leptos 0.8.15 
nd rust-ui components- fe
t(d
shbo
rd): complete Memory Explorer view 
nd fix System St
tus- fe
t(d
shbo
rd): initi
lize Aleph D
shbo
rd with Leptos 0.6- fe
t(sh
red-ui-logic): implement Plugins 
nd Providers APIs- fe
t(sh
red-ui-logic): implement WASM WebSocket connector- fe
t(sh
red-ui-logic): implement API 
nd Observ
bility l
yers- fe
t(sh
red_ui_logic): implement protocol l
yer- fe
t(sh
red_ui_logic): implement n
tive WebSocket connector- fe
t(sh
red_ui_logic): initi
lize Aleph UI Logic SDK- fe
t(cortex): implement LLM-b
sed critic report gener
tion- fe
t(cortex): 
dd AiProvider to CriticAgent- fe
t(cortex): implement LLM-b
sed root c
use 
n
lysis- fe
t(cortex): 
dd AiProvider to Re
ctiveReflector- fe
t(
gent_loop): 
dd met
-cognition integr
tion for Ph
se 6- fe
t(cortex): implement CortexIntegr
tion orchestr
tor (T
sk #11)- fe
t(cortex): implement experience clustering 
nd deduplic
tion- fe
t(disp
tcher): implement L1.5 ExperienceRepl
yL
yer- fe
t(cortex): implement Cortex Dre
ming b
ckground service- fe
t(cortex): implement LLM-b
sed p
ttern extr
ction- fe
t(cortex): implement Distill
tionService core structure- fe
t(engine): 
dd Fe
tureExtr
ctor for 
dv
nced ML rule le
rning- fe
t(cortex): implement multi-dimension
l experience v
lue estim
tor- fe
t(cortex): 
dd 
gent loop telemetry c
pture- fe
t(cortex): implement Experience CRUD oper
tions- fe
t(cortex): define core d
t
 structures- fe
t(engine): 
dd ML-b
sed L2 rule gener
tion (RuleLe
rner)- fe
t(cortex): 
dd experience_repl
ys d
t
b
se t
ble- fe
t(builtin_tools): 
dd AtomicOpsTool for 
tomic oper
tions- fe
t(browser): implement J
v
Script-b
sed context freeze/resume- fe
t(browser): implement Ph
se 2.4 CDP integr
tion for context freeze/resume- fe
t(engine): 
dd comprehensive testing 
nd perform
nce v
lid
tion- fe
t(executor): 
dd AtomicActionExecutor with L1/L2 routing- fe
t(engine): implement 
tomic engine with L1/L2/L3 routing- fe
t(disp
tcher): implement Ph
se 2 Intelligent Scheduling for Liquid Hub- fe
t(m
cos): 
dd guest session 
ctivity log UI- fe
t(m
cos): 
dd 
ctivity log RPC types 
nd methods- fe
t(g
tew
y): 
dd RPC request 
ctivity logging for guest sessions- fe
t(g
tew
y): 
dd guests.getActivityLogs RPC h
ndler- fe
t(g
tew
y): integr
te 
ctivity logging into GuestSessionM
n
ger- fe
t: implement guests.revokeInvit
tion RPC method- fe
t(m
cos): 
dd Guest m
n
gement UI in Settings- fe
t(g
tew
y): register config.get 
nd config.p
tch RPC h
ndlers- fe
t(g
tew
y): 
dd SessionIdentityMet
 for identity stor
ge- fe
t(protocol): 
dd IdentityContext for st
teless security- fe
t(g
tew
y): 
dd config.p
tch RPC h
ndler with events- fe
t(memory): 
dd idempotent n
mesp
ce migr
tion- fe
t(g
tew
y): 
dd RPC h
ndlers for guest m
n
gement- fe
t(memory): 
dd n
mesp
ce column for d
t
 isol
tion- fe
t(protocol): 
dd discovery types for mDNS- fe
t(protocol): 
dd ConfigCh
ngedEvent for config sync- fe
t(g
tew
y): 
dd Invit
tionM
n
ger for guest invit
tions- fe
t(protocol): 
dd invit
tion types for guest m
n
gement- fe
t(g
tew
y): 
dd PolicyEngine for permission checks- fe
t(g
tew
y): 
dd IdentityM
p for extern
l identity resolution- fe
t(protocol): 
dd Role 
nd GuestScope for Owner+Guest model- fe
t(ph
se3): complete T
uri Desktop migr
tion to thin client- fe
t(ph
se3): migr
te T
uri Desktop to SDK 
rchitecture (WIP)- fe
t(ph
se2): ref
ctor CLI to use SDK- fe
t(ph
se2): implement G
tew
yClient with 
uthentic
tion- fe
t(ph
se2): implement tr
nsport 
nd RPC l
yers in SDK- fe
t(ph
se2): cre
te 
leph-client-sdk skeleton- fe
t(g
tew
y): 
dd Server-Client routing infr
structure to ConnectionSt
te- fe
t: 
dd tool routing config 
nd scope checking for Server-Client 
rchitecture- fe
t(executor): integr
te RoutedExecutor with Agent Loop- fe
t(cli): cre
te 
leph-cli 
s protocol reference implement
tion- fe
t(protocol): cre
te 
leph-protocol cr
te for sh
red types- fe
t(executor): integr
te ToolRouter with execution engine- fe
t(disp
tcher): 
dd execution_policy field to UnifiedTool- fe
t(executor): 
dd ToolRouter for Server-Client routing decisions- fe
t(g
tew
y): 
dd tool.c
ll protocol mess
ges- fe
t(g
tew
y): 
dd ReverseRpcM
n
ger for Server-to-Client c
lls- fe
t(g
tew
y): store ClientM
nifest in ConnectionSt
te- fe
t(g
tew
y): extend ConnectP
r
ms to 
ccept ClientM
nifest- fe
t(g
tew
y): 
dd ClientM
nifest for c
p
bility negoti
tion- fe
t(disp
tcher): 
dd ExecutionPolicy enum for Server-Client routing- fe
t(spec_driven): implement BDD du
l-tr
ck testing system- fe
t(dom
in): implement DDD found
tion with m
rker tr
its- fe
t(disp
tcher): implement L2 
sync LLM enh
ncement for tool descriptions- fe
t(memory): 
dd perform
nce monitoring for LLM c
lls- fe
t(scheduler): implement recursion depth tr
cking- fe
t(scheduler): implement 
nti-st
rv
tion logic- fe
t(scheduler): implement L
neScheduler core- fe
t: implement CompressionD
emon for b
ckground compression scheduling- fe
t(scheduler): implement L
neSt
te with queue 
nd sem
phore- fe
t: enh
nce ContextComptroller with priority-b
sed token m
n
gement- fe
t: implement V
lueEstim
tor for memory import
nce scoring- fe
t(scheduler): 
dd l
ne scheduler infr
structure- fe
t: 
dd sliding window chunking to Tr
nscriptIndexer- fe
t: 
dd Tr
nscriptIndexer for ne
r-re
ltime memory indexing- fe
t(sub_
gents): 
dd 
ctive runs query 
nd st
ts to SubAgentRegistry- fe
t(sub_
gents): 
dd F
ctsDB persistence helpers for SubAgentRun- fe
t(sub_
gents): 
dd st
te tr
nsition to SubAgentRegistry- fe
t(sub_
gents): 
dd SubAgentRegistry with in-memory indexing- fe
t(memory): 
dd SubAgent f
ct types for Multi-Agent 2.0 persistence- fe
t(sub_
gents): 
dd SubAgentRun d
t
 model for Multi-Agent 2.0- fe
t(disp
tcher): integr
te Hydr
tionPipeline into Agent Loop- fe
t(core): export tool_index types from lib.rs- fe
t(memory): 
dd VectorD
t
b
se::in_memory() for testing- fe
t(disp
tcher): 
dd ToolRetriev
l with du
l-threshold hydr
tion- fe
t(disp
tcher): 
dd ToolIndexCoordin
tor for Memory synchroniz
tion- fe
t(disp
tcher): 
dd Sem
nticPurposeInferrer for L0/L1 inference- fe
t(disp
tcher): 
dd tool_index module with ToolRetriev
lConfig- fe
t(memory): 
dd Tool v
ri
nt to F
ctType for tool-
s-resource- fe
t(memory): 
dd Multi-Agent Resilience d
t
b
se l
yer- fe
t(g
tew
y): 
dd identity m
n
gement RPC h
ndlers- fe
t(thinker): 
dd thinking tr
nsp
rency guid
nce to PromptBuilder- fe
t(
gent_loop): integr
te ThinkingP
rser into DecisionP
rser- fe
t(g
tew
y): 
dd Re
soningBlock 
nd Uncert
intySign
l stre
m events- fe
t(
gent_loop): 
dd ThinkingP
rser for sem
ntic re
soning extr
ction- fe
t(
gent_loop): 
dd StructuredThinking types for CoT Tr
nsp
rency- fe
t(thinker): integr
te Soul into PromptBuilder- fe
t(thinker): 
dd m
rkdown p
rser for soul.md files- fe
t(thinker): 
dd IdentityResolver for l
yered identity resolution- fe
t(thinker): 
dd SoulM
nifest types for Embodiment Engine- fe
t(test): migr
te logging, security, 
nd e2e tests to BDD- fe
t(test): migr
te iMess
ge routing 
nd sub
gent tests to BDD- fe
t(g
tew
y): 
dd Ch
nnelProvider tr
it for inter
ction m
nifests- fe
t(
gent_loop): 
dd Silent 
nd He
rtbe
tOk decision types- fe
t(thinker): 
dd environment contr
ct 
nd security sections to PromptBuilder- fe
t(thinker): 
dd ContextAggreg
tor for environment reconcili
tion- fe
t(test): migr
te m
rkdown skills tests to BDD- fe
t(thinker): 
dd SecurityContext for policy-driven permissions- fe
t(thinker): 
dd Inter
ctionM
nifest for ch
nnel c
p
bility 
w
reness- fe
t(test): migr
te models 
nd protocol integr
tion tests to BDD- fe
t(test): migr
te DAG 
nd worldmodel disp
tcher tests to BDD- fe
t(test): migr
te sm
rt tool discovery 
nd sessions tests to BDD- fe
t(thinker): 
dd provider-specific context c
ching str
tegies- fe
t(disp
tcher): 
dd du
l-l
yer profile-b
sed tool filtering- fe
t(test): migr
te extension v2 
nd runtime tests to BDD- fe
t(g
tew
y): 
dd Worksp
ceM
n
ger for Anti-Gr
vity Architecture- fe
t(test): migr
te extension plugin registry tests to BDD- fe
t(test): migr
te tool server tests to BDD- fe
t(test): migr
te g
tew
y inbound router tests to BDD- fe
t(test): migr
te disp
tcher cortex tests to BDD- fe
t(test): migr
te memory integr
tion tests to BDD- fe
t(tests): migr
te memory f
cts tests to BDD- fe
t(tests): migr
te mess
ge builder tests to BDD- fe
t(tests): migr
te thinker prompt builder tests to BDD- fe
t(tests): migr
te POE tests to BDD- fe
t(tests): migr
te 
gent loop tests to BDD- fe
t(config): 
dd ProfileConfig for Worksp
ce Architecture- fe
t(tests): migr
te perception 
nd w
tcher tests to BDD- fe
t(tests): migr
te d
emon IPC 
nd l
unchd tests to BDD- fe
t(tests): migr
te d
emon core tests to BDD- fe
t(tests): migr
te config v
lid
tion tests to BDD- fe
t(tests): migr
te config b
sic tests to BDD- fe
t(tests): migr
te scripting engine tests to BDD- fe
t(tests): 
dd cucumber BDD infr
structure- fe
t: 
dd ex
mple YAML policies 
nd E2E tests- fe
t(disp
tcher): 
dd YAML policy lo
der 
nd PolicyEngine integr
tion- fe
t(disp
tcher): implement Y
mlPolicy with Rh
i ev
lu
tion- fe
t(scripting): 
dd B
selineApi with l
zy TTL c
ching- fe
t(scripting): implement HistoryApi.l
st() with WorldModel queries- fe
t(scripting): implement EventApi 
nd EventCollection filtering- fe
t(scripting): 
dd HistoryApi 
nd EventCollection stubs- fe
t(scripting): 
dd dur
tion p
rsing 
nd helpers for Rh
i- fe
t(disp
tcher): 
dd YAML rule schem
 p
rsing- fe
t(disp
tcher): 
dd Rh
i s
ndbox engine with strict limits- fe
t(worldmodel): 
dd JSON st
te persistence- fe
t(disp
tcher): 
dd core d
t
 structures- fe
t(d
emon): integr
te perception l
yer with d
emon CLI- fe
t(d
emon): implement FSEventW
tcher- fe
t(d
emon): implement SystemSt
teW
tcher- fe
t(d
emon): implement ProcessW
tcher- fe
t(d
emon): implement TimeW
tcher- fe
t(d
emon): 
dd w
tcher tr
it 
nd registry- fe
t(d
emon): 
dd perception configur
tion system- fe
t(d
emon): 
dd event system found
tion- fe
t(protocols): implement hot relo
d with notify file w
tching- fe
t(protocols): implement ProtocolLo
der file 
nd directory lo
ding- fe
t(protocols): implement Configur
bleProtocol custom mode with templ
te rendering- fe
t(protocols): implement Configur
bleProtocol minim
l mode (extends b
se + differences)- fe
t(protocols): 
dd JSONP
th p
rser for response v
lue extr
ction- fe
t(protocols): 
dd templ
te engine wr
pper for request/response tr
nsform
tion- fe
t(protocols): 
dd dependencies for configur
ble protocols (h
ndleb
rs, jsonp
th, notify)- fe
t(providers): 
dd ProtocolLo
der stub for hot relo
d- fe
t(providers): 
dd Configur
bleProtocol stub- fe
t(providers): implement ProtocolRegistry for dyn
mic protocol m
n
gement- fe
t(providers): 
dd ProtocolDefinition types for YAML configs- fe
t(tools): implement Virtu
lFs s
ndbox mode- fe
t(tools): 
dd Evolution 
uto-lo
d integr
tion- fe
t(g
tew
y): 
dd M
rkdown Skills RPC h
ndlers- fe
t(tools): 
dd repl
ce_tool() API with explicit upd
te sem
ntics- fe
t(tools): 
dd hot relo
d support for M
rkdown Skills (Ph
se 4)- fe
t(tools): 
dd Evolution Loop integr
tion for M
rkdown Skills (Ph
se 3)- fe
t(tools): 
dd ex
mples() method to AetherTool tr
it (Ph
se 2)- fe
t(tools): complete M
rkdown Tool Ad
pter integr
tion- fe
t(tools): implement M
rkdown Tool Ad
pter (Ph
se 1)- fe
t(providers): 
dd Tier 3 speci
lized OpenAI-comp
tible provider presets- fe
t(providers): 
dd Tier 2 OpenAI-comp
tible provider presets- fe
t(providers): 
dd Tier 1 OpenAI-comp
tible provider presets- fe
t(providers): 
dd Gemini presets 
nd upd
te f
ctory- fe
t(providers): implement GeminiProtocol 
d
pter- fe
t(providers): 
dd Gemini API types module- fe
t(providers): 
dd Cl
ude/Anthropic presets- fe
t(providers): implement AnthropicProtocol 
d
pter- fe
t(providers): 
dd Anthropic API types module- fe
t(g
tew
y): 
dd 
pprov
l RPC h
ndlers- fe
t(mcp): 
dd Approv
lH
ndler for hum
n-in-the-loop- fe
t(mcp): 
dd 
pprov
l request types for hum
n-in-the-loop- fe
t(mcp): 
dd stre
ming types for s
mpling responses- fe
t(mcp): 
dd TokenRefreshM
n
ger for 
utom
tic token refresh- fe
t(mcp): 
dd OAuth token refresh support- fe
t(mcp): integr
te context injection with S
mplingH
ndler- fe
t(mcp): 
dd ContextInjector for cross-server context- fe
t(mcp): 
dd IncludeContext enum type for s
mpling requests- fe
t(config): 
dd protocol field to ProviderConfig- fe
t(providers): 
dd provider presets registry- fe
t(providers): 
dd HttpProvider cont
iner with ProtocolAd
pter- fe
t(providers): implement OpenAiProtocol 
d
pter- fe
t(providers): 
dd ProtocolAd
pter tr
it with stre
ming support- fe
t(providers): 
dd RequestP
ylo
d DTO for protocol 
d
pters- fe
t(mcp): 
dd s
mpling c
llb
ck integr
tion to McpM
n
ger- fe
t(mcp): 
dd response mech
nism for server-initi
ted requests- fe
t(mcp): integr
te S
mplingH
ndler with McpClient- fe
t(memory): complete Memory v3 Milestones 4-6- fe
t(mcp): 
dd S
mplingH
ndler for server-initi
ted LLM c
lls- fe
t(mcp): implement re
l SSE event listening with reqwest-eventsource- fe
t(mcp): 
dd SSE event types 
nd reqwest-eventsource dependency- fe
t(memory): implement CLI list 
nd show comm
nds- fe
t(memory): implement AuditLogger for oper
tion tr
cking- fe
t(mcp): 
dd S
mpling RPC types for P2 server-initi
ted LLM c
lls- fe
t(memory): 
dd 
udit log schem
 
nd types- fe
t(memory): 
dd CLI module with file locking- fe
t(memory): implement Archiv
lService for scr
tchp
d 
rchiving- fe
t(memory): implement HybridTrigger with token threshold s
fety net- fe
t(memory): implement L
zyDec
yEngine for re
d-time dec
y ev
lu
tion- fe
t(memory): 
dd type-
w
re dec
y c
lcul
tion with tempor
l scope- fe
t(memory): 
dd dec
y_inv
lid
ted_
t field for recycle bin- fe
t(memory): complete Milestone 1 - Scr
tchp
d Found
tion- fe
t(memory): implement Scr
tchp
dM
n
ger with CRUD oper
tions- fe
t(memory): implement SessionHistory for scr
tchp
d 
rchiv
l- fe
t(memory): 
dd scr
tchp
d module structure 
nd templ
te- fe
t(mcp): implement re
l McpResourceM
n
ger 
nd McpPromptM
n
ger- fe
t(tools): 
dd mcp_get_prompt builtin tool- fe
t(tools): 
dd mcp_re
d_resource builtin tool- fe
t(mcp): implement re
l 
ggreg
tion for resources 
nd prompts- fe
t(mcp): 
dd resources 
nd prompts methods to McpClient- fe
t(mcp): 
dd resources 
nd prompts support to McpServerConnection- fe
t(mcp): 
dd Resources 
nd Prompts RPC types- fe
t(mcp): 
dd he
lth check logic for servers- fe
t(g
tew
y): wire MCP h
ndlers to McpM
n
gerH
ndle- fe
t(mcp): implement McpM
n
gerActor core loop- fe
t(mcp): 
dd config persistence for McpM
n
ger- fe
t(mcp): 
dd McpM
n
gerH
ndle public API- fe
t(mcp): 
dd McpComm
nd 
nd McpM
n
gerEvent types- fe
t(cortex): implement DecisionConfig with session override- fe
t(cortex): implement security rules (t
g injection, PII m
sking, instruction override)- fe
t(cortex): 
dd S
nitizerRule tr
it 
nd SecurityPipeline- fe
t(cortex): 
dd greedy JSON rep
ir logic- fe
t(cortex): implement JsonStre
mDetector st
te m
chine- fe
t(cortex): 
dd module skeleton with unified error types- fe
t(extension): 
dd PluginHttpH
ndler for plugin REST routes- fe
t(extension): 
dd PluginProviderAd
pter for plugin AI providers- fe
t(extension): 
dd Ch
nnelM
n
ger skeleton for plugin ch
nnels- fe
t(extension): 
dd HTTP route types- fe
t(extension): 
dd provider plugin types- fe
t(extension): 
dd ch
nnel plugin types- fe
t(g
tew
y): 
dd service lifecycle RPC h
ndlers- fe
t(extension): integr
te ServiceM
n
ger with ExtensionM
n
ger- fe
t(extension): 
dd ServiceM
n
ger for b
ckground services- fe
t(extension): 
dd service lifecycle types- fe
t(g
tew
y): 
dd plugins.executeComm
nd RPC h
ndler- fe
t(extension): 
dd comm
nd execution to PluginLo
der- fe
t(extension): 
dd DirectComm
ndResult type- fe
t(extension): implement scope-
w
re skill injection- fe
t(extension): implement V2 prompt lo
ding with scope support- fe
t(extension): 
dd scope 
nd bound_tool to ExtensionSkill- fe
t(extension): 
dd PromptScope enum for V2 skill injection- fe
t(extension): 
dd V2 hook conversion from TOML m
nifest- fe
t(extension): implement typed hook execution (interceptor/observer/resolver)- fe
t(extension): 
dd kind 
nd priority to HookConfig- fe
t(extension): 
dd HookKind 
nd HookPriority enums- fe
t(extension): integr
te TOML p
rser with 
uto-detection (TOML > JSON)- fe
t(extension): 
dd V2 fields to PluginM
nifest- fe
t(extension): 
dd TOML m
nifest p
rser types- fe
t(exec): check skill_
llowlist in 
pprov
l decision- fe
t(exec): 
dd skill_
llowlist config option- fe
t(exec): extend ExecContext with skill origin info- fe
t(skills): implement CLI Wr
pper v
lid
tor- fe
t(skills): 
dd he
lth checking methods to SkillsRegistry- fe
t(skills): 
dd inst
ll suggestion methods to SkillsInst
ller- fe
t(skills): implement He
lthChecker for dependency v
lid
tion- fe
t(skills): extend SkillFrontm
tter with requirements 
nd met
d
t
- fe
t(skills): 
dd types for requirements 
nd he
lth checking- fe
t(poe): repl
ce Pl
ceholderWorker with re
l AgentLoopWorker- fe
t(g
tew
y): wire POE contr
ct signing to G
tew
y- fe
t(poe): implement contr
ct signing workflow for first principles closure- fe
t(core): 
dd sn
pshot c
pture tool 
nd registry upd
tes- fe
t(config): 
dd memory configur
tion types 
nd v
lid
tion- fe
t(memory): enh
nce retriev
l 
nd 
dd dre
ming module- fe
t(m
cos): 
dd tool emoji form
tting to H
loStre
mingView- fe
t(m
cos): upd
te G
tew
yStre
mAd
pter with enh
nced summ
ry- fe
t(m
cos): 
dd H
loResultViewV2 with det
il popover support- fe
t(m
cos): 
dd H
loResultDet
ilPopover for det
iled results- fe
t(m
cos): 
dd Enh
ncedRunSumm
ry 
nd ToolSumm
ryItem models- fe
t(g
tew
y): 
dd Enh
ncedRunSumm
ry 
nd per-runId sequences- fe
t(g
tew
y): 
dd mess
ge deduplic
tion with text norm
liz
tion- fe
t(g
tew
y): 
dd stre
m buffer for block-level text flushing- fe
t(g
tew
y): 
dd tool displ
y module with emoji 
nd sm
rt form
tting- fe
t(h
lo): integr
te comm
ndList st
te into H
loViewV2- fe
t(h
lo): 
dd H
loComm
ndListView for / comm
nd p
nel- fe
t(h
lo): 
dd Comm
ndItem 
nd Comm
ndListContext types for / comm
nd- fe
t(h
lo): 
dd H
loInputCoordin
tor for lightweight input h
ndling- fe
t(g
tew
y): 
dd 150ms throttling for response chunks- fe
t(h
lo): 
dd H
loViewV2 m
in component integr
ting 
ll st
te views- fe
t(h
lo): 
dd H
loHistoryListView for convers
tion history- fe
t(h
lo): 
dd H
loResultView for comp
ct result displ
y- fe
t(h
lo): 
dd H
loStre
mingView for unified stre
ming displ
y- fe
t(h
lo): 
dd H
loSt
teV2 with 6 simplified st
tes- fe
t(h
lo): 
dd new stre
ming types for simplified st
te model- fe
t(skill-evolution): implement Skill Compiler (Ph
se 10)- fe
t(
gent-loop): 
dd on_user_question method to LoopC
llb
ck- fe
t(
gent-loop): 
dd AskUserRich decision v
ri
nt with QuestionKind- fe
t(
gent-loop): export question 
nd 
nswer modules- fe
t(
gent-loop): 
dd UserAnswer type for structured responses- fe
t(
gent-loop): 
dd QuestionKind types for structured user inter
ction- fe
t(resilient): 
dd cron integr
tion with Podc
stT
sk ex
mple- fe
t(resilient): implement ResilientExecutor with retry 
nd f
llb
ck- fe
t(resilient): define ResilientT
sk tr
it- fe
t(resilient): 
dd core types for resilient t
sk execution- fe
t(skill_evolution): implement GitCommitter for 
uto-commit- fe
t(skill_evolution): implement SkillGener
tor for SKILL.md cre
tion- fe
t(skill_evolution): implement Solidific
tionDetector for p
ttern detection- fe
t(skill_evolution): implement EvolutionTr
cker for execution logging- fe
t(skill_evolution): 
dd core types for skill evolution system- fe
t(spec_driven): implement SpecDrivenWorkflow orchestr
tor- fe
t(spec_driven): implement LlmJudge for ev
lu
tion- fe
t(spec_driven): implement TestWriter for test gener
tion- fe
t(spec_driven): implement SpecWriter for requirement 
n
lysis- fe
t(spec_driven): 
dd core types for spec-driven workflow- fe
t(g
tew
y): 
dd exec.c
llb
ck.h
ndle RPC for 
pprov
l c
llb
cks- fe
t(telegr
m): 
dd edit_mess
ge method for 
pprov
l upd
tes- fe
t(g
tew
y): 
dd 
pprov
l bridge h
ndler utilities- fe
t(exec): 
dd Approv
lBridge for ch
nnel integr
tion- fe
t(telegr
m): 
dd c
llb
ck query h
ndling- fe
t(telegr
m): 
dd inline keybo
rd support### Fixed- fix: remove unused imports 
cross codeb
se (c
rgo fix)- fix: resolve 42 test w
rnings — deprec
ted API, unused imports, de
d code- fix: sl
sh comm
nd f
st-p
th + CLI 
rg p
rser + E2E tests- fix: en
ble sl
sh comm
nd f
st-p
th for WebCh
t ch
t.send- fix: repl
ce env!("HOME") with dirs::home_dir() for Windows comp
tibility- fix: correct PluginKind::Mcp m
pping 
nd remove debug output- fix: upd
te discovery to find CC-form
t plugins in inst
lled/ directory- fix: ch
nnel binding not repl
cing old peer_id rows- fix: ch
nnel st
tus showing disconnected 
fter p
ge refresh- fix: p
ss session_m
n
ger to BuiltinToolConfig for session tools- fix: resolve 
gent from session_key inste
d of Worksp
ceM
n
ger- fix: sep
r
te 
gent identity files from worksp
ce directory- fix: use bold *n
me* for 
gent prefix inste
d of [n
me]- fix: use M
rkdown (leg
cy) inste
d of M
rkdownV2 for Telegr
m mess
ges- fix: remove b
cksl
sh esc
ping from 
gent n
me prefix in replies- fix: override rel
tive working_dir with 
gent worksp
ce- fix: ch
nge def
ult worksp
ce root from 
gents/ to worksp
ces/- fix: def
ult b
sh/code_exec working directory to 
gent worksp
ce- fix: register JSON Schem
 for 
ll builtin tools + Codex protocol 
lignment- fix: prevent token regener
tion on HMAC mism
tch to protect v
ult secrets- fix: Codex SSE function_c
ll_
rguments delt
 collection + logging- fix: use v
ult_key() function inste
d of undefined VAULT_KEY const
nt- fix: unify rer
nking v
ult key form
t with other modules- fix: rer
nking P
nel fetches per-provider API key from v
ult- fix: cle
r 
pi_key from rer
nking config sign
l 
fter s
ve- fix: isol
te rer
nk API keys per provider in v
ult- fix: move rer
nk API key from config.toml to encrypted v
ult- fix: correct def
ult rer
nking model n
me in P
nel 
nd tests- fix: ACP p
nel buttons h
ng due to sp
wn_loc
l context loss- fix: ACP test/s
ve button h
ng 
nd preset mode def
ults- fix: ACP p
nel gemini preset ID mism
tch 
nd test button h
ng- fix: resolve 
ll 75 compil
tion errors from provider routing ref
ctor- fix: v
ult-b
cked provider API keys 
nd config h
ndler improvements- fix(
cp): 
d
pt h
rnesses to re
l CLI protocols 
fter e2e probe testing- fix: worksp
ce schem
 migr
tion, worksp
ce.getActive response, 
nd providers p
ge freeze- fix: remove redund
nt binding in ConfigP
tcher- fix: session history, 
gent.list RPC, 
nd embedding dedup- fix: count only running runs for concurrency limit, reduce cle
nup del
y- fix: 
dd multi-dimension vector columns to memories t
ble schem
- fix: hot-sw
p runtime provider when switching def
ult vi
 P
nel UI- fix: resolve ch
t qu
lity issues — bootstr
p, esc
l
tion, 
nd response form
t- fix: resolve pre-existing test compil
tion errors- fix: wire missing RPC h
ndlers 
nd correct TUI method n
mes- fix: upd
te rem
ining port 18789 references to 18790- fix: unify ch
nnel config persistence — P
nel UI s
ve/lo
d/connect now works- fix: resolve compil
tion errors from fe
ture fl
g remov
l- fix(desktop): 
ddress fin
l review — version 
lignment, input v
lid
tion, Unicode- fix(desktop): 
ddress clippy needless-borrow w
rning in 
gent h
ndler- fix(desktop): 
ddress code qu
lity review — v
lid
tion, 
pprov
l g
tes- fix(desktop): wire N
tiveDesktop into registry + complete re-exports- fix: logic review R2 
rchitecture — 14 findings 
cross 5 c
tegories- fix: logic review R2 — 29 files 
cross 4 priority b
tches- fix: 
ddress code review findings for self-configur
tion- fix: RAII sem
phore gu
rd 
nd env v
r exp
nsion ordering (Known Issues)- fix: repl
ce std::sync::RwLock with cr
te::sync_primitives (P2-15)- fix: sort H
shM
p-derived collections for deterministic ordering (P2-14)- fix: repl
ce SystemTime UNIX_EPOCH .unwr
p() with .unwr
p_or_def
ult() (P2-12)- fix: rele
se locks before 
w
iting in 4 
sync p
tterns (P2-11)- fix: norm
lize t
sk_type 
nd t
sk_id in SessionKey::t
sk() (P1-9)- fix: use bounded c
st for POE token count u32 conversion (P1-8)- fix: resolve rem
ining UTF-8 byte slicing p
nics (P1-7)- fix: ConfigP
tcher use s
ve_increment
l 
nd h
rd-error on conflict- fix: logic review Ph
se 6 — 45 fixes 
cross g
tew
y, memory, poe, exec, providers, 
nd 15 more modules- fix: resolve 5 rem
ining W
rning-level issues from logic review Ph
se 5- fix: logic review Ph
se 4 — 18 fixes 
cross d
emon, engine, secrets, skills, components, cron- fix: resolve 5 Known Issues from logic review- fix: comprehensive logic review fixes 
cross 53 files in 77 modules- fix: use cfg(fe
ture = "loom") inste
d of cfg(loom) to 
void poisoning dependencies- fix(g
tew
y): elimin
te TOCTOU in execution_engine concurrent run limit check- fix(g
tew
y): use Mutex for ch
nnel_registry t
ke-once inbound_rx p
ttern- fix(resilience): simplify governor session_tokens from AtomicU64 to u64- fix: upd
te doctest to use poe::met
_cognition::Beh
vior
lAnchor- fix: 
dd Clone derive to NoiseFilter 
nd remove duplic
te mod decl
r
tions- fix: remove duplic
te scoring_pipeline module decl
r
tion in memory/mod.rs- fix(clippy): resolve print_liter
l w
rnings in secret providers comm
nd- fix(tests): migr
te secret_bound
ry_integr
tion tests to 
sync- fix(runtimes): 
ddress critic
l 
nd import
nt code review findings- fix: resolve 
ll clippy w
rnings in 
leph-t
uri 
nd 
lephcore- fix(desktop): use ERR_NOT_IMPLEMENTED for stubbed methods, 
dd debug logging- fix(h
lo): 
ddress code review findings for view 
nd events- fix(h
lo): gu
rd 
g
inst empty run_id in event h
ndler- fix(h
lo): use monotonic counter for unique mess
ge IDs, remove redund
nt ph
se gu
rd- fix(desktop): restrict UDS socket to owner-only 
ccess- fix(desktop): 
dd 30s timeout to UDS request to prevent indefinite t
sk h
ng- fix(desktop): log ev
lu
teJ
v
Script errors in C
nv
s, 
dd runAsync m
in-thre
d 
ssert- fix(desktop): repl
ce deprec
ted 
ctiv
te(options:) with 
ctiv
te() for m
cOS 15- fix(desktop): 
void PNG round-trip in OCR p
th by sh
ring c
ptureCurrentScreen- fix: 
ddress code review findings- fix(desktop): repl
ce strcpy with strncpy to prevent buffer overflow- fix(desktop): require x/y for click 
nd window_id for focus_window- fix(desktop): remove misle
ding serde t
gs from DesktopRequest, 
dd From conversions- fix(skill): 
ddress code review findings- fix(skill): resolve clippy w
rnings in skill module- fix(skill): use single colon sep
r
tor for SkillId (m
tches OpenCl
w convention)- fix(st
rt): 
dd cfg gu
rd for builder mod, tighten h
ndler visibility to pub(in cr
te::comm
nds::st
rt)- fix(st
rt): move session b
nner print into register_session_h
ndlers for consistency- fix: resolve 
ll compil
tion errors from server purific
tion- fix: cle
n up rem
ining Server-Client terminology in source comments- fix: rep
ir 2 broken doc-tests in skill_evolution module- fix: resolve 8 pre-existing test f
ilures- fix(control-pl
ne): document AlertsApi integr
tion limit
tion- fix(control-pl
ne): complete mock d
t
 remov
l- fix(control-pl
ne): fix memory le
ks 
nd improve error h
ndling in 
lert subscriptions- fix(sh
red-ui-logic): improve error h
ndling in 
lerts API- fix(control-pl
ne): use T
ilwind CDN for CSS compil
tion- fix(control-pl
ne): 
dd WASM initi
liz
tion in lib.rs- fix(control-pl
ne): upd
te st
rtup log mess
ge to show correct URL- fix(control-pl
ne): fix root p
th 
ccess 
nd st
tic 
sset lo
ding- fix: resolve compil
tion errors 
nd 
dd missing imports- fix(d
shbo
rd): 
dd w
sm_bindgen entry point to en
ble 
pp initi
liz
tion- fix(g
tew
y): extr
ct guest_session_id when require_
uth=f
lse- fix: resolve compil
tion errors in 
uth 
nd guest h
ndlers- fix: use rowid inste
d of id for sqlite-vec virtu
l t
ble upd
tes- fix(ph
se2): fix RPC tests 
nd upd
te progress report- fix(cli): use correct method n
mes for session comm
nds- fix(cli): resolve event stre
ming issue between g
tew
y 
nd CLI- fix(cli): 
lign comm
nd h
ndlers with g
tew
y API- fix(memory): h
ndle new SubAgent F
ctType v
ri
nts in consolid
tion- fix: resolve f
iling BDD tests for embodiment 
nd CoT tr
nsp
rency- fix: resolve f
iling unit tests- fix: resolve module export 
nd test compil
tion errors- fix: resolve 
ll 29 compiler w
rnings- fix: 
dd dylib.* p
ttern to gitignore- fix: upd
te .gitignore for Aleph ren
me 
nd remove dylib from tr
cking- fix(compressor): fix string conc
ten
tion in tests- fix(protocols): error on nonexistent JSONP
th inste
d of returning null- fix(scr
tchp
d): use EAFP p
ttern inste
d of sync exists() checks- fix(scr
tchp
d): remove 
sync from exists() 
nd export Scr
tchp
dConfig- fix(core): fix form
t strings in m
nifest.rs 
nd doctest in pty.rs- fix: cle
n up rem
ining MultiTurnCoordin
tor references- fix(g
tew
y): remove MultiTurnCoordin
tor dependency from 
d
pter- fix(h
lo): upd
te DependencyCont
iner comment for H
loInputCoordin
tor- fix(h
lo): upd
te AppDeleg
te to use H
loInputCoordin
tor- fix(h
lo): upd
te HotkeyService to use H
loInputCoordin
tor- fix: upd
te tests for 5 builtin tools 
nd skill evolution- fix: compil
tion errors in skill evolution 
nd perception modules- fix: resolve test compil
tion errors### Ch
nged- ref
ctor: ren
me ch
tgpt → codex protocol 
cross codeb
se- ref
ctor: ren
me ToolGroup → ToolC
tegory to 
void confusion with Te
m- ph
se4: cle
n 
ll T
uri references from codeb
se- ph
se4: remove T
uri, 
rchive old 
pps, move Swift bridge to cr
tes/desktop-m
cos/bridge- ref
ctor: move CLI/TUI/WebCh
t to interf
ces/, client to sh
red/- cle
nup: remove bootstr
p 
uto-clone 
nd leg
cy plugin index code- cle
nup: remove AgentLifecycleEvent::Switched 
nd AgentRouter from inbound router- cle
nup: remove 
gent switching (tool, intent detector, /switch comm
nd)- cle
nup: remove unregistered self-m
n
gement tool source files- cle
nup: remove old sub
gent tools (sp
wn/steer/kill + deleg
te)- cle
nup: move e2e tests into tests/, remove unused sh
red_ui_logic cr
te, 
dd secret sc
nning exclusion- cle
nup: remove tempor
ry debug logging for ch
tgpt protocol- ref
ctor: ren
me worksp
ce to 
gent 
cross memory/config/p
ths, enh
nce 
gent loop 
nd Ch
tGPT protocol- cle
nup: remove zombie code, upd
te def
ult config 
nd sh
red_ui_logic- cle
nup: remove st
le ALEPH_MASTER_KEY references from docs 
nd error mess
ges- ref
ctor: fl
tten 
gent_loop/ — remove minim
l/ subdirectory- cle
nup: remove deprec
ted APIs (register_
gent_tools, with_working_dir, ToolC
tegory::N
tive, PolicyEngine stubs, AuditStore, Inv
lid
teOld)- ref
ctor: ren
me Minim
l* types to st
nd
rd n
mes — this IS the loop- cle
nup: fix clippy w
rning in leg
cy_
d
pter detect_entry_point- cle
nup: elimin
te 
ll clippy w
rnings (58→0)- cle
nup: fix clippy w
rnings (derive Def
ult, redund
nt closures, simplified condition
ls)- cle
nup: remove st
le 
pp_bundle_id references from comments 
nd BDD tests- cle
nup: remove TypeScript webch
t (repl
ced by P
nel /ch
t route)- cle
nup: remove de
d Sub
gentAuthority 
nd tools/sessions dom
in l
yer- ref
ctor: simplify memory types, use floor_ch
r_bound
ry, 
dd mtime c
che to d
ily memory- ref
ctor(pdf): split pdf_gener
te.rs into module directory- ref
ctor: strip #[cfg(fe
ture)] from g
tew
y, server, extension, 
nd misc modules- ref
ctor: strip #[cfg(fe
ture)] from 
ll 12 ch
nnel implement
tions- ref
ctor: strip 20+ C
rgo fe
ture fl
gs from core cr
te- ref
ctor: Occ
m's R
zor p
ss — elimin
te clippy w
rnings 
nd de
d code- cle
nup: remove f
stembed 
nd loc
l embedding model remn
nts- cle
nup: fix unused import in host_functions.rs- ref
ctor(w
sm): simplify PermissionChecker to f
c
de over W
smC
p
bilities- cle
nup: bro
d DRY ref
ctoring 
nd clippy compli
nce 
cross codeb
se- cle
nup: remove st
le f
stembed references, fix integr
tion tests- cle
nup: remove m
cOS-specific CI workflow 
nd build scripts (C8-C12)- cle
nup: remove deprec
ted m
cOS Swift 
pp (C7)- cle
nup: remove UniFFI Swift bindings (C1-C2)- ref
ctor(core): introduce register_h
ndler! m
cro, elimin
te h
ndler boilerpl
te (W
ve 4)- ref
ctor(core): repl
ce &Vec<T> with &[T] in 
rrow_convert 
nd sh
dow_repl
y (W
ve 3B)- ref
ctor(core): convert Intern
lEventH
ndler String p
r
ms to &str (W
ve 3A)- ref
ctor(core): m
nu
l Clippy fixes — expect_fun_c
ll, useless_vec, ptr_
rg, type_complexity, module_inception, needless_borrows, 
nd more (W
ve 2B)- ref
ctor(core): repl
ce Def
ult::def
ult() field re
ssignment with struct liter
ls (W
ve 2A)- ref
ctor(core): 
uto-fix Clippy w
rnings 
nd remove unused imports (W
ve 1)- ref
ctor(runtimes): delete old runtime m
n
gers, repl
ce with Ledger/Probe system- ref
ctor(video): repl
ce RuntimeRegistry with C
p
bilityLedger in c
ption.rs- ref
ctor(init): repl
ce forced runtime inst
ll
tion with zero-inst
ll ledger- ref
ctor(desktop): delete RPC proxy comm
nds 
nd cle
n up de
d code (~1600 lines)- ref
ctor(h
lo): delete Re
ct frontend source from T
uri 
pp- ref
ctor(h
lo): point T
uri h
lo window to Leptos server URL- ref
ctor(h
lo): delete leg
cy Swift H
lo views 
nd fix references (~4500 lines removed)- ref
ctor(st
rt): split initi
lize_
uth, extr
ct lo
d_
pp_config, restore register c
lls to orchestr
tor- ref
ctor(st
rt): move register_* h
ndler functions to comm
nds/builder/h
ndlers.rs- ref
ctor(extension): thin mod.rs f
c
de, deleg
te lo
d_
ll to ComponentLo
der- ref
ctor(st
rt): extr
ct subsystem initi
lizers from st
rt_server- ref
ctor: remove distributed execution infr
structure (ExecutionPolicy, ClientM
nifest, ReverseRpc, ToolRouter, RoutedExecutor)- ref
ctor: cle
n up 
uth h
ndler by removing ClientM
nifest references- ref
ctor: simplify g
tew
y server by removing client routing infr
structure- ref
ctor: simplify ExecutionEngine by removing client routing- ref
ctor: ren
me g
tew
y/ch
nnels/ to g
tew
y/interf
ces/- ref
ctor: ren
me clients/ to 
pps/- cle
nup: remove unused imports from exec_security_g
te (post-reb
se)- cle
nup: fix Arc misuse, l
rge v
ri
nts, 
nd priv
te interf
ces (P
ss 3 fin
l)- cle
nup: extr
ct type 
li
ses 
nd p
r
meter structs (P
ss 3)- cle
nup: suppress module_inception for intention
l nested module p
ttern- cle
nup: fix 22 miscell
neous clippy w
rnings- cle
nup: P
ss 2 loc
l ref
ctoring (clone, strip_prefix, de
d code, redund
nt closures)- cle
nup: fix boole
n simplific
tions, identity ops, 
nd &P
thBuf sign
tures- cle
nup: remove unused imports 
nd repl
ce deriv
ble impls- cle
nup: 
pply c
rgo clippy --fix 
uto-corrections- ref
ctor(control-pl
ne): split Sideb
r into sideb
r/ directory- ref
ctor(control-pl
ne): use nested routes for Settings with SettingsL
yout- ref
ctor(control-pl
ne): remove /cp prefix from routing- ref
ctor(core): ren
me 
leph-g
tew
y to 
leph-server- ref
ctor(m
cos): completely remove settings UI from m
cOS client- ref
ctor(desktop): completely remove settings UI from T
uri client- ref
ctor(desktop): migr
te Plugins, Skills, 
nd Policies settings to D
shbo
rd- ref
ctor(clients): complete Ph
se 4 - remove Gener
tion Providers UI- ref
ctor(clients): migr
te Providers, Memory, 
nd MCP config to D
shbo
rd- ref
ctor(
gent_loop): introduce RunContext p
ttern for cle
ner API- ref
ctor(
gent-loop): 
dd RunContext structure (WIP)- ref
ctor(dom
in): implement Newtype p
ttern for Answer 
nd Ruleset- ref
ctor(dom
in): implement Newtype p
ttern for 5 ID types- ref
ctor(
pi): implement FromStr tr
it for rem
ining types- ref
ctor(
pi): implement FromStr tr
it for extension 
nd resilience types- ref
ctor(
pi): implement FromStr tr
it for memory context types- ref
ctor(perf): repl
ce trim_st
rt_m
tches with strip_prefix for fixed prefixes- ref
ctor(perf): optimize &P
thBuf → &P
th in 6 files- ref
ctor(core): 
dd #[
llow(de
d_code)] to 12 reserved fields- ref
ctor(deps): remove 5 unused dependencies- ref
ctor(core): remove 2 confirmed de
d code items- ref
ctor(core): remove 160+ unused imports 
cross 50 files- ref
ctor(tools): extr
ct builtin tool registr
tion 
nd types (Ph
se 6)- ref
ctor(g
tew
y): modul
rize plugins h
ndlers (Ph
se 5.1)- ref
ctor(poe): extr
ct services to dedic
ted modules (Ph
se 4.2 - P1)- ref
ctor(poe): extr
ct h
ndler types to dedic
ted modules (Ph
se 4.1 - P0)- ref
ctor(browser): extr
ct types 
nd scripts modules (Ph
se 3 - P
rt 1)- ref
ctor(engine): complete 
tomic executor composition ref
ctoring (Ph
se 2)- ref
ctor(engine): 
dd 
tomic module b
se 
rchitecture (Ph
se 2 WIP)- ref
ctor(extension): split types.rs into modul
r structure- ref
ctor(security): tr
nsform PolicyEngine to st
teless- ref
ctor(protocol): 
dd equ
lity derives 
nd helper methods to 
uth types- ref
ctor(ph
se1): reorg
nize client directory structure- ref
ctor: complete fin
l Aether to Aleph cle
nup- ref
ctor: complete Aether to Aleph ren
me - scripts, workflows, 
nd rem
ining code- ref
ctor: complete Aether to Aleph ren
me 
cross entire codeb
se- ref
ctor(providers): use ProtocolRegistry in cre
te_provider f
ctory- ref
ctor(providers): remove technic
l 
li
s presets- ref
ctor(config): remove provider_type field from ProviderConfig- ref
ctor: fix P3 clippy w
rnings - b
tch 2- ref
ctor: fix P3 clippy w
rnings - b
tch 1- ref
ctor: fix P1/P2 clippy w
rnings 
nd improve code qu
lity- ref
ctor(providers): delete leg
cy OpenAiProvider- ref
ctor(providers): delete leg
cy GeminiProvider- ref
ctor(providers): delete leg
cy Cl
udeProvider- ref
ctor(providers): use HttpProvider for Anthropic protocol- ref
ctor(providers): remove redund
nt vendor wr
ppers (~850 lines)- ref
ctor(providers): use HttpProvider for OpenAI protocol in f
ctory- ref
ctor(m
cos): cle
nup 
nd improve hotkey/h
lo components- ref
ctor(h
lo): repl
ce H
loSt
te with simplified 6-st
te version- ref
ctor(h
lo): switch H
loWindow to V2 components- ref
ctor(h
lo): remove MultiTurn references from EventH
ndler- ref
ctor(h
lo): remove MultiTurn directory (~3000 lines)- ref
ctor: split l
rge modules into sm
ller files- cle
nup: remove unused modules 
nd merge thinking into thinker- cle
nup: elimin
te 
ll compil
tion w
rnings- cle
nup(lib): slim down exports from 590 to 272 lines- cle
nup: remove FFI-rel
ted comments- cle
nup: ren
me FFI types to st
nd
rd n
mes- cle
nup(disp
tcher): ren
me ffi.rs to tool_info.rs- cle
nup(intent): remove Type A FFI residu
ls### Build- build: unify version source — VERSION file drives 
ll version strings- rele
se: v0.2.8- docs: 
dd multimod
l probe tests implement
tion pl
n- docs: 
dd multimod
l probe tests design spec- docs: 
dd core multimod
l enh
ncement implement
tion pl
n- docs: fix spec review issues in core multimod
l design- docs: 
dd core multimod
l enh
ncement design spec- docs: 
dd Telegr
m ch
nnel enh
ncement implement
tion pl
n- docs: fix spec review issues in Telegr
m enh
ncement design- docs: 
dd Telegr
m ch
nnel enh
ncement design spec- docs: 
dd Feishu enh
nced fe
tures implement
tion pl
n- docs: 
ddress spec review — FeishuEventEmitter, typing lifecycle, c
p
bilities- docs: 
dd Feishu enh
nced fe
tures design spec- docs: 
dd Feishu ch
nnel implement
tion pl
n- docs: 
ddress spec review feedb
ck for Feishu ch
nnel- docs: 
dd Feishu/L
rk ch
nnel design spec- rele
se: v0.2.7 — multi-
gent system, UI upd
tes, bug fixes- docs: fix spec issues from review — st
le fin
l_text, test pl
n, consecutive_errors- docs: 
dd Persistent Completion Protocol design spec- docs: fix multi-
gent modes spec per review findings- docs: 
dd multi-
gent modes t
xonomy design spec- docs: 
dd t
sk coordin
tion implement
tion pl
n (12 t
sks)- docs: fix event type conventions in t
sk coordin
tion spec- docs: 
ddress spec review findings for t
sk coordin
tion- docs: 
dd t
sk coordin
tion system design spec- build: upd
te WASM p
nel dist- ci: upgr
de GitHub Actions to Node.js 24 comp
tible versions- ci: scope fmt check to m
int
ined cr
tes (skip leg
cy form
tting issues)- build: consolid
te to single rele
se workflow, fix CI protoc dependency- build: remove 
rchive from git (l
rge bin
ries exceed GitHub limit)- rele
se: bump version to 0.2.6- build: upd
te inst
ll scripts for 
leph-server bin
ry n
me- build: ren
me workflows, fix --bin 
leph→
leph-server, 
dd pl
tform rele
se workflows- build: upd
te justfile 
nd CI workflows for post-T
uri 
rchitecture- build: 
dd swift-bridge recipe to justfile for m
cOS n
tive APIs- docs: 
dd Ph
se 3 implement
tion pl
n for m
cOS PIM & system c
p
bilities- docs: 
dd Ph
se 2 implement
tion pl
n for screen control n
tive migr
tion- docs: 
ddress spec review feedb
ck for hier
rchic
l comm
nds- docs: 
dd hier
rchic
l sl
sh comm
nds design spec- docs: 
dd Ph
se 1 implement
tion pl
n for desktop n
tive c
p
bilities- docs: 
dd desktop n
tive c
p
bilities design spec- docs: upd
te design spec with new directory structure- docs: 
dd implement
tion pl
n for intermedi
te mess
ge delivery- docs: 
dd PLUGIN_SYSTEM.md — CC-comp
tible plugin 
rchitecture reference- docs: 
ddress spec review feedb
ck for CLI/TUI sep
r
tion- docs: 
dd CLI/TUI sep
r
tion design spec- docs: 
dd P4 runtime migr
tion implement
tion pl
n- docs: 
dd prompt guid
nce 
s in-scope ch
nges to intermedi
te mess
ge spec- docs: 
dd edge c
ses to intermedi
te mess
ge delivery spec- docs: 
dd intermedi
te mess
ge delivery design spec- docs: 
dd P3 scope m
n
gement implement
tion pl
n- docs: 
dd P2 m
rketpl
ce system implement
tion pl
n- docs: 
dd P0+P1 implement
tion pl
n for plugin CC comp
t- docs: fix rem
ining spec review items (round 2)- docs: 
ddress spec review findings for plugin comp
t design- docs: 
dd plugin system Cl
ude Code comp
tibility redesign spec- docs: upd
te spec 
nd pl
n — keep peer_id sign
tures unch
nged- docs: upd
te 
gent-bot 1:1 binding spec with review fixes- docs: 
dd 
gent-bot 1:1 binding simplific
tion design spec- docs: 
dd ch
t sideb
r redesign spec 
nd implement
tion pl
n- docs: 
dd p
nel 
gent routing fix design spec- docs: 
dd worksp
ce output migr
tion implement
tion pl
n- docs: revise worksp
ce output migr
tion spec 
fter review- docs: 
dd worksp
ce output migr
tion design spec- docs: 
dd gener
tion providers wiring implement
tion pl
n- docs: fix gener
tion providers spec 
fter review- docs: 
dd gener
tion providers wiring design spec- docs: 
dd Cl
wHub integr
tion implement
tion pl
n- docs: 
ddress spec review feedb
ck for Cl
wHub integr
tion- docs: 
dd Cl
wHub integr
tion design spec- ci: upgr
de GitHub Actions to Node.js 24, fix Windows de
d-code w
rnings- docs: fix pl
n review issues (3 blockers + 6 w
rnings)- docs: 
ddress spec review feedb
ck for Chrome DevTools MCP Mode- docs: 
dd Chrome DevTools MCP Mode design spec- docs: 
dd process m
n
gement rules to CLAUDE.md- docs: 
dd tool permission system implement
tion pl
n- docs: upd
te tool permission spec 
fter review- docs: 
dd tool permission system design spec- docs: 
dd ACP probe tests design document- docs: 
dd ACP h
rness m
n
gement implement
tion pl
n- docs: 
dd ACP h
rness m
n
gement design document- docs: 
dd provider routing ref
ctor implement
tion pl
n- docs: fix rem
ining spec review issues- docs: fix spec issues from review- docs: 
dd provider routing ref
ctor design spec- docs: 
dd provider config testing implement
tion pl
n- docs: upd
te provider config testing spec 
fter review- docs: 
dd provider config testing design spec- docs: 
dd simplify-model-config implement
tion pl
n- docs: upd
te simplify-model-config spec 
fter review- docs: 
dd simplify-model-config design spec- ci: re
d rele
se version from VERSION file inste
d of m
nu
l input- docs: 
dd cron probe tests implement
tion pl
n- docs: 
dd cron probe tests design spec- docs: 
dd cron module redesign implement
tion pl
n- docs: 
dd cron module redesign spec- build: rebuild p
nel WASM 
nd upd
te docs 
fter worktree merges- docs: 
dd provider zero-config implement
tion pl
n- docs: 
dd mess
ge pipeline implement
tion pl
n- docs: 
dd provider zero-config UX design spec- docs: 
dd mess
ge pipeline design for g
tew
y pre-processing- docs: 
dd model discovery probe tests implement
tion pl
n- docs: 
dd model discovery probe tests design spec- docs: 
dd model discovery implement
tion pl
n- docs: fix model discovery spec issues from review- docs: 
dd model discovery design spec- docs: 
dd cognitive evolution bet
 implement
tion pl
n- docs: 
dd cognitive evolution bet
 design (immune-complete loop)- docs: 
dd POE Ph
se 2+3 implement
tion pl
n- docs: 
dd POE Ph
se 1 implement
tion pl
n (Bl
stR
dius + T
boo)- docs: 
dd POE Architecture Evolution Whitep
per 2026- ci: fix Linux/Windows compil
tion errors for missing imports- docs: upd
te extension system 
rchitecture document
tion- docs: 
dd unified plugin system implement
tion pl
n- docs: 
dd unified plugin system design- docs: 
dd one-line inst
ll comm
nds 
s prim
ry inst
ll
tion method- docs: remove ref
ctoring b
ckstory from intent section- docs: upd
te intent detection section to reflect unified LLM pipeline- docs: 
dd det
iled Aleph vs OpenCl
w comp
rison- docs: 
dd P4.3 core plugins implement
tion pl
n- docs: 
dd plugin development guide- docs: 
dd P4 plugin ecosystem implement
tion pl
n- ci: 
dd Windows x86_64 build t
rget 
nd PowerShell inst
ller- docs: 
dd P3 medi
 pipeline implement
tion pl
n- ci: fix Linux w
rn import, remove d
rwin-x86_64 t
rget- ci: 
dd libxdo-dev for Linux, fix d
rwin x86_64 AVX-512 link error- ci: fix Linux pipewire comp
t (ubuntu-24.04) 
nd m
cOS x86_64 openssl- ci: 
dd libegl 
nd X11 extension deps for Linux build- ci: use m
cos-l
test for x86_64 cross-compile (m
cos-13 EOL)- ci: 
dd dbus, drm, gbm deps for Linux build- ci: 
dd pipewire 
nd cl
ng deps for Linux xc
p build- ci: 
dd libw
yl
nd-dev to Linux build dependencies- docs: 
dd 
uthor note to README- docs: ren
me p
nel screenshots with consistent numbering- docs: restore d
shbo
rd screenshot, keep 
ll 3 p
nel im
ges- docs: upd
te README screenshots with P
nel ch
t 
nd settings views- build: remove webch
t recipes from justfile- docs: 
dd webch
t Rust rewrite implement
tion pl
n- docs: 
dd webch
t Rust rewrite design- docs: remove 
cknowledgments section from README- ci: en
ble 
ll pl
tform build t
rgets for server rele
se- ci: 
dd m
nu
l server rele
se workflow 
nd improve inst
ll script- docs: overh
ul README.md, CLAUDE.md 
nd 
dd LICENSE- docs: 
dd inline directives 
nd leg
cy cle
nup implement
tion pl
n- docs: 
dd inline directives 
nd leg
cy cle
nup design- docs: 
dd l
ngu
ge-
gnostic intent detection implement
tion pl
n- docs: 
dd l
ngu
ge-
gnostic intent detection design- docs: upd
te cle
nup pl
n with execution results- docs: cl
rify cle
nup str
tegy — scoped responsibility, not f
llb
ck- docs: 
dd multi-
gent code redund
ncy cle
nup pl
n- docs: 
dd A2A protocol implement
tion pl
n- docs: 
dd A2A protocol design document- docs: 
dd per-
gent tool configur
tion implement
tion pl
n- docs: 
dd per-
gent tool configur
tion design- docs: 
dd multi-bot P
nel UI implement
tion pl
n- docs: 
dd multi-bot P
nel UI design- docs: 
dd multi-bot ch
nnel implement
tion pl
n- docs: 
dd multi-bot ch
nnel support design- docs: 
dd memory 
lignment design for du
l-directory 
rchitecture- docs: 
dd 
gent-worksp
ce sep
r
tion implement
tion pl
n- docs: 
dd 
gent-worksp
ce sep
r
tion design- docs: 
dd 
gent m
n
gement p
nel implement
tion pl
n- docs: 
dd 
gent m
n
gement p
nel design- docs: 
dd webch
t restructure implement
tion pl
n- docs: 
dd webch
t restructure design- docs: 
dd 
gent switching enh
ncement implement
tion pl
n- docs: 
dd 
gent switching enh
ncement design- docs: 
dd unified comm
nd registry implement
tion pl
n- docs: 
dd unified comm
nd registry design- docs: 
dd dyn
mic 
gent switching implement
tion pl
n- docs: 
dd dyn
mic 
gent switching design- docs: 
dd system prompt optimiz
tion implement
tion pl
n- docs: 
dd system prompt 
rchitecture optimiz
tion design- docs: 
dd Agent/Worksp
ce/Session unific
tion implement
tion pl
n- docs: 
dd Agent/Worksp
ce/Session rel
tionship design- docs: 
dd t
sk routing decision l
yer implement
tion pl
n- docs: 
dd t
sk routing decision l
yer design- docs: 
dd 
rchitecture 
ctiv
tion di
gnostic report- docs: 
dd 
rchitecture 
ctiv
tion di
gnostic implement
tion pl
n- docs: 
dd 
rchitecture 
ctiv
tion di
gnostic design- docs: 
dd n
tive tool_use implement
tion pl
n (9 t
sks)- docs: 
dd n
tive tool_use migr
tion design- docs: 
dd PDF du
l-engine implement
tion pl
n- docs: 
dd PDF du
l-engine rendering design- docs: 
dd cron 
nd group ch
t b
ckend implement
tion pl
n- docs: 
dd cron 
nd group ch
t b
ckend implement
tion design- docs: 
dd scheduled t
sks p
nel implement
tion pl
n- docs: 
dd scheduled t
sks p
nel design- docs: 
dd CLI full RPC cover
ge implement
tion pl
n- docs: 
dd CLI full RPC cover
ge design- docs: 
dd CLI bugfix 
nd JSON unific
tion design- docs: 
dd CLI full comm
nds implement
tion pl
n- docs: 
dd CLI full comm
nds design- docs: 
dd CLI infr
structure enh
ncement implement
tion pl
n- docs: 
dd CLI infr
structure enh
ncement design- docs: 
dd lifecycle observ
bility logging implement
tion pl
n- docs: 
dd lifecycle observ
bility logging design- docs: 
dd system prompt enh
ncement implement
tion pl
n- docs: 
dd system prompt enh
ncement design- docs: 
dd 
gent system Ph
se 2 full cover
ge implement
tion pl
n- docs: 
dd 
gent system full cover
ge design (Ph
se 2)- docs: 
dd Codex p
nel UI design 
nd implement
tion pl
n- docs: 
dd Codex Responses API implement
tion pl
n- docs: 
dd Codex Responses API protocol 
d
pter design- docs: 
dd g
tew
y enh
ncement implement
tion pl
n (20 t
sks)- docs: 
dd g
tew
y enh
ncement design (OpenCl
w-inspired)- docs: 
dd implement
tion pl
n for 
gent/worksp
ce/binding- docs: 
dd 
gent definition + worksp
ce + binding design- docs: 
dd OpenAI subscription provider implement
tion pl
n- docs: 
dd OpenAI subscription provider design- docs: 
dd L
zy POE Activ
tion design- build: ren
me just server → just build, 
dd just 
ll- docs: upd
te bin
ry n
me 
nd port references 
cross 
ll document
tion- build: en
ble 
xum ws fe
ture for port unific
tion- docs: 
dd port unific
tion implement
tion pl
n- docs: 
dd port unific
tion 
nd bin
ry ren
me design- docs: 
dd ch
nnel infr
structure fix implement
tion pl
n- docs: 
dd ch
nnel infr
structure fix design- docs: upd
te CLAUDE.md for fe
ture fl
g remov
l- build: simplify justfile — remove 
ll --fe
tures fl
gs- docs: 
dd runtime ch
nnel control implement
tion pl
n- docs: 
dd runtime ch
nnel control design — elimin
te fe
ture fl
g fr
gment
tion- docs: 
dd ch
t persistence & memory pipeline implement
tion pl
n- docs: 
dd ch
t persistence & memory pipeline fix design- docs: 
dd full ch
in + sm
rt rec
ll implement
tion pl
n- docs: 
dd full ch
in + sm
rt rec
ll design- docs: 
dd worksp
ce enh
ncements implement
tion pl
n (9 t
sks)- docs: 
dd worksp
ce enh
ncements design (4 fe
tures)- docs: 
dd worksp
ce wiring implement
tion pl
n (11 t
sks)- docs: 
dd worksp
ce wiring design for multi-role person
 system- docs: 
dd config extern
liz
tion implement
tion pl
n- docs: 
dd config extern
liz
tion design for ~/.
leph worksp
ce- ci: keep only m
cOS ARM64 build, document other pl
tform blockers- ci: fix rem
ining build issues 
cross pl
tforms- ci: fix cross-pl
tform build issues- ci: pin w
sm-bindgen-cli to 0.2.108 m
tching C
rgo.lock- ci: 
llow test job to f
il without blocking builds- ci: 
dd X11/xscrns
ver dev libr
ries for Linux builds- ci: inst
ll protoc for l
nce-encoding build dependency- ci: improve rele
se workflow with WASM build, test job, 
nd cross-pl
tform desktop- build: rewrite justfile for desktop-
s-muscle 
rchitecture- docs: 
dd cr
tes/desktop to project structure 
nd build comm
nds- docs: 
dd Desktop-
s-Muscle implement
tion pl
n- docs: 
dd Desktop-
s-Muscle 
rchitecture design- docs: 
dd self-configur
tion implement
tion pl
n- docs: 
dd self-configur
tion design document- ci: 
dd loom concurrency test job 
nd incre
se proptest cover
ge- build: 
dd test-proptest, test-loom, test-logic just recipes- docs: 
dd logic review system implement
tion pl
n (15 t
sks, 49 properties)- docs: 
dd logic review system design (three-l
yer defense 
rchitecture)- docs: move obsolete embedding/sqlite-vec pl
ns to leg
cy- docs: upd
te memory system docs to reflect remote embedding migr
tion- build: repl
ce trunk with m
nu
l WASM pipeline in justfile- docs: fix m
cOS Resources p
th in build pipeline design- build: 
dd justfile for unified build pipeline- docs: 
dd unified build pipeline design- docs: 
dd ch
nnel config p
nel implement
tion pl
n- docs: 
dd ch
nnel config p
nel design document- docs: 
dd POE full evolution implement
tion pl
n (19 t
sks, 4 ph
ses)- docs: 
dd POE full evolution design (event-driven closed loop)- docs: 
dd WASM c
p
bility kernel implement
tion pl
n- docs: 
dd WASM c
p
bility kernel design- docs: 
dd m
cOS PIM n
tive API implement
tion pl
n- docs: 
dd m
cOS PIM n
tive API integr
tion design- docs: 
dd POE cognitive hub implement
tion pl
n- docs: 
dd POE cognitive hub upgr
de design- docs: 
dd soci
l bot ch
nnels exp
nsion implement
tion pl
n- docs: 
dd soci
l bot ch
nnels exp
nsion design- docs: 
dd surgic
l DRY ref
ctoring implement
tion pl
n- docs: 
dd surgic
l DRY ref
ctoring design for embedding provider files- docs: 
dd embedding provider LLM migr
tion implement
tion pl
n- docs: 
dd embedding provider LLM migr
tion design- docs: 
dd l
rge file ref
ctoring implement
tion pl
n — 6 t
sks, 5 files- docs: 
dd l
rge file ref
ctoring design — 5 files, pure module splitting- ci: 
dd server, m
cOS 
pp, 
nd T
uri rele
se workflows- docs: 
dd distribution implement
tion pl
n (24 t
sks, 9 ph
ses)- docs: 
dd distribution 
rchitecture design- docs: 
dd PromptPipeline implement
tion pl
n — 10 t
sks, TDD, str
ngler fig- docs: 
dd PromptPipeline design — Tr
it-per-L
yer evolution from Pl
n A- docs: 
dd 
utom
tion skills implement
tion pl
n- docs: 
dd 
utom
tion skills (#21-30) design- docs: 
dd memory event sourcing implement
tion pl
n- docs: 
dd memory event sourcing design (CQRS Light)- docs: 
dd prompt system enh
ncement implement
tion pl
n- docs: 
dd prompt system enh
ncement design- docs: 
dd skills system, upd
te runtimes refs, 
dd m
cOS components- docs: upd
te 
ccept
nce results 
fter bridge fixes (27/30 p
ss)- docs: 
dd implement
tion pl
n for fixing bridge known issues- docs: 
dd design for fixing bridge known issues- docs: remove rem
ining Swift references from CLAUDE.md- docs: upd
te CLAUDE.md 
nd cre
te migr
tion completion record (C13-C16)- docs: 
dd m
cOS Swift 
pp remov
l implement
tion pl
n- docs: 
dd m
cOS Swift 
pp remov
l design with 
ccept
nce criteri
- docs: 
dd desktop c
p
bilities evolution implement
tion pl
n- docs: 
dd desktop c
p
bilities evolution design- docs: 
dd sem
ntic t
rgeting implement
tion pl
n- docs: 
dd sem
ntic t
rgeting 
nd 
ction primitives design- docs: upd
te CLAUDE.md for Server-Centric Build Architecture- docs: 
dd Ph
se 3 
nd Ph
se 4 implement
tion pl
ns- docs: repl
ce Ghost 
esthetic with concrete product constr
ints R5-R7- docs: 
dd Ph
se 2.5 bridge integr
tion completion pl
n- docs: 
dd design for removing Ghost 
esthetic concept- docs: 
dd Ph
se 1 bridge skeleton implement
tion pl
n- docs: 
dd server-centric build 
rchitecture design- docs: upd
te worktree guidelines with EnterWorktree CWD lock c
ve
t- docs: 
dd cron system redesign pl
n — surp
ssing opencl
w- docs: 
dd memory optimiz
tion implement
tion pl
n- docs: 
dd memory module optimiz
tion design- docs: 
ddress code review findings (JIT-
pprov
l TODO, RwLock r
tion
le)- docs: bring in L
te-Binding Secure Execution design 
nd pl
n from m
in- docs: 
dd L
te-Binding Secure Execution implement
tion pl
n (14 t
sks, 4 w
ves)- docs: 
dd L
te-Binding Secure Execution Architecture design- docs: 
dd git worktree s
fety guide; fix missing ScreenRegion import- docs: 
dd Rust ref
ctoring implement
tion pl
n (7 t
sks, 4 w
ves)- docs: 
dd Rust core ref
ctoring design (4-w
ve str
tegy)- docs: 
dd runtime on-dem
nd implement
tion pl
n (13 t
sks, 4 ph
ses)- docs: 
dd runtime on-dem
nd implement
tion pl
n (13 t
sks, 4 ph
ses)- docs: 
dd runtime on-dem
nd n
tive bootstr
pping 
rchitecture design- docs: 
dd verific
tion test results to T
uri shell design doc- docs: 
dd T
uri cross-pl
tform shell implement
tion pl
n- docs: 
dd T
uri cross-pl
tform shell & DesktopBridge design- build(h
lo): rebuild WASM with /h
lo route- docs: split CLAUDE.md 
nd reorg
nize docs/ into docs/reference/- docs: 
dd 1-2-3-4 
rchitecture constitution design document- docs: 
dd H
lo UI Unific
tion implement
tion pl
n (10 t
sks)- docs: est
blish 1-2-3-4 
rchitecture model 
s constitution
l principles in CLAUDE.md- build(m
cos): 
dd WebKit fr
mework dependency for Settings WebView- docs: 
dd Ph
se 1 implement
tion pl
n — Settings WebView integr
tion- docs: 
dd UI unific
tion design — Leptos 
s single UI codeb
se- docs: 
dd Desktop Bridge implement
tion pl
n (11 t
sks, 4 ph
ses)- docs: 
dd Desktop Bridge design for UDS-b
sed Swift-Rust IPC- docs: 
dd Skill System v2 implement
tion pl
n (15 TDD t
sks)- docs: 
dd Skill System v2 design (complete DDD rebuild)- docs: upd
te 
ll document
tion for server-centric 
rchitecture- docs: upd
te CLAUDE.md for server-centric 
rchitecture- docs: 
dd server purific
tion implement
tion pl
n- docs: 
dd server purific
tion design - remove desktop control, embr
ce MCP plugins- docs: 
dd Skill System implement
tion pl
n with 14 TDD t
sks- docs: 
dd server-centric 
rchitecture implement
tion pl
n- docs: 
dd server-centric 
rchitecture refr
ming design- docs: 
dd Skill System dom
in-driven design document- docs: 
dd P0 ref
ctoring implement
tion pl
n for st
rt.rs 
nd extension/mod.rs- docs: 
dd CODE_ORGANIZATION guide with ref
ctoring b
cklog- docs: 
dd soci
l connectivity evolution design 
nd implement
tion pl
n- build: 
dd missing imports in control-pl
ne cfg block- docs: 
dd IronCl
w Ph
se 2/3 det
iled implement
tion pl
n- docs: 
dd IronCl
w Ph
se 2/3 design (host-bound
ry + EVM signing)- docs: 
dd code cle
nup implement
tion pl
n (16 t
sks, 3 p
sses)- docs: 
dd code cle
nup design pl
n (Occ
m's R
zor P
ss)- docs: 
dd ACMA implement
tion pl
n with 7 TDD t
sks- docs: 
dd ACMA (Aleph Cognitive Memory Architecture) design document- docs: 
dd exec security integr
tion design- docs: 
dd blog post on PII filtering g
tew
y implement
tion- docs: 
dd 
gent secret m
n
gement implement
tion pl
n- docs: 
dd 
gent secret m
n
gement design (Ph
se 1)- docs: 
dd Discord Control Pl
ne implement
tion pl
n- docs: 
dd Discord Control Pl
ne p
nel design- docs: 
dd memory worksp
ce implement
tion pl
n- docs: 
dd memory worksp
ce isol
tion design- docs: upd
te 
rchitecture docs to reflect L
nceDB migr
tion- docs: 
dd Wh
tsApp Bridge implement
tion pl
n (10 t
sks)- docs: 
dd Wh
tsApp Bridge design (Thin Sidec
r + Rich Ad
pter)- docs: upd
te MEMORY_SYSTEM.md 
nd CLAUDE.md for L
nceDB migr
tion- docs: embedding evolution implement
tion pl
n (13 t
sks)- docs: embedding evolution design (
bstr
ct provider + l
zy migr
tion)- docs: 
dd Memory VFS Evolution implement
tion pl
n- docs: 
dd Memory VFS Evolution design document- docs: 
dd Sw
rm Agent Loop integr
tion implement
tion pl
n- docs: 
dd Sw
rm Intelligence Architecture Agent Loop integr
tion design- docs(ssb): 
dd Ph
se 6 cross-pl
tform implement
tion pl
n- docs(ssb): 
dd cross-pl
tform 
rchitecture design- docs: cl
rify server-side execution model in CLAUDE.md- docs(ssb): 
dd Ph
se 6 enh
ncement pl
n 
nd complete ro
dm
p- docs: 
dd Sw
rm Intelligence Architecture design- build(control-pl
ne): upd
te compiled UI 
ssets for Ph
se 3- docs: 
dd System St
te Bus (SSB) 
rchitecture design- docs(skill-evolution): 
dd comprehensive document
tion 
nd ex
mples- docs: 
dd Coll
bor
tive Skill Evolution 
rchitecture design- docs: 
dd det
iled implement
tion pl
n for Control Pl
ne three-column l
yout- docs: 
dd Control Pl
ne three-column l
yout 
rchitecture design- docs: upd
te Control Pl
ne UI build workflow with T
ilwind CSS compil
tion- docs(cl
ude.md): 
dd WASM initi
liz
tion mech
nism expl
n
tion- docs(cl
ude.md): 
dd comprehensive Server development 
nd deployment guide- docs: 
dd UI comp
rison 
n
lysis for ControlPl
ne 
nd T
uri settings- docs: 
dd WebSocket client implement
tion summ
ry 
nd migr
tion pl
n- docs: 
dd ControlPl
ne integr
tion implement
tion summ
ry- docs: 
dd Ph
se 3 implement
tion pl
n- docs: 
dd Ph
se 3 design for skill s
ndboxing- docs: 
dd comprehensive skill s
ndboxing document
tion- docs: 
dd Ph
se 2 skill s
ndboxing implement
tion pl
n- docs: 
dd Ph
se 2 skill s
ndboxing design document- docs(sh
red-ui-logic): m
rk API L
yer 
s complete- docs(sh
red-ui-logic): m
rk WASM connector 
s complete- docs(sh
red-ui-logic): upd
te README with API 
nd Observ
bility progress- docs(sh
red_ui_logic): upd
te README with protocol l
yer st
tus- docs(sh
red_ui_logic): upd
te README with n
tive connector st
tus- docs(sh
red_ui_logic): 
dd comprehensive README- docs: 
dd sh
red_ui_logic design document- docs: complete Ph
se 3 
rchitecture document
tion- docs: 
dd Ph
se 1 implement
tion pl
n for skill s
ndboxing- docs: 
dd skill s
ndboxing 
rchitecture design- docs(
rchitecture): 
dd comprehensive cle
nup design document- docs: reorg
nize root directory 
nd est
blish document
tion structure- docs(
rchitecture): 
dd Ph
se 3 browser ref
ctoring design- docs(
rchitecture): 
dd Ph
se 6 tools server ref
ctoring design- docs(
rchitecture): 
dd Ph
se 5 plugins h
ndlers ref
ctoring design- docs(
rchitecture): 
dd Ph
se 4 POE h
ndlers ref
ctoring design- docs: 
dd Ph
se 2 continu
tion guide for next session- docs(
rchitecture): 
dd Ph
se 2 
tomic executor ref
ctoring design- docs(
rchitecture): 
dd Ph
se 1 types ref
ctoring design- docs(cortex): 
dd Month 3 implement
tion pl
n- docs(cortex): 
dd Month 3 Met
-Cognition L
yer design- docs: 
dd Atomic Engine fin
l implement
tion report- docs: 
dd comprehensive Atomic Engine document
tion- docs: 
dd Atomic Engine progress report (90% complete)- docs: 
dd Atomic Engine short-term t
sk completion st
tus- docs: 
dd Cortex evolution system design- docs: 
dd Atomic Engine evolution ro
dm
p (3-12+ months)- docs: 
dd 
tomic engine implement
tion st
tus report- docs: 
dd l
ngu
ge preference to CLAUDE.md- docs: 
dd Ph
se 2 Intelligent Scheduling design- docs: 
dd guest session 
ctivity logging implement
tion pl
n- docs: 
dd Liquid Hub cross-pl
tform 
rchitecture design- docs: complete Identity Context security document
tion- docs: 
dd Identity Context & Security Enforcement design- docs: 
dd ConfigM
n
ger 
nd Memory N
mesp
ce implement
tion pl
n- docs: 
dd ConfigM
n
ger 
nd Memory N
mesp
ce design- docs: 
dd Person
l AI Hub implement
tion pl
n- docs: 
dd Person
l AI Hub 
rchitecture design- docs: 
dd client 
rchitecture document
tion 
nd testing guide- docs: 
dd Ph
se 2 progress report- docs: 
dd client 
rchitecture ref
ctoring pl
n- docs: document Server-Client 
rchitecture in CLAUDE.md- docs: 
dd Server-Client implement
tion pl
n- docs: 
dd Server-Client 
rchitecture design- docs: 
dd DDD terminology 
nd dom
in modeling guide- docs: 
dd DDD+BDD du
l-wheel 
rchitecture design- docs: 
dd comprehensive Tool-
s-Resource us
ge guide 
nd upd
te Ph
se 4 st
tus- docs: upd
te Ph
se 3 progress - L2 
nd observ
bility completed- docs: upd
te Ph
se 2 checkboxes to completed- docs: upd
te MEMORY_SYSTEM.md with Memory Evolution fe
tures- docs(bdd): 
dd comprehensive BDD testing guide 
nd upd
te pl
ns- docs: 
dd Ph
se 3 implement
tion pl
n- docs: m
rk Ph
se 2 
s complete with 
ll t
sks done- docs: document Ph
se 2 memory system components in TOOL_SYSTEM.md- docs: upd
te Ph
se 2 pl
n with completion st
tus- docs: upd
te implement
tion pl
n with completion summ
ry- docs: 
dd Ph
se 1 MVP implement
tion pl
n- docs: 
dd Multi-Agent 2.0 Ph
se 1 implement
tion pl
n- docs: 
dd memory system evolution design- docs: 
dd Multi-Agent Resilience document
tion- docs: upd
te Ph
se 1 checkboxes to completed- docs: upd
te Tool-
s-Resource design st
tus to In Progress- docs: 
dd Tool-
s-Resource implement
tion pl
n- docs: 
dd Multi-Agent Resilience & Govern
nce 
rchitecture design- docs: 
dd Tool-
s-Resource 
rchitecture design- docs: 
dd Embodiment Engine 
nd CoT Tr
nsp
rency document
tion- docs: 
dd Multi-Agent 2.0 
rchitecture design- docs(pl
ns): 
dd Embodiment Engine & CoT Tr
nsp
rency design- docs(
gent-system): 
dd Ch
nnel C
p
bility Aw
reness document
tion- docs: 
dd ch
nnel c
p
bility 
w
reness implement
tion pl
n- docs: 
dd ch
nnel c
p
bility 
w
reness 
rchitecture design- docs: 
dd worksp
ce 
rchitecture design- docs: 
dd Ph
se 5 implement
tion pl
n- docs: 
dd Ph
se 5 Custom Rules Engine 
rchitecture design- docs: 
dd WorldModel + Disp
tcher 
rchitecture design- docs(d
emon): 
dd perception l
yer document
tion- docs: 
dd Protocol Ad
pter Ph
se 4 implement
tion summ
ry- docs(
rchitecture): document configur
ble protocol 
d
pter system- docs(protocols): 
dd comprehensive protocol 
d
pter user guide- docs: 
dd Ph
se 2 Perception L
yer implement
tion pl
n- docs(protocols): 
dd ex
mple YAML protocol configur
tions- docs: 
dd Ph
se 2 Perception L
yer design- docs: 
dd d
emon module document
tion- docs: 
dd Ph
se 1 d
emon implement
tion pl
n- docs: 
dd pro
ctive AI 
rchitecture design- build: remove deprec
ted c
bi fe
ture 
nd fix Discord API- docs: 
dd comprehensive M
rkdown Tool Ad
pter implement
tion summ
ry- docs: 
dd Protocol Ad
pter Ph
se 4 design- docs: 
dd M
rkdown Tool Ad
pter design specific
tion- docs: 
dd Protocol Ad
pter Ph
se 3 implement
tion summ
ry- docs: 
dd Protocol Ad
pter Ph
se 2 implement
tion summ
ry- docs: 
dd Protocol Ad
pter Ph
se 2 implement
tion pl
n- docs: 
dd Protocol Ad
pter Ph
se 2 design for Cl
ude/Gemini migr
tion- docs(providers): upd
te module document
tion for Protocol Ad
pter 
rchitecture- docs: 
dd Protocol Ad
pter implement
tion pl
n- docs: 
dd Protocol Ad
pter 
rchitecture design- docs(pl
ns): 
dd P2.5 MCP Adv
nced Fe
tures implement
tion pl
n- docs(mcp): 
dd P2 
dv
nced fe
tures implement
tion pl
n- docs: 
dd Memory v3 implement
tion pl
n with bite-sized TDD t
sks- docs(mcp): 
dd P1 c
p
bilities implement
tion pl
n- docs: 
dd Memory System v3 "Gl
ss Box" 
rchitecture design- docs(mcp): 
dd MCP Orchestr
tion L
yer implement
tion pl
n- docs(mcp): 
dd MCP Orchestr
tion L
yer design- docs(cortex): 
dd det
iled implement
tion pl
n with TDD steps- docs(extension): 
dd P0.5-P2 fe
ture document
tion- docs(extension): 
dd P0.5-P2 implement
tion pl
n- docs(extension): 
dd SDK V2 document
tion- docs(disp
tcher): 
dd Cortex 2.0 
rchitecture design- docs(extension): 
dd SDK V2 P0 implement
tion pl
n- docs(extension): 
dd Aether Extension SDK V2 design specific
tion- docs(skills): 
dd det
iled implement
tion pl
n for requirements fe
ture- docs(skills): 
dd requirements & CLI wr
pper 
rchitecture design- docs(poe): 
dd contr
ct signing design for first principles closure- docs: upd
te memory system docs 
nd 
dd h
lo comm
nd system pl
n- docs: 
dd mess
ge flow optimiz
tion design 
nd implement
tion pl
n- docs: 
dd H
lo-Only mess
ge flow design 
nd implement
tion pl
n- docs: 
dd comprehensive 
rchitecture document
tion- docs: 
dd det
iled POE implement
tion pl
n- docs: 
dd POE (Principle-Oper
tion-Ev
lu
tion) 
rchitecture design- docs: 
dd Agent-Action inter
ction implement
tion pl
n- docs: 
dd Agent-Action inter
ction system design- docs: m
rk Milestone 6 (ResilientT
sk) 
s complete- docs: 
dd Rust l
yer code cle
nup design pl
n- docs: 
dd Milestone 6 resilient t
sk implement
tion pl
n- docs: m
rk Milestone 5 (skill evolution) 
s complete- docs: 
dd Milestone 5 skill evolution implement
tion pl
n- docs: m
rk Milestone 4 (spec-driven dev) 
s complete- docs: 
dd Milestone 4 spec-driven development implement
tion pl
n- docs: m
rk Milestone 3 (Telegr
m 
pprov
l) 
s complete

### Added
- Added "Conversation Modes" section to CLAUDE.md documenting single-turn (FROZEN) and multi-turn (ACTIVE) mode boundaries
- Added "Conversation Modes" chapter to ARCHITECTURE.md with detailed implementation and data flow diagrams
- Added conversation history saving logic in Agent Loop for multi-turn mode (`agent_loop.rs:319-352`)
- Added automatic history trimming (max 50 messages = 25 turns) to prevent memory bloat
- Added 🔒 FROZEN and ✅ ACTIVE emoji markers throughout codebase to clarify mode boundaries

### Fixed
- **Critical**: Fixed missing conversation history saving in Agent Loop, enabling proper context injection for multi-turn conversations
- Multi-turn conversations now correctly accumulate and inject conversation history from previous turns

### Changed
- Enhanced `ProcessOptions.topic_id` documentation with comprehensive mode explanation
- Improved code comments in `orchestration.rs`, `agent_loop.rs`, and `prompt_helpers.rs` with mode-specific annotations
- Updated function documentation for `build_history_summary_from_conversations` to clarify single-turn vs multi-turn behavior

### Developer Notes
- **Single-turn mode** is now feature-locked (FROZEN). All future enhancements target multi-turn mode.
- Development constraint: Modifications to single-turn code paths require explicit approval (bug fixes only)
- Multi-turn mode is the active development focus for AI agent capabilities

---

## [0.1.0] - 2026-01-27

### Project Status
- Phase 9 Complete: Agent Loop Hardening
- Established architectural boundary between single-turn and multi-turn conversation modes
- Clarified development direction: single-turn frozen, multi-turn active

---

## Format Guidelines

### Types of Changes
- **Added** for new features
- **Changed** for changes in existing functionality
- **Deprecated** for soon-to-be removed features
- **Removed** for now removed features
- **Fixed** for any bug fixes
- **Security** in case of vulnerabilities

### Commit Message Convention
```
<type>(<scope>): <subject>

<body>

Co-Authored-By: Claude Sonnet 4.5 <noreply@anthropic.com>
```

Types: `feat`, `fix`, `docs`, `style`, `refactor`, `test`, `chore`
