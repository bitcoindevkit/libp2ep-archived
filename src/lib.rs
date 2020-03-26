// C                S
//    -- VERS -->
//    <-- VERS ---
//    -- PROOF -->
//    <-- TXOUT --
//    -- SIGS  -->
//    <-- TXID ---

use std::convert::TryFrom;

use serde::{de, ser};
use serde::{Deserialize, Serialize};
use serde_json::json;

pub use bitcoin;
use bitcoin::consensus::{deserialize, serialize, Decodable, Encodable};
use bitcoin::hashes::hex::{Error as HexError, FromHex, ToHex};
use bitcoin::{OutPoint, Script, Transaction, Txid};

const VERSION: &str = "1.0";

pub mod blockchain;
pub mod client;
pub mod common;
pub mod demo;
pub mod jsonrpc;
pub mod server;
pub mod signer; // TODO: not pub

pub use blockchain::Blockchain;
pub use client::Client;
pub use server::Server;
pub use signer::Signer;

macro_rules! impl_error {
    ( $err:ident, $from:ty, $to:ident ) => {
        impl std::convert::From<$from> for $err {
            fn from(err: $from) -> Self {
                $err::$to(err)
            }
        }
    };
}

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
pub enum Request {
    Version {
        version: String,
    },
    Proof {
        #[serde(deserialize_with = "from_hex", serialize_with = "to_hex")]
        transaction: Transaction,
    },
    Witnesses {
        fees: u64,
        change_script: Script,
        receiver_input_position: usize,
        receiver_output_position: usize,
        witnesses: Vec<Vec<WitnessWrapper>>,
    },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Message {
    Request {
        #[serde(flatten)]
        request: Request,
    },
    Response {
        result: Response,
    },
    Error {
        error: ProtocolError,
    },
}

impl From<Request> for Message {
    fn from(request: Request) -> Message {
        Message::Request { request }
    }
}
impl From<Response> for Message {
    fn from(result: Response) -> Message {
        Message::Response { result }
    }
}
impl From<ProtocolError> for Message {
    fn from(error: ProtocolError) -> Message {
        Message::Error { error }
    }
}

impl TryFrom<Message> for Request {
    type Error = Error;

    fn try_from(other: Message) -> Result<Request, Error> {
        if let Message::Request { request, .. } = other {
            Ok(request)
        } else {
            Err(ProtocolError::UnexpectedMessage.into())
        }
    }
}
impl TryFrom<Message> for Response {
    type Error = Error;

    fn try_from(other: Message) -> Result<Response, Error> {
        if let Message::Response { result, .. } = other {
            Ok(result)
        } else {
            Err(ProtocolError::UnexpectedMessage.into())
        }
    }
}

impl Message {
    pub fn as_json(&self, id: &str) -> Result<serde_json::Value, Error> {
        let mut data = match self {
            Message::Request { request, .. } => serde_json::to_value(request)?,
            Message::Response { result, .. } => json!({"result": serde_json::to_value(result)?}),
            Message::Error { error, .. } => json!({"error": serde_json::to_value(error)?}),
        };

        data["jsonrpc"] = "2.0".into();
        data["id"] = id.into();

        Ok(data)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum Response {
    Version {
        version: String,
    },
    Utxos {
        utxos: Vec<OutPoint>,
    },
    Txid {
        txid: Txid,
        #[serde(deserialize_with = "from_hex", serialize_with = "to_hex")]
        transaction: Transaction,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum ProtocolError {
    UnexpectedMessage,
    Expected(String),
    InvalidVersion(String),
    InvalidProof(common::ProofTransactionError),
    InvalidFinalTransaction(common::FinalTransactionError),
    InvalidUtxo,
    MissingData,
}

impl_error!(ProtocolError, common::ProofTransactionError, InvalidProof);
impl_error!(
    ProtocolError,
    common::FinalTransactionError,
    InvalidFinalTransaction
);

#[derive(Debug)]
pub enum Error {
    Serde(serde_json::Error),
    IO(std::io::Error),
    Socks(tokio_socks::Error),
    Electrum(electrum_client::types::Error),

    Protocol(ProtocolError),
    PeerError(ProtocolError),
    Timeout,
    EOF,
    Other,
}

impl_error!(Error, serde_json::Error, Serde);
impl_error!(Error, std::io::Error, IO);
impl_error!(Error, tokio_socks::Error, Socks);
impl_error!(Error, electrum_client::Error, Electrum);

impl From<()> for Error {
    fn from(_other: ()) -> Self {
        Error::Other
    }
}

impl<T: Into<ProtocolError>> From<T> for Error {
    fn from(other: T) -> Self {
        Error::Protocol(other.into())
    }
}

#[cfg(test)]
mod test {
    use crate::*;

    #[test]
    fn test() {
        let msg = ProtocolError::MissingData;
        let msg: Message = msg.into();
        let json = msg.as_json("42").unwrap();
        println!("{:#?}", json);

        let msg: Message = serde_json::from_value(json).unwrap();
        println!("{:?}", msg);
    }

    use electrum_client::Client as ElectrumClient;
    #[test]
    fn electrum_client() {
        let client = ElectrumClient::new("kirsche.emzy.de:50001").unwrap();
        let electrum = demo::ElectrumBlockchain::with_capacity(client, 1);
        let coinbase_seed = OutPoint {
            txid: Txid::from_hex(
                "8bc784db1013c86f17addf91163055647fbfd4b8c78bfe96809b014764bbf5d4",
            )
            .unwrap(),
            vout: 0,
        };
        let utxo = electrum.get_random_utxo(&coinbase_seed);
        assert!(utxo.is_ok());
        assert!(utxo.unwrap().is_none());
        let seed = OutPoint {
            txid: Txid::from_hex(
                "0768c50f4b337a9e8a7791b8f20ef8a68130e2529192f5c8ff3bc382c6653559",
            )
            .unwrap(),
            vout: 0,
        };
        let utxo = electrum.get_random_utxo(&seed);
        assert!(utxo.is_ok());
        assert!(utxo.unwrap().is_some());
    }
}
