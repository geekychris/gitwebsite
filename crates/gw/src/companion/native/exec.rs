//! System commands: a fixed catalogue (launchctl, defaults, plutil, sfltool …), each with checked
//! arguments; anything that changes the system asks first (scope `native.exec`).

use super::super::Gateway;
use axum::Router;
use std::sync::Arc;

pub fn router() -> Router<Arc<Gateway>> {
    Router::new()
}

pub fn capabilities(_gw: &Gateway) -> Vec<String> {
    vec![]
}
