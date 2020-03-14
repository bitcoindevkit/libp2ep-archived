use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::io::{Write, BufRead, BufReader};
use std::collections::HashMap;

use log::{debug, info, trace};

use bitcoin::{Transaction, TxOut, TxIn, Script, PublicKey, OutPoint, Txid};
use bitcoin::secp256k1::{Secp256k1, Message as SecpMessage, Signature, All};
use bitcoin::blockdata::script::Builder;
use bitcoin::blockdata::opcodes::all::OP_RETURN;
use bitcoin::util::bip143::SighashComponents;

use crate::{VERSION, Message, ProtocolError, Error, WitnessWrapper};
use crate::blockchain::Blockchain;
use crate::signer::Signer;

#[derive(Debug, Default)]
struct ServerState
{
    client_version: Option<String>,
    client_proof: Option<Transaction>,
    client_witnesses: Option<(Vec<WitnessWrapper>, usize, usize)>,

    real_utxo_position: Option<usize>,
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

    our_utxo: OutPoint,
    expected_script: Script,
    expected_amount: u64,
}

impl<B, S> Server<B, S>
where
    B: Blockchain + std::fmt::Debug,
    <B as Blockchain>::Error: Into<Error> + std::fmt::Debug,
    S: Signer + std::fmt::Debug,
    <S as Signer>::Error: Into<Error> + std::fmt::Debug,
{
    pub fn new<A: ToSocketAddrs>(bind: A, blockchain: B, signer: S, our_utxo: OutPoint, expected_script: Script, expected_amount: u64) -> Result<Server<B, S>, Error> {
        Ok(Server {
            listener: TcpListener::bind(bind)?,
            blockchain,
            signer,

            our_utxo,
            expected_script,
            expected_amount,
        })
    }

    pub fn mainloop(&self) -> Result<(), Error> {
        info!("Server running!");

        for stream in self.listener.incoming() {
            debug!("Accepting connection");
            let result = self.handle_client(stream?);
            debug!("result = {:?}", result);

            if result.is_ok() {
                break;
            }
        }

        Ok(())
    }

    fn handle_client(&self, mut stream: TcpStream) -> Result<(), Error> {
        let mut bufreader = BufReader::new(stream.try_clone()?);
        let mut raw_line = String::new();
        let mut state = ServerState::default();

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

            let (new_state, response) = self.apply_message(state, message)?;
            debug!("<== {:?}", response);

            let mut raw = serde_json::to_vec(&response.to_request()?)?;
            raw.extend_from_slice(b"\n");
            stream.write_all(&raw)?;
            stream.flush()?;

            raw_line.clear();
            state = new_state;
        }

        Ok(())
    }

    fn apply_message(&self, mut state: ServerState, message: Message) -> Result<(ServerState, Message), Error> {
        let VERSION_STRING: String = VERSION.into();

        match (&state.client_version, message.clone()) {
            (None, Message::Version{version: VERSION_STRING}) => {
                state.client_version = Some(VERSION_STRING.clone());
                return Ok((state, Message::Version{version: VERSION_STRING}));
            }
            (None, Message::Version{version}) => return Err(ProtocolError::InvalidVersion(version.into()).into()),
            (None, _) => return Err(ProtocolError::Expected("VERSION".into()).into()),
            _ => {},
        }

        match (&state.client_proof, message.clone()) {
            (None, Message::Proof{transaction}) => {
                self.validate_proof(&transaction)?;
                state.client_proof = Some(transaction);

                // TODO: add random utxos
                state.real_utxo_position = Some(0);
                let response = Message::Utxos{utxos: vec![self.our_utxo.clone()]};
                return Ok((state, response));
            },
            (None, _) => return Err(ProtocolError::Expected("PROOF".into()).into()),
            _ => {},
        }

        match (&state.client_witnesses, message.clone()) {
            (None, Message::Witnesses{witnesses, change_script, fees, receiver_input_position, receiver_output_position}) => {
                let txid = self.validate_witnesses(state.client_proof.as_ref().unwrap(), change_script, fees, receiver_input_position, receiver_output_position, witnesses.get(state.real_utxo_position.unwrap()).ok_or(ProtocolError::InvalidProof)?)?;
                state.client_witnesses = Some((witnesses[state.real_utxo_position.unwrap()].clone(), receiver_input_position, receiver_output_position));

                let response = Message::Txid{txid};
                return Ok((state, response));
            },
            (None, _) => return Err(ProtocolError::Expected("WITNESSES".into()).into()),
            _ => {},
        }

        Err(ProtocolError::UnexpectedMessage.into())
    }

    fn validate_proof(&self, tx: &Transaction) -> Result<(), Error> {
        let expected_script = Builder::new().push_opcode(OP_RETURN).into_script();

        // One single output of 21M Bitcoin
        if tx.output.len() == 0 || tx.output[0].value != 21_000_000__00_000_000 || tx.output[0].script_pubkey != expected_script {
            trace!("Initial checks failed");
            return Err(ProtocolError::InvalidProof.into());
        }

        let secp: Secp256k1<All> = Secp256k1::gen_new();
        let comp = SighashComponents::new(tx);

        // Only P2WPKH inputs and unspent
        for input in &tx.input {
            let prev_tx = self.blockchain.get_tx(&input.previous_output.txid).map_err(|_| ProtocolError::InvalidProof)?;
            let prev_out = prev_tx.output.get(input.previous_output.vout as usize).ok_or(ProtocolError::InvalidProof)?;
            if !prev_out.script_pubkey.is_v0_p2wpkh() || !self.blockchain.is_unspent(&input.previous_output).map_err(|_| ProtocolError::InvalidProof)? {
                trace!("Invalid prev_out (wrong type or spent)");
                return Err(ProtocolError::InvalidProof.into());
            }

            let hash = comp.sighash_all(&input, &prev_out.script_pubkey, prev_out.value);
            let pubkey = input.witness.get(0).ok_or(ProtocolError::InvalidProof)?;
            let signature = input.witness.get(1).ok_or(ProtocolError::InvalidProof)?;
            let sig_len = signature.len() - 1;

            secp.verify(
                &SecpMessage::from_slice(&hash).unwrap(),
                &Signature::from_der(&signature[..sig_len]).map_err(|_| ProtocolError::InvalidProof)?,
                &PublicKey::from_slice(&pubkey).map_err(|_| ProtocolError::InvalidProof)?.key,
            ).map_err(|_| ProtocolError::InvalidProof)?;
        }

        Ok(())
    }

    fn validate_witnesses(&self, tx: &Transaction, sender_change: Script, fees: u64, our_input_pos: usize, our_output_pos: usize, witnesses: &Vec<WitnessWrapper>) -> Result<Txid, Error> {
        // TODO
        // add our input
        // add the two outputs
        // check the outputs amount
        // check the outputs addresses (types/addresses)
        // sign
        // verify everything
        // locktime?

        let mut tx = tx.clone();
        tx.output.clear();

        let mut sender_input_value = 0;
        for input in &tx.input {
            let prev_tx = self.blockchain.get_tx(&input.previous_output.txid).map_err(|e| e.into())?;
            sender_input_value += prev_tx.output[input.previous_output.vout as usize].value;
        }
        let their_output = TxOut {
            script_pubkey: sender_change,
            value: sender_input_value.checked_sub(fees).ok_or(ProtocolError::InvalidProof)?.checked_sub(self.expected_amount).ok_or(ProtocolError::InvalidProof)?,
        };

        let our_prev_tx = self.blockchain.get_tx(&self.our_utxo.txid).map_err(|e| e.into())?;
        let our_prev_value = our_prev_tx.output[self.our_utxo.vout as usize].value;
        let our_output = TxOut {
            script_pubkey: self.expected_script.clone(),
            value: self.expected_amount + our_prev_value,
        };

        if our_output_pos == 0 {
            tx.output.extend_from_slice(&vec![our_output, their_output]);
        } else {
            tx.output.extend_from_slice(&vec![their_output, our_output]);
        }

        tx.input.insert(our_input_pos, TxIn {
            previous_output: self.our_utxo,
            sequence: 0xFFFFFFFF,
            ..Default::default()
        });
        self.signer.sign(&mut tx, &[our_input_pos]).map_err(|e| e.into())?;

        // TODO: tx.verify()
        self.blockchain.broadcast(&tx).map_err(|e| e.into())?;

        Ok(tx.txid())
    }
}
