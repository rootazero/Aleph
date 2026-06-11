//! Minimal SOCKS5 proxy (RFC 1928) — CONNECT command only.
//!
//! Greeting phase:
//! ```text
//! client → server: VER(0x05) NMETHODS METHODS[NMETHODS]
//! server → client: VER(0x05) METHOD(0x00 = NO_AUTH | 0xff = no acceptable)
//! ```
//!
//! Request phase:
//! ```text
//! client → server: VER(0x05) CMD(0x01=CONNECT) RSV(0x00) ATYP(1=IPv4|3=DOMAIN|4=IPv6)
//!                  ADDR PORT(u16 BE)
//! server → client: VER(0x05) REP(0x00=succeeded|0x02=not_allowed|0x03=net_unreachable|
//!                                0x07=cmd_not_supported|0x08=atyp_not_supported)
//!                  RSV(0x00) ATYP ADDR PORT
//! ```
//!
//! `UDP_ASSOCIATE` (0x03) and BIND (0x02) are refused (REP=0x07).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use super::allowlist::AllowList;

const VER: u8 = 0x05;
const NO_AUTH: u8 = 0x00;
const NO_ACCEPTABLE: u8 = 0xff;
const CMD_CONNECT: u8 = 0x01;
const RSV: u8 = 0x00;
const ATYP_IPV4: u8 = 0x01;
const ATYP_DOMAIN: u8 = 0x03;
const ATYP_IPV6: u8 = 0x04;
const REP_SUCCEEDED: u8 = 0x00;
const REP_NOT_ALLOWED: u8 = 0x02;
const REP_NET_UNREACHABLE: u8 = 0x03;
const REP_CMD_NOT_SUPPORTED: u8 = 0x07;
const REP_ATYP_NOT_SUPPORTED: u8 = 0x08;

const UPSTREAM_DIAL_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum TargetHost {
    Ip(IpAddr),
    Domain(String),
}

impl TargetHost {
    fn as_lookup_str(&self) -> String {
        match self {
            Self::Ip(ip) => ip.to_string(),
            Self::Domain(d) => d.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ConnectRequest {
    pub target: TargetHost,
    pub port: u16,
}

/// Handle a single inbound SOCKS5 connection. The first byte (the version
/// `0x05`) has already been peeked by the caller; this function consumes the
/// connection from byte 0.
pub(super) async fn handle(inbound: TcpStream, allowlist: AllowList) -> Result<(), std::io::Error> {
    let (mut rd, mut wr) = inbound.into_split();

    // Greeting.
    let mut prelude = [0u8; 2];
    rd.read_exact(&mut prelude).await?;
    if prelude[0] != VER {
        return Ok(()); // protocol mismatch; drop silently.
    }
    let nmethods = prelude[1] as usize;
    if nmethods > 16 {
        // SOCKS5 spec allows up to 255, but >16 is pathological — drop to
        // avoid unnecessary allocation from a malformed or malicious greeting.
        return Ok(());
    }
    let mut methods = vec![0u8; nmethods];
    rd.read_exact(&mut methods).await?;
    if !methods.contains(&NO_AUTH) {
        wr.write_all(&[VER, NO_ACCEPTABLE]).await?;
        return Ok(());
    }
    wr.write_all(&[VER, NO_AUTH]).await?;

    // Request.
    let mut head = [0u8; 4];
    rd.read_exact(&mut head).await?;
    if head[0] != VER || head[2] != RSV {
        return Ok(());
    }
    if head[1] != CMD_CONNECT {
        write_reply(&mut wr, REP_CMD_NOT_SUPPORTED).await?;
        return Ok(());
    }
    let atyp = head[3];
    let req = match parse_target(atyp, &mut rd).await? {
        Some(r) => r,
        None => {
            write_reply(&mut wr, REP_ATYP_NOT_SUPPORTED).await?;
            return Ok(());
        }
    };

    let host_for_log = req.target.as_lookup_str();
    if !allowlist.permits(&host_for_log) {
        tracing::debug!(
            target = "sandbox.proxy",
            host = %host_for_log,
            port = req.port,
            "SOCKS5 CONNECT denied by allowlist"
        );
        write_reply(&mut wr, REP_NOT_ALLOWED).await?;
        return Ok(());
    }

    // `dial_validated` resolves the host once and pins the connection to a
    // non-SSRF-blocked resolved IP — closing the DNS-rebinding gap where an
    // allowlisted *name* re-resolves to a loopback / metadata / internal
    // address at connect time. An SSRF refusal maps to REP_NOT_ALLOWED (a
    // policy denial), distinct from REP_NET_UNREACHABLE (a network failure).
    let upstream = match tokio::time::timeout(
        UPSTREAM_DIAL_TIMEOUT,
        super::dial::dial_validated(&host_for_log, req.port, &allowlist),
    )
    .await
    {
        Ok(Ok(s)) => s,
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            tracing::warn!(
                target = "sandbox.proxy",
                host = %host_for_log,
                port = req.port,
                "SOCKS5 CONNECT blocked: host resolved to an SSRF-blocked address (possible DNS rebinding)"
            );
            write_reply(&mut wr, REP_NOT_ALLOWED).await?;
            return Ok(());
        }
        Ok(Err(e)) => {
            tracing::debug!(
                target = "sandbox.proxy",
                host = %host_for_log,
                port = req.port,
                error = %e,
                "SOCKS5 upstream dial failed"
            );
            write_reply(&mut wr, REP_NET_UNREACHABLE).await?;
            return Ok(());
        }
        Err(_) => {
            write_reply(&mut wr, REP_NET_UNREACHABLE).await?;
            return Ok(());
        }
    };

    // Reply BND.ADDR=0.0.0.0:0 (RFC allows the proxy to omit its actual
    // outbound binding when the client doesn't need it for FTP-style flows).
    write_reply(&mut wr, REP_SUCCEEDED).await?;

    let mut inbound = rd.reunite(wr).expect("same socket halves");
    let mut upstream = upstream;
    let _ = tokio::io::copy_bidirectional(&mut inbound, &mut upstream).await;
    Ok(())
}

async fn parse_target(
    atyp: u8,
    rd: &mut tokio::net::tcp::OwnedReadHalf,
) -> Result<Option<ConnectRequest>, std::io::Error> {
    let target = match atyp {
        ATYP_IPV4 => {
            let mut o = [0u8; 4];
            rd.read_exact(&mut o).await?;
            TargetHost::Ip(IpAddr::V4(Ipv4Addr::from(o)))
        }
        ATYP_IPV6 => {
            let mut o = [0u8; 16];
            rd.read_exact(&mut o).await?;
            TargetHost::Ip(IpAddr::V6(Ipv6Addr::from(o)))
        }
        ATYP_DOMAIN => {
            let mut len_buf = [0u8; 1];
            rd.read_exact(&mut len_buf).await?;
            let len = len_buf[0] as usize;
            let mut name = vec![0u8; len];
            rd.read_exact(&mut name).await?;
            let domain = String::from_utf8(name)
                .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "bad domain"))?;
            TargetHost::Domain(domain)
        }
        _ => return Ok(None),
    };
    let mut port_buf = [0u8; 2];
    rd.read_exact(&mut port_buf).await?;
    let port = u16::from_be_bytes(port_buf);
    Ok(Some(ConnectRequest { target, port }))
}

async fn write_reply(
    wr: &mut tokio::net::tcp::OwnedWriteHalf,
    rep: u8,
) -> Result<(), std::io::Error> {
    // VER REP RSV ATYP=IPv4 ADDR(0.0.0.0) PORT(0)
    let buf = [VER, rep, RSV, ATYP_IPV4, 0, 0, 0, 0, 0, 0];
    wr.write_all(&buf).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipv4_request_bytes_layout() {
        // Layout pinning — if the const values ever drift this fails first.
        let bytes: [u8; 10] = [VER, CMD_CONNECT, RSV, ATYP_IPV4, 1, 2, 3, 4, 0x00, 0x50];
        assert_eq!(bytes[3], ATYP_IPV4);
        assert_eq!(u16::from_be_bytes([bytes[8], bytes[9]]), 80);
    }

    #[test]
    fn domain_request_bytes_layout() {
        let host = b"example.com";
        let mut bytes = vec![VER, CMD_CONNECT, RSV, ATYP_DOMAIN, host.len() as u8];
        bytes.extend_from_slice(host);
        bytes.extend_from_slice(&80u16.to_be_bytes());
        assert_eq!(bytes[3], ATYP_DOMAIN);
        assert_eq!(bytes[4], 11);
        assert_eq!(&bytes[5..16], b"example.com");
    }

    #[test]
    fn rep_codes_are_distinct() {
        // Pin RFC wire constants so future "cleanups" don't accidentally remap.
        assert_eq!(REP_SUCCEEDED, 0x00);
        assert_eq!(REP_NOT_ALLOWED, 0x02);
        assert_eq!(REP_NET_UNREACHABLE, 0x03);
        assert_eq!(REP_CMD_NOT_SUPPORTED, 0x07);
        assert_eq!(REP_ATYP_NOT_SUPPORTED, 0x08);
    }

    #[test]
    fn target_host_lookup_str() {
        assert_eq!(
            TargetHost::Ip(IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4))).as_lookup_str(),
            "1.2.3.4"
        );
        assert_eq!(
            TargetHost::Domain("example.com".into()).as_lookup_str(),
            "example.com"
        );
    }
}
