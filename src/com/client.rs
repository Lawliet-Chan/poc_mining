use crate::com::api::*;
use futures::stream::Stream;
use futures::Future;
use futures::future;
use reqwest::header::{HeaderMap, HeaderName};
use reqwest::r#async::{Client as InnerClient, Decoder};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::mem;
use std::sync::Arc;
use std::time::Duration;
use url::form_urlencoded::byte_serialize;
use url::Url;

pub use substrate_subxt::{
    system::System,
    ExtrinsicSuccess,
    Call,
    Error as SubError,
    Client as SubClient,
    DefaultNodeRuntime as Runtime,
    ClientBuilder,
};
use sp_core::storage::StorageKey;
use sp_keyring::AccountKeyring;
use sp_runtime::traits::{SaturatedConversion, Header};
use sub_runtime::poc::{Difficulty, MiningInfo};

type AccountId = <Runtime as System>::AccountId;

pub const MODULE: &str = "PoC";
pub const MINING: &str = "mining";

/// A client for communicating with Pool/Proxy/Wallet.
#[derive(Clone)]
pub struct Client {
    inner: SubClient<Runtime>,
    account_id_to_secret_phrase: Arc<HashMap<u64, String>>,
    base_uri: Url,
    total_size_gb: usize,
}

/// Parameters ussed for nonce submission.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubmissionParameters {
    pub account_id: u64,
    pub nonce: u64,
    pub height: u64,
    pub block: u64,
    pub deadline_unadjusted: u64,
    pub deadline: u64,
    pub gen_sig: [u8; 32],
}

/// Usefull for deciding which submission parameters are the newest and best.
/// We always cache the currently best submission parameters and on fail
/// resend them with an exponential backoff. In the meantime if we get better
/// parameters the old ones need to be replaced.
impl Ord for SubmissionParameters {
    fn cmp(&self, other: &Self) -> Ordering {
        if self.block < other.block {
            Ordering::Less
        } else if self.block > other.block {
            Ordering::Greater
        } else if self.gen_sig == other.gen_sig {
            // on the same chain, best deadline wins
            if self.deadline <= other.deadline {
                Ordering::Greater
            } else {
                Ordering::Less
            }
        } else {
            // switched to a new chain
            Ordering::Less
        }
    }
}

impl PartialOrd for SubmissionParameters {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Whether to send additional data for Proxies.
#[derive(Clone, PartialEq, Debug)]
pub enum ProxyDetails {
    /// Send additional data like capacity, miner name, ...
    Enabled,
    /// Don't send any additional data:
    Disabled,
}

impl Client {

    /// Create a new client communicating with Pool/Proxy/Wallet.
    pub fn new(
        base_uri: Url,
        mut secret_phrases: HashMap<u64, String>,
        timeout: u64,
        total_size_gb: usize,
        proxy_details: ProxyDetails,
        additional_headers: HashMap<String, String>,
    ) -> Self {
        for secret_phrase in secret_phrases.values_mut() {
            *secret_phrase = byte_serialize(secret_phrase.as_bytes()).collect();
        }

        let url = base_uri.as_str();
        let client = ClientBuilder::<Runtime>::new()
            .set_url(url)
            .build().unwrap();

        Self {
            inner: client,
            account_id_to_secret_phrase: Arc::new(secret_phrases),
            base_uri,
            total_size_gb,
        }
    }

    /// Get current mining info.
    pub fn get_mining_info(&self) -> impl Future<Item = MiningInfoResponse, Error = FetchError> {
        async_std::task::block_on(async move {
            // use block_hash as gen_sig
            let block_hash = self.inner.block_hash(None).await.unwrap().unwrap().as_fixed_bytes();

            let targets_key = StorageKey(b"TargetInfo".to_vec());
            let targets_opt: Option<Vec<Difficulty>> = self.inner.fetch(targets_key, None).await.unwrap();
            let mut base_target = 488671834567_u64;
            if let Some(targets) = targets_opt {
                let target = targets.last().unwrap();
                base_target = target.base_target;
            }

            let mut height = self.get_current_height().await;
            let mut deadline = 0_u64;
            let dl_key = StorageKey(b"DlInfo".to_vec());
            let dl_opt: Option<Vec<MiningInfo<AccountId>>> = self.inner.fetch(dl_key, None).await.unwrap();
            if let Some(dls) = dl_opt {
                if let Some(dl) = dls.last(){
                    deadline = dl.best_dl;
                }
            }
            future::ok(MiningInfoResponse{
                base_target,
                height,
                generation_signature: *block_hash,
                target_deadline: deadline,
            })
        })

    }

    /// Submit nonce to the pool and get the corresponding deadline.
    pub fn submit_nonce(
        &self,
        submission_data: &SubmissionParameters,
    ) -> impl Future<Item = SubmitNonceResponse, Error = FetchError> {
        let xt_result =
        async_std::task::block_on(async move {
            let signer = AccountKeyring::Alice.pair();
            let xt = self.inner.xt(signer, None).await?;
            let xt_result = xt
                .watch()
                .submit(Self::mining(
                    submission_data.account_id,
                    submission_data.height,
                    submission_data.gen_sig,
                    submission_data.nonce,
                    submission_data.deadline
                )).await?;
            Ok(xt_result)
        });

        match xt_result {
            Ok(success) => {
                match success
                    .find_event::<(AccountId, bool)>(
                        MODULE, "VerifyDeadline",
                    ) {
                    Some(Ok((_id, verify_result))) => {
                        return future::ok(SubmitNonceResponse{verify_result})
                    }
                    Some(Err(err)) => return future::err(err.into()),
                    None => return future::err(FetchError::Substrate(SubError::Other("Failed to find PoC::VerifyDeadline".to_string()))),
                }
            }
            Err(err) => future::err(err),
        }

    }

    fn mining(account_id: u64, height: u64, sig: [u8; 32], nonce: u64, deadline: u64) -> Call<MiningArgs>{
        Call::new(MODULE, MINING, MiningArgs{
            account_id,
            height,
            sig,
            nonce,
            deadline,
        })
    }

    async fn get_current_height(&self) -> u64 {
        let header = self.inner.header(None).await.unwrap().unwrap();
        let block_num = header.number();
        block_num.saturated_into::<u64>()
    }
}
