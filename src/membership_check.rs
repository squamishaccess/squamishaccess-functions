use http_types::auth::{AuthenticationScheme, Authorization, BasicAuth};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tide::{Response, StatusCode};

// The info! logging macro comes from crate::azure_function::logger
use crate::azure_function::{AzureFnLogger, AzureFnLoggerExt};
use crate::AppRequest;

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

    let mc_query = MailchimpQuery {
        fields: &["FNAME", "EXPIRES"],
    };

    // Attempt to fetch the member to our MailChimp list.
    let mc_path = format!("3.0/lists/{}/members/{:x}", state.mc_list_id, hash);
    let mut mailchimp_res = state
        .mailchimp
        .get(&mc_path)
        .query(&mc_query)?
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
                "personalizations": [{
                    "to": [{
                        "email": mc_json.email_address
                    }]
                }],
                "from": {
                    "email": "info@squamishaccess.ca"
                },
                "to": [{
                    "email": mc_json.email_address
                }],
                "template_id": state.template_membership_check,
                "dynamic_template_data": {
                    "first_name": mc_json.merge_fields.first_name,
                    "expires": mc_json.merge_fields.expires,
                    "status": membership
                }
            });

            let authz =
                Authorization::new(AuthenticationScheme::Bearer, state.twilio_api_key.clone());

            let mut twilio_res = state
                .twilio
                .post("v3/mail/send")
                .header(authz.name(), authz.value())
                .body(body)
                .await?;

            if twilio_res.status() == StatusCode::Accepted {
                Ok(StatusCode::Accepted.into())
            } else {
                info!(logger, "Twilio error: {}", twilio_res.body_string().await?);
                Ok(StatusCode::InternalServerError.into())
            }
        }
        StatusCode::NotFound => {
            info!(logger, "No such member: {}", email);

            let body = json!({
                "personalizations": [{
                    "to": [{
                        "email": email
                    }]
                }],
                "from": {
                    "email": "info@squamishaccess.ca"
                },
                "to": [{
                    "email": email
                }],
                "template_id": state.template_membership_notfound
            });

            let authz =
                Authorization::new(AuthenticationScheme::Bearer, state.twilio_api_key.clone());

            let mut twilio_res = state
                .twilio
                .post("v3/mail/send")
                .header(authz.name(), authz.value())
                .body(body)
                .await?;

            if twilio_res.status() == StatusCode::Accepted {
                Ok(StatusCode::Accepted.into())
            } else {
                info!(logger, "Twilio error: {}", twilio_res.body_string().await?);
                Ok(StatusCode::InternalServerError.into())
            }
        }
        s if s.is_client_error() => {
            info!(
                logger,
                "Mailchimp client error: {} - {}",
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
                "Mailchimp unknown status: {} - {}",
                s,
                mailchimp_res.body_string().await?
            );
            Ok(Response::builder(StatusCode::InternalServerError)
                .body("Internal Server Error: unknown mailchimp status code")
                .into())
        }
    }
}
