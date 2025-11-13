//! This example connects via Bluetooth LE to the radio and prints out all received packets.

#[allow(unused)]
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Result, anyhow};
use chrono::Local;
use clap::{Parser, Subcommand};
use meshtastic::Message;
use meshtastic::api::StreamApi;
use meshtastic::protobufs::mesh_packet;
use meshtastic::protobufs::{Data, User};
use meshtastic::protobufs::{FromRadio, PortNum};
use meshtastic::protobufs::{MeshPacket, from_radio};
use meshtastic::utils::stream::BleId;

const STORAGE_FILE: &str = "storage.json";

mod storage;
mod telegram;
//mod utils;

use serde_cbor::Deserializer;
use storage::Storage;
use tokio::select;
use tokio_util::sync::CancellationToken;

use crate::telegram::TelegramBot;

#[derive(Parser)]
#[command(name = "mbbs")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the network node
    Start,
    /// Discover peers
    Discover,
    /// Dump and pretty-print a CBOR file
    Dump {
        /// Path to the CBOR file
        file: PathBuf,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_secs()
        .init();

    let cli = Cli::parse();
    match cli.command {
        Commands::Start => start().await?,
        Commands::Discover => discover().await?,
        Commands::Dump { file } => dump(file).await?,
    }

    Ok(())
}

async fn dump(path: PathBuf) -> Result<()> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let stream = Deserializer::from_reader(reader).into_iter::<MeshPacket>();
    for mesh_packet in stream {
        println!("{:?}", mesh_packet);
    }
    Ok(())
}

async fn discover() -> Result<()> {
    log::info!("Scanning BLE devices...");
    let devices = meshtastic::utils::stream::available_ble_devices(Duration::from_secs(5)).await?;
    for device in devices {
        log::info!(
            "Found BLE device: name={:?} mac={}",
            device.name,
            device.mac_address
        );
    }
    Ok(())
}

async fn start() -> Result<()> {
    let telegram_bot_token = std::env::var("TELEGRAM_BOT_TOKEN")?;
    let telegram_bot_chatid = std::env::var("TELEGRAM_BOT_CHATID")?.parse()?;
    let ble_device = std::env::var("BLE_DEVICE")?;

    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        cancel_clone.cancel();
    });

    log::info!("Connecting to telegram...");
    let mut bot = TelegramBot::new(telegram_bot_token, telegram_bot_chatid);
    let mut storage = Storage::load(Path::new(STORAGE_FILE))?;

    loop {
        let mut service = Service::new(cancel.clone(), &mut bot, &mut storage, ble_device.clone());
        if let Err(err) = service.run().await {
            bot.send_message(format!("⚠️ Error running service: {}", err))
                .await?;
            log::error!("Error running service: {}", err);
        }

        select! {
            _ = cancel.cancelled() => break,
            _ = tokio::time::sleep(Duration::from_secs(5)) => {},
        };
    }
    Ok(())
}

struct Service<'a> {
    cancel: CancellationToken,
    bot: &'a mut TelegramBot,
    storage: &'a mut Storage,
    ble_device: String,
}

impl<'a> Service<'a> {
    pub fn new(
        cancel: CancellationToken,
        bot: &'a mut TelegramBot,
        storage: &'a mut Storage,
        ble_device: String,
    ) -> Self {
        Self {
            cancel,
            bot,
            storage,
            ble_device,
        }
    }

    pub async fn run(&mut self) -> Result<()> {
        log::info!("Opening BLE to meshtastic device {}...", &self.ble_device);
        let _ = self
            .bot
            .send_message(format!("Start get events from {}", &self.ble_device))
            .await;

        let ble_stream = meshtastic::utils::stream::build_ble_stream(
            &BleId::from_name(&self.ble_device),
            Duration::from_secs(5),
        )
        .await?;

        let stream_api = StreamApi::new();
        let (mut decoded_listener, stream_api) = stream_api.connect(ble_stream).await;

        let config_id = meshtastic::utils::generate_rand_id();
        let stream_api = stream_api.configure(config_id).await?;

        let mut err = None;

        let mut idle_counter = 0;
        while err.is_none() {
            select! {
                _ = self.cancel.cancelled() => break,
                Some(from_radio) = decoded_listener.recv() => {
                    log::info!("Got message.");
                    idle_counter=0;
                    if let Err(process_err) = self.process(from_radio).await {
                        err = Some(process_err);
                    }
                }
                _ = tokio::time::sleep(Duration::from_secs(10)) => {
                    idle_counter += 10;
                    log::info!("Nothing happened in {}s, but still alive", idle_counter);
                    if idle_counter >= 300{
                        log::warn!("Idle for more than 300s, I'm going to reset");
                        err = Some(anyhow!("Idle for more than 300s, I'm going to reset"));
                    }
                }
            }
        }

        let _ = stream_api.disconnect().await?;

        if let Some(err) = err {
            Err(err)
        } else {
            Ok(())
        }
    }

    pub async fn process(&mut self, from_radio: FromRadio) -> Result<()> {
        let Some(payload_variant) = from_radio.payload_variant else {
            return Ok(());
        };

        match payload_variant {
            // This message is recieved from the storage of the node
            from_radio::PayloadVariant::NodeInfo(node_info) => {
                if let Some(user) = node_info.user {
                    self.storage.users.insert(node_info.num, user);
                }
            }
            // This message is recieved from the mesh
            from_radio::PayloadVariant::Packet(mesh_packet) => {
                if let Some(ref pv) = mesh_packet.payload_variant {
                    match pv {
                        mesh_packet::PayloadVariant::Decoded(data) => {
                            self.process_data(&mesh_packet, data).await?;
                        }
                        mesh_packet::PayloadVariant::Encrypted(_) => {}
                    }
                }
            }
            _ => {}
        }

        Ok(())
    }

    pub async fn process_data(&mut self, mesh_packet: &MeshPacket, data: &Data) -> Result<()> {
        let date = Local::now().format("%Y-%m-%d").to_string();
        let filename = format!("network.{}.cbor", date);
        let file = File::options().create(true).append(true).open(&filename)?;
        serde_cbor::to_writer(&file, &mesh_packet)?;

        let port_num = PortNum::try_from(data.portnum).unwrap_or(PortNum::UnknownApp);
        let from = self.storage.long_name_of(mesh_packet.from);
        let to = if mesh_packet.to == 0xffffffff {
            format!("BROADCAST")
        } else {
            self.storage.long_name_of(mesh_packet.to)
        };

        match port_num {
            PortNum::TextMessageApp => {
                let msg = String::from_utf8(data.payload.clone()).unwrap_or("Non-utf8 msg".into());
                self.bot
                    .send_message(format!("💬 {} : {} ({})", from, msg, to))
                    .await?;
            }
            PortNum::NodeinfoApp => {
                if let Ok(user) = User::decode(data.payload.as_slice()) {
                    self.storage.users.insert(mesh_packet.from, user.clone());
                }
            }
            _ => {}
        };
        Ok(())
    }
}
