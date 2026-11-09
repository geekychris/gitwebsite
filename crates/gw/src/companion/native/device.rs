//! Devices: serial ports, Bluetooth LE, USB, HID and MIDI for the Web Serial / Web Bluetooth /
//! WebUSB / WebHID / Web MIDI shims the app injects into tool windows (scope `native.device`).

use super::super::Gateway;
use axum::Router;
use std::sync::Arc;

pub fn router() -> Router<Arc<Gateway>> {
    Router::new()
}

pub fn capabilities(_gw: &Gateway) -> Vec<String> {
    vec![]
}
