use std::collections::HashMap;
use std::io::{Read, Write};
use std::cell::RefCell;

use log::debug;

use crate::blockchain::*;
use crate::signer::*;

use bitcoin::consensus::encode::{deserialize, serialize};
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
}

impl<T> ElectrumBlockchain<T>
where
    T: Read + Write,
{
    pub fn new(electrum_client: Client<T>) -> Self {
        ElectrumBlockchain { electrum_client: RefCell::new(electrum_client) }
    }
}

impl<T> Blockchain for ElectrumBlockchain<T>
where
    T: Read + Write,
{
    type Error = (); //TODO: maybe use electrum_client::types::Error ?

    fn get_tx(&self, txid: &Txid) -> Result<Transaction, Self::Error> {
        self.electrum_client.borrow_mut().transaction_get(txid).map_err(|_| ())
    }

    fn is_unspent(&self, txout: &OutPoint) -> Result<bool, Self::Error> {
        let script = &self.get_tx(&txout.txid)?.output[txout.vout as usize].script_pubkey;
        let unspent_utxos = &self.electrum_client.borrow_mut().script_list_unspent(&script).map_err(|_| ())?;
        Ok(unspent_utxos.into_iter().filter(|x| x.tx_hash == txout.txid).count() > 0)
    }

    fn get_random_utxo(&self) -> Result<OutPoint, Self::Error> {
        Ok(OutPoint {
            txid: Txid::from_hex(
                "0f3fb1116e30963f1dc6631ad0cd7f00e324de7f3348264a1bba539fb4721c5d",
            )
            .unwrap(),
            vout: 0,
        })
    }

    fn broadcast(&self, tx: &Transaction) -> Result<Txid, Self::Error> {
        self.electrum_client.borrow_mut().transaction_broadcast(tx).map_err(|_| ())
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
