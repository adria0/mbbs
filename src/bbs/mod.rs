use mini_moka::sync::Cache;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};

mod storage;

use crate::bbs::storage::ChannelMessage;
use crate::bbs::storage::Storage;
use crate::bbs::storage::User;
use crate::bbs::storage::UserPkHash;

#[derive(Debug, Clone)]
struct Session {
    created: Instant,
    user_id: u32,
    current_channel: u32,
}

struct BBS<S: Storage> {
    storage: S,
    sessions: Cache<UserPkHash, Session>,
}

impl<S: Storage> BBS<S> {
    pub fn new(storage: S) -> Self {
        Self {
            storage,
            sessions: Cache::builder()
                .max_capacity(1024)
                .time_to_live(Duration::from_secs(3600))
                .build(),
        }
    }
    pub async fn init(&mut self) -> Result<()> {
        if self.storage.get_channel_by_name("general").await.is_err() {
            self.storage.add_channel("general").await.unwrap();
        }
        Ok(())
    }
    pub async fn handle(
        &mut self,
        user_pk_hash: [u8; 32],
        radio_userid: u32,
        command: &str,
    ) -> Result<String> {
        let mut session = if let Some(session) = self.sessions.get(&user_pk_hash) {
            session
        } else {
            let current_channel = self
                .storage
                .get_channel_by_name("general")
                .await
                .unwrap()
                .cid;

            let user_id = if let Ok(user) = self.storage.get_user_by_pkhash(&user_pk_hash).await {
                user.uid
            } else {
                self.storage
                    .add_user(&User {
                        uid: 0,
                        radio_userid: radio_userid,
                        pk_hash: user_pk_hash,
                        last_ts: 200,
                    })
                    .await?
            };

            Session {
                created: Instant::now(),
                current_channel,
                user_id,
            }
        };

        let command: Vec<_> = command.splitn(2, ' ').collect();
        match command[0] {
            "/chs" if command.len() == 1 => {
                let channels = self.storage.get_channels().await?;
                let list = channels
                    .iter()
                    .map(|c| c.name.clone())
                    .collect::<Vec<String>>()
                    .join(",");
                return Ok(list);
            }
            "/join" if command.len() == 2 => {
                let Ok(channel) = self.storage.get_channel_by_name(command[1]).await else {
                    bail!("Channel not found");
                };
                session.current_channel = channel.cid;
                self.sessions.insert(user_pk_hash, session);
                return Ok("Ack".into());
            }
            "/post" if command.len() == 2 => {
                let message = ChannelMessage {
                    ts: 0,
                    uid: session.user_id,
                    text: command[1].to_string(),
                };

                self.storage
                    .add_message(session.current_channel, &message)
                    .await?;

                return Ok("Ack".into());
            }
            _ => bail!("Unknown command"),
        }
    }
}
