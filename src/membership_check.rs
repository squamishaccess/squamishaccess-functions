use http_types::auth::BasicAuth;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tide::{Response, StatusCode};

// The info! logging macro comes from crate::azure_function::logger
use crate::azure_function::{AzureFnLogger, AzureFnLoggerExt};
use crate::AppRequest;

#[derive(Debug, Deserialize, Serialize)]
struct MailchimpResponse {
    status: String,
    email_address: String,
    merge_fields: Value,
}

#[derive(Debug, Deserialize)]
struct MailchimpErrorResponse {
    title: String,
}

/// Check if an email is in MailChimp & when it's expiry date is, if available.
pub async fn membership_check(mut req: AppRequest) -> tide::Result<Response> {
    let mut logger = req
        .ext_mut::<AzureFnLogger>()
        .expect("Must install AzureFnMiddleware")
        .clone();

    #[derive(Debug, Deserialize)]
    struct Incoming {
        email: String,
    }

    let Incoming { email } = req.body_json().await?;

    info!(logger, "Membership check - Email: {}", email);

    // Must be done after we take the main request body.
    //
    // An atomic reference-counted pointer to our application state, with shared http clients.
    let state = req.state();

    // The MailChimp api is a bit strange.
    let hash = md5::compute(&email.to_lowercase());
    let authz = BasicAuth::new("any", &state.mc_api_key);

    // Attempt to fetch the member to our MailChimp list.
    let mc_path = format!("3.0/lists/{}/members/{:x}", state.mc_list_id, hash);
    let mut mailchimp_res = state
        .mailchimp
        .get(&mc_path)
        .header(authz.name(), authz.value())
        .await?;

    match mailchimp_res.status() {
        StatusCode::Ok => {
            let mc_json: MailchimpResponse = mailchimp_res.body_json().await?;

            let membership = if mc_json.status == "pending" || mc_json.status == "subscribed" {
                "active"
            } else {
                "expired"
            };

            let body = json!({
                "membership": membership,
                "expiration": mc_json.merge_fields.get("EXPIRES")
            });

            Ok(Response::builder(StatusCode::Ok).body(body).into())
        }
        StatusCode::NotFound => {
            info!(logger, "No such member: {}", email);

            Ok(Response::builder(StatusCode::NotFound)
                .body("No such member")
                .into())
        }
        s if s.is_client_error() => {
            info!(
                logger,
                "Client error: {} - {}",
                s,
                mailchimp_res.body_string().await?
            );
            Ok(Response::builder(StatusCode::InternalServerError)
                .body("Internal Server Error: mailchimp client error")
                .into())
        }
        s => {
            // Something else?
            info!(
                logger,
                "Unknown status: {} - {}",
                s,
                mailchimp_res.body_string().await?
            );
            Ok(Response::builder(StatusCode::InternalServerError)
                .body("Internal Server Error: unknown mailchimp status code")
                .into())
        }
    }
}
