//! Lenient parser for the WiFred `/api/getConfigXML` response.
//!
//! The firmware emits a malformed XML prolog (`<?XML version="1.0"
//! encoding="UTF?8"?>` — uppercase `XML`, `UTF?8`) and serves it as
//! `text/html`. We parse with `quick-xml`'s reader, skipping the declaration
//! and any stray tokens, and only collect the attributes we care about.

use std::str;

use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;

/// Parsed device config.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceConfig {
    /// `<structurVersion value="..."/>`.
    pub structure_version: Option<String>,
    /// `<throttleName value="..."/>`.
    pub throttle_name: Option<String>,
    /// `<firmwareRevision value="..."/>`.
    pub firmware_revision: Option<String>,
    /// `<batteryVoltage value="..."/>` in millivolts.
    pub battery_mv: Option<u32>,
    /// `<LOCOS>/<LOCO>` entries (in order).
    pub locos: Vec<LocoConfig>,
    /// `<NETWORKS>/<NETWORK>` entries.
    pub networks: Vec<NetworkConfig>,
    /// `<LOCOSERVER>` block.
    pub loco_server: Option<LocoServerConfig>,
}

/// One loco slot.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocoConfig {
    /// `<LOCO ID="n">` (1-based).
    pub id: Option<u8>,
    /// `<DCCadress value="n"/>` (-1 disables the slot).
    pub address: i64,
    /// `<Mode value="..."/>`.
    pub mode: Option<String>,
    /// `<Direction value="n"/>`.
    pub direction: Option<u8>,
    /// `<LongAdress value="n"/>`.
    pub long_address: Option<bool>,
    /// `<Function ID="n" value="v"/>` entries.
    pub functions: Vec<FunctionEntry>,
}

/// One function mapping entry.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FunctionEntry {
    /// `ID` attribute.
    pub index: u8,
    /// `value` attribute.
    pub value: u8,
}

/// One known WiFi network.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NetworkConfig {
    /// `<SSID value="..."/>`.
    pub ssid: String,
    /// `<Key value="..."/>` (cleartext — never logged).
    pub key: Option<String>,
    /// `<Enabled value="1|0"/>`.
    pub enabled: bool,
}

/// `<LOCOSERVER>` block.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocoServerConfig {
    /// `<ServerName value="..."/>`.
    pub name: String,
    /// `<Port value="..."/>`.
    pub port: u16,
    /// `<Automatic value="1|0"/>`.
    pub automatic: bool,
}

/// Parse the XML body. Lenient: ignores the declaration, unknown elements,
/// and parse errors on individual attributes.
///
/// # Errors
///
/// Returns a [`String`] message when the body is not valid UTF-8 or the
/// reader fails catastrophically.
pub fn parse(body: &[u8]) -> Result<DeviceConfig, String> {
    let text = str::from_utf8(body).map_err(|e| format!("non-utf8 body: {e}"))?;
    let mut reader = Reader::from_str(text);
    reader.config_mut().trim_text(true);

    let mut cfg = DeviceConfig::default();
    let mut buf = Vec::new();
    let mut current_loco: Option<LocoConfig> = None;
    let mut current_network: Option<NetworkConfig> = None;
    let mut in_functions = false;

    loop {
        let event = reader
            .read_event_into(&mut buf)
            .map_err(|e| format!("xml read: {e}"))?;
        match event {
            Event::Start(ref e) | Event::Empty(ref e) => handle_element(
                e,
                &mut cfg,
                &mut current_loco,
                &mut current_network,
                &mut in_functions,
            ),
            Event::End(ref e) => {
                let name = e.name();
                match name.as_ref() {
                    b"LOCO" => {
                        if let Some(loco) = current_loco.take() {
                            cfg.locos.push(loco);
                        }
                    }
                    b"NETWORK" => {
                        if let Some(net) = current_network.take() {
                            cfg.networks.push(net);
                        }
                    }
                    b"FUNCTIONS" => in_functions = false,
                    _ => {}
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    Ok(cfg)
}

/// Handle one start/empty element, dispatching by tag name.
fn handle_element(
    e: &BytesStart<'_>,
    cfg: &mut DeviceConfig,
    current_loco: &mut Option<LocoConfig>,
    current_network: &mut Option<NetworkConfig>,
    in_functions: &mut bool,
) {
    let name = e.name();
    match name.as_ref() {
        b"structurVersion" => {
            cfg.structure_version = attr_str(e, b"value");
        }
        b"throttleName" => {
            cfg.throttle_name = attr_str(e, b"value");
        }
        b"firmwareRevision" => {
            cfg.firmware_revision = attr_str(e, b"value");
        }
        b"batteryVoltage" => {
            cfg.battery_mv = attr_u32(e, b"value");
        }
        b"LOCO" => {
            *current_loco = Some(LocoConfig {
                id: attr_u8(e, b"ID"),
                address: -1,
                ..Default::default()
            });
        }
        b"DCCadress" => {
            if let Some(loco) = current_loco.as_mut() {
                loco.address = attr_i64(e, b"value").unwrap_or(-1);
            }
        }
        b"Mode" => {
            if let Some(loco) = current_loco.as_mut() {
                loco.mode = attr_str(e, b"value");
            }
        }
        b"Direction" => {
            if let Some(loco) = current_loco.as_mut() {
                loco.direction = attr_u8(e, b"value");
            }
        }
        b"LongAdress" => {
            if let Some(loco) = current_loco.as_mut() {
                loco.long_address = attr_bool(e, b"value");
            }
        }
        b"FUNCTIONS" => {
            *in_functions = true;
        }
        b"Function" => {
            if *in_functions {
                if let Some(loco) = current_loco.as_mut() {
                    let index = attr_u8(e, b"ID").unwrap_or(0);
                    let value = attr_u8(e, b"value").unwrap_or(0);
                    loco.functions.push(FunctionEntry { index, value });
                }
            }
        }
        b"NETWORK" => {
            *current_network = Some(NetworkConfig::default());
        }
        b"SSID" => {
            if let Some(net) = current_network.as_mut() {
                net.ssid = attr_str(e, b"value").unwrap_or_default();
            }
        }
        b"Key" => {
            if let Some(net) = current_network.as_mut() {
                net.key = attr_str(e, b"value");
            }
        }
        b"Enabled" => {
            if let Some(net) = current_network.as_mut() {
                net.enabled = attr_bool(e, b"value").unwrap_or(false);
            }
        }
        b"ServerName" => {
            cfg.loco_server.get_or_insert_with(Default::default).name =
                attr_str(e, b"value").unwrap_or_default();
        }
        b"Port" => {
            if let Some(s) = cfg.loco_server.as_mut() {
                s.port = attr_u16(e, b"value").unwrap_or(0);
            }
        }
        b"Automatic" => {
            if let Some(s) = cfg.loco_server.as_mut() {
                s.automatic = attr_bool(e, b"value").unwrap_or(false);
            }
        }
        _ => {}
    }
}

/// Read a string-valued attribute.
fn attr_str(e: &BytesStart<'_>, name: &[u8]) -> Option<String> {
    for a in e.attributes().flatten() {
        if a.key.as_ref() == name {
            return a.unescape_value().ok().map(|c| c.into_owned());
        }
    }
    None
}

/// Read a `u8` attribute.
fn attr_u8(e: &BytesStart<'_>, name: &[u8]) -> Option<u8> {
    attr_str(e, name).and_then(|s| s.trim().parse::<u8>().ok())
}

/// Read a `u16` attribute.
fn attr_u16(e: &BytesStart<'_>, name: &[u8]) -> Option<u16> {
    attr_str(e, name).and_then(|s| s.trim().parse::<u16>().ok())
}

/// Read a `u32` attribute.
fn attr_u32(e: &BytesStart<'_>, name: &[u8]) -> Option<u32> {
    attr_str(e, name).and_then(|s| s.trim().parse::<u32>().ok())
}

/// Read an `i64` attribute.
fn attr_i64(e: &BytesStart<'_>, name: &[u8]) -> Option<i64> {
    attr_str(e, name).and_then(|s| s.trim().parse::<i64>().ok())
}

/// Read a boolean attribute ("1"/"0" or "true"/"false").
fn attr_bool(e: &BytesStart<'_>, name: &[u8]) -> Option<bool> {
    attr_str(e, name).and_then(|s| match s.trim() {
        "1" | "true" => Some(true),
        "0" | "false" => Some(false),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"<?XML version="1.0" encoding="UTF?8"?>
<wiFred>
<structurVersion value="1"/>
<throttleName value="122145"/>
<firmwareRevision value="2022-10-16-71ca8c3-master"/>
<batteryVoltage value="3850"/>
<WiFi>
  <Connected value="0"/>
  <SSID value=" "/>
  <signalStrength value=" "/>
  <macAdress value="b827abcdef"/>
</WiFi>
<LOCOS>
 <LOCO ID="1">
  <DCCadress value="3"/>
  <Mode value="128"/>
  <Direction value="0"/>
  <LongAdress value="0"/>
  <FUNCTIONS>
     <Function ID="0" value="0"/>
     <Function ID="1" value="4"/>
  </FUNCTIONS>
 </LOCO>
 <LOCO ID="2">
  <DCCadress value="-1"/>
  <Mode value=""/>
  <Direction value="0"/>
  <LongAdress value="0"/>
  <FUNCTIONS>
     <Function ID="0" value="0"/>
  </FUNCTIONS>
 </LOCO>
</LOCOS>
<NETWORKS>
 <NETWORK>
  <SSID value="bigfred2"/>
  <Key value="secret-pass"/>
  <Enabled value="1"/>
 </NETWORK>
</NETWORKS>
<LOCOSERVER>
   <ServerName value="bigfred.local"/>
   <Port value="12090"/>
   <Automatic value="0"/>
</LOCOSERVER>
<centerSwitch value="0"/>
</wiFred>
"#;

    #[test]
    fn parses_fixture_with_malformed_prolog() {
        let cfg = parse(FIXTURE.as_bytes()).expect("parse");
        assert_eq!(cfg.structure_version.as_deref(), Some("1"));
        assert_eq!(cfg.throttle_name.as_deref(), Some("122145"));
        assert_eq!(
            cfg.firmware_revision.as_deref(),
            Some("2022-10-16-71ca8c3-master")
        );
        assert_eq!(cfg.battery_mv, Some(3850));
        assert_eq!(cfg.locos.len(), 2);
        assert_eq!(cfg.locos[0].id, Some(1));
        assert_eq!(cfg.locos[0].address, 3);
        assert_eq!(cfg.locos[0].mode.as_deref(), Some("128"));
        assert_eq!(cfg.locos[0].direction, Some(0));
        assert_eq!(cfg.locos[0].long_address, Some(false));
        assert_eq!(cfg.locos[0].functions.len(), 2);
        assert_eq!(
            cfg.locos[0].functions[0],
            FunctionEntry { index: 0, value: 0 }
        );
        assert_eq!(
            cfg.locos[0].functions[1],
            FunctionEntry { index: 1, value: 4 }
        );
        assert_eq!(cfg.locos[1].address, -1);
        assert_eq!(cfg.networks.len(), 1);
        assert_eq!(cfg.networks[0].ssid, "bigfred2");
        assert_eq!(cfg.networks[0].key.as_deref(), Some("secret-pass"));
        assert!(cfg.networks[0].enabled);
        let srv = cfg.loco_server.expect("server");
        assert_eq!(srv.name, "bigfred.local");
        assert_eq!(srv.port, 12090);
        assert!(!srv.automatic);
    }

    #[test]
    fn parses_empty_body() {
        let cfg = parse(b"<wiFred></wiFred>").expect("parse");
        assert!(cfg.structure_version.is_none());
        assert!(cfg.locos.is_empty());
    }

    #[test]
    fn rejects_non_utf8() {
        assert!(parse(&[0xff, 0xfe, 0xfd]).is_err());
    }
}
