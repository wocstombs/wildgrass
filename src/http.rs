//! HTTP client helpers with JSON parsing and error logging.

use reqwest::RequestBuilder;
use serde::de::DeserializeOwned;

use crate::log;

/// Dispatches `request` and deserializes the JSON response body.
/// Returns `None` on network, HTTP error status, or deserialization failures, logging errors under `label`.
pub async fn get_json<T: DeserializeOwned>(request: RequestBuilder, label: &str) -> Option<T> {
    let answer = async { request.send().await?.error_for_status()?.json().await };
    match answer.await {
        Ok(parsed) => Some(parsed),
        Err(error) => {
            log::error(format!("{label}: {error}"));
            None
        }
    }
}
