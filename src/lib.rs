// C                S
//    -- VERS -->
//    <-- VERS ---
//    -- PROOF -->
//    <-- TXOUT --
//    -- SIGS  -->
//    <-- TXID ---

use std::convert::{TryFrom, TryInto};

use serde::{de, ser};
use serde::{Deserialize, Serialize};

pub use bitcoin;
use bitcoin::consensus::encode::Error as ConsensusEncodeError;
use bitcoin::consensus::{deserialize, serialize, Decodable, Encodable};
use bitcoin::hashes::hex::{Error as HexError, FromHex, ToHex};
use bitcoin::{OutPoint, Script, Transaction, Txid};
// TODO: wrap signatures instead of using Vec<u8>

const VERSION: &'static str = "1.0";

pub mod blockchain;
pub mod client;
pub mod demo;
pub mod server;
pub mod signer;

fn from_hex<'de, T, D>(deserializer: D) -> Result<T, D::Error>
where
    T: Decodable,
    D: de::Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    let bytes: Vec<u8> = FromHex::from_hex(&s).map_err(de::Error::custom)?;

    deserialize(&bytes).map_err(de::Error::custom)
}

fn to_hex<S, T>(data: &T, serializer: S) -> Result<S::Ok, S::Error>
where
    T: Encodable,
    S: ser::Serializer,
{
    let bytes: Vec<u8> = serialize(data);
    bytes.to_hex().serialize(serializer)
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(try_from = "String", into = "String")]
pub struct WitnessWrapper(Vec<u8>);

impl WitnessWrapper {
    pub fn new<T: Encodable>(data: &T) -> WitnessWrapper {
        WitnessWrapper(serialize(data).to_vec())
    }
}

impl AsRef<[u8]> for WitnessWrapper {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl TryFrom<String> for WitnessWrapper {
    type Error = HexError;

    fn try_from(other: String) -> Result<Self, Self::Error> {
        Ok(WitnessWrapper(FromHex::from_hex(&other)?))
    }
}

impl Into<String> for WitnessWrapper {
    fn into(self) -> String {
        self.0.to_hex()
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "UPPERCASE")]
#[serde(tag = "method", content = "params")]
pub enum Message {
    Version {
        version: String,
    },
    Proof {
        #[serde(deserialize_with = "from_hex", serialize_with = "to_hex")]
        transaction: Transaction,
    },
    Utxos {
        utxos: Vec<OutPoint>,
    },
    Witnesses {
        fees: u64,
        change_script: Script,
        receiver_input_position: usize,
        receiver_output_position: usize,
        witnesses: Vec<Vec<WitnessWrapper>>,
    },
    Txid {
        txid: Txid,
    },
}

impl Message {
    pub fn to_request(&self) -> Result<serde_json::Value, Error> {
        let mut obj = serde_json::to_value(&self)?;
        obj["jsonrpc"] = "2.0".into();
        Ok(obj)
    }
}

#[derive(Debug)]
pub enum ProtocolError {
    UnexpectedMessage,
    Expected(String),
    InvalidVersion(String),
    InvalidProof,
    InvalidUtxo,
    MissingData,
}

#[derive(Debug)]
pub enum Error {
    Serde(serde_json::Error),
    IO(std::io::Error),

    Protocol(ProtocolError),
    Other,
}

impl From<serde_json::Error> for Error {
    fn from(other: serde_json::Error) -> Self {
        Error::Serde(other)
    }
}

impl From<std::io::Error> for Error {
    fn from(other: std::io::Error) -> Self {
        Error::IO(other)
    }
}

impl From<ProtocolError> for Error {
    fn from(other: ProtocolError) -> Self {
        Error::Protocol(other)
    }
}

impl From<()> for Error {
    fn from(other: ()) -> Self {
        Error::Other
    }
}

#[cfg(test)]
mod test {
    use crate::*;

    #[test]
    fn test() {
        let msg = Message::Version {
            version: "1.0".into(),
        };
        println!("{:#?}", msg.to_request());
    }
}
