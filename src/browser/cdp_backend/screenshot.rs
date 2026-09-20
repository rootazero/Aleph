//! Pixels and page-level overrides: screenshot, pdf, resize, emulate.

use std::path::Path;

use aleph_cdp::methods::{browser as cdp_browser, emulation, network, page};

use crate::browser::engine::EngineCapabilities;
use crate::browser::error::BrowserError;
use crate::browser::types::{
    CdpNetworkConditions, ColorScheme, EmulateOptions, ScreenshotOpts, ScreenshotOutput,
};

use super::{map_cdp_err, CdpBackend};

/// JPEG quality, when the caller asks for JPEG. One number, named, because a
/// literal at the call site reads as arbitrary and this one is a trade the
/// screenshot budget depends on.
const JPEG_QUALITY: u8 = 80;

pub(super) async fn screenshot(
    be: &CdpBackend,
    tab_id: &str,
    opts: ScreenshotOpts,
) -> Result<ScreenshotOutput, BrowserError> {
    let handle = be.handle().await?;
    let session = handle.ensure_tab(tab_id).await?;
    let fmt = match opts.format.to_ascii_lowercase().as_str() {
        "jpeg" | "jpg" => page::ScreenshotFormat::Jpeg {
            quality: JPEG_QUALITY,
        },
        _ => page::ScreenshotFormat::Png,
    };
    // `captureBeyondViewport` is what makes "full page" mean the whole document
    // rather than the visible box — without it the flag would return the same
    // image and report success.
    let bytes = page::capture_screenshot(&handle.conn, Some(&session), fmt, opts.full_page)
        .await
        .map_err(|e| map_cdp_err(be.engine(), "Page.captureScreenshot", e))?;
    Ok(ScreenshotOutput { png_bytes: bytes })
}

pub(super) async fn pdf(
    be: &CdpBackend,
    caps: &EngineCapabilities,
    tab_id: &str,
    output_path: &Path,
) -> Result<(), BrowserError> {
    // Refusal before the wire, and before the handle is even resolved: a verb
    // this engine cannot do must not open a browser to say so.
    super::require(caps, be.engine(), |c| c.pdf, "pdf")?;
    let handle = be.handle().await?;
    let session = handle.ensure_tab(tab_id).await?;
    let bytes = page::print_to_pdf(&handle.conn, Some(&session))
        .await
        .map_err(|e| map_cdp_err(be.engine(), "Page.printToPDF", e))?;
    tokio::fs::write(output_path, bytes)
        .await
        .map_err(BrowserError::Io)
}

pub(super) async fn resize(
    be: &CdpBackend,
    tab_id: &str,
    width: u32,
    height: u32,
) -> Result<(), BrowserError> {
    let handle = be.handle().await?;
    let session = handle.ensure_tab(tab_id).await?;
    emulation::set_device_metrics_override(&handle.conn, Some(&session), width, height, 1.0, false)
        .await
        .map_err(|e| map_cdp_err(be.engine(), "Emulation.setDeviceMetricsOverride", e))
}

pub(super) async fn emulate(
    be: &CdpBackend,
    tab_id: &str,
    opts: &EmulateOptions,
) -> Result<(), BrowserError> {
    // Validated at the boundary, before any wire traffic — the same ordering the
    // managed backend uses.
    opts.validate().map_err(BrowserError::ActionFailed)?;
    let handle = be.handle().await?;
    let session = handle.ensure_tab(tab_id).await?;
    let s = Some(&session);
    let engine = be.engine();

    if let Some(scheme) = opts.color_scheme {
        // Owned pairs, because Task 4's wrapper takes `&[(String, String)]`.
        let features: Vec<(String, String)> = match scheme {
            ColorScheme::Dark => vec![("prefers-color-scheme".into(), "dark".into())],
            ColorScheme::Light => vec![("prefers-color-scheme".into(), "light".into())],
            // "Auto" is the absence of an override, not a third value — and an
            // empty `features` slice is how CDP spells "clear the ones I set".
            ColorScheme::Auto => Vec::new(),
        };
        // `media: None` — this backend overrides the colour-scheme feature and
        // never the media type, and passing `Some("screen")` would pin a value
        // nobody asked for.
        emulation::set_emulated_media(&handle.conn, s, None, &features)
            .await
            .map_err(|e| map_cdp_err(engine, "Emulation.setEmulatedMedia", e))?;
    }
    if let Some(geo) = opts.geolocation {
        emulation::set_geolocation_override(&handle.conn, s, geo.latitude, geo.longitude, 1.0)
            .await
            .map_err(|e| map_cdp_err(engine, "Emulation.setGeolocationOverride", e))?;
    }
    if let Some(cond) = opts.network_condition {
        // `None` means Online, which CDP expresses by clearing the throttle
        // rather than by setting one.
        let c = cond.as_cdp().unwrap_or(CdpNetworkConditions {
            offline: false,
            latency_ms: 0.0,
            download_bps: -1.0,
            upload_bps: -1.0,
        });
        network::emulate_network_conditions(
            &handle.conn,
            s,
            c.offline,
            c.latency_ms,
            c.download_bps,
            c.upload_bps,
        )
        .await
        .map_err(|e| map_cdp_err(engine, "Network.emulateNetworkConditions", e))?;
    }
    if let Some(rate) = opts.cpu_throttle {
        emulation::set_cpu_throttling_rate(&handle.conn, s, rate)
            .await
            .map_err(|e| map_cdp_err(engine, "Emulation.setCPUThrottlingRate", e))?;
    }
    if let Some(headers) = &opts.extra_http_headers {
        // `BTreeMap` -> the `&[(String, String)]` Task 4's wrapper takes. The
        // map's ordering carries through, which keeps the wire deterministic.
        let pairs: Vec<(String, String)> = headers
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        network::set_extra_http_headers(&handle.conn, s, &pairs)
            .await
            .map_err(|e| map_cdp_err(engine, "Network.setExtraHTTPHeaders", e))?;
    }
    if let Some(ua) = &opts.user_agent {
        // An empty string means "clear the override". CDP has no clear verb, so
        // the real UA is read back from the engine and set — the only honest way
        // to undo an override without restarting the browser.
        let value = if ua.is_empty() {
            cdp_browser::get_version(&handle.conn)
                .await
                .map_err(|e| map_cdp_err(engine, "Browser.getVersion", e))?
                .user_agent
        } else {
            ua.clone()
        };
        emulation::set_user_agent_override(&handle.conn, s, &value)
            .await
            .map_err(|e| map_cdp_err(engine, "Emulation.setUserAgentOverride", e))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use aleph_cdp::testkit::{FakeCdpServer, Responder};
    use serde_json::json;

    use crate::browser::backend::BrowserBackend;
    use crate::browser::cdp_backend::test_support::*;
    use crate::browser::engine::{Cap, Engine};
    use crate::browser::error::BrowserError;
    use crate::browser::types::{ColorScheme, EmulateOptions};

    /// The managed driver refuses colour-scheme emulation and says so; this one
    /// applies it. A single test asserting "emulate returned ok" would pass on a
    /// backend that dropped the override silently, so the assertion is on the
    /// wire parameters.
    #[tokio::test]
    async fn a_colour_scheme_override_reaches_the_engine_as_an_emulated_media_feature() {
        let server = FakeCdpServer::start(FakeCdpServer::scripted(vec![])).await;
        wire_session(&server, "S1");
        server.on("Emulation.setEmulatedMedia", Responder::Reply(json!({})));
        let (_reg, backend) = backend_with(&server, Engine::Chromium, open_guard()).await;
        let handle = backend.handle().await.expect("handle");
        handle
            .attach_tab(&aleph_cdp::TargetId("T1".into()))
            .await
            .expect("attach");

        backend
            .emulate(
                "T1",
                &EmulateOptions {
                    color_scheme: Some(ColorScheme::Dark),
                    ..Default::default()
                },
            )
            .await
            .expect("the CDP backend can apply a colour scheme");

        let sent = server.received();
        let call = sent
            .iter()
            .find(|m| m["method"].as_str() == Some("Emulation.setEmulatedMedia"))
            .expect("the override reached the wire");
        assert_eq!(
            call["params"]["features"][0]["name"],
            "prefers-color-scheme"
        );
        assert_eq!(call["params"]["features"][0]["value"], "dark");
        assert!(
            call["params"].get("media").is_none(),
            "this backend never pins a media type: {}",
            call["params"]
        );
    }

    /// A verb the engine cannot do must name the engine that can. `None` here
    /// would be fail-dead: a closed gate with no door (判据 §14).
    ///
    /// The capability row is INJECTED (R42), so the branch is exercised
    /// whatever the production table says today. `supported_by` still reads the
    /// real table, which is why the hint is `Chromium` rather than something
    /// derived from this fixture — the door it names has to be a real one.
    #[tokio::test]
    async fn pdf_refuses_when_the_table_says_the_engine_cannot() {
        let server = FakeCdpServer::start(FakeCdpServer::scripted(vec![])).await;
        wire_session(&server, "S1");
        let (_reg, backend) = backend_with(&server, Engine::Obscura, open_guard()).await;

        let mut caps = all_supported();
        caps.pdf = Cap::Unsupported;
        let err = super::pdf(
            &backend,
            &caps,
            "T1",
            std::path::Path::new("/nonexistent/never-written.pdf"),
        )
        .await
        .expect_err("a table saying the engine cannot must refuse");
        match err {
            BrowserError::UnsupportedByEngine {
                engine,
                verb,
                supported_by,
            } => {
                assert_eq!(engine, Engine::Obscura);
                assert_eq!(verb, "pdf");
                assert_eq!(supported_by, Some(Engine::Chromium));
            }
            other => panic!("expected UnsupportedByEngine, got {other:?}"),
        }
        assert!(
            methods(&server).is_empty(),
            "a capability refusal must not touch the wire: {:?}",
            methods(&server)
        );
    }

    /// The positive half of the same branch: a table that says the engine CAN
    /// must not refuse. Without it, `require` returning `Err` unconditionally
    /// would leave the test above green (判据 §2 — a guard with only one
    /// outcome exercised is one outcome tested).
    #[tokio::test]
    async fn pdf_reaches_the_wire_when_the_table_says_the_engine_can() {
        let server = FakeCdpServer::start(FakeCdpServer::scripted(vec![])).await;
        wire_session(&server, "S1");
        server.on(
            "Page.printToPDF",
            // base64 of "%PDF-1.4" — enough to prove the bytes travelled.
            Responder::Reply(json!({ "data": "JVBERi0xLjQ=" })),
        );
        let (_reg, backend) = backend_with(&server, Engine::Obscura, open_guard()).await;
        let handle = backend.handle().await.expect("handle");
        handle
            .attach_tab(&aleph_cdp::TargetId("T1".into()))
            .await
            .expect("attach");

        let dir = tempfile::tempdir().expect("tempdir");
        let out = dir.path().join("page.pdf");
        super::pdf(&backend, &all_supported(), "T1", &out)
            .await
            .expect("a supported table must not refuse");
        assert_eq!(
            std::fs::read(&out).expect("the pdf was written"),
            b"%PDF-1.4",
            "the engine's bytes, not a placeholder"
        );
    }

    /// Every field of `EmulateOptions` is in the axis map, derived from the
    /// type rather than from the map.
    ///
    /// The literal below is **exhaustive on purpose**: a seventh axis is a
    /// compile error right here, which is the only thing that makes
    /// `EMULATE_AXIS_METHODS` — and therefore both guards that read it —
    /// unable to go stale in silence.
    #[test]
    fn the_axis_map_covers_every_field_of_emulate_options() {
        use std::collections::{BTreeMap, BTreeSet};

        use crate::browser::types::{Geolocation, NetworkCondition};

        let all = EmulateOptions {
            color_scheme: Some(ColorScheme::Dark),
            geolocation: Some(Geolocation {
                latitude: 1.0,
                longitude: 2.0,
            }),
            network_condition: Some(NetworkCondition::Offline),
            cpu_throttle: Some(2.0),
            extra_http_headers: Some(BTreeMap::from([("X-Aleph".to_string(), "1".to_string())])),
            user_agent: Some("aleph-unit-agent".to_string()),
        };
        let keys: BTreeSet<String> = match serde_json::to_value(&all).expect("serialise") {
            serde_json::Value::Object(map) => map.keys().cloned().collect(),
            other => panic!("EmulateOptions must serialise to an object, got {other}"),
        };
        let mapped: BTreeSet<String> = crate::browser::cdp_backend::EMULATE_AXIS_METHODS
            .iter()
            .map(|(axis, _)| (*axis).to_string())
            .collect();
        assert_eq!(
            keys, mapped,
            "the axis map and EmulateOptions disagree about which axes exist; \
             an axis missing from the map is one no guard checks"
        );
    }

    /// Each axis must reach its OWN CDP method.
    ///
    /// A single "emulate returned ok" assertion would pass on a backend that
    /// dropped five of the six on the floor (判据 §11), and the DESCRIPTION
    /// guard in `browser_tools::emulate` rests on exactly this: it tells the
    /// model that everything but the engine-refused axis reaches the engine,
    /// and that sentence is only true while this test is green.
    ///
    /// Run against [`Engine::default`] — the engine of a default install — so
    /// the test is about the profile the sentence is about. The fake answers
    /// every method, which is the point: this half measures **Aleph's** send,
    /// and what a real engine does with each method is the other half, read
    /// from Task 0's matrix in the DESCRIPTION guard.
    #[tokio::test]
    async fn emulate_sends_every_axis_to_its_own_cdp_method() {
        use std::collections::BTreeMap;

        use crate::browser::types::{Geolocation, NetworkCondition};

        let server = FakeCdpServer::start(FakeCdpServer::scripted(vec![])).await;
        wire_session(&server, "S1");
        for (_, method) in crate::browser::cdp_backend::EMULATE_AXIS_METHODS {
            server.on(method, Responder::Reply(json!({})));
        }
        let (_reg, backend) = backend_with(&server, Engine::default(), open_guard()).await;
        let handle = backend.handle().await.expect("handle");
        handle
            .attach_tab(&aleph_cdp::TargetId("T1".into()))
            .await
            .expect("attach");

        backend
            .emulate(
                "T1",
                // A NON-empty user agent, deliberately: the empty string means
                // "clear the override" and takes a `Browser.getVersion`
                // read-back first, which would make this test about that path
                // instead of about the six sends.
                &EmulateOptions {
                    color_scheme: Some(ColorScheme::Dark),
                    geolocation: Some(Geolocation {
                        latitude: 1.0,
                        longitude: 2.0,
                    }),
                    network_condition: Some(NetworkCondition::Offline),
                    cpu_throttle: Some(2.0),
                    extra_http_headers: Some(BTreeMap::from([(
                        "X-Aleph".to_string(),
                        "1".to_string(),
                    )])),
                    user_agent: Some("aleph-unit-agent".to_string()),
                },
            )
            .await
            .expect("every axis is routed; none is gated at this layer");

        let sent = methods(&server);
        for (axis, method) in crate::browser::cdp_backend::EMULATE_AXIS_METHODS {
            assert!(
                sent.iter().any(|m| m.as_str() == method),
                "axis `{axis}` never reached `{method}` — sent: {sent:?}"
            );
        }
    }

    /// `resize` has to reach `Emulation.setDeviceMetricsOverride` with the
    /// numbers it was given. A verb that returned `Ok` without sending them is
    /// the report-success no-op (判据 §11), and the viewport would simply never
    /// change.
    #[tokio::test]
    async fn a_resize_sends_the_requested_metrics() {
        let server = FakeCdpServer::start(FakeCdpServer::scripted(vec![])).await;
        wire_session(&server, "S1");
        server.on(
            "Emulation.setDeviceMetricsOverride",
            Responder::Reply(json!({})),
        );
        let (_reg, backend) = backend_with(&server, Engine::Chromium, open_guard()).await;
        let handle = backend.handle().await.expect("handle");
        handle
            .attach_tab(&aleph_cdp::TargetId("T1".into()))
            .await
            .expect("attach");

        backend.resize("T1", 390, 844).await.expect("resize ok");
        let params = server
            .last_params("Emulation.setDeviceMetricsOverride")
            .expect("the override reached the wire");
        assert_eq!(params["width"], 390);
        assert_eq!(params["height"], 844);
    }
}
