use adp_core::AdpError;
use adp_server::error::ApiError;
use axum::http::StatusCode;
use axum::response::IntoResponse;

#[test]
fn nip98_protocol_errors_map_to_401() {
    let response = ApiError::Protocol(AdpError::Nip98("expired".into())).into_response();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[test]
fn internal_protocol_errors_map_to_500() {
    for error in [
        AdpError::Encryption("nip44 failed".into()),
        AdpError::Nostr("tag parse failed".into()),
        AdpError::Serde(serde_json::from_str::<serde_json::Value>("{").unwrap_err()),
    ] {
        let response = ApiError::Protocol(error).into_response();

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }
}

#[test]
fn malformed_protocol_errors_still_map_to_400() {
    let response = ApiError::Protocol(AdpError::MissingTag("d")).into_response();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}
