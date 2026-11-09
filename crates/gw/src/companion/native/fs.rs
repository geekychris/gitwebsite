//! Files: native open/save panels and reading/writing the files the user chose (scope `native.fs`).

use super::super::Gateway;
use axum::Router;
use std::sync::Arc;

pub fn router() -> Router<Arc<Gateway>> {
    Router::new()
}

pub fn capabilities(_gw: &Gateway) -> Vec<String> {
    vec![]
}
