use crate::com::api::*;
use futures::stream::Stream;
use futures::Future;
use reqwest::header::{HeaderMap, HeaderName};
use reqwest::r#async::{Client as InnerClient, ClientBuilder, Decoder};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::mem;
use std::sync::Arc;
use std::time::Duration;
use url::form_urlencoded::byte_serialize;
use url::Url;

use codec::{
    Decode,
    Encode,
};
pub use substrate_subxt::{
    system::System,
    ExtrinsicSuccess,
    Call,
    Error as SubError,
    Client as SubClient,
    ClientBuilder,
};
use sub_runtime::Runtime;
use sp_core::storage::StorageKey;
use sp_keyring::AccountKeyring;

type AccountId = <Runtime as System>::AccountId;

pub const MODULE: &str = "PoC";
pub const MINING: &str = "mining";

/// A client for communicating with Pool/Proxy/Wallet.
#[derive(Clone, Debug)]
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
            .build()
            .await?;

        Self {
            inner: client,
            account_id_to_secret_phrase: Arc::new(secret_phrases),
            base_uri,
            total_size_gb,
        }
    }

    /// Get current mining info.
    pub fn get_mining_info(&self) -> impl Future<Item = MiningInfoResponse, Error = FetchError> {
        // use block_hash as gen_sig
        let block_hash = self.inner.block_hash(None).await?.unwrap().as_fixed_bytes();

        let targets_key = StorageKey(b"TargetInfo".to_vec());
        let targets_opt = self.inner.fetch(targets_key, None).await?;
        let mut base_target = 488671834567_u64;
        if let Some(targets) = targets_opt {
            let target = targets.last().unwrap();
            base_target = target.base_target;
        }

        let mut height = 0_u64;
        let mut deadline = 0_u64;
        let dl_key = StorageKey(b"DlInfo".to_vec());
        let dl_opt = self.inner.fetch(dl_key, None).await?;
        if let Some(dls) = dl_opt {
            if let Some(dl) = dls.last(){
                height = dl.block;
                deadline = dl.best_dl;
            }
        }
        Ok(MiningInfoResponse{
            base_target,
            height,
            generation_signature: block_hash,
            target_deadline: deadline,
        })

    }

    /// Submit nonce to the pool and get the corresponding deadline.
    pub fn submit_nonce(
        &self,
        submission_data: &SubmissionParameters,
    ) -> impl Future<Item = SubmitNonceResponse, Error = FetchError> {
        let signer = AccountKeyring::Alice.pair();
        let xt = self.inner.xt(signer, None).await?;
        let xt_result = xt
            .watch()
            .submit(Self::mining(submission_data.account_id, submission_data.gen_sig, submission_data.nonce, submission_data.deadline))
            .await?;
        match xt_result {
            Ok(success) => {
                match success
                    .find_event::<(AccountId, bool)>(
                        MODULE, "VerifyDeadline",
                    ) {
                    Some(Ok((_id, verify_ok))) => {
                        return Ok(SubmitNonceResponse{verify_result})
                    }
                    Some(Err(err)) => return Err(err.into()),
                    None => return Err(FetchError::Substrate(SubError::Other("Failed to find PoC::VerifyDeadline".to_string()))),
                }
            }
            Err(err) => Err(err),
        }
    }

    fn mining(account_id: u64, sig: [u8; 32], nonce: u64, deadline: u64) -> Call<MiningArgs>{
        Call::new(MODULE, MINING, MiningArgs{
            account_id,
            sig,
            nonce,
            deadline,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio;

    static BASE_URL: &str = "http://94.130.178.37:31000";

    #[test]
    fn test_submit_params_cmp() {
        let submit_params_1 = SubmissionParameters {
            account_id: 1337,
            nonce: 12,
            height: 112,
            block: 0,
            deadline_unadjusted: 7123,
            deadline: 1193,
            gen_sig: [0; 32],
        };

        let mut submit_params_2 = submit_params_1.clone();
        submit_params_2.block += 1;
        assert!(submit_params_1 < submit_params_2);

        let mut submit_params_2 = submit_params_1.clone();
        submit_params_2.deadline -= 1;
        assert!(submit_params_1 < submit_params_2);

        let mut submit_params_2 = submit_params_1.clone();
        submit_params_2.gen_sig[0] = 1;
        submit_params_2.deadline += 1;
        assert!(submit_params_1 < submit_params_2);

        let mut submit_params_2 = submit_params_1.clone();
        submit_params_2.deadline += 1;
        assert!(submit_params_1 > submit_params_2);
    }

    #[test]
    fn test_requests() {
        let mut rt = tokio::runtime::Runtime::new().expect("can't create runtime");

        let client = Client::new(
            BASE_URL.parse().unwrap(),
            HashMap::new(),
            5000,
            12,
            ProxyDetails::Enabled,
            HashMap::new(),
        );

        let height = match rt.block_on(client.get_mining_info()) {
            Err(e) => panic!(format!("can't get mining info: {:?}", e)),
            Ok(mining_info) => mining_info.height,
        };

        // this fails if pinocchio switches to a new block height in the meantime
        let nonce_submission_response = rt.block_on(client.submit_nonce(&SubmissionParameters {
            account_id: 1337,
            nonce: 12,
            height,
            block: 1,
            deadline_unadjusted: 7123,
            deadline: 1193,
            gen_sig: [0; 32],
        }));

        if let Err(e) = nonce_submission_response {
            assert!(false, format!("can't submit nonce: {:?}", e));
        }
    }
}
