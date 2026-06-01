use async_trait::async_trait;
use serde::Deserialize;

use super::{Embedding, EmbeddingProvider};
use crate::error::{MemoryError, Result};

/// OpenAI `/v1/embeddings` compatible provider.
/// Works with Azure OpenAI, local LiteLLM proxies, and any other
/// service that exposes the same endpoint shape.
pub struct OpenAiEmbedding {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
    dimensions: usize,
}

impl OpenAiEmbedding {
    pub fn new(api_key: &str, model: &str, base_url: Option<&str>) -> Result<Self> {
        let base_url = base_url
            .filter(|u| !u.is_empty())
            .unwrap_or("https://api.openai.com")
            .trim_end_matches('/')
            .to_string();

        let dimensions = match model {
            "text-embedding-3-small" => 1536,
            "text-embedding-3-large" => 3072,
            "text-embedding-ada-002" => 1536,
            _ => {
                // Unknown model — guess from common dimensionalities.
                // The first embed call will tell us the real value; this
                // is just a pre-flight estimate.
                1536
            }
        };

        Ok(Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .map_err(|e| MemoryError::Other(format!("OpenAI client init: {e}")))?,
            base_url,
            api_key: api_key.to_string(),
            model: model.to_string(),
            dimensions,
        })
    }
}

#[derive(Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingData>,
}

#[derive(Deserialize)]
struct EmbeddingData {
    embedding: Vec<f32>,
}

#[async_trait]
impl EmbeddingProvider for OpenAiEmbedding {
    async fn embed(&self, text: &str) -> Result<Embedding> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Ok(Embedding {
                vector: vec![0.0_f32; self.dimensions],
            });
        }

        let body = serde_json::json!({
            "model": self.model,
            "input": trimmed,
        });

        let resp = self
            .client
            .post(format!("{}/v1/embeddings", self.base_url))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| MemoryError::Other(format!("OpenAI embed request: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            // Truncate to prevent API key or verbose proxy errors from
            // leaking into logs / error messages returned to callers.
            let summary: String = body.chars().take(200).collect();
            return Err(MemoryError::Other(format!(
                "OpenAI embed error {status}: {summary}"
            )));
        }

        let data: EmbeddingResponse = resp
            .json()
            .await
            .map_err(|e| MemoryError::Other(format!("OpenAI embed parse: {e}")))?;

        let vector = data
            .data
            .into_iter()
            .next()
            .map(|d| d.embedding)
            .unwrap_or_else(|| vec![0.0_f32; self.dimensions]);

        Ok(Embedding { vector })
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }
}
