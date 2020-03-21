use std::collections::HashMap;
use std::str::FromStr;

use tokio::runtime::Runtime;
use tokio::task;

use log::info;

use libp2ep::bitcoin::hashes::hex::FromHex;
use libp2ep::bitcoin::secp256k1::{All, Secp256k1};
use libp2ep::bitcoin::*;
use libp2ep::demo::*;
use libp2ep::server::*;

fn main() {
    env_logger::init();

    let mut rt = Runtime::new().unwrap();
    let local = task::LocalSet::new();

    local.block_on(&mut rt, run());
}

async fn run() {
    let secp: Secp256k1<All> = Secp256k1::gen_new();
    let sk = PrivateKey::from_str("KwDiBf89QgGbjEhKnhXJuH7LrciVrZi3qYjgd9M7rFU73sVHnoWn").unwrap();
    let address = Address::p2wpkh(&sk.public_key(&secp), Network::Regtest);
    //info!("address: {}", address.to_string());

    let our_output = OutPoint {
        txid: Txid::from_hex("17eb46f996ebfbc404080872e29352cc55dc3906458ceb279bc9eb768727c5e0")
            .unwrap(),
        vout: 0,
    };

    let mut meta_map = HashMap::new();
    meta_map.insert(our_output.clone(), (200_000_000, address.script_pubkey()));

    let electrum = ElectrumBlockchain::new();
    let signer = SoftwareSigner::new(sk, meta_map);

    let mut server = Server::new(
        "127.0.0.1:9000",
        electrum,
        signer,
        our_output,
        address.script_pubkey(),
        3_000_000,
    )
    .await
    .unwrap();

    let full_addr = server.setup(Network::Regtest).unwrap();
    info!("BIP21: {}", full_addr);

    server.mainloop().await.unwrap();
}
