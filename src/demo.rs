use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};

use log::debug;

use crate::blockchain::*;
use crate::signer::*;

use bitcoin::hashes::hex::{FromHex, ToHex};
use bitcoin::hashes::Hash;
use bitcoin::secp256k1::{All, Message, Secp256k1};
use bitcoin::util::bip143::SighashComponents;
use bitcoin::*;

use electrum_client::Client;

#[derive(Debug)]
pub struct ElectrumBlockchain<T>
where
    T: Read + Write,
{
    electrum_client: RefCell<Client<T>>,
    utxo_set: RefCell<Vec<OutPoint>>,
    capacity: usize,
}

const DEFAULT_CAPACITY: usize = 10;

impl<T> ElectrumBlockchain<T>
where
    T: Read + Write,
{
    pub fn new(electrum_client: Client<T>) -> Self {
        Self::with_capacity(electrum_client, DEFAULT_CAPACITY)
    }

    pub fn with_capacity(electrum_client: Client<T>, capacity: usize) -> Self {
        ElectrumBlockchain {
            electrum_client: RefCell::new(electrum_client),
            utxo_set: RefCell::new(Vec::with_capacity(capacity)),
            capacity,
        }
    }
}

impl<T> Blockchain for ElectrumBlockchain<T>
where
    T: Read + Write,
{
    type Error = electrum_client::types::Error;

    fn get_tx(&self, txid: &Txid) -> Result<Transaction, Self::Error> {
        self.electrum_client.borrow_mut().transaction_get(txid)
    }

    fn is_unspent(&self, txout: &OutPoint) -> Result<bool, Self::Error> {
        let script = &self.get_tx(&txout.txid)?.output[txout.vout as usize].script_pubkey;
        let unspent_utxos = &self
            .electrum_client
            .borrow_mut()
            .script_list_unspent(&script)?;
        Ok(unspent_utxos
            .into_iter()
            .filter(|x| x.tx_hash == txout.txid)
            .count()
            > 0)
    }

    fn get_random_utxo(&self, seed: &OutPoint) -> Result<Option<OutPoint>, Self::Error> {
        if self.utxo_set.borrow().len() == 0 {
            let mut txid = &seed.txid;
            let mut tx = self.get_tx(&txid)?.clone();
            let mut scripts = HashSet::new();
            let coinbase_txid =
                Txid::from_hex("0000000000000000000000000000000000000000000000000000000000000000")
                    .unwrap();
            while self.utxo_set.borrow().len() < self.capacity {
                txid = &tx.input[0].previous_output.txid;

                // We hit a coinbase!
                if txid == &coinbase_txid {
                    break;
                }

                tx = self.get_tx(&txid)?.clone();
                scripts.extend(
                    tx.output
                        .iter()
                        .map(|x| x.script_pubkey.clone())
                        .filter(|x| x.is_p2pkh())
                        .collect::<Vec<_>>(),
                );

                for script in &scripts {
                    let unspent: Vec<_> = self
                        .electrum_client
                        .borrow_mut()
                        .script_list_unspent(&script)?
                        .iter()
                        .map(|x| OutPoint {
                            txid: x.tx_hash,
                            vout: x.tx_pos as u32,
                        })
                        .collect();
                    self.utxo_set.borrow_mut().extend(unspent);
                }
            }
        }

        Ok(self.utxo_set.borrow_mut().pop())
    }

    fn broadcast(&self, tx: &Transaction) -> Result<Txid, Self::Error> {
        self.electrum_client.borrow_mut().transaction_broadcast(tx)
    }
}

#[derive(Debug)]
pub struct SoftwareSigner {
    key: PrivateKey,
    metadata: HashMap<OutPoint, (u64, Script)>,
}

impl SoftwareSigner {
    pub fn new(key: PrivateKey, metadata: HashMap<OutPoint, (u64, Script)>) -> Self {
        SoftwareSigner { key, metadata }
    }
}

impl Signer for SoftwareSigner {
    type Error = ();

    fn sign(&self, transaction: &mut Transaction, inputs: &[usize]) -> Result<(), Self::Error> {
        debug!("signing tx: {:?}", transaction);

        let secp: Secp256k1<All> = Secp256k1::gen_new();
        let comp = SighashComponents::new(&transaction);

        for (index, input) in transaction.input.iter_mut().enumerate() {
            if !inputs.contains(&index) {
                continue;
            }

            let (amount, prev_script) = self.metadata.get(&input.previous_output).unwrap();
            let script_code = Self::p2wpkh_scriptcode(&prev_script);
            println!(
                "input: {} scriptcode: {} value: {}",
                index,
                script_code.to_hex(),
                *amount
            );

            let hash = comp.sighash_all(input, &script_code, *amount);
            let sig = secp.sign(
                &Message::from_slice(&hash.into_inner()[..]).unwrap(),
                &self.key.key,
            );

            let mut pubkey = self.key.public_key(&secp);
            pubkey.compressed = true;
            let mut sig_with_sighash = sig.serialize_der().to_vec();
            sig_with_sighash.push(0x01);

            input.witness = vec![sig_with_sighash, pubkey.to_bytes().to_vec()];

            debug!("signature: {:?}", sig);
        }

        Ok(())
    }
}
