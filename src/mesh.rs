use std::{io::Write, time::Duration};

use anyhow::{Result, bail};
use tokio::signal;

use crate::mesh::service::Handler;

mod router;
mod service;
mod types;
mod utils;

pub async fn dump_ble_devices() -> Result<()> {
    let devices = meshtastic::utils::stream::available_ble_devices(Duration::from_secs(2)).await?;

    for device in devices {
        println!(
            "- Found BLE device: name={:?} mac={}",
            device.name, device.mac_address
        )
    }
    Ok(())
}

pub async fn ble_device_auto() -> Result<String> {
    let mut devices =
        meshtastic::utils::stream::available_ble_devices(Duration::from_secs(2)).await?;
    match devices.len() {
        0 => {
            bail!("No BLE devices found.");
        }
        1 => {
            return Ok(devices.remove(0).name.unwrap());
        }
        _ => {
            dump_ble_devices().await?;
            bail!("Multiple devices found, please specify one.");
        }
    }
}

pub async fn repl() -> Result<()> {
    println!("Starting REPL. Type 'help' for commands.");
    let mut handler: Option<Handler> = None;
    loop {
        if let Some(handler) = &handler
            && let Some(short_name) = handler.state.read().await.my_short_name().await
        {
            print!("{short_name}");
        }
        print!(">");
        std::io::stdout().flush()?; // ensure prompt shows before blocking
        let mut command = String::new();
        std::io::stdin().read_line(&mut command)?; // reads until '\n'
        let line: Vec<&str> = command.trim().split(" ").collect(); // remove trailing newline
        match line[0] {
            "exit" | "quit" => break,
            "ble" => {
                if line.len() < 2 {
                    println!("Usage: ble <device_name|auto>");
                    dump_ble_devices().await?;
                    continue;
                }
                let mut device_name = line[1].to_string();
                if device_name == String::from("auto") {
                    match ble_device_auto().await {
                        Ok(name) => device_name = name,
                        Err(e) => {
                            println!("Error: {}", e);
                            continue;
                        }
                    }
                }
                if let Some(h) = handler.take() {
                    println!("Disconnecting from previous device...");
                    h.finish().await;
                    println!("Disconnected.");
                }

                let mut new_handler = service::Service::from_ble(&device_name).await?;
                println!("Using device: {}, booting..", device_name);
                if let Err(err) = new_handler.wait_for_boot_ready(30).await {
                    println!("Error: {}", err);
                }

                handler = Some(new_handler);
            }
            "listen" => {
                if let Some(mut handler) = handler.as_mut() {
                    listen(&mut handler).await?;
                }
            }
            "send" => {
                if line.len() < 3 {
                    println!("Usage: send <node_short_name> <message>");
                    continue;
                }
                let short_name = line[1];
                let message = line[2..].join(" ");

                if let Some(mut handler) = handler.as_mut() {
                    let user_id = {
                        let state = handler.state.read().await;
                        let Some(user_id) = state.get_node_id_by_short_name(short_name) else {
                            println!("Node not found: {}", short_name);
                            continue;
                        };
                        user_id
                    };

                    println!("Sending message to{}...", short_name);
                    handler.send_text(message, user_id).await?;
                    listen(&mut handler).await?;
                }
            }
            "nodes" => {
                if let Some(handler) = handler.as_ref() {
                    let state = handler.state.read().await;
                    let mut nodes: Vec<_> = state
                        .nodes
                        .iter()
                        .map(|(_, user)| &user.short_name)
                        .collect();
                    nodes.sort();
                    println!("{:?}", nodes);
                }
            }

            _ => {
                println!("Unknown command: {}", command);
            }
        }
    }
    Ok(())
}

pub async fn listen(handler: &mut Handler) -> Result<()> {
    println!("Listening for messages...press Ctrl+C to exit");
    loop {
        tokio::select! {
            status = handler.status_rx.recv() => {
                let Some(status) = status else { bail!("Channel closed"); };
                match status {
                    service::Status::Ready => {
                        println!("Ready");
                    },
                    service::Status::NewMessage(id) => {
                        let state = handler.state.read().await;
                        let msg = state.msg(id).await.unwrap();
                        println!("{}", state.format_msg(&msg));
                        if state.my_node_num().await == msg.to {
                            handler.send_text(format!("Got {}", msg.text), msg.from).await?;
                        }
                    },
                    service::Status::UpdatedMessage(id) => {
                        let state = handler.state.read().await;
                        let msg = state.msg(id).await.unwrap();
                        println!("{}", state.format_msg(&msg));
                    },
                    service::Status::Heartbeat(_packet_count) => {
                        println!("Heartbeat.");
                    },
                    _ => {}
                }
            }
            _ = handler.cancel.cancelled() => break,
            _ = signal::ctrl_c() => break,

        }
    }

    Ok(())
}
