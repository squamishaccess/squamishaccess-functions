#![forbid(unsafe_code)]
#![deny(future_incompatible)]
#![warn(
    meta_variable_misuse,
    missing_debug_implementations,
    noop_method_call,
    rust_2018_idioms,
    trivial_casts,
    unused_lifetimes,
    unused_qualifications,
    unused_macro_rules,
    variant_size_differences
)]
#![doc(test(attr(deny(future_incompatible, rust_2018_idioms, warnings))))]
#![doc(test(attr(allow(unused_extern_crates, unused_variables))))]
#![deny(
    clippy::allow_attributes_without_reason,
    clippy::default_union_representation,
    clippy::exit,
    clippy::lossy_float_literal,
    clippy::mem_forget,
    clippy::multiple_inherent_impl,
    clippy::mut_mut,
    clippy::ptr_as_ptr,
    clippy::unwrap_in_result,
    clippy::unwrap_used,
    clippy::wildcard_dependencies
)]
#![warn(
    clippy::dbg_macro,
    clippy::empty_drop,
    clippy::fallible_impl_from,
    clippy::inefficient_to_string,
    clippy::macro_use_imports,
    clippy::match_same_arms,
    clippy::no_effect_underscore_binding,
    clippy::panic,
    clippy::print_stderr,
    clippy::print_stdout,
    clippy::same_name_method,
    clippy::single_char_lifetime_names,
    clippy::string_to_string,
    clippy::trait_duplication_in_bounds,
    clippy::type_repetition_in_bounds,
    clippy::unimplemented,
    clippy::unneeded_field_pattern,
    clippy::unseparated_literal_suffix,
    clippy::used_underscore_binding
)]

use std::convert::TryInto;
use std::env;
use std::sync::Arc;

use color_eyre::eyre::Result;
use http_types::auth::{AuthenticationScheme, Authorization, BasicAuth};
use log::{info, warn};
use surf::{Client, Config, Url};

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
    let paypal_base_url = if paypal_sandbox {
        warn!("SANDBOX: Using PayPal sandbox environment");
        Url::parse("https://ipnpb.sandbox.paypal.com/")?
    } else {
        Url::parse("https://ipnpb.paypal.com/")?
    };

    // Set up re-useable api clients for efficiency & ergonomics.
    let client_config = Config::new().set_http_keep_alive(false);
    let mailchimp: Client = client_config
        .clone()
        .set_base_url(mc_base_url)
        .add_header(mc_auth.name(), mc_auth.value())
        .expect("Provided MailChimp auth must be valid")
        .try_into()?;
    let twilio: Client = client_config
        .clone()
        .set_base_url(Url::parse("https://api.sendgrid.com/")?)
        .add_header(twilio_auth.name(), twilio_auth.value())
        .expect("Provided Twilio auth must be valid")
        .try_into()?;
    let paypal: Client = client_config.set_base_url(paypal_base_url).try_into()?;

    // Application shared state.
    // This is set behind an atomic reference counted pointer.
    let state = AppState {
        mailchimp,
        mc_list_id,
        paypal,
        paypal_sandbox,
        template_membership_check,
        template_membership_notfound,
        twilio,
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
