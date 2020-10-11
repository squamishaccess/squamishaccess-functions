use std::env;
use std::sync::Arc;

use chrono::prelude::*;
use chrono::Duration;
use chrono::SecondsFormat::Secs;
use color_eyre::eyre::Result;
use http_types::auth::BasicAuth;
use serde::{Deserialize, Serialize};
use surf::{Client, Url};
use tide::http::Method;
use tide::{Body, Request, Response, StatusCode};
use tracing::{debug, error, info, warn};

mod logger;
use logger::LogMiddleware;

const PRODUCTION_VERIFY_URL: &str = "https://ipnpb.paypal.com/";
const SANDBOX_VERIFY_URL: &str = "https://ipnpb.sandbox.paypal.com/";

struct State {
    mailchimp: Client,
    paypal: Client,
    mc_api_key: String,
    mc_list_id: String,
    paypal_verify: &'static str,
}

#[derive(Debug, Deserialize, PartialEq, Serialize)]
struct IPNTransationMessage {
    txn_id: String,
    txn_type: String,
    payment_status: String,
    payer_email: String,
    first_name: String,
    last_name: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[allow(non_snake_case)]
struct MergeFields {
    FNAME: String,
    LNAME: String,
    JOINED: String,
    EXPIRES: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct MailchimpRequest<'email> {
    email_address: &'email str,
    merge_fields: MergeFields,
    status: &'static str,
}

#[derive(Deserialize, Serialize)]
struct MailchimpResponse {
    status: String,
    email_address: String,
}

#[derive(Default, Deserialize, Serialize)]
struct MailchimpErrorResponse {
    detail: String,
}

async fn handler(mut req: Request<Arc<State>>) -> tide::Result<Response> {
    if req.method() != Method::Post {
        warn!("Request method was not allowed. Was: {}", req.method());
        return Ok(Response::builder(StatusCode::MethodNotAllowed)
            .body(StatusCode::MethodNotAllowed.to_string())
            .into());
    }
    info!("PayPal IPN Notification Event received successfully.");

    let ipn_transaction_message_raw = req.body_string().await?;
    let verification_body = ["cmd=_notify-validate&", &ipn_transaction_message_raw].concat();

    // Must be done after we take the main request body.
    let state = req.state();

    let verify_response = state
        .paypal
        .post(state.paypal_verify)
        .body(verification_body)
        .recv_string()
        .await?;

    let ipn_transaction_message: IPNTransationMessage;
    match serde_qs::from_str(&ipn_transaction_message_raw) {
        Ok(msg) => {
            ipn_transaction_message = msg;
        }
        Err(error) => {
            error!(
                "Invalid IPN: unparseable IPN: {}",
                ipn_transaction_message_raw
            );
            return Err(tide::Error::from_str(
                StatusCode::InternalServerError,
                error.to_string(),
            ));
        }
    }

    match verify_response.as_str() {
        "VERIFIED" => info!(
            "Verified IPN: IPN message for Transaction ID \"{}\" is verified",
            ipn_transaction_message.txn_id
        ),
        "INVALID" => {
            error!(
                "Invalid IPN: IPN message for Transaction ID \"{}\" is invalid",
                ipn_transaction_message.txn_id
            );
            return Ok(StatusCode::InternalServerError.into());
        }
        s => {
            error!("Invalid IPN: Unexpected IPN verify response body: {}", s);
            return Ok(StatusCode::InternalServerError.into());
        }
    }

    if ipn_transaction_message.payment_status != "Completed" {
        info!(
            "IPN: Payment status was not \"Completed\": {}",
            ipn_transaction_message.payment_status
        );
        return Ok(StatusCode::Ok.into());
    }

    match ipn_transaction_message.txn_type.as_str() {
        "web_accept" => (),        // Ok
        "subscr_payment" => (),    // TODO: check amount
        "send_money" => (),        // TODO: check amount
        "recurring_payment" => (), // TODO: check amount
        _ => {
            error!(
                "IPN: Payment status was not \"Completed\": {}",
                ipn_transaction_message.payment_status
            );
            return Ok(StatusCode::InternalServerError.into());
        }
    }

    info!("Email: {}", ipn_transaction_message.payer_email);

    let utc_now: DateTime<Utc> = Utc::now();
    let utc_expires: DateTime<Utc> = Utc::now() + Duration::days(365 * 5 + 1);

    let mc_req = MailchimpRequest {
        email_address: &ipn_transaction_message.payer_email,
        merge_fields: MergeFields {
            FNAME: ipn_transaction_message.first_name,
            LNAME: ipn_transaction_message.last_name,
            JOINED: utc_now.to_rfc3339_opts(Secs, true),
            EXPIRES: utc_expires.to_rfc3339_opts(Secs, true),
        },
        status: "pending",
    };

    debug!("{:?}", mc_req);

    let mc_path = format!("3.0/lists/{}/members", state.mc_list_id);
    let authz = BasicAuth::new("any", &state.mc_api_key);
    let mut mailchimp_res = state
        .mailchimp
        .post(&mc_path)
        .header(authz.name(), authz.value())
        .body(Body::from_json(&mc_req)?)
        .await?;

    if mailchimp_res.status().is_client_error() || mailchimp_res.status().is_server_error() {
        let error_body = mailchimp_res.body_string().await?;

        let maybe_json = serde_json::from_str::<MailchimpErrorResponse>(error_body.as_str());

        warn!(
            "Mailchimp error: {} -- {}",
            mailchimp_res.status(),
            error_body
        );

        if maybe_json.is_ok()
            && maybe_json
                .unwrap()
                .detail
                .contains(ipn_transaction_message.payer_email.as_str())
        {
            // Assume the user exists in mailchimp or was permenantly removed.
            // This is to deal with PayPal IPN retry nonsense.
            // You are not supposted to retry with the same information for
            // error code 400 but PayPal doesn't care and retries continuously.
            Ok(StatusCode::Ok.into())
        } else {
            Ok(Response::builder(mailchimp_res.status())
                .body(error_body)
                .into())
        }
    } else {
        let mc_json: MailchimpResponse = mailchimp_res.body_json().await?;
        if mc_json.status == "pending" || mc_json.status == "subscribed" {
            info!(
                "Mailchimp: successfully set subscription status \"{}\" for: {}",
                mc_json.status, mc_json.email_address
            );
            Ok(StatusCode::Ok.into())
        } else {
            warn!(
                "Mailchimp: unsuccessful result: {}",
                serde_json::to_string(&mc_json)?
            );
            Ok(StatusCode::InternalServerError.into())
        }
    }
}

#[async_std::main]
async fn main() -> Result<()> {
    color_eyre::install()?;
    dotenv::dotenv().ok();

    let log_level: femme::LevelFilter = env::var("LOGLEVEL")
        .map(|v| v.parse().expect("LOGLEVEL must be a valid log level."))
        .unwrap_or(femme::LevelFilter::Info);
    femme::with_level(log_level);
    info!("Logger started - level: {}", log_level);

    let mc_api_key = env::var("MAILCHIMP_API_KEY").expect("MAILCHIMP_API_KEY is required.");
    let mc_list_id = env::var("MAILCHIMP_LIST_ID").expect("MAILCHIMP_LIST_ID is required.");
    let mc_base_url = Url::parse(&format!(
        "https://{}.api.mailchimp.com",
        mc_api_key
            .split('-')
            .nth(1)
            .expect("Requires a valid, full mailchimp api key")
    ))?;

    let paypal_base_url;
    let paypal_verify;
    if env::var("PAYPAL_SANDBOX").is_ok() {
        warn!("SANDBOX: Using PayPal sandbox environment");
        paypal_base_url = Url::parse(SANDBOX_VERIFY_URL)?;
        paypal_verify = "cgi-bin/webscr";
    } else {
        paypal_base_url = Url::parse(PRODUCTION_VERIFY_URL)?;
        paypal_verify = "cgi/webscr";
    };

    let mut mailchimp = surf::client();
    mailchimp.set_base_url(mc_base_url);
    let mut paypal = surf::client();
    paypal.set_base_url(paypal_base_url);

    let state = State {
        mailchimp,
        paypal,
        mc_api_key,
        mc_list_id,
        paypal_verify,
    };

    let mut server = tide::with_state(Arc::new(state));
    server.with(LogMiddleware::new());
    server.at("/api/SimpleHttpTrigger").post(handler);

    let port: u16 = env::var("FUNCTIONS_CUSTOMHANDLER_PORT")
        .map(|v| {
            v.parse()
                .expect("FUNCTIONS_CUSTOMHANDLER_PORT must be a number.")
        })
        .unwrap_or(80);
    let host = env::var("HOST").unwrap_or_else(|_| "127.0.0.1".to_string());

    server
        .listen((host.as_str(), port))
        .await
        .map_err(Into::into)
}
