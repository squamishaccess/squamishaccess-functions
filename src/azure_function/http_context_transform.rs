use std::sync::Arc;

use async_std::sync::RwLock;
use log::info;
use serde_json::{json, Value};
use tide::http::headers::CONTENT_TYPE;
use tide::{Body, Middleware, Next, Request, Result};

use super::AzureFnLoggerInner;

/// Middleware for non-forwarding Azure Functions
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
    async fn transform<'a, State: Clone + Send + Sync + 'static>(
        &'a self,
        mut req: Request<State>,
        next: Next<'a, State>,
    ) -> Result {
        if req.ext::<AzureFnMiddlewareHasBeenRun>().is_some() {
            return Ok(next.run(req).await);
        }
        req.set_ext(AzureFnMiddlewareHasBeenRun);

        let req_data = req.body_string().await?;

        info!("req_data: {}", req_data);

        // let azure_function_payload: Value = req.body_json().await?;
        let azure_function_payload: Value = serde_json::from_str(&req_data)?;
        let mut invocation_id = "(id missing)".to_string();
        if let Some(val) = req.header("X-Azure-Functions-InvocationId") {
            invocation_id = val.last().as_str().to_string();
        }

        let logger = AzureFnLoggerInner {
            logs: vec![],
            invocation_id,
        };
        let logger = Arc::new(RwLock::new(logger));

        req.set_ext(logger.clone());

        if let Some(external_req_body) = azure_function_payload.pointer("/Data/req/Body") {
            info!("external_req_body: {}", external_req_body);
            if let Value::String(body) = external_req_body {
                req.set_body(Body::from_string(body.to_owned()));
            }
        }

        let mut res = next.run(req).await;

        let logger = Arc::try_unwrap(logger).unwrap();

        let out = json!({
            "ReturnValue": res.take_body().into_string().await?,
            "Logs": logger.into_inner().logs,
        });

        res.set_body(Body::from_json(&out)?);
        res.remove_header(CONTENT_TYPE);
        res.insert_header(CONTENT_TYPE, tide::http::mime::JSON);

        Ok(res)
    }
}

#[tide::utils::async_trait]
impl<State: Clone + Send + Sync + 'static> Middleware<State> for AzureFnMiddleware {
    async fn handle(&self, req: Request<State>, next: Next<'_, State>) -> Result {
        self.transform(req, next).await
    }
}
