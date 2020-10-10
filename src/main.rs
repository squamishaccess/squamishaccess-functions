use std::env;

use chrono::prelude::*;
use chrono::Duration;
use color_eyre::eyre::Result;
use http_types::auth::BasicAuth;
use serde::{Deserialize, Serialize};
use tide::http::Method;
use tide::{Request, Response, StatusCode};
use tracing::{info, warn};

#[derive(Debug, Deserialize, PartialEq, Serialize)]
struct IPNTransationMessage {
    txn_id: String,
    payment_status: String,
    payer_email: String,
    first_name: String,
    last_name: String,
}

#[derive(Deserialize, Serialize)]
#[allow(non_snake_case)]
struct MergeFields {
    FNAME: String,
    LNAME: String,
    JOINED: String,
    EXPIRES: String,
}

#[derive(Deserialize, Serialize)]
struct MailchimpRequest {
    email_address: String,
    merge_fields: MergeFields,
    status: String,
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

const PRODUCTION_VERIFY_URI: &str = "https://ipnpb.paypal.com/cgi/webscr";
const SANDBOX_VERIFY_URI: &str = "https://ipnpb.sandbox.paypal.com/cgi-bin/webscr";

async fn handler(mut req: Request<()>) -> tide::Result<Response> {
    let api_key = env::var("MAILCHIMP_API_KEY").unwrap();
    let list_id = env::var("MAILCHIMP_LIST_ID").unwrap();
    let base_url = format!(
        "https://{0}.api.mailchimp.com/3.0",
        api_key.split('-').nth(1).unwrap()
    );

    let sandbox = env::var("PAYPAL_SANDBOX").is_ok();

    if req.method() != Method::Post {
        warn!("Request method was not allowed. Was: {}", req.method());
        return Ok(Response::builder(StatusCode::MethodNotAllowed)
            .body(StatusCode::MethodNotAllowed.to_string())
            .into());
    }
    info!("PayPal IPN Notification Event received successfully.");

    if sandbox {
        warn!("SANDBOX: Using PayPal sandbox environment");
    }
    let paypal_verify_uri = if sandbox {
        SANDBOX_VERIFY_URI
    } else {
        PRODUCTION_VERIFY_URI
    };

    let ipn_transaction_message_raw = req.body_string().await?;
    let verification_body = ["cmd=_notify-validate&", &ipn_transaction_message_raw].concat();

    let mut paypal_res: surf::Response = surf::post(paypal_verify_uri)
        .body(verification_body)
        .await?;

    let ipn_transaction_message: IPNTransationMessage;
    match serde_qs::from_str::<IPNTransationMessage>(&ipn_transaction_message_raw) {
        Ok(msg) => {
            ipn_transaction_message = msg;
        }
        Err(error) => {
            return Err(tide::Error::from_str(
                StatusCode::InternalServerError,
                error.to_string(),
            ))
        }
    }

    let verify_response: String = paypal_res.body_string().await?;
    match verify_response.as_str() {
        "VERIFIED" => info!(
            "Verified IPN: IPN message for Transaction ID: {} is verified",
            ipn_transaction_message.txn_id
        ),
        "INVALID" => {
            info!(
                "Invalid IPN: IPN message for Transaction ID: {} is invalid",
                ipn_transaction_message.txn_id
            );
            return Ok(StatusCode::InternalServerError.into());
        }
        s => {
            info!("Invalid IPN: Unexpected IPN verify response body: {}", s);
            return Ok(StatusCode::InternalServerError.into());
        }
    }

    if ipn_transaction_message.payment_status != "Completed" {
        info!(
            "IPN: Payment status was not \"Completed\": {}",
            ipn_transaction_message.payment_status
        );
        return Ok(StatusCode::InternalServerError.into());
    }

    info!("Mailchimp: {}", ipn_transaction_message.payer_email);

    let utc_now: DateTime<Utc> = Utc::now();
    let utc_expires: DateTime<Utc> = Utc::now() + Duration::days(365 * 5 + 1);

    let json_struct = MailchimpRequest {
        email_address: ipn_transaction_message.payer_email.clone(),
        merge_fields: MergeFields {
            FNAME: ipn_transaction_message.first_name,
            LNAME: ipn_transaction_message.last_name,
            JOINED: utc_now.to_rfc3339(),
            EXPIRES: utc_expires.to_rfc3339(),
        },
        status: "pending".to_string(),
    };

    let url = format!("{0}/lists/{1}/members", base_url, list_id);

    let authz = BasicAuth::new("any", api_key);

    let mut mailchimp_res: surf::Response = surf::post(url.as_str())
        .header(authz.name(), authz.value())
        .body(surf::Body::from_json(&json_struct)?)
        .await?;

    if mailchimp_res.status().is_client_error() || mailchimp_res.status().is_server_error() {
        let error_body = mailchimp_res.body_string().await?;

        let maybe_json = serde_json::from_str::<MailchimpErrorResponse>(error_body.as_str());
        let was_json = maybe_json.is_ok();
        let json = maybe_json.unwrap_or_default();

        warn!(
            "Mailchimp error: {} -- {}",
            mailchimp_res.status(),
            error_body
        );

        if was_json
            && json
                .detail
                .contains(ipn_transaction_message.payer_email.as_str())
        {
            Ok(Response::builder(StatusCode::Ok)
                .body(StatusCode::Ok.to_string())
                .into())
        } else {
            Ok(Response::builder(mailchimp_res.status())
                .body(error_body)
                .into())
        }
    } else {
        let mc_json: MailchimpResponse = mailchimp_res.body_json().await?;
        info!(
            "Mailchimp: successfully set subscription status \"{0}\" for: {1}",
            mc_json.status, mc_json.email_address
        );
        Ok(Response::builder(StatusCode::Ok)
            .body(StatusCode::Ok.to_string())
            .into())
    }
}

#[async_std::main]
async fn main() -> Result<()> {
    let port_key = "FUNCTIONS_CUSTOMHANDLER_PORT";
    let port: u16 = match env::var(port_key) {
        Ok(val) => val.parse().expect("Custom Handler port is not a number!"),
        Err(_) => 3000,
    };
    let addr = "127.0.0.1";

    let mut server = tide::new();

    server.at("/api/SimpleHttpTrigger").get(handler);

    println!("Listening on http://{}:{}", addr, port);
    server.listen((addr, port)).await.map_err(Into::into)
}
