use std::sync::Arc;

use async_std::sync::RwLock;
use serde::Serialize;
use serde_json::Value;
use tide::{Body, Middleware, Next, Request, Result};

use super::AzureFnLoggerInner;

/// Middleware for non-forwarding Azure Functions
#[derive(Clone, Debug, Default)]
pub struct AzureFnMiddleware {
    _priv: (),
}

struct AzureFnMiddlewareHasBeenRun;

#[derive(Debug, Serialize)]
#[allow(non_snake_case)]
struct AzureFnOutput {
    ReturnValue: String,
    Logs: Vec<String>,
}

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

        let azure_function_payload: Value = req.body_json().await?;
        let mut invocation_id = "(id missing)".to_string();
        if let Some(val) = azure_function_payload.pointer("Metadata/Id") {
            if let Value::String(id) = val {
                invocation_id = id.to_owned();
            }
        }

        let logger = AzureFnLoggerInner {
            logs: vec![],
            invocation_id,
        };
        let logger = Arc::new(RwLock::new(logger));

        req.set_ext(logger.clone());

        if let Some(external_req_data) = azure_function_payload.pointer("Data/req") {
            req.set_body(Body::from_json(external_req_data)?);
        }

        let mut res = next.run(req).await;

        let logger = Arc::try_unwrap(logger).unwrap();
        let mut out = AzureFnOutput {
            ReturnValue: String::new(),
            Logs: logger.into_inner().logs,
        };

        out.ReturnValue = res.take_body().into_string().await?;
        res.set_body(Body::from_json(&out)?);

        Ok(res)
    }
}

#[tide::utils::async_trait]
impl<State: Clone + Send + Sync + 'static> Middleware<State> for AzureFnMiddleware {
    async fn handle(&self, req: Request<State>, next: Next<'_, State>) -> Result {
        self.transform(req, next).await
    }
}
