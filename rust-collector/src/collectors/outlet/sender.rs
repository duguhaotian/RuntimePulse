//! Central and local report sender.

use reqwest::blocking::Client;

use crate::collectors::core::config::CollectorConfig;
use crate::collectors::core::error::{CollectorError, Result};
use crate::collectors::core::model::{IngestBatch, PluginOutput};

pub fn send_batch(client: &Client, config: &CollectorConfig, batch: &IngestBatch) -> Result<()> {
    let response = client.post(&config.ingest_url).json(batch).send()?;
    let status = response.status();
    let body = response.text().unwrap_or_default();

    if !status.is_success() {
        return Err(CollectorError::Ingest {
            status: status.as_u16(),
            body,
        });
    }

    Ok(())
}

pub fn send_local_report(client: &Client, url: &str, output: &PluginOutput) -> Result<()> {
    let response = client.post(url).json(output).send()?;
    let status = response.status();
    let body = response.text().unwrap_or_default();

    if !status.is_success() {
        return Err(CollectorError::Ingest {
            status: status.as_u16(),
            body,
        });
    }

    Ok(())
}
