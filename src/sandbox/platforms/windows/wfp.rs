//! Per-run WFP confinement backend (#48/#53, slice E).
//!
//! One dynamic `Fwpm` session per run with a random provider GUID and a
//! random sublayer GUID. Filters come only from
//! [`build_filter_specs`] for the run's AppContainer package SID: per ALE
//! layer one catch-all `Block` plus one `Permit` per exact loopback TCP
//! port. Nothing outside the run SID is matched; there is no loopback-wide
//! or host permit.
//!
//! Readback enumerates every filter carrying the run's provider key on each
//! layer, decodes it back into a [`FilterSpec`] and compares the multiset
//! exactly ([`verify_filter_readback`]). Removal deletes filters, sublayer and
//! provider in one transaction and re-enumerates to confirm nothing is left.
//! The session is dynamic, so if the owning process dies the base filtering
//! engine drops everything as well (the job is kill-on-close, so the service
//! dies with it).
//!
//! Status: installing filters needs Administrator. The driver never enables
//! this backend on its own: `CURRENT_CAPABILITIES` stays all-false until the
//! admin experiment (`admin_tests`) has proven that the permit filters win
//! arbitration against AppContainer loopback isolation on the target host.

use windows_sys::core::GUID;
use windows_sys::Win32::Foundation::{LocalFree, HANDLE};
use windows_sys::Win32::NetworkManagement::WindowsFilteringPlatform::*;

use super::launcher::ConfinementBackend;
use super::managed_launch::{
    build_filter_specs, verify_filter_readback, ConfinementPlan, FilterSpec, RunSid, WfpAction,
    WfpLayer, WfpMatch,
};

const IPPROTO_TCP: u8 = 6;
/// `RPC_C_AUTHN_DEFAULT`.
const AUTHN_DEFAULT: u32 = 0xFFFF_FFFF;
const SUBLAYER_WEIGHT: u16 = u16::MAX;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn check(step: &str, code: u32) -> Result<(), String> {
    if code == 0 {
        Ok(())
    } else {
        Err(format!("{step} failed: {code:#010x}"))
    }
}

fn guid_eq(a: &GUID, b: &GUID) -> bool {
    a.data1 == b.data1 && a.data2 == b.data2 && a.data3 == b.data3 && a.data4 == b.data4
}

fn random_guid() -> GUID {
    GUID::from_u128(uuid::Uuid::new_v4().as_u128())
}

fn layer_key(layer: WfpLayer) -> GUID {
    match layer {
        WfpLayer::ConnectV4 => FWPM_LAYER_ALE_AUTH_CONNECT_V4,
        WfpLayer::ConnectV6 => FWPM_LAYER_ALE_AUTH_CONNECT_V6,
        WfpLayer::ListenV4 => FWPM_LAYER_ALE_AUTH_LISTEN_V4,
        WfpLayer::ListenV6 => FWPM_LAYER_ALE_AUTH_LISTEN_V6,
        WfpLayer::RecvAcceptV4 => FWPM_LAYER_ALE_AUTH_RECV_ACCEPT_V4,
        WfpLayer::RecvAcceptV6 => FWPM_LAYER_ALE_AUTH_RECV_ACCEPT_V6,
    }
}

const LAYERS: [WfpLayer; 6] = [
    WfpLayer::ConnectV4,
    WfpLayer::ConnectV6,
    WfpLayer::ListenV4,
    WfpLayer::ListenV6,
    WfpLayer::RecvAcceptV4,
    WfpLayer::RecvAcceptV6,
];

fn layer_from_key(key: &GUID) -> Option<WfpLayer> {
    LAYERS.into_iter().find(|l| guid_eq(&layer_key(*l), key))
}

/// Binary SID parsed from its string form; `LocalFree`d on drop.
struct BinarySid(*mut core::ffi::c_void);

impl BinarySid {
    fn parse(sid: &RunSid) -> Result<Self, String> {
        use windows_sys::Win32::Security::Authorization::ConvertStringSidToSidW;
        let w = wide(sid.as_str());
        let mut out: *mut core::ffi::c_void = std::ptr::null_mut();
        // SAFETY: `w` is NUL-terminated; `out` is a valid out-pointer.
        if unsafe { ConvertStringSidToSidW(w.as_ptr(), &mut out) } == 0 || out.is_null() {
            return Err("ConvertStringSidToSidW failed".into());
        }
        Ok(Self(out))
    }
}

impl Drop for BinarySid {
    fn drop(&mut self) {
        // SAFETY: allocated by ConvertStringSidToSidW with LocalAlloc.
        unsafe { LocalFree(self.0) };
    }
}

fn sid_to_string(sid: *mut core::ffi::c_void) -> Result<String, String> {
    use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
    let mut out: *mut u16 = std::ptr::null_mut();
    // SAFETY: `sid` came from a filter returned by BFE; `out` is valid.
    if unsafe { ConvertSidToStringSidW(sid, &mut out) } == 0 || out.is_null() {
        return Err("ConvertSidToStringSidW failed".into());
    }
    let mut len = 0usize;
    // SAFETY: NUL-terminated LocalAlloc'd string.
    let text = unsafe {
        while *out.add(len) != 0 {
            len += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(out, len))
    };
    // SAFETY: allocated by ConvertSidToStringSidW.
    unsafe { LocalFree(out as *mut core::ffi::c_void) };
    Ok(text)
}

/// Open dynamic `Fwpm` engine session; closed (and every dynamic object
/// dropped by BFE) on drop.
struct Engine(HANDLE);

// SAFETY: an engine handle may be used from any thread (one call at a time,
// which `&mut`/ownership guarantees here).
unsafe impl Send for Engine {}

impl Engine {
    fn open_dynamic() -> Result<Self, String> {
        let mut name = wide("Aleph managed run");
        let mut desc = wide("Per-run confinement for one managed service");
        // SAFETY: zero-initialized POD session, then filled in.
        let mut session: FWPM_SESSION0 = unsafe { std::mem::zeroed() };
        session.displayData.name = name.as_mut_ptr();
        session.displayData.description = desc.as_mut_ptr();
        session.flags = FWPM_SESSION_FLAG_DYNAMIC;
        let mut h: HANDLE = std::ptr::null_mut();
        // SAFETY: valid session struct and out-pointer.
        check("FwpmEngineOpen0", unsafe {
            FwpmEngineOpen0(
                std::ptr::null(),
                AUTHN_DEFAULT,
                std::ptr::null(),
                &session,
                &mut h,
            )
        })?;
        Ok(Self(h))
    }

    fn transaction<T>(&self, f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
        // SAFETY: valid engine handle.
        check("FwpmTransactionBegin0", unsafe {
            FwpmTransactionBegin0(self.0, 0)
        })?;
        match f() {
            Ok(v) => {
                // SAFETY: valid engine handle with an open transaction.
                check("FwpmTransactionCommit0", unsafe {
                    FwpmTransactionCommit0(self.0)
                })?;
                Ok(v)
            }
            Err(e) => {
                // SAFETY: valid engine handle with an open transaction.
                unsafe { FwpmTransactionAbort0(self.0) };
                Err(e)
            }
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        // SAFETY: owned engine handle, closed once.
        unsafe { FwpmEngineClose0(self.0) };
    }
}

/// Installed per-run confinement. Dropping it closes the dynamic session
/// (BFE removes the objects); [`WfpBackend::remove`] does it explicitly and
/// verifies. The launcher `mem::forget`s it when the job is not confirmed
/// empty, so the filters outlive any unreaped service.
pub(crate) struct WfpConfinement {
    engine: Engine,
    provider: GUID,
    sublayer: GUID,
    filter_ids: Vec<u64>,
    expected: Vec<FilterSpec>,
}

/// Production [`ConfinementBackend`].
#[derive(Debug, Default)]
pub(crate) struct WfpBackend;

fn add_filter(
    engine: &Engine,
    provider: &mut GUID,
    sublayer: GUID,
    spec: &FilterSpec,
    sid: &BinarySid,
) -> Result<u64, String> {
    let mut name = wide("Aleph managed run filter");
    let mut conds: Vec<FWPM_FILTER_CONDITION0> = Vec::with_capacity(4);
    // SAFETY: zeroed POD, fields set below.
    let mut c: FWPM_FILTER_CONDITION0 = unsafe { std::mem::zeroed() };
    c.fieldKey = FWPM_CONDITION_ALE_PACKAGE_ID;
    c.matchType = FWP_MATCH_EQUAL;
    c.conditionValue.r#type = FWP_SID;
    c.conditionValue.Anonymous.sid = sid.0 as *mut _;
    conds.push(c);
    if let WfpMatch::LoopbackTcpPort { port, remote } = spec.matcher {
        // SAFETY: zeroed POD.
        let mut f: FWPM_FILTER_CONDITION0 = unsafe { std::mem::zeroed() };
        f.fieldKey = FWPM_CONDITION_FLAGS;
        f.matchType = FWP_MATCH_FLAGS_ALL_SET;
        f.conditionValue.r#type = FWP_UINT32;
        f.conditionValue.Anonymous.uint32 = FWP_CONDITION_FLAG_IS_LOOPBACK;
        conds.push(f);
        if layer_has_protocol_condition(spec.layer) {
            // SAFETY: zeroed POD.
            let mut p: FWPM_FILTER_CONDITION0 = unsafe { std::mem::zeroed() };
            p.fieldKey = FWPM_CONDITION_IP_PROTOCOL;
            p.matchType = FWP_MATCH_EQUAL;
            p.conditionValue.r#type = FWP_UINT8;
            p.conditionValue.Anonymous.uint8 = IPPROTO_TCP;
            conds.push(p);
        }
        // SAFETY: zeroed POD.
        let mut q: FWPM_FILTER_CONDITION0 = unsafe { std::mem::zeroed() };
        q.fieldKey = if remote {
            FWPM_CONDITION_IP_REMOTE_PORT
        } else {
            FWPM_CONDITION_IP_LOCAL_PORT
        };
        q.matchType = FWP_MATCH_EQUAL;
        q.conditionValue.r#type = FWP_UINT16;
        q.conditionValue.Anonymous.uint16 = port;
        conds.push(q);
    }
    // SAFETY: zeroed POD filter, fields set below.
    let mut filter: FWPM_FILTER0 = unsafe { std::mem::zeroed() };
    filter.displayData.name = name.as_mut_ptr();
    filter.providerKey = provider;
    filter.layerKey = layer_key(spec.layer);
    filter.subLayerKey = sublayer;
    filter.weight.r#type = FWP_UINT8;
    filter.weight.Anonymous.uint8 = spec.weight;
    filter.numFilterConditions = conds.len() as u32;
    filter.filterCondition = conds.as_mut_ptr();
    match spec.action {
        WfpAction::Permit => {
            filter.action.r#type = FWP_ACTION_PERMIT;
            // Let the exact permit override lower-priority sublayers' blocks
            // (whether it beats AppContainer loopback isolation is exactly
            // what the admin experiment has to prove).
            filter.flags = FWPM_FILTER_FLAG_CLEAR_ACTION_RIGHT;
        }
        WfpAction::Block => {
            filter.action.r#type = FWP_ACTION_BLOCK;
            filter.flags = FWPM_FILTER_FLAG_NONE;
        }
    }
    let mut id: u64 = 0;
    // SAFETY: every pointer in `filter` points to live locals.
    check("FwpmFilterAdd0", unsafe {
        FwpmFilterAdd0(engine.0, &filter, std::ptr::null_mut(), &mut id)
    })?;
    Ok(id)
}

/// Whether `FWPM_CONDITION_IP_PROTOCOL` is available at `layer`. Per the
/// "Filtering conditions available at each filtering layer" table, the
/// ALE_AUTH_LISTEN layers do not carry it (listen() is TCP-only); adding it
/// there makes `FwpmFilterAdd0` fail.
fn layer_has_protocol_condition(layer: WfpLayer) -> bool {
    !matches!(layer, WfpLayer::ListenV4 | WfpLayer::ListenV6)
}

/// Decode one filter returned by BFE back into a [`FilterSpec`]. Any shape
/// this module never installs is an error (so readback cannot silently pass).
///
/// # Safety
/// `f` must point to a valid `FWPM_FILTER0` returned by `FwpmFilterEnum0`.
unsafe fn decode(f: &FWPM_FILTER0, sublayer: &GUID) -> Result<FilterSpec, String> {
    if !guid_eq(&f.subLayerKey, sublayer) {
        return Err("filter in a foreign sublayer".into());
    }
    let layer = layer_from_key(&f.layerKey).ok_or("filter on an unexpected layer")?;
    let action = match f.action.r#type {
        FWP_ACTION_PERMIT => WfpAction::Permit,
        FWP_ACTION_BLOCK => WfpAction::Block,
        other => return Err(format!("unexpected action {other:#x}")),
    };
    if f.weight.r#type != FWP_UINT8 {
        return Err(format!("unexpected weight type {}", f.weight.r#type));
    }
    // SAFETY: type checked above.
    let weight = unsafe { f.weight.Anonymous.uint8 };
    let (mut sid, mut loopback, mut tcp, mut port) = (None, false, false, None);
    for i in 0..f.numFilterConditions as usize {
        // SAFETY: BFE returns `numFilterConditions` valid entries.
        let c = unsafe { &*f.filterCondition.add(i) };
        let v = &c.conditionValue;
        // SAFETY (each arm): the union member read matches the checked `type`.
        if guid_eq(&c.fieldKey, &FWPM_CONDITION_ALE_PACKAGE_ID)
            && v.r#type == FWP_SID
            && c.matchType == FWP_MATCH_EQUAL
            && sid.is_none()
        {
            sid = Some(sid_to_string(unsafe { v.Anonymous.sid } as *mut _)?);
        } else if guid_eq(&c.fieldKey, &FWPM_CONDITION_FLAGS)
            && v.r#type == FWP_UINT32
            && c.matchType == FWP_MATCH_FLAGS_ALL_SET
            && unsafe { v.Anonymous.uint32 } == FWP_CONDITION_FLAG_IS_LOOPBACK
        {
            loopback = true;
        } else if guid_eq(&c.fieldKey, &FWPM_CONDITION_IP_PROTOCOL)
            && v.r#type == FWP_UINT8
            && c.matchType == FWP_MATCH_EQUAL
            && unsafe { v.Anonymous.uint8 } == IPPROTO_TCP
        {
            tcp = true;
        } else if (guid_eq(&c.fieldKey, &FWPM_CONDITION_IP_REMOTE_PORT)
            || guid_eq(&c.fieldKey, &FWPM_CONDITION_IP_LOCAL_PORT))
            && v.r#type == FWP_UINT16
            && c.matchType == FWP_MATCH_EQUAL
            && port.is_none()
        {
            let remote = guid_eq(&c.fieldKey, &FWPM_CONDITION_IP_REMOTE_PORT);
            port = Some((unsafe { v.Anonymous.uint16 }, remote));
        } else {
            return Err("filter carries an unexpected condition".into());
        }
    }
    let run_sid = sid.ok_or("filter is not scoped to a package SID")?;
    // The listen layers do not offer FWPM_CONDITION_IP_PROTOCOL (listen is
    // TCP-only), so the protocol condition is required exactly where it can
    // exist.
    let tcp_ok = tcp == layer_has_protocol_condition(layer);
    let matcher = match (loopback, tcp, port) {
        (true, _, Some((port, remote))) if tcp_ok => WfpMatch::LoopbackTcpPort { port, remote },
        (false, false, None) => WfpMatch::Any,
        _ => return Err("filter has a partial loopback/port match".into()),
    };
    let clear_right = f.flags & FWPM_FILTER_FLAG_CLEAR_ACTION_RIGHT != 0;
    if clear_right != (action == WfpAction::Permit) {
        return Err("filter flags do not match its action".into());
    }
    Ok(FilterSpec {
        layer,
        action,
        weight,
        run_sid,
        matcher,
    })
}

/// Every filter carrying `provider` on every confinement layer, decoded.
fn enumerate(engine: &Engine, provider: &GUID, sublayer: &GUID) -> Result<Vec<FilterSpec>, String> {
    let mut out = Vec::new();
    for layer in LAYERS {
        let mut key = *provider;
        // SAFETY: zeroed POD template, fields set below.
        let mut template: FWPM_FILTER_ENUM_TEMPLATE0 = unsafe { std::mem::zeroed() };
        template.providerKey = &mut key;
        template.layerKey = layer_key(layer);
        template.enumType = FWP_FILTER_ENUM_OVERLAPPING;
        template.actionMask = 0xFFFF_FFFF;
        let mut h: HANDLE = std::ptr::null_mut();
        // SAFETY: valid engine, template and out-pointer.
        check("FwpmFilterCreateEnumHandle0", unsafe {
            FwpmFilterCreateEnumHandle0(engine.0, &template, &mut h)
        })?;
        let result = (|| -> Result<(), String> {
            loop {
                let mut entries: *mut *mut FWPM_FILTER0 = std::ptr::null_mut();
                let mut n: u32 = 0;
                // SAFETY: valid handles and out-pointers.
                check("FwpmFilterEnum0", unsafe {
                    FwpmFilterEnum0(engine.0, h, 64, &mut entries, &mut n)
                })?;
                let mut decoded: Result<(), String> = Ok(());
                for i in 0..n as usize {
                    // SAFETY: BFE returned `n` valid filter pointers.
                    match unsafe { decode(&**entries.add(i), sublayer) } {
                        Ok(spec) => out.push(spec),
                        Err(e) => {
                            decoded = Err(e);
                            break;
                        }
                    }
                }
                if !entries.is_null() {
                    // SAFETY: memory returned by FwpmFilterEnum0.
                    unsafe {
                        FwpmFreeMemory0(&mut entries as *mut _ as *mut *mut core::ffi::c_void)
                    };
                }
                decoded?;
                if n < 64 {
                    return Ok(());
                }
            }
        })();
        // SAFETY: enum handle created above.
        unsafe { FwpmFilterDestroyEnumHandle0(engine.0, h) };
        result?;
    }
    Ok(out)
}

impl ConfinementBackend for WfpBackend {
    type Net = WfpConfinement;

    fn install(&mut self, plan: &ConfinementPlan, sid: &RunSid) -> Result<WfpConfinement, String> {
        let expected = build_filter_specs(plan, sid);
        let binary_sid = BinarySid::parse(sid)?;
        let engine = Engine::open_dynamic()?;
        let mut provider = random_guid();
        let sublayer = random_guid();
        let filter_ids = engine.transaction(|| {
            let mut pname = wide("Aleph managed run provider");
            // SAFETY: zeroed POD, fields set below.
            let mut p: FWPM_PROVIDER0 = unsafe { std::mem::zeroed() };
            p.providerKey = provider;
            p.displayData.name = pname.as_mut_ptr();
            // SAFETY: valid engine and struct.
            check("FwpmProviderAdd0", unsafe {
                FwpmProviderAdd0(engine.0, &p, std::ptr::null_mut())
            })?;
            let mut sname = wide("Aleph managed run sublayer");
            // SAFETY: zeroed POD.
            let mut s: FWPM_SUBLAYER0 = unsafe { std::mem::zeroed() };
            s.subLayerKey = sublayer;
            s.displayData.name = sname.as_mut_ptr();
            s.providerKey = &mut provider;
            s.weight = SUBLAYER_WEIGHT;
            // SAFETY: valid engine and struct.
            check("FwpmSubLayerAdd0", unsafe {
                FwpmSubLayerAdd0(engine.0, &s, std::ptr::null_mut())
            })?;
            let mut ids = Vec::with_capacity(expected.len());
            for spec in &expected {
                ids.push(add_filter(
                    &engine,
                    &mut provider,
                    sublayer,
                    spec,
                    &binary_sid,
                )?);
            }
            Ok(ids)
        })?;
        Ok(WfpConfinement {
            engine,
            provider,
            sublayer,
            filter_ids,
            expected,
        })
    }

    fn verify(
        &mut self,
        net: &WfpConfinement,
        plan: &ConfinementPlan,
        sid: &RunSid,
    ) -> Result<bool, String> {
        let expected = build_filter_specs(plan, sid);
        if expected != net.expected || net.filter_ids.len() != expected.len() {
            return Ok(false);
        }
        let actual = enumerate(&net.engine, &net.provider, &net.sublayer)?;
        verify_filter_readback(&expected, &actual)?;
        Ok(true)
    }

    fn remove(&mut self, net: WfpConfinement) -> Result<(), String> {
        let engine = &net.engine;
        engine.transaction(|| {
            for id in &net.filter_ids {
                // SAFETY: valid engine; id returned by FwpmFilterAdd0.
                check("FwpmFilterDeleteById0", unsafe {
                    FwpmFilterDeleteById0(engine.0, *id)
                })?;
            }
            // SAFETY: valid engine and key.
            check("FwpmSubLayerDeleteByKey0", unsafe {
                FwpmSubLayerDeleteByKey0(engine.0, &net.sublayer)
            })?;
            // SAFETY: valid engine and key.
            check("FwpmProviderDeleteByKey0", unsafe {
                FwpmProviderDeleteByKey0(engine.0, &net.provider)
            })?;
            Ok(())
        })?;
        let left = enumerate(engine, &net.provider, &net.sublayer)?;
        if !left.is_empty() {
            return Err(format!(
                "{} filter(s) still present after removal",
                left.len()
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_confinement_layer_maps_to_a_distinct_ale_key_and_back() {
        for l in LAYERS {
            assert_eq!(layer_from_key(&layer_key(l)), Some(l));
        }
        let keys: Vec<_> = LAYERS.iter().map(|l| layer_key(*l)).collect();
        for (i, a) in keys.iter().enumerate() {
            for b in &keys[i + 1..] {
                assert!(!guid_eq(a, b));
            }
        }
        assert!(layer_from_key(&FWPM_LAYER_ALE_FLOW_ESTABLISHED_V4).is_none());
    }

    #[test]
    fn per_run_guids_are_random() {
        assert!(!guid_eq(&random_guid(), &random_guid()));
    }

    #[test]
    fn run_sid_round_trips_through_the_binary_form() {
        let sid = RunSid::parse("S-1-15-2-1-2-3-4-5-6-7").unwrap();
        let bin = BinarySid::parse(&sid).unwrap();
        assert_eq!(sid_to_string(bin.0).unwrap(), sid.as_str());
    }
}

/// Admin-only end-to-end probe. Never runs in CI or by default.
///
/// From an elevated PowerShell in the worktree:
/// `cargo test -p alephcore --lib --offline sandbox::platforms::windows::wfp::admin_tests -- --ignored --nocapture --test-threads=1`
#[cfg(test)]
mod admin_tests {
    use std::collections::HashMap;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::Duration;

    use super::super::launcher::{launch, ConfinementBackend, ManagedLaunchSpec};
    use super::super::managed_launch::{ConfinementCapabilities, NetworkRequest};
    use super::*;
    use crate::sandbox::managed::ManagedProcess;

    /// Test-only: the experiment needs the ordering to run; production
    /// capabilities stay all-false.
    const PROBE_CAPS: ConfinementCapabilities = ConfinementCapabilities {
        per_run_wfp_confinement: true,
        wfp_arbitration_verified: true,
        elevated: true,
    };

    /// Installs nothing: the baseline (AppContainer alone).
    struct NoConfinement;
    impl ConfinementBackend for NoConfinement {
        type Net = ();
        fn install(&mut self, _: &ConfinementPlan, _: &RunSid) -> Result<(), String> {
            Ok(())
        }
        fn verify(&mut self, _: &(), _: &ConfinementPlan, _: &RunSid) -> Result<bool, String> {
            Ok(true)
        }
        fn remove(&mut self, _: ()) -> Result<(), String> {
            Ok(())
        }
    }

    fn system32(exe: &str) -> String {
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        format!(r"{root}\System32\{exe}")
    }

    fn serve(listener: TcpListener) {
        std::thread::spawn(move || {
            for mut s in listener.incoming().flatten() {
                let mut buf = [0u8; 512];
                let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
                let _ = s.read(&mut buf);
                let _ = s.write_all(b"HTTP/1.0 200 OK\r\nContent-Length: 2\r\n\r\nok");
            }
        });
    }

    /// Run `program args` in a fresh AppContainer through the real launcher
    /// with `backend`; `Ok(exit code)` or a launch/teardown error string.
    fn run<B: ConfinementBackend>(
        backend: B,
        program: String,
        args: &[&str],
        port: u16,
        caps: &[&str],
    ) -> Result<i32, String> {
        let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
        let spec = ManagedLaunchSpec {
            program,
            args: args.iter().map(|s| s.to_string()).collect(),
            // Winsock needs SystemRoot; nothing else from the parent env.
            env: HashMap::from([(
                "SystemRoot".to_string(),
                std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into()),
            )]),
            cwd: dir.path().to_path_buf(),
            app_container_capabilities: caps.iter().map(|c| c.to_string()).collect(),
            workspace: None,
            deny_read_globs: Vec::new(),
            max_memory_mb: None,
        };
        let req = NetworkRequest {
            loopback_ports: vec![port],
            allow_hosts: Vec::new(),
        };
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        rt.block_on(async move {
            let mut p = tokio::task::spawn_blocking(move || {
                launch(spec, backend, &req, &PROBE_CAPS, Duration::from_secs(10))
            })
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
            let _io = p.take_stdio();
            let exit = tokio::time::timeout(Duration::from_secs(60), p.wait()).await;
            match exit {
                Ok(Ok(e)) => Ok(e.code.unwrap_or(-1)),
                Ok(Err(e)) => Err(e.to_string()),
                Err(_) => {
                    let _ = p.start_kill();
                    let _ = p.wait().await;
                    Err("timed out".into())
                }
            }
        })
    }

    fn curl(
        backend: impl ConfinementBackend,
        url: &str,
        permitted_port: u16,
        caps: &[&str],
    ) -> serde_json::Value {
        match run(
            backend,
            system32("curl.exe"),
            &["-s", "-m", "5", "-o", "NUL", url],
            permitted_port,
            caps,
        ) {
            Ok(code) => serde_json::json!({ "curl_exit": code, "connected": code == 0 }),
            Err(e) => serde_json::json!({ "error": sanitize(&e) }),
        }
    }

    /// Strip SIDs and GUID-like tokens from error text.
    fn sanitize(s: &str) -> String {
        s.split_whitespace()
            .map(|w| {
                if w.contains("S-1-") || w.matches('-').count() >= 4 {
                    "<redacted>"
                } else {
                    w
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    #[ignore = "admin-only: installs real per-run WFP filters"]
    fn admin_per_run_wfp_confinement_probe() {
        let elevated = super::super::elevation::process_is_elevated().unwrap_or(false);
        if !elevated {
            println!(
                "{}",
                serde_json::json!({ "elevated": false, "verdict": "not_run_needs_admin" })
            );
            panic!("run from an elevated shell");
        }
        let allowed = TcpListener::bind("127.0.0.1:0").unwrap();
        let other = TcpListener::bind("127.0.0.1:0").unwrap();
        let (a, b) = (
            allowed.local_addr().unwrap().port(),
            other.local_addr().unwrap().port(),
        );
        serve(allowed);
        serve(other);
        let url = |p: u16| format!("http://127.0.0.1:{p}/");

        // 1. Suspended ping.exe admitted with real per-run filters: launch
        //    only succeeds if install + exact readback passed before resume;
        //    wait only succeeds if removal re-enumerated to zero.
        let ping = run(
            WfpBackend,
            system32("PING.EXE"),
            &["-n", "1", "127.0.0.1"],
            a,
            &[],
        );
        let readback_and_removal = ping.is_ok();

        // 2. Baselines without WFP filters. AppContainer loopback isolation
        //    already denies loopback, and a container without internetClient
        //    already denies the internet, so "blocked" under WFP proves
        //    nothing unless the matching baseline was reachable. The
        //    non-loopback pair therefore runs WITH internetClient on both
        //    sides: only then does "blocked under WFP" prove the block filter.
        let baseline = curl(NoConfinement, &url(a), a, &[]);
        let baseline_net = curl(NoConfinement, "http://1.1.1.1/", a, &["internetClient"]);
        // 3. With per-run filters: exact port, another loopback port, non-loopback.
        let permitted = curl(WfpBackend, &url(a), a, &[]);
        let other_port = curl(WfpBackend, &url(b), a, &[]);
        let non_loopback = curl(WfpBackend, "http://1.1.1.1/", a, &["internetClient"]);

        let ok = |v: &serde_json::Value| v.get("connected").and_then(|c| c.as_bool());
        let arbitration = match (ok(&permitted), ok(&other_port), ok(&non_loopback)) {
            (Some(true), Some(false), Some(false)) => "permit_wins_exact_port_only",
            (Some(false), _, _) => "block_wins_permit_ineffective",
            (Some(true), Some(true), _) => "not_exact_other_loopback_port_reachable",
            (Some(true), _, Some(true)) => "not_confined_non_loopback_reachable",
            _ => "inconclusive",
        };
        // The block filter is only demonstrated when the same request was
        // reachable without it. `other_loopback_port` blocked is NOT such
        // evidence (loopback isolation alone denies it).
        let block_filter_demonstrated =
            ok(&baseline_net) == Some(true) && ok(&non_loopback) == Some(false);
        let verdict = serde_json::json!({
            "elevated": true,
            "ping_admission_readback_and_removal_ok": readback_and_removal,
            "ping_error": ping.err().map(|e| sanitize(&e)),
            "baseline_appcontainer_only_loopback": baseline,
            "baseline_internet_client_non_loopback": baseline_net,
            "block_filter_demonstrated": block_filter_demonstrated,
            "permitted_port": permitted,
            "other_loopback_port": other_port,
            "non_loopback_1_1_1_1": non_loopback,
            "arbitration": arbitration,
        });
        println!("WFP_PROBE_VERDICT {verdict}");
        assert!(readback_and_removal, "{verdict}");
    }
}
