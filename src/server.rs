use std::convert::TryFrom;

use std::time::Duration;

use tokio::net::{TcpListener, ToSocketAddrs};

use log::{debug, info, warn};

use rand::Rng;

use bitcoin::{OutPoint, Script, Transaction, TxIn, TxOut, Txid};

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

                    let mut utxos = Vec::with_capacity(100);
                    for _i in 0..99 {
                        utxos.push(self.blockchain.get_random_utxo()?);
                    }
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
        })
    }

    pub async fn mainloop(&mut self) -> Result<(), Error> {
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
                Ok(_) => break,
                Err(e) => warn!("{:?}", e),
            }
        }

        Ok(())
    }
}
