use bitcoin::{Txid, Transaction, TxOut, OutPoint};

pub trait Blockchain {
    type Error;

    fn get_tx(&self, txid: &Txid) -> Result<Transaction, Self::Error>;
    fn is_unspent(&self, txout: &OutPoint) -> Result<bool, Self::Error>;
    fn get_random_utxo(&self) -> Result<OutPoint, Self::Error>;
    fn broadcast(&self, tx: &Transaction) -> Result<(), Self::Error>;
}
