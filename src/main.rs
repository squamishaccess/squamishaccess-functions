#![forbid(unsafe_code)]
#![deny(future_incompatible)]
#![warn(
    missing_debug_implementations,
    rust_2018_idioms,
    trivial_casts,
    unused_qualifications
)]
#![doc(test(attr(deny(rust_2018_idioms, warnings))))]
#![doc(test(attr(allow(unused_extern_crates, unused_variables))))]

use std::convert::TryFrom;
use std::env;
use std::sync::Arc;

use color_eyre::eyre::Result;
use http_client::{h1::H1Client, Config};
use http_types::auth::{AuthenticationScheme, Authorization, BasicAuth};
use log::{info, warn};
use surf::{Client, Url};

use lib::azure_function::{AzureFnLogMiddleware, AzureFnMiddleware};
use lib::AppState;
use squamishaccess_functions as lib;

/// The main function. The binary is initialized to here.
#[async_std::main]
async fn main() -> Result<()> {
    // Nicer error formatting for start-up errors.
    color_eyre::install()?;

    #[cfg(debug_assertions)] // Non-release mode.
    dotenv::dotenv().ok();

    let log_level: femme::LevelFilter = env::var("LOGLEVEL")
        .map(|v| v.parse().expect("LOGLEVEL must be a valid log level."))
        .unwrap_or(femme::LevelFilter::Info);
    femme::with_level(log_level);
    info!("Logger started - level: {}", log_level);

    // MailChimp
    let mc_api_key = env::var("MAILCHIMP_API_KEY").expect("MAILCHIMP_API_KEY is required.");
    let mc_list_id = env::var("MAILCHIMP_LIST_ID").expect("MAILCHIMP_LIST_ID is required.");
    let mc_base_url = Url::parse(&format!(
        "https://{}.api.mailchimp.com",
        mc_api_key
            .split('-')
            .nth(1)
            .expect("Requires a valid, full mailchimp api key")
    ))?;
    let mc_auth = BasicAuth::new("any", mc_api_key);

    // Twilio (email sends)
    let twilio_api_key = env::var("TWILIO_API_KEY").expect("TWILIO_API_KEY is required.");
    let twilio_auth = Authorization::new(AuthenticationScheme::Bearer, twilio_api_key);

    // Twilio email templates
    let template_membership_check =
        env::var("TEMPLATE_MEMBERSHIP_CHECK").expect("TEMPLATE_MEMBERSHIP_CHECK is required.");
    let template_membership_notfound = env::var("TEMPLATE_MEMBERSHIP_NOTFOUND")
        .expect("TEMPLATE_MEMBERSHIP_NOTFOUND is required.");

    // PayPal
    let paypal_sandbox = env::var("PAYPAL_SANDBOX").is_ok();
    let paypal_base_url;
    if paypal_sandbox {
        warn!("SANDBOX: Using PayPal sandbox environment");
        paypal_base_url = Url::parse("https://ipnpb.sandbox.paypal.com/")?;
    } else {
        paypal_base_url = Url::parse("https://ipnpb.paypal.com/")?;
    };

    // Set up re-useable api clients for efficiency & ergonomics.
    let client_config = Config::new().set_http_keep_alive(false);
    let mut mailchimp = Client::with_http_client(H1Client::try_from(client_config.clone())?);
    mailchimp.set_base_url(mc_base_url);
    let mut twilio = Client::with_http_client(H1Client::try_from(client_config.clone())?);
    twilio.set_base_url(Url::parse("https://api.sendgrid.com/")?);
    let mut paypal = Client::with_http_client(H1Client::try_from(client_config)?);
    paypal.set_base_url(paypal_base_url);

    // Application shared state.
    // This is set behind an atomic reference counted pointer.
    let state = AppState {
        mailchimp,
        mc_auth,
        mc_list_id,
        paypal,
        paypal_sandbox,
        template_membership_check,
        template_membership_notfound,
        twilio,
        twilio_auth,
    };

    let mut server = tide::with_state(Arc::new(state));
    server.with(AzureFnMiddleware::new());
    server.with(AzureFnLogMiddleware::new());

    lib::setup_routes(&mut server);

    let port: u16 = env::var("FUNCTIONS_CUSTOMHANDLER_PORT").map_or(80, |v| {
        v.parse()
            .expect("FUNCTIONS_CUSTOMHANDLER_PORT must be a number.")
    });
    let host = env::var("HOST").unwrap_or_else(|_| "127.0.0.1".to_string());

    server
        .listen((host.as_str(), port))
        .await
        .map_err(Into::into)
}
