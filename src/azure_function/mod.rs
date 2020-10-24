use std::sync::Arc;

use async_std::sync::RwLock;

mod http_context_transform;
mod logger;

pub use http_context_transform::AzureFnMiddleware;
pub use logger::LogMiddleware;

pub type AzureFnLogger = Arc<RwLock<AzureFnLoggerInner>>;

#[derive(Debug)]
pub struct AzureFnLoggerInner {
    logs: Vec<String>,
    invocation_id: String,
}

#[tide::utils::async_trait]
pub trait AzureFnLoggerExt {
    async fn log(&mut self, log_line: String);
}

#[must_use = "requires await"]
#[tide::utils::async_trait]
impl AzureFnLoggerExt for AzureFnLogger {
    async fn log(&mut self, log_line: String) {
        let mut inner = self.write().await;
        let line = format!("{} {}", inner.invocation_id, log_line);
        inner.logs.push(line);
    }
}

#[must_use = "requires await"]
#[tide::utils::async_trait]
impl AzureFnLoggerExt for &'_ mut AzureFnLogger {
    async fn log(&mut self, log_line: String) {
        let mut inner = self.write().await;
        let line = format!("{} {}", inner.invocation_id, log_line);
        inner.logs.push(line);
    }
}
