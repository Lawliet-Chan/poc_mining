use bytes::Buf;
use reqwest::r#async::Chunk;
use serde::de::{self, DeserializeOwned};
use std::fmt;
use codec::{
    Decode,
    Encode,
};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmitNonceRequest<'a> {
    pub request_type: &'a str,
    pub account_id: u64,
    pub nonce: u64,
    pub secret_phrase: Option<&'a String>,
    pub blockheight: u64,
    pub deadline: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetMiningInfoRequest<'a> {
    pub request_type: &'a str,
}

#[derive(Encode)]
pub struct SubmitNonceResponse {
    pub verify_result: bool,
}

#[derive(Encode)]
pub struct MiningArgs {
    pub account_id: u64,
    pub height: u64,
    pub sig: [u8; 32],
    pub nonce: u64,
    pub deadline: u64,
}

pub struct MiningInfoResponse {
    pub generation_signature: [u8; 32],

    pub base_target: u64,

    pub height: u64,

    pub target_deadline: u64,
}

fn default_target_deadline() -> u64 {
    std::u64::MAX
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PoolErrorWrapper {
    error: PoolError,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PoolError {
    pub code: i32,
    pub message: String,
}

#[derive(Debug)]
pub enum FetchError {
    Http(reqwest::Error),
    Pool(PoolError),
    Substrate(substrate_subxt::Error),
}

impl From<substrate_subxt::Error> for FetchError {
    fn from(err: substrate_subxt::Error) -> FetchError {
        FetchError::Substrate(err)
    }
}

impl From<reqwest::Error> for FetchError {
    fn from(err: reqwest::Error) -> FetchError {
        FetchError::Http(err)
    }
}

impl From<PoolError> for FetchError {
    fn from(err: PoolError) -> FetchError {
        FetchError::Pool(err)
    }
}

// MOTHERFUCKING pool
fn from_str_or_int<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: de::Deserializer<'de>,
{
    struct StringOrIntVisitor;

    impl<'de> de::Visitor<'de> for StringOrIntVisitor {
        type Value = u64;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("string or int")
        }

        fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
            v.parse::<u64>().map_err(de::Error::custom)
        }

        fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E> {
            Ok(v)
        }
    }

    deserializer.deserialize_any(StringOrIntVisitor)
}

pub fn parse_json_result<T: DeserializeOwned>(body: &Chunk) -> Result<T, PoolError> {
    match serde_json::from_slice(body.bytes()) {
        Ok(x) => Ok(x),
        _ => match serde_json::from_slice::<PoolErrorWrapper>(body.bytes()) {
            Ok(x) => Err(x.error),
            _ => {
                let v = body.to_vec();
                Err(PoolError {
                    code: 0,
                    message: String::from_utf8_lossy(&v).to_string(),
                })
            }
        },
    }
}
