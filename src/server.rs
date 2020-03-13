use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::io::{Write, BufRead, BufReader};
use std::collections::HashMap;

use log::{debug, info, trace};

use bitcoin::{Transaction, TxOut, Script, PublicKey, OutPoint};
use bitcoin::secp256k1::{Secp256k1, Message as SecpMessage, Signature, All};
use bitcoin::blockdata::script::Builder;
use bitcoin::blockdata::opcodes::all::OP_RETURN;
use bitcoin::util::bip143::SighashComponents;

use crate::{VERSION, Message, ProtocolError, Error};
use crate::blockchain::Blockchain;
use crate::signer::Signer;

#[derive(Debug)]
struct ServerStateMachine<'b, B>
where
    B: Blockchain,
{
    client_version: Option<String>,
    client_proof: Option<Transaction>,
    client_signatures: Option<Vec<Vec<u8>>>,

    blockchain: &'b B,

    pub server_utxo: Option<OutPoint>,
}

impl<'b, B> ServerStateMachine<'b, B>
where
    B: Blockchain,
    <B as Blockchain>::Error: Into<Error> + std::fmt::Debug,
{
    fn new(blockchain: &'b B) -> Self {
        ServerStateMachine {
            client_version: None,
            client_proof: None,
            client_signatures: None,

            blockchain,

            server_utxo: None,
        }
    }

    fn apply_message(mut self, message: Message) -> Result<(Self, Message), ProtocolError> {
        let VERSION_STRING: String = VERSION.into();

        match (&self.client_version, message.clone()) {
            (None, Message::Version{version: VERSION_STRING}) => {
                self.client_version = Some(VERSION_STRING.clone());
                return Ok((self, Message::Version{version: VERSION_STRING}));
            }
            (None, Message::Version{version}) => return Err(ProtocolError::InvalidVersion(version.into())),
            (None, _) => return Err(ProtocolError::Expected("VERSION".into())),
            _ => {},
        }

        match (&self.client_proof, message.clone()) {
            (None, Message::Proof{transaction}) => {
                self.validate_proof(&transaction)?;

                // TODO: add random utxos
                let response = Message::Utxos{utxos: vec![self.server_utxo.as_ref().cloned().ok_or(ProtocolError::MissingData)?]};
                return Ok((self, response));
            },
            (None, _) => return Err(ProtocolError::Expected("PROOF".into())),
            _ => {},
        }

        unreachable!();
    }

    fn validate_proof(&self, tx: &Transaction) -> Result<(), ProtocolError> {
        let expected_script = Builder::new().push_opcode(OP_RETURN).push_slice(&[0; 32]).into_script();

        // One single output of 21M Bitcoin. V2 tx, locktime = 0
        if tx.output.len() == 0 || tx.output[0].value != 21_000_000__00_000_000 || tx.output[0].script_pubkey != expected_script || tx.version != 2 || tx.lock_time != 0 {
            trace!("Initial checks failed");
            return Err(ProtocolError::InvalidProof);
        }

        let secp: Secp256k1<All> = Secp256k1::gen_new();
        let comp = SighashComponents::new(tx);

        // Only P2WPKH inputs and unspent
        for input in &tx.input {
            let prev_tx = self.blockchain.get_tx(&input.previous_output.txid).map_err(|_| ProtocolError::InvalidProof)?;
            let prev_out = prev_tx.output.get(input.previous_output.vout as usize).ok_or(ProtocolError::InvalidProof)?;
            if !prev_out.script_pubkey.is_v0_p2wpkh() || !self.blockchain.is_unspent(&prev_out).map_err(|_| ProtocolError::InvalidProof)? {
                trace!("Invalid prev_out (wrong type or spent)");
                return Err(ProtocolError::InvalidProof);
            }

            let hash = comp.sighash_all(&input, &prev_out.script_pubkey, prev_out.value);
            let pubkey = input.witness.get(0).ok_or(ProtocolError::InvalidProof)?;
            let signature = input.witness.get(1).ok_or(ProtocolError::InvalidProof)?;

            secp.verify(
                &SecpMessage::from_slice(&hash).unwrap(),
                &Signature::from_der(&signature).map_err(|_| ProtocolError::InvalidProof)?,
                &PublicKey::from_slice(&pubkey).map_err(|_| ProtocolError::InvalidProof)?.key,
            ).map_err(|_| ProtocolError::InvalidProof)?;
        }

        Ok(())
    }
}

pub struct Server<B, S>
where
    B: Blockchain + std::fmt::Debug,
    <B as Blockchain>::Error: Into<Error> + std::fmt::Debug,
    S: Signer + std::fmt::Debug,
    <S as Signer>::Error: Into<Error> + std::fmt::Debug,
{
    listener: TcpListener,
    blockchain: B,
    signer: S,
}

impl<B, S> Server<B, S>
where
    B: Blockchain + std::fmt::Debug,
    <B as Blockchain>::Error: Into<Error> + std::fmt::Debug,
    S: Signer + std::fmt::Debug,
    <S as Signer>::Error: Into<Error> + std::fmt::Debug,
{
    pub fn new<A: ToSocketAddrs>(bind: A, blockchain: B, signer: S) -> Result<Server<B, S>, Error> {
        Ok(Server {
            listener: TcpListener::bind(bind)?,
            blockchain,
            signer,
        })
    }

    pub fn mainloop(&self, our_utxo: &OutPoint) -> Result<(), Error> {
        info!("Server running!");

        for stream in self.listener.incoming() {
            debug!("Accepting connection");
            let result = self.handle_client(stream?, our_utxo);
            debug!("result = {:?}", result);

            if result.is_ok() {
                break;
            }
        }

        Ok(())
    }

    fn handle_client(&self, mut stream: TcpStream, our_utxo: &OutPoint) -> Result<(), Error> {
        let mut bufreader = BufReader::new(stream.try_clone()?);
        let mut raw_line = String::new();
        let mut state_machine = ServerStateMachine::new(&self.blockchain);

        state_machine.server_utxo = Some(our_utxo.clone());

        while let Ok(size) = bufreader.read_line(&mut raw_line) {
            if size == 0 {
                break;
            }
            let line = raw_line.trim_end_matches(char::is_whitespace);
            if line.is_empty() {
                continue;
            }

            trace!("==> {:?}", line);

            let message = serde_json::from_str::<Message>(line)?;
            debug!("==> {:?}", message);

            let (new_sm, response) = state_machine.apply_message(message)?;
            debug!("<== {:?}", response);

            let mut raw = serde_json::to_vec(&response.to_request()?)?;
            raw.extend_from_slice(b"\n");
            stream.write_all(&raw)?;
            stream.flush()?;

            raw_line.clear();
            state_machine = new_sm;
        }

        Ok(())
    }
}
