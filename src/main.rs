//! This example connects via Bluetooth LE to the radio and prints out all received packets.
#[allow(unused)]
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use clap::{Parser, Subcommand};
use meshtastic::protobufs::MeshPacket;

mod mesh;
mod repl;
mod service;
mod storage;
mod telegram;
mod utils;

use serde_cbor::Deserializer;
use storage::Storage;
use tokio::select;
use tokio_util::sync::CancellationToken;

use crate::service::Service;
use crate::telegram::TelegramBot;

include!(concat!(env!("OUT_DIR"), "/build_info.rs"));

#[derive(Parser)]
#[command(name = "mbbs")]
#[command(version = VERSION)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Discover BLE nodes
    Repl,
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
        Commands::Repl => repl::repl().await?,
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
    println!("VERSION {}", VERSION);

    let telegram_bot_token = std::env::var("TELEGRAM_BOT_TOKEN")?;
    let telegram_bot_chatid = std::env::var("TELEGRAM_BOT_CHATID")?.parse()?;
    let ble_device = std::env::var("BLE_DEVICE")?;

    let cancel = CancellationToken::new();
    let cancel_ctrl_c = cancel.clone();
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        cancel_ctrl_c.cancel();
    });

    log::info!("Connecting to telegram...");
    let mut bot = TelegramBot::new(telegram_bot_token, telegram_bot_chatid);
    let mut storage = Storage::default();

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
