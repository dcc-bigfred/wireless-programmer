//! Fake radio and Soft-AP HTTP device mocks for wireless-programmer tests.

#![forbid(unsafe_code)]

mod composite;
mod device;
mod longfred;
mod radio;
mod server;
mod wifred;

pub use composite::CompositeFakeDevice;
pub use device::{not_found, ok_json, ok_text, ok_xml, FakeDevice, FakeRequest, FakeResponse};
pub use longfred::LongFredFake;
pub use radio::FakeRadio;
pub use server::{bind_and_serve, FakeHttpServer};
pub use wifred::WifredFake;
