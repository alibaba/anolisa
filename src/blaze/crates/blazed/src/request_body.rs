// SPDX-License-Identifier: Apache-2.0
//! Bounded HTTP request-body collection for daemon API routes.

use http_body_util::BodyExt;
use hyper::Request;
use hyper::body::{Body, Bytes};
use hyper::header::CONTENT_LENGTH;

/// Request-body failures that are classified by the API layer.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CollectError {
    /// A body header or stream could not be read as a valid request.
    BadRequest(String),
    /// The declared or observed body size exceeded the route limit.
    TooLarge { actual: u64, limit: usize },
}

/// Collect a request body without buffering more than `limit` bytes.
pub(crate) async fn collect<B>(req: Request<B>, limit: usize) -> Result<Vec<u8>, CollectError>
where
    B: Body<Data = Bytes> + Unpin,
    B::Error: std::fmt::Display,
{
    if let Some(declared) = declared_body_length(&req)?
        && declared > limit as u64
    {
        return Err(CollectError::TooLarge {
            actual: declared,
            limit,
        });
    }

    let mut body = req.into_body();
    let mut collected = Vec::new();
    while let Some(frame) = body.frame().await {
        let frame =
            frame.map_err(|error| CollectError::BadRequest(format!("request body: {error}")))?;
        let Ok(data) = frame.into_data() else {
            continue;
        };
        let actual = collected
            .len()
            .checked_add(data.len())
            .ok_or(CollectError::TooLarge {
                actual: u64::MAX,
                limit,
            })?;
        if actual > limit {
            return Err(CollectError::TooLarge {
                actual: actual as u64,
                limit,
            });
        }
        collected.extend_from_slice(&data);
    }
    Ok(collected)
}

fn declared_body_length<B>(req: &Request<B>) -> Result<Option<u64>, CollectError> {
    let mut declared = None;
    for value in req.headers().get_all(CONTENT_LENGTH) {
        let value = value
            .to_str()
            .map_err(|_| CollectError::BadRequest("invalid Content-Length".into()))?;
        for item in value.split(',') {
            let item = item.trim();
            if item.is_empty() || !item.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(CollectError::BadRequest("invalid Content-Length".into()));
            }
            let length = item
                .parse::<u64>()
                .map_err(|_| CollectError::BadRequest("invalid Content-Length".into()))?;
            match declared {
                Some(previous) if previous != length => {
                    return Err(CollectError::BadRequest(
                        "conflicting Content-Length values".into(),
                    ));
                }
                None => declared = Some(length),
                _ => {}
            }
        }
    }
    Ok(declared)
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::fmt;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::{Context, Poll};

    use hyper::body::Frame;
    use hyper::header::{HeaderValue, TRANSFER_ENCODING};

    use super::*;

    #[derive(Debug)]
    struct TestBodyError;

    impl fmt::Display for TestBodyError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("test body failed")
        }
    }

    impl std::error::Error for TestBodyError {}

    struct TestBody {
        frames: VecDeque<Result<Frame<Bytes>, TestBodyError>>,
        polls: Arc<AtomicUsize>,
        panic_when_exhausted: bool,
    }

    impl TestBody {
        fn new(
            frames: impl IntoIterator<Item = Result<Frame<Bytes>, TestBodyError>>,
            polls: Arc<AtomicUsize>,
        ) -> Self {
            Self {
                frames: frames.into_iter().collect(),
                polls,
                panic_when_exhausted: false,
            }
        }

        fn panic_when_exhausted(mut self) -> Self {
            self.panic_when_exhausted = true;
            self
        }
    }

    impl Body for TestBody {
        type Data = Bytes;
        type Error = TestBodyError;

        fn poll_frame(
            self: Pin<&mut Self>,
            _context: &mut Context<'_>,
        ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
            let this = self.get_mut();
            this.polls.fetch_add(1, Ordering::AcqRel);
            match this.frames.pop_front() {
                Some(frame) => Poll::Ready(Some(frame)),
                None if this.panic_when_exhausted => {
                    panic!("collector polled after the limit had already been exceeded")
                }
                None => Poll::Ready(None),
            }
        }
    }

    #[tokio::test]
    async fn accepts_body_at_declared_limit() {
        let polls = Arc::new(AtomicUsize::new(0));
        let body = TestBody::new(
            [
                Ok(Frame::data(Bytes::from_static(b"ab"))),
                Ok(Frame::data(Bytes::from_static(b"cd"))),
            ],
            polls.clone(),
        );
        let request = Request::builder()
            .header(CONTENT_LENGTH, "4")
            .body(body)
            .expect("request");

        assert_eq!(collect(request, 4).await.expect("body"), b"abcd");
        assert_eq!(polls.load(Ordering::Acquire), 3);
    }

    #[tokio::test]
    async fn rejects_large_content_length_before_polling_body() {
        let polls = Arc::new(AtomicUsize::new(0));
        let body = TestBody::new(
            [Ok(Frame::data(Bytes::from_static(b"body")))],
            polls.clone(),
        )
        .panic_when_exhausted();
        let request = Request::builder()
            .header(CONTENT_LENGTH, "5")
            .body(body)
            .expect("request");

        assert_eq!(
            collect(request, 4).await,
            Err(CollectError::TooLarge {
                actual: 5,
                limit: 4
            })
        );
        assert_eq!(polls.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn accepts_matching_repeated_and_combined_content_lengths() {
        let polls = Arc::new(AtomicUsize::new(0));
        let body = TestBody::new(
            [Ok(Frame::data(Bytes::from_static(b"body")))],
            polls.clone(),
        );
        let mut request = Request::new(body);
        request
            .headers_mut()
            .append(CONTENT_LENGTH, HeaderValue::from_static("4"));
        request
            .headers_mut()
            .append(CONTENT_LENGTH, HeaderValue::from_static("4, 4"));

        assert_eq!(collect(request, 4).await.expect("body"), b"body");
        assert_eq!(polls.load(Ordering::Acquire), 2);
    }

    #[tokio::test]
    async fn rejects_conflicting_content_lengths_before_polling_body() {
        let polls = Arc::new(AtomicUsize::new(0));
        let body = TestBody::new([], polls.clone()).panic_when_exhausted();
        let mut request = Request::new(body);
        request
            .headers_mut()
            .append(CONTENT_LENGTH, HeaderValue::from_static("3"));
        request
            .headers_mut()
            .append(CONTENT_LENGTH, HeaderValue::from_static("4"));

        assert_eq!(
            collect(request, 4).await,
            Err(CollectError::BadRequest(
                "conflicting Content-Length values".into()
            ))
        );
        assert_eq!(polls.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn rejects_conflicting_or_empty_content_length_lists() {
        for value in ["3, 4", "4,", ",4"] {
            let polls = Arc::new(AtomicUsize::new(0));
            let body = TestBody::new([], polls.clone()).panic_when_exhausted();
            let request = Request::builder()
                .header(CONTENT_LENGTH, value)
                .body(body)
                .expect("request");

            assert!(matches!(
                collect(request, 4).await,
                Err(CollectError::BadRequest(_))
            ));
            assert_eq!(polls.load(Ordering::Acquire), 0);
        }
    }

    #[tokio::test]
    async fn rejects_invalid_content_length_before_polling_body() {
        for value in ["not-a-number", "+4", "-1", "4 0", "4.0"] {
            let polls = Arc::new(AtomicUsize::new(0));
            let body = TestBody::new([], polls.clone()).panic_when_exhausted();
            let request = Request::builder()
                .header(CONTENT_LENGTH, value)
                .body(body)
                .expect("request");

            assert_eq!(
                collect(request, 4).await,
                Err(CollectError::BadRequest("invalid Content-Length".into()))
            );
            assert_eq!(polls.load(Ordering::Acquire), 0);
        }
    }

    #[tokio::test]
    async fn stops_undelimited_body_at_observed_limit() {
        let polls = Arc::new(AtomicUsize::new(0));
        let body = TestBody::new(
            [
                Ok(Frame::data(Bytes::from_static(b"ab"))),
                Ok(Frame::data(Bytes::from_static(b"cde"))),
            ],
            polls.clone(),
        )
        .panic_when_exhausted();
        let request = Request::builder()
            .header(TRANSFER_ENCODING, "chunked")
            .body(body)
            .expect("request");

        assert_eq!(
            collect(request, 4).await,
            Err(CollectError::TooLarge {
                actual: 5,
                limit: 4
            })
        );
        assert_eq!(polls.load(Ordering::Acquire), 2);
    }

    #[tokio::test]
    async fn stops_body_without_length_header_at_observed_limit() {
        let polls = Arc::new(AtomicUsize::new(0));
        let body = TestBody::new(
            [
                Ok(Frame::data(Bytes::from_static(b"ab"))),
                Ok(Frame::data(Bytes::from_static(b"cde"))),
            ],
            polls.clone(),
        )
        .panic_when_exhausted();

        assert_eq!(
            collect(Request::new(body), 4).await,
            Err(CollectError::TooLarge {
                actual: 5,
                limit: 4
            })
        );
        assert_eq!(polls.load(Ordering::Acquire), 2);
    }

    #[tokio::test]
    async fn ignores_underreported_length_and_stops_at_observed_limit() {
        let polls = Arc::new(AtomicUsize::new(0));
        let body = TestBody::new(
            [
                Ok(Frame::data(Bytes::from_static(b"ab"))),
                Ok(Frame::data(Bytes::from_static(b"cde"))),
            ],
            polls.clone(),
        )
        .panic_when_exhausted();
        let request = Request::builder()
            .header(CONTENT_LENGTH, "1")
            .body(body)
            .expect("request");

        assert_eq!(
            collect(request, 4).await,
            Err(CollectError::TooLarge {
                actual: 5,
                limit: 4
            })
        );
        assert_eq!(polls.load(Ordering::Acquire), 2);
    }

    #[tokio::test]
    async fn reports_body_read_failures_as_bad_requests() {
        let body = TestBody::new([Err(TestBodyError)], Arc::new(AtomicUsize::new(0)));

        assert_eq!(
            collect(Request::new(body), 4).await,
            Err(CollectError::BadRequest(
                "request body: test body failed".into()
            ))
        );
    }
}
