use anyhow::{Result, bail};
use std::{collections::HashMap, future::pending, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::{
        RwLock,
        mpsc::{UnboundedReceiver, UnboundedSender},
        oneshot,
    },
};
use tokio_util::sync::CancellationToken;

use meshtastic::{
    Message,
    api::{ConnectedStreamApi, StreamApi, StreamHandle, state::Configured},
    packet::PacketDestination,
    protobufs::{
        Data, FromRadio, MeshPacket, MyNodeInfo, PortNum, Routing, User, from_radio,
        mesh_packet::{self, Priority},
        routing,
    },
    types::{MeshChannel, NodeId},
    utils::{
        generate_rand_id,
        stream::{BleId, build_ble_stream},
    },
};

use crate::mesh::service;

use super::router::*;
pub use super::types::*;

#[macro_export]
macro_rules! r {
    ($slf:ident . $field:ident) => {
        $slf.state.read().await.$field
    };
}
#[macro_export]
macro_rules! w {
    ($slf:ident . $field:ident) => {
        $slf.state.write().await.$field
    };
}

use TextMessageStatus::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Heartbeat(usize),
    Ready,
    NewMessage(u32),
    UpdatedMessage(u32),
}

#[derive(Default)]
pub struct HandlerState {
    pub my_node_info: Option<MyNodeInfo>,
    pub nodes: HashMap<u32, User>,
    pub messages: HashMap<u32, TextMessage>,
}

pub type State = Arc<RwLock<HandlerState>>;

pub struct Handler {
    pub state: State,
    pub msg_tx: UnboundedSender<TextMessage>,
    pub status_rx: UnboundedReceiver<Status>,

    pub cancel: CancellationToken,
    finished_rx: tokio::sync::oneshot::Receiver<()>,
}

pub struct Service {
    state: State,
    cancel: CancellationToken,
    packet_rx: UnboundedReceiver<FromRadio>,
    stream_api: ConnectedStreamApi<Configured>,
    msg_rx: UnboundedReceiver<TextMessage>,
    status_tx: UnboundedSender<Status>,
    finished_tx: tokio::sync::oneshot::Sender<()>,
    config_complete: bool,
}

impl HandlerState {
    pub fn get_long_name_by_node_id(&self, user_id: u32) -> Option<String> {
        self.nodes.get(&user_id).map(|user| user.long_name.clone())
    }
    pub fn get_short_name_by_node_id(&self, user_id: u32) -> Option<String> {
        self.nodes.get(&user_id).map(|user| user.long_name.clone())
    }
    pub fn get_node_id_by_short_name(&self, short_name: &str) -> Option<u32> {
        for (id, user) in &self.nodes {
            if user.short_name == short_name {
                return Some(*id);
            }
        }
        None
    }

    pub fn format_msg(&self, msg: &TextMessage) -> String {
        let me = self.my_node_info.as_ref().unwrap().my_node_num;
        let name = |id| {
            self.get_long_name_by_node_id(id)
                .unwrap_or(format!("NodeId({})", id))
        };

        let status = match msg.status {
            Sent => "📤".into(),
            Recieved => "".into(),
            ImplicitAck => "✔️".into(),
            ExplicitAck => "✔️✔️".into(),
            RoutingError(error) => format!("❌ {:?}", error),
        };

        if msg.to == 0xffffffff {
            format!("💬 {} : {} {} ", name(msg.from), msg.text, status)
        } else if msg.to == me {
            format!("👤 {} : {} {}", name(msg.from), msg.text, status)
        } else {
            format!(
                "📩 {} → {} : {} {}",
                name(msg.from),
                name(msg.to),
                msg.text,
                status
            )
        }
    }

    pub async fn msg(&self, id: u32) -> Option<TextMessage> {
        self.messages.get(&id).cloned()
    }

    pub async fn my_node_num(&self) -> u32 {
        self.my_node_info.as_ref().unwrap().my_node_num
    }
    pub async fn my_short_name(&self) -> Option<String> {
        self.my_node_info
            .as_ref()
            .and_then(|n| self.get_short_name_by_node_id(n.my_node_num))
    }
}

impl Handler {
    pub async fn wait_for_boot_ready(&mut self, timeout_secs: u64) -> Result<()> {
        let now = tokio::time::Instant::now();
        loop {
            tokio::select! {
                status = self.status_rx.recv() => {
                    let Some(status) = status else { bail!("Channel closed"); };
                    if status == service::Status::Ready {
                        break;
                    }
                },
                _ = self.cancel.cancelled() => bail!("Cancelled"),
                _ = tokio::time::sleep(Duration::from_secs(1)) => {
                    if now.elapsed().as_secs() >= timeout_secs {
                        bail!("Timeout reached");
                    }
                }
            }
        }
        Ok(())
    }
    pub async fn send_text<T: Into<String>, D: Into<Destination>>(
        &self,
        text: T,
        to: D,
    ) -> Result<()> {
        let from = r!(self.my_node_info).as_ref().unwrap().my_node_num;
        let to = match to.into() {
            Destination::Node(node_num) => node_num,
            Destination::Broadcast => 0xffffffff,
            Destination::ShortName(short_name) => {
                let mut id = None;
                for (node_id, node) in &r!(self.nodes) {
                    if node.short_name == short_name {
                        id = Some(*node_id);
                        break;
                    }
                }
                let Some(id) = id else {
                    bail!("Node '{short_name}' not found")
                };
                id
            }
        };
        self.msg_tx.send(TextMessage::sent(from, to, text.into()))?;
        Ok(())
    }
    pub async fn finish(mut self) {
        self.cancel.cancel();
        loop {
            tokio::select! {
                _ = &mut self.finished_rx => {
                    break;
                }
                _ = tokio::time::sleep(Duration::from_millis(100)) => {}
            }
        }
    }
}

impl Service {
    pub async fn from_ble(ble_device: &str) -> Result<Handler> {
        let ble_stream =
            build_ble_stream(&BleId::from_name(&ble_device), Duration::from_secs(5)).await?;
        Self::build(ble_stream).await
    }

    async fn build<S>(stream_handle: StreamHandle<S>) -> Result<Handler>
    where
        S: AsyncReadExt + AsyncWriteExt + Send + 'static,
    {
        let stream_api = StreamApi::new();
        let config_id = generate_rand_id();

        let (packet_rx, stream_api) = stream_api.connect(stream_handle).await;
        let stream_api = stream_api.configure(config_id).await?;

        let (status_tx, status_rx) = tokio::sync::mpsc::unbounded_channel::<Status>();
        let (msg_tx, msg_rx) = tokio::sync::mpsc::unbounded_channel::<TextMessage>();

        let (finished_tx, finished_rx) = oneshot::channel::<()>();

        let state = Arc::new(RwLock::new(HandlerState::default()));

        let cancel = CancellationToken::new();

        let handler = Handler {
            state: state.clone(),
            cancel: cancel.clone(),
            msg_tx,
            status_rx,
            finished_rx,
        };

        let service = Service {
            state,
            cancel,
            packet_rx,
            stream_api,
            msg_rx,
            status_tx,
            finished_tx,
            config_complete: false,
        };

        tokio::spawn(service.start());

        Ok(handler)
    }
    pub async fn start(mut self) -> Result<()> {
        let mut buffer_flushed = false;
        self.status_tx.send(Status::Heartbeat(0))?;
        let mut packet_count = 0;
        let mut hearthbeat_interval = 1000;
        loop {
            tokio::select! {
                from_radio = self.packet_rx.recv() => {
                    packet_count += 1;
                    let Some(from_radio) = from_radio else {
                        bail!("BLE stream closed");
                    };
                    self.process_from_radio(from_radio).await?;
                }
                msg = async {
                    if buffer_flushed {
                        self.msg_rx.recv().await
                    } else {
                        pending().await
                    }
                } => {
                    let Some(msg) = msg else {
                        bail!("Text message stream closed");
                    };
                    self.process_send_text(msg).await?;
                }
                _ = tokio::time::sleep(Duration::from_millis(hearthbeat_interval)) => {
                    if !buffer_flushed && self.config_complete {
                        buffer_flushed = true;
                        hearthbeat_interval = 10_000;
                        self.status_tx.send(Status::Ready)?;
                    } else {
                        self.status_tx.send(Status::Heartbeat(packet_count))?;
                    }
                }
                _ = self.cancel.cancelled() => {
                    break;
                }
            }
        }
        self.packet_rx.close();
        self.stream_api.disconnect().await?;
        self.finished_tx.send(()).unwrap();
        Ok(())
    }

    async fn process_send_text(&mut self, msg: TextMessage) -> Result<()> {
        let from = r!(self.my_node_info).as_ref().unwrap().my_node_num;
        let mut packet_router = Router::new(NodeId::new(from));
        self.stream_api
            .send_text(
                &mut packet_router,
                msg.text.clone(),
                PacketDestination::Node(NodeId::new(msg.to)),
                true,
                MeshChannel::new(0).unwrap(),
            )
            .await?;
        let id = packet_router.last_sent().unwrap().id;
        w!(self.messages).insert(id, msg);
        self.status_tx.send(Status::NewMessage(id))?;

        Ok(())
    }

    async fn process_from_radio(&mut self, from_radio: FromRadio) -> Result<()> {
        let Some(payload) = from_radio.payload_variant else {
            bail!("No payload");
        };
        match payload {
            // Load for information about my node
            from_radio::PayloadVariant::MyInfo(node_info) => {
                w!(self.my_node_info) = Some(node_info);
            }
            // Local for the data in NodeDB
            from_radio::PayloadVariant::NodeInfo(node_info) if node_info.user.is_some() => {
                w!(self.nodes).insert(node_info.num, node_info.user.unwrap());
            }
            from_radio::PayloadVariant::ConfigCompleteId(_) => {
                self.config_complete = true;
            }
            // Mesh packet loaded
            from_radio::PayloadVariant::Packet(mesh_packet) => {
                if let Some(mesh_packet::PayloadVariant::Decoded(ref data)) =
                    mesh_packet.payload_variant
                {
                    match PortNum::try_from(data.portnum) {
                        Ok(PortNum::NodeinfoApp) => {
                            self.handle_nodeinfo(&mesh_packet, data).await?
                        }
                        Ok(PortNum::TextMessageApp) => {
                            self.handle_textmessage(&mesh_packet, data).await?
                        }
                        Ok(PortNum::RoutingApp) => self.handle_routing(&mesh_packet, &data).await?,
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    async fn handle_nodeinfo(&self, mesh_packet: &MeshPacket, data: &Data) -> Result<()> {
        let user = User::decode(data.payload.as_slice())?;
        w!(self.nodes).insert(mesh_packet.from, user);
        Ok(())
    }

    async fn handle_textmessage(&self, mesh_packet: &MeshPacket, data: &Data) -> Result<()> {
        let msg = String::from_utf8(data.payload.clone())?;
        w!(self.messages).insert(
            mesh_packet.id,
            TextMessage::recieved(mesh_packet.from, mesh_packet.to, msg),
        );
        self.status_tx.send(Status::NewMessage(mesh_packet.id))?;

        Ok(())
    }

    async fn handle_routing(&self, mesh_packet: &MeshPacket, data: &Data) -> Result<()> {
        let Routing { variant } = Routing::decode(data.payload.as_slice())?;
        let Some(routing::Variant::ErrorReason(routing_error)) = variant else {
            return Ok(());
        };
        let mut status = None;

        if routing_error != routing::Error::None as i32 {
            status = Some(RoutingError(routing::Error::try_from(routing_error)?));
        } else if mesh_packet.from == mesh_packet.to && mesh_packet.priority == Priority::Ack as i32
        {
            status = Some(ImplicitAck);
        } else if mesh_packet.from != mesh_packet.to {
            status = Some(ExplicitAck);
        }

        if let Some(msg) = w!(self.messages).get_mut(&data.request_id)
            && let Some(status) = status
        {
            msg.status = status;
            self.status_tx
                .send(Status::UpdatedMessage(data.request_id))?;
        }

        Ok(())
    }
}
