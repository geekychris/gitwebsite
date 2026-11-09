//! The app itself: window title / edited dot / menus, notifications, menu-bar items and the tool
//! palette, through the app's [`super::host::NativeHost`] (scope `native.app`).

use super::super::Gateway;
use axum::Router;
use std::sync::Arc;

pub fn router() -> Router<Arc<Gateway>> {
    Router::new()
}

pub fn capabilities(_gw: &Gateway) -> Vec<String> {
    vec![]
}
