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
                "key": state.mandrill_key,
                "template_name": state.template_membership_check,
                "template_content": [],
                "message": {
                    "to": [{
                        "email": mc_json.email_address
                    }]
                },
                "global_merge_vars": [{
                    "name": "STATUS",
                    "content": membership,
                }]
            });

            let mandrill_path = format!("api/1.0/messages/send-template");
            let mut mandrill_res = state.mandrill.post(&mandrill_path).body(body).await?;
            let status = mandrill_res.status();

            let res_body = mandrill_res.body_string().await?;
            info!(logger, "Mandrill response: {}", res_body);

            let res_body: Value = serde_json::from_str(&res_body)?;
            if res_body.pointer("0/reject_reason").is_some() {
                Ok(StatusCode::BadRequest.into())
            } else if status.is_client_error() || status.is_server_error() {
                Ok(StatusCode::InternalServerError.into())
            } else {
                Ok(StatusCode::Ok.into())
            }
        }
        StatusCode::NotFound => {
            info!(logger, "No such member: {}", email);

            let body = json!({
                "key": state.mandrill_key,
                "template_name": state.template_membership_notfound,
                "template_content": [],
                "message": {
                    "to": [{
                        "email": email
                    }]
                },
            });

            let mandrill_path = format!("api/1.0/messages/send-template");
            let mut mandrill_res = state.mandrill.post(&mandrill_path).body(body).await?;
            let status = mandrill_res.status();

            let res_body = mandrill_res.body_string().await?;
            info!(logger, "Mandrill response: {}", res_body);

            let res_body: Value = serde_json::from_str(&res_body)?;
            if res_body.pointer("0/reject_reason").is_some() {
                Ok(StatusCode::BadRequest.into())
            } else if status.is_client_error() || status.is_server_error() {
                Ok(StatusCode::InternalServerError.into())
            } else {
                Ok(StatusCode::Ok.into())
            }
        }
        s if s.is_client_error() => {
            info!(
                logger,
                "Mailchimp (mandrill) client error: {} - {}",
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
                "Mailchimp (mandrill) unknown status: {} - {}",
                s,
                mailchimp_res.body_string().await?
            );
            Ok(Response::builder(StatusCode::InternalServerError)
                .body("Internal Server Error: unknown mailchimp status code")
                .into())
        }
    }
}
