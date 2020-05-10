use azure_functions::{
    bindings::{HttpRequest, HttpResponse},
    func,
    http::Status,
};
use chrono::Duration;
use chrono::prelude::*;
use fehler::*;
use log::{info, warn};
use serde::{Deserialize, Serialize};
use std::env;
use thiserror::Error;

extern crate serde_derive;
extern crate serde_qs as qs;

#[derive(Debug, PartialEq, Deserialize, Serialize)]
struct IPNTransationMessage {
    txn_id: String,
    payment_status: String,
    payer_email: String,
    first_name: String,
    last_name: String,
}

#[derive(Serialize, Deserialize)]
#[allow(non_snake_case)]
struct MergeFields {
    FNAME: String,
    LNAME: String,
    JOINED: String,
    EXPIRES: String,
}

#[derive(Serialize, Deserialize)]
struct MailchimpRequest {
    email_address: String,
    merge_fields: MergeFields,
    status: String,
}

#[derive(Serialize, Deserialize)]
struct MailchimpResponse {
    status: String,
    email_address: String,
}

#[derive(Serialize, Deserialize, Default)]
struct MailchimpErrorResponse {
    detail: String,
}

#[derive(Error, Debug)]
pub enum FunctionError {
    // #[error("Internal Server Error: {0}")]
    // Message(String),
    #[error("Internal Server Error")]
    Generic,
    #[error("QueryString decode error: '{0}' for: {1}")]
    QueryString(String, String)
}

const PRODUCTION_VERIFY_URI: &str = "https://ipnpb.paypal.com/cgi/webscr";
const SANDBOX_VERIFY_URI: &str = "https://ipnpb.sandbox.paypal.com/cgi-bin/webscr";

#[func]
pub async fn function(req: HttpRequest) -> HttpResponse {
    match errorable_function(req).await {
        Ok(response) => response,
        Err(error) => {
            info!("Internal error: {}", error);

            HttpResponse::build()
                .status(Status::InternalServerError)
                .body(Status::InternalServerError.to_string())
                .finish()
        },
    }
}

#[throws(anyhow::Error)]
async fn errorable_function(req: HttpRequest) -> HttpResponse {
    let api_key = env::var("MAILCHIMP_API_KEY").unwrap();
    let list_id = env::var("MAILCHIMP_LIST_ID").unwrap();
    let base_url = format!("https://{0}.api.mailchimp.com/3.0", api_key.split("-").nth(1).unwrap());

    let sandbox = env::var("PAYPAL_SANDBOX").is_ok();

    if req.method() != "POST" {
        info!("Request method was not allowed. Was: {}", req.method());
        return HttpResponse::build()
            .status(Status::MethodNotAllowed)
            .body(Status::MethodNotAllowed.to_string())
            .finish();
    }
    info!("PayPal IPN Notification Event received successfully.");

    if sandbox {
        info!("SANDBOX: Using PayPal sandbox environment");
    }
    let paypal_verify_uri = if sandbox { SANDBOX_VERIFY_URI } else { PRODUCTION_VERIFY_URI };

    let ipn_transaction_message_raw = req.body().to_string();
    let verification_body = ["cmd=_notify-validate&", &ipn_transaction_message_raw].concat();

    let paypal_client = reqwest::Client::new();
    let paypal_res: reqwest::Response = paypal_client.post(paypal_verify_uri)
        .body(verification_body)
        .send()
        .await?;

    let ipn_transaction_message: IPNTransationMessage;
    match qs::from_str::<IPNTransationMessage>(&ipn_transaction_message_raw) {
        Ok(msg) => {
            ipn_transaction_message = msg;
        }
        Err(error) => {
            throw!(FunctionError::QueryString(error.description().to_string(), ipn_transaction_message_raw.to_string()));
        }
    }

    let verify_response: String = paypal_res.text().await?;
    match verify_response.as_str() {
        "VERIFIED" => info!("Verified IPN: IPN message for Transaction ID: {} is verified", ipn_transaction_message.txn_id),
        "INVALID" => {
            info!("Invalid IPN: IPN message for Transaction ID: {} is invalid", ipn_transaction_message.txn_id);
            throw!(FunctionError::Generic);
        }
        s => {
            info!("Invalid IPN: Unexpected IPN verify response body: {}", s);
            throw!(FunctionError::Generic);
        }
    }

    if ipn_transaction_message.payment_status != "Completed" {
        info!("IPN: Payment status was not \"Completed\": {}", ipn_transaction_message.payment_status);
        throw!(FunctionError::Generic);
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

    let mailchimp_client = reqwest::Client::new();
    let mailchimp_res: reqwest::Response = mailchimp_client.post(url.as_str())
        .basic_auth("any", Some(api_key))
        .json(&json_struct)
        .send()
        .await?;

    match mailchimp_res.error_for_status_ref() {
        Ok(_) => {
            let mc_json: MailchimpResponse = mailchimp_res.json().await?;
            info!("Mailchimp: successfully set subscription status \"{0}\" for: {1}", mc_json.status, mc_json.email_address);
            return HttpResponse::build()
                .status(Status::Ok)
                .body(Status::Ok.to_string())
                .finish();
        },
        Err(error) => {
            let error_body = mailchimp_res.text().await?;

            let maybe_json = serde_json::from_str::<MailchimpErrorResponse>(error_body.as_str());
            let was_json = maybe_json.is_ok();
            let json = maybe_json.unwrap_or_default();
            let err_status = match error.status() {
                Some(status) => status.as_u16(),
                None => 500
            };

            warn!("Mailchimp error: {} -- {}", error, error_body);

            if was_json && json.detail.contains(ipn_transaction_message.payer_email.as_str()) {
                return HttpResponse::build()
                    .status(Status::Ok)
                    .body(Status::Ok.to_string())
                    .finish();
            } else if error.is_status() {
                return HttpResponse::build()
                    .status(Status::from(err_status))
                    .body(error.to_string())
                    .finish();
            } else {
                throw!(error);
            }
        },
    };
}
