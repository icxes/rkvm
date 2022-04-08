use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer};
use std::fmt::{self, Formatter};
use std::path::PathBuf;
use std::collections::HashMap;

pub type Config = HashMap<String, Server>;

#[derive(Deserialize, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct Server {
    pub server_address: ServerAddress,
    pub certificate_path: PathBuf,
}

#[derive(Clone)]
pub struct ServerAddress {
    pub host: String,
    pub port: u16,
}

impl<'de> Deserialize<'de> for ServerAddress {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(ServerAddressVisitor)
    }
}

struct ServerAddressVisitor;

impl<'de> Visitor<'de> for ServerAddressVisitor {
    type Value = ServerAddress;

    fn expecting(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "a server description (hostname:port)")
    }

    fn visit_str<E>(self, data: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        let err = || E::custom("Invalid server description");

        let mut split = data.split(':');
        let host = split.next().ok_or_else(err)?;
        let port = split
            .next()
            .and_then(|data| data.parse().ok())
            .ok_or_else(err)?;

        if split.next().is_some() {
            return Err(E::custom("Extraneous data"));
        }

        Ok(ServerAddress {
            host: host.to_owned(),
            port,
        })
    }
}
