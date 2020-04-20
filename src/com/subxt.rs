use codec::{
    Decode,
    Encode,
};
use futures::future::Future;
use substrate_subxt::{
    system::System,
    ExtrinsicSuccess,
    frame::Call,
    Error,
};
use sub_runtime::Runtime;
use sp_keyring::AccountKeyring;
use sp_core::storage::StorageKey;

type AccountId = <Runtime as System>::AccountId;

pub fn get_mining_info() -> impl Future<Item = MiningInfoResp, Error = substrate_subxt::Error>{
    let targets_key = StorageKey(b"TargetInfo".to_vec());
    let cli = substrate_subxt::ClientBuilder::<Runtime>::new()
        .build()
        .await?;
    let block_hash = cli.block_hash(None).await?.unwrap();
    let block_hash = block_hash.as_fixed_bytes();
    let targets_opt = cli.fetch(targets_key, None).await?;
    if let Some(targets) = targets_opt {

    }
}

pub fn submit_nonce(account_id: u64, sig: [u8; 32], nonce: u64, deadline: u64) {
    let signer = AccountKeyring::Alice.pair();

    let cli = substrate_subxt::ClientBuilder::<Runtime>::new()
        .build()
        .await?;
    let xt = cli.xt(signer, None).await?;
    let xt_result = xt
        .watch()
        .submit(mining(account_id, sig, nonce, deadline))
        .await?;
}

pub const MODULE: &str = "PoC";
pub const MINING: &str = "mining";

#[derive(Encode)]
pub struct MiningArgs {
    account_id: u64,
    sig: [u8; 32],
    nonce: u64,
    deadline: u64,
}

fn mining(account_id: u64, sig: [u8; 32], nonce: u64, deadline: u64) -> Call<MiningArgs>{
    Call::new(MODULE, MINING, MiningArgs{
        account_id,
        sig,
        nonce,
        deadline,
    })
}