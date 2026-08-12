//! Minimal fake Soft-AP HTTP device contract.

/// An inbound HTTP request presented to a [`FakeDevice`].
pub struct FakeRequest<'a> {
    /// HTTP method (e.g. `"GET"`).
    pub method: &'a str,
    /// Request path including query string (e.g. `/index.html?loco=1`).
    pub path: &'a str,
    /// Optional request body.
    pub body: Option<&'a [u8]>,
}

/// An outbound HTTP response from a [`FakeDevice`].
pub struct FakeResponse {
    /// HTTP status code.
    pub status: u16,
    /// `Content-Type` header value.
    pub content_type: &'static str,
    /// Response body bytes.
    pub body: Vec<u8>,
}

/// Build a `200` text/plain response.
#[must_use]
pub fn ok_text(body: impl Into<String>) -> FakeResponse {
    FakeResponse {
        status: 200,
        content_type: "text/plain",
        body: body.into().into_bytes(),
    }
}

/// Build a `200` text/xml (or HTML-compatible) response.
#[must_use]
pub fn ok_xml(body: impl Into<Vec<u8>>) -> FakeResponse {
    FakeResponse {
        status: 200,
        content_type: "text/html",
        body: body.into(),
    }
}

/// Build a `200` application/json response.
#[must_use]
pub fn ok_json(body: impl Into<Vec<u8>>) -> FakeResponse {
    FakeResponse {
        status: 200,
        content_type: "application/json",
        body: body.into(),
    }
}

/// Build a `404` text/plain response.
#[must_use]
pub fn not_found() -> FakeResponse {
    FakeResponse {
        status: 404,
        content_type: "text/plain",
        body: b"not found".to_vec(),
    }
}

/// A mock Soft-AP HTTP device.
pub trait FakeDevice: Send {
    /// Stable driver id string (`"wifred"`, `"longfred"`, …).
    fn driver_id(&self) -> &'static str;

    /// Handle one HTTP request.
    fn handle(&mut self, req: FakeRequest<'_>) -> FakeResponse;
}
