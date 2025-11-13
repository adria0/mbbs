use anyhow::Result;
use meshtastic::protobufs::{MeshPacket, User};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};
use time::OffsetDateTime;

#[derive(Debug, Serialize, Deserialize)]
pub struct Storage {
    pub users: HashMap<u32, User>,
    pub stats: BTreeMap<String, (u32, u64)>,
}

impl Storage {
    pub fn new() -> Self {
        Storage {
            stats: BTreeMap::new(),
            users: HashMap::new(),
        }
    }
    pub fn long_name_of(&self, user_id: u32) -> String {
        if let Some(user) = self.users.get(&user_id) {
            user.long_name.clone()
        } else {
            format!("{}", user_id)
        }
    }
    pub fn load(path: &Path) -> Result<Self> {
        if let Ok(fs) = std::fs::File::open(path) {
            let storage = serde_json::from_reader(fs)?;
            Ok(storage)
        } else {
            Ok(Storage::new())
        }
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        let mut fs = std::fs::File::create(path)?;
        serde_json::to_writer(&mut fs, self)?;
        Ok(())
    }
    pub fn insert_stat(&mut self, mesh_packet: &MeshPacket, info: &str) {
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

    pub fn print_stats(&self) {
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
