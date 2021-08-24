use http_types::headers::LOCATION;
use serde::Deserialize;
use serde_json::json;
use tide::{Response, StatusCode};

// The info! logging macro comes from crate::azure_function::logger
use crate::azure_function::{AzureFnLogger, AzureFnLoggerExt};
use crate::{AppRequest, MailchimpQuery, MailchimpResponse};

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

    let Incoming { email } = req.body_form().await?;

    if email.is_empty() {
        let mut res: Response = StatusCode::SeeOther.into();
        res.insert_header(LOCATION, "https://squamishaccess.ca/membership");
        return Ok(res);
    }

    info!(logger, "Membership check - Email: {}", email);

    // Must be done after we take the main request body.
    //
    // An atomic reference-counted pointer to our application state, with shared http clients.
    let state = req.state();

    // The MailChimp api is a bit strange.
    let hash = md5::compute(&email.to_lowercase());

    let mc_query = MailchimpQuery {
        fields: &["FNAME", "EXPIRES"],
    };

    // Attempt to fetch the member to our MailChimp list.
    let mc_path = format!("3.0/lists/{}/members/{:x}", state.mc_list_id, hash);
    let mut mailchimp_res = state.mailchimp.get(&mc_path).query(&mc_query)?.await?;

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
                    }],
                    "dynamic_template_data": {
                        "member_name": mc_json.merge_fields.first_name,
                        "expires": mc_json.merge_fields.expires,
                        "status": membership
                    }
                }],
                "from": {
                    "email": "noreply@squamishaccess.ca"
                },
                "to": [{
                    "email": mc_json.email_address
                }],
                "template_id": state.template_membership_check
            });

            let mut twilio_res = state.twilio.post("v3/mail/send").body(body).await?;

            if twilio_res.status() == StatusCode::Accepted {
                let mut res: Response = StatusCode::SeeOther.into();
                res.insert_header(
                    LOCATION,
                    "https://squamishaccess.ca/membership-check-response",
                );
                Ok(res)
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
                    "email": "noreply@squamishaccess.ca"
                },
                "to": [{
                    "email": email
                }],
                "template_id": state.template_membership_notfound
            });

            let mut twilio_res = state.twilio.post("v3/mail/send").body(body).await?;

            if twilio_res.status() == StatusCode::Accepted {
                let mut res: Response = StatusCode::SeeOther.into();
                res.insert_header(
                    LOCATION,
                    "https://squamishaccess.ca/membership-check-response",
                );
                Ok(res)
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
