use chrono::prelude::*;
use chrono::Duration;
use chrono::SecondsFormat::Secs;
use serde::Deserialize;
use serde_json::json;
use tide::http::Method;
use tide::{Body, Response, StatusCode};

// The info! logging macro comes from crate::azure_function::logger
use crate::azure_function::{AzureFnLogger, AzureFnLoggerExt};
use crate::{AppRequest, MailchimpQuery, MailchimpResponse};

#[allow(clippy::upper_case_acronyms)]
#[derive(Debug, Deserialize)]
struct IPNTransationMessage {
    txn_id: String,
    txn_type: String,
    payment_status: String,
    payer_email: String,
    first_name: String,
    last_name: String,
    mc_currency: String,
    mc_gross: String,
    exchange_rate: Option<String>,
    payment_date: Option<String>,
}

#[allow(clippy::upper_case_acronyms)]
#[derive(Debug, Deserialize)]
struct IPNMessageTypeOnly {
    txn_type: Option<String>,
}

/// Handle a PayPal Instant Payment Notification (IPN) and attempt to subscribe to MailChimp.
pub async fn ipn_handler(mut req: AppRequest) -> tide::Result<Response> {
    let mut logger = req
        .ext_mut::<AzureFnLogger>()
        .expect("Must install AzureFnMiddleware")
        .clone();

    if req.method() != Method::Post {
        info!(
            logger,
            "Request method was not allowed. Was: {}",
            req.method()
        );
        return Err(tide::Error::from_str(
            StatusCode::MethodNotAllowed,
            StatusCode::MethodNotAllowed.to_string(),
        ));
    }
    info!(
        logger,
        "PayPal IPN Notification Event received successfully."
    );

    let ipn_transaction_message_raw = req.body_string().await?;
    let verification_body = ["cmd=_notify-validate&", &ipn_transaction_message_raw].concat();

    // Must be done after we take the main request body.
    //
    // An atomic reference-counted pointer to our application state, with shared http clients.
    let state = req.state();

    if state.paypal_sandbox {
        info!(logger, "SANDBOX: Using PayPal sandbox environment");
    }

    // Verify the IPN with PayPal. PayPal requires this.
    let mut verify_response = state
        .paypal
        .post("/cgi-bin/webscr")
        .body(verification_body)
        .await?;

    if !verify_response.status().is_success() {
        return Err(tide::Error::from_str(
            StatusCode::InternalServerError,
            format!(
                "PayPal IPN verification failed - status: {}",
                verify_response.status()
            ),
        ));
    }

    // Check just the `txn_type` of the IPN message.
    let txn_type = match serde_qs::from_str::<IPNMessageTypeOnly>(&ipn_transaction_message_raw) {
        Ok(msg) => msg.txn_type,
        Err(error) => {
            return Err(tide::Error::from_str(
                StatusCode::InternalServerError,
                format!(
                    "Invalid IPN: unparseable IPN: \"{}\" - error: {}",
                    ipn_transaction_message_raw, error
                ),
            ));
        }
    };

    // PayPal buttons - we accept yearly subscriptions ("subscr_payment") and one-off yearly payments ("web_accept").
    match txn_type.as_deref() {
        Some("web_accept") => (),     // Ok
        Some("subscr_payment") => (), // Ok
        Some(txn_type) => {
            return Err(tide::Error::from_str(
                StatusCode::Ok, // Don't want PayPal to retry.
                format!("IPN: txn_type was not acceptable: {}", txn_type),
            ));
        }
        None => {
            return Err(tide::Error::from_str(
                StatusCode::Ok, // Don't want PayPal to retry.
                format!(
                    "IPN: no transaction type. IPN: \"{}\"",
                    ipn_transaction_message_raw
                ),
            ));
        }
    }

    // Attempt to deserialize the IPN message.
    let ipn_transaction_message: IPNTransationMessage;
    match serde_qs::from_str(&ipn_transaction_message_raw) {
        Ok(msg) => {
            ipn_transaction_message = msg;
        }
        Err(error) => {
            return Err(tide::Error::from_str(
                StatusCode::InternalServerError,
                format!(
                    "Invalid IPN: unparseable IPN: \"{}\" - error: {}",
                    ipn_transaction_message_raw, error
                ),
            ));
        }
    }

    if let Some(payment_date) = ipn_transaction_message.payment_date {
        info!(logger, "Payment Timestamp: {}", payment_date);
    }

    // Check the result of IPN verification.
    let verify_status = verify_response.body_string().await?;
    match verify_status.as_str() {
        "VERIFIED" => {
            info!(
                logger,
                "Verified IPN: IPN message for Transaction ID \"{}\" is verified",
                ipn_transaction_message.txn_id
            );
        }
        "INVALID" => {
            return Err(tide::Error::from_str(
                StatusCode::InternalServerError,
                format!(
                    "Invalid IPN: IPN message for Transaction ID \"{}\" is invalid. IPN: \"{}\"",
                    ipn_transaction_message.txn_id, ipn_transaction_message_raw
                ),
            ));
        }
        unknown => {
            return Err(tide::Error::from_str(
                StatusCode::InternalServerError,
                format!(
                    "Invalid IPN: Unexpected IPN verify response body: \"{}\" - IPN: {}",
                    unknown, ipn_transaction_message_raw
                ),
            ));
        }
    }

    // Anything that isn't "Completed" we don't care about.
    //
    // Usually this means a "Completed" IPN will be sent later from a pending transaction.
    if ipn_transaction_message.payment_status != "Completed" {
        info!(
            logger,
            "IPN: Payment status was not \"Completed\": {}", ipn_transaction_message.payment_status
        );
        return Ok(StatusCode::Ok.into());
    }

    // Temporary: figure out why kind of payment values PayPal is actually giving us, as the docs are unclear.
    info!(
        logger,
        "IPN: type: \"{}\" - gross amount: {} - currency: {} - exchange rate: {}",
        ipn_transaction_message.txn_type,
        ipn_transaction_message.mc_gross,
        ipn_transaction_message.mc_currency,
        ipn_transaction_message
            .exchange_rate
            .as_deref()
            .unwrap_or("(none)"),
    );

    let payment_amount: f64 = ipn_transaction_message.mc_gross.parse()?;
    if payment_amount < 10.0 {
        info!(logger, "Refusing membership, payment amount too low.",);
        return Ok(StatusCode::Ok.into());
    }

    info!(logger, "Email: {}", ipn_transaction_message.payer_email);

    // The MailChimp api is a bit strange.
    let hash = md5::compute(&ipn_transaction_message.payer_email.to_lowercase());

    let mc_query = MailchimpQuery {
        fields: &["EXPIRES"],
    };

    // Check if the person is already in our MailChimp list.
    let mc_path = format!("3.0/lists/{}/members/{:x}", state.mc_list_id, hash);
    let mut mailchimp_res = state
        .mailchimp
        .get(&mc_path)
        .query(&mc_query)?
        .header(state.mc_auth.name(), state.mc_auth.value())
        .await?;

    if mailchimp_res.status().is_server_error() {
        let error_body = mailchimp_res.body_string().await?;

        return Err(tide::Error::from_str(
            mailchimp_res.status(),
            format!("Mailchimp GET: error body: \"{}\"", error_body),
        ));
    }

    let utc_now: DateTime<Utc> = Utc::now();
    let mut utc_expires: DateTime<Utc> = Utc::now() + Duration::days(365);

    let status;
    if mailchimp_res.status().is_client_error() {
        // If the person is not in our list, set them as pending to give them an opportunity to properly accept if they want an email subscription.
        status = "pending";
    } else {
        let mc_json: MailchimpResponse = mailchimp_res.body_json().await?;
        info!(
            logger,
            "Mailchimp existing status: {}",
            mc_json.status.as_str(),
        );
        status = match mc_json.status.as_str() {
            // Don't re-subscribe someone who has unsubscribed from our emails. They will still be a list member regardless.
            "unsubscribed" => return Ok(StatusCode::Ok.into()),
            "subscribed" => "subscribed",
            _ => "pending",
        };

        let existing_expire_day =
            NaiveDate::parse_from_str(&mc_json.merge_fields.expires, "%Y-%m-%d")?;
        let existing_expire = existing_expire_day.and_hms(12, 0, 0);
        let existing_expire = DateTime::from_utc(existing_expire, Utc);
        if existing_expire > utc_expires {
            info!(
                logger,
                "existing EXPIRES is beyond one year, using it: {}", mc_json.merge_fields.expires
            );
            utc_expires = existing_expire;
        }
    };

    // Set up the new member's MailChimp information.
    let mc_req = json!({
        "email_address": &ipn_transaction_message.payer_email,
        "merge_fields": {
            "FNAME": ipn_transaction_message.first_name,
            "LNAME": ipn_transaction_message.last_name,
            "JOINED": utc_now.to_rfc3339_opts(Secs, true),
            "EXPIRES": utc_expires.to_rfc3339_opts(Secs, true),
        },
        "status": status,
    });

    // Add the new member to our MailChimp list.
    let mc_path = format!("3.0/lists/{}/members/{:x}", state.mc_list_id, hash);
    let mut mailchimp_res = state
        .mailchimp
        .put(&mc_path)
        .header(state.mc_auth.name(), state.mc_auth.value())
        .body(Body::from_json(&mc_req)?)
        .await?;

    if !mailchimp_res.status().is_success() {
        let error_body = mailchimp_res.body_string().await?;

        Err(tide::Error::from_str(
            mailchimp_res.status(),
            format!("Mailchimp error: {}", error_body),
        ))
    } else {
        let mc_json: MailchimpResponse = mailchimp_res.body_json().await?;
        if mc_json.status == "pending" || mc_json.status == "subscribed" {
            info!(
                logger,
                "Mailchimp: successfully set subscription status \"{}\" for: {}",
                mc_json.status,
                mc_json.email_address
            );
            Ok(StatusCode::Ok.into())
        } else {
            Err(tide::Error::from_str(
                StatusCode::InternalServerError,
                format!(
                    "Mailchimp: unsuccessful result: {}",
                    serde_json::to_string(&mc_json)?
                ),
            ))
        }
    }
}
