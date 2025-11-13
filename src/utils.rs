use meshtastic::{
    Message,
    protobufs::{Data, FromRadio, MyNodeInfo, PortNum, User, from_radio, mesh_packet},
    types::NodeId,
};

pub enum RecievedPacket {
    ConnectionClosed,
    RoutingApp(Data),
    MyInfo(MyNodeInfo),
    NodeInfo(NodeId, User),
    TextMessage {
        from: NodeId,
        to: NodeId,
        msg: String,
    },
    Other,
}
impl From<Option<FromRadio>> for RecievedPacket {
    fn from(from_radio: Option<FromRadio>) -> Self {
        use RecievedPacket::*;
        let Some(from_radio) = from_radio else {
            return ConnectionClosed;
        };
        let Some(payload) = from_radio.payload_variant else {
            return Other;
        };
        match payload {
            from_radio::PayloadVariant::MyInfo(my_node_info) => MyInfo(my_node_info),
            from_radio::PayloadVariant::NodeInfo(node_info) => {
                if let Some(user) = node_info.user {
                    NodeInfo(NodeId::new(node_info.num), user)
                } else {
                    Other
                }
            }
            from_radio::PayloadVariant::Packet(recv_packet) => {
                let Some(pv) = recv_packet.payload_variant else {
                    return Other;
                };
                let mesh_packet::PayloadVariant::Decoded(data) = pv else {
                    return Other;
                };
                match PortNum::try_from(data.portnum) {
                    Ok(PortNum::RoutingApp) => RoutingApp(data),
                    Ok(PortNum::TextMessageApp) => {
                        let msg = String::from_utf8(data.payload.clone())
                            .unwrap_or("Non-utf8 msg".into());
                        TextMessage {
                            from: NodeId::new(data.source),
                            to: NodeId::new(data.dest),
                            msg,
                        }
                    }
                    Ok(PortNum::NodeinfoApp) => {
                        if let Ok(user) = User::decode(data.payload.as_slice()) {
                            NodeInfo(NodeId::new(recv_packet.from), user)
                        } else {
                            Other
                        }
                    }
                    _ => Other,
                }
            }
            _ => Other,
        }
    }
}
