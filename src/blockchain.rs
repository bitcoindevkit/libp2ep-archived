use bitcoin::{OutPoint, Transaction, Txid};
//use std::collections::HashSet;

pub trait Blockchain {
    type Error;

    fn get_tx(&self, txid: &Txid) -> Result<Transaction, Self::Error>;
    fn is_unspent(&self, txout: &OutPoint) -> Result<bool, Self::Error>;
    fn get_random_utxo(&self, txout: &OutPoint, seed: u64) -> Result<Vec<OutPoint>, Self::Error>;
    fn broadcast(&self, tx: &Transaction) -> Result<Txid, Self::Error>;
}
