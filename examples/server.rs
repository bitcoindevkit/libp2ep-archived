use std::str::FromStr;
use std::collections::HashMap;

use log::{info, debug};

use libp2ep::bitcoin::*;
use libp2ep::bitcoin::hashes::hex::FromHex;
use libp2ep::bitcoin::secp256k1::{Secp256k1, All};
use libp2ep::server::*;
use libp2ep::demo::*;

fn main() {
    env_logger::init();

    let secp: Secp256k1<All> = Secp256k1::gen_new();
    let sk = PrivateKey::from_str("5JYkZjmN7PVMjJUfJWfRFwtuXTGB439XV6faajeHPAM9Z2PT2R3").unwrap();
    let address = Address::p2wpkh(&sk.public_key(&secp), Network::Regtest);
    info!("address: {}", address.to_string());

    let our_output = OutPoint {
        txid: Txid::from_hex("dae03e5b58137ccf6432d80dcaf183814d36301b580d083d6b263129bb57f4c2").unwrap(),
        vout: 0,
    };

    let mut meta_map = HashMap::new();
    meta_map.insert(our_output.clone(), (200_000_000, address.script_pubkey()));

    let electrum = ElectrumBlockchain::new();
    let signer = SoftwareSigner::new(sk, meta_map);

    let server = Server::new("127.0.0.1:9000", electrum, signer).unwrap();
    server.mainloop(&our_output).unwrap();
}
