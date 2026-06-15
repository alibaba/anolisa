//! HTTP Connection Aggregator - correlates HTTP requests with responses
//
//! This module implements the HTTP Aggregator specification for correlating
//! parsed HTTP requests and responses into complete request/response pairs.

use super::super::result::AggregatedResult;
use super::pair::HttpPair;
use super::response::AggregatedResponse;
use crate::config::DEFAULT_CONNECTION_CAPACITY;
use crate::parser::http::{ParsedRequest, ParsedResponse};
use crate::parser::sse::{ParsedSseEvent, SseParser};
use crate::probes::sslsniff::SslEvent;
use lru::LruCache;
use std::num::NonZeroUsize;

/// Connection identifier - uniquely identifies an SSL connection
#[derive(Debug, Clone, Copy, Hash, Eq, PartialEq)]
pub struct ConnectionId {
    pub pid: u32,
    pub ssl_ptr: u64,
}

impl ConnectionId {
    /// Create from SslEvent
    pub fn from_ssl_event(event: &SslEvent) -> Self {
        ConnectionId {
            pid: event.pid,
            ssl_ptr: event.ssl_ptr,
        }
    }
}

/// Connection state machine
#[derive(Debug, Clone)]
pub enum ConnectionState {
    /// Idle - waiting for request
    Idle,
    /// Request pending - waiting for response
    RequestPending { request: ParsedRequest },
    /// Request body pending - body not yet complete, waiting for more data or response
    RequestBodyPending {
        request: ParsedRequest,
        expected_body_len: Option<usize>,
        body_buffer: Vec<u8>,
    },
    /// SSE active - response headers received, body streaming
    SseActive {
        request: Option<ParsedRequest>,
        response_headers: ParsedResponse,
        sse_events: Vec<ParsedSseEvent>,
    },
}

/// HTTP Connection Aggregator
#[derive(Debug)]
pub struct HttpConnectionAggregator {
    connections: LruCache<ConnectionId, ConnectionState>,
}

impl Default for HttpConnectionAggregator {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpConnectionAggregator {
    /// Create a new aggregator with default capacity
    pub fn new() -> Self {
        HttpConnectionAggregator {
            connections: LruCache::new(NonZeroUsize::new(DEFAULT_CONNECTION_CAPACITY).unwrap()),
        }
    }

    /// Create a new aggregator with custom capacity
    pub fn with_capacity(capacity: usize) -> Self {
        HttpConnectionAggregator {
            connections: LruCache::new(NonZeroUsize::new(capacity).unwrap()),
        }
    }

    /// Insert connection state, logging if an unrelated entry is evicted by LRU
    fn insert(&mut self, key: ConnectionId, state: ConnectionState) {
        if let Some((evicted_key, evicted_state)) = self.connections.push(key, state) {
            if evicted_key != key {
                log::warn!(
                    "[HttpAggregator] LRU evicted conn={:?} state={} | capacity={}",
                    evicted_key,
                    match evicted_state {
                        ConnectionState::Idle => "Idle",
                        ConnectionState::RequestPending { .. } => "RequestPending",
                        ConnectionState::RequestBodyPending { .. } => "RequestBodyPending",
                        ConnectionState::SseActive { .. } => "SseActive",
                    },
                    self.connections.cap(),
                );
            }
        }
    }

    /// Parse initial SSE body bytes from the first HTTP response packet.
    ///
    /// When HTTP response headers and the first SSE `data:` chunk arrive in the
    /// same SSL_read buffer, the parser only emits `ParsedResponse`. Downstream
    /// SSE analysis consumes `sse_events`, so we must convert the response body
    /// into initial `ParsedSseEvent`s before entering `SseActive`.
    fn initial_sse_events(response: &ParsedResponse) -> Vec<ParsedSseEvent> {
        let body = response.body();
        if body.is_empty() {
            return Vec::new();
        }

        let synthetic_event = std::rc::Rc::new(SslEvent {
            source: response.source_event.source,
            timestamp_ns: response.source_event.timestamp_ns,
            delta_ns: response.source_event.delta_ns,
            pid: response.source_event.pid,
            tid: response.source_event.tid,
            uid: response.source_event.uid,
            len: body.len() as u32,
            rw: response.source_event.rw,
            comm: response.source_event.comm.clone(),
            buf: body.to_vec(),
            is_handshake: response.source_event.is_handshake,
            ssl_ptr: response.source_event.ssl_ptr,
        });

        SseParser::new().parse(synthetic_event)
    }

    /// Process HTTP Request (from HTTP Parser)
    pub fn process_request(&mut self, request: ParsedRequest) {
        let connection_id = ConnectionId::from_ssl_event(&request.source_event);

        // Check if body is complete by comparing with Content-Length
        let content_length: Option<usize> = request
            .headers
            .get("content-length")
            .and_then(|v| v.parse().ok());

        let body_complete = match content_length {
            Some(cl) => request.body_len >= cl,
            None => {
                // No Content-Length: check for Transfer-Encoding: chunked
                let is_chunked = request
                    .headers
                    .get("transfer-encoding")
                    .map(|v| v.contains("chunked"))
                    .unwrap_or(false);
                if is_chunked {
                    // Check if body contains chunked terminator
                    let body = request.body();
                    body.windows(5).any(|w| w == b"0\r\n\r\n")
                } else {
                    true // No Content-Length and not chunked → body is complete
                }
            }
        };

        if body_complete {
            log::trace!(
                "[HttpAggregator] State transition: -> RequestPending | conn={:?} | method={} | path={}",
                connection_id,
                request.method,
                request.path,
            );
            self.insert(connection_id, ConnectionState::RequestPending { request });
        } else {
            log::debug!(
                "[HttpAggregator] State transition: -> RequestBodyPending | conn={:?} | method={} | path={} | body_len={} | content_length={:?}",
                connection_id,
                request.method,
                request.path,
                request.body_len,
                content_length,
            );
            let initial_body = request.body().to_vec();
            self.insert(
                connection_id,
                ConnectionState::RequestBodyPending {
                    request,
                    expected_body_len: content_length,
                    body_buffer: initial_body,
                },
            );
        }
    }

    /// Process HTTP Response (from HTTP Parser)
    /// Returns completed HttpPair or SSE started signal
    pub fn process_response(&mut self, response: ParsedResponse) -> Option<AggregatedResult> {
        let connection_id = ConnectionId::from_ssl_event(&response.source_event);

        let state = self.connections.pop(&connection_id)?;

        match state {
            ConnectionState::RequestBodyPending {
                request,
                expected_body_len,
                mut body_buffer,
            } => {
                // Response arrived → request must be complete (server replies only after full request)
                log::debug!(
                    "[HttpAggregator] State transition: RequestBodyPending -> Complete (response-driven) | conn={:?} | buffered={}",
                    connection_id,
                    body_buffer.len(),
                );
                if let Some(cl) = expected_body_len {
                    body_buffer.truncate(cl);
                }
                let mut completed_request = request;
                completed_request.reassembled_body = Some(body_buffer);

                if response.is_sse() {
                    let mut response_headers = response;
                    let sse_events = Self::initial_sse_events(&response_headers);
                    response_headers.body_len = 0;
                    self.insert(
                        connection_id,
                        ConnectionState::SseActive {
                            request: Some(completed_request),
                            response_headers,
                            sse_events,
                        },
                    );
                    None
                } else {
                    let pair = HttpPair::from_parsed(connection_id, completed_request, response);
                    Some(AggregatedResult::HttpComplete(pair))
                }
            }
            ConnectionState::RequestPending { request } => {
                if response.is_sse() {
                    log::trace!(
                        "[HttpAggregator] State transition: RequestPending -> SseActive | conn={:?} | status={}",
                        connection_id,
                        response.status_code,
                    );
                    let mut response_headers = response;
                    let sse_events = Self::initial_sse_events(&response_headers);
                    response_headers.body_len = 0;
                    // Transition to SSE active state, wait for SSE events
                    self.insert(
                        connection_id,
                        ConnectionState::SseActive {
                            request: Some(request),
                            response_headers,
                            sse_events,
                        },
                    );

                    // Don't return HttpPair yet, wait for SSE events to complete
                    None
                } else {
                    log::trace!(
                        "[HttpAggregator] State transition: RequestPending -> Complete | conn={:?} | status={}",
                        connection_id,
                        response.status_code,
                    );
                    let pair = HttpPair::from_parsed(connection_id, request, response);
                    Some(AggregatedResult::HttpComplete(pair))
                }
            }
            ConnectionState::Idle => {
                if response.is_sse() {
                    // SSE response without prior request - still need to wait for SSE events
                    log::trace!(
                        "[HttpAggregator] State transition: Idle -> SseActive (no request) | conn={:?} | status={}",
                        connection_id,
                        response.status_code
                    );
                    let mut response_headers = response;
                    let sse_events = Self::initial_sse_events(&response_headers);
                    response_headers.body_len = 0;
                    self.insert(
                        connection_id,
                        ConnectionState::SseActive {
                            request: None,
                            response_headers,
                            sse_events,
                        },
                    );
                    None
                } else {
                    log::trace!(
                        "[HttpAggregator] State transition: Idle -> ResponseOnly | conn={:?} | status={}",
                        connection_id,
                        response.status_code
                    );
                    let aggregated_response = AggregatedResponse::from_parsed(response);
                    Some(AggregatedResult::ResponseOnly {
                        connection_id,
                        response: aggregated_response,
                    })
                }
            }
            ConnectionState::SseActive { .. } => {
                log::trace!(
                    "[HttpAggregator] State transition: SseActive (unexpected response) | conn={connection_id:?}"
                );
                // Response on SSE connection - shouldn't happen normally
                // Restore state and return None
                self.insert(connection_id, state);
                None
            }
        }
    }

    /// Process raw body data (continuation bytes for an in-progress request)
    pub fn process_raw_body_data(&mut self, ssl_event: &SslEvent) -> Option<AggregatedResult> {
        let connection_id = ConnectionId::from_ssl_event(ssl_event);
        let state = self.connections.pop(&connection_id)?;

        match state {
            ConnectionState::RequestBodyPending {
                request,
                expected_body_len,
                mut body_buffer,
            } => {
                // Append new data to buffer
                let data = &ssl_event.buf[..ssl_event.buf_size() as usize];
                body_buffer.extend_from_slice(data);

                // Check if body is now complete
                let complete = match expected_body_len {
                    Some(cl) => body_buffer.len() >= cl,
                    None => {
                        // chunked: check for terminator
                        body_buffer.windows(5).any(|w| w == b"0\r\n\r\n")
                    }
                };

                if complete {
                    log::debug!(
                        "[HttpAggregator] State transition: RequestBodyPending -> RequestPending (body complete) | conn={:?} | total_body={}",
                        connection_id,
                        body_buffer.len(),
                    );
                    if let Some(cl) = expected_body_len {
                        body_buffer.truncate(cl);
                    }
                    let mut completed_request = request;
                    completed_request.reassembled_body = Some(body_buffer);
                    self.insert(
                        connection_id,
                        ConnectionState::RequestPending {
                            request: completed_request,
                        },
                    );
                } else {
                    log::trace!(
                        "[HttpAggregator] RequestBodyPending: buffered more data | conn={:?} | total={}",
                        connection_id,
                        body_buffer.len(),
                    );
                    self.insert(
                        connection_id,
                        ConnectionState::RequestBodyPending {
                            request,
                            expected_body_len,
                            body_buffer,
                        },
                    );
                }
                None
            }
            other => {
                // Not in RequestBodyPending state, restore and ignore
                self.insert(connection_id, other);
                None
            }
        }
    }

    /// Process SSE Event (from SSE Parser)
    /// Only valid when connection is in SseActive state
    pub fn process_sse_event(
        &mut self,
        connection_id: &ConnectionId,
        sse_event: ParsedSseEvent,
    ) -> Option<AggregatedResult> {
        let state = self.connections.pop(connection_id)?;

        match state {
            ConnectionState::SseActive {
                request,
                response_headers,
                mut sse_events,
            } => {
                // Check if stream is done before processing
                let is_done = sse_event.is_done();

                log::trace!(
                    "[HttpAggregator] SSE event in SseActive | conn={connection_id:?} | is_done={is_done}",
                );

                // Add SSE event to the list
                sse_events.push(sse_event);

                if is_done {
                    log::trace!(
                        "[HttpAggregator] State transition: SseActive -> Complete | conn={connection_id:?}",
                    );

                    // Build aggregated response with SSE events
                    let mut response = AggregatedResponse::from_parsed(response_headers);
                    response.set_sse_events(sse_events);

                    // Return appropriate result based on whether request exists
                    if let Some(req) = request {
                        let parsed = response.parsed.clone();
                        let mut pair = HttpPair::from_parsed(*connection_id, req, parsed);
                        pair.response = response;
                        Some(AggregatedResult::SseComplete(pair))
                    } else {
                        Some(AggregatedResult::ResponseOnly {
                            connection_id: *connection_id,
                            response,
                        })
                    }
                } else {
                    // Continue SSE active state
                    self.insert(
                        *connection_id,
                        ConnectionState::SseActive {
                            request,
                            response_headers,
                            sse_events,
                        },
                    );

                    None
                }
            }
            _ => {
                log::trace!(
                    "[HttpAggregator] SSE event in unexpected state | conn={connection_id:?}"
                );
                // Not in SSE active state, restore state
                self.insert(*connection_id, state);
                None
            }
        }
    }

    /// Get active connection count
    pub fn active_connections(&self) -> usize {
        self.connections.len()
    }

    /// Check if connection has pending request
    pub fn has_pending_request(&self, connection_id: &ConnectionId) -> bool {
        matches!(
            self.connections.peek(connection_id),
            Some(ConnectionState::RequestPending { .. })
        )
    }

    /// Check if connection is SSE active
    pub fn is_sse_active(&self, connection_id: &ConnectionId) -> bool {
        matches!(
            self.connections.peek(connection_id),
            Some(ConnectionState::SseActive { .. })
        )
    }

    /// Check if there are any pending connections
    pub fn has_pending(&self) -> bool {
        !self.connections.is_empty()
    }

    /// Clear all connections
    pub fn clear(&mut self) {
        self.connections.clear();
    }

    /// Drain all connections (for force complete)
    pub fn drain_connections(&mut self) -> Vec<(ConnectionId, ConnectionState)> {
        self.connections
            .iter_mut()
            .map(|(k, v)| (*k, v.clone()))
            .collect::<Vec<_>>()
            .into_iter()
            .map(|(k, _)| (k, self.connections.pop(&k).unwrap()))
            .collect()
    }

    /// Drain all connections belonging to a specific PID.
    ///
    /// Returns `(ConnectionId, ConnectionState)` for entries that were in
    /// `RequestPending` or `SseActive` state.  `Idle` entries are silently
    /// discarded.  Used by crash detection on `ProcMon::Exit`.
    pub fn drain_connections_for_pid(&mut self, pid: u32) -> Vec<(ConnectionId, ConnectionState)> {
        let keys: Vec<ConnectionId> = self
            .connections
            .iter()
            .filter(|(k, _)| k.pid == pid)
            .map(|(k, _)| *k)
            .collect();

        if keys.is_empty() {
            return vec![];
        }

        let mut result = Vec::new();
        for key in keys {
            if let Some(state) = self.connections.pop(&key) {
                match state {
                    ConnectionState::Idle => {}
                    _ => {
                        log::debug!(
                            "[HttpAggregator] Draining connection for exited PID: pid={} ssl_ptr={:#x}",
                            key.pid,
                            key.ssl_ptr,
                        );
                        result.push((key, state));
                    }
                }
            }
        }

        if !result.is_empty() {
            log::info!(
                "[HttpAggregator] Drained {} connection(s) for exited pid={}",
                result.len(),
                pid,
            );
        }

        result
    }

    /// Drain connections whose PID is no longer alive.
    ///
    /// Checks `/proc/{pid}` for each unique PID in the connection pool.
    /// Returns `(ConnectionId, ConnectionState)` for dead-PID entries that
    /// were in `RequestPending` or `SseActive` state.  `Idle` entries are
    /// silently discarded.  This allows the caller to persist orphaned
    /// in-flight requests before they are lost.
    pub fn drain_dead_pid_connections(&mut self) -> Vec<(ConnectionId, ConnectionState)> {
        use std::collections::HashSet;

        // 1. Collect unique PIDs
        let pids: HashSet<u32> = self.connections.iter().map(|(k, _)| k.pid).collect();

        // 2. Determine which PIDs are dead
        let dead_pids: HashSet<u32> = pids
            .into_iter()
            .filter(|pid| !std::path::Path::new(&format!("/proc/{pid}")).exists())
            .collect();

        if dead_pids.is_empty() {
            return vec![];
        }

        // 3. Collect keys for dead PIDs (can't mutate while iterating)
        let dead_keys: Vec<ConnectionId> = self
            .connections
            .iter()
            .filter(|(k, _)| dead_pids.contains(&k.pid))
            .map(|(k, _)| *k)
            .collect();

        // 4. Pop dead entries and return non-Idle ones
        let mut result = Vec::new();
        for key in dead_keys {
            if let Some(state) = self.connections.pop(&key) {
                match state {
                    ConnectionState::Idle => {
                        // Silently discard idle entries
                    }
                    _ => {
                        log::debug!(
                            "[HttpAggregator] Draining dead-PID connection: pid={} ssl_ptr={:#x}",
                            key.pid,
                            key.ssl_ptr,
                        );
                        result.push((key, state));
                    }
                }
            }
        }

        if !result.is_empty() {
            log::info!(
                "[HttpAggregator] Drained {} connection(s) for dead PIDs: {:?}",
                result.len(),
                dead_pids,
            );
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::rc::Rc;

    fn create_mock_ssl_event(pid: u32, ssl_ptr: u64) -> Rc<SslEvent> {
        Rc::new(SslEvent {
            source: 0,
            timestamp_ns: 1000,
            delta_ns: 0,
            pid,
            tid: 1,
            uid: 0,
            len: 0,
            rw: 0,
            comm: String::new(),
            buf: Vec::new(),
            is_handshake: false,
            ssl_ptr,
        })
    }

    #[test]
    fn test_connection_id() {
        let id = ConnectionId {
            pid: 1234,
            ssl_ptr: 0x1000,
        };
        assert_eq!(id.pid, 1234);
        assert_eq!(id.ssl_ptr, 0x1000);
    }

    #[test]
    fn test_process_request_response_pair() {
        let mut aggregator = HttpConnectionAggregator::new();
        let event = create_mock_ssl_event(1234, 0x1000);

        // Process request
        let request = ParsedRequest {
            method: "GET".to_string(),
            path: "/api/test".to_string(),
            version: 11,
            headers: HashMap::new(),
            body_offset: 0,
            body_len: 0,
            source_event: event.clone(),
            reassembled_body: None,
        };
        aggregator.process_request(request);

        assert!(aggregator.has_pending_request(&ConnectionId {
            pid: 1234,
            ssl_ptr: 0x1000
        }));

        // Process response
        let response = ParsedResponse {
            version: 11,
            status_code: 200,
            reason: "OK".to_string(),
            headers: HashMap::new(),
            body_offset: 0,
            body_len: 0,
            source_event: event,
        };

        let result = aggregator.process_response(response);
        assert!(result.is_some());

        if let Some(AggregatedResult::HttpComplete(pair)) = result {
            assert_eq!(pair.request.method, "GET");
            assert_eq!(pair.response.status_code(), 200);
            assert!(pair.response.sse_events.is_empty());
        } else {
            panic!("Expected HttpComplete result");
        }
    }

    #[test]
    fn test_sse_detection() {
        let mut aggregator = HttpConnectionAggregator::new();
        let event = create_mock_ssl_event(1234, 0x1000);

        // Process request
        let request = ParsedRequest {
            method: "GET".to_string(),
            path: "/stream".to_string(),
            version: 11,
            headers: HashMap::new(),
            body_offset: 0,
            body_len: 0,
            source_event: event.clone(),
            reassembled_body: None,
        };
        aggregator.process_request(request);

        // Process SSE response
        let mut headers = HashMap::new();
        headers.insert("content-type".to_string(), "text/event-stream".to_string());

        let response = ParsedResponse {
            version: 11,
            status_code: 200,
            reason: "OK".to_string(),
            headers,
            body_offset: 0,
            body_len: 0,
            source_event: event,
        };

        let result = aggregator.process_response(response);

        // SSE response should not return result immediately, but should activate SSE state
        assert!(result.is_none());
        assert!(aggregator.is_sse_active(&ConnectionId {
            pid: 1234,
            ssl_ptr: 0x1000
        }));
    }

    fn create_mock_ssl_event_with_buf(
        pid: u32,
        ssl_ptr: u64,
        buf: Vec<u8>,
        rw: i32,
    ) -> Rc<SslEvent> {
        Rc::new(SslEvent {
            source: 0,
            timestamp_ns: 1000,
            delta_ns: 0,
            pid,
            tid: 1,
            uid: 0,
            len: buf.len() as u32,
            rw,
            comm: String::new(),
            buf,
            is_handshake: false,
            ssl_ptr,
        })
    }

    #[test]
    fn test_request_body_aggregation_content_length() {
        let mut aggregator = HttpConnectionAggregator::new();

        // Simulate a request with Content-Length: 20 but only 5 bytes in first event
        let headers_and_partial_body = b"POST /api HTTP/1.1\r\nContent-Length: 20\r\n\r\nhello";
        let event1 =
            create_mock_ssl_event_with_buf(1234, 0x2000, headers_and_partial_body.to_vec(), 1);

        // Parse as request (simulating what HttpParser would produce)
        let header_end = headers_and_partial_body
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .unwrap()
            + 4;
        let body_len = headers_and_partial_body.len() - header_end;

        let mut headers = HashMap::new();
        headers.insert("content-length".to_string(), "20".to_string());

        let request = ParsedRequest {
            method: "POST".to_string(),
            path: "/api".to_string(),
            version: 1,
            headers,
            body_offset: header_end,
            body_len,
            source_event: event1,
            reassembled_body: None,
        };

        // Process request - should enter RequestBodyPending since body_len(5) < content_length(20)
        aggregator.process_request(request);
        let conn_id = ConnectionId {
            pid: 1234,
            ssl_ptr: 0x2000,
        };
        assert!(!aggregator.has_pending_request(&conn_id));

        // Send continuation data (10 bytes)
        let continuation1 = SslEvent {
            source: 0,
            timestamp_ns: 2000,
            delta_ns: 0,
            pid: 1234,
            tid: 1,
            uid: 0,
            len: 10,
            rw: 1,
            comm: String::new(),
            buf: b" world fir".to_vec(),
            is_handshake: false,
            ssl_ptr: 0x2000,
        };
        let result = aggregator.process_raw_body_data(&continuation1);
        assert!(result.is_none()); // Still incomplete (15 < 20)

        // Send final continuation (5 bytes, total = 5 + 10 + 5 = 20)
        let continuation2 = SslEvent {
            source: 0,
            timestamp_ns: 3000,
            delta_ns: 0,
            pid: 1234,
            tid: 1,
            uid: 0,
            len: 5,
            rw: 1,
            comm: String::new(),
            buf: b"st!!!".to_vec(),
            is_handshake: false,
            ssl_ptr: 0x2000,
        };
        let result = aggregator.process_raw_body_data(&continuation2);
        assert!(result.is_none()); // Transitioned to RequestPending

        // Now the request should be in RequestPending with full body
        assert!(aggregator.has_pending_request(&conn_id));

        // Sending a response should complete the pair
        let resp_event = create_mock_ssl_event_with_buf(
            1234,
            0x2000,
            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nOK".to_vec(),
            0,
        );
        let response = ParsedResponse {
            version: 1,
            status_code: 200,
            reason: "OK".to_string(),
            headers: HashMap::new(),
            body_offset: 0,
            body_len: 2,
            source_event: resp_event,
        };

        let result = aggregator.process_response(response);
        assert!(result.is_some());
        if let Some(AggregatedResult::HttpComplete(pair)) = result {
            assert_eq!(pair.request.method, "POST");
            // Verify the reassembled body
            let body = pair.request.body();
            assert_eq!(body, b"hello world first!!!");
            assert_eq!(body.len(), 20);
        } else {
            panic!("Expected HttpComplete result");
        }
    }

    #[test]
    fn test_request_body_aggregation_response_completion() {
        let mut aggregator = HttpConnectionAggregator::new();

        // Request with Content-Length but body will be completed by response arrival
        let headers_and_partial = b"POST /chat HTTP/1.1\r\nContent-Length: 100\r\n\r\npartial";
        let event = create_mock_ssl_event_with_buf(5678, 0x3000, headers_and_partial.to_vec(), 1);

        let header_end = headers_and_partial
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .unwrap()
            + 4;
        let body_len = headers_and_partial.len() - header_end;

        let mut headers = HashMap::new();
        headers.insert("content-length".to_string(), "100".to_string());

        let request = ParsedRequest {
            method: "POST".to_string(),
            path: "/chat".to_string(),
            version: 1,
            headers,
            body_offset: header_end,
            body_len,
            source_event: event,
            reassembled_body: None,
        };

        aggregator.process_request(request);

        // Send some continuation
        let cont = SslEvent {
            source: 0,
            timestamp_ns: 2000,
            delta_ns: 0,
            pid: 5678,
            tid: 1,
            uid: 0,
            len: 10,
            rw: 1,
            comm: String::new(),
            buf: b"_more_data".to_vec(),
            is_handshake: false,
            ssl_ptr: 0x3000,
        };
        aggregator.process_raw_body_data(&cont);

        // Response arrives before Content-Length is satisfied → force-complete
        let resp_event =
            create_mock_ssl_event_with_buf(5678, 0x3000, b"HTTP/1.1 200 OK\r\n\r\n{}".to_vec(), 0);
        let response = ParsedResponse {
            version: 1,
            status_code: 200,
            reason: "OK".to_string(),
            headers: HashMap::new(),
            body_offset: 0,
            body_len: 2,
            source_event: resp_event,
        };

        let result = aggregator.process_response(response);
        assert!(result.is_some());
        if let Some(AggregatedResult::HttpComplete(pair)) = result {
            // Body should be truncated to content_length (100) but we only have 17 bytes
            // Since total buffer (17) < content_length (100), truncate does nothing
            let body = pair.request.body();
            assert_eq!(body, b"partial_more_data");
        } else {
            panic!("Expected HttpComplete result");
        }
    }

    #[test]
    fn test_request_body_single_event_no_aggregation() {
        let mut aggregator = HttpConnectionAggregator::new();

        // Request where body fits in single event (body_len >= content_length)
        let full_request = b"POST /api HTTP/1.1\r\nContent-Length: 5\r\n\r\nhello";
        let event = create_mock_ssl_event_with_buf(1234, 0x4000, full_request.to_vec(), 1);

        let header_end = full_request
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .unwrap()
            + 4;
        let body_len = full_request.len() - header_end;

        let mut headers = HashMap::new();
        headers.insert("content-length".to_string(), "5".to_string());

        let request = ParsedRequest {
            method: "POST".to_string(),
            path: "/api".to_string(),
            version: 1,
            headers,
            body_offset: header_end,
            body_len,
            source_event: event,
            reassembled_body: None,
        };

        // Should go directly to RequestPending (no aggregation needed)
        aggregator.process_request(request);
        let conn_id = ConnectionId {
            pid: 1234,
            ssl_ptr: 0x4000,
        };
        assert!(aggregator.has_pending_request(&conn_id));
    }

    #[test]
    fn test_raw_data_ignored_when_no_pending() {
        let mut aggregator = HttpConnectionAggregator::new();

        // Send raw data for a connection that has no pending body
        let raw = SslEvent {
            source: 0,
            timestamp_ns: 1000,
            delta_ns: 0,
            pid: 9999,
            tid: 1,
            uid: 0,
            len: 5,
            rw: 1,
            comm: String::new(),
            buf: b"hello".to_vec(),
            is_handshake: false,
            ssl_ptr: 0x5000,
        };
        let result = aggregator.process_raw_body_data(&raw);
        assert!(result.is_none());
        assert_eq!(aggregator.active_connections(), 0);
    }

    #[test]
    fn test_request_body_pending_with_sse_response() {
        let mut aggregator = HttpConnectionAggregator::new();

        // Request with incomplete body
        let partial = b"POST /stream HTTP/1.1\r\nContent-Length: 50\r\n\r\ndata";
        let event = create_mock_ssl_event_with_buf(1234, 0x6000, partial.to_vec(), 1);

        let header_end = partial.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
        let body_len = partial.len() - header_end;

        let mut headers = HashMap::new();
        headers.insert("content-length".to_string(), "50".to_string());

        let request = ParsedRequest {
            method: "POST".to_string(),
            path: "/stream".to_string(),
            version: 1,
            headers,
            body_offset: header_end,
            body_len,
            source_event: event,
            reassembled_body: None,
        };

        aggregator.process_request(request);

        // SSE response arrives → should force-complete body and enter SseActive
        let resp_event = create_mock_ssl_event_with_buf(
            1234,
            0x6000,
            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n".to_vec(),
            0,
        );
        let mut resp_headers = HashMap::new();
        resp_headers.insert("content-type".to_string(), "text/event-stream".to_string());

        let response = ParsedResponse {
            version: 1,
            status_code: 200,
            reason: "OK".to_string(),
            headers: resp_headers,
            body_offset: 0,
            body_len: 0,
            source_event: resp_event,
        };

        let result = aggregator.process_response(response);
        // SSE response should not return immediately, should enter SseActive
        assert!(result.is_none());
        let conn_id = ConnectionId {
            pid: 1234,
            ssl_ptr: 0x6000,
        };
        assert!(aggregator.is_sse_active(&conn_id));
    }

    #[test]
    fn test_sse_first_chunk_in_initial_response_body_is_preserved() {
        let mut aggregator = HttpConnectionAggregator::new();

        let req_event = create_mock_ssl_event_with_buf(
            4321,
            0x7000,
            b"POST /stream HTTP/1.1\r\nContent-Length: 2\r\n\r\n{}".to_vec(),
            1,
        );
        let mut req_headers = HashMap::new();
        req_headers.insert("content-length".to_string(), "2".to_string());
        let request = ParsedRequest {
            method: "POST".to_string(),
            path: "/stream".to_string(),
            version: 1,
            headers: req_headers,
            body_offset: 43,
            body_len: 2,
            source_event: req_event,
            reassembled_body: None,
        };
        aggregator.process_request(request);

        let resp_buf = b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\ndata: {\"choices\":[{\"delta\":{\"content\":\"3\"}}]}\n\n".to_vec();
        let resp_event = create_mock_ssl_event_with_buf(4321, 0x7000, resp_buf.clone(), 0);
        let response = ParsedResponse {
            version: 1,
            status_code: 200,
            reason: "OK".to_string(),
            headers: {
                let mut h = HashMap::new();
                h.insert("content-type".to_string(), "text/event-stream".to_string());
                h
            },
            body_offset: resp_buf.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4,
            body_len: resp_buf.len()
                - (resp_buf.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4),
            source_event: resp_event,
        };

        let result = aggregator.process_response(response);
        assert!(result.is_none());

        let done_event =
            create_mock_ssl_event_with_buf(4321, 0x7000, b"data: [DONE]\n\n".to_vec(), 0);
        let done = ParsedSseEvent::new(None, None, None, 6, 6, done_event);
        let conn_id = ConnectionId {
            pid: 4321,
            ssl_ptr: 0x7000,
        };
        let result = aggregator.process_sse_event(&conn_id, done);
        let pair = match result {
            Some(AggregatedResult::SseComplete(pair)) => pair,
            other => panic!("expected SseComplete, got {other:?}"),
        };

        assert_eq!(pair.response.sse_event_count(), 2);
        let chunks = pair.response.json_body();
        assert_eq!(chunks.len(), 1);
        assert_eq!(
            chunks[0]["choices"][0]["delta"]["content"].as_str(),
            Some("3")
        );
        assert!(pair.response.parsed.body_str().is_empty());
    }
}
