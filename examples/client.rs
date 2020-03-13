use std::str::FromStr;
use std::collections::HashMap;

use log::{info, debug};

use libp2ep::bitcoin::*;
use libp2ep::bitcoin::secp256k1::{Secp256k1, All};
use libp2ep::bitcoin::consensus::encode::{serialize, deserialize};
use libp2ep::bitcoin::hashes::hex::FromHex;
use libp2ep::client::*;
use libp2ep::blockchain::*;
use libp2ep::signer::*;
use libp2ep::demo::*;

fn main() {
    env_logger::init();

    let secp: Secp256k1<All> = Secp256k1::gen_new();
    let sk = PrivateKey::from_str("cVt4o7BGAig1UXywgGSmARhxMdzP5qvQsxKkSsc1XEkw3tDTQFpy").unwrap();
    let address = Address::p2wpkh(&sk.public_key(&secp), Network::Regtest);
    info!("address: {}", address.to_string());

    let previous_output = OutPoint {
        txid: Txid::from_hex("c790622f0b33ff5b99ee10f8cb4bfb9271390ed7cfeb596209be75fb6d86e088").unwrap(),
        vout: 0,
    };
    let vin = TxIn {
        previous_output,
        sequence: 0xFFFFFFFF,
        ..Default::default()
    };
    let tx = Transaction {
        version: 2,
        lock_time: 0,
        input: vec![vin],
        output: vec![],
    };

    let mut meta_map = HashMap::new();
    meta_map.insert(tx.input[0].previous_output.clone(), (100_000_000, address.script_pubkey()));

    let electrum = ElectrumBlockchain::new();
    let signer = SoftwareSigner::new(sk, meta_map);

    let mut client = Client::new("127.0.0.1:9000", electrum, signer).unwrap();
    client.start(&tx, &address.script_pubkey()).unwrap();
}
