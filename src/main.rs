#![recursion_limit = "1024"]
// #[macro_use]
// extern crate error_chain;

use dotenv::dotenv;
use log::{debug, error, info};
// use serde::{Deserialize, Serialize};
// use std::env;
use chrono::{DateTime, Duration, Utc};
use std::thread::sleep;
use std::time::SystemTime;
use structopt::StructOpt;
use terra_rust_api::core_types::{Coin, Msg};
use terra_rust_api::messages::oracle::{
    MsgAggregateExchangeRatePreVote, MsgAggregateExchangeRateVote,
};
use terra_rust_api::{GasOptions, PrivateKey, Terra};
const VERSION: Option<&'static str> = option_env!("CARGO_PKG_VERSION");
const NAME: Option<&'static str> = option_env!("CARGO_PKG_NAME");

mod errors;
mod price_server;

use bitcoin::secp256k1::{All, Secp256k1};
use errors::*;
use price_server::PriceServer;
use rand::RngCore;
use rust_decimal_macros::dec;
use std::collections::{HashMap, HashSet};

#[derive(StructOpt)]
struct Cli {
    #[structopt(
        name = "lcd",
        env = "TERRARUST_LCD",
        default_value = "https://tequila-lcd.terra.dev",
        short,
        long = "lcd-client-url",
        help = "https://lcd.terra.dev is main-net"
    )]
    /// Terra cli Client daemon
    lcd: String,
    #[structopt(
        name = "chain",
        env = "TERRARUST_CHAIN",
        default_value = "tequila-0004",
        short,
        long = "chain",
        help = "tequila-0004 is testnet, columbus-4 is main-net"
    )]
    chain_id: String,
    /// Wallet name
    #[structopt(
        name = "wallet",
        env = "TERRARUST_WALLET",
        default_value = "default",
        short,
        long = "wallet",
        help = "the default wallet to look for keys in"
    )]
    wallet: String,
    #[structopt(
        name = "seed",
        env = "TERRARUST_SEED_PHRASE",
        default_value = "",
        short,
        long = "seed",
        help = "the seed phrase to use with this private key"
    )]
    seed: String,
    #[structopt(
        name = "fees",
        default_value = "",
        short,
        long = "fees",
        help = "the fees to use. This will override gas parameters if specified."
    )]
    fees: String,
    #[structopt(
        name = "gas",
        default_value = "auto",
        long = "gas",
        help = "the gas amount to use 'auto' to estimate"
    )]
    gas: String,
    #[structopt(
        name = "gas-prices",
        env = "TERRARUST_GAS_PRICES",
        default_value = "",
        long = "gas-prices",
        help = "the gas price to use to calculate fee. Format is NNNtoken eg. 1000uluna. note we only support a single price for now"
    )]
    gas_price: String,
    #[structopt(
        name = "gas-adjustment",
        default_value = "1.0",
        env = "TERRARUST_GAS_ADJUSTMENT",
        long = "gas-adjustment",
        help = "the adjustment to multiply the estimate to calculate the fee"
    )]
    gas_adjustment: f64,
    /// price server url
    #[structopt(
        name = "price-server-url",
        default_value = "https://127.0.0.1:8532/latest",
        short,
        long = "price-server",
        help = "URL where your price server resides"
    )]
    price_server_url: String,
    /// price server url
    #[structopt(
        name = "denoms",
        default_value = "sdr,krw,usd,mnt,eur,cny,jpy,gbp,inr,cad,chf,hkd,aud,sgd,thb",
        short,
        long = "denoms",
        help = "denominations to feed"
    )]
    denoms: String,
    #[structopt(
        name = "feeder_key",
        default_value = "feeder",
        long = "feeder",
        help = "wallet entry for feeder private key"
    )]
    feeder_key: String,
    #[structopt(
        name = "validator",
        long = "validator",
        help = "validator key in terravaloper format"
    )]
    validator: String,
    #[structopt(long, parse(from_flag))]
    debug: std::sync::atomic::AtomicBool,
}

impl Cli {
    pub fn gas_opts(&self) -> Result<GasOptions> {
        let fees = Coin::parse(&self.fees)?;
        let gas_str = &self.gas;
        let (estimate_gas, gas) = if gas_str == "auto" {
            (true, None)
        } else {
            let g = &self.gas.parse::<u64>()?;
            (false, Some(*g))
        };
        let gas_price = Coin::parse(&self.gas_price)?;
        let gas_adjustment = Some(self.gas_adjustment);
        Ok(GasOptions {
            fees,
            estimate_gas,
            gas,
            gas_price,
            gas_adjustment,
        })
    }
}

async fn do_vote<'a>(
    secp: &Secp256k1<All>,
    terra: &Terra<'a>,
    price_server: &PriceServer<'a>,
    feeder_key: &PrivateKey,
    validator: &str,
    denoms_wanted: &HashSet<String>,
    previous_salt_string: Option<String>,
    salt_string: String,
) -> Result<bool> {
    let prices = price_server.get_prices().await?;
    let x = prices
        .created_at
        .checked_add_signed(Duration::minutes(1))
        .unwrap();
    if Utc::now() > x {
        log::error!(
            "Price Feed return results from {}, which is more than a minute old. Skipping",
            prices.created_at
        );
        return Ok(false);
    } else {
        if terra.debug {
            log::debug!("Applying prices from {}", prices.created_at)
        }
    }
    let mut price_map = HashMap::new();
    for pair in prices.prices {
        if denoms_wanted.contains(pair.currency.to_lowercase().as_str()) {
            price_map.insert(pair.currency.to_lowercase(), pair.price);
        }
    }
    for d in denoms_wanted {
        if !price_map.contains_key(d) {
            price_map.insert(String::from(d), dec!(0.0));
        }
    }
    let mut coins: Vec<Coin> = Vec::with_capacity(price_map.len());
    for items in price_map {
        let coin_name = format!("u{}", items.0);
        coins.push(Coin::create(&coin_name, items.1));
    }

    let feeder_public = feeder_key.public_key(&secp);
    let mut messages: Vec<Box<dyn Msg>> = vec![];
    let vote_message = MsgAggregateExchangeRateVote::create(
        salt_string.clone(),
        coins,
        feeder_public.account()?,
        validator.parse().unwrap(),
    );
    let pre = match previous_salt_string {
        Some(prev_salt) => {
            let pre_vote_message: MsgAggregateExchangeRatePreVote =
                vote_message.gen_pre_vote(&prev_salt);
            messages.push(Box::new(vote_message));
            pre_vote_message
        }
        None => {
            let pre_vote_message: MsgAggregateExchangeRatePreVote =
                vote_message.gen_pre_vote(&salt_string);
            pre_vote_message
        }
    };

    messages.push(Box::new(pre));

    let memo = format!(
        "PFC-{}@{}",
        NAME.unwrap_or("TERRA-RUST-ORACLE-FEEDER"),
        VERSION.unwrap_or("")
    );
    // let messages: Vec<Box<dyn Msg>> = vec![Box::new(vote_message), Box::new(pre_vote_message)];
    let (signed_msg, sigs) = terra
        .generate_transaction_to_broadcast(secp, feeder_key, &messages, Some(memo))
        .await?;
    let resp = terra.tx().broadcast_async(&signed_msg, &sigs).await?;
    /*
       match resp.code {
           Some(code) => {
               log::error!("{}", serde_json::to_string(&resp)?);
               eprintln!("Transaction returned a {} {}", code, resp.txhash);
               log::error!("TX FAIL {}", resp.txhash);
               Ok(false)
           }
           None => {
               log::info!("TX  OK  {}", resp.txhash);
               println!("TX OK {}", resp.txhash);
               Ok(true)
           }
       }

    */

    println!("TX OK {}", resp.txhash);
    Ok(true)
}
/// Generate the random string
fn gen_salt() -> [u8; 2] {
    let mut salt: [u8; 2] = [0; 2];

    rand::thread_rng().fill_bytes(&mut salt);
    salt
    //  let salt_string = hex::encode(salt);
    //  salt_string
}
fn u_currency_to_real(u_currency: &str) -> Result<&str> {
    if u_currency.to_lowercase().starts_with("u") {
        Ok(&u_currency[1..])
    } else {
        Err(ErrorKind::OracleWhiteList.into())
    }
}

// main loop
// TODO: every X hours, refresh key from store
async fn run() -> Result<()> {
    let cli: Cli = Cli::from_args();
    let gas_opts: GasOptions = cli.gas_opts()?;

    let terra = Terra::lcd_client(
        &cli.lcd,
        &cli.chain_id,
        &gas_opts,
        Some(cli.debug.into_inner()),
    )
    .await?;
    let seed: Option<&str> = if cli.seed.is_empty() {
        None
    } else {
        Some(&cli.seed)
    };

    let secp = Secp256k1::new();
    let private_key = get_private_key(&secp, &cli.wallet, &cli.feeder_key, seed)?;

    let price_server = PriceServer::create(&cli.price_server_url);
    let mut denoms_set = HashSet::new();
    for d in cli.denoms.split(",") {
        //  let lc = d.to_string().to_lowercase();
        denoms_set.insert(d);
    }

    log::debug!("A{:?}", &denoms_set);

    let mut last_vote_period: Option<u64> = None;
    let mut previous_salt_string: Option<String> = None;
    let mut salt_string = gen_salt();

    loop {
        let hour_loop: DateTime<Utc> = Utc::now().checked_add_signed(Duration::hours(1)).unwrap();
        // TODO refresh things here like keys

        while hour_loop > Utc::now() {
            let now = SystemTime::now();

            if terra.debug {
                log::debug!("Refreshing Oracle Parameters");
            }
            let oracle_params = terra.oracle().parameters().await?;
            if last_vote_period.is_none()
                || oracle_params.result.vote_period > last_vote_period.unwrap_or(0)
            {
                let mut denoms_wanted_set: HashSet<String> = HashSet::new();

                let denoms_wanted = oracle_params.result.whitelist;
                for currency_pair in denoms_wanted.iter() {
                    let conv = u_currency_to_real(&currency_pair.name)?;
                    denoms_wanted_set.insert(String::from(conv));
                }

                if terra.debug {
                    log::debug!(
                        "Validator wants following currencies - {}",
                        serde_json::to_string(&denoms_wanted_set)?
                    );
                }
                for wanted in &denoms_wanted_set {
                    if !denoms_set.contains(wanted.as_str()) {
                        return Err(format!("Missing currency {}", wanted).into());
                    }
                }
                //  denoms_wanted_set.insert(String::from("xyz"));

                sleep(std::time::Duration::from_millis(20));

                match do_vote(
                    &secp,
                    &terra,
                    &price_server,
                    &private_key,
                    &cli.validator,
                    &denoms_wanted_set,
                    previous_salt_string.clone(),
                    hex::encode(salt_string),
                )
                .await
                {
                    Ok(result) => {
                        if result {
                            last_vote_period = Some(oracle_params.result.vote_period);
                            previous_salt_string = Some(hex::encode(salt_string));
                            salt_string = gen_salt();
                        }
                    }
                    Err(ref e) => {
                        eprintln!("Vote Failed {}", e);
                        log::error!("Vote Failed {}", e)
                    }
                }
            } else {
                log::debug!("vote period of {} ", last_vote_period.unwrap_or(0))
            }
            let end = SystemTime::now();
            let diff = end.duration_since(now)?;
            if terra.debug {
                log::debug!("Feed Submission took {:#?}", diff);
            }
            if diff.as_millis() < 5000 {
                let sleep_time = std::time::Duration::from_millis(5000) - diff;
                if terra.debug {
                    log::debug!("Sleeping for {:#?}", sleep_time);
                }
                sleep(sleep_time);
            }
        }
    }
}
// TODO implement a terra-rust-wallet-keyring package for this stuff
macro_rules! key_format {
    () => {
        "TERRA-RUST-{}-{}"
    };
}
pub fn get_private_key(
    secp: &Secp256k1<All>,
    wallet: &str,
    name: &str,
    seed: Option<&str>,
) -> Result<PrivateKey> {
    let keyname = format!(key_format!(), wallet, name);
    let keyring = keyring::Keyring::new(&wallet, &keyname);
    let phrase = keyring.get_password()?;

    match seed {
        None => Ok(PrivateKey::from_words(secp, &phrase)?),
        Some(seed_str) => Ok(PrivateKey::from_words_seed(secp, &phrase, seed_str)?),
    }
}

#[tokio::main]
async fn main() {
    env_logger::init();
    dotenv().ok();
    if let Err(ref e) = run().await {
        error!("error: {}", e);

        //  $env:RUST_LOG="output_log=info"
        for e in e.iter().skip(1) {
            info!("caused by: {}", e);
        }

        // The backtrace is not always generated. Try to run this example
        // with `$env:RUST_BACKTRACE=1`.
        if let Some(backtrace) = e.backtrace() {
            debug!("backtrace: {:?}", backtrace);
        }

        ::std::process::exit(1);
    }
}
