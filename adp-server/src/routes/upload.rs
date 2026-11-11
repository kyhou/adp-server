use super::auth::Authenticated;
use axum::extract::{Multipart, State};
use axum::Json;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::error::{ApiError, ApiResult};
use crate::relay::RelayLookup;
use crate::state::AppState;
use adp_core::AdpListing;

use super::game::parse_coordinate;

#[derive(Serialize)]
pub struct UploadResponse {
    game_coordinate: String,
    file_hash: String,
    download_url: String,
}

fn required_file_hash(listing: &AdpListing) -> ApiResult<&str> {
    listing
        .file_hash
        .as_deref()
        .ok_or_else(|| ApiError::BadRequest("listing is missing required file_hash tag".into()))
}

// Bound metadata separately from the advertised archive-byte limit.
const MAX_LISTING_BYTES: u64 = 1024 * 1024;

fn multipart_error(error: axum::extract::multipart::MultipartError) -> ApiError {
    if error.status() == axum::http::StatusCode::PAYLOAD_TOO_LARGE {
        ApiError::PayloadTooLarge(
            "multipart request exceeds upload limit plus metadata allowance".into(),
        )
    } else {
        ApiError::BadRequest(format!("malformed multipart body: {error}"))
    }
}

/// Limit parser buffering while reading multipart headers, independently of file size.
/// The stream is sliced so a large transport frame cannot consume the header budget.
pub struct UploadMultipart {
    inner: Multipart,
    header_bytes: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

#[async_trait::async_trait]
impl<S: Send + Sync> axum::extract::FromRequest<S> for UploadMultipart {
    type Rejection = ApiError;

    async fn from_request(
        request: axum::extract::Request,
        state: &S,
    ) -> Result<Self, Self::Rejection> {
        use futures_util::StreamExt;
        use std::sync::{
            atomic::{AtomicU64, Ordering},
            Arc,
        };
        let (parts, body) = request.into_parts();
        let header_bytes = Arc::new(AtomicU64::new(0));
        let counter = Arc::clone(&header_bytes);
        let stream = futures_util::stream::try_unfold(
            (body.into_data_stream(), axum::body::Bytes::new()),
            move |(mut stream, mut pending)| {
                let counter = Arc::clone(&counter);
                async move {
                    // Multer drains every immediately-ready frame before parsing. Yield
                    // between slices to bound read-ahead and let it consume each slice.
                    tokio::task::yield_now().await;
                    while pending.is_empty() {
                        match stream.next().await {
                            Some(Ok(bytes)) => pending = bytes,
                            Some(Err(error)) => return Err(std::io::Error::other(error)),
                            None => return Ok(None),
                        }
                    }
                    let chunk = pending.split_to(pending.len().min(16 * 1024));
                    let used = counter.load(Ordering::Relaxed);
                    if used != u64::MAX {
                        let next = used.saturating_add(chunk.len() as u64);
                        if next > 64 * 1024 {
                            return Err(std::io::Error::other("multipart headers exceed 64 KiB"));
                        }
                        counter.store(next, Ordering::Relaxed);
                    }
                    Ok(Some((chunk, (stream, pending))))
                }
            },
        );
        let request =
            axum::extract::Request::from_parts(parts, axum::body::Body::from_stream(stream));
        let inner = Multipart::from_request(request, state)
            .await
            .map_err(|error| ApiError::BadRequest(error.to_string()))?;
        Ok(Self {
            inner,
            header_bytes,
        })
    }
}

async fn receive_upload(
    mut multipart: UploadMultipart,
    files_dir: &str,
    max_upload_bytes: u64,
) -> ApiResult<(String, tempfile::TempPath, String)> {
    use tokio::io::AsyncWriteExt;

    tokio::fs::create_dir_all(files_dir)
        .await
        .map_err(|e| ApiError::Internal(format!("failed to create files dir: {e}")))?;
    let directory = files_dir.to_owned();
    let temporary = tokio::task::spawn_blocking(move || tempfile::NamedTempFile::new_in(directory))
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let (file, path) = temporary.into_parts();
    let mut file = tokio::fs::File::from_std(file);
    let mut listing = None;
    let mut received_file = false;
    let mut hasher = Sha256::new();
    loop {
        use std::sync::atomic::Ordering;
        multipart.header_bytes.store(0, Ordering::Relaxed);
        let Some(mut field) = multipart
            .inner
            .next_field()
            .await
            .map_err(multipart_error)?
        else {
            break;
        };
        multipart.header_bytes.store(u64::MAX, Ordering::Relaxed);
        match field.name() {
            Some("listing_event") if listing.is_none() => {
                let mut bytes = Vec::new();
                while let Some(chunk) = field.chunk().await.map_err(multipart_error)? {
                    if (bytes.len() as u64).saturating_add(chunk.len() as u64) > MAX_LISTING_BYTES {
                        return Err(ApiError::PayloadTooLarge(
                            "listing_event exceeds metadata limit".into(),
                        ));
                    }
                    bytes.extend_from_slice(&chunk);
                }
                listing = Some(
                    String::from_utf8(bytes)
                        .map_err(|_| ApiError::BadRequest("listing_event is not UTF-8".into()))?,
                );
            }
            Some("file") if !received_file => {
                received_file = true;
                let mut size = 0_u64;
                while let Some(chunk) = field.chunk().await.map_err(multipart_error)? {
                    size = size
                        .checked_add(chunk.len() as u64)
                        .ok_or_else(|| ApiError::PayloadTooLarge("upload size overflow".into()))?;
                    if size > max_upload_bytes {
                        return Err(ApiError::PayloadTooLarge(format!(
                            "file exceeds max_upload_bytes ({max_upload_bytes})"
                        )));
                    }
                    hasher.update(&chunk);
                    file.write_all(&chunk)
                        .await
                        .map_err(|e| ApiError::Internal(format!("failed to stage upload: {e}")))?;
                }
            }
            _ => {
                return Err(ApiError::BadRequest(
                    "unexpected or duplicate multipart field".into(),
                ))
            }
        }
    }
    let listing =
        listing.ok_or_else(|| ApiError::BadRequest("missing listing_event field".into()))?;
    if !received_file {
        return Err(ApiError::BadRequest("missing file field".into()));
    }
    file.flush()
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    drop(file);
    Ok((listing, path, hex::encode(hasher.finalize())))
}

#[cfg(test)]
mod streaming_tests {
    use super::*;
    use axum::{body::Body, extract::FromRequest, http::Request};

    async fn receive(
        bytes: usize,
        limit: u64,
        malformed: bool,
        directory: &str,
    ) -> ApiResult<(String, tempfile::TempPath, String)> {
        let mut body = b"--test\r\nContent-Disposition: form-data; name=\"listing_event\"\r\n\r\n{}\r\n--test\r\nContent-Disposition: form-data; name=\"file\"; filename=\"game.zip\"\r\n\r\n".to_vec();
        body.extend(vec![42; bytes]);
        if !malformed {
            body.extend_from_slice(b"\r\n--test--\r\n");
        }
        let request = Request::builder()
            .header("content-type", "multipart/form-data; boundary=test")
            .body(Body::from(body))
            .expect("request");
        let multipart = UploadMultipart::from_request(request, &())
            .await
            .expect("multipart");
        receive_upload(multipart, directory, limit).await
    }

    #[tokio::test]
    async fn streaming_upload_exact_limit_hash_and_cleanup() {
        let dir = tempfile::tempdir().expect("directory");
        let (_, path, hash) = receive(1024, 1024, false, dir.path().to_str().expect("path"))
            .await
            .expect("upload");
        assert_eq!(tokio::fs::read(&path).await.expect("file"), vec![42; 1024]);
        assert_eq!(hash, hex::encode(Sha256::digest(vec![42; 1024])));
        drop(path);
        assert_eq!(std::fs::read_dir(dir.path()).expect("directory").count(), 0);
    }

    #[tokio::test]
    async fn streaming_upload_over_limit_returns_413_and_cleans_up() {
        use axum::response::IntoResponse;
        let dir = tempfile::tempdir().expect("directory");
        let error = receive(1025, 1024, false, dir.path().to_str().expect("path"))
            .await
            .expect_err("oversized");
        assert_eq!(
            error.into_response().status(),
            axum::http::StatusCode::PAYLOAD_TOO_LARGE
        );
        assert_eq!(std::fs::read_dir(dir.path()).expect("directory").count(), 0);
    }

    #[tokio::test]
    async fn streaming_upload_truncated_body_cleans_up() {
        let dir = tempfile::tempdir().expect("directory");
        assert!(
            receive(1024, 2048, true, dir.path().to_str().expect("path"))
                .await
                .is_err()
        );
        assert_eq!(std::fs::read_dir(dir.path()).expect("directory").count(), 0);
    }

    #[tokio::test]
    async fn streaming_upload_rejects_unterminated_headers_with_bounded_read_ahead() {
        use futures_util::{stream, StreamExt};
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };
        for after_file in [false, true] {
            let dir = tempfile::tempdir().expect("directory");
            let mut prefix = if after_file {
                b"--test\r\nContent-Disposition: form-data; name=\"file\"\r\n\r\nfile bytes\r\n"
                    .to_vec()
            } else {
                Vec::new()
            };
            prefix.extend_from_slice(
                b"--test\r\nContent-Disposition: form-data; name=\"listing_event\"; filename=\"",
            );
            let consumed = Arc::new(AtomicUsize::new(0));
            let counter = Arc::clone(&consumed);
            let endless_header = stream::repeat(axum::body::Bytes::from(vec![b'a'; 16 * 1024]))
                .map(move |bytes| {
                    counter.fetch_add(bytes.len(), Ordering::Relaxed);
                    Ok::<_, std::io::Error>(bytes)
                });
            let body =
                stream::once(async { Ok::<_, std::io::Error>(axum::body::Bytes::from(prefix)) })
                    .chain(endless_header);
            let request = Request::builder()
                .header("content-type", "multipart/form-data; boundary=test")
                .body(Body::from_stream(body))
                .expect("request");
            let multipart = UploadMultipart::from_request(request, &())
                .await
                .expect("multipart");
            let result = tokio::time::timeout(
                std::time::Duration::from_secs(2),
                receive_upload(
                    multipart,
                    dir.path().to_str().expect("path"),
                    8 * 1024 * 1024 * 1024,
                ),
            )
            .await
            .expect("bounded rejection without EOF");
            assert!(result.is_err());
            assert!(consumed.load(Ordering::Relaxed) <= 128 * 1024);
            assert_eq!(std::fs::read_dir(dir.path()).expect("directory").count(), 0);
        }
    }

    #[tokio::test]
    async fn streaming_upload_cancellation_removes_staging() {
        use futures_util::{stream, StreamExt};
        let dir = tempfile::tempdir().expect("directory");
        let directory = dir.path().to_str().expect("path").to_owned();
        let prefix =
            b"--test\r\nContent-Disposition: form-data; name=\"file\"\r\n\r\npartial file bytes";
        let body = stream::once(async { Ok::<_, std::io::Error>(prefix.as_slice()) })
            .chain(stream::pending());
        let request = Request::builder()
            .header("content-type", "multipart/form-data; boundary=test")
            .body(Body::from_stream(body))
            .expect("request");
        let multipart = UploadMultipart::from_request(request, &())
            .await
            .expect("multipart");
        let task = tokio::spawn(async move { receive_upload(multipart, &directory, 1024).await });
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if std::fs::read_dir(dir.path())
                    .expect("directory")
                    .any(|entry| entry.expect("entry").metadata().expect("metadata").len() > 0)
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("staged bytes");
        task.abort();
        assert!(task.await.expect_err("cancelled").is_cancelled());
        assert_eq!(std::fs::read_dir(dir.path()).expect("directory").count(), 0);
    }
}

/// `POST /upload` — developer- or fulfillment-key-authenticated. See ADP-01.md.
pub async fn upload_handler(
    State(state): State<AppState>,
    Authenticated(uploader): Authenticated,
    multipart: UploadMultipart,
) -> ApiResult<Json<UploadResponse>> {
    let (listing_event_json, staged, computed_hash) = receive_upload(
        multipart,
        &state.config.files_dir,
        state.config.max_upload_bytes,
    )
    .await?;

    let listing_event: nostr_sdk::Event = serde_json::from_str(&listing_event_json)
        .map_err(|_| ApiError::BadRequest("malformed listing_event JSON".into()))?;
    if listing_event.verify().is_err() {
        return Err(ApiError::BadRequest(
            "invalid listing_event signature".into(),
        ));
    }

    let submitted_listing = AdpListing::from_event(listing_event)?;
    let coordinate = submitted_listing.coordinate();
    let (developer_pubkey, d_tag) = parse_coordinate(&coordinate)?;
    let listing_event = match state.relay.lookup_listing(developer_pubkey, &d_tag).await {
        RelayLookup::Found(event) => event,
        RelayLookup::ConfirmedAbsent { .. } => {
            return Err(ApiError::NotFound("listing not found on relays".into()))
        }
        RelayLookup::Unavailable { .. } => {
            return Err(ApiError::ServiceUnavailable(
                "listing lookup unavailable".into(),
            ))
        }
    };
    if listing_event.verify().is_err() {
        return Err(ApiError::BadRequest(
            "invalid relay listing signature".into(),
        ));
    }
    let listing = AdpListing::from_event(listing_event)?;
    if listing.coordinate() != coordinate {
        return Err(ApiError::Internal(
            "relay listing does not match the submitted game coordinate".into(),
        ));
    }

    let now = nostr_sdk::Timestamp::now().as_secs();
    if uploader != listing.developer_pubkey {
        let mut candidates = listing
            .fulfillment_authorizations
            .iter()
            .filter(|candidate| candidate.fulfillment_pubkey == uploader)
            .cloned()
            .collect::<Vec<_>>();
        candidates.sort_by_key(|candidate| candidate.root_event_id.to_hex());
        let mut authorized = false;
        for candidate in candidates {
            if super::entitlement::resolve_listing_authorization(
                &state,
                developer_pubkey,
                &coordinate,
                &candidate,
                adp_core::CAPABILITY_UPLOAD_BUILD,
                now,
            )
            .await?
            {
                authorized = true;
                break;
            }
        }
        if !authorized {
            return Err(ApiError::Unauthorized(
                "uploader lacks an enabled upload_build authorization".into(),
            ));
        }
    }

    let expected_hash = required_file_hash(&listing)?;
    if expected_hash != computed_hash {
        return Err(ApiError::BadRequest(
            "uploaded file's SHA-256 does not match the listing's file_hash tag".into(),
        ));
    }

    let file_path = format!("{}/{}", state.config.files_dir, computed_hash);
    tokio::fs::rename(&staged, &file_path)
        .await
        .map_err(|e| ApiError::Internal(format!("failed to store file: {e}")))?;

    let coordinate = listing.coordinate();
    state
        .storage
        .record_file(&computed_hash, &coordinate, &file_path)
        .await?;

    Ok(Json(UploadResponse {
        download_url: format!("{}/game/{}", state.config.server_url, coordinate),
        game_coordinate: coordinate,
        file_hash: computed_hash,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr_sdk::{EventBuilder, Keys, Kind, Tag, Timestamp};

    fn tag(values: Vec<&str>) -> Tag {
        Tag::parse(values).expect("valid tag")
    }

    fn listing(tags: Vec<Tag>, created_at: u64) -> AdpListing {
        let developer = Keys::generate();
        let mut all_tags = vec![tag(vec!["d", "game-1"])];
        all_tags.extend(tags);
        let event = EventBuilder::new(Kind::Custom(30402), "")
            .tags(all_tags)
            .custom_created_at(Timestamp::from(created_at))
            .sign_with_keys(&developer)
            .expect("sign listing");
        AdpListing::from_event(event).expect("parse listing")
    }

    #[test]
    fn required_file_hash_rejects_missing_tag() {
        let listing = listing(vec![], 1_700_000_000);

        assert!(required_file_hash(&listing).is_err());
    }

    #[test]
    fn required_file_hash_returns_declared_hash() {
        let listing = listing(vec![tag(vec!["file_hash", "abc123"])], 1_700_000_000);

        assert_eq!(required_file_hash(&listing).expect("file hash"), "abc123");
    }
}
