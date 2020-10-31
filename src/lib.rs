#![forbid(unsafe_code, future_incompatible)]
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
use surf::Client;
use tide::{Request, Response, Server, StatusCode};

pub mod azure_function;
mod ipn_handler;

use ipn_handler::ipn_handler;

#[derive(Debug)]
pub struct AppState {
    pub mailchimp: Client,
    pub paypal: Client,
    pub mc_api_key: String,
    pub mc_list_id: String,
    pub paypal_sandbox: bool,
}

pub type AppRequest = Request<Arc<AppState>>;

async fn get_ping(_req: AppRequest) -> tide::Result<Response> {
    Ok(StatusCode::Ok.into())
}

pub fn setup_routes(server: &mut Server<Arc<AppState>>) {
    server.at("/").get(get_ping);
    server.at("/Paypal-IPN").post(ipn_handler);
}