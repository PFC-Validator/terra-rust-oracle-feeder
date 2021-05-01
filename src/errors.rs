use error_chain::error_chain;
#[cfg(test)]
impl PartialEq for Error {
    fn eq(&self, _other: &Self) -> bool {
        false
    }
}
error_chain! {
    foreign_links {
        TerraError ( terra_rust_api::errors::Error);
        KeyringError(keyring::KeyringError);
        Secp256k1(bitcoin::secp256k1::Error);
        SerdeJson(serde_json::Error);
        ParseInt(std::num::ParseIntError);
        ReqwestError(::reqwest::Error);
        SystemTimeError(std::time::SystemTimeError);

    }
    errors {
        PriceServer(err:String) {
            description("Price Server Error")
            display("Price Server Error: {}" ,err)
        }
        AccountError(err:String) {
            description("Account details error")
            display("Account Details error: {}" ,err)
        }
        OracleWhiteList {
            description("Expected whitelist to contain currencies prefixed with 'u'")
            display("Expected whitelist to contain currencies prefixed with 'u'")
        }
    }
}
