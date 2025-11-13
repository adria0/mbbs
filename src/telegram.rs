use anyhow::Result;
use std::{collections::VecDeque, time::Instant};
use teloxide::{Bot, prelude::*, types::ChatId};

pub struct TelegramBot {
    bot: Bot,
    chatid: ChatId,
    pending: VecDeque<(String, Instant)>,
}

impl TelegramBot {
    pub fn new(token: String, chatid: i64) -> Self {
        Self {
            bot: Bot::new(&token),
            chatid: ChatId(chatid),
            pending: VecDeque::new(),
        }
    }
    pub async fn send_message<S: Into<String>>(&mut self, message: S) -> Result<()> {
        self.pending.push_back((message.into(), Instant::now()));
        self.send_pending_messages().await
    }
    pub async fn send_pending_messages(&mut self) -> Result<()> {
        while let Some((text, ts)) = self.pending.pop_front() {
            let suffix = if ts.elapsed() > std::time::Duration::from_secs(60) {
                format!("[{}s ago]", ts.elapsed().as_secs())
            } else {
                "".to_string()
            };
            if let Err(e) = self
                .bot
                .send_message(self.chatid, format!("{}{}", text, suffix))
                .await
            {
                eprint!("{}", e);
                self.pending.push_front((text, ts));
            }
        }

        Ok(())
    }
}
