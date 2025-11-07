//! This example connects via Bluetooth LE to the radio and prints out all received packets.
use std::collections::{BTreeMap, HashMap};
use std::io::{self, BufRead};
use std::path::{Display, Path};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Error, Result};
use meshtastic::Message;
use meshtastic::api::{ConnectedStreamApi, StreamApi, state};
use meshtastic::packet::{PacketDestination, PacketRouter};
use meshtastic::protobufs::mesh_packet;
use meshtastic::protobufs::{Data, User};
use meshtastic::protobufs::{FromRadio, PortNum};
use meshtastic::protobufs::{MeshPacket, from_radio};
use meshtastic::types::{MeshChannel, NodeId};
use meshtastic::utils;
use meshtastic::utils::stream::BleId;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

const STORAGE_FILE: &str = "storage.json";

#[derive(Debug, Serialize, Deserialize)]
struct Storage {
    users: HashMap<u32, User>,
    stats: BTreeMap<String, (u32, u64)>,
}

impl Storage {
    fn new() -> Self {
        Storage {
            stats: BTreeMap::new(),
            users: HashMap::new(),
        }
    }
    fn long_name_of(&self, user_id: u32) -> String {
        if let Some(user) = self.users.get(&user_id) {
            user.long_name.clone()
        } else {
            format!("{}", user_id)
        }
    }
    fn nodeid_by_name(&self, name: &str) -> Option<NodeId> {
        self.users
            .iter()
            .find(|(k, user)| user.long_name == name)
            .map(|(k, _)| NodeId::new(*k))
    }
    fn load(path: &Path) -> Result<Self> {
        if let Ok(fs) = std::fs::File::open(path) {
            let storage = serde_json::from_reader(fs)?;
            Ok(storage)
        } else {
            Ok(Storage::new())
        }
    }
    fn save(&self, path: &Path) -> Result<()> {
        let mut fs = std::fs::File::create(path)?;
        serde_json::to_writer(&mut fs, self)?;
        Ok(())
    }
    fn insert_stat(&mut self, mesh_packet: &MeshPacket, info: &str) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs(); // u64 seconds

        let key = format!("{}:{}", self.long_name_of(mesh_packet.from), info);
        if let Some(entry) = self.stats.get_mut(&key) {
            entry.0 += 1;
            entry.1 = now;
        } else {
            self.stats.insert(key.clone(), (1, now));
        }
    }

    fn print_stats(&self) {
        let datetime_format =
            time::format_description::parse("[month]-[day] [hour]:[minute]:[second]").unwrap();

        println!("Stats -------------------------------------");
        for (k, (count, ts)) in &self.stats {
            let dt = OffsetDateTime::from_unix_timestamp(*ts as i64).unwrap();
            let dt = dt.format(&datetime_format).unwrap();
            println!("{:>10}: {} [{}]", k, count, dt);
        }
        println!("------------------------------------------");
    }
}
#[derive(Default)]
struct MyPacketRouter {
    _source_node_id: NodeId,
}

impl MyPacketRouter {
    fn new(node_id: u32) -> Self {
        MyPacketRouter {
            _source_node_id: node_id.into(),
        }
    }
}

#[derive(Debug)]
struct RouterError {}
impl std::fmt::Display for RouterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RouterError")
    }
}
impl std::error::Error for RouterError {}

impl PacketRouter<(), RouterError> for MyPacketRouter {
    fn handle_packet_from_radio(
        &mut self,
        _packet: FromRadio,
    ) -> std::result::Result<(), RouterError> {
        println!("handle_palcket_from_radio called but not sure what to do");
        Ok(())
    }

    fn handle_mesh_packet(&mut self, _packet: MeshPacket) -> std::result::Result<(), RouterError> {
        println!("handle_mesh_packet called but not sure what to do here");
        Ok(())
    }

    fn source_node_id(&self) -> NodeId {
        self._source_node_id
    }
}

async fn send_to_meshtastic(stream_api: &mut ConnectedStreamApi, text: &str) -> Result<()> {
    // Create a text message data payload
    let data = Data {
        portnum: PortNum::TextMessageApp as i32,
        payload: text.as_bytes().to_vec(),
        want_response: false,
        ..Default::default()
    };

    // Create mesh packet for broadcast
    let mesh_packet = MeshPacket {
        to: 0xffffffff, // Broadcast address
        from: 0,        // Will be filled by the device
        channel: 0,
        id: 0, // Will be assigned by the device
        priority: mesh_packet::Priority::Default as i32,
        payload_variant: Some(mesh_packet::PayloadVariant::Decoded(data)),
        ..Default::default()
    };

    // Create the payload variant
    let payload_variant = Some(meshtastic::protobufs::to_radio::PayloadVariant::Packet(
        mesh_packet,
    ));

    // Send using the stream API's send_to_radio_packet method
    println!("Attempting to send packet to Meshtastic radio...");
    match stream_api.send_to_radio_packet(payload_variant).await {
        Ok(_) => {
            println!("Successfully sent to Meshtastic: {}", text);
            Ok(())
        }
        Err(e) => {
            println!("Failed to send to Meshtastic: {}", e);
            Err(anyhow::anyhow!("Failed to send message: {}", e))
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let stream_api = StreamApi::new();
    let mut storage = Storage::load(Path::new(STORAGE_FILE))?;
    storage.print_stats();

    let ble_device = if let Some(ble_device) = std::env::args().nth(1) {
        ble_device
    } else {
        println!("Scanning BLE devices...");
        let devices =
            meshtastic::utils::stream::available_ble_devices(Duration::from_secs(5)).await?;
        for device in devices {
            println!(
                "Found BLE device: name={:?} mac={}",
                device.name, device.mac_address
            );
        }
        return Ok(());
    };

    // You can also use `BleId::from_mac_address(..)` instead of `BleId::from_name(..)` to
    // search for a MAC address.
    let ble_stream =
        utils::stream::build_ble_stream(&BleId::from_name(&ble_device), Duration::from_secs(5))
            .await?;
    let (mut decoded_listener, stream_api) = stream_api.connect(ble_stream).await;

    let config_id = utils::generate_rand_id();
    let mut stream_api = stream_api.configure(config_id).await?;
    send_to_meshtastic(&mut stream_api, "Meshtastic BBS test").await?;

    let mut last_print = std::time::Instant::now();

    // This loop can be broken with ctrl+c, by disabling bluetooth or by turning off the radio.
    println!("Start looping");
    while let Some(decoded) = decoded_listener.recv().await {
        let Some(packet) = decoded.payload_variant else {
            continue;
        };

        match packet {
            from_radio::PayloadVariant::NodeInfo(node_info) => {
                if let Some(user) = node_info.user {
                    storage.users.insert(node_info.num, user);
                }
            }
            from_radio::PayloadVariant::Packet(mesh_packet) => {
                let info = if let Some(ref pv) = mesh_packet.payload_variant {
                    match pv {
                        mesh_packet::PayloadVariant::Decoded(decoded) => {
                            let port_num =
                                PortNum::try_from(decoded.portnum).unwrap_or(PortNum::UnknownApp);
                            match port_num {
                                PortNum::TextMessageApp => {
                                    let msg = String::from_utf8(decoded.payload.clone())
                                        .unwrap_or("Non-utf8 msg".into());
                                    format!("TextMessageApp: {}", msg)
                                }
                                PortNum::NodeinfoApp => {
                                    if let Ok(user) = User::decode(decoded.payload.as_slice()) {
                                        storage.users.insert(mesh_packet.from, user.clone());
                                        format!("NodeinfoApp: {}", user.long_name)
                                    } else {
                                        eprintln!("{:?}", mesh_packet);
                                        format!("NodeInfoDecodeErr")
                                    }
                                }
                                _ => format!("{}", port_num.as_str_name()),
                            }
                        }

                        mesh_packet::PayloadVariant::Encrypted(_) => format!("Encrypted"),
                    }
                } else {
                    format!("")
                };

                storage.insert_stat(&mesh_packet, &info);

                println!(
                    "network> {} -> {} snr:{} hop:{}({}) {}",
                    storage.long_name_of(mesh_packet.from),
                    if mesh_packet.to == 0xffffffff {
                        format!("Broadcast")
                    } else {
                        storage.long_name_of(mesh_packet.to)
                    },
                    mesh_packet.rx_snr,
                    mesh_packet.hop_limit,
                    mesh_packet.hop_start,
                    info
                );

                if last_print.elapsed() > std::time::Duration::from_secs(60) {
                    last_print = std::time::Instant::now();
                    storage.print_stats();
                }

                storage.save(Path::new(STORAGE_FILE))?;
            }
            _ => {}
        }
    }

    // Note that in this specific example, this will only be called when
    // the radio is disconnected, as the above loop will never exit.
    // Typically, you would allow the user to manually kill the loop,
    // for example, with tokio::select!.
    let _stream_api = stream_api.disconnect().await?;

    Ok(())
}

#[test]
fn test_decode() {
    let payload: Vec<u8> = vec![
        10, 9, 33, 100, 97, 101, 100, 54, 57, 100, 101, 18, 9, 240, 159, 147, 161, 95, 54, 57, 100,
        101, 26, 4, 240, 159, 147, 161, 34, 6, 232, 142, 218, 237, 105, 222, 40, 95, 66, 32, 86,
        239, 246, 241, 23, 143, 78, 97, 90, 32, 91, 124, 186, 255, 183, 25, 55, 254, 144, 234, 207,
        132, 215, 127, 215, 239, 80, 141, 204, 171, 191, 85, 72, 1,
    ];
    let ni =
        meshtastic::protobufs::User::decode(payload.as_slice()).expect("Failed to decode payload");
    println!("{:?}", ni);
    unreachable!();

    // Add your test code here
}
