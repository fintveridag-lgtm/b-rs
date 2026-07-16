pub mod ibkr;
pub mod multi;
pub mod paper;
pub mod revolutx;

use crate::types::{Order, OrderRequest, Position};
use anyhow::Result;

/// Megler-abstraksjonen. Alt engine og UI vet om en megler går gjennom
/// denne traiten — nye meglere (f.eks. Nordnet den dagen API-et åpner,
/// eller Saxo) legges til som nye implementasjoner uten å røre resten.
#[async_trait::async_trait]
pub trait Broker: Send + Sync {
    fn name(&self) -> &'static str;

    async fn place_order(&self, req: OrderRequest) -> Result<Order>;

    /// Kanseller alle åpne ordrer (kill switch).
    async fn cancel_all(&self) -> Result<()>;

    async fn positions(&self) -> Result<Vec<Position>>;

    async fn cash(&self) -> Result<f64>;

    /// Ny kurs observert — papirmegleren bruker dette til å markere posisjoner.
    async fn on_quote(&self, _symbol: &str, _price: f64) {}

    /// Kontoene bak megleren som (navn, kontanter, valuta) — én per
    /// undermegler for multi. Tom liste (standard) betyr «én konto,
    /// bruk cash()» — da slipper enkle meglere et ekstra API-kall.
    async fn accounts(&self) -> Vec<(String, f64, String)> {
        Vec::new()
    }
}
