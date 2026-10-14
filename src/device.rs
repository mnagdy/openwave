//! Read-only discovery and identity from the published BluOS control interface.
use anyhow::{bail, Context, Result};
use mdns_sd::{ServiceDaemon, ServiceEvent};
use quick_xml::{events::Event, Reader};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::Read,
    net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket},
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiracInterface {
    pub version: u32,
    pub control_port: u16,
    pub rest_port: Option<u16>,
    pub capabilities: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Device {
    pub address: String,
    pub name: String,
    pub model: String,
    pub firmware: String,
    pub dirac_active: Option<bool>,
    pub dirac_interface: Option<DiracInterface>,
}

pub fn address(input: &str) -> Result<SocketAddr> {
    let s = input.trim();
    let addr = if let Ok(a) = s.parse::<SocketAddr>() {
        a
    } else if let Ok(a) = s.parse::<IpAddr>() {
        SocketAddr::new(a, 11000)
    } else {
        bail!("Enter a local player IP address, optionally with its port")
    };
    let local = match addr.ip() {
        IpAddr::V4(a) => a.is_private() || a.is_link_local() || a.is_loopback(),
        IpAddr::V6(a) => a.is_unique_local() || a.is_unicast_link_local() || a.is_loopback(),
    };
    anyhow::ensure!(
        local && addr.port() != 0,
        "A local-network address and nonzero port are required"
    );
    Ok(addr)
}
fn get(addr: SocketAddr, path: &str) -> Result<String> {
    let client = reqwest::blocking::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(4))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let response = client
        .get(format!("http://{addr}{path}"))
        .send()?
        .error_for_status()?;
    let mut buf = Vec::new();
    response.take(65537).read_to_end(&mut buf)?;
    anyhow::ensure!(buf.len() <= 65536, "Player response exceeds 64 KiB");
    Ok(String::from_utf8(buf)?)
}
fn attributes(xml: &str, element: &str) -> Result<BTreeMap<String, String>> {
    let mut reader = Reader::from_str(xml);
    loop {
        match reader.read_event()? {
            Event::Start(e) | Event::Empty(e) if e.name().as_ref() == element.as_bytes() => {
                let mut out = BTreeMap::new();
                for a in e.attributes() {
                    let a = a?;
                    out.insert(
                        String::from_utf8(a.key.as_ref().to_vec())?,
                        a.decode_and_unescape_value(reader.decoder())?.into_owned(),
                    );
                }
                return Ok(out);
            }
            Event::Eof => bail!("Player XML did not contain {element}"),
            _ => {}
        }
    }
}
fn parse_device(addr: SocketAddr, sync: &str, status: Option<&str>) -> Result<Device> {
    let a = attributes(sync, "SyncStatus")?;
    let model = a
        .get("modelName")
        .or_else(|| a.get("model"))
        .context("Player did not report a model")?
        .clone();
    let name = a.get("name").cloned().unwrap_or_else(|| model.clone());
    let firmware = a
        .get("version")
        .cloned()
        .unwrap_or_else(|| "Unknown".into());
    let dirac_active = status.and_then(|s| {
        let mut r = Reader::from_str(s);
        loop {
            match r.read_event().ok()? {
                Event::Start(e) if e.name().as_ref() == b"dirac" => {
                    return match r.read_text(e.name()).ok()?.as_ref() {
                        "1" => Some(true),
                        "0" => Some(false),
                        _ => None,
                    }
                }
                Event::Eof => return None,
                _ => {}
            }
        }
    });
    Ok(Device {
        address: addr.to_string(),
        name,
        model,
        firmware,
        dirac_active,
        dirac_interface: None,
    })
}
pub fn probe(input: &str) -> Result<Device> {
    let addr = address(input)?;
    let sync = get(addr, "/SyncStatus").context("Could not read BluOS player identity")?;
    let status = get(addr, "/Status").ok();
    let mut d = parse_device(addr, &sync, status.as_deref())?;
    d.dirac_interface = inspect_dirac(input).ok();
    Ok(d)
}

pub fn discover() -> Result<Vec<Device>> {
    Ok(discover_report()?.devices)
}

#[derive(Debug)]
pub struct DiscoveryReport {
    pub devices: Vec<Device>,
    pub warnings: Vec<String>,
}

/// Both discovery protocols run independently. A Bonjour failure must not prevent
/// UPnP discovery, and failed identity probes are reported rather than hidden.
pub fn discover_report() -> Result<DiscoveryReport> {
    let (bonjour, upnp) = std::thread::scope(|scope| {
        let bonjour = scope.spawn(discover_bonjour);
        let upnp = scope.spawn(discover_upnp);
        (bonjour.join(), upnp.join())
    });
    let mut addresses = BTreeMap::new();
    let mut warnings = Vec::new();
    let mut successful_searches = 0;
    for (label, result) in [("Bonjour", bonjour), ("UPnP", upnp)] {
        match result {
            Ok(Ok(found)) => {
                successful_searches += 1;
                addresses.extend(found.into_iter().map(|a| (a, ())));
            }
            Ok(Err(e)) => warnings.push(format!("{label} discovery failed: {e:#}")),
            Err(_) => warnings.push(format!("{label} discovery worker stopped unexpectedly")),
        }
    }
    anyhow::ensure!(successful_searches > 0, "{}", warnings.join("\n"));
    // Probe in parallel so an unrelated UPnP device cannot delay every player.
    let results = std::thread::scope(|scope| {
        let jobs: Vec<_> = addresses
            .keys()
            .take(32)
            .map(|addr| {
                let addr = *addr;
                scope.spawn(move || (addr, probe(&addr.to_string())))
            })
            .collect();
        jobs.into_iter().map(|job| job.join()).collect::<Vec<_>>()
    });
    let mut devices = Vec::new();
    for result in results {
        match result {
            Ok((_, Ok(device))) => devices.push(device),
            Ok((addr, Err(e))) => {
                warnings.push(format!("No readable BluOS identity at {addr}: {e:#}"))
            }
            Err(_) => warnings.push("Player identity worker stopped unexpectedly".into()),
        }
    }
    Ok(DiscoveryReport { devices, warnings })
}

fn discover_bonjour() -> Result<Vec<SocketAddr>> {
    let daemon = ServiceDaemon::new()?;
    let mut receivers = Vec::new();
    for ty in ["_musc._tcp.local.", "_musp._tcp.local."] {
        receivers.push(daemon.browse(ty)?);
    }
    let until = Instant::now() + Duration::from_secs(4);
    let mut addresses = BTreeMap::new();
    while Instant::now() < until {
        for rx in &receivers {
            while let Ok(event) = rx.try_recv() {
                if let ServiceEvent::ServiceResolved(info) = event {
                    for ip in info.get_addresses() {
                        let a = SocketAddr::new(*ip, info.get_port());
                        if address(&a.to_string()).is_ok() {
                            addresses.insert(a, ());
                        }
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    for ty in ["_musc._tcp.local.", "_musp._tcp.local."] {
        let _ = daemon.stop_browse(ty);
    }
    let _ = daemon.shutdown();
    Ok(addresses.into_keys().collect())
}

const ROOT_SEARCH: &[u8] = b"M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nMAN: \"ssdp:discover\"\r\nMX: 4\r\nST: upnp:rootdevice\r\n\r\n";

/// Search each local IPv4 interface; responses go to our ephemeral source port.
/// Device.xml's port describes UPnP, not the BluOS identity API on port 11000.
fn discover_upnp() -> Result<Vec<SocketAddr>> {
    let ips: std::collections::BTreeSet<_> = if_addrs::get_if_addrs()?
        .into_iter()
        .filter_map(|interface| match interface.ip() {
            IpAddr::V4(ip) if !ip.is_loopback() && (ip.is_private() || ip.is_link_local()) => {
                Some(ip)
            }
            _ => None,
        })
        .collect();
    let destination = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(239, 255, 255, 250)), 1900);
    let mut sockets = Vec::new();
    let mut failures = Vec::new();
    for ip in ips {
        let result = (|| -> Result<UdpSocket> {
            let socket = UdpSocket::bind((ip, 0))?;
            socket.set_nonblocking(true)?;
            socket.set_multicast_ttl_v4(2)?;
            socket2::SockRef::from(&socket).set_multicast_if_v4(&ip)?;
            socket.send_to(ROOT_SEARCH, destination)?;
            Ok(socket)
        })();
        match result {
            Ok(socket) => sockets.push(socket),
            Err(e) => failures.push(format!("{ip}: {e:#}")),
        }
    }
    anyhow::ensure!(
        !sockets.is_empty(),
        "No usable local IPv4 interface for UPnP. {}",
        failures.join("; ")
    );
    collect_upnp(&sockets, Duration::from_secs(6))
}

fn collect_upnp(sockets: &[UdpSocket], duration: Duration) -> Result<Vec<SocketAddr>> {
    let until = Instant::now() + duration;
    let mut addresses = BTreeMap::new();
    let mut buffer = [0; 8192];
    while Instant::now() < until {
        for socket in sockets {
            // Bound processing as well as elapsed time on noisy networks.
            for _ in 0..64 {
                match socket.recv_from(&mut buffer) {
                    Ok((n, from)) => {
                        if let Ok(candidate) = upnp_candidate(&buffer[..n], from) {
                            if addresses.len() < 32 {
                                addresses.insert(candidate, ());
                            }
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(e) => return Err(e.into()),
                }
            }
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    Ok(addresses.into_keys().collect())
}

fn upnp_candidate(response: &[u8], from: SocketAddr) -> Result<SocketAddr> {
    let response = std::str::from_utf8(response)?;
    let mut lines = response.lines();
    anyhow::ensure!(
        lines.next().is_some_and(|line| {
            let mut fields = line.split_whitespace();
            fields
                .next()
                .is_some_and(|v| v.eq_ignore_ascii_case("HTTP/1.1"))
                && fields.next() == Some("200")
        }),
        "Not an SSDP success response"
    );
    let location = lines
        .find_map(|line| {
            line.split_once(':').and_then(|(key, value)| {
                key.eq_ignore_ascii_case("location").then_some(value.trim())
            })
        })
        .context("SSDP response has no location")?;
    descriptor_location(location, from.ip())?;
    address(&SocketAddr::new(from.ip(), 11000).to_string())
}

fn descriptor_location(location: &str, player: IpAddr) -> Result<reqwest::Url> {
    let url = reqwest::Url::parse(location)?;
    anyhow::ensure!(
        url.scheme() == "http"
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        "Unsupported UPnP location"
    );
    let target: IpAddr = url
        .host_str()
        .context("UPnP location has no host")?
        .parse()?;
    anyhow::ensure!(
        target == player,
        "UPnP descriptor host differs from the selected player"
    );
    address(&SocketAddr::new(target, url.port().unwrap_or(80)).to_string())?;
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn limits_network_addresses() {
        assert_eq!(address("192.168.1.2").unwrap().port(), 11000);
        assert!(address("https://192.168.1.2").is_err());
        assert!(address("8.8.8.8:80").is_err());
        assert!(address("127.0.0.1:0").is_err());
        assert!(address("[fd00::1]:11000").is_ok());
    }
    #[test]
    fn parses_identity_without_serial_identifiers() {
        let d=parse_device(address("127.0.0.1").unwrap(),r#"<?xml version="1.0"?><SyncStatus name="A &amp; B" model="M10" version="4.16.6" mac="ignored"/>"#,Some("<status><dirac>1</dirac></status>")).unwrap();
        assert_eq!(d.name, "A & B");
        assert_eq!(d.dirac_active, Some(true));
        assert!(serde_json::to_string(&d).unwrap().find("ignored").is_none());
    }
    #[test]
    fn invalid_identity_fails() {
        assert!(parse_device(address("127.0.0.1").unwrap(), "<error/>", None).is_err());
    }
    #[test]
    fn ssdp_m10_uses_bluos_port_not_descriptor_port() {
        let packet = b"HTTP/1.1 200 OK\r\nlOcAtIoN: http://192.168.1.20:8080/Device.xml\r\nST: upnp:rootdevice\r\n\r\n";
        assert_eq!(
            upnp_candidate(packet, "192.168.1.20:1900".parse().unwrap()).unwrap(),
            "192.168.1.20:11000".parse().unwrap()
        );
    }
    #[test]
    fn ssdp_rejects_invalid_and_cross_host_locations() {
        let from = "192.168.1.20:1900".parse().unwrap();
        for location in [
            "http://192.168.1.21:8080/Device.xml",
            "http://8.8.8.8/Device.xml",
            "https://192.168.1.20/Device.xml",
            "http://name.local/Device.xml",
            "http://user@192.168.1.20/Device.xml",
            "http://192.168.1.20/Device.xml?q=x",
        ] {
            let packet = format!("HTTP/1.1 200 OK\r\nLOCATION: {location}\r\n\r\n");
            assert!(
                upnp_candidate(packet.as_bytes(), from).is_err(),
                "{location}"
            );
        }
        assert!(upnp_candidate(
            b"HTTP/1.1 500 Error\r\nLOCATION: http://192.168.1.20/Device.xml\r\n",
            from
        )
        .is_err());
        assert!(upnp_candidate(b"HTTP/1.1 200 OK\r\n\r\n", from).is_err());
    }
    #[test]
    fn ssdp_receiver_ignores_malformed_packets_and_deduplicates() {
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        receiver.set_nonblocking(true).unwrap();
        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        let target = receiver.local_addr().unwrap();
        sender.send_to(b"invalid", target).unwrap();
        let packet = b"HTTP/1.1 200 OK\r\nLOCATION: http://127.0.0.1:8080/Device.xml\r\n\r\n";
        sender.send_to(packet, target).unwrap();
        sender.send_to(packet, target).unwrap();
        assert_eq!(
            collect_upnp(&[receiver], Duration::from_millis(100)).unwrap(),
            vec!["127.0.0.1:11000".parse::<SocketAddr>().unwrap()]
        );
    }
}

/// Inspect only the supplied player's UPnP root descriptor. No calibration commands.
pub fn inspect_dirac(input: &str) -> Result<DiracInterface> {
    let player = address(input)?;
    anyhow::ensure!(
        player.is_ipv4(),
        "Direct UPnP discovery currently requires IPv4"
    );
    let route = UdpSocket::bind("0.0.0.0:0")?;
    route.connect(SocketAddr::new(player.ip(), 1900))?;
    let local = route.local_addr()?.ip();
    drop(route);
    let socket = UdpSocket::bind(SocketAddr::new(local, 0))?;
    socket.set_read_timeout(Some(Duration::from_millis(500)))?;
    socket.send_to(ROOT_SEARCH, SocketAddr::new(player.ip(), 1900))?;
    let until = Instant::now() + Duration::from_secs(6);
    let mut buffer = [0; 8192];
    while Instant::now() < until {
        let (n, from) = match socket.recv_from(&mut buffer) {
            Ok(v) => v,
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                continue
            }
            Err(e) => return Err(e.into()),
        };
        if from.ip() != player.ip() {
            continue;
        }
        let response = String::from_utf8_lossy(&buffer[..n]);
        let Some(location) = response.lines().find_map(|l| {
            l.split_once(':').and_then(|(k, v)| {
                if k.eq_ignore_ascii_case("location") {
                    Some(v.trim())
                } else {
                    None
                }
            })
        }) else {
            continue;
        };
        let url = descriptor_location(location, player.ip())?;
        let addr = SocketAddr::new(player.ip(), url.port().unwrap_or(80));
        let xml = get(addr, url.path())?;
        return parse_dirac_descriptor(&xml);
    }
    bail!("Player did not advertise a Dirac descriptor during the bounded search")
}
fn parse_dirac_descriptor(xml: &str) -> Result<DiracInterface> {
    let mut reader = Reader::from_str(xml);
    let mut namespaces = BTreeMap::new();
    let mut fields = BTreeMap::new();
    let mut version = 0;
    loop {
        match reader.read_event()? {
            Event::Start(e) | Event::Empty(e) if e.local_name().as_ref() == b"root" => {
                for a in e.attributes() {
                    let a = a?;
                    if let Some(prefix) = a.key.as_ref().strip_prefix(b"xmlns:") {
                        let ns = a.decode_and_unescape_value(reader.decoder())?.into_owned();
                        if ns == "urn:schemas-dirac-com:diracio:2" {
                            namespaces.insert(prefix.to_vec(), 2u32);
                        } else if ns == "urn:schemas-dirac-se:diracio-1-0" {
                            namespaces.insert(prefix.to_vec(), 1u32);
                        }
                    }
                }
            }
            Event::Start(e) => {
                let full = e.name();
                let key = full.as_ref();
                if let Some(i) = key.iter().position(|b| *b == b':') {
                    if let Some(v) = namespaces.get(&key[..i]) {
                        let field = String::from_utf8(key[i + 1..].to_vec())?.to_ascii_lowercase();
                        if ["x_port", "x_rest_api", "x_type"].contains(&field.as_str()) {
                            version = *v;
                            let text = reader.read_text(e.name())?.into_owned();
                            fields.insert(field, text);
                        }
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    anyhow::ensure!(
        version > 0,
        "Descriptor contains no supported Dirac interface"
    );
    let control_port: u16 = fields
        .get("x_port")
        .context("No Dirac control port")?
        .parse()?;
    let rest_port = fields
        .get("x_rest_api")
        .map(|s| s.parse::<u16>())
        .transpose()?;
    anyhow::ensure!(
        control_port > 0 && rest_port != Some(0),
        "Invalid advertised Dirac port"
    );
    let capabilities = fields
        .get("x_type")
        .map(|s| {
            s.split(';')
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    Ok(DiracInterface {
        version,
        control_port,
        rest_port,
        capabilities,
    })
}
#[cfg(test)]
mod dirac_tests {
    use super::*;
    #[test]
    fn uses_declared_namespace_and_ports() {
        let d=parse_dirac_descriptor(r#"<root xmlns:d="urn:schemas-dirac-com:diracio:2"><d:X_port>5000</d:X_port><d:X_rest_api>8090</d:X_rest_api><d:X_type>Stimuli;Filtering</d:X_type></root>"#).unwrap();
        assert_eq!(d.version, 2);
        assert_eq!(d.control_port, 5000);
        assert_eq!(d.capabilities, ["Stimuli", "Filtering"]);
    }
    #[test]
    fn rejects_unsupported_namespaces_and_zero_ports() {
        assert!(parse_dirac_descriptor(
            r#"<root xmlns:d="unknown"><d:X_port>5000</d:X_port></root>"#
        )
        .is_err());
        assert!(parse_dirac_descriptor(
            r#"<root xmlns:d="urn:schemas-dirac-com:diracio:2"><d:X_port>0</d:X_port></root>"#
        )
        .is_err());
    }
}
