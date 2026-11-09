//! Per-tool storage in Application Support, behind `k.store` in the app (scope `native.store`).

use super::super::Gateway;
use axum::Router;
use std::sync::Arc;

pub fn router() -> Router<Arc<Gateway>> {
    Router::new()
}

pub fn capabilities(_gw: &Gateway) -> Vec<String> {
    vec![]
}
