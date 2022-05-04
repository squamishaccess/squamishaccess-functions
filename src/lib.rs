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

use std::sync::Arc;

use log::warn;
use serde::{Deserialize, Serialize};
use surf::Client;
use tide::{Request, Response, Server, StatusCode};

#[macro_use]
pub mod azure_function;

// Our functions
mod ipn_handler;
mod membership_check;

use ipn_handler::ipn_handler;
use membership_check::membership_check;

#[derive(Debug)]
pub struct AppState {
    pub mailchimp: Client,
    pub mc_list_id: String,
    pub paypal: Client,
    pub paypal_sandbox: bool,
    pub template_membership_check: String, // twilio email template id
    pub template_membership_notfound: String, // twilio email template id
    pub twilio: Client,                    // Email sending
}

pub type AppRequest = Request<Arc<AppState>>;

async fn get_ping(_req: AppRequest) -> tide::Result<Response> {
    Ok(StatusCode::Ok.into())
}

pub fn setup_routes(server: &mut Server<Arc<AppState>>) {
    // Required so that Azure known when our custom handler is listening, _I think_.
    server.at("/").get(get_ping);

    // The PayPal IPN handler, set the path where it's `function.json` sits in the project.
    server.at("/Paypal-IPN").post(ipn_handler);

    // The Membership Check handler, set the path where it's `function.json` sits in the project.
    server.at("/Membership-Check").post(membership_check);
}

#[derive(Debug, Serialize)]
struct MailchimpQuery {
    fields: &'static [&'static str],
}

#[derive(Debug, Deserialize, Serialize)]
struct McMergeFields {
    #[serde(rename = "FNAME")]
    first_name: String,
    #[serde(rename = "EXPIRES")]
    expires: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct MailchimpResponse {
    status: String,
    email_address: String,
    merge_fields: McMergeFields,
}
