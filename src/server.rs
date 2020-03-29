use std::convert::TryFrom;
use std::fs::File;
use std::io::Read;
use std::time::Duration;

use rand::distributions::Alphanumeric;
use rand::{thread_rng, Rng};

use tokio::net::{TcpListener, ToSocketAddrs};

use log::{debug, info, warn};

use bitcoin::hashes::hex::{FromHex, ToHex};
use bitcoin::{Address, Network, OutPoint, Script, Transaction, TxIn, TxOut, Txid};

use libtor::{HiddenServiceVersion, Tor, TorAddress, TorFlag};

use crate::blockchain::Blockchain;
use crate::common::*;
use crate::jsonrpc::*;
use crate::signer::Signer;
use crate::{Error, ProtocolError, Request, Response, VERSION};

#[derive(Debug)]
enum StateVariant {
    WaitingVersion,
    ClientVersion {
        version: String,
    },
    ClientProof {
        version: String,
        proof: ProofTransaction<Validated>,
        utxos: Vec<OutPoint>,
        our_utxo_position: usize,
    },
    ClientWitnesses {
        version: String,
        final_transaction: Transaction,
    },
}

#[derive(Debug)]
struct ServerState<'a, B, S> {
    our_utxo: OutPoint,
    our_txout: TxOut,

    state: StateVariant,

    blockchain: &'a B,
    signer: &'a S,
}

impl<'a, B, S> ServerState<'a, B, S>
where
    B: Blockchain + std::fmt::Debug,
    Error: From<<B as Blockchain>::Error>,
    S: Signer + std::fmt::Debug,
    Error: From<<S as Signer>::Error>,
{
    fn new(
        our_utxo: OutPoint,
        our_txout: TxOut,
        blockchain: &'a B,
        signer: &'a S,
    ) -> ServerState<'a, B, S> {
        ServerState {
            our_utxo,
            our_txout,
            state: StateVariant::WaitingVersion,
            blockchain,
            signer,
        }
    }

    fn transition(&mut self, message: Request) -> Result<Option<Response>, Error> {
        match &self.state {
            StateVariant::WaitingVersion => match message {
                Request::Version { version } if version == VERSION => {
                    self.state = StateVariant::ClientVersion { version };

                    Ok(Some(Response::Version {
                        version: VERSION.to_string(),
                    }))
                }
                Request::Version { version } => Err(ProtocolError::InvalidVersion(version).into()),
                _ => Err(ProtocolError::Expected("VERSION".into()).into()),
            },
            StateVariant::ClientVersion { version } => match message {
                Request::Proof { transaction } => {
                    let proof =
                        ProofTransaction::<Validated>::try_from((transaction, self.blockchain))?;

                    let mut utxos = self
                        .blockchain
                        .get_random_utxo(&self.our_utxo, thread_rng().gen::<u64>())?;

                    let our_utxo_position = rand::thread_rng().gen_range(0, 100);
                    utxos.insert(our_utxo_position, self.our_utxo.clone());

                    self.state = StateVariant::ClientProof {
                        version: version.to_string(),
                        proof,
                        utxos: utxos.clone(),
                        our_utxo_position,
                    };

                    Ok(Some(Response::Utxos { utxos }))
                }
                _ => Err(ProtocolError::Expected("PROOF".into()).into()),
            },
            StateVariant::ClientProof {
                version,
                proof,
                our_utxo_position,
                ..
            } => match message {
                Request::Witnesses {
                    witnesses,
                    change_script,
                    fees,
                    receiver_input_position,
                    receiver_output_position,
                } => {
                    let receiver_txin = TxIn {
                        sequence: 0xFFFF_FFFF,
                        previous_output: self.our_utxo,
                        ..Default::default()
                    };
                    let final_transaction_meta = FinalTransactionMeta {
                        tx: proof.clone(),
                        fees,
                        sender_script: change_script,
                        receiver_txin,
                        receiver_input_index: receiver_input_position,
                        receiver_txout: self.our_txout.clone(),
                        receiver_output_index: receiver_output_position,
                    };
                    let final_transaction = FinalTransaction::<Unsigned>::try_from((
                        final_transaction_meta,
                        self.blockchain,
                    ))?;
                    let final_transaction = FinalTransaction::<SenderSigned>::try_from((
                        final_transaction,
                        witnesses
                            .get(*our_utxo_position)
                            .ok_or(ProtocolError::MissingData)?,
                    ))?;
                    let final_transaction =
                        FinalTransaction::<Signed>::try_from((final_transaction, self.signer))?;

                    self.blockchain.broadcast(&final_transaction)?;

                    self.state = StateVariant::ClientWitnesses {
                        version: version.to_string(),
                        final_transaction: final_transaction.clone().into_inner(),
                    };

                    Ok(Some(Response::Txid {
                        txid: final_transaction.txid(),
                        transaction: final_transaction.into_inner(),
                    }))
                }
                _ => Err(ProtocolError::Expected("WITNESSES".into()).into()),
            },
            _ => Err(ProtocolError::UnexpectedMessage.into()),
        }
    }
}

impl<'a, B, S> JsonRpcState for ServerState<'a, B, S>
where
    B: Blockchain + std::fmt::Debug,
    Error: From<<B as Blockchain>::Error>,
    S: Signer + std::fmt::Debug,
    Error: From<<S as Signer>::Error>,
{
    type OutMessage = Response;
    type InMessage = Request;
    type Response = Txid;
    type Error = Error;

    fn message(
        &mut self,
        message: Self::InMessage,
    ) -> Result<Option<Self::OutMessage>, Self::Error> {
        Ok(self.transition(message)?)
    }

    fn done(&self) -> Result<Self::Response, ()> {
        if let StateVariant::ClientWitnesses {
            final_transaction, ..
        } = &self.state
        {
            Ok(final_transaction.txid())
        } else {
            Err(())
        }
    }
}

pub struct Server<B, S>
where
    B: Blockchain + std::fmt::Debug,
    S: Signer + std::fmt::Debug,
{
    listener: TcpListener,
    blockchain: B,
    signer: S,

    our_utxo: OutPoint,
    our_txout: TxOut,

    tor_hs: Option<String>,
}

impl<B, S> Server<B, S>
where
    B: Blockchain + std::fmt::Debug,
    Error: From<<B as Blockchain>::Error>,
    S: Signer + std::fmt::Debug,
    Error: From<<S as Signer>::Error>,
{
    pub async fn new<A: ToSocketAddrs>(
        bind: A,
        blockchain: B,
        signer: S,
        our_utxo: OutPoint,
        expected_script: Script,
        expected_amount: u64,
    ) -> Result<Server<B, S>, Error> {
        Ok(Server {
            listener: TcpListener::bind(bind).await?,
            blockchain,
            signer,

            our_utxo,
            our_txout: TxOut {
                script_pubkey: expected_script,
                value: expected_amount,
            },

            tor_hs: None,
        })
    }

    fn start_tor(&mut self) -> Result<String, Error> {
        let rand_string: String = thread_rng().sample_iter(&Alphanumeric).take(30).collect();

        let mut dir = std::env::temp_dir();
        dir.push(rand_string);

        debug!("Using tempdir: {}", dir.display());

        Tor::new()
            .flag(TorFlag::DataDirectory(dir.to_str().unwrap().into()))
            .flag(TorFlag::SocksPort(0))
            .flag(TorFlag::HiddenServiceDir(
                dir.join("hs").to_str().unwrap().into(),
            ))
            .flag(TorFlag::HiddenServiceVersion(HiddenServiceVersion::V3))
            .flag(TorFlag::HiddenServicePort(
                TorAddress::Port(9000),
                None.into(),
            ))
            .start_background();

        let hostname_file = dir.join("hs/hostname");
        let mut attempts = 0;

        while attempts < 10 && !hostname_file.exists() {
            debug!("Waiting for the HS hostname...");
            std::thread::sleep(Duration::from_secs(1));

            attempts += 1;
        }

        let mut file = File::open(hostname_file)?;
        let mut contents = String::new();
        file.read_to_string(&mut contents)?;
        contents = contents.trim().into();

        debug!("HS: {}", contents);
        self.tor_hs = Some(contents.clone());

        Ok(contents)
    }

    pub fn setup(&mut self, network: Network) -> Result<String, Error> {
        if self.tor_hs.is_none() {
            info!("Starting Tor...");
            self.start_tor()?;
        }

        Ok(format!(
            "bitcoin:{}?amount={}&endpoint={}",
            Address::from_script(&self.our_txout.script_pubkey, network).unwrap(),
            self.our_txout.value,
            self.tor_hs.as_ref().unwrap()
        ))
    }

    pub async fn mainloop(&mut self) -> Result<(), Error> {
        self.setup(Network::Regtest)?;

        info!("Server running!");

        loop {
            let (mut stream, _) = self.listener.accept().await?;
            debug!("Accepting connection");

            // Handle in the same task on purpose, to avoid conflicts with multiple connections at
            // the same time
            let state = ServerState::new(
                self.our_utxo,
                self.our_txout.clone(),
                &self.blockchain,
                &self.signer,
            );
            let mut jsonrpc = JsonRpc::new(&mut stream, state, Duration::from_secs(10));
            match jsonrpc.mainloop().await {
                Ok(_) => {
                    // sleep a little bit to allow the client to read everything from the socket
                    // before closing it

                    std::thread::sleep(Duration::from_secs(1));
                    break;
                }
                Err(e) => warn!("{:?}", e),
            }
        }

        Ok(())
    }
}
