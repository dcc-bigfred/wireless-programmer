//! WiFred Soft-AP HTTP mock.

use wp_drivers::wifred::{DeviceConfig, FunctionEntry, LocoConfig, NetworkConfig};

use crate::device::{not_found, ok_text, ok_xml, FakeDevice, FakeRequest, FakeResponse};

/// Fake WiFred config-mode HTTP device.
pub struct WifredFake {
    /// Current device configuration (GET XML shape).
    pub cfg: DeviceConfig,
    /// Set when `/restart.html` is hit.
    pub restarted: bool,
    /// Active loco slot index (0-based) from `loco=N`.
    pub active_loco: Option<usize>,
}

impl WifredFake {
    /// Default factory state: structure version 1, four empty loco slots.
    #[must_use]
    pub fn new() -> Self {
        Self {
            cfg: DeviceConfig {
                structure_version: Some("1".into()),
                throttle_name: None,
                firmware_revision: None,
                battery_mv: None,
                locos: (0..4)
                    .map(|_| LocoConfig {
                        address: -1,
                        ..Default::default()
                    })
                    .collect(),
                networks: Vec::new(),
                loco_server: None,
            },
            restarted: false,
            active_loco: None,
        }
    }

    /// Serialize `cfg` to WiFred-compatible XML.
    #[must_use]
    pub fn serialize_xml(&self) -> Vec<u8> {
        serialize_xml(&self.cfg)
    }

    fn apply_query(&mut self, query: &str) {
        let mut pending_ssid: Option<String> = None;
        let mut pending_key: Option<String> = None;

        for pair in query.split('&') {
            if pair.is_empty() {
                continue;
            }
            let (raw_key, raw_val) = match pair.split_once('=') {
                Some((k, v)) => (k, v),
                None => (pair, ""),
            };
            let key = percent_decode(raw_key);
            let value = percent_decode(raw_val);

            match key.as_str() {
                "throttleName" => {
                    self.cfg.throttle_name = Some(value);
                }
                "loco" => {
                    if let Ok(n) = value.parse::<usize>() {
                        if n >= 1 {
                            let idx = n - 1;
                            while self.cfg.locos.len() <= idx {
                                self.cfg.locos.push(LocoConfig {
                                    address: -1,
                                    ..Default::default()
                                });
                            }
                            self.active_loco = Some(idx);
                        }
                    }
                }
                "loco.address" => {
                    if let Some(loco) = self.active_loco_mut() {
                        loco.address = value.parse().unwrap_or(-1);
                    }
                }
                "loco.mode" => {
                    if let Some(loco) = self.active_loco_mut() {
                        loco.mode = Some(value);
                    }
                }
                "loco.direction" => {
                    if let Some(loco) = self.active_loco_mut() {
                        loco.direction = value.parse().ok();
                    }
                }
                "loco.longAddress" => {
                    if value == "on" {
                        if let Some(loco) = self.active_loco_mut() {
                            loco.long_address = Some(true);
                        }
                    }
                }
                "loco.serverName" => {
                    self.cfg
                        .loco_server
                        .get_or_insert_with(Default::default)
                        .name = value;
                }
                "loco.serverPort" => {
                    let port = value.parse().unwrap_or(0);
                    self.cfg
                        .loco_server
                        .get_or_insert_with(Default::default)
                        .port = port;
                }
                "loco.automatic" => {
                    if value == "on" {
                        self.cfg
                            .loco_server
                            .get_or_insert_with(Default::default)
                            .automatic = true;
                    }
                }
                "remove" => {
                    self.cfg.networks.retain(|n| n.ssid != value);
                }
                "wifiSSID" => {
                    pending_ssid = Some(value);
                }
                "wifiKEY" => {
                    pending_key = Some(value);
                }
                other if is_function_key(other) => {
                    let index: u8 = other[1..].parse().unwrap_or(0);
                    let fval: u8 = value.parse().unwrap_or(0);
                    if let Some(loco) = self.active_loco_mut() {
                        if let Some(existing) = loco.functions.iter_mut().find(|f| f.index == index)
                        {
                            existing.value = fval;
                        } else {
                            loco.functions.push(FunctionEntry { index, value: fval });
                        }
                    }
                }
                _ => {}
            }
        }

        if let Some(ssid) = pending_ssid {
            upsert_network(&mut self.cfg.networks, ssid, pending_key);
        }
    }

    fn active_loco_mut(&mut self) -> Option<&mut LocoConfig> {
        let idx = self.active_loco?;
        self.cfg.locos.get_mut(idx)
    }
}

impl Default for WifredFake {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeDevice for WifredFake {
    fn driver_id(&self) -> &'static str {
        "wifred"
    }

    fn handle(&mut self, req: FakeRequest<'_>) -> FakeResponse {
        if req.method != "GET" {
            return not_found();
        }
        let path = req.path;
        if path.starts_with("/api/getConfigXML") {
            return ok_xml(self.serialize_xml());
        }
        if path.starts_with("/restart.html") {
            self.restarted = true;
            return ok_text("ok");
        }
        if path.starts_with("/flashred.html") {
            return ok_text("flash");
        }
        if path.starts_with("/index.html") {
            if let Some(q) = path.split_once('?').map(|(_, q)| q) {
                self.apply_query(q);
            }
            return ok_text("ok");
        }
        not_found()
    }
}

fn is_function_key(key: &str) -> bool {
    let mut chars = key.chars();
    if chars.next() != Some('f') {
        return false;
    }
    let rest: String = chars.collect();
    !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit())
}

fn upsert_network(networks: &mut Vec<NetworkConfig>, ssid: String, key: Option<String>) {
    if let Some(existing) = networks.iter_mut().find(|n| n.ssid == ssid) {
        existing.enabled = true;
        if key.is_some() {
            existing.key = key;
        }
    } else {
        networks.push(NetworkConfig {
            ssid,
            key,
            enabled: true,
        });
    }
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let h1 = from_hex(bytes[i + 1]);
                let h2 = from_hex(bytes[i + 2]);
                if let (Some(a), Some(b)) = (h1, h2) {
                    out.push((a << 4) | b);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn from_hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Emit XML tags that [`wp_drivers::wifred::parse`] understands.
pub fn serialize_xml(cfg: &DeviceConfig) -> Vec<u8> {
    let mut s = String::from("<?XML version=\"1.0\" encoding=\"UTF?8\"?>\n<wiFred>\n");
    if let Some(v) = &cfg.structure_version {
        push_empty(&mut s, "structurVersion", v);
    }
    if let Some(v) = &cfg.throttle_name {
        push_empty(&mut s, "throttleName", v);
    }
    if let Some(v) = &cfg.firmware_revision {
        push_empty(&mut s, "firmwareRevision", v);
    }
    if let Some(mv) = cfg.battery_mv {
        push_empty(&mut s, "batteryVoltage", &mv.to_string());
    }

    s.push_str("<LOCOS>\n");
    for (i, loco) in cfg.locos.iter().enumerate() {
        let id = loco.id.unwrap_or((i + 1) as u8);
        s.push_str(&format!(" <LOCO ID=\"{id}\">\n"));
        push_empty(&mut s, "DCCadress", &loco.address.to_string());
        if let Some(mode) = &loco.mode {
            push_empty(&mut s, "Mode", mode);
        } else {
            push_empty(&mut s, "Mode", "");
        }
        if let Some(dir) = loco.direction {
            push_empty(&mut s, "Direction", &dir.to_string());
        }
        if let Some(long) = loco.long_address {
            push_empty(&mut s, "LongAdress", if long { "1" } else { "0" });
        }
        s.push_str("  <FUNCTIONS>\n");
        for f in &loco.functions {
            s.push_str(&format!(
                "     <Function ID=\"{}\" value=\"{}\"/>\n",
                f.index, f.value
            ));
        }
        s.push_str("  </FUNCTIONS>\n");
        s.push_str(" </LOCO>\n");
    }
    s.push_str("</LOCOS>\n");

    s.push_str("<NETWORKS>\n");
    for net in &cfg.networks {
        s.push_str(" <NETWORK>\n");
        push_empty(&mut s, "SSID", &net.ssid);
        if let Some(key) = &net.key {
            push_empty(&mut s, "Key", key);
        }
        push_empty(&mut s, "Enabled", if net.enabled { "1" } else { "0" });
        s.push_str(" </NETWORK>\n");
    }
    s.push_str("</NETWORKS>\n");

    if let Some(srv) = &cfg.loco_server {
        s.push_str("<LOCOSERVER>\n");
        push_empty(&mut s, "ServerName", &srv.name);
        push_empty(&mut s, "Port", &srv.port.to_string());
        push_empty(&mut s, "Automatic", if srv.automatic { "1" } else { "0" });
        s.push_str("</LOCOSERVER>\n");
    }

    s.push_str("</wiFred>\n");
    s.into_bytes()
}

fn push_empty(out: &mut String, tag: &str, value: &str) {
    out.push_str(&format!("<{tag} value=\"{}\"/>\n", xml_escape(value)));
}

fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use wp_drivers::wifred::{parse, LocoServerConfig};

    #[test]
    fn serialize_round_trip() {
        let mut fake = WifredFake::new();
        fake.cfg.throttle_name = Some("122145".into());
        fake.cfg.firmware_revision = Some("2022-10-16".into());
        fake.cfg.battery_mv = Some(3850);
        fake.cfg.locos[0] = LocoConfig {
            id: Some(1),
            address: 3,
            mode: Some("128".into()),
            direction: Some(0),
            long_address: Some(false),
            functions: vec![
                FunctionEntry { index: 0, value: 0 },
                FunctionEntry { index: 1, value: 4 },
            ],
        };
        fake.cfg.networks.push(NetworkConfig {
            ssid: "bigfred2".into(),
            key: Some("secret-pass".into()),
            enabled: true,
        });
        fake.cfg.loco_server = Some(LocoServerConfig {
            name: "bigfred.local".into(),
            port: 12090,
            automatic: false,
        });

        let xml = fake.serialize_xml();
        let parsed = parse(&xml).expect("parse");
        assert_eq!(parsed.structure_version.as_deref(), Some("1"));
        assert_eq!(parsed.throttle_name.as_deref(), Some("122145"));
        assert_eq!(parsed.battery_mv, Some(3850));
        assert_eq!(parsed.locos[0].address, 3);
        assert_eq!(parsed.locos[0].mode.as_deref(), Some("128"));
        assert_eq!(parsed.locos[0].functions.len(), 2);
        assert_eq!(parsed.networks[0].ssid, "bigfred2");
        assert_eq!(parsed.networks[0].key.as_deref(), Some("secret-pass"));
        let srv = parsed.loco_server.expect("server");
        assert_eq!(srv.name, "bigfred.local");
        assert_eq!(srv.port, 12090);
    }

    #[test]
    fn program_sequence_mutations() {
        let mut fake = WifredFake::new();
        let _ = fake.handle(FakeRequest {
            method: "GET",
            path: "/index.html?throttleName=pilot1",
            body: None,
        });
        let _ = fake.handle(FakeRequest {
            method: "GET",
            path: "/index.html?loco=1&loco.address=3&loco.mode=128&loco.direction=0&loco.longAddress=on",
            body: None,
        });
        let _ = fake.handle(FakeRequest {
            method: "GET",
            path: "/index.html?loco=1&f0=0&f1=4",
            body: None,
        });
        let _ = fake.handle(FakeRequest {
            method: "GET",
            path: "/index.html?loco.serverName=bigfred.local&loco.serverPort=12090",
            body: None,
        });
        let _ = fake.handle(FakeRequest {
            method: "GET",
            path: "/index.html?wifiSSID=club-wifi&wifiKEY=secret",
            body: None,
        });
        let _ = fake.handle(FakeRequest {
            method: "GET",
            path: "/restart.html",
            body: None,
        });

        assert_eq!(fake.cfg.throttle_name.as_deref(), Some("pilot1"));
        assert_eq!(fake.cfg.locos[0].address, 3);
        assert_eq!(fake.cfg.locos[0].mode.as_deref(), Some("128"));
        assert_eq!(fake.cfg.locos[0].long_address, Some(true));
        assert_eq!(fake.cfg.locos[0].functions.len(), 2);
        assert_eq!(
            fake.cfg.loco_server.as_ref().map(|s| s.name.as_str()),
            Some("bigfred.local")
        );
        assert_eq!(fake.cfg.networks.len(), 1);
        assert_eq!(fake.cfg.networks[0].ssid, "club-wifi");
        assert!(fake.restarted);

        let xml = fake.serialize_xml();
        let parsed = parse(&xml).expect("parse");
        assert_eq!(parsed.throttle_name.as_deref(), Some("pilot1"));
        assert_eq!(parsed.locos[0].address, 3);
        assert!(parsed.networks.iter().any(|n| n.ssid == "club-wifi"));
    }

    #[test]
    fn percent_decode_plus_and_hex() {
        assert_eq!(percent_decode("a+b%20c"), "a b c");
        assert_eq!(percent_decode("bigfred%2Elocal"), "bigfred.local");
    }
}
