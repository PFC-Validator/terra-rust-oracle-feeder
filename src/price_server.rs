use chrono::{DateTime, Utc};
use reqwest::{Client, RequestBuilder};
use serde::{Deserialize, Serialize};

use crate::errors::{ErrorKind, Result};
use reqwest::header::{HeaderMap, CONTENT_TYPE, USER_AGENT};
use rust_decimal::Decimal;
use terra_rust_api::client_types::terra_decimal_format;

const VERSION: Option<&'static str> = option_env!("CARGO_PKG_VERSION");

/// Convert a JSON date time into a rust one
pub mod price_datetime_format {
    use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
    use serde::{self, Deserialize, Deserializer, Serializer};

    //2021-04-26T22:31:01.571Z
    const FORMAT: &str = "%Y-%m-%dT%H:%M:%S";

    // The signature of a serialize_with function must follow the pattern:
    //
    //    fn serialize<S>(&T, S) -> Result<S::Ok, S::Error>
    //    where
    //        S: Serializer
    //
    // although it may also be generic over the input types T.

    #[allow(missing_docs)]
    pub fn serialize<S>(date: &DateTime<Utc>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let s = format!("{}", date.format(FORMAT));
        serializer.serialize_str(&s)
    }

    // The signature of a deserialize_with function must follow the pattern:
    //
    //    fn deserialize<'de, D>(D) -> Result<T, D::Error>
    //    where
    //        D: Deserializer<'de>
    //
    // although it may also be generic over the output types T.
    #[allow(missing_docs)]
    pub fn deserialize<'de, D>(deserializer: D) -> Result<DateTime<Utc>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s: String = String::deserialize(deserializer)?;
        let len = s.len();
        let slice_len = if s.contains('.') {
            len.saturating_sub(5)
        } else {
            len
        };

        // match Utc.datetime_from_str(&s, FORMAT) {
        let sliced = &s[0..slice_len];
        match NaiveDateTime::parse_from_str(sliced, FORMAT) {
            Err(_e) => {
                eprintln!("DateTime Fail {} {:#?}", sliced, _e);
                Err(serde::de::Error::custom(_e))
            }

            Ok(dt) => Ok(Utc.from_utc_datetime(&dt)),
        }
    }
}

#[derive(Deserialize, Serialize, Debug)]
pub struct PriceResponseCurrencyPair {
    pub currency: String,
    #[serde(with = "terra_decimal_format")]
    pub price: Decimal,
}

#[derive(Deserialize, Serialize, Debug)]
pub struct PriceResponse {
    #[serde(with = "price_datetime_format")]
    pub created_at: DateTime<Utc>,
    pub prices: Vec<PriceResponseCurrencyPair>,
}

pub struct PriceServer<'a> {
    /// reqwest Client
    client: Client,
    url: &'a str,
}

impl<'a> PriceServer<'_> {
    pub fn create(url: &'a str) -> PriceServer<'a> {
        let client = reqwest::Client::new();
        PriceServer { client, url }
    }
    pub async fn get_prices(&self) -> Result<PriceResponse> {
        let req = self
            .client
            .get(self.url)
            .headers(PriceServer::construct_headers());

        PriceServer::resp::<PriceResponse>(&self.url, req).await
    }

    fn construct_headers() -> HeaderMap {
        let mut headers = HeaderMap::new();

        headers.insert(
            USER_AGENT,
            format!("PFC-TerraRust-Oracle-Feeder/{}", VERSION.unwrap_or("-?-"))
                .parse()
                .unwrap(),
        );
        headers.insert(CONTENT_TYPE, "application/json".parse().unwrap());
        headers
    }
    async fn resp<T: for<'de> Deserialize<'de>>(
        request_url: &str,
        req: RequestBuilder,
    ) -> Result<T> {
        let response = req.send().await?;
        if !response.status().is_success() {
            let status_text = response.text().await?;

            log::error!("URL={} - {}", &request_url, &status_text);
            Err(ErrorKind::PriceServer(status_text).into())
        } else {
            let struct_response: T = response.json::<T>().await?;
            Ok(struct_response)
        }
    }
}
