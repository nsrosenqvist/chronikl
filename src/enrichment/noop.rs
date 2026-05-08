//! No-op enricher — used when no PR-fetching backend is available
//! (non-GitHub CI, missing token, or `--no-pr-enrichment`).

use async_trait::async_trait;

use crate::enrichment::{EnrichError, EnrichOutcome, PrEnricher};
use crate::models::EnrichedCommit;

#[derive(Debug, Default)]
pub struct NoOpEnricher;

#[async_trait]
impl PrEnricher for NoOpEnricher {
    async fn enrich(&self, _commits: &mut [EnrichedCommit]) -> Result<EnrichOutcome, EnrichError> {
        Ok(EnrichOutcome::default())
    }

    fn name(&self) -> &str {
        "no-op"
    }
}
