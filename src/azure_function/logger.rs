use tide::{Middleware, Next, Request, Result};

use super::{AzureFnLogger, AzureFnLoggerExt};

/// Log all incoming requests and responses.
///
/// This middleware is enabled by default in Tide. In the case of
/// nested applications, this middleware will only run once for each
/// request.
///
/// # Examples
///
/// ```
/// let mut app = tide::Server::new();
/// app.with(tide::log::LogMiddleware::new());
/// ```
#[derive(Debug, Default, Clone)]
pub struct LogMiddleware {
    _priv: (),
}

struct LogMiddlewareHasBeenRun;

impl LogMiddleware {
    /// Create a new instance of `LogMiddleware`.
    #[must_use]
    pub fn new() -> Self {
        Self { _priv: () }
    }

    /// Log a request and a response.
    async fn log<'a, State: Clone + Send + Sync + 'static>(
        &'a self,
        mut req: Request<State>,
        next: Next<'a, State>,
    ) -> Result {
        if req.ext::<LogMiddlewareHasBeenRun>().is_some() {
            return Ok(next.run(req).await);
        }
        req.set_ext(LogMiddlewareHasBeenRun);

        let mut logger = req
            .ext_mut::<AzureFnLogger>()
            .expect("Must install AzureFnMiddleware")
            .clone();

        let start = std::time::Instant::now();
        let response = next.run(req).await;
        let status = response.status();

        if status.is_server_error() {
            if let Some(error) = response.error() {
                logger.log(format!("Internal error. message: {:?}, error_type: {:?}, status: {}, duration: {:?}",
                    error,
                    error.type_name(),
                    format!("{} - {}", status as u16, status.canonical_reason()),
                    start.elapsed(),
                )).await;
            } else {
                logger
                    .log(format!(
                        "Internal error. status: {}, duration: {:?}",
                        format!("{} - {}", status as u16, status.canonical_reason()),
                        start.elapsed(),
                    ))
                    .await;
            }
        } else if status.is_client_error() {
            if let Some(error) = response.error() {
                logger
                    .log(format!(
                        "Client error. message: {:?}, error_type: {:?}, status: {}, duration: {:?}",
                        error,
                        error.type_name(),
                        format!("{} - {}", status as u16, status.canonical_reason()),
                        start.elapsed(),
                    ))
                    .await;
            } else {
                logger
                    .log(format!(
                        "Client error. status: {}, duration: {:?}",
                        format!("{} - {}", status as u16, status.canonical_reason()),
                        start.elapsed(),
                    ))
                    .await;
            }
        }
        Ok(response)
    }
}

#[tide::utils::async_trait]
impl<State: Clone + Send + Sync + 'static> Middleware<State> for LogMiddleware {
    async fn handle(&self, req: Request<State>, next: Next<'_, State>) -> Result {
        self.log(req, next).await
    }
}
