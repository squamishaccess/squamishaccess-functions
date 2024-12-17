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

use chrono::{Datelike, Local, NaiveDate, ParseError};
use chrono_tz::America::Vancouver;
use log::warn;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
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

trait MailchimpDateFormat {
    fn to_mailchimp_format(&self) -> String;
}

impl MailchimpDateFormat for NaiveDate {
    fn to_mailchimp_format(&self) -> String {
        self.format("%Y-%m-%d").to_string()
    }
}

pub fn parse_mailchimp_date(iso_date: &str) -> Result<NaiveDate, ParseError> {
    NaiveDate::parse_from_str(iso_date, "%Y-%m-%d")
}

pub fn today_ppt() -> NaiveDate {
    Local::now().with_timezone(&Vancouver).date_naive()
}

pub fn safe_add_year(date: NaiveDate, years: i32) -> NaiveDate {
    let target_year = date.year() + years;
    date.with_year(target_year)
        .or_else(|| NaiveDate::from_ymd_opt(target_year, 2, 28)) // Handle feb 29th edge case
        .expect("Failed to calculate a valid date")
}

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
