use std::sync::Arc;

use async_std::sync::RwLock;
use serde_json::{json, Map, Value};
use tide::http::headers::CONTENT_TYPE;
use tide::{Body, Middleware, Next, Request, Result, StatusCode};

use super::AzureFnLoggerInner;

/// Middleware for non-forwarding Azure Functions
///
/// This is required in order to make logging work with azure funtion custom handlers.
///
/// This middleware re-writes the request to and from specialized json structures to interface with azure.
///
/// This middleware requires that azure `function.json` be set up like so.
/// In particular, the naming of `req` & `res` MUST be the same.
/// ```json
/// {
///     "bindings": [
///         {
///             "name": "req",
///             "type": "httpTrigger",
///             "direction": "in",
///             "methods": [
///                 "list http methods here"
///             ]
///         },
///         {
///             "name": "res",
///             "type": "http",
///             "direction": "out"
///         }
///     ]
/// }
/// ```
#[derive(Clone, Debug, Default)]
pub struct AzureFnMiddleware {
    _priv: (),
}

struct AzureFnMiddlewareHasBeenRun;

impl AzureFnMiddleware {
    /// Create a new instance of `AzureFnMiddleware`.
    #[must_use]
    pub fn new() -> Self {
        Self { _priv: () }
    }

    /// Log a request and a response.
    async fn transform<'mw, State: Clone + Send + Sync + 'static>(
        &'mw self,
        mut req: Request<State>,
        next: Next<'mw, State>,
    ) -> Result {
        if req.ext::<AzureFnMiddlewareHasBeenRun>().is_some() {
            return Ok(next.run(req).await);
        }
        req.set_ext(AzureFnMiddlewareHasBeenRun);

        let mut invocation_id = "(id missing)".to_string();
        if let Some(val) = req.header("X-Azure-Functions-InvocationId") {
            invocation_id = val.last().as_str().to_string();
        }

        let mut logs = vec![];

        let azure_function_payload: Value = req.body_json().await?;
        if let Some(external_req_body) = azure_function_payload.pointer("/Data/req/Body") {
            if let Value::String(body) = external_req_body {
                // Re-write the request body to the extracted external request body.
                req.set_body(Body::from_string(body.to_owned()));
            } else {
                logs.push(
                    "AzureFnMiddleware Error: \"/Data/req/Body\" not a String, check function.json"
                        .to_string(),
                );
            }
        } else {
            logs.push(
                "AzureFnMiddleware Error: \"/Data/req/Body\" not found, check function.json"
                    .to_string(),
            );
        }

        let logger = AzureFnLoggerInner {
            logs,
            invocation_id,
        };
        let logger = Arc::new(RwLock::new(logger));
        req.set_ext(logger.clone());

        let mut res = next.run(req).await; // Continue middleware stack.

        let logger =
            Arc::try_unwrap(logger).expect("Logger not being free here is a fundimental logic bug");

        // Transform our headers into an iterator of JSON key/value pairs, and then construct a JSON object from it.
        let headers_iter = res.iter().map(|(name, values)| {
            (
                name.as_str().to_owned(),
                Value::String(
                    values
                        .iter()
                        .map(|v| v.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                ),
            )
        });

        let out = json!({
            "Outputs": {
                "res": {
                    // The external response status code.
                    "statusCode": res.status(),
                    // Headers, now constructing the JSON object from the iterator.
                    "headers": headers_iter.collect::<Map<_, _>>(),
                    // Encapsulate the external response.
                    "body": res.take_body().into_string().await?
                }
            },
            // This is currently the only way to log from a custom handler.
            "Logs": logger.into_inner().logs,
        });

        res.set_body(Body::from_json(&out)?);
        res.remove_header(CONTENT_TYPE);
        res.insert_header(CONTENT_TYPE, tide::http::mime::JSON);

        // Azure only likes status code 200, and logs get dropped if it is anything else.
        res.set_status(StatusCode::Ok);

        Ok(res)
    }
}

#[tide::utils::async_trait]
impl<State: Clone + Send + Sync + 'static> Middleware<State> for AzureFnMiddleware {
    async fn handle(&self, req: Request<State>, next: Next<'_, State>) -> Result {
        self.transform(req, next).await
    }
}
